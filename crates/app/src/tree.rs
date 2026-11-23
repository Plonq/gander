//! The sidebar file tree: paths flattened depth-first, collapse/visibility, and row rendering.

use std::collections::HashSet;

use crate::{theme, threads::CommentSide, ItemData, ReviewApp};
use diff_core::{DiffRow, FileDiff, FileStatus};
use fuzzy_matcher::{skim::SkimMatcherV2, FuzzyMatcher};
use gpui::component::{checkbox::Checkbox, Disableable as _, Icon, Sizable as _};
use gpui::component::{
    list::ListItem,
    tree::{TreeEvent, TreeItem, TreeState},
};
use gpui::{div, prelude::*, px, Hsla, KeyBinding, ScrollStrategy, SharedString};
use gpui_kit_assets::IconName;

gpui::actions!(file_filter, [HighlightNextFile, HighlightPreviousFile]);

pub(crate) fn keybindings() -> [KeyBinding; 2] {
    [
        KeyBinding::new("down", HighlightNextFile, Some("FileFilter > Input")),
        KeyBinding::new("up", HighlightPreviousFile, Some("FileFilter > Input")),
    ]
}

/// Row height of sidebar file-tree entries.
const TREE_ROW_HEIGHT: f32 = 24.0;

/// One row of the file tree, flattened depth-first for a uniform_list.
/// Directories precede files at each level; both are name-sorted.
#[derive(Debug, PartialEq)]
pub(crate) struct TreeEntry {
    pub(crate) depth: usize,
    /// Display name: a file name, or a compressed single-child directory
    /// chain ("src/core/flags").
    pub(crate) name: SharedString,
    pub(crate) kind: TreeEntryKind,
}

#[derive(Debug, PartialEq)]
pub(crate) enum TreeEntryKind {
    /// Full path of the chain's deepest directory — the collapse-state key.
    Dir { path: String },
    /// Index into `PrDiff::files` (and thus `ItemData::file_rows`).
    File { file_ix: usize },
}

/// Group file paths (diff order) into a depth-first tree. Directory chains
/// with a single child directory and no files of their own compress into one
/// entry, GitHub-style.
pub(crate) fn build_tree(paths: &[&str]) -> Vec<TreeEntry> {
    #[derive(Default)]
    struct DirNode {
        dirs: std::collections::BTreeMap<String, DirNode>,
        files: Vec<(String, usize)>,
    }
    let mut root = DirNode::default();
    for (file_ix, path) in paths.iter().enumerate() {
        let (dirs, name) = match path.rsplit_once('/') {
            Some((dirs, name)) => (Some(dirs), name),
            None => (None, *path),
        };
        let mut node = &mut root;
        for part in dirs.into_iter().flat_map(|dirs| dirs.split('/')) {
            node = node.dirs.entry(part.to_string()).or_default();
        }
        node.files.push((name.to_string(), file_ix));
    }
    fn flatten(node: DirNode, prefix: &str, depth: usize, out: &mut Vec<TreeEntry>) {
        for (name, mut child) in node.dirs {
            let mut label = name;
            let mut path = if prefix.is_empty() {
                label.clone()
            } else {
                format!("{prefix}/{label}")
            };
            while child.files.is_empty() && child.dirs.len() == 1 {
                let (next_name, next) = child.dirs.into_iter().next().unwrap();
                label.push('/');
                label.push_str(&next_name);
                path.push('/');
                path.push_str(&next_name);
                child = next;
            }
            out.push(TreeEntry {
                depth,
                name: label.into(),
                kind: TreeEntryKind::Dir { path: path.clone() },
            });
            flatten(child, &path, depth + 1, out);
        }
        let mut files = node.files;
        files.sort();
        for (name, file_ix) in files {
            out.push(TreeEntry {
                depth,
                name: name.into(),
                kind: TreeEntryKind::File { file_ix },
            });
        }
    }
    let mut out = Vec::new();
    flatten(root, "", 0, &mut out);
    out
}

/// File indices whose full path fuzzy-matches `query`, best score first.
/// An empty query keeps every file in diff order.
pub(crate) fn fuzzy_file_matches(paths: &[&str], query: &str) -> Vec<usize> {
    let query = query.trim();
    if query.is_empty() {
        return (0..paths.len()).collect();
    }
    let matcher = SkimMatcherV2::default();
    let mut scored: Vec<(i64, usize)> = paths
        .iter()
        .enumerate()
        .filter_map(|(ix, path)| matcher.fuzzy_match(path, query).map(|score| (score, ix)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, ix)| ix).collect()
}

/// Status label for file headers and color shared with tree entries.
pub(crate) fn status_style(status: FileStatus, cx: &gpui::App) -> (&'static str, gpui::Rgba) {
    match status {
        FileStatus::Added => ("added", theme::green(cx)),
        FileStatus::Deleted => ("deleted", theme::red(cx)),
        FileStatus::Modified => ("modified", theme::blue(cx)),
        FileStatus::Renamed => ("renamed", theme::mauve(cx)),
        FileStatus::Binary => ("binary", theme::peach(cx)),
    }
}

/// A sidebar row's topology and current stock-tree expansion state.
#[derive(Clone, Copy)]
pub(crate) struct TreeListRow {
    pub entry_ix: usize,
    pub expanded: bool,
}

pub(crate) fn file_id(path: &str) -> SharedString {
    format!("file:{path}").into()
}
fn directory_id(path: &str) -> SharedString {
    format!("directory:{path}").into()
}

/// Adapts our compressed hierarchy to stable stock identities, pruning files
/// that do not match and directories with no matching descendants. Filtering
/// preserves the original topology and order, and reveals matching subtrees.
/// Visible positions are never identities.
fn kit_items(
    entries: &[TreeEntry],
    paths: &[&str],
    collapsed: &HashSet<String>,
    query: &str,
) -> Vec<TreeItem> {
    let matches: Option<HashSet<usize>> =
        (!query.trim().is_empty()).then(|| fuzzy_file_matches(paths, query).into_iter().collect());
    fn level(
        entries: &[TreeEntry],
        paths: &[&str],
        collapsed: &HashSet<String>,
        matches: Option<&HashSet<usize>>,
        at: &mut usize,
        depth: usize,
    ) -> Vec<TreeItem> {
        let mut items = Vec::new();
        while let Some(entry) = entries.get(*at).filter(|entry| entry.depth == depth) {
            *at += 1;
            let item = match &entry.kind {
                TreeEntryKind::File { file_ix } => {
                    if matches.is_some_and(|matches| !matches.contains(file_ix)) {
                        continue;
                    }
                    TreeItem::new(file_id(paths[*file_ix]), entry.name.clone())
                }
                TreeEntryKind::Dir { path } => {
                    let children = level(entries, paths, collapsed, matches, at, depth + 1);
                    if children.is_empty() {
                        continue;
                    }
                    TreeItem::new(directory_id(path), entry.name.clone())
                        .expanded(matches.is_some() || !collapsed.contains(path))
                        .children(children)
                }
            };
            items.push(item);
        }
        items
    }
    level(entries, paths, collapsed, matches.as_ref(), &mut 0, 0)
}

pub(crate) struct FileTree {
    pub state: gpui::Entity<TreeState>,
    paths: Vec<String>,
    query: String,
    selection: Option<SharedString>,
    _subscription: gpui::Subscription,
}

impl FileTree {
    pub fn new(item_id: u64, cx: &mut gpui::Context<ReviewApp>) -> Self {
        let state = cx.new(|cx| TreeState::new(cx));
        let subscription = cx.subscribe(&state, move |app, _, event: &TreeEvent, cx| {
            let Some(crate::ItemState::Ready(data)) = app
                .items
                .iter_mut()
                .find(|item| item.id == item_id)
                .map(|item| &mut item.state)
            else {
                return;
            };
            // Filtered trees start expanded. Let the stock tree handle toggles,
            // but do not overwrite the unfiltered tree's collapse preferences.
            if data
                .tree_view
                .as_ref()
                .is_some_and(|view| !view.query.is_empty())
            {
                cx.notify();
                return;
            }
            match event {
                TreeEvent::Expanded(id) => {
                    if let Some(path) = id.strip_prefix("directory:") {
                        data.collapsed.remove(path);
                    }
                }
                TreeEvent::Collapsed(id) => {
                    if let Some(path) = id.strip_prefix("directory:") {
                        data.collapsed.insert(path.into());
                    }
                }
            }
            cx.notify();
        });
        Self {
            state,
            paths: Vec::new(),
            query: String::new(),
            selection: None,
            _subscription: subscription,
        }
    }

    pub fn reconcile(
        &mut self,
        entries: &[TreeEntry],
        paths: &[&str],
        collapsed: &HashSet<String>,
        query: &str,
        cx: &mut gpui::App,
    ) {
        if self
            .paths
            .iter()
            .map(String::as_str)
            .eq(paths.iter().copied())
            && self.query == query
        {
            return;
        }
        if let Some(selected) = self.state.read(cx).selected_item() {
            self.selection = Some(selected.id.clone());
        }
        let reset_highlight = self.query != query && !query.is_empty();
        self.paths = paths.iter().map(|p| p.to_string()).collect();
        self.query = query.into();
        let items = kit_items(entries, paths, collapsed, query);
        self.state.update(cx, |state, cx| {
            state.set_items(items, cx);
            // Filtering starts on the first visible file, not the highest fuzzy
            // score. This is the stock tree highlight, separate from the diff.
            let ix = if reset_highlight {
                (0..)
                    .map_while(|ix| state.entry(ix).map(|entry| (ix, entry)))
                    .find(|(_, entry)| entry.item().id.starts_with("file:"))
                    .map(|(ix, _)| ix)
            } else {
                self.selection.as_ref().and_then(|id| state.index_of(id))
            };
            // Do not use set_selected_item/reveal_item: they expand ancestors.
            state.set_selected_index(ix, cx);
            if reset_highlight {
                state.scroll_to_item(0, ScrollStrategy::Top);
            }
        });
    }

    /// Move only the stock highlight; opening files and focus are unchanged.
    /// Directory headings (and descendants hidden by a collapse) are skipped.
    pub fn highlight_file(&self, delta: i32, cx: &mut gpui::App) {
        self.state.update(cx, |state, cx| {
            let files: Vec<_> = (0..)
                .map_while(|ix| state.entry(ix).map(|entry| (ix, entry)))
                .filter(|(_, entry)| entry.item().id.starts_with("file:"))
                .map(|(ix, _)| ix)
                .collect();
            let ix = match state.selected_index() {
                Some(current) if delta > 0 => files
                    .iter()
                    .copied()
                    .find(|&ix| ix > current)
                    .or_else(|| files.last().copied()),
                Some(current) => files
                    .iter()
                    .copied()
                    .rev()
                    .find(|&ix| ix < current)
                    .or_else(|| files.first().copied()),
                None => files.first().copied(),
            };
            state.set_selected_index(ix, cx);
            if let Some(ix) = ix {
                state.scroll_to_item(ix, ScrollStrategy::Nearest);
            }
        });
    }
}

/// Count visible threads, excluding replies regardless of disclosure state.
/// Hidden and unrenderable threads contribute none.
fn file_thread_count(data: &ItemData, file_ix: usize) -> usize {
    if !data.comments_visible {
        return 0;
    }
    let file = &data.diff.files[file_ix];
    let Some(anchors) = data
        .comments
        .as_ref()
        .and_then(|index| index.threads.get(file.display_path()))
    else {
        return 0;
    };
    let upgrade = data.upgrades.get(&file_ix);
    if data.shows_file_media_for(file_ix)
        || (crate::presentation::auto_collapse(crate::presentation::file_lines(file, upgrade))
            && !data.expanded_regions.contains(&(file_ix, 0)))
    {
        return 0;
    }
    anchors
        .iter()
        .filter(|((side, line), _)| {
            // Whitespace filtering realigns rows but preserves source line
            // numbers, and context folding leaves comment anchors visible.
            file.hunks.iter().flat_map(|hunk| &hunk.rows).any(|row| {
                let no = match (side, row) {
                    (
                        CommentSide::Left,
                        DiffRow::Context { old_no, .. } | DiffRow::Removed { old_no, .. },
                    ) => Some(*old_no),
                    (
                        CommentSide::Right,
                        DiffRow::Context { new_no, .. } | DiffRow::Added { new_no, .. },
                    ) => Some(*new_no),
                    _ => None,
                };
                no.is_some_and(|no| u64::from(no) == *line)
            }) || upgrade.is_some_and(|upgrade| {
                (0..=file.hunks.len()).any(|gap| {
                    let (old, new, count) =
                        crate::rows::gap_span(&file.hunks, gap, upgrade.new_lines.len() as u32);
                    let start = u64::from(if *side == CommentSide::Left { old } else { new });
                    *line >= start && *line < start + u64::from(count)
                })
            })
        })
        .flat_map(|(_, threads)| threads)
        .count()
}

/// Kit paints mouse selection; `current` is a separate narrow viewport marker.
/// Kit toggles directories; leaf clicks open the file in the diff.
pub(crate) fn render_tree_row(
    row: TreeListRow,
    pos: usize,
    current: bool,
    composing: bool,
    data: &ItemData,
    entity: &gpui::Entity<ReviewApp>,
    reloading: bool,
    cx: &gpui::App,
) -> ListItem {
    let comments = |file_ix: usize| {
        let count = file_thread_count(data, file_ix);
        (count > 0).then(|| {
            div()
                .debug_selector(move || format!("file-comments-{file_ix}"))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_1()
                .text_size(px(10.))
                .text_color(theme::subtext(cx))
                .child(
                    Icon::default()
                        .path("icons/gander/comment-16.svg")
                        .xsmall()
                        .text_color(theme::overlay0(cx)),
                )
                .child(SharedString::from(count.to_string()))
        })
    };
    let checkbox = |file_ix: usize| {
        let path = data.diff.files[file_ix].display_path();
        let state = data.viewed.state(path);
        // GitHub does not track viewed state for every file in every diff
        // (e.g. changes since the last review that were then reverted), so
        // those files can never be marked viewed. Hide the checkbox rather
        // than showing a disabled one that is indistinguishable from the
        // transient disabling during gh activity.
        let state = state?;
        let entity = entity.clone();
        Some(
            div()
                .id(("viewed-control", file_ix))
                .debug_selector(move || format!("viewed-{file_ix}"))
                .flex_shrink_0()
                .flex()
                .items_center()
                // A checkbox click must never also open its file, including
                // when disabled during a mutation or refresh.
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(
                    Checkbox::new(("file-viewed", file_ix))
                        .xsmall()
                        // Enlarge the control's hitbox, not its visual indicator.
                        .size(px(TREE_ROW_HEIGHT))
                        .items_center()
                        .justify_center()
                        .checked(state == github::FileViewedState::Viewed)
                        .disabled(
                            reloading
                                || data.viewed.pending.contains(path)
                                || data.viewed.uncertain.contains(path),
                        )
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            entity.update(cx, |this, cx| {
                                window.focus(&this.focus_handle, cx);
                                this.toggle_file_viewed(file_ix, cx);
                            });
                        }),
                )
                .into_any_element(),
        )
    };
    let stats = |file: &FileDiff| {
        div()
            .flex()
            .items_center()
            .gap_1()
            .flex_shrink_0()
            .text_size(px(10.))
            .child(
                div()
                    .text_color(Hsla::from(theme::green(cx)).opacity(0.7))
                    .child(SharedString::from(format!("+{}", file.additions))),
            )
            .child(
                div()
                    .text_color(Hsla::from(theme::red(cx)).opacity(0.7))
                    .child(SharedString::from(format!("−{}", file.deletions))),
            )
    };
    let entity = entity.clone();
    let base = div()
        .id(("tree-row", pos))
        .debug_selector(move || format!("tree-row-{pos}"))
        .h(px(TREE_ROW_HEIGHT))
        .w_full()
        .flex()
        .items_center()
        .gap_1()
        // Leave room for the stock tree's overlay scrollbar, even on hover.
        .pr_5()
        .cursor_pointer()
        .child(
            div()
                .w(px(3.))
                .h(px(14.))
                .flex_shrink_0()
                .when(current, |marker| marker.bg(theme::blue(cx)))
                .debug_selector(move || format!("current-file-{pos}")),
        );
    let content = {
        let entry = &data.tree[row.entry_ix];
        let indent = px(8. + entry.depth as f32 * 12.);
        let base = base.pl(indent);
        match &entry.kind {
            TreeEntryKind::Dir { .. } => {
                let chevron = if row.expanded { "▾" } else { "▸" };
                base.child(
                    div()
                        .w(px(12.))
                        .flex_shrink_0()
                        .text_color(theme::overlay0(cx))
                        .child(SharedString::from(chevron)),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(theme::overlay0(cx))
                        .child(entry.name.clone()),
                )
                .into_any_element()
            }
            TreeEntryKind::File { file_ix } => {
                let file = &data.diff.files[*file_ix];
                let (_, status_color) = status_style(file.status, cx);
                let file_ix = *file_ix;
                base.on_click(move |_, window, cx| {
                    entity.update(cx, |this, cx| this.jump_to_file(file_ix, window, cx));
                })
                .child(div().w(px(12.)).flex_shrink_0()) // aligns with dir chevrons
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(status_color)
                        .child(entry.name.clone()),
                )
                .when(composing, |row| {
                    row.child(
                        div()
                            .debug_selector(move || format!("file-composer-{file_ix}"))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .child(
                                Icon::new(IconName::Pencil)
                                    .xsmall()
                                    .text_color(theme::overlay0(cx)),
                            ),
                    )
                })
                .children(comments(file_ix))
                .child(stats(file))
                .when(data.pr_meta.is_some(), |row| {
                    row.when_some(checkbox(file_ix), |row, checkbox| row.child(checkbox))
                })
                .into_any_element()
            }
        }
    };
    ListItem::new(("file-tree-row", pos))
        .h(px(TREE_ROW_HEIGHT))
        .p_0()
        // ListItem's text_base overrides the sidebar's inherited 12px size.
        .text_size(px(12.))
        .child(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::rows::{build_rows, Row, RowOptions, RowsView, ViewMode};
    use crate::threads::{group_comments, CommentIndex, CommentTime, ReviewComment};

    fn comment_data(comments: Option<CommentIndex>) -> ItemData {
        let diff = crate::test_util::sample_diff();
        let built = build_rows(RowsView {
            diff: &diff,
            upgrades: &Default::default(),
            comments: comments.as_ref(),
            options: RowOptions {
                mode: ViewMode::Unified,
                file_ix: 0,
                comments_visible: true,
                comment_wrap: 80,
            },
        });
        ItemData::new(None, diff, ViewMode::Unified, 0, built, comments, None)
    }

    fn assert_visible_count(data: &mut ItemData, expected: usize) {
        // Check the badge even while a different file is open.
        data.file_ix = 1;
        data.set_rows(data.row_build());
        assert_eq!(file_thread_count(data, 0), expected);
        data.file_ix = 0;
        data.set_rows(data.row_build());
        assert_eq!(file_thread_count(data, 0), expected);
        assert_eq!(
            data.rows
                .iter()
                .filter(|row| matches!(row, Row::CommentHeader { top: true, .. }))
                .count(),
            expected,
        );
    }

    #[test]
    fn file_thread_counts_use_display_path() {
        let mut index = crate::test_util::sample_comments();
        let mut renamed = crate::test_util::sample_comments()
            .threads
            .remove("a.rs")
            .unwrap();
        renamed.retain(|(side, _), _| *side == CommentSide::Right);
        index.threads.insert("renamed.rs".into(), renamed);
        let mut data = comment_data(Some(index));
        assert_visible_count(&mut data, 2);

        data.diff.files[0].status = FileStatus::Renamed;
        data.diff.files[0].new_path = Some("renamed.rs".into());
        assert_visible_count(&mut data, 1);

        data.diff.files[0].status = FileStatus::Deleted;
        data.diff.files[0].new_path = None;
        assert_visible_count(&mut data, 2);
        data.comments = None;
        assert_visible_count(&mut data, 0);
    }

    #[test]
    fn file_thread_counts_exclude_replies_and_unrenderable_threads() {
        let comment = |id: u64, line, parent: Option<u64>| ReviewComment {
            id: id.into(),
            path: "a.rs".into(),
            line,
            side: CommentSide::Right,
            start_line: None,
            body: "comment".into(),
            author: "alice".into(),
            time: CommentTime::Draft,
            in_reply_to_id: parent.map(Into::into),
        };
        let index = group_comments(vec![
            comment(1, None, None),
            comment(2, None, Some(1)),
            comment(3, Some(2), None),
            comment(4, None, Some(3)),
            // Current anchors can still be outside the displayed file contents.
            comment(5, Some(99), None),
            comment(6, None, Some(5)),
            // A full-file upgrade can make gap anchors visible.
            comment(7, Some(6), None),
        ]);
        assert_eq!(index.counts["a.rs"], (5, 2));
        let mut data = comment_data(Some(index));
        for mode in [ViewMode::Unified, ViewMode::Split] {
            data.mode = mode;
            assert_visible_count(&mut data, 1);
            data.upgrades.insert(
                0,
                crate::rows::FileUpgrade {
                    new_lines: (0..12).map(|_| "line".into()).collect(),
                    old_spans: Vec::new(),
                    new_spans: Vec::new(),
                },
            );
            assert_visible_count(&mut data, 2);
            data.preferences.collapse_unchanged = Some(true);
            assert_visible_count(&mut data, 2);
            data.upgrades.clear();
            data.comments_visible = false;
            assert_visible_count(&mut data, 0);
            data.comments_visible = true;
        }
        data.upgrades.insert(
            0,
            crate::rows::FileUpgrade {
                new_lines: vec!["line".into(); crate::presentation::AUTO_COLLAPSE_LINES],
                old_spans: Vec::new(),
                new_spans: Vec::new(),
            },
        );
        assert_visible_count(&mut data, 0);
        data.expanded_regions.insert((0, 0));
        assert_visible_count(&mut data, 3);
    }

    #[test]
    fn file_thread_counts_are_unchanged_by_disclosure_resolution_and_editing() {
        let remote = vec![
            crate::test_util::rc(
                1,
                "a.rs",
                Some("RIGHT"),
                Some(2),
                "root",
                "alice",
                "a",
                None,
            ),
            crate::test_util::rc(
                2,
                "a.rs",
                Some("RIGHT"),
                Some(2),
                "reply",
                "bob",
                "b",
                Some(1),
            ),
            crate::test_util::rc(3, "a.rs", Some("LEFT"), Some(2), "open", "alice", "c", None),
        ];
        let mut discussion = github::PrDiscussion::from_review_comments(remote.clone());
        discussion.threads[0].is_resolved = true;
        let mut data = comment_data(Some(group_comments(
            remote.into_iter().map(Into::into).collect(),
        )));
        data.discussion = Some(discussion);
        for mode in [ViewMode::Unified, ViewMode::Split] {
            data.mode = mode;
            assert_visible_count(&mut data, 2);
            data.expanded_threads.insert("1".into());
            assert_visible_count(&mut data, 2);
            data.expanded_threads.clear();
            assert_visible_count(&mut data, 2);
            data.editing_comments
                .insert(2, std::rc::Rc::new(std::cell::Cell::new(px(40.))));
            assert_visible_count(&mut data, 2);
            data.editing_comments.clear();
            assert_visible_count(&mut data, 2);
            data.discussion.as_mut().unwrap().threads[0].is_resolved = false;
            assert_visible_count(&mut data, 2);
            data.discussion.as_mut().unwrap().threads[0].is_resolved = true;
        }
    }

    #[test]
    fn file_thread_counts_exclude_media_previews() {
        let mut index = crate::test_util::sample_comments();
        let anchors = index.threads.remove("a.rs").unwrap();
        index.threads.insert("a.svg".into(), anchors);
        let mut data = comment_data(Some(index));
        data.diff.files[0].old_path = Some("a.svg".into());
        data.diff.files[0].new_path = Some("a.svg".into());
        assert_eq!(file_thread_count(&data, 0), 0);
        data.svg_code.insert("a.svg".into());
        assert_visible_count(&mut data, 2);
        data.svg_code.clear();
        assert_eq!(file_thread_count(&data, 0), 0);
        data.svg_code.insert("a.svg".into());
        data.diff.files[0].status = FileStatus::Binary;
        assert_visible_count(&mut data, 0);
    }

    /// (depth, display name, Some(file_ix) for files / None for dirs).
    fn flat(entries: &[TreeEntry]) -> Vec<(usize, &str, Option<usize>)> {
        entries
            .iter()
            .map(|e| {
                let file_ix = match &e.kind {
                    TreeEntryKind::Dir { .. } => None,
                    TreeEntryKind::File { file_ix } => Some(*file_ix),
                };
                (e.depth, e.name.as_ref(), file_ix)
            })
            .collect()
    }

    fn dir_paths(entries: &[TreeEntry]) -> Vec<&str> {
        entries
            .iter()
            .filter_map(|e| match &e.kind {
                TreeEntryKind::Dir { path } => Some(path.as_str()),
                TreeEntryKind::File { .. } => None,
            })
            .collect()
    }

    #[test]
    fn tree_nests_dirs_first_and_sorts_names() {
        let tree = build_tree(&["src/main.rs", "README.md", "src/lib.rs"]);
        assert_eq!(
            flat(&tree),
            vec![
                (0, "src", None),
                (1, "lib.rs", Some(2)),
                (1, "main.rs", Some(0)),
                (0, "README.md", Some(1)),
            ]
        );
        // No directories at all: a flat, sorted file list.
        let tree = build_tree(&["b.txt", "a.txt"]);
        assert_eq!(
            flat(&tree),
            vec![(0, "a.txt", Some(1)), (0, "b.txt", Some(0))]
        );
    }

    #[test]
    fn tree_compresses_single_child_dir_chains() {
        // core→flags has one child dir and no files: compressed. src has a
        // file of its own, so it is not folded into the chain.
        let tree = build_tree(&[
            "src/core/flags/defs.rs",
            "src/core/flags/parse.rs",
            "src/main.rs",
        ]);
        assert_eq!(
            flat(&tree),
            vec![
                (0, "src", None),
                (1, "core/flags", None),
                (2, "defs.rs", Some(0)),
                (2, "parse.rs", Some(1)),
                (1, "main.rs", Some(2)),
            ]
        );
        // The collapse key is the chain's full path.
        assert_eq!(dir_paths(&tree), vec!["src", "src/core/flags"]);

        // A chain starting at the root compresses too.
        let tree = build_tree(&["a/b/c.txt"]);
        assert_eq!(flat(&tree), vec![(0, "a/b", None), (1, "c.txt", Some(0))]);
        assert_eq!(dir_paths(&tree), vec!["a/b"]);

        // A dir with its own files stops the chain even with one child dir.
        let tree = build_tree(&["a/f.txt", "a/b/g.txt"]);
        assert_eq!(
            flat(&tree),
            vec![
                (0, "a", None),
                (1, "b", None),
                (2, "g.txt", Some(1)),
                (1, "f.txt", Some(0)),
            ]
        );
    }

    #[gpui::test]
    fn collapse_hides_subtrees(cx: &mut gpui::TestAppContext) {
        let paths = [
            "src/core/flags/defs.rs",
            "src/core/flags/parse.rs",
            "src/main.rs",
            "README.md",
        ];
        let tree = build_tree(&paths);
        cx.update(|cx| {
            for (collapsed, count) in [
                (HashSet::new(), 6),
                (HashSet::from(["src/core/flags".into()]), 4),
                (HashSet::from(["src".into(), "src/core/flags".into()]), 2),
            ] {
                let state = TreeState::new(cx).items(kit_items(&tree, &paths, &collapsed, ""));
                assert!(state.entry(count - 1).is_some());
                assert!(state.entry(count).is_none());
            }
            let collapsed = HashSet::from(["src".into(), "src/core/flags".into()]);
            let filtered = TreeState::new(cx).items(kit_items(&tree, &paths, &collapsed, "defs"));
            assert_eq!(filtered.entry(0).unwrap().item().id, directory_id("src"));
            assert_eq!(
                filtered.entry(1).unwrap().item().id,
                directory_id("src/core/flags")
            );
            assert_eq!(filtered.entry(2).unwrap().item().id, file_id(paths[0]));
            assert!(filtered.entry(3).is_none());
            // Clearing the filter restores the original collapse state.
            let restored = TreeState::new(cx).items(kit_items(&tree, &paths, &collapsed, ""));
            assert_eq!(restored.entry(0).unwrap().item().id, directory_id("src"));
            assert_eq!(restored.entry(1).unwrap().item().id, file_id("README.md"));
            assert!(restored.entry(2).is_none());
        });
    }

    #[gpui::test]
    fn fuzzy_filter_preserves_topology_labels_and_tree_order(cx: &mut gpui::TestAppContext) {
        // Keep src and core/flags separate even when filtering leaves a single
        // child directory; do not rebuild/compress a different tree of matches.
        let paths = [
            "src/core/flags/main_test.rs",
            "src/core/flags/main.rs",
            "src/main.rs",
            "docs/guide.md",
            "src/other/nope.txt",
            "main.rs",
        ];
        let tree = build_tree(&paths);
        cx.update(|cx| {
            let state =
                TreeState::new(cx).items(kit_items(&tree, &paths, &HashSet::new(), "main.rs"));
            let rows: Vec<_> = (0..)
                .map_while(|ix| state.entry(ix))
                .map(|entry| {
                    (
                        entry.depth(),
                        entry.item().label.as_ref(),
                        entry.item().id.as_ref(),
                    )
                })
                .collect();
            assert_eq!(
                rows,
                vec![
                    (0, "src", "directory:src"),
                    (1, "core/flags", "directory:src/core/flags"),
                    (2, "main.rs", "file:src/core/flags/main.rs"),
                    (2, "main_test.rs", "file:src/core/flags/main_test.rs"),
                    (1, "main.rs", "file:src/main.rs"),
                    (0, "main.rs", "file:main.rs"),
                ]
            );
            // Matching a directory name keeps its files, with their ancestors.
            let state =
                TreeState::new(cx).items(kit_items(&tree, &paths, &HashSet::new(), "flags"));
            assert_eq!(state.entry(0).unwrap().item().id, directory_id("src"));
            assert_eq!(
                state.entry(1).unwrap().item().id,
                directory_id("src/core/flags")
            );
            assert_eq!(state.entry(2).unwrap().item().id, file_id(paths[1]));
            assert_eq!(state.entry(3).unwrap().item().id, file_id(paths[0]));
            assert!(state.entry(4).is_none());
        });
        assert!(kit_items(&tree, &paths, &HashSet::new(), "zzzqqq").is_empty());
        assert!(kit_items(&[], &[], &HashSet::new(), "main").is_empty());
    }

    #[test]
    fn fuzzy_file_matches_rank_by_score() {
        let paths = ["src/core/flags/defs.rs", "src/main.rs", "docs/guide.md"];
        assert_eq!(fuzzy_file_matches(&paths, "defs"), vec![0]);
        assert_eq!(fuzzy_file_matches(&paths, "guide"), vec![2]);
        assert!(fuzzy_file_matches(&paths, "zzzqqq").is_empty());
        // Empty / whitespace query: everything, diff order.
        assert_eq!(fuzzy_file_matches(&paths, ""), vec![0, 1, 2]);
        assert_eq!(fuzzy_file_matches(&paths, "  "), vec![0, 1, 2]);
        // Best score first: the contiguous match beats the spread-out one.
        let paths = ["main_test.rs", "main.rs"];
        assert_eq!(fuzzy_file_matches(&paths, "main.rs"), vec![1, 0]);
    }

    #[gpui::test]
    fn status_style_matches_file_header_tags(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui::init(cx);
            assert_eq!(
                status_style(FileStatus::Added, cx),
                ("added", theme::green(cx))
            );
            assert_eq!(
                status_style(FileStatus::Deleted, cx),
                ("deleted", theme::red(cx))
            );
            assert_eq!(
                status_style(FileStatus::Modified, cx),
                ("modified", theme::blue(cx))
            );
            assert_eq!(
                status_style(FileStatus::Renamed, cx),
                ("renamed", theme::mauve(cx))
            );
            assert_eq!(
                status_style(FileStatus::Binary, cx),
                ("binary", theme::peach(cx))
            );
        });
    }
}
