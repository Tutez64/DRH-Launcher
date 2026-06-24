use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use slint::StyledText;

use crate::MarkdownBlockView;

pub fn markdown_blocks(body: &str) -> Vec<MarkdownBlockView> {
    let body = body.trim().replace("\r\n", "\n");
    if body.is_empty() {
        return vec![markdown_block(
            "No changelog was provided for this release.",
            MarkdownBlockKind::Paragraph,
        )];
    }

    let mut renderer = MarkdownRenderer::default();
    let options = Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_GFM;
    for event in Parser::new_ext(&body, options) {
        renderer.push_event(event);
    }
    renderer.finish()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MarkdownBlockKind {
    Paragraph = 0,
    Heading1 = 1,
    Heading2 = 2,
    Heading3 = 3,
    Bullet = 4,
    Numbered = 5,
    Code = 6,
    Quote = 7,
    Rule = 8,
    Image = 9,
}

impl MarkdownBlockKind {
    fn from_heading(level: HeadingLevel) -> Self {
        match level {
            HeadingLevel::H1 => Self::Heading1,
            HeadingLevel::H2 => Self::Heading2,
            HeadingLevel::H3 | HeadingLevel::H4 | HeadingLevel::H5 | HeadingLevel::H6 => {
                Self::Heading3
            }
        }
    }
}

#[derive(Default)]
struct MarkdownRenderer {
    blocks: Vec<MarkdownBlockView>,
    current: Option<MarkdownBlockBuilder>,
    list_stack: Vec<Option<u64>>,
    capture_stack: Vec<MarkdownCapture>,
    link_stack: Vec<String>,
    quote_depth: usize,
}

impl MarkdownRenderer {
    fn push_event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start_tag(tag),
            Event::End(tag) => self.end_tag(tag),
            Event::Text(text) => self.push_text(&text),
            Event::Code(code) => self.push_inline_code(&code),
            Event::InlineMath(math) => self.push_text(&format!("${math}$")),
            Event::DisplayMath(math) => self.push_text(&format!("$$\n{math}\n$$")),
            Event::Html(html) | Event::InlineHtml(html) => self.push_text(&html),
            Event::FootnoteReference(reference) => self.push_text(&format!("[^{reference}]")),
            Event::SoftBreak => self.push_text(" "),
            Event::HardBreak => self.push_text("\n"),
            Event::Rule => {
                self.flush_current();
                self.blocks
                    .push(markdown_block("", MarkdownBlockKind::Rule));
            }
            Event::TaskListMarker(checked) => {
                self.push_text(if checked { "[x] " } else { "[ ] " });
            }
        }
    }

    fn start_tag(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                if self.current.is_none() {
                    self.start_block(if self.quote_depth > 0 {
                        MarkdownBlockKind::Quote
                    } else {
                        MarkdownBlockKind::Paragraph
                    });
                }
            }
            Tag::Heading { level, .. } => self.start_block(MarkdownBlockKind::from_heading(level)),
            Tag::BlockQuote(_) => {
                self.flush_current();
                self.quote_depth += 1;
            }
            Tag::CodeBlock(kind) => {
                self.start_block(MarkdownBlockKind::Code);
                if let CodeBlockKind::Fenced(language) = kind {
                    let language = language.trim();
                    if !language.is_empty() {
                        self.push_raw_text(language);
                        self.push_raw_text("\n");
                    }
                }
            }
            Tag::List(start) => {
                self.flush_current();
                self.list_stack.push(start);
            }
            Tag::Item => {
                self.flush_current();
                let depth = self.list_stack.len().saturating_sub(1);
                let indent = "  ".repeat(depth);
                if let Some(Some(number)) = self.list_stack.last_mut() {
                    let current = *number;
                    *number += 1;
                    self.start_block(MarkdownBlockKind::Numbered);
                    self.push_raw_text(&format!("{indent}{current}. "));
                } else {
                    self.start_block(MarkdownBlockKind::Bullet);
                    self.push_raw_text(&format!("{indent}• "));
                }
            }
            Tag::Link { dest_url, .. } => {
                self.push_raw_text("[");
                self.link_stack.push(dest_url.to_string());
            }
            Tag::Image { dest_url, .. } => {
                self.capture_stack
                    .push(MarkdownCapture::new(MarkdownCaptureKind::Image, &dest_url));
            }
            Tag::Table(_)
            | Tag::TableHead
            | Tag::TableRow
            | Tag::TableCell
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::FootnoteDefinition(_)
            | Tag::MetadataBlock(_) => {
                if self.current.is_none() {
                    self.start_block(MarkdownBlockKind::Paragraph);
                }
            }
            Tag::Emphasis => self.push_raw_text("*"),
            Tag::Strong => self.push_raw_text("**"),
            Tag::Strikethrough => self.push_raw_text("~~"),
            Tag::Superscript | Tag::Subscript => {}
            Tag::HtmlBlock => self.start_block(MarkdownBlockKind::Code),
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::Item => {
                self.flush_current();
            }
            TagEnd::BlockQuote(_) => {
                self.flush_current();
                self.quote_depth = self.quote_depth.saturating_sub(1);
            }
            TagEnd::List(_) => {
                self.flush_current();
                self.list_stack.pop();
            }
            TagEnd::Link => {
                let url = self.link_stack.pop().unwrap_or_default();
                self.push_raw_text(&format!("]({})", escape_markdown_link_url(&url)));
            }
            TagEnd::Image => self.finish_capture(MarkdownCaptureKind::Image),
            TagEnd::TableRow => {
                self.push_text("\n");
            }
            TagEnd::TableCell => {
                self.push_text("  ");
            }
            TagEnd::HtmlBlock
            | TagEnd::FootnoteDefinition
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::Table
            | TagEnd::TableHead
            | TagEnd::Superscript
            | TagEnd::Subscript
            | TagEnd::MetadataBlock(_) => {}
            TagEnd::Emphasis => self.push_raw_text("*"),
            TagEnd::Strong => self.push_raw_text("**"),
            TagEnd::Strikethrough => self.push_raw_text("~~"),
        }
    }

    fn start_block(&mut self, kind: MarkdownBlockKind) {
        self.flush_current();
        self.current = Some(MarkdownBlockBuilder {
            text: String::new(),
            kind,
        });
    }

    fn push_text(&mut self, text: &str) {
        if let Some(capture) = self.capture_stack.last_mut() {
            capture.text.push_str(text);
            if capture.kind == MarkdownCaptureKind::Image {
                return;
            }
        }

        let text = if self
            .current
            .as_ref()
            .is_some_and(|current| current.kind == MarkdownBlockKind::Code)
        {
            text.to_string()
        } else {
            escape_markdown_text(text)
        };
        self.push_raw_text(&text);
    }

    fn push_raw_text(&mut self, text: &str) {
        if self.current.is_none() {
            self.start_block(if self.quote_depth > 0 {
                MarkdownBlockKind::Quote
            } else {
                MarkdownBlockKind::Paragraph
            });
        }
        if let Some(current) = &mut self.current {
            current.text.push_str(text);
        }
    }

    fn push_inline_code(&mut self, code: &str) {
        if let Some(capture) = self.capture_stack.last_mut() {
            capture.text.push_str(code);
            if capture.kind == MarkdownCaptureKind::Image {
                return;
            }
        }

        if !code.is_empty() {
            self.push_raw_text("`");
            self.push_raw_text(&escape_markdown_code_span(code));
            self.push_raw_text("`");
        }
    }

    fn finish_capture(&mut self, kind: MarkdownCaptureKind) {
        let Some(capture) = self.capture_stack.pop() else {
            return;
        };
        if capture.kind != kind {
            return;
        }

        let label = capture.text.trim();
        let target = capture.target.trim();
        if target.is_empty() {
            return;
        }

        if kind == MarkdownCaptureKind::Image {
            self.flush_current();
            let label = if label.is_empty() { "Image" } else { label };
            self.blocks.push(markdown_target_block(
                &format!("Image: {label}"),
                target,
                MarkdownBlockKind::Image,
            ));
        }
    }

    fn flush_current(&mut self) {
        if let Some(current) = self.current.take() {
            let text = current.text.trim();
            if !text.is_empty() {
                self.blocks.push(markdown_block(text, current.kind));
            }
        }
    }

    fn finish(mut self) -> Vec<MarkdownBlockView> {
        self.flush_current();
        if self.blocks.is_empty() {
            self.blocks.push(markdown_block(
                "No changelog was provided for this release.",
                MarkdownBlockKind::Paragraph,
            ));
        }
        self.blocks
    }
}

struct MarkdownBlockBuilder {
    text: String,
    kind: MarkdownBlockKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MarkdownCaptureKind {
    Image,
}

struct MarkdownCapture {
    kind: MarkdownCaptureKind,
    target: String,
    text: String,
}

impl MarkdownCapture {
    fn new(kind: MarkdownCaptureKind, target: &str) -> Self {
        Self {
            kind,
            target: target.to_string(),
            text: String::new(),
        }
    }
}

fn markdown_block(text: &str, kind: MarkdownBlockKind) -> MarkdownBlockView {
    markdown_target_block(text, "", kind)
}

fn markdown_target_block(text: &str, target: &str, kind: MarkdownBlockKind) -> MarkdownBlockView {
    MarkdownBlockView {
        text: markdown_block_text(text, kind),
        kind: kind as i32,
        target: target.into(),
    }
}

fn markdown_block_text(text: &str, kind: MarkdownBlockKind) -> StyledText {
    if kind == MarkdownBlockKind::Code || kind == MarkdownBlockKind::Image {
        return StyledText::from_plain_text(text);
    }

    StyledText::from_markdown(text)
        .unwrap_or_else(|_| StyledText::from_plain_text(&unescape_markdown_text(text)))
}

fn escape_markdown_text(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        if matches!(
            character,
            '\\' | '`'
                | '*'
                | '_'
                | '{'
                | '}'
                | '['
                | ']'
                | '('
                | ')'
                | '#'
                | '+'
                | '-'
                | '.'
                | '!'
                | '<'
                | '>'
                | '~'
                | '|'
        ) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn escape_markdown_code_span(text: &str) -> String {
    text.replace('`', "'")
}

fn escape_markdown_link_url(url: &str) -> String {
    url.replace(')', "%29")
}

fn unescape_markdown_text(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut escaped = false;
    for character in text.chars() {
        if escaped {
            plain.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else {
            plain.push(character);
        }
    }
    if escaped {
        plain.push('\\');
    }
    plain
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_markdown_changelog_as_display_blocks() {
        let blocks = markdown_blocks(
            r#"# V2

Changes:

- Added **feature**
- Fixed [bug](https://example.test/bug) with `--safe-mode`

![Preview](https://example.test/preview.png)

```text
code sample
```
"#,
        );

        assert_eq!(blocks[0].kind, MarkdownBlockKind::Heading1 as i32);
        assert_eq!(blocks[1].kind, MarkdownBlockKind::Paragraph as i32);
        assert_eq!(blocks[2].kind, MarkdownBlockKind::Bullet as i32);
        assert_eq!(blocks[3].kind, MarkdownBlockKind::Bullet as i32);
        assert!(format!("{:?}", blocks[3].text).contains("Fixed bug with --safe-mode"));
        assert!(format!("{:?}", blocks[3].text).contains("https://example.test/bug"));
        assert_eq!(blocks[4].target, "https://example.test/preview.png");
        assert_eq!(blocks[4].kind, MarkdownBlockKind::Image as i32);
        assert!(format!("{:?}", blocks[5].text).contains("text\\ncode sample"));
        assert_eq!(blocks[5].kind, MarkdownBlockKind::Code as i32);
    }
}
