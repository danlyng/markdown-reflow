# markdown-reflow

Reflow Markdown paragraphs while preserving block structure, inline formatting, and explicit hard breaks. Available as a Rust library and a command-line tool.

## Library

The public API consists of `reflow`, `Options`, `EdgeWidths`, and `ReflowWidth`, exported from the crate root:

```rust
use markdown_reflow::{reflow, EdgeWidths, Options, ReflowWidth};

let options = Options {
    widths: EdgeWidths {
        first: ReflowWidth::Finite(80),
        middle: ReflowWidth::Finite(80),
        last: ReflowWidth::Finite(80),
        single: ReflowWidth::Finite(80),
    },
    linkify_urls: false,
    frontmatter: true,
};
let output = reflow("Some Markdown to reflow.", options)?;
```

Set all four budgets alike for uniform wrapping. Set different budgets to reserve space for external prefixes and suffixes, such as comment delimiters:

- `first`: first content line.
- `middle`: continuation lines.
- `last`: last content line.
- `single`: content that occupies just one line.

`ReflowWidth` makes the three behaviors explicit:

- `Finite(usize)`: wrap at the given width. Zero is a literal zero-width budget; indivisible content such as a word can still exceed it.
- `Infinite`: join existing soft breaks without inserting new ones. An infinite `middle` ignores the other budgets.
- `Disabled`: skip reflow, preserving existing paragraph soft breaks while still applying other transformations. Setting any of the four budgets to `Disabled` disables reflow for the entire document. A soft break is joined only where the rendered line would start with block syntax, as a lazy continuation line can once it gains its container's prefix, or where a paragraph's first line would become an HTML block or a link reference definition.

`Options::default()` uses `Infinite` for all budgets, leaves linkification disabled, and recognizes frontmatter. Explicit hard breaks remain intact in every mode.

Wrapping breaks lines only at existing whitespace and never starts a line with Markdown block syntax, such as a list marker, block quote, heading, code fence, HTML block, link reference definition, setext underline, thematic break, or table delimiter row. It also never ends a line with a backslash, which would become a hard break, and never breaks inside a shortcut or collapsed reference, whose text is its label. When no acceptable break fits the budget, the line overflows instead. A line that cannot move, such as one after a hard break, has its leading block syntax escaped instead. Widths count the rendered Markdown, including link destinations and code span backticks. Line breaks are chosen as if words were separated by single spaces, and a wrapped line keeps wider spacing, such as two spaces after a sentence, only if it still fits; this keeps the output a fixed point.

The output parses to the same Markdown as the input, apart from where soft breaks fall. Text keeps its source spelling, including backslash escapes and character references, and so do emphasis delimiters (`*` or `_`), code spans, images, autolinks, and reference labels. Other syntax is normalized: bullets become `*`, ordered lists use `.`, setext headings without hard breaks become ATX headings, code blocks are fenced, link destinations and titles are re-encoded, and tables are padded. Adjacent lists alternate markers (`-`, `)`) so they stay separate, loose lists and items keep their blank lines, a block quote in a tight item ends with an empty quote line where the next line would otherwise continue its last paragraph, and definitions and HTML blocks keep the blank lines that separate them from paragraphs. Reflowing the output again leaves it unchanged.

YAML (`---`) and TOML (`+++`) frontmatter at the start of the input is recognized when a matching closing fence is present. The frontmatter block is copied from the source, preserving indentation, comments, blank lines, and multiline values without reflow or linkification. The renderer adds a final newline if needed and normalizes spacing between Markdown blocks. Set `frontmatter: false` for Markdown fragments, such as documentation comments, where leading `---` or `+++` lines are a thematic break, a setext underline, or paragraph text.

For example, linkify without reflowing:

```rust
let output = reflow(input, Options {
    widths: EdgeWidths {
        middle: ReflowWidth::Disabled,
        ..EdgeWidths::default()
    },
    linkify_urls: true,
    ..Options::default()
})?;
```

`linkify_urls: true` converts bare URL, `www.`, and email autolink literals into Markdown links. Text already inside a link is left alone, and a URL stays bare where brackets would change its neighbors, such as after `!` or next to an emphasis delimiter. Table formatting and Markdown syntax normalization also still apply when reflow is disabled; this mode does not preserve source formatting byte for byte. The return type is `Result<String, String>`.

Version 0.5 replaces the `Option<usize>` fields in `EdgeWidths` with `ReflowWidth`. Replace `Some(width)` with `ReflowWidth::Finite(width)` and `None` with `ReflowWidth::Infinite`; use `ReflowWidth::Disabled` to skip reflow. The CLI now also defaults to `Infinite` when width is omitted; zero and negative widths still select `Infinite`.

Version 0.8 adds the `frontmatter` field to `Options`. A struct literal that names every field needs `frontmatter: true` to keep the earlier behavior, or `..Options::default()` to fill it in; set `frontmatter: false` for Markdown fragments such as documentation comments.

## CLI

```sh
cargo install --path . --locked
markdown-reflow [--width WIDTH]                              # stdin to stdout
markdown-reflow [--width WIDTH] [--from LINE] [--to LINE] FILE... # edit files in place
markdown-reflow --width 80 --from 2 --to -1 notes/*.md
```

Width defaults to `Infinite`, matching the Rust API: existing soft breaks are joined without inserting new ones. Positive widths enable wrapping; zero and negative widths also select `Infinite`. `--from` defaults to 1 (the first line), and `--to` defaults to -1 (the last line). Bounds are one-indexed and inclusive; negative indices count backward from each file's end. Either bound may be supplied independently, and the same range is applied to every file. End bounds beyond a file's length are clamped to its last line. Range flags require at least one file; omitting all file paths reads stdin and writes stdout.

The shell expands unquoted globs such as `notes/*.md` into multiple file paths. Numeric filenames are ordinary paths; use `--` before paths beginning with a dash. Files are processed in argument order. If a file fails, the CLI reports the error on stderr, continues with the remaining files, and exits nonzero. Earlier successful edits are retained.

Version 0.6 replaces positional widths and line bounds with `--width`, `--from`, and `--to`; for example, `markdown-reflow 80 notes.md 2 -1` becomes `markdown-reflow --width 80 --from 2 --to -1 notes.md`. The library API is unchanged.

Fish completions are provided in `completions/markdown-reflow.fish`; copy it into `~/.config/fish/completions/` to enable them.

## Development

```sh
cargo test --locked
cargo fmt --check
```
