use markdown::mdast::*;
use markdown::unist::Position;
use std::fmt::Write;

/// Whether to wrap, join soft breaks, or leave existing line breaks alone.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ReflowWidth {
    /// Wrap at this many characters. Zero is a literal zero-width budget;
    /// indivisible content such as a word can still exceed the budget.
    Finite(usize),
    /// Join existing soft breaks without inserting new ones.
    #[default]
    Infinite,
    /// Skip reflow while still applying parsing and rendering transformations.
    Disabled,
}

impl ReflowWidth {
    fn saturating_sub(self, prefix_width: usize) -> Self {
        match self {
            Self::Finite(width) => Self::Finite(width.saturating_sub(prefix_width)),
            other => other,
        }
    }

    fn is_exceeded_by(self, width: usize) -> bool {
        matches!(self, Self::Finite(limit) if width > limit)
    }
}

/// Available content widths for each position in the rendered output.
///
/// `Infinite` leaves a width unconstrained. An infinite `middle` joins soft
/// breaks without wrapping and ignores the other budgets. Setting any budget
/// to `Disabled` skips reflow for the entire document, preserving soft breaks
/// except those before a line that would start with block syntax.
/// Other parsing and rendering transformations still apply in all modes.
/// Widths count characters, including Markdown delimiters but excluding
/// prefixes supplied by the caller.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EdgeWidths {
    /// Budget for the first rendered content line.
    pub first: ReflowWidth,
    /// Budget for continuation lines; `Infinite` disables width-based wrapping.
    pub middle: ReflowWidth,
    /// Budget for the last rendered content line.
    pub last: ReflowWidth,
    /// Budget when the content occupies a single rendered line.
    pub single: ReflowWidth,
}

impl EdgeWidths {
    fn reflow_disabled(self) -> bool {
        [self.first, self.middle, self.last, self.single].contains(&ReflowWidth::Disabled)
    }
}

/// Reflow configuration. Defaults to unlimited width, no linkification, and
/// frontmatter recognition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub widths: EdgeWidths,
    /// Turn bare URL, `www.`, and email autolink literals into Markdown links.
    /// Off by default: plain URLs pass through as ordinary text.
    pub linkify_urls: bool,
    /// Recognize YAML (`---`) and TOML (`+++`) frontmatter at the start of the
    /// input and copy it unchanged. On by default. Turn it off for Markdown
    /// that is not a standalone document, where such leading lines are a
    /// thematic break, a setext underline, or paragraph text.
    pub frontmatter: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            widths: EdgeWidths::default(),
            linkify_urls: false,
            frontmatter: true,
        }
    }
}

/// Reflow Markdown using the supplied configuration.
///
/// Paragraphs are reflowed while block structure and explicit hard breaks are
/// preserved, and the output parses to the same Markdown as the input apart
/// from soft break placement: text keeps its escapes and character
/// references. Inserted line breaks never start a line with block syntax, such
/// as a list marker, and never split text that had no whitespace; a line
/// overflows its budget instead. Recognized YAML and TOML frontmatter contents
/// are kept without reflow or linkification. With the default options,
/// existing soft breaks are joined without inserting new ones. To wrap
/// uniformly, set all four width budgets alike.
/// Set any budget to `ReflowWidth::Disabled` to skip reflow while retaining
/// other transformations, such as linkification and table formatting.
///
/// ```
/// use markdown_reflow::{reflow, EdgeWidths, Options, ReflowWidth};
///
/// let options = Options {
///     widths: EdgeWidths {
///         first: ReflowWidth::Finite(10),
///         middle: ReflowWidth::Finite(10),
///         last: ReflowWidth::Finite(10),
///         single: ReflowWidth::Finite(10),
///     },
///     ..Options::default()
/// };
/// assert_eq!(reflow("one two three", options)?, "one two\nthree\n");
/// # Ok::<(), String>(())
/// ```
pub fn reflow(input: &str, reflow_options: Options) -> Result<String, String> {
    let mut ast =
        markdown::to_mdast(input, &parse_options(reflow_options)).map_err(|e| e.to_string())?;

    restore_source(&mut ast, input);
    reflow_root(&mut ast, reflow_options.widths, input);

    let mut output = String::new();
    render_root(&ast, input, &mut output);
    Ok(output)
}

fn parse_options(reflow_options: Options) -> markdown::ParseOptions {
    markdown::ParseOptions {
        constructs: markdown::Constructs {
            frontmatter: reflow_options.frontmatter,
            gfm_table: true,
            gfm_strikethrough: true,
            gfm_footnote_definition: true,
            gfm_label_start_footnote: true,
            // When linkification is off, bare URLs stay plain text and are
            // rendered back verbatim.
            gfm_autolink_literal: reflow_options.linkify_urls,
            gfm_task_list_item: true,
            ..markdown::Constructs::default()
        },
        ..markdown::ParseOptions::default()
    }
}

// ---------------------------------------------------------------------------
// Source text: escapes, character references, and delimiters
// ---------------------------------------------------------------------------

/// Restore the source spelling of text before reflow. The syntax tree stores
/// decoded text, so rendering its values would drop backslash escapes and
/// character references, turning `\*` into emphasis or `-\/` into `-/`. Text
/// values become Markdown source with container prefixes and line edge
/// whitespace removed, and the renderer copies them verbatim.
fn restore_source(node: &mut Node, src: &str) {
    match node {
        Node::Link(_) if is_literal_autolink(node, src) => {
            // A bare URL is rendered as a link with brackets, unless its
            // text would not parse back as plain link text, or holds
            // backticks, which the destination would repeat for an unmatched
            // backtick to pair with.
            let plain = matches!(node, Node::Link(link)
                if link_text(link).is_some_and(|text| is_plain_link_text(&text)));
            if !plain {
                *node = Node::Text(Text {
                    value: source(node, src).unwrap_or_default().to_string(),
                    position: node.position().cloned(),
                });
            }
        }
        _ => {
            if matches!(node, Node::Link(_) | Node::LinkReference(_)) {
                // Links cannot contain links, but the autolink literal
                // extension finds URLs in link text; keep their text only.
                if let Some(children) = node.children_mut() {
                    unwrap_links(children);
                }
            }
            let delimiters = matches!(node, Node::Emphasis(_) | Node::Strong(_) | Node::Delete(_))
                .then(|| delimiter(node, src).to_string());
            if let Some(children) = node.children_mut() {
                // The end of the previous sibling's source.
                let mut covered = 0;
                for child in children.iter_mut() {
                    if let Node::Text(text) = child {
                        text.value = source_text(src, text.position.as_ref(), &text.value, covered);
                    } else {
                        restore_source(child, src);
                    }
                    if let Some(position) = child.position() {
                        covered = position.end.offset;
                    }
                }
                let delimiters = delimiters.as_deref();
                keep_bare_urls_by_delimiters(children, delimiters, delimiters, src);
            }
        }
    }
}

/// Keep a bare URL as text where the brackets of a link would change how
/// its neighbors parse: after `!`, which would make an image, a backslash,
/// which would escape the bracket, or `]`, which would make a full
/// reference, and next to an emphasis or strikethrough delimiter, whose
/// flanking depends on whether it touches a letter or punctuation. `open`
/// and `close` are the delimiters of the parent around `children`.
fn keep_bare_urls_by_delimiters(
    children: &mut [Node],
    open: Option<&str>,
    close: Option<&str>,
    src: &str,
) {
    let rendered = |node: &Node| {
        let mut out = String::new();
        render_inline(node, src, &mut out);
        out
    };
    for index in 0..children.len() {
        if !is_literal_autolink(&children[index], src) {
            continue;
        }
        let before = match index.checked_sub(1) {
            Some(previous) => rendered(&children[previous]).chars().next_back(),
            None => open.and_then(|open| open.chars().next_back()),
        };
        let after = match children.get(index + 1) {
            Some(next) => rendered(next).chars().next(),
            None => close.and_then(|close| close.chars().next()),
        };
        if before.is_some_and(|c| "*_~!\\]".contains(c)) || after.is_some_and(|c| "*_~".contains(c))
        {
            let value = source(&children[index], src)
                .unwrap_or_default()
                .to_string();
            let position = children[index].position().cloned();
            children[index] = Node::Text(Text { value, position });
        }
    }
}

/// Whether `value` parses back unchanged as the text of a link. Inside
/// brackets, characters such as `*` or `]` could become emphasis or end the
/// link text.
fn is_plain_link_text(value: &str) -> bool {
    if value.contains('`') {
        return false;
    }
    let markdown = format!("[{value}](u)");
    let options = Options {
        frontmatter: false,
        ..Options::default()
    };
    let tree = markdown::to_mdast(&markdown, &parse_options(options));
    let Ok(tree) = tree else {
        return false;
    };
    let Some([Node::Paragraph(paragraph)]) = tree.children().map(Vec::as_slice) else {
        return false;
    };
    let [Node::Link(link)] = paragraph.children.as_slice() else {
        return false;
    };
    link_text(link).is_some_and(|text| text == value)
}

/// The text of a link whose children are all text.
fn link_text(link: &Link) -> Option<String> {
    link.children
        .iter()
        .map(|child| match child {
            Node::Text(text) => Some(text.value.as_str()),
            _ => None,
        })
        .collect()
}

/// Replace links among `children` and their descendants with their text.
fn unwrap_links(children: &mut Vec<Node>) {
    let mut index = 0;
    while index < children.len() {
        if let Node::Link(link) = &mut children[index] {
            let text = std::mem::take(&mut link.children);
            children.splice(index..=index, text);
        } else {
            if let Some(grandchildren) = children[index].children_mut() {
                unwrap_links(grandchildren);
            }
            index += 1;
        }
    }
}

/// The source of `node`, if it is known.
fn source<'a>(node: &Node, src: &'a str) -> Option<&'a str> {
    position_source(node.position(), src)
}

fn position_source<'a>(position: Option<&Position>, src: &'a str) -> Option<&'a str> {
    let position = position?;
    src.get(position.start.offset..position.end.offset)
}

/// The source of `node` when it lies on one line, so that copying it does
/// not drop container prefixes.
fn single_line_source<'a>(node: &Node, src: &'a str) -> Option<&'a str> {
    source(node, src).filter(|source| !source.contains(['\n', '\r']))
}

/// Whether `node` is an autolink such as `<https://example.com>`.
fn is_autolink(node: &Node, src: &str) -> bool {
    matches!(node, Node::Link(_)) && source(node, src).is_some_and(|s| s.starts_with('<'))
}

/// Whether `node` is a bare URL recognized by the autolink literal extension.
fn is_literal_autolink(node: &Node, src: &str) -> bool {
    matches!(node, Node::Link(_)) && source(node, src).is_some_and(|s| !s.starts_with(['[', '<']))
}

/// The Markdown source of a text node whose decoded value is `value`, where
/// the source before `covered` belongs to earlier nodes. Falls back to
/// escaping every ASCII punctuation character when the source is unknown or
/// does not decode to `value`.
fn source_text(src: &str, position: Option<&Position>, value: &str, covered: usize) -> String {
    position
        .and_then(|position| {
            // The parser starts text that begins with a backslash escape
            // after the backslash.
            let mut start = position.start.offset;
            let escaped = start
                .checked_sub(1)
                .is_some_and(|before| before >= covered && src.as_bytes()[before] == b'\\')
                && src
                    .as_bytes()
                    .get(start)
                    .is_some_and(u8::is_ascii_punctuation);
            if escaped {
                start -= 1;
            }
            src.get(start..position.end.offset)
        })
        .and_then(|source| align_source(source, value))
        .unwrap_or_else(|| {
            let mut escaped = String::new();
            push_escaped(value, |c| c.is_ascii_punctuation(), &mut escaped);
            escaped
        })
}

/// Walk `source` alongside its decoded `value`, keeping escapes and character
/// references and dropping what the parser removed: container prefixes and
/// whitespace at the edges of lines. Line endings become `\n`.
fn align_source(source: &str, value: &str) -> Option<String> {
    // Text that the parser did not decode, such as a bare URL, is its source.
    if source == value {
        return Some(source.to_string());
    }
    let mut out = String::with_capacity(source.len());
    let mut value = value;
    let mut rest = source;
    let mut first = true;
    loop {
        let line_len = rest.find(['\r', '\n']).unwrap_or(rest.len());
        let line = &rest[..line_len];
        // A later line starts with a container prefix and indentation,
        // whose length is unknown, since content can also start with `>`.
        let prefix_max = if first {
            0
        } else {
            line.len() - line.trim_start_matches([' ', '\t', '>']).len()
        };
        let last = line_len == rest.len();
        let (aligned, remaining) = (0..=prefix_max)
            .rev()
            .find_map(|prefix| align_line(&line[prefix..], value, last))?;
        out.push_str(&aligned);
        value = remaining;
        if last {
            break;
        }
        rest = &rest[line_len + line_ending_len(&rest[line_len..])?..];
        value = &value[line_ending_len(value)?..];
        out.push('\n');
        first = false;
    }
    value.is_empty().then_some(out)
}

/// Align one source line, after its prefix, with the start of `value`. The
/// line must decode to the text before a line ending in `value`, or to all
/// of it on the `last` line, except for trailing whitespace, which the
/// parser drops. Returns the line's source and the rest of `value`.
fn align_line<'a>(line: &str, mut value: &'a str, last: bool) -> Option<(String, &'a str)> {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(c) = rest.chars().next() {
        let (len, decoded) = decoded_prefix(rest)
            .unwrap_or_else(|| (c.len_utf8(), rest[..c.len_utf8()].to_string()));
        if let Some(remaining) = value.strip_prefix(decoded.as_str()) {
            out.push_str(&rest[..len]);
            value = remaining;
            rest = &rest[len..];
        } else if rest.trim_end_matches([' ', '\t']).is_empty() {
            break;
        } else {
            return None;
        }
    }
    let ends = if last {
        value.is_empty()
    } else {
        line_ending_len(value).is_some()
    };
    ends.then_some((out, value))
}

fn line_ending_len(s: &str) -> Option<usize> {
    if s.starts_with("\r\n") {
        Some(2)
    } else if s.starts_with(['\r', '\n']) {
        Some(1)
    } else {
        None
    }
}

/// The length and decoded text of a backslash escape or character reference
/// at the start of `s`.
fn decoded_prefix(s: &str) -> Option<(usize, String)> {
    match s.as_bytes() {
        [b'\\', escaped, ..] if escaped.is_ascii_punctuation() => {
            Some((2, char::from(*escaped).to_string()))
        }
        [b'&', ..] => character_reference(s),
        _ => None,
    }
}

/// The length and decoded text of the character reference, such as `&amp;`
/// or `&#47;`, at the start of `s`.
fn character_reference(s: &str) -> Option<(usize, String)> {
    let rest = s.strip_prefix('&')?;
    let (marker_len, radix, max_len) = match rest.as_bytes() {
        [b'#', b'x' | b'X', ..] => (2, 16, 6),
        [b'#', ..] => (1, 10, 7),
        _ => (0, 0, 31),
    };
    let body = &rest[marker_len..];
    let len = body
        .bytes()
        .take_while(|b| match radix {
            16 => b.is_ascii_hexdigit(),
            10 => b.is_ascii_digit(),
            _ => b.is_ascii_alphanumeric(),
        })
        .count();
    if len == 0 || len > max_len || body.as_bytes().get(len) != Some(&b';') {
        return None;
    }
    let decoded = if radix == 0 {
        markdown::decode_named(&body[..len], true)?
    } else {
        markdown::decode_numeric(&body[..len], radix)
    };
    Some((1 + marker_len + len + 1, decoded))
}

/// Append `value` with backslash escapes so that it parses back unchanged:
/// before each character for which `special` holds, before a backslash that
/// would otherwise escape the next character or a closing delimiter, and
/// before an `&` that would start a character reference.
fn push_escaped(value: &str, special: impl Fn(char) -> bool, out: &mut String) {
    for (index, c) in value.char_indices() {
        let next = value[index + c.len_utf8()..].chars().next();
        if special(c)
            || (c == '\\' && next.is_none_or(|next| next.is_ascii_punctuation()))
            || (c == '&' && character_reference(&value[index..]).is_some())
        {
            out.push('\\');
        }
        out.push(c);
    }
}

/// The delimiter that opened emphasis, strong emphasis, or strikethrough in
/// the source. Keeping `_` and single `~` delimiters avoids new delimiter
/// runs, such as `**` from `_` inside `*`, that would parse differently.
fn delimiter<'a>(node: &Node, src: &'a str) -> &'a str {
    // The fallback, and the delimiter's length, or `None` for the whole run.
    let (fallback, len) = match node {
        Node::Emphasis(_) => ("*", Some(1)),
        Node::Strong(_) => ("**", Some(2)),
        _ => ("~~", None),
    };
    let markers: &[u8] = if fallback.starts_with('~') {
        b"~"
    } else {
        b"*_"
    };
    let rest = node
        .position()
        .and_then(|position| src.get(position.start.offset..))
        .unwrap_or_default();
    let Some(&marker) = rest.as_bytes().first().filter(|b| markers.contains(b)) else {
        return fallback;
    };
    let run = rest.bytes().take_while(|&b| b == marker).count();
    let len = len.unwrap_or(run);
    if run < len || len > 2 {
        return fallback;
    }
    &rest[..len]
}

// ---------------------------------------------------------------------------
// Span: a flat representation of inline (phrasing) content
// ---------------------------------------------------------------------------

/// A piece of inline content.
#[derive(Clone, Debug)]
enum Span {
    /// Markdown source text.
    Text(String),
    /// A hard line break (backslash-newline or two-spaces-newline in source).
    Break(Break),
    /// An inline node that cannot be split across lines, such as a code
    /// span, inline HTML, an image, an autolink, or a footnote reference.
    Void(Node),
    /// Emphasis, strong emphasis, strikethrough, a link, or a link reference:
    /// the node without its children, which follow as spans. The node keeps
    /// its position, which locates its source delimiters.
    Container(Node, Vec<Span>),
    /// A soft break we insert during reflow.
    SoftBreak,
}

// ---------------------------------------------------------------------------
// Converting AST phrasing children ⇄ Spans
// ---------------------------------------------------------------------------

fn node_into_spans(mut node: Node, src: &str, spans: &mut Vec<Span>) {
    match node {
        Node::Text(text) => {
            if !text.value.is_empty() {
                spans.push(Span::Text(text.value));
            }
        }
        Node::Break(hard_break) => spans.push(Span::Break(hard_break)),
        Node::Link(_) if is_autolink(&node, src) => spans.push(Span::Void(node)),
        Node::Emphasis(_)
        | Node::Strong(_)
        | Node::Delete(_)
        | Node::Link(_)
        | Node::LinkReference(_) => {
            let mut children = Vec::new();
            for child in node.children_mut().map(std::mem::take).unwrap_or_default() {
                node_into_spans(child, src, &mut children);
            }
            spans.push(Span::Container(node, children));
        }
        _ => spans.push(Span::Void(node)),
    }
}

fn spans_into_nodes(spans: Vec<Span>) -> Vec<Node> {
    spans.into_iter().map(span_into_node).collect()
}

fn span_into_node(span: Span) -> Node {
    match span {
        Span::Text(value) => Node::Text(Text {
            value,
            position: None,
        }),
        Span::Break(hard_break) => Node::Break(hard_break),
        Span::Void(node) => node,
        Span::Container(mut node, children) => {
            if let Some(slot) = node.children_mut() {
                *slot = spans_into_nodes(children);
            }
            node
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
    /// Visible text: a word, or a run of spaces that is a break opportunity.
    Word(String),
    /// Text whose spaces must not become line breaks.
    Unbreakable(String),
    /// Hard line break.
    HardBreak(Break),
    /// An atomic inline node.
    Void(Node),
    /// Opening delimiter of a container span, whose node has no children.
    Open(Node),
    /// Closing delimiter of a container span, such as a link destination.
    Close(Node),
    /// A soft break we insert.
    SoftBreak,
}

impl WrapToken {
    /// Append the Markdown source this token renders to. A hard break renders
    /// only what precedes its line ending, which takes no column.
    fn render(&self, src: &str, out: &mut String) {
        match self {
            WrapToken::Word(s) | WrapToken::Unbreakable(s) => out.push_str(s),
            WrapToken::HardBreak(hard_break) => {
                out.push_str(render_hard_break(hard_break, out, src));
            }
            WrapToken::Void(node) => render_inline(node, src, out),
            WrapToken::Open(node) => render_open(node, src, out),
            WrapToken::Close(node) => render_close(node, src, out),
            WrapToken::SoftBreak => {}
        }
    }

    /// Whitespace between words: the only place a line break may be inserted.
    /// It contains a newline where the source had a line break.
    fn is_space(&self) -> bool {
        matches!(self, WrapToken::Word(s) if s.bytes().all(|b| matches!(b, b' ' | b'\n')))
    }
}

/// Flatten spans into a token stream, splitting compound spans into
/// open/close markers so the word-wrapper can break inside them.
fn tokenize(spans: Vec<Span>, breakable: bool, out: &mut Vec<WrapToken>) {
    for span in spans {
        tokenize_span(span, breakable, out);
    }
}

fn tokenize_span(span: Span, breakable: bool, out: &mut Vec<WrapToken>) {
    match span {
        // The parser includes container prefixes in a reference label that
        // spans lines, so the text of a reference used as its own label stays
        // on one line.
        Span::Text(s) if !breakable => {
            let normalized = s.replace(|c: char| c.is_ascii_whitespace(), " ");
            out.push(WrapToken::Unbreakable(normalized));
        }
        Span::Text(s) => {
            // Normalise Markdown whitespace (tabs, etc.) to spaces, keeping
            // line breaks, then split into words and whitespace runs. Other
            // Unicode whitespace, such as a no-break space, belongs to its
            // word.
            let normalized: String = s
                .chars()
                .map(|c| match c {
                    '\n' => '\n',
                    c if c.is_ascii_whitespace() => ' ',
                    c => c,
                })
                .collect();
            let is_space = |c: char| matches!(c, ' ' | '\n');
            let mut current = String::new();
            for ch in normalized.chars() {
                if is_space(ch) {
                    if !current.is_empty() && !current.ends_with(is_space) {
                        out.push(WrapToken::Word(current.clone()));
                        current.clear();
                    }
                    current.push(ch);
                } else {
                    if current.ends_with(is_space) {
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
        Span::Break(hard_break) => out.push(WrapToken::HardBreak(hard_break)),
        Span::Void(node) => out.push(WrapToken::Void(node)),
        Span::Container(node, children) => {
            let label = matches!(&node, Node::LinkReference(reference)
                if reference.reference_kind != ReferenceKind::Full);
            out.push(WrapToken::Open(node.clone()));
            tokenize(children, breakable && !label, out);
            out.push(WrapToken::Close(node));
        }
        Span::SoftBreak => out.push(WrapToken::SoftBreak),
    }
}

/// An unbreakable run of tokens. Lines break only in the whitespace before a
/// chunk, so no break separates tokens that were not separated by whitespace.
struct Chunk {
    /// Whitespace separating this chunk from the previous one; it disappears
    /// when a line break replaces it.
    space: String,
    tokens: Vec<WrapToken>,
    /// Rendered Markdown source, used to measure the chunk and to detect
    /// block syntax that a line break would expose.
    text: String,
    width: usize,
}

impl Chunk {
    fn is_hard_break(&self) -> bool {
        matches!(self.tokens.as_slice(), [WrapToken::HardBreak(_)])
    }
}

// Characters that can make up a whole line of block syntax, as bit flags.
const EQUALS: u8 = 1 << 0;
const DASH: u8 = 1 << 1;
const STAR: u8 = 1 << 2;
const UNDERSCORE: u8 = 1 << 3;
const PIPE: u8 = 1 << 4;
const COLON: u8 = 1 << 5;
/// Any other character.
const OTHER: u8 = 1 << 6;
const LINE_MARKS: usize = 7;

fn line_marks(text: &str) -> u8 {
    text.chars().fold(0, |marks, c| {
        marks
            | match c {
                '=' => EQUALS,
                '-' => DASH,
                '*' => STAR,
                '_' => UNDERSCORE,
                '|' => PIPE,
                ':' => COLON,
                // Spaces, such as those of a hard break, end a line freely.
                ' ' | '\t' => 0,
                _ => OTHER,
            }
    })
}

/// Whether a line of `words` words using exactly these characters is a
/// setext heading underline, a thematic break, or a table delimiter row.
/// Lone markers are included, since they would also open an empty list item.
/// Only `=` underlines cannot contain spaces.
fn is_line_block_syntax(marks: u8, words: usize) -> bool {
    (marks == EQUALS && words == 1)
        || matches!(marks, STAR | UNDERSCORE)
        || (marks & DASH != 0 && marks & !(DASH | PIPE | COLON) == 0)
}

/// Whether `line` is a setext heading underline, a thematic break, or a
/// table delimiter row. Unlike `is_line_block_syntax`, which also refuses
/// lone markers for line breaks, this is exact, since it decides escapes.
fn is_line_block_text(line: &str) -> bool {
    let underline = |marker: char| !line.is_empty() && line.chars().all(|c| c == marker);
    let delimiter_row = line.contains('-')
        && line
            .chars()
            .all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t'));
    underline('=') || underline('-') || is_thematic_break(line) || delimiter_row
}

/// The chunks between hard breaks; a hard break ends its segment's last chunk.
struct Segment {
    chunks: Vec<Chunk>,
    /// `offsets[i]` is the width of chunks `..i` with one space between
    /// each pair, ignoring the whitespace before the first chunk.
    offsets: Vec<usize>,
    /// `mark_counts[i][bit]` counts chunks `..i` whose `line_marks` include
    /// `1 << bit`.
    mark_counts: Vec<[usize; LINE_MARKS]>,
    /// Whether a line break may replace the whitespace before chunk `i`.
    breakable: Vec<bool>,
    /// Whether the segment starts its paragraph, and so a block.
    opens_paragraph: bool,
}

impl Segment {
    fn new(chunks: Vec<Chunk>, opens_paragraph: bool) -> Self {
        let mut offsets = vec![0];
        let mut mark_counts = vec![[0; LINE_MARKS]];
        for (i, chunk) in chunks.iter().enumerate() {
            let space = usize::from(i > 0);
            offsets.push(offsets[i] + space + chunk.width);
            let marks = line_marks(&chunk.text);
            let mut counts = mark_counts[i];
            for (bit, count) in counts.iter_mut().enumerate() {
                *count += usize::from(marks & (1 << bit) != 0);
            }
            mark_counts.push(counts);
        }
        let mut segment = Segment {
            chunks,
            offsets,
            mark_counts,
            breakable: Vec::new(),
            opens_paragraph,
        };
        segment.breakable = (0..segment.chunks.len())
            .map(|i| i > 0 && segment.allows_break_before(i))
            .collect();
        segment
    }

    /// Width of chunks `start..end` on one line, with single spaces between
    /// them. Line breaks are chosen by this width, so that a break replacing
    /// a wider run of whitespace does not make the line fit on the next pass.
    fn width(&self, start: usize, end: usize) -> usize {
        self.offsets[end] - self.offsets[start] - usize::from(start > 0)
    }

    /// Width of chunks `start..end` on one line with their own whitespace.
    fn spaced_width(&self, start: usize, end: usize) -> usize {
        self.width(start, end)
            + self.chunks[start + 1..end]
                .iter()
                .map(|chunk| chunk.space.len() - 1)
                .sum::<usize>()
    }

    /// Whether chunks `start..end` on one line would form a setext
    /// underline, thematic break, or table delimiter row.
    fn is_line_block(&self, start: usize, end: usize) -> bool {
        let (before, through) = (&self.mark_counts[start], &self.mark_counts[end]);
        let marks = (0..LINE_MARKS)
            .filter(|&bit| through[bit] != before[bit])
            .fold(0, |marks, bit| marks | 1 << bit);
        is_line_block_syntax(marks, end - start)
    }

    /// A break before chunk `i` must not end the previous line with a
    /// backslash, which would become a hard break, or start the next line
    /// with Markdown block syntax.
    fn allows_break_before(&self, i: usize) -> bool {
        // Later breaks may end the line anywhere, but the line that keeps the
        // remainder of the segment has no other end to choose.
        self.allows_line_start(i) && !self.is_line_block(i, self.chunks.len())
    }

    /// Whether a line may start at chunk `i` after a break, judging the line
    /// by its first words only.
    fn allows_line_start(&self, i: usize) -> bool {
        if self.chunks[i - 1].text.ends_with('\\') || self.chunks[i].is_hard_break() {
            return false;
        }
        !self.starts_block(i)
    }

    /// Whether a line starting at chunk `i` would open a block construct.
    fn starts_block(&self, i: usize) -> bool {
        // Prefix constructs are decided by the line's first word, except that
        // a link reference definition label may span words.
        let next = &self.chunks[i].text;
        if next.starts_with('[') {
            let mut rest = String::new();
            for chunk in &self.chunks[i..] {
                if rest.len() > MAX_LABEL_LOOKAHEAD {
                    break;
                }
                rest.push_str(&chunk.text);
                rest.push(' ');
            }
            starts_block_syntax(&rest)
        } else {
            starts_block_syntax(next)
        }
    }

    /// Lines at the source's line breaks, for reflow that keeps them. A break
    /// is dropped where the next line would open a block construct, which
    /// could happen when a lazy continuation line gains its container prefix
    /// or a continuation line loses indentation.
    fn existing_lines(&self) -> Vec<(usize, usize)> {
        let len = self.chunks.len();
        let breaks: Vec<usize> = (1..len)
            .filter(|&i| self.chunks[i].space.contains('\n'))
            .chain([len])
            .collect();
        let mut lines = Vec::new();
        let mut start = 0;
        for pair in breaks.windows(2) {
            let (i, next) = (pair[0], pair[1]);
            if self.allows_line_start(i)
                && !self.is_line_block(start, i)
                && !self.is_line_block(i, next)
                && !(start == 0 && self.first_line_opens_block(i))
            {
                lines.push((start, i));
                start = i;
            }
        }
        if len > 0 {
            lines.push((start, len));
        }
        lines
    }

    /// Escape block syntax at the start of a segment, which no break can
    /// move. In the source, a line after a hard break could have been a lazy
    /// or deeply indented continuation line, which cannot open a block but
    /// would once rendered with its container's prefix. A paragraph can
    /// start with block syntax when it continues a link reference
    /// definition, which is rendered on its own line. The text that makes
    /// the line a block is escaped: leading punctuation, or the delimiter
    /// after an ordered list number. A leading run of a delimiter that could
    /// pair with another run, such as `*` or a backtick, is escaped whole,
    /// so that no shorter run is left to pair. `$` is left alone, as it
    /// opens no block without math.
    fn escape_block_start(mut self) -> Self {
        let Some(first) = self.chunks.first().map(|chunk| chunk.text.as_str()) else {
            return self;
        };
        let blocks = if first.starts_with('`') {
            // A backtick fence needs an info string without backticks.
            let fence = first.bytes().take_while(|&b| b == b'`').count();
            fence >= 3
                && !first[fence..].contains('`')
                && !self.chunks[1..]
                    .iter()
                    .any(|chunk| chunk.text.contains('`'))
        } else {
            // Breaks never end a line where it would be a line block, so
            // only a segment that is one throughout needs an escape.
            let text: Vec<&str> = self
                .chunks
                .iter()
                .map(|chunk| chunk.text.as_str())
                .collect();
            !first.starts_with('$')
                && (self.starts_block(0) || is_line_block_text(text.join(" ").trim_end()))
        };
        let chunk = match self.chunks.first_mut() {
            Some(chunk) if blocks => chunk,
            _ => return self,
        };
        let Some(WrapToken::Word(word)) = chunk.tokens.first_mut() else {
            return self;
        };
        // A link reference definition cannot interrupt a paragraph, but a
        // footnote definition can; its `^` is escaped, since an escaped `[`
        // could complete a label opened on an earlier line.
        let at = if word.starts_with('[') {
            if !word.starts_with("[^") {
                return self;
            }
            1
        } else {
            word.bytes().take_while(u8::is_ascii_digit).count()
        };
        let Some(marker) = word[at..].chars().next().filter(char::is_ascii_punctuation) else {
            return self;
        };
        let run = if "*_~`".contains(marker) {
            word[at..].chars().take_while(|&c| c == marker).count()
        } else {
            1
        };
        let mut escaped = word[..at].to_string();
        for c in word[at..].chars().take(run) {
            escaped.push('\\');
            escaped.push(c);
        }
        escaped.push_str(&word[at + run..]);
        // The word renders verbatim at the start of the chunk.
        chunk.text.replace_range(..word.len(), &escaped);
        chunk.width += run;
        *word = escaped;
        Segment::new(self.chunks, self.opens_paragraph)
    }

    /// Whether a line may span chunks `start..end`, followed by a break.
    fn allows_line(&self, start: usize, end: usize) -> bool {
        self.breakable[end]
            && !self.is_line_block(start, end)
            && !(start == 0 && self.first_line_opens_block(end))
    }

    /// Whether the segment's first line ending before chunk `end` would open
    /// a block that the rest of the line prevented in the source: a code
    /// fence whose info string held a backtick, or, at the start of a
    /// paragraph, an HTML block from a lone tag or a link reference
    /// definition whose destination was followed by more text. The line may
    /// end once that text shows it is no definition.
    fn first_line_opens_block(&self, end: usize) -> bool {
        let first = &self.chunks[0];
        let definition = || {
            let line: Vec<&str> = self.chunks[..end]
                .iter()
                .map(|chunk| chunk.text.as_str())
                .collect();
            !rules_out_definition(&line.join(" "))
        };
        if self.opens_paragraph
            && ((end == 1 && matches!(first.tokens.as_slice(), [WrapToken::Void(Node::Html(_))]))
                || (first.text.starts_with('[') && self.starts_block(0) && definition()))
        {
            return true;
        }
        let fence = first.text.bytes().take_while(|&b| b == b'`').count();
        fence >= 3
            && !first.text[fence..].contains('`')
            && !self.chunks[1..end]
                .iter()
                .any(|chunk| chunk.text.contains('`'))
    }

    /// Choose where the line starting at chunk `start` ends: the last
    /// acceptable break within `budget`, or, when the line has none, the first
    /// acceptable break after it. Lines overflow rather than expose block
    /// syntax or break between tokens that had no whitespace between them.
    fn line_end(&self, start: usize, budget: ReflowWidth) -> usize {
        let len = self.chunks.len();
        let mut fit = start + 1;
        while fit < len && !budget.is_exceeded_by(self.width(start, fit + 1)) {
            fit += 1;
        }
        if fit == len {
            return len;
        }
        let acceptable = |end: &usize| self.allows_line(start, *end);
        (start + 1..=fit)
            .rev()
            .find(acceptable)
            .or_else(|| (fit + 1..len).find(acceptable))
            .unwrap_or(len)
    }
}

/// Bound on how much of a line is inspected for a link reference definition;
/// CommonMark labels hold at most 999 characters.
const MAX_LABEL_LOOKAHEAD: usize = 1024;

/// Whether a line beginning with `line` would open a block construct rather
/// than continue a paragraph: a list item, block quote, ATX heading, code
/// fence, math block, HTML block, or link reference or footnote definition.
/// Ordered list markers are rejected for every start number, since a line
/// can also continue an enclosing list.
fn starts_block_syntax(line: &str) -> bool {
    let bytes = line.as_bytes();
    let space_or_end = |i: usize| bytes.get(i).is_none_or(|b| matches!(b, b' ' | b'\t'));
    let run = |byte: u8| bytes.iter().take_while(|&&b| b == byte).count();
    match bytes.first() {
        Some(b'-' | b'+' | b'*') => space_or_end(1),
        Some(b'>') => true,
        Some(b'#') => run(b'#') <= 6 && space_or_end(run(b'#')),
        Some(&fence @ (b'`' | b'~')) => run(fence) >= 3,
        Some(b'$') => run(b'$') >= 2,
        Some(b'<') => bytes
            .get(1)
            .is_some_and(|b| b.is_ascii_alphabetic() || matches!(b, b'!' | b'?' | b'/')),
        Some(b'[') => {
            let mut escaped = false;
            for (i, &b) in bytes.iter().enumerate().skip(1) {
                match b {
                    _ if escaped => escaped = false,
                    b'\\' => escaped = true,
                    b'[' => return false,
                    b']' => return bytes.get(i + 1) == Some(&b':'),
                    _ => {}
                }
            }
            false
        }
        Some(b'0'..=b'9') => {
            let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
            digits <= 9
                && matches!(bytes.get(digits), Some(b'.' | b')'))
                && space_or_end(digits + 1)
        }
        _ => false,
    }
}

/// Whether a paragraph's first line `line`, which starts with a link
/// reference definition's `[label]:`, holds text that no definition allows,
/// whatever lines follow: text after the destination that cannot start a
/// title, or text after a title. A definition ends with its line, and its
/// destination or title can start on a later line. A footnote definition
/// needs no destination.
fn rules_out_definition(line: &str) -> bool {
    let bytes = line.as_bytes();
    let Some(close) = (1..bytes.len()).find(|&i| bytes[i] == b']' && !is_escaped(line, i)) else {
        return false;
    };
    if bytes.get(1) == Some(&b'^') {
        return false;
    }
    let rest = line[close + 1..].strip_prefix(':').unwrap_or_default();
    let destination = rest.trim_start_matches([' ', '\t']);
    let destination_len = if destination.starts_with('<') {
        match (1..destination.len())
            .find(|&i| destination.as_bytes()[i] == b'>' && !is_escaped(destination, i))
        {
            Some(end) => end + 1,
            None => return false,
        }
    } else {
        destination.find([' ', '\t']).unwrap_or(destination.len())
    };
    let after = &destination[destination_len..];
    let title = after.trim_start_matches([' ', '\t']);
    if destination_len == 0 || title.is_empty() || title.len() == after.len() {
        return false;
    }
    let closer = match title.as_bytes()[0] {
        b'"' => b'"',
        b'\'' => b'\'',
        b'(' => b')',
        _ => return true,
    };
    (1..title.len())
        .find(|&i| title.as_bytes()[i] == closer && !is_escaped(title, i))
        .is_some_and(|end| !title[end + 1..].trim_matches([' ', '\t']).is_empty())
}

/// Group tokens into segments of unbreakable chunks separated by whitespace.
fn segments(tokens: Vec<WrapToken>, src: &str) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut space = String::new();
    let mut current: Option<Chunk> = None;
    for token in tokens {
        if token.is_space() {
            chunks.extend(current.take());
            if let WrapToken::Word(s) = &token {
                space.push_str(s);
            }
            continue;
        }
        let chunk = current.get_or_insert_with(|| Chunk {
            space: std::mem::take(&mut space),
            tokens: Vec::new(),
            text: String::new(),
            width: 0,
        });
        let start = chunk.text.len();
        token.render(src, &mut chunk.text);
        chunk.width += chunk.text[start..].chars().count();
        let hard_break = matches!(token, WrapToken::HardBreak(_));
        chunk.tokens.push(token);
        if hard_break {
            chunks.extend(current.take());
            let opens_paragraph = segments.is_empty();
            segments.push(Segment::new(std::mem::take(&mut chunks), opens_paragraph));
            space.clear();
        }
    }
    chunks.extend(current);
    // Whitespace after the final chunk ends the paragraph and is dropped.
    if !chunks.is_empty() || segments.is_empty() {
        let opens_paragraph = segments.is_empty();
        segments.push(Segment::new(chunks, opens_paragraph));
    }
    segments
}

/// Insert soft breaks at word boundaries within the supplied content budgets.
/// An infinite `middle` joins all soft breaks, and a disabled one keeps
/// those of the source.
fn wrap_spans(spans: Vec<Span>, widths: EdgeWidths, opens_block: bool, src: &str) -> Vec<Span> {
    // 1. Tokenize: flatten compound spans into open/close markers.
    let mut tokens: Vec<WrapToken> = Vec::new();
    tokenize(spans, true, &mut tokens);

    // 2. Choose line breaks between unbreakable chunks, greedily for finite
    // widths. Each entry is the chunk range of one line within its segment.
    let segments: Vec<Segment> = segments(tokens, src)
        .into_iter()
        .enumerate()
        .map(|(index, segment)| {
            if index > 0 || opens_block {
                segment.escape_block_start()
            } else {
                segment
            }
        })
        .collect();
    let mut lines: Vec<Vec<(usize, usize)>> = Vec::with_capacity(segments.len());
    for (index, segment) in segments.iter().enumerate() {
        let segment_lines = match widths.middle {
            ReflowWidth::Disabled => segment.existing_lines(),
            ReflowWidth::Infinite => vec![(0, segment.chunks.len())],
            ReflowWidth::Finite(_) => {
                let mut segment_lines = Vec::new();
                let mut start = 0;
                while start < segment.chunks.len() {
                    let budget = if index == 0 && start == 0 {
                        widths.first
                    } else {
                        widths.middle
                    };
                    let end = segment.line_end(start, budget);
                    segment_lines.push((start, end));
                    start = end;
                }
                segment_lines
            }
        };
        lines.push(segment_lines);
    }

    if matches!(widths.middle, ReflowWidth::Finite(_)) {
        reserve_final_line_width(&segments, &mut lines, widths);
    }

    // 3. Emit the tokens, replacing the whitespace at each break. A wrapped
    // line keeps its whitespace, such as two spaces after a sentence, only
    // if the line still fits; otherwise each run becomes one space.
    let line_count: usize = lines.iter().map(Vec::len).sum();
    let mut line_index = 0;
    let mut result_tokens = Vec::new();
    for (segment, segment_lines) in segments.into_iter().zip(lines) {
        let spaced: Vec<bool> = segment_lines
            .iter()
            .map(|&(start, end)| {
                let budget = match (line_index == 0, line_index + 1 == line_count) {
                    (true, true) => widths.single,
                    (true, false) => widths.first,
                    (false, true) => widths.last,
                    (false, false) => widths.middle,
                };
                line_index += 1;
                !matches!(widths.middle, ReflowWidth::Finite(_))
                    || !budget.is_exceeded_by(segment.spaced_width(start, end))
            })
            .collect();
        let mut chunks = segment.chunks.into_iter().enumerate();
        for (line, &(start, end)) in segment_lines.iter().enumerate() {
            if line > 0 {
                result_tokens.push(WrapToken::SoftBreak);
            }
            for (i, chunk) in chunks.by_ref().take(end - start) {
                if i > start {
                    // A joined line break is one space.
                    let space = if spaced[line] && !chunk.space.contains('\n') {
                        chunk.space
                    } else {
                        " ".to_string()
                    };
                    result_tokens.push(WrapToken::Word(space));
                }
                result_tokens.extend(chunk.tokens);
            }
        }
    }

    // 4. Convert the token stream back to spans.
    tokens_to_spans(result_tokens)
}

/// Move words from an overlong final line onto a new final line, splitting as
/// late as possible so that the earlier line keeps its own budget.
fn reserve_final_line_width(
    segments: &[Segment],
    lines: &mut [Vec<(usize, usize)>],
    widths: EdgeWidths,
) {
    let (Some(segment), Some(segment_lines)) = (segments.last(), lines.last_mut()) else {
        return;
    };
    let Some(&(start, end)) = segment_lines.last() else {
        return;
    };
    let first_line = segments.len() == 1 && segment_lines.len() == 1;
    let limit = if first_line {
        widths.single
    } else {
        widths.last
    };
    let ReflowWidth::Finite(limit) = limit else {
        return;
    };
    if segment.width(start, end) <= limit {
        return;
    }
    let budget = if first_line {
        widths.first
    } else {
        widths.middle
    };
    for split in (start + 1..end).rev() {
        if segment.width(split, end) > limit {
            return;
        }
        if segment.allows_line(start, split) && !budget.is_exceeded_by(segment.width(start, split))
        {
            segment_lines.pop();
            segment_lines.extend([(start, split), (split, end)]);
            return;
        }
    }
}

/// Reconstruct spans from a token stream, matching open/close markers.
fn tokens_to_spans(tokens: Vec<WrapToken>) -> Vec<Span> {
    /// Parse a sequence of tokens into a list of spans, consuming tokens
    /// until we hit a close marker (or end of input).
    fn parse_spans(tokens: &mut std::vec::IntoIter<WrapToken>) -> Vec<Span> {
        let mut spans: Vec<Span> = Vec::new();
        while let Some(token) = tokens.next() {
            match token {
                WrapToken::Word(s) | WrapToken::Unbreakable(s) => spans.push(Span::Text(s)),
                WrapToken::HardBreak(hard_break) => spans.push(Span::Break(hard_break)),
                WrapToken::Void(node) => spans.push(Span::Void(node)),
                WrapToken::Open(node) => {
                    let children = parse_spans(tokens);
                    spans.push(Span::Container(node, children));
                }
                WrapToken::Close(_) => return spans,
                WrapToken::SoftBreak => spans.push(Span::SoftBreak),
            }
        }
        spans
    }

    parse_spans(&mut tokens.into_iter())
}

// ---------------------------------------------------------------------------
// AST walking and reflow
// ---------------------------------------------------------------------------

fn reflow_root(root: &mut Node, widths: EdgeWidths, src: &str) {
    // Disabled reflow keeps the source's line breaks where it can.
    let widths = if widths.reflow_disabled() {
        EdgeWidths {
            first: ReflowWidth::Disabled,
            middle: ReflowWidth::Disabled,
            last: ReflowWidth::Disabled,
            single: ReflowWidth::Disabled,
        }
    } else {
        widths
    };
    if let Some(children) = root.children_mut() {
        let mut initial_line_pending = widths.first != widths.middle
            || widths.last != widths.middle
            || widths.single != widths.middle;
        reflow_children(children, widths, 0, &mut initial_line_pending, src);
    }
}

fn reflow_children(
    children: &mut [Node],
    widths: EdgeWidths,
    prefix_width: usize,
    initial_line_pending: &mut bool,
    src: &str,
) {
    for child in children.iter_mut() {
        reflow_node(child, widths, prefix_width, initial_line_pending, src);
    }
}

fn reflow_node(
    node: &mut Node,
    widths: EdgeWidths,
    prefix_width: usize,
    initial_line_pending: &mut bool,
    src: &str,
) {
    match node {
        // Paragraph: reflow phrasing content.
        Node::Paragraph(p) => {
            reflow_paragraph(
                &mut p.children,
                widths,
                prefix_width,
                initial_line_pending,
                src,
            );
        }
        // Blockquote: content is prefixed with "> " (2 chars) on every line.
        Node::Blockquote(_) => {
            if let Some(children) = node.children_mut() {
                reflow_children(
                    children,
                    widths,
                    prefix_width + 2,
                    initial_line_pending,
                    src,
                );
            }
        }
        // List: each item has a marker that adds to the prefix width.
        Node::List(list) => {
            let start = list.start.unwrap_or(1);
            for (idx, child) in list.children.iter_mut().enumerate() {
                let marker = if list.ordered {
                    format!("{}. ", item_number(start, idx))
                } else {
                    "* ".to_string()
                };
                reflow_node(
                    child,
                    widths,
                    prefix_width + marker.len(),
                    initial_line_pending,
                    src,
                );
            }
        }
        Node::Heading(heading) => reflow_heading(heading, widths, src),
        // A footnote definition's label precedes its first line.
        Node::FootnoteDefinition(definition) => {
            let marker_width = footnote_marker(definition, src).chars().count();
            reflow_children(
                &mut definition.children,
                widths,
                prefix_width + marker_width,
                initial_line_pending,
                src,
            );
        }
        // Container nodes: recurse into children.
        Node::Root(_)
        | Node::ListItem(_)
        | Node::Table(_)
        | Node::TableRow(_)
        | Node::TableCell(_)
        | Node::Link(_)
        | Node::LinkReference(_)
        | Node::Emphasis(_)
        | Node::Strong(_)
        | Node::Delete(_)
        | Node::MdxJsxFlowElement(_)
        | Node::MdxJsxTextElement(_) => {
            if let Some(children) = node.children_mut() {
                reflow_children(children, widths, prefix_width, initial_line_pending, src);
            }
        }
        // Leaf/void nodes: nothing to do.
        _ => {}
    }
}

/// Reflow a paragraph's children.
fn reflow_paragraph(
    children: &mut Vec<Node>,
    widths: EdgeWidths,
    prefix_width: usize,
    initial_line_pending: &mut bool,
    src: &str,
) {
    // 1. Convert all children to spans.
    let mut spans: Vec<Span> = Vec::new();
    for child in std::mem::take(children) {
        node_into_spans(child, src, &mut spans);
    }

    if spans.is_empty() {
        return;
    }

    // Account for the prefix that rendering adds to each line.
    let mut available = EdgeWidths {
        first: widths.first.saturating_sub(prefix_width),
        middle: widths.middle.saturating_sub(prefix_width),
        last: widths.last.saturating_sub(prefix_width),
        single: widths.single.saturating_sub(prefix_width),
    };
    if *initial_line_pending {
        *initial_line_pending = false;
    } else {
        available.first = available.middle;
        available.single = available.last;
    }
    *children = rewrap(spans, available, true, src);
}

/// Wrap phrasing content and convert it back to nodes. `opens_block` tells
/// whether the content starts its block's first line.
fn rewrap(spans: Vec<Span>, widths: EdgeWidths, opens_block: bool, src: &str) -> Vec<Node> {
    // 2. Wrap.
    let wrapped = wrap_spans(spans, widths, opens_block, src);

    // 3. Merge adjacent SoftBreak + Text("\n") patterns and trim.
    let wrapped = coalesce_spans(wrapped);

    // 4. Convert back to nodes.
    spans_into_nodes(wrapped)
}

/// Join the lines of a setext heading, or keep them where safe when reflow
/// is disabled. Headings are never wrapped.
fn reflow_heading(heading: &mut Heading, widths: EdgeWidths, src: &str) {
    let atx = position_source(heading.position.as_ref(), src)
        .is_some_and(|source| source.trim_start().starts_with('#'));
    let mut spans: Vec<Span> = Vec::new();
    if atx && let Some(dropped) = dropped_heading_hashes(heading, src) {
        spans.push(Span::Text(dropped.to_owned()));
    }
    for child in std::mem::take(&mut heading.children) {
        node_into_spans(child, src, &mut spans);
    }
    let width = if widths.middle == ReflowWidth::Disabled {
        ReflowWidth::Disabled
    } else {
        ReflowWidth::Infinite
    };
    let widths = EdgeWidths {
        first: width,
        middle: width,
        last: width,
        single: width,
    };
    // An ATX heading's content follows its `#` marker, so block syntax there
    // needs no escape. A setext heading's first line can continue a
    // definition, like a paragraph's.
    heading.children = rewrap(spans, widths, !atx, src);
}

/// The source that starts an ATX heading's content and that the parser
/// drops: a run of `#`, as in `# # Title`, through the start of the first
/// child, or through the end of the content when no child is left, as in
/// `# # #`, whose content is `#`. Rendering escapes the first `#`.
fn dropped_heading_hashes<'a>(heading: &Heading, src: &'a str) -> Option<&'a str> {
    let position = heading.position.as_ref()?;
    let line = src.get(position.start.offset..position.end.offset)?;
    let content = line
        .trim_start_matches([' ', '\t'])
        .trim_start_matches('#')
        .trim_start_matches([' ', '\t']);
    let start = position.end.offset - content.len();
    let end = match heading.children.first() {
        Some(child) => child.position()?.start.offset,
        None => {
            // A final run of `#` after whitespace, or alone, is a closing
            // sequence.
            let content = content.trim_end_matches([' ', '\t']);
            let open = content.trim_end_matches('#');
            let content = if open.is_empty() || open.ends_with([' ', '\t']) {
                open.trim_end_matches([' ', '\t'])
            } else {
                content
            };
            start + content.len()
        }
    };
    // The first child's source includes an escape before its start.
    let dropped = src.get(start..end)?;
    let len = dropped
        .find(|c| !matches!(c, '#' | ' ' | '\t'))
        .unwrap_or(dropped.len());
    dropped.starts_with('#').then_some(&dropped[..len])
}

/// Whitespace that Markdown collapses or trims. Other Unicode whitespace,
/// such as a no-break space, is content.
fn is_markdown_whitespace(c: char) -> bool {
    c.is_ascii_whitespace()
}

/// Clean up the span list: merge adjacent Text spans, remove empty ones, etc.
fn coalesce_spans(spans: Vec<Span>) -> Vec<Span> {
    let mut result: Vec<Span> = Vec::new();

    for span in spans {
        match span {
            Span::Text(s) if s.is_empty() => continue,
            Span::Text(s) => {
                // Merge with previous Text span if present.
                if let Some(Span::Text(prev)) = result.last_mut() {
                    prev.push_str(&s);
                } else {
                    result.push(Span::Text(s));
                }
            }
            Span::SoftBreak => {
                // Remove trailing whitespace before the soft break.
                if let Some(Span::Text(prev)) = result.last_mut() {
                    prev.truncate(prev.trim_end_matches(is_markdown_whitespace).len());
                    if prev.is_empty() {
                        result.pop();
                    }
                }
                result.push(Span::SoftBreak);
            }
            other => result.push(other),
        }
    }

    // Trim leading/trailing whitespace on the whole line sequence.
    // Remove leading soft breaks.
    while result.first().is_some_and(|s| matches!(s, Span::SoftBreak)) {
        result.remove(0);
    }
    // Remove trailing soft breaks.
    while result.last().is_some_and(|s| matches!(s, Span::SoftBreak)) {
        result.pop();
    }
    // Trim leading whitespace in first text.
    if let Some(Span::Text(s)) = result.first_mut() {
        *s = s.trim_start_matches(is_markdown_whitespace).to_string();
        if s.is_empty() {
            result.remove(0);
        }
    }
    // Trim trailing whitespace in last text.
    if let Some(Span::Text(s)) = result.last_mut() {
        s.truncate(s.trim_end_matches(is_markdown_whitespace).len());
        if s.is_empty() {
            result.pop();
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Markdown rendering
// ---------------------------------------------------------------------------

fn render_root(root: &Node, input: &str, out: &mut String) {
    if let Some(children) = root.children() {
        if let Some(frontmatter @ (Node::Yaml(_) | Node::Toml(_))) = children.first() {
            // The parser's value omits the final line ending, making empty
            // content and trailing blank lines ambiguous. Keep the source
            // block, including its fences, rather than reconstructing it.
            if let Some(position) = frontmatter.position() {
                let end = position.end.offset;
                let after = &input[end..];
                let line_ending_len = line_ending_len(after).unwrap_or(0);
                out.push_str(&input[..end + line_ending_len]);
                if line_ending_len == 0 {
                    out.push('\n');
                }
                if children.len() > 1 {
                    out.push('\n');
                }
                render_flow_children(&children[1..], false, false, input, out);
                return;
            }
        }
        render_flow_children(children, false, false, input, out);
    }
}

/// Render a list of flow-level children. Blocks are separated by blank
/// lines, except consecutive definitions and the children of a tight list
/// item, where a blank line would make the list loose. With `close`, the
/// last child closes a paragraph that it leaves open in a block quote; see
/// `closes_quoted_paragraph`.
fn render_flow_children(children: &[Node], tight: bool, close: bool, src: &str, out: &mut String) {
    let mut previous: Option<&Node> = None;
    // The bullet or ordered list delimiter of the previous list.
    let mut list_delimiter: Option<char> = None;
    for (index, child) in children.iter().enumerate() {
        let after_paragraph = matches!(previous, Some(Node::Paragraph(_)));
        if let Some(previous) = previous
            && !tight
            && !matches!(
                (previous, child),
                (Node::Definition(_), Node::Definition(_))
            )
            && !continues_definition(previous, child)
        {
            out.push('\n');
        }
        let close = match children.get(index + 1) {
            Some(next) => tight && closes_quoted_paragraph(child, Some(next), src),
            None => close && closes_quoted_paragraph(child, None, src),
        };
        match child {
            // Adjacent lists of the same kind need different markers, or
            // they would merge into one list.
            Node::List(list) => {
                let adjacent = matches!(previous, Some(Node::List(previous)) if previous.ordered == list.ordered);
                let avoid = list_delimiter.filter(|_| adjacent);
                list_delimiter = Some(render_list(list, avoid, close, src, out));
            }
            // Directly after a paragraph, `---` would be a setext underline.
            Node::ThematicBreak(_) if after_paragraph && tight => out.push_str("***\n"),
            _ => render_node(child, close, src, out),
        }
        previous = Some(child);
    }
}

/// Whether `node` must end with a line that closes a paragraph it leaves
/// open in a block quote, such as an empty quote line, because the next line
/// follows without a blank line and lacks the quote's marker. That line
/// would be a lazy continuation of the paragraph, unless it opens a block
/// that interrupts a paragraph. In the source, the paragraph was closed, so
/// `next` started such a block or an empty quote line came first. Without
/// `next`, the line after the node's container follows.
fn closes_quoted_paragraph(node: &Node, next: Option<&Node>, src: &str) -> bool {
    if !ends_in_quoted_paragraph(node, false) {
        return false;
    }
    let Some(next) = next else {
        return true;
    };
    let mut rendered = String::new();
    render_node(next, false, src, &mut rendered);
    !interrupts_paragraph(rendered.lines().next().unwrap_or(""))
}

/// Whether the last block is a paragraph inside a block quote, possibly
/// nested in lists, footnotes, or further quotes.
fn ends_in_quoted_paragraph(node: &Node, quoted: bool) -> bool {
    match node {
        Node::Paragraph(_) => quoted,
        Node::Blockquote(_) | Node::List(_) | Node::ListItem(_) | Node::FootnoteDefinition(_) => {
            let quoted = quoted || matches!(node, Node::Blockquote(_));
            node.children()
                .and_then(|children| children.last())
                .is_some_and(|last| ends_in_quoted_paragraph(last, quoted))
        }
        _ => false,
    }
}

/// Whether a line starting with `line` certainly interrupts a paragraph: a
/// block quote, ATX heading, code fence, thematic break, or list item that
/// is not empty and, if ordered, starts at 1. Other lines may continue it.
fn interrupts_paragraph(line: &str) -> bool {
    let bytes = line.as_bytes();
    let run = |byte: u8| bytes.iter().take_while(|&&b| b == byte).count();
    let item = |marker: usize| {
        bytes.get(marker) == Some(&b' ') && !line[marker..].trim_matches([' ', '\t']).is_empty()
    };
    match bytes.first() {
        Some(b'>') => true,
        Some(b'#') => run(b'#') <= 6 && bytes.get(run(b'#')).is_none_or(|&b| b == b' '),
        Some(b'`') => run(b'`') >= 3 && !line[run(b'`')..].contains('`'),
        Some(b'~') => run(b'~') >= 3,
        Some(b'-' | b'+' | b'*') => is_thematic_break(line) || item(1),
        Some(b'1') => matches!(bytes.get(1), Some(b'.' | b')')) && item(2),
        _ => false,
    }
}

/// Whether `node` is a paragraph that continued the source lines of the
/// definition before it and starts with inline HTML. Definitions are taken
/// from the start of a paragraph, so the rest can begin with a tag that
/// cannot interrupt a paragraph but would open an HTML block after a blank
/// line. Other block syntax at the start of a paragraph is escaped instead.
fn continues_definition(previous: &Node, node: &Node) -> bool {
    match (previous, node) {
        (Node::Definition(_), Node::Paragraph(paragraph))
            if matches!(paragraph.children.first(), Some(Node::Html(_))) =>
        {
            matches!((previous.position(), node.position()),
                (Some(definition), Some(paragraph)) if paragraph.start.line == definition.end.line + 1)
        }
        _ => false,
    }
}

/// Append `inner` with `first` before its first line and `rest` before
/// later lines. Blank lines get the prefixes without trailing whitespace.
fn push_prefixed(inner: &str, first: &str, rest: &str, out: &mut String) {
    if inner.is_empty() {
        out.push_str(first.trim_end());
        out.push('\n');
        return;
    }
    for (index, line) in inner.lines().enumerate() {
        let prefix = if index == 0 { first } else { rest };
        if line.is_empty() {
            out.push_str(prefix.trim_end());
        } else {
            out.push_str(prefix);
            out.push_str(line);
        }
        out.push('\n');
    }
}

/// Render a block. With `close`, the block ends with a line that closes a
/// paragraph it leaves open in a block quote; see `closes_quoted_paragraph`.
fn render_node(node: &Node, close: bool, src: &str, out: &mut String) {
    match node {
        // ---- Block-level ----
        Node::Root(root) => render_flow_children(&root.children, false, false, src, out),
        Node::Paragraph(p) => {
            match p.children.as_slice() {
                // Inline HTML that would fill the first line keeps its line
                // breaks, which kept it from being a lone complete tag, and
                // so an HTML block, in the source.
                [Node::Html(html), rest @ ..]
                    if html.value.contains(['\n', '\r'])
                        && rest.first().is_none_or(
                            |next| matches!(next, Node::Text(text) if text.value.starts_with('\n')),
                        ) =>
                {
                    out.push_str(&html.value.replace("\r\n", "\n").replace('\r', "\n"));
                    render_phrasing(rest, src, out);
                }
                children => render_phrasing(children, src, out),
            }
            out.push('\n');
        }
        Node::Heading(h) => {
            let mut content = String::new();
            render_phrasing(&h.children, src, &mut content);
            if content.contains('\n') && h.depth <= 2 {
                // Only setext headings span lines; an ATX heading would end
                // at the first line ending.
                out.push_str(&content);
                out.push_str(if h.depth == 1 { "\n===\n" } else { "\n---\n" });
                return;
            }
            for _ in 0..h.depth {
                out.push('#');
            }
            if !content.is_empty() {
                out.push(' ');
                // Some parsers drop a leading run of `#` from the content.
                if content.starts_with('#') {
                    content.insert(0, '\\');
                }
                // A final run of `#` after a space would be a closing
                // sequence and be dropped.
                let hashes = content.len() - content.trim_end_matches('#').len();
                let before = &content[..content.len() - hashes];
                if hashes > 0 && (before.is_empty() || before.ends_with([' ', '\t'])) {
                    out.push_str(before);
                    out.push('\\');
                    out.push_str(&content[before.len()..]);
                } else {
                    out.push_str(&content);
                }
            }
            out.push('\n');
        }
        Node::Blockquote(bq) => {
            let mut inner = String::new();
            render_flow_children(&bq.children, false, false, src, &mut inner);
            // An empty quote line closes the quote's last paragraph.
            // markdown 1.0.0 also needs it after an unescaped trailing pipe:
            // otherwise its pending table header state leaks out of the
            // quote and adds a column to a later table's header count. A
            // blank line outside the quote does not clear that state.
            let tail = inner.trim_end_matches([' ', '\t', '\r', '\n']);
            let pending_table = ends_in_quoted_paragraph(node, false)
                && tail.ends_with('|')
                && !is_escaped(tail, tail.len() - 1);
            if close || pending_table {
                inner.push('\n');
            }
            push_prefixed(&inner, "> ", "> ", out);
        }
        Node::List(list) => {
            render_list(list, None, close, src, out);
        }
        Node::Code(code) => {
            // The info string keeps its source, whose escapes and character
            // references the parser decodes. Indented code has none.
            let source_info = code.lang.as_ref().and(code_info_source(node, src));
            let info = source_info.map(str::to_string).unwrap_or_else(|| {
                let mut info = String::new();
                if let Some(ref lang) = code.lang {
                    push_escaped(lang, |c| c.is_ascii_punctuation(), &mut info);
                }
                if let Some(ref meta) = code.meta {
                    info.push(' ');
                    push_escaped(meta, |c| c.is_ascii_punctuation(), &mut info);
                }
                info
            });
            // A backtick fence cannot have backticks in its info string.
            let marker = if info.contains('`') { '~' } else { '`' };
            let fence = fence(&code.value, marker, 3);
            out.push_str(&fence);
            out.push_str(&info);
            out.push('\n');
            push_block_content(&code.value, out);
            out.push_str(&fence);
            out.push('\n');
        }
        Node::ThematicBreak(_) => {
            out.push_str("---\n");
        }
        Node::Html(h) => {
            // An HTML block left open at the end of the input includes its
            // final line ending.
            out.push_str(&h.value);
            if !h.value.ends_with('\n') {
                out.push('\n');
            }
        }
        Node::Math(m) => {
            let fence = fence(&m.value, '$', 2);
            out.push_str(&fence);
            if let Some(ref meta) = m.meta {
                out.push_str(meta);
            }
            out.push('\n');
            push_block_content(&m.value, out);
            out.push_str(&fence);
            out.push('\n');
        }
        Node::Table(table) => {
            render_table(table, src, out);
        }
        Node::Definition(def) => {
            out.push('[');
            out.push_str(
                &definition_label(source(node, src), 1).unwrap_or_else(|| def.identifier.clone()),
            );
            out.push_str("]: ");
            render_destination(&def.url, out);
            if let Some(ref title) = def.title {
                out.push(' ');
                render_title(title, out);
            }
            out.push('\n');
        }
        Node::FootnoteDefinition(fd) => {
            let mut inner = String::new();
            render_flow_children(&fd.children, false, close, src, &mut inner);
            push_prefixed(&inner, &footnote_marker(fd, src), "    ", out);
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
            out.push('{');
            out.push_str(&e.value);
            out.push_str("}\n");
        }
        Node::MdxJsxFlowElement(el) => {
            render_mdx_jsx_flow(el, src, out);
        }

        // ---- Phrasing / inline (fallback for unexpected contexts) ----
        _ => render_inline(node, src, out),
    }
}

/// Render phrasing content: join all children without separators.
fn render_phrasing(children: &[Node], src: &str, out: &mut String) {
    for child in children {
        render_inline(child, src, out);
    }
}

fn render_inline(node: &Node, src: &str, out: &mut String) {
    match node {
        // Text values hold Markdown source; see `restore_source`.
        Node::Text(t) => out.push_str(&t.value),
        Node::Break(hard_break) => {
            out.push_str(render_hard_break(hard_break, out, src));
            out.push('\n');
        }
        // Code keeps its source fence, which the source proved does not pair
        // with stray backticks in nearby text.
        Node::InlineCode(_) | Node::InlineMath(_) if single_line_source(node, src).is_some() => {
            out.push_str(single_line_source(node, src).unwrap_or_default());
        }
        Node::InlineCode(ic) => render_code_span(&ic.value, '`', source(node, src), out),
        Node::InlineMath(im) => render_code_span(&im.value, '$', source(node, src), out),
        // Inline HTML shares its node type with HTML blocks, which end
        // their line. Inline HTML must stay within its line, and its line
        // endings become spaces, which HTML treats alike: a later line could
        // start with block syntax once it gains its container's prefix.
        Node::Html(h) => out.push_str(&h.value.replace("\r\n", " ").replace(['\r', '\n'], " ")),
        Node::Link(_) if is_autolink(node, src) => {
            out.push_str(source(node, src).unwrap_or_default());
        }
        Node::Emphasis(_)
        | Node::Strong(_)
        | Node::Delete(_)
        | Node::Link(_)
        | Node::LinkReference(_) => {
            render_open(node, src, out);
            render_phrasing(node.children().map_or(&[], Vec::as_slice), src, out);
            render_close(node, src, out);
        }
        // Images and references keep their source, which holds the alt
        // text's markup and the label's spelling, when it lies on one line.
        Node::Image(_) | Node::ImageReference(_) | Node::FootnoteReference(_)
            if single_line_source(node, src).is_some() =>
        {
            out.push_str(single_line_source(node, src).unwrap_or_default());
        }
        Node::Image(img) => {
            out.push_str("![");
            push_escaped(&img.alt, |c| "`*_~[]<".contains(c), out);
            render_link_close(&img.url, img.title.as_deref(), out);
        }
        Node::ImageReference(ir) => {
            out.push_str("![");
            push_escaped(&ir.alt, |c| "`*_~[]<".contains(c), out);
            out.push(']');
            match ir.reference_kind {
                ReferenceKind::Full => write!(out, "[{}]", ir.identifier).unwrap(),
                ReferenceKind::Collapsed => out.push_str("[]"),
                ReferenceKind::Shortcut => {}
            }
        }
        Node::FootnoteReference(fr) => {
            write!(out, "[^{}]", fr.identifier).unwrap();
        }
        Node::MdxTextExpression(e) => {
            out.push('{');
            out.push_str(&e.value);
            out.push('}');
        }
        Node::MdxJsxTextElement(el) => {
            render_mdx_jsx_text(el, src, out);
        }

        // Fallback: use debug representation.
        other => {
            write!(out, "{other:?}").unwrap();
        }
    }
}

/// A hard break before its line ending: a backslash, unless the text before
/// it ends with a backslash, which a backslash would escape, with an
/// emphasis or strikethrough delimiter, whose flanking depends on whether
/// whitespace or punctuation follows, or with what may be a bare URL. Those
/// keep the source's form.
fn render_hard_break(hard_break: &Break, before: &str, src: &str) -> &'static str {
    // A bare URL would take in a backslash. A URL after `](` is a link
    // destination.
    let word = before
        .rsplit(|c: char| c.is_ascii_whitespace())
        .next()
        .unwrap_or("");
    let url = ["://", "www.", "@"].iter().any(|marker| {
        word.find(marker)
            .is_some_and(|at| !word[..at].contains("](") && !word[..at].ends_with('<'))
    });
    if !before.ends_with(['\\', '*', '_', '~']) && !url {
        return "\\";
    }
    let source = position_source(hard_break.position.as_ref(), src);
    if source.is_some_and(|source| source.starts_with('\\')) && !before.ends_with('\\') {
        "\\"
    } else {
        "  "
    }
}

/// Render the opening delimiter of a container span.
fn render_open(node: &Node, src: &str, out: &mut String) {
    match node {
        Node::Link(_) | Node::LinkReference(_) => out.push('['),
        _ => out.push_str(delimiter(node, src)),
    }
}

/// Render the closing delimiter of a container span: the emphasis delimiter,
/// a link's destination and title, or a reference's label.
fn render_close(node: &Node, src: &str, out: &mut String) {
    match node {
        Node::Link(link) => render_link_close(&link.url, link.title.as_deref(), out),
        Node::LinkReference(lr) => {
            out.push(']');
            match lr.reference_kind {
                ReferenceKind::Full => {
                    out.push('[');
                    out.push_str(
                        &reference_label(node, src).unwrap_or_else(|| lr.identifier.clone()),
                    );
                    out.push(']');
                }
                ReferenceKind::Collapsed => out.push_str("[]"),
                ReferenceKind::Shortcut => {}
            }
        }
        _ => out.push_str(delimiter(node, src)),
    }
}

/// The source of the label that ends a full reference, `[text][label]`.
/// The parsed identifier is case folded, and the parsed label is decoded.
fn reference_label(node: &Node, src: &str) -> Option<String> {
    let source = source(node, src)?.strip_suffix(']')?;
    let start = (0..source.len())
        .rev()
        .find(|&i| source.as_bytes()[i] == b'[' && !is_escaped(source, i))?;
    Some(one_line_label(&source[start + 1..]))
}

/// The source of the label in a definition, which follows the `skip` bytes
/// `[` or `[^` at the start of `source`.
fn definition_label(source: Option<&str>, skip: usize) -> Option<String> {
    let source = source?.trim_start_matches([' ', '\t']);
    let end =
        (skip..source.len()).find(|&i| source.as_bytes()[i] == b']' && !is_escaped(source, i))?;
    source.get(skip..end).map(one_line_label)
}

/// A label's source joined onto one line, without the container prefixes
/// that start its later lines.
fn one_line_label(label: &str) -> String {
    let mut lines = label.lines();
    let mut joined = lines.next().unwrap_or_default().to_string();
    for line in lines {
        joined.push(' ');
        joined.push_str(line.trim_start_matches([' ', '\t', '>']));
    }
    joined
}

/// The `[^label]: ` marker before a footnote definition's content.
fn footnote_marker(definition: &FootnoteDefinition, src: &str) -> String {
    let source = position_source(definition.position.as_ref(), src);
    let label = definition_label(source, 2).unwrap_or_else(|| definition.identifier.clone());
    format!("[^{label}]: ")
}

/// Whether the byte at `index` follows an odd number of backslashes.
fn is_escaped(s: &str, index: usize) -> bool {
    s.as_bytes()[..index]
        .iter()
        .rev()
        .take_while(|&&b| b == b'\\')
        .count()
        % 2
        == 1
}

/// Render a list with a bullet, or an ordered list delimiter, other than
/// `avoid`, which the list before it used. Returns the character used. With
/// `close`, the last item closes a paragraph it leaves open in a block quote.
fn render_list(list: &List, avoid: Option<char>, close: bool, src: &str, out: &mut String) -> char {
    /// Render the items of `children`. The parser can nest a list directly
    /// in a list, such as after an unclosed code fence in an item; its items
    /// continue the outer list.
    fn render_items(children: &[Node], close: bool, src: &str, items: &mut Vec<String>) {
        for (index, child) in children.iter().enumerate() {
            let close = close && index + 1 == children.len();
            let mut inner = String::new();
            match child {
                Node::ListItem(item) => {
                    if let Some(checked) = item.checked {
                        inner.push_str(if checked { "[x] " } else { "[ ] " });
                    }
                    let spread = separated_by_blank_line(&item.children, item.spread, src);
                    render_flow_children(&item.children, !spread, close, src, &mut inner);
                }
                Node::List(list) => {
                    render_items(&list.children, close, src, items);
                    continue;
                }
                other => render_node(other, close, src, &mut inner),
            }
            items.push(inner);
        }
    }
    let mut items = Vec::new();
    render_items(&list.children, close, src, &mut items);
    let loose = separated_by_blank_line(&list.children, list.spread, src);
    let candidates: &[char] = if list.ordered {
        &['.', ')']
    } else {
        &['*', '-', '+']
    };
    // A bullet must not make an item's first line a thematic break, as
    // `*` would with content `**`.
    let delimiter = candidates
        .iter()
        .copied()
        .filter(|&c| Some(c) != avoid)
        .find(|&c| {
            list.ordered
                || !items.iter().any(|inner| {
                    is_thematic_break(&format!("{c} {}", inner.lines().next().unwrap_or("")))
                })
        })
        .unwrap_or(candidates[0]);
    let start = list.start.unwrap_or(1);
    for (idx, inner) in items.iter().enumerate() {
        // A blank line between items keeps a loose list loose.
        if idx > 0 && loose {
            out.push('\n');
        }
        let marker = if list.ordered {
            format!("{}{delimiter} ", item_number(start, idx))
        } else {
            format!("{delimiter} ")
        };
        push_prefixed(inner, &marker, &" ".repeat(marker.len()), out);
    }
    delimiter
}

/// The number of the ordered list item at `index` in a list starting at
/// `start`. Only the first number matters, and a marker has at most nine
/// digits, so later items repeat the largest number rather than exceed it.
fn item_number(start: u32, index: usize) -> u32 {
    const MAX_ITEM_NUMBER: u32 = 999_999_999;
    let index = u32::try_from(index).unwrap_or(MAX_ITEM_NUMBER);
    start.saturating_add(index).min(MAX_ITEM_NUMBER)
}

/// Whether a blank line separates any two of `nodes` in the source, which
/// makes a list or list item loose. The parser also marks an item spread when
/// a blank line follows an empty nested item at its end, which rendering
/// would then make real. Without positions, `spread` is the answer.
///
/// A line is blank when it holds only spaces, tabs, and the `>` markers of
/// the block quotes that contain the nodes, which the next node's line
/// repeats before the node starts. Another `>` continues a block quote
/// inside the previous node, and a lazy continuation line holds text. Such a
/// line can end the previous node's span, as a line of spaces after a nested
/// list does, so the line before the next node is checked rather than the
/// previous node's end.
fn separated_by_blank_line(nodes: &[Node], spread: bool, src: &str) -> bool {
    let mut separated = false;
    for pair in nodes.windows(2) {
        let (Some(previous), Some(next)) = (pair[0].position(), pair[1].position()) else {
            return spread;
        };
        let Some(before) = src.get(..next.start.offset) else {
            return spread;
        };
        // Lines between the two nodes are blank, so checking the line just
        // before the next node is enough, unless that line holds the start
        // of the previous node.
        let next_start = line_start(before);
        if next_start == 0 {
            continue;
        }
        let line_end = before[..next_start]
            .strip_suffix('\n')
            .map_or(next_start - 1, |rest| {
                rest.strip_suffix('\r').unwrap_or(rest).len()
            });
        let start = line_start(&before[..line_end]);
        if start <= previous.start.offset {
            continue;
        }
        let quote_markers = |line: &str| line.matches('>').count();
        let line = &before[start..line_end];
        separated |= line.chars().all(|c| matches!(c, ' ' | '\t' | '>'))
            && quote_markers(line) == quote_markers(&before[next_start..]);
    }
    separated
}

/// The offset of the line that ends `text`, after its last line ending:
/// `\n`, `\r\n`, or `\r`.
fn line_start(text: &str) -> usize {
    text.rfind(['\n', '\r']).map_or(0, |i| i + 1)
}

/// Whether `line` is a thematic break: three or more of one of `*`, `-`, or
/// `_`, and spaces or tabs.
fn is_thematic_break(line: &str) -> bool {
    let mut marks = line.chars().filter(|c| !matches!(c, ' ' | '\t'));
    let first = marks.next();
    matches!(first, Some('*' | '-' | '_'))
        && marks.clone().all(|c| Some(c) == first)
        && marks.count() >= 2
}

/// The source of a fenced code block's info string.
fn code_info_source<'a>(node: &Node, src: &'a str) -> Option<&'a str> {
    let line = source(node, src)?.lines().next()?.trim_start();
    let marker = line.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let fence = line.chars().take_while(|&c| c == marker).count();
    (fence >= 3).then(|| line[fence..].trim())
}

/// A fence of `marker` characters, at least `min` long and longer than any
/// run of them in `value`.
fn fence(value: &str, marker: char, min: usize) -> String {
    let longest = value
        .split(|c| c != marker)
        .map(|run| run.len() / marker.len_utf8())
        .max()
        .unwrap_or(0);
    marker.to_string().repeat(min.max(longest + 1))
}

/// Append the content of a fenced block, whose final line ending the parser
/// does not include in the value.
fn push_block_content(value: &str, out: &mut String) {
    if !value.is_empty() {
        out.push_str(value);
        out.push('\n');
    }
}

/// Render the end of a link after its text: `](url)` or `](url "title")`.
fn render_link_close(url: &str, title: Option<&str>, out: &mut String) {
    out.push_str("](");
    render_destination(url, out);
    if let Some(title) = title {
        out.push(' ');
        render_title(title, out);
    }
    out.push(')');
}

/// Render a link destination that parses back to `url`, in angle brackets
/// when it is empty, contains spaces or controls, or has unbalanced
/// parentheses.
fn render_destination(url: &str, out: &mut String) {
    let mut depth: isize = 0;
    let mut balanced = true;
    for c in url.chars() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                balanced &= depth >= 0;
            }
            _ => {}
        }
    }
    let bare = balanced
        && depth == 0
        && !url.is_empty()
        && !url.starts_with('<')
        && !url.chars().any(|c| c == ' ' || c.is_ascii_control());
    // A destination cannot hold a line ending, but a character reference
    // can produce one.
    let mut escaped = String::new();
    if bare {
        push_escaped(url, |_| false, &mut escaped);
    } else {
        escaped.push('<');
        push_escaped(url, |c| matches!(c, '<' | '>'), &mut escaped);
        escaped.push('>');
    }
    out.push_str(&escaped.replace('\n', "&#10;").replace('\r', "&#13;"));
}

/// Render a link title in quotes. Line endings become character references,
/// so that the title stays on one line and keeps whitespace around them.
fn render_title(title: &str, out: &mut String) {
    out.push('"');
    let mut escaped = String::new();
    push_escaped(title, |c| c == '"', &mut escaped);
    out.push_str(&escaped.replace('\n', "&#10;").replace('\r', "&#13;"));
    out.push('"');
}

/// Render a code span, or inline math with `$`, whose content is `value`.
/// Line endings in the content become spaces, as they do in HTML, so the
/// span stays on one line. The fence of the `source` span is kept when
/// possible.
fn render_code_span(value: &str, marker: char, source: Option<&str>, out: &mut String) {
    // Line endings become spaces. A paragraph drops the indentation of its
    // continuation lines before code spans are read, but the parser keeps it
    // in the value.
    let value = value
        .replace("\r\n", "\n")
        .split(['\r', '\n'])
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                line
            } else {
                line.trim_start_matches([' ', '\t'])
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let value = value.as_str();
    // The fence must differ in length from every run of the marker inside.
    let runs: Vec<usize> = value
        .split(|c| c != marker)
        .map(str::len)
        .filter(|&len| len > 0)
        .collect();
    let fence_len = source
        .map(|source| source.chars().take_while(|&c| c == marker).count())
        .into_iter()
        .chain(1..)
        .find(|len| *len > 0 && !runs.contains(len))
        .unwrap_or(1);
    let fence = marker.to_string().repeat(fence_len);
    // Padding keeps a marker at either end from joining the fence. The
    // parser strips one space from each end when both are spaces.
    let pad = value.starts_with(marker)
        || value.ends_with(marker)
        || (value.starts_with(' ') && value.ends_with(' ') && value.bytes().any(|b| b != b' '));
    out.push_str(&fence);
    if pad {
        out.push(' ');
    }
    out.push_str(value);
    if pad {
        out.push(' ');
    }
    out.push_str(&fence);
}

/// Escape each pipe that a table row would otherwise read as a cell boundary.
/// The parser removes the escape from pipes in code spans as well as text.
fn escape_pipes(cell: &str) -> String {
    let mut out = String::with_capacity(cell.len());
    for (index, c) in cell.char_indices() {
        if c == '|' && !is_escaped(cell, index) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn render_table(table: &Table, src: &str, out: &mut String) {
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

    // The header row sets the column count. Missing cells are empty; extra
    // cells, which do not belong to any column, are kept without padding.
    let col_count = rows[0].children.len();

    // Render every cell up front so columns can be padded to a common width.
    let mut grid: Vec<Vec<String>> = Vec::with_capacity(rows.len());
    for row in &rows {
        let mut cells: Vec<String> = Vec::with_capacity(col_count);
        for cell in &row.children {
            if let Node::TableCell(tc) = cell {
                let mut text = String::new();
                render_phrasing(&tc.children, src, &mut text);
                cells.push(escape_pipes(&text));
            }
        }
        if cells.len() < col_count {
            cells.resize(col_count, String::new());
        }
        grid.push(cells);
    }

    // Column width: the widest cell, but at least 3 so the alignment row can
    // hold a conventional `---` marker.
    let mut widths = vec![3usize; col_count];
    for cells in &grid {
        for (j, cell) in cells.iter().take(col_count).enumerate() {
            widths[j] = widths[j].max(cell.chars().count());
        }
    }

    // Render each row with cells padded to the column width, so the pipes
    // line up vertically for human readability.
    for (i, cells) in grid.iter().enumerate() {
        out.push('|');
        for (j, cell) in cells.iter().enumerate() {
            out.push(' ');
            out.push_str(cell);
            for _ in cell.chars().count()..widths.get(j).copied().unwrap_or(0) {
                out.push(' ');
            }
            out.push_str(" |");
        }
        out.push('\n');

        // Render alignment row after header (first row), with the dashes
        // stretched to the column width.
        if i == 0 {
            out.push('|');
            for (j, &w) in widths.iter().enumerate() {
                let align = table.align.get(j).unwrap_or(&AlignKind::None);
                out.push(' ');
                match align {
                    AlignKind::Left => {
                        out.push(':');
                        out.push_str(&"-".repeat(w - 1));
                    }
                    AlignKind::Right => {
                        out.push_str(&"-".repeat(w - 1));
                        out.push(':');
                    }
                    AlignKind::Center => {
                        out.push(':');
                        out.push_str(&"-".repeat(w - 2));
                        out.push(':');
                    }
                    AlignKind::None => out.push_str(&"-".repeat(w)),
                }
                out.push_str(" |");
            }
            out.push('\n');
        }
    }
}

fn render_mdx_jsx_flow(el: &MdxJsxFlowElement, src: &str, out: &mut String) {
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
            render_flow_children(&el.children, false, false, src, out);
            out.push_str("</");
            out.push_str(name);
            out.push_str(">\n");
        }
    }
}

fn render_mdx_jsx_text(el: &MdxJsxTextElement, src: &str, out: &mut String) {
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
            render_phrasing(&el.children, src, out);
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

    fn uniform_options(width: usize) -> Options {
        Options {
            widths: EdgeWidths {
                first: ReflowWidth::Finite(width),
                middle: ReflowWidth::Finite(width),
                last: ReflowWidth::Finite(width),
                single: ReflowWidth::Finite(width),
            },
            ..Options::default()
        }
    }

    /// Check content and container prefixes as well as the width constraint.
    fn assert_wrapped_text(
        output: &str,
        text: &str,
        width: usize,
        first_prefix: &str,
        continuation_prefix: &str,
    ) {
        let mut words = Vec::new();
        for (index, line) in output.lines().enumerate() {
            assert!(
                line.chars().count() <= width,
                "width {width}: line too long: {line:?}"
            );
            let prefix = if index == 0 {
                first_prefix
            } else {
                continuation_prefix
            };
            let content = line
                .strip_prefix(prefix)
                .unwrap_or_else(|| panic!("width {width}: missing prefix {prefix:?} in {line:?}"));
            assert!(!content.is_empty(), "unexpected blank line: {output:?}");
            words.extend(content.split_whitespace());
        }
        assert_eq!(
            words,
            text.split_whitespace().collect::<Vec<_>>(),
            "width {width}: content changed: {output:?}"
        );
        assert!(output.ends_with('\n'), "missing final newline: {output:?}");
    }

    #[test]
    fn unlimited_width_joins_soft_breaks_and_preserves_formatting() {
        let input = "Hello **bold\ntext** and [a\nlink](https://example.com).\nMore text.\n";
        let expected = "Hello **bold text** and [a link](https://example.com). More text.\n";
        assert_eq!(reflow(input, Options::default()).unwrap(), expected);
        assert_eq!(
            reflow(
                input,
                Options {
                    widths: EdgeWidths {
                        first: ReflowWidth::Finite(1),
                        single: ReflowWidth::Finite(1),
                        ..EdgeWidths::default()
                    },
                    ..Options::default()
                }
            )
            .unwrap(),
            expected
        );
        assert_eq!(
            reflow(
                input,
                Options {
                    widths: EdgeWidths {
                        first: ReflowWidth::Finite(1),
                        last: ReflowWidth::Finite(1),
                        single: ReflowWidth::Finite(1),
                        ..EdgeWidths::default()
                    },
                    ..Options::default()
                }
            )
            .unwrap(),
            expected
        );
    }

    #[test]
    fn unlimited_width_preserves_blocks_and_hard_breaks() {
        let input = "# Heading\n\nFirst\nparagraph.\\\nHard\nbreak.\n\n> * Nested\n>   item.\n\n```text\ncode\nlines\n```\n";
        let expected = "# Heading\n\nFirst paragraph.\\\nHard break.\n\n> * Nested item.\n\n```text\ncode\nlines\n```\n";
        assert_eq!(reflow(input, Options::default()).unwrap(), expected);
    }

    #[test]
    fn unlimited_width_still_applies_options() {
        assert_eq!(
            reflow(
                "See\nhttps://example.com\nnow.\n",
                Options {
                    linkify_urls: true,
                    ..Options::default()
                }
            )
            .unwrap(),
            "See [https://example.com](https://example.com) now.\n"
        );
    }

    #[test]
    fn unlimited_width_does_not_wrap_long_paragraphs() {
        let input = "word\n".repeat(1000);
        assert_eq!(
            reflow(&input, Options::default()).unwrap(),
            format!("{}\n", vec!["word"; 1000].join(" "))
        );
    }

    #[test]
    fn finite_infinite_and_disabled_have_distinct_behavior() {
        let input = "one two\nthree four\n";
        assert_eq!(
            reflow(input, uniform_options(5)).unwrap(),
            "one\ntwo\nthree\nfour\n"
        );
        assert_eq!(
            reflow(input, Options::default()).unwrap(),
            "one two three four\n"
        );
        assert_eq!(
            reflow(
                input,
                Options {
                    widths: EdgeWidths {
                        middle: ReflowWidth::Disabled,
                        ..EdgeWidths::default()
                    },
                    ..Options::default()
                }
            )
            .unwrap(),
            input
        );
    }

    #[test]
    fn disabled_width_preserves_nested_soft_breaks_and_hard_breaks() {
        let input = "First **bold\ntext** and [a\nlink](https://example.com).  \nHard\nbreak.\n\n> * Nested\n>   item.\n\n```text\ncode\nlines\n```\n";
        let expected = "First **bold\ntext** and [a\nlink](https://example.com).\\\nHard\nbreak.\n\n> * Nested\n>   item.\n\n```text\ncode\nlines\n```\n";
        // Any disabled budget bypasses the whole reflow traversal, even when
        // the other budgets would wrap aggressively or join soft breaks.
        for remaining in [
            ReflowWidth::Finite(1),
            ReflowWidth::Infinite,
            ReflowWidth::Disabled,
        ] {
            for disabled_edge in 0..4 {
                let mut widths = [remaining; 4];
                widths[disabled_edge] = ReflowWidth::Disabled;
                let [first, middle, last, single] = widths;
                assert_eq!(
                    reflow(
                        input,
                        Options {
                            widths: EdgeWidths {
                                first,
                                middle,
                                last,
                                single
                            },
                            ..Options::default()
                        }
                    )
                    .unwrap(),
                    expected,
                    "remaining: {remaining:?}, disabled edge: {disabled_edge}"
                );
            }
        }
    }

    #[test]
    fn disabled_width_still_linkifies_and_formats_markdown() {
        let input = "Heading\n=======\n\nSee\nhttps://example.com\nwww.example.com\nperson@example.com\nnow.\n\n|a|long|\n|-|-|\n|b|c|\n";
        let expected = "# Heading\n\nSee\n[https://example.com](https://example.com)\n[www.example.com](http://www.example.com)\n[person@example.com](mailto:person@example.com)\nnow.\n\n| a   | long |\n| --- | ---- |\n| b   | c    |\n";
        assert_eq!(
            reflow(
                input,
                Options {
                    widths: EdgeWidths {
                        middle: ReflowWidth::Disabled,
                        ..EdgeWidths::default()
                    },
                    linkify_urls: true,
                    ..Options::default()
                }
            )
            .unwrap(),
            expected
        );
    }

    #[test]
    fn finite_zero_is_a_budget_and_does_not_disable_reflow() {
        assert_eq!(
            reflow("one two\nthree\n", uniform_options(0)).unwrap(),
            "one\ntwo\nthree\n"
        );
        // Prefix subtraction may also produce a zero-width content budget.
        assert_eq!(
            reflow("* one two\n", uniform_options(1)).unwrap(),
            "* one\n  two\n"
        );
    }

    #[test]
    fn test_simple_paragraph_reflow() {
        let input = "This is a paragraph that needs to be reflowed to a narrower width.\n";
        let result = reflow(input, uniform_options(40)).unwrap();
        assert_eq!(
            result,
            "This is a paragraph that needs to be\nreflowed to a narrower width.\n"
        );
    }

    #[test]
    fn edge_widths_reserve_final_line_by_splitting_late() {
        let input = "The expectation of the Gamma distribution with shape `shape` and rate `rate` is\n`shape / rate`.\n";
        let result = reflow(
            input,
            Options {
                widths: EdgeWidths {
                    first: ReflowWidth::Finite(96),
                    middle: ReflowWidth::Finite(100),
                    last: ReflowWidth::Finite(97),
                    single: ReflowWidth::Finite(93),
                },
                ..Options::default()
            },
        )
        .unwrap();
        assert_eq!(
            result,
            "The expectation of the Gamma distribution with shape `shape` and rate `rate` is\n`shape / rate`.\n"
        );
    }

    #[test]
    fn test_headings_not_combined() {
        let input = "# Hello\n# World\n";
        let result = reflow(input, uniform_options(80)).unwrap();
        assert_eq!(result, "# Hello\n\n# World\n");
    }

    #[test]
    fn test_list_items_not_combined() {
        let input = "* First item\n* Second item\n";
        let result = reflow(input, uniform_options(80)).unwrap();
        assert_eq!(result, input);
    }

    #[test]
    fn test_inline_formatting_preserved() {
        let input = "This is **bold** and *italic* text.\n";
        let result = reflow(input, uniform_options(80)).unwrap();
        assert_eq!(result, input);
    }

    #[test]
    fn test_code_block_preserved() {
        let input = "```rust\nlet x = 1;\nlet y = 2;\n```\n";
        let result = reflow(input, uniform_options(80)).unwrap();
        assert_eq!(result, input);
    }

    #[test]
    fn test_blockquote_preserved() {
        let input = "> A blockquote with some text.\n";
        let result = reflow(input, uniform_options(80)).unwrap();
        assert_eq!(result, input);
    }

    #[test]
    fn test_reflow_narrow() {
        let input = "The quick brown fox jumps over the lazy dog.\n";
        let result = reflow(input, uniform_options(20)).unwrap();
        assert_eq!(result, "The quick brown fox\njumps over the lazy\ndog.\n");
    }

    #[test]
    fn test_reflow_preserves_emphasis_across_lines() {
        let input =
            "This is a long paragraph with **bold text that might need to span** across lines.\n";
        let result = reflow(input, uniform_options(40)).unwrap();
        assert_eq!(
            result,
            "This is a long paragraph with **bold\ntext that might need to span** across\nlines.\n"
        );
    }

    // -----------------------------------------------------------------------
    // Edge cases & real-world scenarios
    // -----------------------------------------------------------------------

    #[test]
    fn test_links_preserved() {
        let input = "See [the docs](https://example.com) for more.\n";
        let result = reflow(input, uniform_options(80)).unwrap();
        assert_eq!(result, input);
    }

    #[test]
    fn test_link_with_inline_formatting() {
        let input = "See **[the docs](https://example.com)** for more.\n";
        let result = reflow(input, uniform_options(80)).unwrap();
        assert_eq!(result, input);
    }

    #[test]
    fn test_ordered_list_preserved() {
        let input = "1. First\n2. Second\n3. Third\n";
        let result = reflow(input, uniform_options(80)).unwrap();
        assert_eq!(result, input);
    }

    #[test]
    fn test_nested_blockquote() {
        let input = "> > Nested quote\n";
        let result = reflow(input, uniform_options(80)).unwrap();
        assert_eq!(result, input);
    }

    #[test]
    fn test_thematic_break_preserved() {
        let input = "Before\n\n---\n\nAfter\n";
        let result = reflow(input, uniform_options(80)).unwrap();
        assert_eq!(result, input);
    }

    #[test]
    fn test_image_preserved() {
        let input = "Look at ![alt text](image.png).\n";
        let result = reflow(input, uniform_options(80)).unwrap();
        assert_eq!(result, input);
    }

    #[test]
    fn test_inline_code_not_split() {
        // Inline code is atomic — even at narrow widths it stays intact.
        let input = "Run `cargo build --release` now.\n";
        let result = reflow(input, uniform_options(25)).unwrap();
        assert_eq!(result, "Run\n`cargo build --release`\nnow.\n");
    }

    #[test]
    fn test_hard_break_preserved() {
        let input = "Line one.\\\nLine two.\n";
        let result = reflow(input, uniform_options(80)).unwrap();
        assert_eq!(result, input);
    }

    #[test]
    fn test_mixed_bold_italic() {
        let input = "This is ***bold and italic*** and **just bold** and *just italic*.\n";
        let result = reflow(input, uniform_options(80)).unwrap();
        assert_eq!(result, input);
    }

    #[test]
    fn test_multiple_paragraphs_blank_line() {
        let input = "First paragraph.\n\nSecond paragraph.\n";
        let result = reflow(input, uniform_options(80)).unwrap();
        assert_eq!(result, input);
    }

    #[test]
    fn test_double_space_sentence_end_preserved() {
        let input = "First sentence.  Second sentence.\n";
        let result = reflow(input, uniform_options(80)).unwrap();
        assert_eq!(result, input);
    }

    #[test]
    fn test_unicode_preserved() {
        let input = "Café résumé naïve — très élégant.\n";
        let result = reflow(input, uniform_options(80)).unwrap();
        assert_eq!(result, input);
    }

    #[test]
    fn test_empty_paragraph() {
        let input = "Before\n\n\n\nAfter\n";
        let result = reflow(input, uniform_options(80)).unwrap();
        assert_eq!(result, "Before\n\nAfter\n");
    }

    #[test]
    fn test_very_long_word_force_break() {
        // A word longer than the target width must still be emitted.
        let input = "AReallyLongWordThatExceedsTheTargetWidth significantly.\n";
        let result = reflow(input, uniform_options(20)).unwrap();
        assert_eq!(
            result,
            "AReallyLongWordThatExceedsTheTargetWidth\nsignificantly.\n"
        );
    }

    #[test]
    fn test_reflow_to_wider_width_reduces_lines() {
        let input =
            "This paragraph is wrapped tightly.\nIt has several lines that could\nbe merged.\n";
        let narrow = reflow(input, uniform_options(30)).unwrap();
        let wide = reflow(input, uniform_options(200)).unwrap();
        assert_eq!(
            narrow,
            "This paragraph is wrapped\ntightly. It has several lines\nthat could be merged.\n"
        );
        assert_eq!(
            wide,
            "This paragraph is wrapped tightly. It has several lines that could be merged.\n"
        );
    }

    #[test]
    fn test_reflow_at_small_width() {
        let input = "Short word.\n";
        let result = reflow(input, uniform_options(6)).unwrap();
        assert_eq!(result, "Short\nword.\n");
    }

    #[test]
    fn test_yaml_frontmatter_preserved() {
        let input = "---\ntitle: Test\nauthor: Me\n---\n\nParagraph.\n";
        let result = reflow(input, uniform_options(80)).unwrap();
        assert_eq!(result, input);
    }

    #[test]
    fn yaml_frontmatter_contents_are_opaque_in_every_reflow_mode() {
        let frontmatter = concat!(
            "---\n",
            "# Metadata comment\n",
            "title: '**Literal Markdown**'\n",
            "url: https://example.com/a/very/long/path\n",
            "tags: [one, 'two words']\n",
            "author:\n",
            "  name: \"Me\"\n",
            "  active: true\n",
            "description: |\n",
            "  Keep these lines separate.\n",
            "    And preserve this indentation.\n",
            "\n",
            "summary: >-\n",
            "  Do not join these\n",
            "  lines in the YAML source.\n",
            "---\n",
        );
        let input = format!("{frontmatter}\nBody text\nwraps here.\n");
        for (options, body) in [
            (uniform_options(10), "Body text\nwraps\nhere.\n"),
            (Options::default(), "Body text wraps here.\n"),
            (
                Options {
                    widths: EdgeWidths {
                        middle: ReflowWidth::Disabled,
                        ..EdgeWidths::default()
                    },
                    ..Options::default()
                },
                "Body text\nwraps here.\n",
            ),
        ] {
            let options = Options {
                linkify_urls: true,
                ..options
            };
            assert_eq!(
                reflow(&input, options).unwrap(),
                format!("{frontmatter}\n{body}"),
                "options: {options:?}"
            );
        }
    }

    #[test]
    fn yaml_frontmatter_can_be_empty_or_the_entire_document() {
        for input in [
            "---\n---\n",
            "---\ntitle: Test\n---\n",
            "---\n\n---\n",
            "---\ntitle: Test\n\n\n---\n",
            "---  \ntitle: Test  \n--- \n",
            "---\r\ntitle: Test\r\n\r\n---\r\n",
        ] {
            assert_eq!(reflow(input, uniform_options(1)).unwrap(), input);
        }
        assert_eq!(
            reflow("---\ntitle: Test\n---", Options::default()).unwrap(),
            "---\ntitle: Test\n---\n"
        );
    }

    #[test]
    fn toml_frontmatter_preserved() {
        for input in ["+++\n+++\n", "+++\ntitle = 'Test'\n\n+++\n\nParagraph.\n"] {
            assert_eq!(reflow(input, uniform_options(80)).unwrap(), input);
        }
    }

    #[test]
    fn frontmatter_recognition_does_not_consume_body_thematic_breaks() {
        let input = "---\ntitle: Test\n---\n\nBefore.\n\n---\n\nAfter.\n";
        assert_eq!(reflow(input, Options::default()).unwrap(), input);
        let input = "Before.\n\n---\n\nAfter.\n\n---\n";
        assert_eq!(reflow(input, Options::default()).unwrap(), input);
        let input = "---\n\nParagraph without a closing fence.\n";
        assert_eq!(reflow(input, Options::default()).unwrap(), input);
    }

    #[test]
    fn test_definition_preserved() {
        let input = "[ref]: https://example.com \"Title\"\n\nSee [ref] for more.\n";
        let result = reflow(input, uniform_options(80)).unwrap();
        assert_eq!(result, input);
    }

    #[test]
    fn test_footnote_preserved() {
        let input = "Text with footnote[^1].\n\n[^1]: The footnote.\n";
        let result = reflow(input, uniform_options(80)).unwrap();
        assert_eq!(result, input);
    }

    #[test]
    fn test_bare_url_left_alone_by_default() {
        let input = "See https://example.com/docs for details.\n";
        assert_eq!(reflow(input, uniform_options(80)).unwrap(), input);
    }

    #[test]
    fn test_bare_url_linkified_with_option() {
        let input = "See https://example.com/docs for details.\n";
        let result = reflow(
            input,
            Options {
                linkify_urls: true,
                ..uniform_options(80)
            },
        )
        .unwrap();
        assert_eq!(
            result,
            "See [https://example.com/docs](https://example.com/docs) for details.\n"
        );
    }

    #[test]
    fn test_explicit_link_kept_without_linkify() {
        // Only bare URLs are affected — explicit links always stay links.
        let input = "See [the docs](https://example.com) for more.\n";
        assert_eq!(reflow(input, uniform_options(80)).unwrap(), input);
    }

    #[test]
    fn test_table_preserved() {
        let input = "| A | B |\n| --- | --- |\n| 1 | 2 |\n";
        let result = reflow(input, uniform_options(80)).unwrap();
        assert_eq!(result, "| A   | B   |\n| --- | --- |\n| 1   | 2   |\n");
    }

    #[test]
    fn quoted_trailing_pipes_do_not_hide_later_tables() {
        let table = "| a   | b   |\n| --- | --- |\n| c   | d   |\n";
        for quote in [
            "> a|\n>\n",
            "> |\n>\n",
            "> a\\\\|\n>\n",
            "> * a|\n>\n",
            "> > a|\n> >\n",
            "* > a|\n  >\n",
            "1. > a|\n   >\n",
        ] {
            // An empty quote line clears the parser's pending table header.
            // Keep it even when a blank line separates the following block.
            for between in ["\n", "\n```\ncode\n```\n\n", "\n# Heading\n\n"] {
                let input = format!("{quote}{between}{table}");
                let output = reflow(&input, uniform_options(20)).unwrap();
                assert_eq!(output, input, "quote {quote:?}, between {between:?}");
                assert_eq!(reflow(&output, uniform_options(20)).unwrap(), output);
                assert_eq!(
                    meaning(&output, uniform_options(20)),
                    meaning(&input, uniform_options(20))
                );
            }
        }
        // Escaped pipes and pipes inside code or actual tables do not
        // leave a pending header, so they need no extra quote line.
        for quote in [
            "> a\\|\n",
            "> a&#124;\n",
            "> `|`\n",
            "> | a   |\n> | --- |\n> | b   |\n",
        ] {
            let input = format!("{quote}\n{table}");
            let output = reflow(&input, uniform_options(20)).unwrap();
            assert_eq!(output, input);
            assert_eq!(reflow(&output, uniform_options(20)).unwrap(), output);
        }
    }

    #[test]
    fn table_after_quoted_pipe_fuzz_regression() {
        // Fuzz case u_4923, seed 22: reflow used to remove the empty quote
        // line that reset table detection, destroying the table on pass 2.
        let input = "> > éé alpha `code` éé [^n] c\n1. > --- # word ---`code`|\n>\n\t & *em* \\* <span>\n| a | b |\n| - | - |\n| = éé [ 😀\n\n  * ## *em* \t $ ***c***  \n  * & `` a`b `` éé\n  \n[ \\\\ ~~del~~ \\\n";
        let options = uniform_options(20);
        let output = reflow(input, options).unwrap();
        assert_eq!(
            output,
            "> > éé alpha `code`\n> > éé [^n] c\n\n1. > --- # word\n   > ---`code`|\n   >\n\n>\n\n```\n & *em* \\* <span>\n```\n\n| a        | b   |\n| -------- | --- |\n| = éé [ 😀 |     |\n\n* ## *em*   $ ***c***\n* & `` a`b `` éé\n\n[ \\\\ ~~del~~ \\\n"
        );
        assert_eq!(reflow(&output, options).unwrap(), output);
        // The parser changes an item's spread flag for the added quote
        // line, even though the item holds only a quote. Compare HTML here
        // to check its rendered meaning, allowing reflowed whitespace.
        let html = |source| {
            markdown::to_html_with_options(
                source,
                &markdown::Options {
                    parse: parse_options(options),
                    ..markdown::Options::default()
                },
            )
            .unwrap()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
        };
        assert_eq!(html(&output), html(input));
    }

    #[test]
    fn test_table_column_padding_is_preserved() {
        // Human-readable alignment padding must survive a round trip.
        let input = "\
| node            | mover | Stop payoff |
| --------------- | ----- | ----------- |
| round 1 (root)  | 0     | `(1, 0)`    |
| round 4 (final) | 1     | `(0, 4)`    |
";
        let result = reflow(input, uniform_options(100)).unwrap();
        assert_eq!(result, input);
    }

    #[test]
    fn test_table_unpadded_columns_are_aligned() {
        // A ragged table is canonicalized to padded, aligned columns.
        let input = "| node | mover |\n| --- | --- |\n| round 1 (root) | 0 |\n";
        let result = reflow(input, uniform_options(100)).unwrap();
        assert_eq!(
            result,
            "\
| node           | mover |
| -------------- | ----- |
| round 1 (root) | 0     |
"
        );
    }

    #[test]
    fn test_table_alignment_markers_stretch_to_width() {
        let input = "| left | right | center |\n| :-- | --: | :-: |\n| a | b | c |\n";
        let result = reflow(input, uniform_options(100)).unwrap();
        assert_eq!(
            result,
            "\
| left | right | center |
| :--- | ----: | :----: |
| a    | b     | c      |
"
        );
    }

    #[test]
    fn test_list_item_with_nested_paragraph() {
        let input = "* Item one.\n\n  Paragraph under item.\n* Item two.\n";
        let result = reflow(input, uniform_options(80)).unwrap();
        assert_eq!(result, input);
    }

    #[test]
    fn test_line_width_hard_cap() {
        // At various widths, no line (excluding code blocks) exceeds the cap.
        let input = "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat.\n";
        for width in [20, 40, 60, 80, 100] {
            let result = reflow(input, uniform_options(width)).unwrap();
            assert_wrapped_text(&result, input, width, "", "");
        }
    }

    #[test]
    fn test_delete_formatting_preserved() {
        let input = "This is ~~deleted~~ text.\n";
        let result = reflow(input, uniform_options(80)).unwrap();
        assert_eq!(result, input);
    }

    #[test]
    fn test_no_trailing_whitespace_on_lines() {
        let input = "A paragraph that gets reflowed to a specific width.\n";
        let result = reflow(input, uniform_options(30)).unwrap();
        // The first line exactly fills the width; the space after it does
        // not count toward the line.
        assert_eq!(
            result,
            "A paragraph that gets reflowed\nto a specific width.\n"
        );
    }

    #[test]
    fn test_force_break_unbreakable_word() {
        // A word longer than the target width gets its own line.
        let input = "Supercalifragilisticexpialidocious.\n";
        let result = reflow(input, uniform_options(20)).unwrap();
        assert_eq!(result, input);
    }

    #[test]
    fn test_list_items_respect_width() {
        // List items should not exceed target_width after adding the bullet marker.
        let input = "* This is a bullet point that should be reflowed to stay within the character limit.\n";
        for width in [20, 40, 60, 80] {
            let result = reflow(input, uniform_options(width)).unwrap();
            assert_wrapped_text(
                &result,
                input.strip_prefix("* ").unwrap(),
                width,
                "* ",
                "  ",
            );
        }
    }

    #[test]
    fn test_ordered_list_items_respect_width() {
        let input = "1. This is an ordered bullet point that should be reflowed to stay within the character limit.\n";
        for width in [20, 40, 60, 80] {
            let result = reflow(input, uniform_options(width)).unwrap();
            assert_wrapped_text(
                &result,
                input.strip_prefix("1. ").unwrap(),
                width,
                "1. ",
                "   ",
            );
        }
    }

    #[test]
    fn test_blockquote_respects_width() {
        let input =
            "> This is a blockquote that should be reflowed to stay within the character limit.\n";
        for width in [20, 40, 60, 80] {
            let result = reflow(input, uniform_options(width)).unwrap();
            assert_wrapped_text(
                &result,
                input.strip_prefix("> ").unwrap(),
                width,
                "> ",
                "> ",
            );
        }
    }

    #[test]
    fn wrapping_never_starts_a_line_with_block_syntax() {
        for (input, width, expected) in [
            // Bullet markers move with the preceding word.
            (
                "aaaa bbbb - cccc + dddd * eeee",
                9,
                "aaaa\nbbbb -\ncccc +\ndddd *\neeee\n",
            ),
            (
                "aaaa bbbb - cccc + dddd * eeee",
                11,
                "aaaa bbbb -\ncccc +\ndddd * eeee\n",
            ),
            // Ordered list markers with any start number.
            (
                "aa bb 1. cc 2) dd 123. ee",
                6,
                "aa\nbb 1.\ncc 2)\ndd 123.\nee\n",
            ),
            ("aa bb > cc ## dd", 6, "aa\nbb >\ncc ##\ndd\n"),
            (
                "aa bb ``` cc ~~~ dd $$ ee",
                6,
                "aa\nbb ```\ncc ~~~\ndd $$\nee\n",
            ),
            ("aa bb <div> cc </p> dd", 6, "aa\nbb <div>\ncc </p>\ndd\n"),
            ("aaa bb [x]: cc", 6, "aaa\nbb [x]:\ncc\n"),
            ("aa bb [^1]: cc", 6, "aa\nbb [^1]:\ncc\n"),
            // Setext underlines, thematic breaks, and table delimiter rows
            // depend on the whole line.
            ("aaa bb == cc", 6, "aaa bb\n== cc\n"),
            ("aaa bb ==", 6, "aaa\nbb ==\n"),
            // An underline of `=` cannot contain spaces.
            ("aaa bb == ==", 6, "aaa bb\n== ==\n"),
            ("aaa bb --- cc ___", 6, "aaa bb\n--- cc ___\n"),
            ("aaa bb cc ___ ___", 6, "aaa bb\ncc ___ ___\n"),
            ("ab | - | - |", 7, "ab | - | -\n|\n"),
        ] {
            let output = reflow(input, uniform_options(width)).unwrap();
            assert_eq!(output, expected, "input {input:?} at width {width}");
            assert_eq!(reflow(&output, uniform_options(width)).unwrap(), output);
        }
    }

    #[test]
    fn wrapping_never_breaks_after_a_trailing_backslash() {
        // A backslash at the end of a line is a hard break.
        assert_eq!(
            reflow("ab cd\\ ef", uniform_options(4)).unwrap(),
            "ab\ncd\\ ef\n"
        );
    }

    #[test]
    fn overlong_token_does_not_push_earlier_words_onto_their_own_lines() {
        assert_eq!(
            reflow(
                "aa bb cc dd `xxxxxxxxxxxxxxxxxxxx` ee ff\n",
                uniform_options(12)
            )
            .unwrap(),
            "aa bb cc dd\n`xxxxxxxxxxxxxxxxxxxx`\nee ff\n"
        );
    }

    #[test]
    fn line_exactly_filling_the_width_is_kept_mid_paragraph() {
        let first = "a".repeat(25);
        assert_eq!(
            reflow(&format!("{first} bbbb\ncc dd\n"), uniform_options(30)).unwrap(),
            format!("{first} bbbb\ncc dd\n")
        );
    }

    #[test]
    fn forced_breaks_occur_only_at_existing_whitespace() {
        for (input, expected) in [
            ("`long`. x", "`long`.\nx\n"),
            ("a (`x`) b", "a\n(`x`)\nb\n"),
            ("a *bbbbbb*. c", "a\n*bbbbbb*.\nc\n"),
        ] {
            assert_eq!(reflow(input, uniform_options(4)).unwrap(), expected);
        }
    }

    #[test]
    fn link_destinations_and_void_inlines_count_toward_width() {
        assert_eq!(
            reflow(
                "see [the docs](https://example.com/a) now",
                uniform_options(20)
            )
            .unwrap(),
            "see [the\ndocs](https://example.com/a)\nnow\n"
        );
        // `![i](u "t")` takes 11 columns, `[^note]` 7, and `![r][]` 6.
        assert_eq!(
            reflow("a ![i](u \"t\") b [^note] c ![r][] d", uniform_options(11)).unwrap(),
            "a\n![i](u \"t\")\nb [^note] c\n![r][] d\n"
        );
    }

    #[test]
    fn no_break_space_is_not_a_break_opportunity() {
        assert_eq!(
            reflow("a\u{a0}b c\u{a0}d\n", uniform_options(3)).unwrap(),
            "a\u{a0}b\nc\u{a0}d\n"
        );
    }

    #[test]
    fn inline_html_stays_on_its_line() {
        let input = "aa <span>x</span> bb\n";
        assert_eq!(reflow(input, Options::default()).unwrap(), input);
        assert_eq!(
            reflow("aa <span>x</span> bb\n", uniform_options(8)).unwrap(),
            "aa <span>x</span>\nbb\n"
        );
    }

    #[test]
    fn frontmatter_recognition_can_be_disabled() {
        let options = Options {
            frontmatter: false,
            ..Options::default()
        };
        // A leading `+++` line is paragraph text, not a TOML fence.
        assert_eq!(
            reflow("+++\na\n+++\nb\n", options).unwrap(),
            "+++ a +++ b\n"
        );
        assert_eq!(
            reflow("---\nTitle\n---\nBody.\n", options).unwrap(),
            "---\n\n## Title\n\nBody.\n"
        );
        // The default keeps recognizing frontmatter.
        assert_eq!(
            reflow("+++\na\n+++\nb\n", Options::default()).unwrap(),
            "+++\na\n+++\n\nb\n"
        );
    }

    /// Node kinds and atomic inline values, ignoring text and positions, so
    /// that line breaks inside text do not change the signature.
    fn structure(markdown: &str) -> String {
        fn walk(node: &Node, out: &mut String) {
            match node {
                Node::Text(_) => {
                    if !out.ends_with("Text ") {
                        out.push_str("Text ");
                    }
                    return;
                }
                Node::InlineCode(code) => write!(out, "Code({}) ", code.value).unwrap(),
                Node::Html(html) => write!(out, "Html({}) ", html.value).unwrap(),
                Node::Link(link) => write!(out, "Link({}) ", link.url).unwrap(),
                Node::Image(image) => write!(out, "Image({}) ", image.url).unwrap(),
                other => {
                    let name = format!("{other:?}");
                    out.push_str(&name[..name.find(['(', ' ', '{']).unwrap_or(name.len())]);
                    out.push(' ');
                }
            }
            if let Some(children) = node.children() {
                out.push_str("[ ");
                for child in children {
                    walk(child, out);
                }
                out.push_str("] ");
            }
        }
        let options = markdown::ParseOptions {
            constructs: markdown::Constructs {
                gfm_table: true,
                gfm_strikethrough: true,
                gfm_footnote_definition: true,
                gfm_label_start_footnote: true,
                ..markdown::Constructs::default()
            },
            ..markdown::ParseOptions::default()
        };
        let mut out = String::new();
        walk(&markdown::to_mdast(markdown, &options).unwrap(), &mut out);
        out
    }

    #[test]
    fn generated_paragraphs_keep_their_structure_and_reach_a_fixed_point() {
        const WORDS: &[&str] = &[
            "alpha",
            "be",
            "c",
            "word",
            "-",
            "+",
            "*",
            "1.",
            "2)",
            "123456789.",
            "1234567890.",
            ">",
            "#",
            "######",
            "#######",
            "```",
            "~~~",
            "``",
            "$$",
            "<div>",
            "</p>",
            "<",
            "=",
            "==",
            "---",
            "___",
            "***",
            "|",
            "|-|",
            ":-:",
            "-|-",
            "[a]:",
            "[^1]:",
            "[a",
            "b]:",
            "foo\\",
            "`code`",
            "`x`.",
            "(`y`)",
            "*em*",
            "**strong text**",
            "~~gone~~",
            "[link](https://example.com/long/url)",
            "[a b](u \"t\")",
            "![img](u)",
            "<span>x</span>",
            "longwordlongwordlongword",
            "a\u{a0}b",
            "x:",
        ];
        const PREFIXES: &[&str] = &["", "* ", "> ", "1. ", "7) ", "> * "];
        let mut state: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = |bound: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % bound as u64) as usize
        };
        for case in 0..3000 {
            let count = 1 + next(14);
            let mut words = vec!["Start"];
            words.extend((0..count).map(|_| WORDS[next(WORDS.len())]));
            let prefix = PREFIXES[case % PREFIXES.len()];
            let input = format!("{prefix}{}\n", words.join(" "));
            let joined = reflow(&input, Options::default()).unwrap();
            assert_eq!(
                structure(&joined),
                structure(&input),
                "structure changed without wrapping: {input:?} -> {joined:?}"
            );
            let width = 1 + next(30);
            // Reserve room for delimiters on the first and last lines, as a
            // caller wrapping comment text would.
            let (opening, closing) = if case % 2 == 0 {
                (0, 0)
            } else {
                (next(5), next(5))
            };
            let options = Options {
                widths: EdgeWidths {
                    first: ReflowWidth::Finite(width.saturating_sub(opening)),
                    middle: ReflowWidth::Finite(width),
                    last: ReflowWidth::Finite(width.saturating_sub(closing)),
                    single: ReflowWidth::Finite(width.saturating_sub(opening + closing)),
                },
                ..Options::default()
            };
            let output = reflow(&input, options).unwrap();
            assert_eq!(
                structure(&output),
                structure(&input),
                "structure changed with {options:?}: {input:?} -> {output:?}"
            );
            assert_eq!(
                reflow(&output, Options::default()).unwrap(),
                joined,
                "content changed with {options:?}: {input:?} -> {output:?}"
            );
            assert_eq!(
                reflow(&output, options).unwrap(),
                output,
                "not a fixed point with {options:?}: {input:?}"
            );
        }
    }

    fn disabled() -> Options {
        Options {
            widths: EdgeWidths {
                middle: ReflowWidth::Disabled,
                ..EdgeWidths::default()
            },
            ..Options::default()
        }
    }

    fn linkify() -> Options {
        Options {
            linkify_urls: true,
            ..Options::default()
        }
    }

    #[test]
    fn escapes_and_character_references_keep_their_source() {
        for input in [
            "a\\*b\\*c costs \\$5 and \\$6.\n",
            "\\# not a heading \\_x\\_ \\` \\< \\[x\\] \\\\ \\~\\~y\\~\\~\n",
            // Escapes that turn into a Lean comment delimiter when dropped.
            "Ratio a-\\/b, -&#47; and /\\- stay.\n",
            "&copy; &amp;amp; &#x2F; &nbsp;x &bogus; &\n",
            "> quoted \\*text\\*\n",
        ] {
            assert_eq!(reflow(input, Options::default()).unwrap(), input);
        }
        // Escapes survive wrapping, across container prefixes.
        assert_eq!(
            reflow("> a \\* b\n> \\> c\n", uniform_options(5)).unwrap(),
            "> a\n> \\*\n> b\n> \\>\n> c\n"
        );
    }

    #[test]
    fn code_spans_keep_a_fence_that_holds_their_backticks() {
        let input = "`` `u` `` and `` a`b `` and ` `` ` end\n";
        assert_eq!(reflow(input, Options::default()).unwrap(), input);
        // A span across lines is rendered on one line from its content.
        assert_eq!(
            reflow("> ``a`\n> b``\n", Options::default()).unwrap(),
            "> ``a` b``\n"
        );
        // A paragraph drops its continuation lines' indentation before the
        // span is read.
        assert_eq!(
            reflow("aaa `bbb  \n   ccc` ddd\n", Options::default()).unwrap(),
            "aaa `bbb   ccc` ddd\n"
        );
    }

    #[test]
    fn linkified_urls_are_not_linkified_again() {
        let input = "[https://x.com](https://x.com) <https://y.com> and <me@example.com>\n";
        assert_eq!(reflow(input, linkify()).unwrap(), input);
        let once = reflow("See www.z.com and https://a.com/x_y.\n", linkify()).unwrap();
        assert_eq!(
            once,
            "See [www.z.com](http://www.z.com) and [https://a.com/x_y](https://a.com/x_y).\n"
        );
        assert_eq!(reflow(&once, linkify()).unwrap(), once);
        // A URL stays bare where brackets would make an image or change
        // emphasis.
        let input = "Hi!www.z.com and *www.z.com* and _a_www.z.com and https://a.com/x_y_.\n";
        assert_eq!(reflow(input, linkify()).unwrap(), input);
    }

    #[test]
    fn reference_definitions_keep_their_labels_and_stay_definitions() {
        let input = "See [Foo \\* Bar][X y].\nText.\n\n[x  Y]: <a b> \"t \\\" q\"\n[other]: /url\n";
        assert_eq!(
            reflow(input, Options::default()).unwrap(),
            "See [Foo \\* Bar][X y]. Text.\n\n[x  Y]: <a b> \"t \\\" q\"\n[other]: /url\n"
        );
        // A definition keeps the blank line that separates it from the
        // paragraph before it.
        let input = "Para.\n\n[ref]: /url\n";
        assert_eq!(reflow(input, Options::default()).unwrap(), input);
        // A collapsed or shortcut reference is its own label, so it is not
        // wrapped.
        assert_eq!(
            reflow("x [a b c] y\n\n[a b c]: /u\n", uniform_options(3)).unwrap(),
            "x\n[a b c]\ny\n\n[a b c]: /u\n"
        );
        // Wrapping must not complete a definition from paragraph text, but
        // may break once the line holds text that no definition allows.
        assert_eq!(
            reflow("[a]: b c d\n", uniform_options(4)).unwrap(),
            "[a]: b c\nd\n"
        );
        for input in ["[a]: /u \"t x\" more\n", "[a]:\n/u\n"] {
            let output = reflow(input, uniform_options(4)).unwrap();
            assert_eq!(
                structure(&output),
                structure(input),
                "{input:?} -> {output:?}"
            );
        }
        let input = "[Bou]: N. Bourbaki, Elements of Mathematics, General Topology, Chapters 1-4,\n\
                     Springer-Verlag, Berlin, 1989, reprint of the 1966 edition, see also the notes\n\
                     at the end of chapter two for the history of uniform spaces.\n";
        assert_eq!(reflow(input, uniform_options(80)).unwrap(), input);
    }

    #[test]
    fn link_destinations_and_titles_are_reencoded() {
        assert_eq!(
            reflow(
                "[a](<b c>) [d](e\\)f) [g](h(i)j) [k](l 'say \"hi\"') [m](n&amp;amp;o)\n",
                Options::default()
            )
            .unwrap(),
            "[a](<b c>) [d](<e)f>) [g](h(i)j) [k](l \"say \\\"hi\\\"\") [m](n\\&amp;o)\n"
        );
    }

    #[test]
    fn emphasis_keeps_its_delimiters() {
        let input = "_a_ *b* __c__ **d** ~e~ ~~f~~ _*g*_ __*h*__ ***i*** _k *l_ m*\n";
        assert_eq!(reflow(input, Options::default()).unwrap(), input);
        assert_eq!(
            reflow("a _bb cc_ d\n", uniform_options(4)).unwrap(),
            "a\n_bb\ncc_\nd\n"
        );
    }

    #[test]
    fn lists_keep_their_looseness_and_separation() {
        // A blank line between any two items separates them all.
        assert_eq!(
            reflow("* a\n\n* b\n* c\n", Options::default()).unwrap(),
            "* a\n\n* b\n\n* c\n"
        );
        for input in [
            "* a\n  * b\n* c\n\n  d\n* e\n",
            "* [ ] todo\n* [x] done\n",
            "*\n* a\n",
            "10. ten more\n11. eleven\n",
        ] {
            assert_eq!(reflow(input, Options::default()).unwrap(), input);
        }
        // Adjacent lists need different markers to stay apart.
        assert_eq!(
            reflow("- a\n* b\n+ c\n", Options::default()).unwrap(),
            "* a\n\n- b\n\n* c\n"
        );
        assert_eq!(
            reflow("1. a\n1) b\n", Options::default()).unwrap(),
            "1. a\n\n1) b\n"
        );
        assert_eq!(
            reflow("3) three\n4) four\n", Options::default()).unwrap(),
            "3. three\n4. four\n"
        );
        // A bullet must not turn its item into a thematic break.
        assert_eq!(reflow("- **\n", Options::default()).unwrap(), "- **\n");
        // A thematic break after a paragraph in a tight item is not `---`.
        let input = "* para\n  ***\n";
        assert_eq!(reflow(input, Options::default()).unwrap(), input);
        // A line of spaces, or of container prefixes, is a blank line too,
        // but a lazy continuation line is not.
        for (input, expected) in [
            ("* + x\n \n  w\n", "* * x\n\n  w\n"),
            ("> * + x\n> \n>   w\n", "> * * x\n>\n>   w\n"),
            ("* * a\nb\n  * c\n", "* * a b\n  * c\n"),
        ] {
            assert_eq!(reflow(input, Options::default()).unwrap(), expected);
        }
        // A paragraph after a block quote in a tight item stays apart.
        let input = "* > a\n  >\n  b\n";
        assert_eq!(reflow(input, Options::default()).unwrap(), input);
        // So does any block that cannot interrupt the quote's paragraph,
        // also after a quote that ends a nested item, while a block that
        // interrupts it needs no empty quote line.
        for (input, expected) in [
            ("- - > q\n    >\n  p\n- x\n", "* * > q\n    >\n  p\n* x\n"),
            (
                "- > q\n  >\n  | a |\n  | - |\n  | b |\n",
                "* > q\n  >\n  | a   |\n  | --- |\n  | b   |\n",
            ),
            (
                "- > q\n  >\n  [a]: /u\n\n[a]\n",
                "* > q\n  >\n  [a]: /u\n\n[a]\n",
            ),
            (
                "- > q\n  >\n  <n>\n  x\n- y\n",
                "* > q\n  >\n  <n>\n  x\n* y\n",
            ),
            ("- > q\n  - b\n", "* > q\n  * b\n"),
        ] {
            let once = reflow(input, Options::default()).unwrap();
            assert_eq!(once, expected);
            assert_eq!(reflow(&once, Options::default()).unwrap(), once);
        }
        // Lines of other characters, other line endings, and the empty line
        // of a block quote inside an item do not change looseness.
        for (input, expected) in [
            ("- a\n→\n  - b\n", "* a →\n  * b\n"),
            ("1. a\néé\n   - b", "1. a éé\n   * b\n"),
            ("- a\r\r  b\r", "* a\n\n  b\n"),
            ("- a\r\r- b\r", "* a\n\n* b\n"),
            ("- a\r\n\r\n  b\r\n", "* a\n\n  b\n"),
            (
                "- > q\n  >\n     ```\n     c\n     ```\n- x\n",
                "* > q\n  ```\n  c\n  ```\n* x\n",
            ),
            (
                "> - > q\n>   >\n>      ```\n>      c\n>      ```\n> - x\n",
                "> * > q\n>   ```\n>   c\n>   ```\n> * x\n",
            ),
        ] {
            assert_eq!(reflow(input, Options::default()).unwrap(), expected);
        }
        // Renumbered items keep markers of at most nine digits.
        let input = "999999999. a\n999999999. b\n";
        assert_eq!(reflow(input, Options::default()).unwrap(), input);
    }

    #[test]
    fn headings_keep_their_content() {
        assert_eq!(
            reflow("Foo #\n===\n", Options::default()).unwrap(),
            "# Foo \\#\n"
        );
        assert_eq!(
            reflow("#\n\n\\#a\n===\n", Options::default()).unwrap(),
            "#\n\n# \\#a\n"
        );
        assert_eq!(
            reflow("Foo\nbar\n---\n", Options::default()).unwrap(),
            "## Foo bar\n"
        );
        // Without reflow, or with a hard break, a heading stays setext.
        let input = "Foo\nbar\n---\n";
        assert_eq!(reflow(input, disabled()).unwrap(), input);
        let input = "Foo\\\nbar\n===\n";
        assert_eq!(reflow(input, Options::default()).unwrap(), input);
        // The parser drops a `#` run that starts ATX content; it is kept,
        // escaped, with its spacing.
        for (input, expected) in [
            ("# # Title\n", "# \\# Title\n"),
            ("# #a\n", "# \\#a\n"),
            ("## # x #\n", "## \\# x\n"),
            ("> # # # T\n", "> # \\# # T\n"),
            ("# # \\#\n", "# \\# \\#\n"),
            ("# # #\n", "# \\#\n"),
            ("# ### #\n", "# \\###\n"),
        ] {
            let once = reflow(input, Options::default()).unwrap();
            assert_eq!(once, expected);
            assert_eq!(reflow(&once, Options::default()).unwrap(), once);
        }
    }

    #[test]
    fn hard_breaks_keep_their_meaning() {
        assert_eq!(reflow("a  \nb\n", Options::default()).unwrap(), "a\\\nb\n");
        // After a backslash or an emphasis delimiter, the form matters.
        for input in ["x\\  \ny\n", "a **  \nb**\n", "a **\\\nb**\n"] {
            assert_eq!(reflow(input, Options::default()).unwrap(), input);
        }
    }

    #[test]
    fn unmovable_lines_escape_block_syntax() {
        // A lazy continuation line after a hard break gains the block
        // quote prefix and would become a setext underline.
        assert_eq!(
            reflow("> a\\\n===\n", Options::default()).unwrap(),
            "> a\\\n> \\===\n"
        );
        // An indented continuation line loses its indentation.
        assert_eq!(
            reflow("a\\\n    - b\n", Options::default()).unwrap(),
            "a\\\n\\- b\n"
        );
        // Text after a definition begins a paragraph of its own.
        assert_eq!(
            reflow("[a]: /u\n2. b\n", Options::default()).unwrap(),
            "[a]: /u\n\n2\\. b\n"
        );
        let input = "[a]: /u\n<span>\n";
        assert_eq!(reflow(input, Options::default()).unwrap(), input);
        // Without reflow, such source line breaks are joined instead.
        assert_eq!(reflow("> a\n===\n", disabled()).unwrap(), "> a ===\n");
        assert_eq!(reflow("a\n    - b\n", disabled()).unwrap(), "a - b\n");
    }

    #[test]
    fn first_lines_keep_what_kept_them_paragraphs() {
        // A lone tag would open an HTML block, and a fence without a
        // backtick after it would open code.
        assert_eq!(
            reflow("<span> word\n", uniform_options(3)).unwrap(),
            "<span> word\n"
        );
        assert_eq!(
            reflow("``` a`b c d\n", uniform_options(4)).unwrap(),
            "``` a`b\nc d\n"
        );
    }

    #[test]
    fn blocks_keep_their_separation_and_content() {
        for input in [
            "para\n\n<div>\nx\n</div>\n",
            "Text[^1].\n\n[^1]: First.\n\n    Second paragraph.\n",
            "````\n```\n````\n",
            "```\n```\n",
            "```text\na\n\n```\n",
            "~~~a`b\nx\n~~~\n",
            "> a\n>\n> b\n",
            // Joined, the tag would be alone on its line, an HTML block.
            "x\n\n<img src=\"a.png\"\nalt=\"b\">\n\n*y*\n",
        ] {
            assert_eq!(reflow(input, Options::default()).unwrap(), input);
        }
        assert_eq!(
            reflow("<img src=\"a.png\"\nalt=\"b\">\nmore\n", disabled()).unwrap(),
            "<img src=\"a.png\" alt=\"b\"> more\n"
        );
        assert_eq!(
            reflow("    indented\n", Options::default()).unwrap(),
            "```\nindented\n```\n"
        );
    }

    #[test]
    fn tables_escape_pipes_and_keep_their_columns() {
        assert_eq!(
            reflow(
                "| a | b |\n| :- | -: |\n| `x\\|y` | c \\| d |\n| e |\n| f | g | h |\n",
                Options::default()
            )
            .unwrap(),
            "| a      | b      |\n| :----- | -----: |\n| `x\\|y` | c \\| d |\n| e      |        |\n| f      | g      | h |\n"
        );
    }

    #[test]
    fn wide_whitespace_does_not_prevent_a_fixed_point() {
        let input = "aaaaaaaaaa.  bbbbbbbb cc\n";
        let output = reflow(input, uniform_options(20)).unwrap();
        assert_eq!(output, "aaaaaaaaaa. bbbbbbbb\ncc\n");
        assert_eq!(reflow(&output, uniform_options(20)).unwrap(), output);
        // Spacing is kept on a line that fits.
        let input = "First sentence.  Second.\n";
        assert_eq!(reflow(input, uniform_options(30)).unwrap(), input);
    }

    /// The syntax tree of `markdown` reduced to its meaning: positions are
    /// dropped, whitespace runs in text collapse to one space, adjacent text
    /// merges, links nested in links become text, and table rows are padded
    /// with empty cells to the header width, as HTML renderers do.
    fn meaning(markdown: &str, options: Options) -> String {
        fn collapse(value: &mut String) {
            let mut collapsed = String::new();
            for c in value.chars() {
                if !c.is_ascii_whitespace() {
                    collapsed.push(c);
                } else if !collapsed.ends_with(' ') {
                    collapsed.push(' ');
                }
            }
            *value = collapsed;
        }
        fn canonicalize(node: &mut Node) {
            node.position_set(None);
            // Line endings in code spans and inline HTML are spaces.
            let line_endings = |value: &mut String| {
                *value = value.replace("\r\n", " ").replace(['\r', '\n'], " ");
            };
            if let Node::InlineCode(InlineCode { value, .. })
            | Node::InlineMath(InlineMath { value, .. }) = node
            {
                line_endings(value);
            }
            if matches!(
                node,
                Node::Paragraph(_)
                    | Node::Heading(_)
                    | Node::TableCell(_)
                    | Node::Emphasis(_)
                    | Node::Strong(_)
                    | Node::Delete(_)
                    | Node::Link(_)
                    | Node::LinkReference(_)
            ) {
                for child in node.children_mut().into_iter().flatten() {
                    if let Node::Html(Html { value, .. }) = child {
                        line_endings(value);
                    }
                }
            }
            // Labels and alt text keep the line breaks of their source.
            match node {
                Node::LinkReference(LinkReference { label, .. })
                | Node::ImageReference(ImageReference { label, .. })
                | Node::FootnoteReference(FootnoteReference { label, .. }) => {
                    label.iter_mut().for_each(collapse);
                }
                _ => {}
            }
            if let Node::Image(Image { alt, .. })
            | Node::ImageReference(ImageReference { alt, .. }) = node
            {
                collapse(alt);
            }
            if let Node::Table(table) = node {
                let columns = table.align.len();
                for row in &mut table.children {
                    if let Node::TableRow(row) = row {
                        while row.children.len() < columns {
                            row.children.push(Node::TableCell(TableCell {
                                children: Vec::new(),
                                position: None,
                            }));
                        }
                    }
                }
            }
            if matches!(node, Node::Link(_) | Node::LinkReference(_)) {
                unwrap_links(node.children_mut().unwrap());
            }
            if let Some(children) = node.children_mut() {
                for child in children.iter_mut() {
                    canonicalize(child);
                }
                let mut merged: Vec<Node> = Vec::new();
                for child in std::mem::take(children) {
                    match (merged.last_mut(), child) {
                        (Some(Node::Text(previous)), Node::Text(text)) => {
                            previous.value.push_str(&text.value);
                        }
                        (_, child) => merged.push(child),
                    }
                }
                for child in &mut merged {
                    if let Node::Text(text) = child {
                        collapse(&mut text.value);
                    }
                }
                *children = merged;
            }
        }
        let mut tree = markdown::to_mdast(markdown, &parse_options(options)).unwrap();
        canonicalize(&mut tree);
        format!("{tree:#?}")
    }

    /// Inputs whose rendering is easy to get wrong.
    const ROUND_TRIP_CORPUS: &[&str] = &[
        // Blank lines of spaces or container prefixes, and lazy lines.
        "* + x\n \n  w\n",
        "> * + x\n> \n>   w\n",
        "* * a\nb\n  * c\n",
        "* > a\n  >\n  b\n",
        // Backslash escapes and character references.
        "a\\*b\\*c and costs \\$5 and \\$6.\n",
        "\\# not a heading\n",
        "\\_x\\_ \\` \\< \\[x\\] \\\\ \\~\\~y\\~\\~ \\> \\- \\+ \\1. \\| \\! \\&amp;\n",
        "Ratio a-\\/b, -&#47; and /\\- stay escaped.\n",
        "&copy; &amp;amp; &#x2F; &#10; &nbsp;x &bogus; & &#;\n",
        "> quoted \\*text\\*\n> continues &lt;here&gt;\n",
        "* item \\*one\\*\n  lazy \\_two\\_\n",
        // Code spans whose content holds backticks or edge spaces.
        "`` `u` `` and `` a`b `` and ``` a``b ``` and ` `` ` end\n",
        "`  a  ` and `   ` and ` a` and `a `\n",
        // Links, images, and autolinks.
        "[https://x.com](https://x.com) <https://y.com> <me@example.com> www.z.com https://a.com/x_y_\n",
        "[a](<b c>) [d](e\\)f) [g](h(i)j) [k](l 'say \"hi\"') [m](n&amp;amp;o) [p](<>)\n",
        "[a\nlink](https://example.com \"title\nlines\") and ![alt *x* \\]](img.png \"t\")\n",
        "![multi\nline](u) and ![ref][Img Label] and [^Note \\*]\n\n[img label]: i.png\n\n[^note \\*]: Footnote.\n",
        "<span>x</span> and <a href=\"y\">z</a>\n",
        // References and definitions.
        "See [Foo \\* Bar][X y] and [x y][] and [X Y].\nText.\n\n[x  Y]: <a b> \"t \\\" q\"\n[other]: /url\n",
        "Para\n[ref]: /not-a-definition\n",
        // Emphasis with every delimiter.
        "_a_ *b* __c__ **d** ~e~ ~~f~~ _*g*_ __*h*__ ***i*** *_j_* _k *l_ m*\n",
        "snake_case_name and a*b*c and 2*3*4\n",
        // Lists.
        "* a\n\n* b\n* c\n",
        "* a\n  * b\n* c\n\n  d\n* e\n",
        "3) three\n4) four\n\n1. one\n",
        "- a\n* b\n+ c\n",
        "1. a\n1) b\n",
        "*\n* a\n",
        "* [ ] todo\n* [x] done\n",
        "* para\n  ***\n* ```\n  code\n  ```\n",
        "10. ten\n    more\n11. eleven\n",
        // Block quotes, headings, breaks, and HTML.
        "> a\nlazy\n> > nested\n\n> b\n>\n> c\n",
        "Foo #\n===\n\nBar\nbaz\n---\n\nQux\\\nquux\n===\n\n# A # B #\n\n## \\#\n\n#\n",
        "hard  \nbreak and\\\nbackslash\n",
        "para\n<div>\nblock\n</div>\n\nafter\n",
        "para\n\n<!-- comment -->\nmore\n",
        // Tables.
        "> a|\n>\n\n| a | b |\n| - | - |\n| c | d |\n",
        "> * a|\n>\n\n| a | b |\n| - | - |\n| c | d |\n",
        "> > a|\n> >\n\n| a | b |\n| - | - |\n| c | d |\n",
        "| a | b |\n| :- | -: |\n| `x\\|y` | c \\| d |\n| e |\n| f | g | h |\n",
        "| *a* | [b](c) |\n| :-: | --- |\n| `|` | \\\\ |\n",
        // Footnotes and code blocks.
        "Text[^1].\n\n[^1]: First.\n\n    Second paragraph.\n[^2]: Other.\n",
        "```\n```\n\n```text\na\n\n```\n\n````\n```\n````\n\n~~~ a`b\nx\n~~~\n\n    indented\n",
        // Paragraph text from the README and other tests.
        "Some Markdown to reflow.\n",
        "Hello **bold\ntext** and [a\nlink](https://example.com).\nMore text.\n",
        "The expectation of the Gamma distribution with shape `shape` and rate `rate` is\n`shape / rate`.\n",
        "aaaa bbbb - cccc + dddd * eeee 1. ff 2) gg > hh # ii ``` jj $$ kk <div> ll\n",
        "a\u{a0}b c\u{a0}d and **bold\u{a0}**\n",
        "line\r\nending\r\n\r\nother\r\n",
        // Lines that cannot move and lines that a break could complete.
        "> a\\\n===\n",
        "a\\\n    - b\n",
        "[a]: /u\n2. b\n",
        "[a]: /u\n<span>\n",
        "<span> word\n",
        "``` a`b c d\n",
        "[a]: b c d\n",
        "#\n\n\\#a\n===\n",
        "  [^n]: x\n",
        // Hard breaks whose form matters.
        "x\\  \ny\n",
        "a **  \nb**\n",
        "a **\\\nb**\n",
        "a www.e.f  \nb\n",
        // Bare URLs next to syntax that brackets would change.
        "Hi!www.z.com and *www.z.com* and _a_www.z.com\n",
        "[ref]x@y.z and a www.e.f\\ b\n\n[ref]: /u\n",
        // Labels, destinations, and titles with unusual content.
        "x [a b c] y\n\n[a b c]: /u\n",
        "[a](b&#10;c) and [d](e \"f&#10;g\")\n",
        "> ``a`\n> b``\n",
        "aaaaaaaaaa.  bbbbbbbb cc\n",
        // Blank lines, lazy lines, and block quotes in tight items.
        "- a\n→\n  - b\n",
        "- a\r\r  b\r",
        "- a\r\n\r\n  b\r\n",
        "- - > q\n    >\n  p\n- x\n",
        "- > q\n  >\n  | a |\n  | - |\n  | b |\n",
        "- > q\n  >\n  [a]: /u\n\n[a]\n",
        "- > q\n  >\n  <n>\n  x\n- y\n",
        "- > > q\n  >\n  p\n- [^n]: > q\n  >\n  p\n\n[^n]\n",
        // Content the parser drops, numbers, labels, and inline HTML.
        "999999999. a\n999999999. b\n",
        "[Bou]: N. Bourbaki, Elements of Mathematics, General Topology, Chapters 1-4, Springer.\n",
        "x\n\n<img src=\"a.png\"\nalt=\"b\">\n\n*y*\n",
    ];

    #[test]
    fn reflow_preserves_meaning_and_reaches_a_fixed_point() {
        let uniform = |width| EdgeWidths {
            first: width,
            middle: width,
            last: width,
            single: width,
        };
        // Budgets that reserve room for comment delimiters, as leanck's do.
        let edges = |middle: usize| EdgeWidths {
            first: ReflowWidth::Finite(middle - 4),
            middle: ReflowWidth::Finite(middle),
            last: ReflowWidth::Finite(middle - 3),
            single: ReflowWidth::Finite(middle - 7),
        };
        let all_widths = [
            uniform(ReflowWidth::Disabled),
            uniform(ReflowWidth::Infinite),
            uniform(ReflowWidth::Finite(80)),
            uniform(ReflowWidth::Finite(20)),
            uniform(ReflowWidth::Finite(6)),
            edges(80),
            edges(12),
        ];
        for input in ROUND_TRIP_CORPUS {
            for widths in all_widths {
                for linkify_urls in [false, true] {
                    let options = Options {
                        widths,
                        linkify_urls,
                        frontmatter: false,
                    };
                    let output = reflow(input, options).unwrap();
                    assert_eq!(
                        meaning(&output, options),
                        meaning(input, options),
                        "meaning changed with {widths:?}, linkify {linkify_urls}: {input:?} -> {output:?}"
                    );
                    assert_eq!(
                        reflow(&output, options).unwrap(),
                        output,
                        "not a fixed point with {widths:?}, linkify {linkify_urls}: {input:?}"
                    );
                }
            }
        }
    }
}
