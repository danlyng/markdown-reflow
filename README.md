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
point. Move width arguments into `Options.widths`; the CLI syntax is unchanged.

## CLI

```sh
cargo install --path . --locked
markdown-reflow [width]                         # stdin to stdout
markdown-reflow [width] <file> [start] [end]     # replace a line range in place
```

Width defaults to 100. Zero and negative widths disable wrapping. Line numbers
are one-indexed and inclusive; negative indices count from the end (`-1` is the
last line). Omitted bounds select the entire file.

## Development

```sh
cargo test --locked
cargo fmt --check
```
