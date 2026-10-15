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
- `Disabled`: skip reflow, preserving existing paragraph soft breaks while still applying other transformations. Setting any of the four budgets to `Disabled` disables reflow for the entire document.

`Options::default()` uses `Infinite` for all budgets and leaves linkification disabled. Explicit hard breaks remain intact in every mode.

For example, linkify without reflowing:

```rust
let output = reflow(input, Options {
    widths: EdgeWidths {
        middle: ReflowWidth::Disabled,
        ..EdgeWidths::default()
    },
    linkify_urls: true,
})?;
```

`linkify_urls: true` converts bare URL, `www.`, and email autolink literals into Markdown links. Table formatting and Markdown syntax normalization also still apply when reflow is disabled; this mode does not preserve source formatting byte for byte. The return type is `Result<String, String>`.

Version 0.5 replaces the `Option<usize>` fields in `EdgeWidths` with `ReflowWidth`. Replace `Some(width)` with `ReflowWidth::Finite(width)` and `None` with `ReflowWidth::Infinite`; use `ReflowWidth::Disabled` to skip reflow. The CLI now also defaults to `Infinite` when width is omitted; zero and negative widths still select `Infinite`.

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
