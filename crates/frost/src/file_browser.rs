//! File browser for the Open menu entry (Round 2; Round 7 makes it a
//! native libcosmic window instead of an HTML page served by the
//! loopback server, which no longer exists).
//!
//! Open is a real working file browser, not a placeholder. The app
//! lists a directory app-side ([`list_dir`]) and the Open window shows
//! it natively: folder clicks re-list, file clicks open the file in the
//! browser as `file://`, `.md` files render as styled native text
//! ([`markdown_blocks`]) instead of formatted HTML.
//!
//! The listing starts at the home directory. Hidden entries (names
//! starting with `.`) are skipped — a home listing with every dotfile
//! would drown the real folders. Sizes are shown for files; directories
//! sort first, then files, each alphabetical (case-insensitive).

use std::path::{Path, PathBuf};

/// The directory the browser starts in: `$HOME`, or `/` if unset.
pub fn home_dir() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/"))
}

/// One row of a directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    /// File size in bytes (0 for directories).
    pub size: u64,
}

/// List `dir`: directories first, then files, each alphabetical
/// (case-insensitive); hidden entries skipped. Returns the entries, or
/// an error string when the directory cannot be read.
pub fn list_dir(dir: &Path) -> Result<Vec<DirEntry>, String> {
    let mut rows: Vec<DirEntry> = Vec::new();
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("Cannot list this folder: {e}"))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let size = if is_dir {
            0
        } else {
            entry.metadata().map(|m| m.len()).unwrap_or(0)
        };
        rows.push(DirEntry { name, is_dir, size });
    }
    rows.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(rows)
}

/// Human-readable file size (`842 B`, `12 KB`, `3.4 MB`).
pub fn human_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{} KB", bytes / 1024)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// Read a Markdown file as text, or `None` when it cannot be read.
pub fn read_markdown(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// One block of rendered Markdown, for the native Markdown view.
/// Inline emphasis/bold/links are flattened to plain text — the block
/// structure (headings, paragraphs, code, lists) is what is styled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MdBlock {
    Heading { level: u8, text: String },
    Paragraph(String),
    Code(String),
    List(Vec<String>),
    Quote(String),
    Rule,
}

/// Parse Markdown text into native blocks (pulldown-cmark events into
/// [`MdBlock`]s). Never fails on odd input — unknown constructs fall
/// back to plain paragraphs.
pub fn markdown_blocks(text: &str) -> Vec<MdBlock> {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

    let parser = Parser::new_ext(text, Options::all());
    let mut blocks: Vec<MdBlock> = Vec::new();
    // The block currently being collected: (kind, text buffer).
    // Kind is one of Heading(level), Paragraph, Code, Quote, ListItem.
    enum Kind {
        Heading(u8),
        Paragraph,
        Code,
        Quote,
        ListItem,
    }
    let mut current: Option<(Kind, String)> = None;
    let mut list_items: Option<Vec<String>> = None;
    // Blockquote nesting depth: a paragraph inside a quote is a
    // Quote block, not a plain Paragraph.
    let mut quote_depth = 0usize;

    // Flush the block being collected into `blocks` (or into the
    // in-progress list, for items).
    macro_rules! flush {
        () => {
            if let Some((kind, text)) = current.take() {
                let text = text.trim().to_string();
                match kind {
                    Kind::Heading(level) => {
                        if !text.is_empty() {
                            blocks.push(MdBlock::Heading { level, text });
                        }
                    },
                    Kind::Paragraph => {
                        if !text.is_empty() {
                            blocks.push(MdBlock::Paragraph(text));
                        }
                    },
                    Kind::Code => {
                        blocks.push(MdBlock::Code(text));
                    },
                    Kind::Quote => {
                        if !text.is_empty() {
                            blocks.push(MdBlock::Quote(text));
                        }
                    },
                    Kind::ListItem => {
                        if !text.is_empty() {
                            if let Some(items) = &mut list_items {
                                items.push(text);
                            }
                        }
                    },
                }
            }
        };
    }

    for event in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                flush!();
                current = Some((Kind::Heading(level as u8), String::new()));
            },
            Event::Start(Tag::Paragraph) => {
                flush!();
                let kind = if quote_depth > 0 {
                    Kind::Quote
                } else {
                    Kind::Paragraph
                };
                current = Some((kind, String::new()));
            },
            Event::Start(Tag::CodeBlock(_)) => {
                flush!();
                current = Some((Kind::Code, String::new()));
            },
            Event::Start(Tag::BlockQuote(_)) => {
                flush!();
                quote_depth += 1;
                current = Some((Kind::Quote, String::new()));
            },
            Event::Start(Tag::List(_)) => {
                flush!();
                list_items = Some(Vec::new());
            },
            Event::Start(Tag::Item) => {
                flush!();
                current = Some((Kind::ListItem, String::new()));
            },
            Event::Start(_) => {
                // Other inline/block tags (emphasis, strong, links,
                // tables, footnotes): keep collecting text into the
                // current block; the structure is flattened, not lost.
                if current.is_none() {
                    current = Some((Kind::Paragraph, String::new()));
                }
            },
            Event::End(TagEnd::List(_)) => {
                flush!();
                if let Some(items) = list_items.take() {
                    if !items.is_empty() {
                        blocks.push(MdBlock::List(items));
                    }
                }
            },
            Event::End(TagEnd::Heading(_))
            | Event::End(TagEnd::Paragraph)
            | Event::End(TagEnd::CodeBlock)
            | Event::End(TagEnd::Item)
            | Event::End(TagEnd::Table)
            | Event::End(TagEnd::TableHead)
            | Event::End(TagEnd::TableRow)
            | Event::End(TagEnd::TableCell)
            | Event::End(TagEnd::FootnoteDefinition)
            | Event::End(TagEnd::HtmlBlock)
            | Event::End(TagEnd::DefinitionList)
            | Event::End(TagEnd::DefinitionListTitle)
            | Event::End(TagEnd::DefinitionListDefinition)
            | Event::End(TagEnd::MetadataBlock(_)) => {
                flush!();
            },
            Event::End(TagEnd::BlockQuote(_)) => {
                quote_depth = quote_depth.saturating_sub(1);
                flush!();
            },
            Event::End(_) => {
                // Inline ends (emphasis, strong, link, strikethrough,
                // subscript, superscript): keep collecting into the
                // current block — flushing here would split one
                // paragraph at every **bold** or [link](…) (found by
                // the unit tests, 2026-11-15).
            },
            Event::Text(text) | Event::Code(text) => {
                if let Some((_, buf)) = &mut current {
                    buf.push_str(&text);
                } else {
                    current = Some((Kind::Paragraph, text.into_string()));
                }
            },
            Event::InlineMath(math) | Event::DisplayMath(math) => {
                // Math is not rendered natively; keep the source text
                // inline so the content is visible, not dropped.
                if let Some((_, buf)) = &mut current {
                    buf.push_str(&math);
                } else {
                    current = Some((Kind::Paragraph, math.into_string()));
                }
            },
            Event::SoftBreak => {
                if let Some((_, buf)) = &mut current {
                    buf.push(' ');
                }
            },
            Event::HardBreak => {
                if let Some((_, buf)) = &mut current {
                    buf.push('\n');
                }
            },
            Event::Rule => {
                flush!();
                blocks.push(MdBlock::Rule);
            },
            Event::Html(_) | Event::InlineHtml(_) | Event::FootnoteReference(_) => {},
            Event::TaskListMarker(_) => {},
        }
    }
    flush!();
    if let Some(items) = list_items.take() {
        if !items.is_empty() {
            blocks.push(MdBlock::List(items));
        }
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_skips_hidden_entries_and_sorts_dirs_first() {
        let dir = std::env::temp_dir().join("frost-file-browser-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir(dir.join("visible-dir")).unwrap();
        std::fs::write(dir.join("visible.txt"), b"hello").unwrap();
        std::fs::write(dir.join(".hidden"), b"nope").unwrap();
        let rows = list_dir(&dir).unwrap();
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["visible-dir", "visible.txt"], "{names:?}");
        assert!(rows[0].is_dir);
        assert_eq!(rows[1].size, 5);
        assert!(list_dir(&dir.join("missing")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn markdown_blocks_keep_structure() {
        let blocks = markdown_blocks("# Hello\n\nSome **bold** text.\n\n- one\n- two\n\n```\ncode()\n```\n");
        assert_eq!(
            blocks,
            vec![
                MdBlock::Heading { level: 1, text: "Hello".to_string() },
                MdBlock::Paragraph("Some bold text.".to_string()),
                MdBlock::List(vec!["one".to_string(), "two".to_string()]),
                MdBlock::Code("code()".to_string()),
            ],
            "{blocks:?}"
        );
        assert!(markdown_blocks("").is_empty());
        assert_eq!(
            markdown_blocks("> quoted\n\n---\n"),
            vec![
                MdBlock::Quote("quoted".to_string()),
                MdBlock::Rule,
            ]
        );
    }

    #[test]
    fn markdown_file_reads_and_parses() {
        let dir = std::env::temp_dir().join("frost-markdown-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("note.md");
        std::fs::write(&path, "# Hello\n\nSome **bold** text.\n").unwrap();
        let text = read_markdown(&path).unwrap();
        let blocks = markdown_blocks(&text);
        assert_eq!(
            blocks,
            vec![
                MdBlock::Heading { level: 1, text: "Hello".to_string() },
                MdBlock::Paragraph("Some bold text.".to_string()),
            ]
        );
        assert!(read_markdown(&dir.join("missing.md")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn human_sizes() {
        assert_eq!(human_size(842), "842 B");
        assert_eq!(human_size(12 * 1024), "12 KB");
        assert_eq!(human_size(3_400_000), "3.2 MB");
    }
}
