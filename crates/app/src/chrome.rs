//! App chrome: loading strip, centered placeholder, window title, and the
//! empty-queue mascot.

use crate::theme;
use gpui::component::{spinner::Spinner, tooltip::Tooltip, Sizable as _};
use gpui::{
    canvas, div, fill, point, prelude::*, px, size, Animation, AnimationExt, Bounds, PathBuilder,
    Pixels, Rgba, SharedString, Window,
};
use std::time::Duration;

const LOADING_STRIPE_PITCH: f32 = 52.;
const LOADING_STRIPE_WIDTH: f32 = 26.;
const LOADING_STRIP_HEIGHT: f32 = 1.;

/// One pitch per second, with fixed-size stripes (five at the default 260px).
/// Clipping the edge stripes makes phase 1 identical to phase 0: no loop jump.
fn loading_stripes(width: f32, phase: f32) -> impl Iterator<Item = (f32, f32)> {
    (-1..=(width / LOADING_STRIPE_PITCH).ceil() as i32).filter_map(move |ix| {
        let start = (ix as f32 + phase) * LOADING_STRIPE_PITCH;
        let left = start.max(0.);
        let right = (start + LOADING_STRIPE_WIDTH).min(width);
        (right > left).then_some((left, right - left))
    })
}

pub(crate) fn file_tree_loading_strip(
    message: Option<&'static str>,
    cx: &gpui::App,
) -> gpui::AnyElement {
    // Always reserve the same pixel, including when idle. Loading must
    // never move the filter, tree rows, or the user's current click target.
    div()
        .id("file-tree-loading-track")
        .debug_selector(|| "file-tree-loading-track".into())
        .w_full()
        .h(px(LOADING_STRIP_HEIGHT))
        .flex_shrink_0()
        .when_some(message, |track, message| {
            let color = gpui::Hsla::from(theme::blue(cx));
            track
                .tooltip(move |window, cx| Tooltip::new(message).build(window, cx))
                .bg(color.opacity(0.15))
                .child(
                    div()
                        .debug_selector(|| "file-tree-loading".into())
                        .size_full()
                        .with_animation(
                            "file-tree-loading-stripes",
                            // Linear by default; no easing, pulsing, or stretching.
                            // GPUI also keeps this static under reduced motion.
                            Animation::new(Duration::from_secs(1)).repeat(),
                            move |strip, phase| {
                                strip.child(
                                    canvas(
                                        |_, _, _| (),
                                        move |bounds, _, window, _| {
                                            for (left, width) in
                                                loading_stripes(bounds.size.width.into(), phase)
                                            {
                                                window.paint_quad(fill(
                                                    Bounds::new(
                                                        bounds.origin + point(px(left), px(0.)),
                                                        size(px(width), bounds.size.height),
                                                    ),
                                                    color,
                                                ));
                                            }
                                        },
                                    )
                                    .size_full(),
                                )
                            },
                        ),
                )
        })
        .into_any_element()
}

pub(crate) fn header_loading_spinner(message: &'static str, cx: &gpui::App) -> gpui::AnyElement {
    div()
        .id("header-loading")
        .debug_selector(|| "header-loading".into())
        .flex()
        .items_center()
        .flex_shrink_0()
        .tooltip(move |window, cx| Tooltip::new(message).build(window, cx))
        .child(Spinner::new().xsmall().color(theme::overlay0(cx).into()))
        .into_any_element()
}

pub(crate) fn centered_message(text: SharedString, color: gpui::Rgba) -> gpui::AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(color)
        .child(text)
        .into_any_element()
}

/// The Gander mascot at rest, for a genuinely empty review queue: the app
/// icon's goose from `scripts/generate-icons.swift`, with its watchful dot
/// eye traded for a contented closed one. Drawn as paths rather than a
/// bundled image so it stays crisp at any scale, bobbing gently — the
/// animation only moves paint inside the fixed square, never layout, and
/// GPUI freezes it under reduced motion.
pub(crate) fn happy_goose(text: SharedString, cx: &gpui::App) -> gpui::AnyElement {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .p_8()
        .child(div().size(px(GOOSE_SIZE)).with_animation(
            "happy-goose-bob",
            Animation::new(Duration::from_secs(4)).repeat(),
            |goose, phase| {
                goose.child(canvas(
                    move |_, _, _| (),
                    move |bounds, _, window, _| paint_goose(bounds, phase, window),
                ))
            },
        ))
        .child(div().text_color(theme::overlay0(cx)).child(text))
        .into_any_element()
}

/// The mascot's square canvas, and the design space its geometry lives in.
const GOOSE_SIZE: f32 = 128.;
const GOOSE_DESIGN: f32 = 1024.;
const GOOSE_SCALE: f32 = GOOSE_SIZE / GOOSE_DESIGN;
/// The idle bob's amplitude, in design units so the geometry's margins
/// (which must always clear it) are reasoned about in one space.
const GOOSE_BOB: f32 = 32.;

/// The icon artwork's palette, shared with `scripts/generate-icons.swift`.
const GOOSE_BODY: Rgba = Rgba {
    r: 0.96,
    g: 0.95,
    b: 0.88,
    a: 1.,
};
const GOOSE_WING: Rgba = Rgba {
    r: 0.69,
    g: 0.77,
    b: 0.73,
    a: 1.,
};
const GOOSE_BILL: Rgba = Rgba {
    r: 0.96,
    g: 0.64,
    b: 0.27,
    a: 1.,
};
const GOOSE_EYE: Rgba = Rgba {
    r: 0.06,
    g: 0.17,
    b: 0.20,
    a: 1.,
};

/// The mascot's parts in paint order, in design space with a top-left
/// origin (the icon script's bottom-left coordinates are y-flipped). The
/// wing and eye are the icon's stroked details, redrawn as tapered bands
/// of the same width so every part paints as a plain fill.
fn goose_parts() -> Vec<(PathBuilder, Rgba)> {
    let mut body = PathBuilder::fill();
    body.move_to(point(px(211.), px(664.)));
    body.cubic_bezier_to(
        point(px(563.), px(758.)),
        point(px(265.), px(789.)),
        point(px(453.), px(813.)),
    );
    body.cubic_bezier_to(
        point(px(682.), px(559.)),
        point(px(663.), px(713.)),
        point(px(682.), px(643.)),
    );
    body.line_to(point(px(682.), px(370.)));
    body.cubic_bezier_to(
        point(px(735.), px(340.)),
        point(px(682.), px(345.)),
        point(px(710.), px(340.)),
    );
    body.line_to(point(px(744.), px(306.)));
    body.cubic_bezier_to(
        point(px(651.), px(223.)),
        point(px(746.), px(249.)),
        point(px(705.), px(214.)),
    );
    body.cubic_bezier_to(
        point(px(557.), px(341.)),
        point(px(592.), px(226.)),
        point(px(557.), px(277.)),
    );
    body.line_to(point(px(557.), px(534.)));
    body.cubic_bezier_to(
        point(px(481.), px(604.)),
        point(px(557.), px(581.)),
        point(px(527.), px(604.)),
    );
    body.cubic_bezier_to(
        point(px(211.), px(664.)),
        point(px(377.), px(562.)),
        point(px(296.), px(639.)),
    );
    body.close();

    let mut wing = PathBuilder::fill();
    wing.move_to(point(px(299.), px(682.)));
    wing.cubic_bezier_to(
        point(px(528.), px(687.)),
        point(px(385.), px(720.)),
        point(px(471.), px(727.)),
    );
    wing.line_to(point(px(528.), px(665.)));
    wing.cubic_bezier_to(
        point(px(299.), px(660.)),
        point(px(471.), px(705.)),
        point(px(385.), px(698.)),
    );
    wing.close();

    let mut bill = PathBuilder::fill();
    bill.move_to(point(px(726.), px(284.)));
    bill.line_to(point(px(817.), px(313.)));
    bill.cubic_bezier_to(
        point(px(735.), px(340.)),
        point(px(828.), px(323.)),
        point(px(770.), px(339.)),
    );
    bill.close();

    // The closed, upward-curving arc that makes the goose look pleased
    // about the empty queue rather than merely awake.
    let mut eye = PathBuilder::fill();
    eye.move_to(point(px(660.), px(281.)));
    eye.curve_to(point(px(732.), px(281.)), point(px(696.), px(225.)));
    eye.curve_to(point(px(660.), px(281.)), point(px(696.), px(269.)));
    eye.close();

    vec![
        (body, GOOSE_BODY),
        (wing, GOOSE_WING),
        (bill, GOOSE_BILL),
        (eye, GOOSE_EYE),
    ]
}

/// Paint the mascot fitted to `bounds` — square by construction — riding
/// the bob for the current animation phase.
fn paint_goose(bounds: Bounds<Pixels>, phase: f32, window: &mut Window) {
    let bob = (phase * std::f32::consts::TAU).sin() * GOOSE_BOB * GOOSE_SCALE;
    let origin = bounds.origin + point(px(0.), px(bob));
    for (mut part, color) in goose_parts() {
        part.scale(GOOSE_SCALE);
        part.translate(origin);
        if let Ok(path) = part.build() {
            window.paint_path(path, color);
        }
    }
}

pub(crate) fn app_title(detail: Option<String>, cx: &gpui::App) -> gpui::AnyElement {
    let mut title = div()
        .flex()
        .items_center()
        .gap_2()
        .flex_1()
        .min_w_0()
        .child(
            div()
                .font_weight(gpui::FontWeight::BOLD)
                .child(SharedString::from("Gander")),
        );
    if let Some(detail) = detail {
        title = title.child(
            div()
                .text_color(theme::subtext(cx))
                .truncate()
                .child(SharedString::from(detail)),
        );
    }
    title.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loading_stripes_are_fixed_width_and_move_at_constant_speed() {
        let stripes = loading_stripes(260., 0.).collect::<Vec<_>>();
        assert_eq!(stripes.len(), 5);
        assert!(stripes
            .iter()
            .all(|(_, width)| *width == LOADING_STRIPE_WIDTH));
        for phase in [0.25, 0.5, 0.75] {
            let moved = loading_stripes(260., phase).collect::<Vec<_>>();
            // This interior stripe is never clipped or stretched.
            assert!(moved.contains(&(LOADING_STRIPE_PITCH * (1. + phase), LOADING_STRIPE_WIDTH)));
        }
    }

    #[test]
    fn loading_stripes_loop_seamlessly_and_clip_at_any_pane_width() {
        for width in [1., 180., 260., 375., 600.] {
            assert_eq!(
                loading_stripes(width, 0.).collect::<Vec<_>>(),
                loading_stripes(width, 1.).collect::<Vec<_>>(),
            );
            for phase in [0., 0.25, 0.5, 0.75, 1.] {
                for (left, stripe_width) in loading_stripes(width, phase) {
                    assert!(left >= 0.);
                    assert!(stripe_width > 0. && stripe_width <= LOADING_STRIPE_WIDTH);
                    assert!(left + stripe_width <= width);
                }
            }
        }
    }

    #[test]
    fn goose_parts_tessellate_with_room_for_the_bob() {
        for (part, _) in goose_parts() {
            let bounds = part.build().expect("goose part must tessellate").bounds;
            // Every part has real area, and clears the bob's travel from
            // each edge of the design square, so no phase can clip it.
            assert!(bounds.size.width > px(0.) && bounds.size.height > px(0.));
            assert!(bounds.left() >= px(GOOSE_BOB));
            assert!(bounds.top() >= px(GOOSE_BOB));
            assert!(bounds.right() <= px(GOOSE_DESIGN - GOOSE_BOB));
            assert!(bounds.bottom() <= px(GOOSE_DESIGN - GOOSE_BOB));
        }
    }

    #[test]
    fn goose_stays_inside_its_canvas_at_every_bob_extreme() {
        for phase in [0., 0.25, 0.5, 0.75, 1.] {
            let bob = (phase * std::f32::consts::TAU).sin() * GOOSE_BOB * GOOSE_SCALE;
            for (mut part, _) in goose_parts() {
                part.scale(GOOSE_SCALE);
                part.translate(point(px(0.), px(bob)));
                let bounds = part.build().expect("goose part must tessellate").bounds;
                assert!(bounds.left() >= px(0.));
                assert!(bounds.top() >= px(0.));
                assert!(bounds.right() <= px(GOOSE_SIZE));
                assert!(bounds.bottom() <= px(GOOSE_SIZE));
            }
        }
    }
}
