use markdown::mdast::*;
use std::fmt::Write;

/// Reflow the given markdown string to the target line width.
///
/// Paragraphs are reflowed; block-level structure (headings, list items,
/// blockquotes, code blocks, etc.) is preserved.
pub fn reflow(input: &str, target_width: usize) -> Result<String, String> {
    reflow_with_initial_width(input, target_width, target_width)
}

/// Reflow the given markdown string using `initial_width` for the first
/// reflowed line and `target_width` for later lines.
///
/// This is useful when the first rendered line has an external prefix, such as
/// a comment opening delimiter, that is not present on continuation lines.
pub fn reflow_with_initial_width(
    input: &str,
    initial_width: usize,
    target_width: usize,
) -> Result<String, String> {
    reflow_with_edge_widths(
        input,
        initial_width,
        target_width,
        target_width,
        initial_width,
    )
}

/// Reflow markdown with distinct budgets for the first, middle, last, and
/// single rendered content line.
pub fn reflow_with_edge_widths(
    input: &str,
    initial_width: usize,
    target_width: usize,
    final_width: usize,
    single_line_width: usize,
) -> Result<String, String> {
    let options = markdown::ParseOptions {
        constructs: markdown::Constructs {
            gfm_table: true,
            gfm_strikethrough: true,
            gfm_footnote_definition: true,
            gfm_label_start_footnote: true,
            gfm_autolink_literal: true,
            gfm_task_list_item: true,
            ..markdown::Constructs::default()
        },
        ..markdown::ParseOptions::default()
    };
    let mut ast = markdown::to_mdast(input, &options).map_err(|e| e.to_string())?;

    reflow_root_with_edge_widths(
        &mut ast,
        initial_width,
        target_width,
        final_width,
        single_line_width,
    );

    let mut output = String::new();
    render_root(&ast, &mut output);
    Ok(output)
}

// ---------------------------------------------------------------------------
// Span: a flat representation of inline (phrasing) content
// ---------------------------------------------------------------------------

/// Represents a piece of inline content that contributes to the visible text
/// width.  The `width` method gives the number of visible columns.
#[derive(Clone, Debug)]
enum Span {
    /// Plain text.
    Text(String),
    /// A hard line break (backslash-newline or two-spaces-newline in source).
    Break,
    /// Inline code (atomic — cannot be split across lines).
    InlineCode(String),
    /// Inline math (atomic).
    InlineMath(String),
    /// Raw inline HTML (atomic).
    Html(String),
    /// Emphasised text.
    Emphasis(Vec<Span>),
    /// Strongly emphasised text.
    Strong(Vec<Span>),
    /// Deleted text.
    Delete(Vec<Span>),
    /// A link.
    Link {
        url: String,
        title: Option<String>,
        children: Vec<Span>,
    },
    /// A link reference.
    LinkReference {
        reference_kind: ReferenceKind,
        identifier: String,
        label: Option<String>,
        children: Vec<Span>,
    },
    /// An image (inline but void).
    Image {
        alt: String,
        url: String,
        title: Option<String>,
    },
    /// An image reference.
    ImageReference {
        alt: String,
        reference_kind: ReferenceKind,
        identifier: String,
        label: Option<String>,
    },
    /// A footnote reference.
    FootnoteReference {
        identifier: String,
        label: Option<String>,
    },
    /// A soft break we insert during reflow.
    SoftBreak,
}

impl Span {
    /// Visible width contributed by this span (ignoring markup characters).
    #[allow(dead_code)]
    fn width(&self) -> usize {
        match self {
            Span::Text(s) => s.chars().count(),
            Span::Break => 1,
            Span::InlineCode(s) => s.chars().count(),
            Span::InlineMath(s) => s.chars().count(),
            Span::Html(_) => 0,
            Span::Emphasis(children) => children.iter().map(Span::width).sum(),
            Span::Strong(children) => children.iter().map(Span::width).sum(),
            Span::Delete(children) => children.iter().map(Span::width).sum(),
            Span::Link { children, .. } => children.iter().map(Span::width).sum(),
            Span::LinkReference { children, .. } => children.iter().map(Span::width).sum(),
            Span::Image { alt, .. } => alt.chars().count(),
            Span::ImageReference { alt, .. } => alt.chars().count(),
            Span::FootnoteReference { .. } => 0,
            Span::SoftBreak => 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Converting AST phrasing children ⇄ Spans
// ---------------------------------------------------------------------------

fn node_to_spans(node: &Node) -> Vec<Span> {
    match node {
        Node::Text(t) => {
            if t.value.is_empty() {
                vec![]
            } else {
                vec![Span::Text(t.value.clone())]
            }
        }
        Node::Break(_) => vec![Span::Break],
        Node::InlineCode(ic) => vec![Span::InlineCode(ic.value.clone())],
        Node::InlineMath(im) => vec![Span::InlineMath(im.value.clone())],
        Node::Html(h) => vec![Span::Html(h.value.clone())],
        Node::Emphasis(em) => {
            let children: Vec<Span> = em.children.iter().flat_map(node_to_spans).collect();
            vec![Span::Emphasis(children)]
        }
        Node::Strong(st) => {
            let children: Vec<Span> = st.children.iter().flat_map(node_to_spans).collect();
            vec![Span::Strong(children)]
        }
        Node::Delete(del) => {
            let children: Vec<Span> = del.children.iter().flat_map(node_to_spans).collect();
            vec![Span::Delete(children)]
        }
        Node::Link(link) => {
            let children: Vec<Span> = link.children.iter().flat_map(node_to_spans).collect();
            vec![Span::Link {
                url: link.url.clone(),
                title: link.title.clone(),
                children,
            }]
        }
        Node::LinkReference(lr) => {
            let children: Vec<Span> = lr.children.iter().flat_map(node_to_spans).collect();
            vec![Span::LinkReference {
                reference_kind: lr.reference_kind,
                identifier: lr.identifier.clone(),
                label: lr.label.clone(),
                children,
            }]
        }
        Node::Image(img) => vec![Span::Image {
            alt: img.alt.clone(),
            url: img.url.clone(),
            title: img.title.clone(),
        }],
        Node::ImageReference(ir) => vec![Span::ImageReference {
            alt: ir.alt.clone(),
            reference_kind: ir.reference_kind,
            identifier: ir.identifier.clone(),
            label: ir.label.clone(),
        }],
        Node::FootnoteReference(fr) => vec![Span::FootnoteReference {
            identifier: fr.identifier.clone(),
            label: fr.label.clone(),
        }],
        _ => vec![],
    }
}

fn spans_to_nodes(spans: &[Span]) -> Vec<Node> {
    spans.iter().map(span_to_node).collect()
}

fn span_to_node(span: &Span) -> Node {
    match span {
        Span::Text(s) => Node::Text(Text {
            value: s.clone(),
            position: None,
        }),
        Span::Break => Node::Break(Break { position: None }),
        Span::InlineCode(s) => Node::InlineCode(InlineCode {
            value: s.clone(),
            position: None,
        }),
        Span::InlineMath(s) => Node::InlineMath(InlineMath {
            value: s.clone(),
            position: None,
        }),
        Span::Html(s) => Node::Html(Html {
            value: s.clone(),
            position: None,
        }),
        Span::Emphasis(children) => Node::Emphasis(Emphasis {
            children: spans_to_nodes(children),
            position: None,
        }),
        Span::Strong(children) => Node::Strong(Strong {
            children: spans_to_nodes(children),
            position: None,
        }),
        Span::Delete(children) => Node::Delete(Delete {
            children: spans_to_nodes(children),
            position: None,
        }),
        Span::Link {
            url,
            title,
            children,
        } => Node::Link(Link {
            url: url.clone(),
            title: title.clone(),
            children: spans_to_nodes(children),
            position: None,
        }),
        Span::LinkReference {
            reference_kind,
            identifier,
            label,
            children,
        } => Node::LinkReference(LinkReference {
            reference_kind: *reference_kind,
            identifier: identifier.clone(),
            label: label.clone(),
            children: spans_to_nodes(children),
            position: None,
        }),
        Span::Image { alt, url, title } => Node::Image(Image {
            alt: alt.clone(),
            url: url.clone(),
            title: title.clone(),
            position: None,
        }),
        Span::ImageReference {
            alt,
            reference_kind,
            identifier,
            label,
        } => Node::ImageReference(ImageReference {
            alt: alt.clone(),
            reference_kind: *reference_kind,
            identifier: identifier.clone(),
            label: label.clone(),
            position: None,
        }),
        Span::FootnoteReference { identifier, label } => {
            Node::FootnoteReference(FootnoteReference {
                identifier: identifier.clone(),
                label: label.clone(),
                position: None,
            })
        }
        Span::SoftBreak => {
            // A soft break becomes a newline in the source markdown.
            // We represent it as a Text node containing just "\n".
            Node::Text(Text {
                value: "\n".to_string(),
                position: None,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Word wrapping
// ---------------------------------------------------------------------------

/// A token in the word-wrapping stream.  Compound inline spans are opened
/// and closed so the wrapper can break at word boundaries inside them.
#[derive(Clone, Debug)]
enum WrapToken {
    /// Visible text (a word or whitespace run).
    Word(String),
    /// Hard line break.
    HardBreak,
    /// Inline code (atomic).
    Code(String),
    /// Inline math (atomic).
    Math(String),
    /// Raw HTML (atomic).
    RawHtml(String),
    /// Image (atomic, void).
    Img {
        alt: String,
        url: String,
        title: Option<String>,
    },
    /// Image reference (atomic, void).
    ImgRef {
        alt: String,
        reference_kind: ReferenceKind,
        identifier: String,
        label: Option<String>,
    },
    /// Footnote reference (atomic, void).
    FnRef {
        identifier: String,
        label: Option<String>,
    },
    /// Open emphasis.
    OpenEm,
    /// Close emphasis.
    CloseEm,
    /// Open strong.
    OpenStrong,
    /// Close strong.
    CloseStrong,
    /// Open delete.
    OpenDel,
    /// Close delete.
    CloseDel,
    /// Open link.
    OpenLink {
        url: String,
        title: Option<String>,
        close_width: usize,
    },
    /// Close link.
    CloseLink,
    /// Open link reference.
    OpenLinkRef {
        reference_kind: ReferenceKind,
        identifier: String,
        label: Option<String>,
        close_width: usize,
    },
    /// Close link reference.
    CloseLinkRef,
    /// A soft break we insert.
    SoftBreak,
}

impl WrapToken {
    /// Source-column footprint, counting visible text *and* markdown
    /// delimiters so that output lines never exceed the target width.
    fn width(&self) -> usize {
        match self {
            WrapToken::Word(s) => s.chars().count(),
            WrapToken::HardBreak => 2,
            WrapToken::Code(s) => {
                // backtick-fence overhead: at least 2, more if content has backticks
                let n = max_consecutive_backticks(s).max(1);
                let fence = n * 2;
                let pad = if n > 1 || s.starts_with('`') || s.ends_with('`') {
                    2
                } else {
                    0
                };
                fence + pad + s.chars().count()
            }
            WrapToken::Math(s) => 2 + s.chars().count(),
            WrapToken::RawHtml(s) => s.chars().count(),
            WrapToken::Img { alt, url, title } => {
                // ![alt](url) or ![alt](url "title")
                5 + alt.chars().count()
                    + url.chars().count()
                    + title.as_ref().map_or(0, |t| 3 + t.chars().count())
            }
            WrapToken::ImgRef {
                alt,
                reference_kind,
                identifier,
                label: _,
            } => {
                // ![alt][id] or ![alt][] or ![alt]
                let base = 2 + alt.chars().count(); // ![
                let close = match reference_kind {
                    ReferenceKind::Full => 3 + identifier.chars().count(),
                    ReferenceKind::Collapsed => 2,
                    ReferenceKind::Shortcut => 0,
                };
                base + close
            }
            WrapToken::FnRef { identifier, .. } => 4 + identifier.chars().count(),
            WrapToken::OpenEm | WrapToken::CloseEm => 1,
            WrapToken::OpenStrong | WrapToken::CloseStrong => 2,
            WrapToken::OpenDel | WrapToken::CloseDel => 2,
            WrapToken::OpenLink { close_width, .. } => 1 + close_width,
            WrapToken::CloseLink => 0,
            WrapToken::OpenLinkRef { close_width, .. } => 1 + close_width,
            WrapToken::CloseLinkRef => 0,
            WrapToken::SoftBreak => 0,
        }
    }

    fn is_hard_break(&self) -> bool {
        matches!(self, WrapToken::HardBreak)
    }

    /// Whether this token can have a line break inserted after it.
    fn is_break_opportunity(&self) -> bool {
        matches!(self, WrapToken::Word(s) if s.ends_with(' '))
    }
}

/// Flatten spans into a token stream, splitting compound spans into
/// open/close markers so the word-wrapper can break inside them.
fn tokenize(spans: &[Span], out: &mut Vec<WrapToken>) {
    for span in spans {
        tokenize_span(span, out);
    }
}

fn tokenize_span(span: &Span, out: &mut Vec<WrapToken>) {
    match span {
        Span::Text(s) => {
            // Normalise all whitespace (newlines, tabs, etc.) to spaces,
            // then split into words and whitespace runs.
            let normalized: String = s
                .chars()
                .map(|c| if c.is_whitespace() { ' ' } else { c })
                .collect();
            let mut current = String::new();
            for ch in normalized.chars() {
                if ch == ' ' {
                    if !current.is_empty() {
                        out.push(WrapToken::Word(current.clone()));
                        current.clear();
                    }
                    current.push(' ');
                } else {
                    if current.ends_with(' ') {
                        out.push(WrapToken::Word(current.clone()));
                        current.clear();
                    }
                    current.push(ch);
                }
            }
            if !current.is_empty() {
                out.push(WrapToken::Word(current));
            }
        }
        Span::Break => out.push(WrapToken::HardBreak),
        Span::InlineCode(s) => out.push(WrapToken::Code(s.clone())),
        Span::InlineMath(s) => out.push(WrapToken::Math(s.clone())),
        Span::Html(s) => out.push(WrapToken::RawHtml(s.clone())),
        Span::Emphasis(children) => {
            out.push(WrapToken::OpenEm);
            tokenize(children, out);
            out.push(WrapToken::CloseEm);
        }
        Span::Strong(children) => {
            out.push(WrapToken::OpenStrong);
            tokenize(children, out);
            out.push(WrapToken::CloseStrong);
        }
        Span::Delete(children) => {
            out.push(WrapToken::OpenDel);
            tokenize(children, out);
            out.push(WrapToken::CloseDel);
        }
        Span::Link {
            url,
            title,
            children,
        } => {
            out.push(WrapToken::OpenLink {
                close_width: 0,
                url: url.clone(),
                title: title.clone(),
            });
            tokenize(children, out);
            out.push(WrapToken::CloseLink);
        }
        Span::LinkReference {
            reference_kind,
            identifier,
            label,
            children,
        } => {
            let close_width = match reference_kind {
                ReferenceKind::Full => 3 + identifier.chars().count(),
                ReferenceKind::Collapsed => 2,
                ReferenceKind::Shortcut => 0,
            };
            out.push(WrapToken::OpenLinkRef {
                reference_kind: *reference_kind,
                identifier: identifier.clone(),
                label: label.clone(),
                close_width,
            });
            tokenize(children, out);
            out.push(WrapToken::CloseLinkRef);
        }
        Span::Image { alt, url, title } => {
            out.push(WrapToken::Img {
                alt: alt.clone(),
                url: url.clone(),
                title: title.clone(),
            });
        }
        Span::ImageReference {
            alt,
            reference_kind,
            identifier,
            label,
        } => {
            out.push(WrapToken::ImgRef {
                alt: alt.clone(),
                reference_kind: *reference_kind,
                identifier: identifier.clone(),
                label: label.clone(),
            });
        }
        Span::FootnoteReference { identifier, label } => {
            out.push(WrapToken::FnRef {
                identifier: identifier.clone(),
                label: label.clone(),
            });
        }
        Span::SoftBreak => out.push(WrapToken::SoftBreak),
    }
}

/// Given a flat list of spans, insert SoftBreak spans at word boundaries
/// so that no line exceeds `target_width` visible characters.
fn wrap_spans(spans: &[Span], target_width: usize) -> Vec<Span> {
    wrap_spans_with_initial_width(spans, target_width, target_width)
}

fn wrap_spans_with_initial_width(
    spans: &[Span],
    initial_width: usize,
    target_width: usize,
) -> Vec<Span> {
    wrap_spans_with_edge_widths(
        spans,
        initial_width,
        target_width,
        target_width,
        initial_width,
    )
}

fn wrap_spans_with_edge_widths(
    spans: &[Span],
    initial_width: usize,
    target_width: usize,
    final_width: usize,
    single_line_width: usize,
) -> Vec<Span> {
    if initial_width == 0 || target_width == 0 || final_width == 0 || single_line_width == 0 {
        return spans.to_vec();
    }

    // 1. Tokenize: flatten compound spans into open/close markers.
    let mut tokens: Vec<WrapToken> = Vec::new();
    tokenize(spans, &mut tokens);

    // 2. Greedy line-breaking on the flat token stream.
    let mut result_tokens: Vec<WrapToken> = Vec::new();
    let mut line_width: usize = 0;
    let mut current_target_width = initial_width;
    let mut i = 0;

    while i < tokens.len() {
        let tok = &tokens[i];

        // Hard breaks reset the line.
        if tok.is_hard_break() {
            result_tokens.push(tok.clone());
            line_width = 0;
            current_target_width = target_width;
            i += 1;
            continue;
        }

        // Skip leading whitespace at start of line.
        if line_width == 0 {
            if let WrapToken::Word(s) = tok {
                let trimmed = s.trim_start_matches(' ');
                if trimmed.is_empty() {
                    i += 1;
                    continue;
                }
                result_tokens.push(WrapToken::Word(trimmed.to_string()));
                line_width = trimmed.chars().count();
                i += 1;
                continue;
            }
        }

        let w = tok.width();

        if line_width + w > current_target_width && line_width > 0 {
            // Need to break.  Search backwards for a break opportunity.
            let mut found = false;
            for j in (0..result_tokens.len()).rev() {
                if result_tokens[j].is_break_opportunity() {
                    // The token at j is a Word ending in whitespace.
                    // Trim the trailing whitespace from that word,
                    // insert a SoftBreak after it, and the next line
                    // starts with the trimmed whitespace removed.
                    if let WrapToken::Word(ref s) = result_tokens[j].clone() {
                        let trimmed = s.trim_end().to_string();
                        result_tokens[j] = WrapToken::Word(trimmed);
                    }
                    result_tokens.insert(j + 1, WrapToken::SoftBreak);
                    current_target_width = target_width;

                    // Recalculate line width for what's after the break.
                    line_width = 0;
                    for k in (j + 2)..result_tokens.len() {
                        line_width += result_tokens[k].width();
                    }
                    found = true;
                    break;
                }
            }

            if found {
                // Continue without advancing i — reprocess current token.
                continue;
            } else {
                // No good break point; force a break before current token.
                result_tokens.push(WrapToken::SoftBreak);
                line_width = 0;
                current_target_width = target_width;
                continue;
            }
        } else {
            result_tokens.push(tok.clone());
            line_width += w;
            i += 1;
        }
    }

    reserve_final_line_width(
        &mut result_tokens,
        target_width,
        final_width,
        single_line_width,
    );

    // 3. Convert the token stream back to spans.
    tokens_to_spans(&result_tokens)
}

fn reserve_final_line_width(
    tokens: &mut Vec<WrapToken>,
    target_width: usize,
    final_width: usize,
    single_line_width: usize,
) {
    loop {
        let line_start = tokens
            .iter()
            .rposition(|tok| matches!(tok, WrapToken::SoftBreak | WrapToken::HardBreak))
            .map_or(0, |i| i + 1);
        let single_line = line_start == 0;
        let limit = if single_line {
            single_line_width
        } else {
            final_width
        };
        let line_width: usize = tokens[line_start..].iter().map(WrapToken::width).sum();
        if line_width <= limit {
            return;
        }

        let mut suffix_width = 0;
        let mut split_at = None;
        for i in (line_start..tokens.len()).rev() {
            suffix_width += tokens[i].width();
            if suffix_width > limit {
                break;
            }
            if i > line_start && tokens[i - 1].is_break_opportunity() {
                let prefix_width: usize =
                    tokens[line_start..i - 1].iter().map(WrapToken::width).sum();
                let trimmed_width = match &tokens[i - 1] {
                    WrapToken::Word(s) => s.trim_end().chars().count(),
                    tok => tok.width(),
                };
                if prefix_width + trimmed_width <= target_width {
                    split_at = Some(i - 1);
                }
            }
        }

        let Some(split_at) = split_at else {
            return;
        };

        if let WrapToken::Word(s) = &mut tokens[split_at] {
            *s = s.trim_end().to_string();
        }
        tokens.insert(split_at + 1, WrapToken::SoftBreak);
    }
}

/// Reconstruct spans from a token stream, matching open/close markers.
fn tokens_to_spans(tokens: &[WrapToken]) -> Vec<Span> {
    /// Parse a sequence of tokens into a list of spans, consuming tokens
    /// until we hit a close marker (or end of input).
    fn parse_spans(tokens: &[WrapToken], pos: &mut usize, stop_at_close: bool) -> Vec<Span> {
        let mut spans: Vec<Span> = Vec::new();

        while *pos < tokens.len() {
            let tok = &tokens[*pos];
            *pos += 1;

            match tok {
                WrapToken::Word(s) => {
                    spans.push(Span::Text(s.clone()));
                }
                WrapToken::HardBreak => {
                    spans.push(Span::Break);
                }
                WrapToken::Code(s) => {
                    spans.push(Span::InlineCode(s.clone()));
                }
                WrapToken::Math(s) => {
                    spans.push(Span::InlineMath(s.clone()));
                }
                WrapToken::RawHtml(s) => {
                    spans.push(Span::Html(s.clone()));
                }
                WrapToken::Img { alt, url, title } => {
                    spans.push(Span::Image {
                        alt: alt.clone(),
                        url: url.clone(),
                        title: title.clone(),
                    });
                }
                WrapToken::ImgRef {
                    alt,
                    reference_kind,
                    identifier,
                    label,
                } => {
                    spans.push(Span::ImageReference {
                        alt: alt.clone(),
                        reference_kind: *reference_kind,
                        identifier: identifier.clone(),
                        label: label.clone(),
                    });
                }
                WrapToken::FnRef { identifier, label } => {
                    spans.push(Span::FootnoteReference {
                        identifier: identifier.clone(),
                        label: label.clone(),
                    });
                }
                WrapToken::OpenEm => {
                    let children = parse_spans(tokens, pos, true);
                    spans.push(Span::Emphasis(children));
                }
                WrapToken::CloseEm => {
                    if stop_at_close {
                        return spans;
                    }
                }
                WrapToken::OpenStrong => {
                    let children = parse_spans(tokens, pos, true);
                    spans.push(Span::Strong(children));
                }
                WrapToken::CloseStrong => {
                    if stop_at_close {
                        return spans;
                    }
                }
                WrapToken::OpenDel => {
                    let children = parse_spans(tokens, pos, true);
                    spans.push(Span::Delete(children));
                }
                WrapToken::CloseDel => {
                    if stop_at_close {
                        return spans;
                    }
                }
                WrapToken::OpenLink { url, title, .. } => {
                    let children = parse_spans(tokens, pos, true);
                    spans.push(Span::Link {
                        url: url.clone(),
                        title: title.clone(),
                        children,
                    });
                }
                WrapToken::CloseLink => {
                    if stop_at_close {
                        return spans;
                    }
                }
                WrapToken::OpenLinkRef {
                    reference_kind,
                    identifier,
                    label,
                    ..
                } => {
                    let children = parse_spans(tokens, pos, true);
                    spans.push(Span::LinkReference {
                        reference_kind: *reference_kind,
                        identifier: identifier.clone(),
                        label: label.clone(),
                        children,
                    });
                }
                WrapToken::CloseLinkRef => {
                    if stop_at_close {
                        return spans;
                    }
                }
                WrapToken::SoftBreak => {
                    spans.push(Span::SoftBreak);
                }
            }
        }

        spans
    }

    parse_spans(tokens, &mut 0, false)
}

// ---------------------------------------------------------------------------
// AST walking and reflow
// ---------------------------------------------------------------------------

fn reflow_root_with_edge_widths(
    root: &mut Node,
    initial_width: usize,
    target_width: usize,
    final_width: usize,
    single_line_width: usize,
) {
    if let Some(children) = root.children_mut() {
        let mut initial_line_pending = initial_width != target_width
            || final_width != target_width
            || single_line_width != target_width;
        reflow_children(
            children,
            initial_width,
            target_width,
            final_width,
            single_line_width,
            0,
            &mut initial_line_pending,
        );
    }
}

fn reflow_children(
    children: &mut Vec<Node>,
    initial_width: usize,
    target_width: usize,
    final_width: usize,
    single_line_width: usize,
    prefix_width: usize,
    initial_line_pending: &mut bool,
) {
    for child in children.iter_mut() {
        reflow_node(
            child,
            initial_width,
            target_width,
            final_width,
            single_line_width,
            prefix_width,
            initial_line_pending,
        );
    }
}

fn reflow_node(
    node: &mut Node,
    initial_width: usize,
    target_width: usize,
    final_width: usize,
    single_line_width: usize,
    prefix_width: usize,
    initial_line_pending: &mut bool,
) {
    match node {
        // Paragraph: reflow phrasing content.
        Node::Paragraph(p) => {
            reflow_paragraph(
                &mut p.children,
                initial_width,
                target_width,
                final_width,
                single_line_width,
                prefix_width,
                initial_line_pending,
            );
        }
        // Blockquote: content is prefixed with "> " (2 chars) on every line.
        Node::Blockquote(_) => {
            if let Some(children) = node.children_mut() {
                reflow_children(
                    children,
                    initial_width,
                    target_width,
                    final_width,
                    single_line_width,
                    prefix_width + 2,
                    initial_line_pending,
                );
            }
        }
        // List: each item has a marker that adds to the prefix width.
        Node::List(list) => {
            let start = list.start.unwrap_or(1);
            for (idx, child) in list.children.iter_mut().enumerate() {
                let marker = if list.ordered {
                    format!("{}. ", start + idx as u32)
                } else {
                    "* ".to_string()
                };
                reflow_node(
                    child,
                    initial_width,
                    target_width,
                    final_width,
                    single_line_width,
                    prefix_width + marker.len(),
                    initial_line_pending,
                );
            }
        }
        // Container nodes: recurse into children.
        Node::Root(_)
        | Node::ListItem(_)
        | Node::FootnoteDefinition(_)
        | Node::Table(_)
        | Node::TableRow(_)
        | Node::TableCell(_)
        | Node::Heading(_)
        | Node::Link(_)
        | Node::LinkReference(_)
        | Node::Emphasis(_)
        | Node::Strong(_)
        | Node::Delete(_)
        | Node::MdxJsxFlowElement(_)
        | Node::MdxJsxTextElement(_) => {
            if let Some(children) = node.children_mut() {
                reflow_children(
                    children,
                    initial_width,
                    target_width,
                    final_width,
                    single_line_width,
                    prefix_width,
                    initial_line_pending,
                );
            }
        }
        // Leaf/void nodes: nothing to do.
        _ => {}
    }
}

/// Reflow a paragraph's children.
fn reflow_paragraph(
    children: &mut Vec<Node>,
    initial_width: usize,
    target_width: usize,
    final_width: usize,
    single_line_width: usize,
    prefix_width: usize,
    initial_line_pending: &mut bool,
) {
    // 1. Convert all children to spans.
    let spans: Vec<Span> = children.iter().flat_map(node_to_spans).collect();

    if spans.is_empty() {
        return;
    }

    // 2. Word-wrap the spans, accounting for the prefix that rendering will add.
    let available_width = target_width.saturating_sub(prefix_width);
    let final_available_width = final_width.saturating_sub(prefix_width);
    let wrapped = if *initial_line_pending {
        *initial_line_pending = false;
        wrap_spans_with_edge_widths(
            &spans,
            initial_width.saturating_sub(prefix_width),
            available_width,
            final_available_width,
            single_line_width.saturating_sub(prefix_width),
        )
    } else if final_available_width != available_width {
        wrap_spans_with_edge_widths(
            &spans,
            available_width,
            available_width,
            final_available_width,
            final_available_width,
        )
    } else {
        wrap_spans(&spans, available_width)
    };

    // 3. Merge adjacent SoftBreak + Text("\n") patterns and trim.
    let wrapped = coalesce_spans(&wrapped);

    // 4. Convert back to nodes.
    *children = spans_to_nodes(&wrapped);
}

/// Clean up the span list: merge adjacent Text spans, remove empty ones, etc.
fn coalesce_spans(spans: &[Span]) -> Vec<Span> {
    let mut result: Vec<Span> = Vec::new();

    for span in spans {
        match span {
            Span::Text(s) if s.is_empty() => continue,
            Span::Text(s) => {
                // Merge with previous Text span if present.
                if let Some(Span::Text(prev)) = result.last_mut() {
                    prev.push_str(s);
                } else {
                    result.push(Span::Text(s.clone()));
                }
            }
            Span::SoftBreak => {
                // Remove trailing whitespace before the soft break.
                if let Some(Span::Text(prev)) = result.last_mut() {
                    *prev = prev.trim_end().to_string();
                    if prev.is_empty() {
                        result.pop();
                    }
                }
                result.push(Span::SoftBreak);
            }
            other => result.push(other.clone()),
        }
    }

    // Trim leading/trailing whitespace on the whole line sequence.
    // Remove leading soft breaks.
    while result
        .first()
        .map_or(false, |s| matches!(s, Span::SoftBreak))
    {
        result.remove(0);
    }
    // Remove trailing soft breaks.
    while result
        .last()
        .map_or(false, |s| matches!(s, Span::SoftBreak))
    {
        result.pop();
    }
    // Trim leading whitespace in first text.
    if let Some(Span::Text(s)) = result.first_mut() {
        *s = s.trim_start().to_string();
        if s.is_empty() {
            result.remove(0);
        }
    }
    // Trim trailing whitespace in last text.
    if let Some(Span::Text(s)) = result.last_mut() {
        *s = s.trim_end().to_string();
        if s.is_empty() {
            result.pop();
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Markdown rendering
// ---------------------------------------------------------------------------

fn render_root(root: &Node, out: &mut String) {
    if let Some(children) = root.children() {
        render_flow_children(children, out);
    }
}

/// Render a list of flow-level children with appropriate spacing.
fn render_flow_children(children: &[Node], out: &mut String) {
    for (i, child) in children.iter().enumerate() {
        // Add blank line between block-level elements.
        if i > 0 && needs_blank_line_before(child) {
            out.push('\n');
        }
        render_node(child, out);
    }
}

/// Whether a blank line should be inserted before this node.
fn needs_blank_line_before(node: &Node) -> bool {
    matches!(
        node,
        Node::Paragraph(_)
            | Node::Heading(_)
            | Node::Blockquote(_)
            | Node::List(_)
            | Node::Code(_)
            | Node::ThematicBreak(_)
            | Node::Table(_)
            | Node::Math(_)
            | Node::Yaml(_)
            | Node::Toml(_)
            | Node::MdxjsEsm(_)
            | Node::MdxFlowExpression(_)
            | Node::MdxJsxFlowElement(_)
    )
}

fn render_node(node: &Node, out: &mut String) {
    match node {
        // ---- Block-level ----
        Node::Root(_) => {
            if let Some(children) = node.children() {
                render_flow_children(children, out);
            }
        }
        Node::Paragraph(p) => {
            render_phrasing(&p.children, out);
            out.push('\n');
        }
        Node::Heading(h) => {
            for _ in 0..h.depth {
                out.push('#');
            }
            out.push(' ');
            render_phrasing(&h.children, out);
            out.push('\n');
        }
        Node::Blockquote(bq) => {
            // Render each child with "> " prefix.
            // We handle multi-line blockquotes by rendering children
            // and prefixing each line.
            let mut inner = String::new();
            render_flow_children(&bq.children, &mut inner);
            for line in inner.lines() {
                out.push_str("> ");
                out.push_str(line);
                out.push('\n');
            }
        }
        Node::List(list) => {
            let start = list.start.unwrap_or(1);
            for (idx, item) in list.children.iter().enumerate() {
                if let Node::ListItem(li) = item {
                    let marker = if list.ordered {
                        format!("{}. ", start + idx as u32)
                    } else {
                        "* ".to_string()
                    };
                    render_list_item(li, &marker, out);
                }
            }
        }
        Node::Code(code) => {
            let lang = code.lang.as_deref().unwrap_or("");
            out.push_str("```");
            out.push_str(lang);
            if let Some(ref meta) = code.meta {
                out.push(' ');
                out.push_str(meta);
            }
            out.push('\n');
            out.push_str(&code.value);
            if !code.value.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("```\n");
        }
        Node::ThematicBreak(_) => {
            out.push_str("---\n");
        }
        Node::Html(h) => {
            out.push_str(&h.value);
            out.push('\n');
        }
        Node::Math(m) => {
            out.push_str("$$\n");
            out.push_str(&m.value);
            if !m.value.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("$$\n");
        }
        Node::Table(table) => {
            render_table(table, out);
        }
        Node::Definition(def) => {
            write!(out, "[{}]: {}", def.identifier, def.url).unwrap();
            if let Some(ref title) = def.title {
                write!(out, " \"{title}\"").unwrap();
            }
            out.push('\n');
        }
        Node::Yaml(y) => {
            out.push_str("---\n");
            out.push_str(&y.value);
            if !y.value.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("---\n");
        }
        Node::Toml(t) => {
            out.push_str("+++\n");
            out.push_str(&t.value);
            if !t.value.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("+++\n");
        }
        Node::MdxjsEsm(e) => {
            out.push_str(&e.value);
            out.push('\n');
        }
        Node::MdxFlowExpression(e) => {
            out.push_str("{");
            out.push_str(&e.value);
            out.push_str("}\n");
        }
        Node::MdxJsxFlowElement(el) => {
            render_mdx_jsx_flow(el, out);
        }

        // ---- Phrasing / inline (fallback for unexpected contexts) ----
        Node::Text(t) => {
            out.push_str(&t.value);
        }
        Node::Break(_) => {
            out.push_str("\\\n");
        }
        Node::InlineCode(ic) => {
            render_inline_code(&ic.value, out);
        }
        Node::InlineMath(im) => {
            out.push('$');
            out.push_str(&im.value);
            out.push('$');
        }
        Node::Emphasis(em) => {
            out.push('*');
            render_phrasing(&em.children, out);
            out.push('*');
        }
        Node::Strong(st) => {
            out.push_str("**");
            render_phrasing(&st.children, out);
            out.push_str("**");
        }
        Node::Delete(del) => {
            out.push_str("~~");
            render_phrasing(&del.children, out);
            out.push_str("~~");
        }
        Node::Link(link) => {
            out.push('[');
            render_phrasing(&link.children, out);
            out.push(']');
            out.push('(');
            out.push_str(&link.url);
            if let Some(ref title) = link.title {
                write!(out, " \"{title}\"").unwrap();
            }
            out.push(')');
        }
        Node::LinkReference(lr) => {
            out.push('[');
            render_phrasing(&lr.children, out);
            out.push(']');
            match lr.reference_kind {
                ReferenceKind::Full => {
                    out.push('[');
                    out.push_str(&lr.identifier);
                    out.push(']');
                }
                ReferenceKind::Collapsed => {
                    out.push_str("[]");
                }
                ReferenceKind::Shortcut => {
                    // No additional markup.
                }
            }
        }
        Node::Image(img) => {
            out.push_str("![");
            out.push_str(&img.alt);
            out.push(']');
            out.push('(');
            out.push_str(&img.url);
            if let Some(ref title) = img.title {
                write!(out, " \"{title}\"").unwrap();
            }
            out.push(')');
        }
        Node::ImageReference(ir) => {
            out.push_str("![");
            out.push_str(&ir.alt);
            out.push(']');
            match ir.reference_kind {
                ReferenceKind::Full => {
                    out.push('[');
                    out.push_str(&ir.identifier);
                    out.push(']');
                }
                ReferenceKind::Collapsed => {
                    out.push_str("[]");
                }
                ReferenceKind::Shortcut => {}
            }
        }
        Node::FootnoteReference(fr) => {
            out.push_str("[^");
            out.push_str(&fr.identifier);
            out.push(']');
        }
        Node::FootnoteDefinition(fd) => {
            out.push_str("[^");
            out.push_str(&fd.identifier);
            out.push_str("]: ");
            render_phrasing(&fd.children, out);
            out.push('\n');
        }
        Node::MdxTextExpression(e) => {
            out.push('{');
            out.push_str(&e.value);
            out.push('}');
        }
        Node::MdxJsxTextElement(el) => {
            render_mdx_jsx_text(el, out);
        }

        // Fallback: use debug representation.
        other => {
            write!(out, "{other:?}").unwrap();
        }
    }
}

/// Render phrasing content: join all children without separators.
fn render_phrasing(children: &[Node], out: &mut String) {
    for child in children {
        render_node(child, out);
    }
}

fn render_list_item(item: &ListItem, marker: &str, out: &mut String) {
    let indent = " ".repeat(marker.len());

    // Render the item's children to a string first.
    let mut inner = String::new();
    render_flow_children(&item.children, &mut inner);

    let mut lines = inner.lines().peekable();
    if let Some(first_line) = lines.next() {
        out.push_str(marker);
        out.push_str(first_line);
        out.push('\n');
        for line in lines {
            if line.is_empty() {
                out.push('\n');
            } else {
                out.push_str(&indent);
                out.push_str(line);
                out.push('\n');
            }
        }
    }
}

fn render_inline_code(value: &str, out: &mut String) {
    // Use the minimal number of backticks.
    let backtick_count = max_consecutive_backticks(value).max(1);
    let fence: String = "`".repeat(backtick_count);
    out.push_str(&fence);
    if backtick_count > 1 || value.starts_with('`') || value.ends_with('`') {
        out.push(' ');
    }
    out.push_str(value);
    if backtick_count > 1 || value.starts_with('`') || value.ends_with('`') {
        out.push(' ');
    }
    out.push_str(&fence);
}

fn max_consecutive_backticks(s: &str) -> usize {
    let mut max = 0;
    let mut current = 0;
    for ch in s.chars() {
        if ch == '`' {
            current += 1;
            max = max.max(current);
        } else {
            current = 0;
        }
    }
    max
}

fn render_table(table: &Table, out: &mut String) {
    // Collect rows with their cell alignments.
    let mut rows: Vec<&TableRow> = Vec::new();
    for child in &table.children {
        if let Node::TableRow(tr) = child {
            rows.push(tr);
        }
    }
    if rows.is_empty() {
        return;
    }

    // Determine column count from first row.
    let col_count = rows[0].children.len();

    // Render each row.
    for (i, row) in rows.iter().enumerate() {
        out.push_str("| ");
        for (j, cell) in row.children.iter().enumerate() {
            if j > 0 {
                out.push_str(" | ");
            }
            if let Node::TableCell(tc) = cell {
                render_phrasing(&tc.children, out);
            }
        }
        out.push_str(" |\n");

        // Render alignment row after header (first row).
        if i == 0 {
            out.push_str("| ");
            for j in 0..col_count {
                if j > 0 {
                    out.push_str(" | ");
                }
                let align = table.align.get(j).unwrap_or(&AlignKind::None);
                match align {
                    AlignKind::Left => out.push_str(":--"),
                    AlignKind::Right => out.push_str("--:"),
                    AlignKind::Center => out.push_str(":-:"),
                    AlignKind::None => out.push_str("---"),
                }
            }
            out.push_str(" |\n");
        }
    }
}

fn render_mdx_jsx_flow(el: &MdxJsxFlowElement, out: &mut String) {
    if let Some(ref name) = el.name {
        out.push('<');
        out.push_str(name);
        for attr in &el.attributes {
            out.push(' ');
            render_mdx_attr_content(attr, out);
        }
        if el.children.is_empty() {
            out.push_str(" />\n");
        } else {
            out.push_str(">\n");
            render_flow_children(&el.children, out);
            out.push_str("</");
            out.push_str(name);
            out.push_str(">\n");
        }
    }
}

fn render_mdx_jsx_text(el: &MdxJsxTextElement, out: &mut String) {
    if let Some(ref name) = el.name {
        out.push('<');
        out.push_str(name);
        for attr in &el.attributes {
            out.push(' ');
            render_mdx_attr_content(attr, out);
        }
        if el.children.is_empty() {
            out.push_str(" />");
        } else {
            out.push('>');
            render_phrasing(&el.children, out);
            out.push_str("</");
            out.push_str(name);
            out.push('>');
        }
    }
}

fn render_mdx_attr_content(attr: &AttributeContent, out: &mut String) {
    match attr {
        AttributeContent::Property(a) => {
            out.push_str(&a.name);
            if let Some(ref value) = a.value {
                out.push('=');
                match value {
                    AttributeValue::Literal(s) => {
                        out.push('"');
                        out.push_str(s);
                        out.push('"');
                    }
                    AttributeValue::Expression(e) => {
                        out.push('{');
                        out.push_str(&e.value);
                        out.push('}');
                    }
                }
            }
        }
        AttributeContent::Expression(e) => {
            out.push('{');
            out.push_str("...");
            out.push_str(&e.value);
            out.push('}');
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_paragraph_reflow() {
        let input = "This is a paragraph that needs to be reflowed to a narrower width.\n";
        let result = reflow(input, 40).unwrap();
        // Should have line breaks at appropriate places.
        for line in result.lines() {
            assert!(
                line.len() <= 40,
                "line too long ({} chars): {line:?}",
                line.len()
            );
        }
    }

    #[test]
    fn test_headings_not_combined() {
        let input = "# Hello\n# World\n";
        let result = reflow(input, 80).unwrap();
        assert!(
            result.contains("# Hello\n"),
            "expected '# Hello' heading: {result:?}"
        );
        assert!(
            result.contains("# World\n"),
            "expected '# World' heading: {result:?}"
        );
    }

    #[test]
    fn test_list_items_not_combined() {
        let input = "* First item\n* Second item\n";
        let result = reflow(input, 80).unwrap();
        assert!(result.contains("* First item\n"), "got: {result:?}");
        assert!(result.contains("* Second item\n"), "got: {result:?}");
    }

    #[test]
    fn test_inline_formatting_preserved() {
        let input = "This is **bold** and *italic* text.\n";
        let result = reflow(input, 80).unwrap();
        assert!(result.contains("**bold**"), "got: {result:?}");
        assert!(result.contains("*italic*"), "got: {result:?}");
    }

    #[test]
    fn test_code_block_preserved() {
        let input = "```rust\nlet x = 1;\nlet y = 2;\n```\n";
        let result = reflow(input, 80).unwrap();
        assert!(
            result.contains("```rust\nlet x = 1;\nlet y = 2;\n```\n"),
            "got: {result:?}"
        );
    }

    #[test]
    fn test_blockquote_preserved() {
        let input = "> A blockquote with some text.\n";
        let result = reflow(input, 80).unwrap();
        assert!(
            result.contains("> A blockquote with some text.\n"),
            "got: {result:?}"
        );
    }

    #[test]
    fn test_reflow_narrow() {
        let input = "The quick brown fox jumps over the lazy dog.\n";
        let result = reflow(input, 20).unwrap();
        for line in result.lines() {
            assert!(line.len() <= 20, "line too long: {line:?}");
        }
        // The text should be split into multiple lines.
        assert!(
            result.lines().count() > 1,
            "expected multiple lines: {result:?}"
        );
    }

    #[test]
    fn test_reflow_preserves_emphasis_across_lines() {
        let input =
            "This is a long paragraph with **bold text that might need to span** across lines.\n";
        let result = reflow(input, 40).unwrap();
        // The bold text might be split across lines; this is valid markdown.
        // Count total ** markers in the whole output — should be balanced.
        let total_open = result.matches("**").count();
        assert_eq!(total_open % 2, 0, "unbalanced ** total: {result:?}");
        // Text content should remain the same.
        assert!(result.contains("**bold"), "missing bold open: {result:?}");
        assert!(result.contains("span**"), "missing close bold: {result:?}");
    }

    // -----------------------------------------------------------------------
    // Edge cases & real-world scenarios
    // -----------------------------------------------------------------------

    #[test]
    fn test_links_preserved() {
        let input = "See [the docs](https://example.com) for more.\n";
        let result = reflow(input, 80).unwrap();
        assert!(
            result.contains("[the docs](https://example.com)"),
            "got: {result:?}"
        );
    }

    #[test]
    fn test_link_with_inline_formatting() {
        let input = "See **[the docs](https://example.com)** for more.\n";
        let result = reflow(input, 80).unwrap();
        assert!(
            result.contains("[the docs](https://example.com)"),
            "got: {result:?}"
        );
        assert!(result.contains("**"), "missing strong: {result:?}");
    }

    #[test]
    fn test_ordered_list_preserved() {
        let input = "1. First\n2. Second\n3. Third\n";
        let result = reflow(input, 80).unwrap();
        assert!(result.contains("1. First\n"), "got: {result:?}");
        assert!(result.contains("2. Second\n"), "got: {result:?}");
        assert!(result.contains("3. Third\n"), "got: {result:?}");
    }

    #[test]
    fn test_nested_blockquote() {
        let input = "> > Nested quote\n";
        let result = reflow(input, 80).unwrap();
        assert!(result.contains("> > Nested quote\n"), "got: {result:?}");
    }

    #[test]
    fn test_thematic_break_preserved() {
        let input = "Before\n\n---\n\nAfter\n";
        let result = reflow(input, 80).unwrap();
        assert!(result.contains("---\n"), "got: {result:?}");
        assert!(result.contains("Before\n"), "got: {result:?}");
        assert!(result.contains("After\n"), "got: {result:?}");
    }

    #[test]
    fn test_image_preserved() {
        let input = "Look at ![alt text](image.png).\n";
        let result = reflow(input, 80).unwrap();
        assert!(result.contains("![alt text](image.png)"), "got: {result:?}");
    }

    #[test]
    fn test_inline_code_not_split() {
        // Inline code is atomic — even at narrow widths it stays intact.
        let input = "Run `cargo build --release` now.\n";
        let result = reflow(input, 25).unwrap();
        // The inline code must appear in full somewhere on one line.
        let on_one_line = result
            .lines()
            .any(|l| l.contains("`cargo build --release`"));
        assert!(on_one_line, "inline code was split: {result:?}");
    }

    #[test]
    fn test_hard_break_preserved() {
        let input = "Line one.\\\nLine two.\n";
        let result = reflow(input, 80).unwrap();
        assert!(result.contains("\\\n"), "hard break lost: {result:?}");
    }

    #[test]
    fn test_mixed_bold_italic() {
        let input = "This is ***bold and italic*** and **just bold** and *just italic*.\n";
        let result = reflow(input, 80).unwrap();
        assert!(result.contains("***bold and italic***"), "got: {result:?}");
        assert!(result.contains("**just bold**"), "got: {result:?}");
        assert!(result.contains("*just italic*"), "got: {result:?}");
    }

    #[test]
    fn test_multiple_paragraphs_blank_line() {
        let input = "First paragraph.\n\nSecond paragraph.\n";
        let result = reflow(input, 80).unwrap();
        // Must be at least one blank line between paragraphs.
        assert!(
            result.contains("\n\n"),
            "no blank line between paragraphs: {result:?}"
        );
    }

    #[test]
    fn test_double_space_sentence_end_preserved() {
        let input = "First sentence.  Second sentence.\n";
        let result = reflow(input, 80).unwrap();
        assert!(result.contains(".  "), "double space lost: {result:?}");
    }

    #[test]
    fn test_unicode_preserved() {
        let input = "Café résumé naïve — très élégant.\n";
        let result = reflow(input, 80).unwrap();
        assert!(result.contains("Café"), "got: {result:?}");
        assert!(result.contains("résumé"), "got: {result:?}");
        assert!(result.contains("—"), "em-dash lost: {result:?}");
    }

    #[test]
    fn test_empty_paragraph() {
        let input = "Before\n\n\n\nAfter\n";
        let result = reflow(input, 80).unwrap();
        assert!(result.contains("Before\n"), "got: {result:?}");
        assert!(result.contains("After\n"), "got: {result:?}");
    }

    #[test]
    fn test_very_long_word_force_break() {
        // A word longer than the target width must still be emitted.
        let input = "AReallyLongWordThatExceedsTheTargetWidth significantly.\n";
        let result = reflow(input, 20).unwrap();
        // The long word appears in the output — it just gets its own line.
        assert!(
            result.contains("AReallyLongWordThatExceedsTheTargetWidth"),
            "long word lost: {result:?}"
        );
    }

    #[test]
    fn test_reflow_to_wider_width_reduces_lines() {
        let input =
            "This paragraph is wrapped tightly.\nIt has several lines that could\nbe merged.\n";
        let narrow = reflow(input, 30).unwrap();
        let wide = reflow(input, 200).unwrap();
        assert!(
            wide.lines().count() < narrow.lines().count(),
            "wider width should produce fewer lines"
        );
    }

    #[test]
    fn test_reflow_at_small_width() {
        let input = "Short word.\n";
        // At width 6, the line should be <= 6 chars.
        let result = reflow(input, 6).unwrap();
        for line in result.lines() {
            assert!(line.chars().count() <= 6, "line too long: {line:?}");
        }
    }

    #[test]
    fn test_yaml_frontmatter_preserved() {
        let input = "---\ntitle: Test\nauthor: Me\n---\n\nParagraph.\n";
        let result = reflow(input, 80).unwrap();
        assert!(result.contains("---\n"), "got: {result:?}");
        assert!(result.contains("title: Test\n"), "got: {result:?}");
        assert!(result.contains("Paragraph.\n"), "got: {result:?}");
    }

    #[test]
    fn test_definition_preserved() {
        let input = "[ref]: https://example.com \"Title\"\n\nSee [ref] for more.\n";
        let result = reflow(input, 80).unwrap();
        assert!(
            result.contains("[ref]: https://example.com"),
            "got: {result:?}"
        );
    }

    #[test]
    fn test_footnote_preserved() {
        let input = "Text with footnote[^1].\n\n[^1]: The footnote.\n";
        let result = reflow(input, 80).unwrap();
        assert!(result.contains("[^1]"), "footnote ref lost: {result:?}");
    }

    #[test]
    fn test_table_preserved() {
        let input = "| A | B |\n| --- | --- |\n| 1 | 2 |\n";
        let result = reflow(input, 80).unwrap();
        assert!(result.contains("| A | B |\n"), "got: {result:?}");
        assert!(result.contains("| --- | --- |\n"), "got: {result:?}");
    }

    #[test]
    fn test_list_item_with_nested_paragraph() {
        let input = "* Item one.\n\n  Paragraph under item.\n* Item two.\n";
        let result = reflow(input, 80).unwrap();
        assert!(result.contains("* Item one.\n"), "got: {result:?}");
        assert!(
            result.contains("Paragraph under item.\n"),
            "got: {result:?}"
        );
        assert!(result.contains("* Item two.\n"), "got: {result:?}");
    }

    #[test]
    fn test_line_width_hard_cap() {
        // At various widths, no line (excluding code blocks) exceeds the cap.
        let input = "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat.\n";
        for width in [20, 40, 60, 80, 100] {
            let result = reflow(input, width).unwrap();
            for line in result.lines() {
                assert!(
                    line.chars().count() <= width,
                    "width {width}: line too long ({chars}): {line:?}",
                    chars = line.chars().count()
                );
            }
        }
    }

    #[test]
    fn test_delete_formatting_preserved() {
        let input = "This is ~~deleted~~ text.\n";
        let result = reflow(input, 80).unwrap();
        assert!(result.contains("~~deleted~~"), "got: {result:?}");
    }

    #[test]
    fn test_no_trailing_whitespace_on_lines() {
        let input = "A paragraph that gets reflowed to a specific width.\n";
        let result = reflow(input, 30).unwrap();
        for line in result.lines() {
            assert!(
                !line.ends_with(' ') && !line.ends_with('\t'),
                "trailing whitespace: {line:?}"
            );
        }
    }

    #[test]
    fn test_force_break_unbreakable_word() {
        // A word longer than the target width gets its own line.
        let input = "Supercalifragilisticexpialidocious.\n";
        let result = reflow(input, 20).unwrap();
        // The long word must appear intact somewhere.
        assert!(
            result.contains("Supercalifragilisticexpialidocious"),
            "long word lost: {result:?}"
        );
    }

    #[test]
    fn test_list_items_respect_width() {
        // List items should not exceed target_width after adding the bullet marker.
        let input = "* This is a bullet point that should be reflowed to stay within the character limit.\n";
        for width in [20, 40, 60, 80] {
            let result = reflow(input, width).unwrap();
            for line in result.lines() {
                assert!(
                    line.chars().count() <= width,
                    "width {width}: line too long ({chars}): {line:?}",
                    chars = line.chars().count()
                );
            }
        }
    }

    #[test]
    fn test_ordered_list_items_respect_width() {
        let input = "1. This is an ordered bullet point that should be reflowed to stay within the character limit.\n";
        for width in [20, 40, 60, 80] {
            let result = reflow(input, width).unwrap();
            for line in result.lines() {
                assert!(
                    line.chars().count() <= width,
                    "width {width}: line too long ({chars}): {line:?}",
                    chars = line.chars().count()
                );
            }
        }
    }

    #[test]
    fn test_blockquote_respects_width() {
        let input =
            "> This is a blockquote that should be reflowed to stay within the character limit.\n";
        for width in [20, 40, 60, 80] {
            let result = reflow(input, width).unwrap();
            for line in result.lines() {
                assert!(
                    line.chars().count() <= width,
                    "width {width}: line too long ({chars}): {line:?}",
                    chars = line.chars().count()
                );
            }
        }
    }
}
