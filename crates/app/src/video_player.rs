//! AVFoundation playback over local files or the private loopback range cache.
//! The player never sees GitHub credentials or signed CDN URLs.
use anyhow::{ensure, Context as _};
use objc2::{rc::Retained, MainThreadMarker};
use objc2_av_foundation::{
    AVMutableVideoComposition, AVPlayer, AVPlayerItem, AVPlayerItemStatus, AVPlayerItemVideoOutput,
    AVPlayerStatus, AVVideoComposition,
};
use objc2_core_foundation::CFRetained;
use objc2_core_media::CMTime;
use objc2_core_video::{
    kCVPixelFormatType_420YpCbCr8BiPlanarFullRange, CVPixelBuffer, CVPixelBufferGetPixelFormatType,
};
use objc2_foundation::{ns_string, NSDictionary, NSMutableCopying, NSNumber, NSString, NSURL};
use std::{
    path::Path,
    ptr,
    time::{Duration, Instant},
};

pub(crate) struct VideoPlayer {
    player: Retained<AVPlayer>,
    item: Retained<AVPlayerItem>,
    output: Retained<AVPlayerItemVideoOutput>,
    output_attached: bool,
    pub frame: Option<CFRetained<CVPixelBuffer>>,
    composition:
        Option<futures::channel::oneshot::Receiver<Result<Retained<AVVideoComposition>, String>>>,
    // User intent is independent of AVPlayer's rate during seeks/buffering.
    play_requested: bool,
    seek_target: Option<f64>,
    seek_in_flight: Option<Seek>,
}

struct Seek {
    target: f64,
    started: Instant,
    completion: futures::channel::oneshot::Receiver<bool>,
}

impl VideoPlayer {
    pub fn new(path: &Path) -> anyhow::Result<Self> {
        let path = path.to_str().context("Invalid video cache path")?;
        Self::with_url(&NSURL::fileURLWithPath(&NSString::from_str(path)))
    }

    pub fn stream(source: &str) -> anyhow::Result<Self> {
        let parsed = http_client::Url::parse(source)?;
        ensure!(
            parsed.scheme() == "http"
                && parsed.host_str() == Some("127.0.0.1")
                && parsed.username().is_empty()
                && parsed.password().is_none(),
            "Only the local video cache may be streamed"
        );
        let url =
            NSURL::URLWithString(&NSString::from_str(source)).context("Invalid local video URL")?;
        Self::with_url(&url)
    }

    fn with_url(url: &NSURL) -> anyhow::Result<Self> {
        let mtm = MainThreadMarker::new().context("Video playback requires the main thread")?;
        // SAFETY: these objects are used only on the main thread. All inputs
        // are owned Foundation values; the file remains pinned by the session.
        unsafe {
            let item = AVPlayerItem::playerItemWithURL(url, mtm);
            item.setPreferredForwardBufferDuration(1.0);
            item.setSeekingWaitsForVideoCompositionRendering(true);
            let surface_properties = NSDictionary::<NSString, objc2::runtime::AnyObject>::new();
            let attributes = NSDictionary::from_slices(
                &[
                    ns_string!("PixelFormatType"),
                    ns_string!("MetalCompatibility"),
                    ns_string!("IOSurfaceProperties"),
                ],
                &[
                    &*NSNumber::new_u32(kCVPixelFormatType_420YpCbCr8BiPlanarFullRange)
                        as &objc2::runtime::AnyObject,
                    &*NSNumber::new_bool(true) as &objc2::runtime::AnyObject,
                    &*surface_properties as &objc2::runtime::AnyObject,
                ],
            );
            let output = AVPlayerItemVideoOutput::initWithPixelBufferAttributes(
                mtm.alloc(),
                Some(&attributes),
            );
            // Asynchronous even for local files: the synchronous composition
            // API can block indefinitely on unsupported containers. Applying
            // the composition honours a phone recording's preferred transform.
            let (tx, rx) = futures::channel::oneshot::channel();
            let tx = std::sync::Mutex::new(Some(tx));
            let callback = block2::RcBlock::new(
                move |composition: *mut AVVideoComposition,
                      _error: *mut objc2_foundation::NSError| {
                    let result = Retained::retain(composition)
                        .ok_or_else(|| "Unsupported video format".to_owned());
                    if let Some(tx) = tx.lock().unwrap().take() {
                        let _ = tx.send(result);
                    }
                },
            );
            AVVideoComposition::videoCompositionWithPropertiesOfAsset_completionHandler(
                &item.asset(),
                &callback,
            );
            let player = AVPlayer::playerWithPlayerItem(Some(&item), mtm);
            // With this disabled a buffer underrun changes rate to zero and
            // leaves playback paused indefinitely.
            player.setAutomaticallyWaitsToMinimizeStalling(true);
            Ok(Self {
                player,
                item,
                output,
                output_attached: false,
                frame: None,
                composition: Some(rx),
                play_requested: false,
                seek_target: None,
                seek_in_flight: None,
            })
        }
    }

    pub fn tick(&mut self) -> anyhow::Result<()> {
        unsafe {
            if let Some(rx) = &mut self.composition {
                if let Some(result) = rx.try_recv()? {
                    let composition = result.map_err(anyhow::Error::msg)?;
                    let composition: Retained<AVMutableVideoComposition> =
                        composition.mutableCopy();
                    // GPUI's surface shader has a fixed full-range BT.601
                    // YCbCr conversion. Request that matrix rather than passing
                    // HD BT.709 YCbCr through the wrong conversion coefficients.
                    composition.setColorPrimaries(Some(ns_string!("ITU_R_709_2")));
                    composition.setColorTransferFunction(Some(ns_string!("ITU_R_709_2")));
                    composition.setColorYCbCrMatrix(Some(ns_string!("ITU_R_601_4")));
                    self.item.setVideoComposition(Some(&composition));
                    self.item.addOutput(&self.output);
                    self.output_attached = true;
                    self.composition = None;
                    if self.seek_target.is_none() {
                        self.seek(0.);
                    }
                }
            }
            ensure!(
                self.player.status() != AVPlayerStatus::Failed,
                "Video playback failed"
            );
            ensure!(
                self.item.status() != AVPlayerItemStatus::Failed,
                "Unsupported format or playback failure{}",
                self.item
                    .error()
                    .map(|e| format!(": {}", e.localizedDescription()))
                    .unwrap_or_default()
            );
            if !self.ready() {
                return Ok(());
            }
            self.finish_seek()?;
            let time = self.player.currentTime();
            if self.output.hasNewPixelBufferForItemTime(time) {
                if let Some(frame) = self
                    .output
                    .copyPixelBufferForItemTime_itemTimeForDisplay(time, ptr::null_mut())
                {
                    // Never allow an unexpected format to reach GPUI's assert.
                    ensure!(
                        CVPixelBufferGetPixelFormatType(&frame)
                            == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
                        "Unsupported decoded pixel format"
                    );
                    if let Some(matrix) = frame.attachment(
                        objc2_core_video::kCVImageBufferYCbCrMatrixKey,
                        ptr::null_mut(),
                    ) {
                        ensure!(
                            matrix
                                .downcast::<objc2_core_foundation::CFString>()
                                .is_ok_and(|matrix| &*matrix
                                    == objc2_core_video::kCVImageBufferYCbCrMatrix_ITU_R_601_4),
                            "Unsupported decoded color matrix"
                        );
                    }
                    self.frame = Some(frame.into());
                }
            }
            // Read the completed seek's frame before chasing the next target.
            self.start_seek();
            if !self.seeking() && self.play_requested && self.player.rate() == 0. {
                if self.duration() > 0. && self.position() >= self.duration() - 0.001 {
                    self.play_requested = false;
                } else {
                    self.player.play();
                }
            }
        }
        Ok(())
    }

    fn finish_seek(&mut self) -> anyhow::Result<()> {
        if let Some(seek) = &mut self.seek_in_flight {
            if let Some(finished) = seek.completion.try_recv()? {
                if finished && self.seek_target == Some(seek.target) {
                    self.seek_target = None;
                }
                self.seek_in_flight = None;
            } else if self.seek_target != Some(seek.target)
                && seek.started.elapsed() >= Duration::from_millis(100)
            {
                // A seek waiting on undownloaded bytes must not prevent a new
                // seek back into cached data. Normal scrubbing lets each seek
                // finish rather than repeatedly cancelling the decoder's work.
                unsafe { self.item.cancelPendingSeeks() };
                self.seek_in_flight = None;
            }
        }
        Ok(())
    }

    fn start_seek(&mut self) {
        if self.seek_in_flight.is_some() {
            return;
        }
        let Some(target) = self.seek_target else {
            return;
        };
        let (tx, rx) = futures::channel::oneshot::channel();
        let tx = std::sync::Mutex::new(Some(tx));
        let callback = block2::RcBlock::new(move |finished: objc2::runtime::Bool| {
            if let Some(tx) = tx.lock().unwrap().take() {
                let _ = tx.send(finished.as_bool());
            }
        });
        unsafe {
            self.player.pause();
            self.player
                .seekToTime_toleranceBefore_toleranceAfter_completionHandler(
                    CMTime::with_seconds(target, 600),
                    objc2_core_media::kCMTimeZero,
                    objc2_core_media::kCMTimeZero,
                    &callback,
                );
        }
        self.seek_in_flight = Some(Seek {
            target,
            started: Instant::now(),
            completion: rx,
        });
    }

    pub fn seeking(&self) -> bool {
        self.seek_target.is_some() || self.seek_in_flight.is_some()
    }

    pub fn ready(&self) -> bool {
        self.composition.is_none()
            && unsafe { self.item.status() == AVPlayerItemStatus::ReadyToPlay }
    }
    pub fn playing(&self) -> bool {
        self.play_requested
    }
    pub fn position(&self) -> f64 {
        self.seek_target
            .unwrap_or_else(|| seconds(unsafe { self.player.currentTime().seconds() }))
    }
    pub fn duration(&self) -> f64 {
        seconds(unsafe { self.item.duration().seconds() })
    }
    pub fn pause(&mut self) {
        self.play_requested = false;
        unsafe { self.player.pause() }
    }
    pub fn play(&mut self) {
        self.play_requested = true;
        if !self.ready() {
            return;
        }
        if !self.seeking() && self.duration() > 0. && self.position() >= self.duration() - 0.05 {
            self.seek(0.);
        }
        if !self.seeking() {
            unsafe { self.player.play() }
        }
    }
    pub fn seek(&mut self, seconds: f64) {
        if seconds.is_finite() {
            self.seek_target = Some(seconds.clamp(0., self.duration()));
            if self.ready() {
                self.start_seek();
            }
        }
    }
    pub fn muted(&self) -> bool {
        unsafe { self.player.isMuted() }
    }
    pub fn set_muted(&self, muted: bool) {
        unsafe { self.player.setMuted(muted) }
    }
    pub fn volume(&self) -> f32 {
        unsafe { self.player.volume() }
    }
    pub fn set_volume(&self, volume: f32) {
        unsafe { self.player.setVolume(volume.clamp(0., 1.)) }
    }
}

fn seconds(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.)
    } else {
        0.
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        unsafe {
            self.player.pause();
            self.item.asset().cancelLoading();
            if self.output_attached {
                self.item.removeOutput(&self.output);
            }
            self.player.replaceCurrentItemWithPlayerItem(None);
        }
    }
}
