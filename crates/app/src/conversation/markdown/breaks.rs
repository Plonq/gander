//! GitHub's issue/PR prose uses hard breaks, unlike Markdown files.
//!
//! Adapt soft paragraph line endings to hard breaks. A GitHub image on its
//! own line uses a block break so the fitted image renderer owns its geometry.
//! Editing source rather than serializing the AST preserves other syntax.
use ::markdown::{mdast::Node, ParseOptions};

pub(in crate::conversation) fn hard_breaks(source: String) -> String {
    if !source.contains(['\r', '\n']) {
        return source;
    }
    // Match Kit's constructs so math contents, like code, are left alone.
    let mut options = ParseOptions::gfm();
    options.constructs.math_text = true;
    options.constructs.math_flow = true;
    let Ok(root) = ::markdown::to_mdast(&source, &options) else {
        return source;
    };
    let mut breaks = Vec::new();
    let mut image_breaks = Vec::new();
    collect(&root, &source, false, &mut breaks, &mut image_breaks);
    image_breaks.sort_unstable();
    image_breaks.dedup();
    // A paragraph boundary replaces the soft break before it. Inserting both
    // would leave a literal backslash at the end of the preceding paragraph.
    breaks.retain(|offset| {
        !image_breaks.iter().any(|boundary| {
            *boundary == offset + 1
                || (source.as_bytes()[*offset] == b'\r' && *boundary == offset + 2)
        })
    });
    let mut edits: Vec<_> = breaks.into_iter().map(|offset| (offset, '\\')).collect();
    edits.extend(image_breaks.into_iter().map(|offset| (offset, '\n')));
    edits.sort_unstable_by_key(|(offset, _)| *offset);
    if edits.is_empty() {
        return source;
    }
    let mut out = String::with_capacity(source.len() + edits.len());
    let mut copied = 0;
    for (offset, insert) in edits {
        out.push_str(&source[copied..offset]);
        out.push(insert);
        copied = offset;
    }
    out.push_str(&source[copied..]);
    out
}

fn collect(
    node: &Node,
    source: &str,
    paragraph: bool,
    breaks: &mut Vec<usize>,
    image_breaks: &mut Vec<usize>,
) {
    // Image alt text isn't rendered prose, and headings use normal GFM rules.
    if matches!(
        node,
        Node::Image(_) | Node::ImageReference(_) | Node::Heading(_)
    ) {
        return;
    }
    if let Node::Paragraph(paragraph) = node {
        for child in &paragraph.children {
            let Node::Html(html) = child else { continue };
            if super::github_image::parse(&html.value).is_none() {
                continue;
            }
            if let Some(position) = &html.position {
                let start = position.start.offset;
                let end = position.end.offset;
                let line_start = source[..start].rfind('\n').map_or(0, |ix| ix + 1);
                let line_end = source[end..].find('\n').map_or(source.len(), |ix| end + ix);
                // True inline images and HTML containers keep their inline
                // semantics. Only standalone image lines become blocks.
                if source[line_start..start].trim().is_empty()
                    && source[end..line_end].trim().is_empty()
                {
                    if line_start > 0 {
                        image_breaks.push(line_start);
                    }
                    if line_end < source.len() {
                        image_breaks.push(line_end + 1);
                    }
                }
            }
        }
    }
    if let Node::Text(text) = node {
        if paragraph {
            if let Some(position) = &text.position {
                let start = position.start.offset;
                let raw = &source[start..position.end.offset];
                for (offset, byte) in raw.bytes().enumerate() {
                    if byte == b'\r' || (byte == b'\n' && !raw[..offset].ends_with('\r')) {
                        breaks.push(start + offset);
                    }
                }
            }
        }
    }
    if let Some(children) = node.children() {
        for child in children {
            collect(
                child,
                source,
                paragraph || matches!(node, Node::Paragraph(_)),
                breaks,
                image_breaks,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_newlines_in_paragraphs_become_hard_breaks() {
        for (source, expected) in [
            ("line1\nline2\nline3", "line1\\\nline2\\\nline3"),
            ("é\r\n二\rthree", "é\\\r\n二\\\rthree"),
            ("**one\ntwo**\n_three_", "**one\\\ntwo**\\\n_three_"),
            (
                "[one\ntwo](https://example.com)",
                "[one\\\ntwo](https://example.com)",
            ),
            ("> one\n> two", "> one\\\n> two"),
            ("- one\n  continued\n- two", "- one\\\n  continued\n- two"),
            ("one \ntwo\n", "one \\\ntwo\n"),
        ] {
            let adapted = hard_breaks(source.into());
            assert_eq!(adapted, expected, "{source:?}");
            assert_eq!(hard_breaks(adapted.clone()), adapted, "idempotent");
        }
    }

    #[test]
    fn structural_and_literal_newlines_are_not_changed() {
        for source in [
            "one\n\ntwo\n",
            "one  \ntwo\\\nthree<br>four",
            "# heading\n\nsetext\nheading\n=======",
            "- one\n- two\n\n> quote\n\n---",
            "`one\ntwo`\n\n``one\ntwo``",
            "```rust\nlet a = 1;\nlet b = 2;\n```",
            "~~~\none\ntwo\n~~~",
            "    one\n    two\n",
            "> ```\n> one\n> two\n> ```",
            "$$\nx + y\n$$",
            "$one\ntwo$",
            "<details>\n<summary>More</summary>\none\ntwo\n</details>",
            "<!-- one\ntwo -->",
            "<pre>one\ntwo</pre>",
            "| one | two |\n| --- | --- |\n| a | b |",
            "![one\ntwo](https://example.com/image)",
            "[ref]: https://example.com\n  \"one\ntwo\"\n\n[ref]",
        ] {
            assert_eq!(hard_breaks(source.into()), source, "{source:?}");
        }
    }

    #[test]
    fn source_line_endings_not_decoded_entities_determine_breaks() {
        let source = "a&#10;b &amp; c\nd";
        assert_eq!(hard_breaks(source.into()), "a&#10;b &amp; c\\\nd");
    }

    #[test]
    fn standalone_github_images_after_prose_use_block_layout() {
        let image = "<img width='1400' height='900' src='https://github.com/user-attachments/assets/large'>";
        for newline in ["\n", "\r\n"] {
            let source = format!("Before{newline}Caption{newline}{image}{newline}After");
            let expected = format!("Before\\{newline}Caption{newline}\n{image}{newline}\nAfter");
            let adapted = hard_breaks(source);
            assert_eq!(adapted, expected);
            assert_eq!(hard_breaks(adapted.clone()), adapted);
        }
    }

    #[test]
    fn github_image_breaks_do_not_split_inline_images_links_or_code() {
        let image = "<img width='1400' height='900' src='https://github.com/user-attachments/assets/large'>";
        for source in [
            format!("Before {image} after\n"),
            format!("[before\n{image}\nafter](https://example.com)"),
            format!("```html\nCaption\n{image}\n```"),
            format!("<div>\nCaption\n{image}\n</div>"),
        ] {
            let adapted = hard_breaks(source.clone());
            assert!(!adapted.contains("\n\n"), "{source}");
        }
    }
}
