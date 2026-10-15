# markdown-reflow

Reflow Markdown paragraphs while preserving block structure, inline formatting,
and explicit hard breaks. Available as a Rust library and a command-line tool.

## Library

The public API consists of `reflow`, `Options`, and `EdgeWidths`, exported from
the crate root:

```rust
use markdown_reflow::{reflow, EdgeWidths, Options};

let options = Options {
    widths: EdgeWidths {
        first: Some(80),
        middle: Some(80),
        last: Some(80),
        single: Some(80),
    },
    linkify_urls: false,
};
let output = reflow("Some Markdown to reflow.", options)?;
```

Set all four budgets alike for uniform wrapping. Set different budgets to reserve
space for external prefixes and suffixes, such as comment delimiters:

- `first`: first content line.
- `middle`: continuation lines.
- `last`: last content line.
- `single`: content that occupies just one line.

Each `None` leaves that width unconstrained. When `middle` is `None`, wrapping
is disabled entirely and the other budgets are ignored. `Options::default()`
joins existing soft breaks without inserting new ones and leaves linkification
disabled. Explicit hard breaks remain intact.

`linkify_urls: true` converts bare URL, `www.`, and email autolink literals into
Markdown links. The return type is `Result<String, String>`.

Version 0.4 replaces the width-taking reflow functions with this single entry
point. Move width arguments into `Options.widths`.

## CLI

```sh
cargo install --path . --locked
markdown-reflow [--width WIDTH]                              # stdin to stdout
markdown-reflow [--width WIDTH] [--from LINE] [--to LINE] FILE... # edit files in place
markdown-reflow --width 80 --from 2 --to -1 notes/*.md
```

Width defaults to 100. Zero and negative widths disable wrapping. `--from` defaults to 1 (the first line), and `--to` defaults to -1 (the last line). Bounds are one-indexed and inclusive; negative indices count backward from each file's end. Either bound may be supplied independently, and the same range is applied to every file. End bounds beyond a file's length are clamped to its last line. Range flags require at least one file; omitting all file paths reads stdin and writes stdout.

The shell expands unquoted globs such as `notes/*.md` into multiple file paths. Numeric filenames are ordinary paths; use `--` before paths beginning with a dash. Files are processed in argument order. If a file fails, the CLI reports the error on stderr, continues with the remaining files, and exits nonzero. Earlier successful edits are retained.

Version 0.6 replaces positional widths and line bounds with `--width`, `--from`, and `--to`; for example, `markdown-reflow 80 notes.md 2 -1` becomes `markdown-reflow --width 80 --from 2 --to -1 notes.md`. The library API is unchanged.

Fish completions are provided in `completions/markdown-reflow.fish`; copy it into `~/.config/fish/completions/` to enable them.

## Development

```sh
cargo test --locked
cargo fmt --check
```
