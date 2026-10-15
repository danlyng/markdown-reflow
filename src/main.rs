use clap::{ColorChoice, Parser};
use markdown_reflow::{EdgeWidths, Options, reflow};

use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;
use std::process;

#[derive(Debug, Parser)]
#[command(version, about = "Reflow Markdown while preserving block structure and hard breaks",
    color = ColorChoice::Never,
    after_help = "Examples:\n  markdown-reflow --width 80 < input.md > output.md\n  markdown-reflow --width 0 *.md\n  markdown-reflow --width 80 --from 2 --to -1 notes/*.md\n\nLine numbers are 1-indexed and inclusive. Negative values count from the end\n(-1 = last line). The same range is applied independently to each file.")]
struct Cli {
    /// Wrap width; zero or negative values disable wrapping
    #[arg(long, default_value = "100", value_parser = parse_width, allow_hyphen_values = true)]
    width: Width,

    /// First line, inclusive (default: 1); requires at least one file
    #[arg(long, value_name = "LINE", requires = "files", allow_negative_numbers = true,
        value_parser = |s: &str| parse_line_arg(s, "from"))]
    from: Option<isize>,

    /// Last line, inclusive (default: -1, the last line); requires a file
    #[arg(long, value_name = "LINE", requires = "files", allow_negative_numbers = true,
        value_parser = |s: &str| parse_line_arg(s, "to"))]
    to: Option<isize>,

    /// Files to edit in place; omit to read stdin and write stdout
    #[arg(value_name = "FILE", value_hint = clap::ValueHint::FilePath)]
    files: Vec<PathBuf>,
}

fn main() {
    let cli = Cli::parse();
    let options = options_for_width(cli.width.0);
    if cli.files.is_empty() {
        run_pipe(options);
        return;
    }

    let mut failed = false;
    for path in &cli.files {
        if let Err(error) = run_replace(options, path, cli.from, cli.to) {
            eprintln!("error: {}: {error}", path.display());
            failed = true;
        }
    }
    if failed {
        process::exit(1);
    }
}

/// Translate the CLI's uniform width into the library configuration.
fn options_for_width(width: Option<usize>) -> Options {
    Options {
        widths: EdgeWidths {
            first: width,
            middle: width,
            last: width,
            single: width,
        },
        ..Options::default()
    }
}

#[derive(Clone, Debug)]
struct Width(Option<usize>);

fn parse_width(arg: &str) -> Result<Width, String> {
    if let Ok(width) = arg.parse::<usize>() {
        Ok(Width((width > 0).then_some(width)))
    } else if arg
        .strip_prefix('-')
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
    {
        // Only the sign matters for negative widths, avoiding integer limits.
        Ok(Width(None))
    } else {
        Err("width must be an integer; try --width 80 or --width 0".into())
    }
}

/// Parse a line-number argument: positive values are 1-indexed from the start
/// of the file, negative values count from the end, and zero is invalid.
fn parse_line_arg(arg: &str, which: &str) -> Result<isize, String> {
    let n: isize = arg
        .parse()
        .map_err(|_| format!("invalid {which} line: {arg}"))?;
    if n == 0 {
        return Err(format!("invalid {which} line: {arg} (must be nonzero)"));
    }
    Ok(n)
}

// ---------------------------------------------------------------------------
// Pipe mode: stdin → stdout
// ---------------------------------------------------------------------------

fn run_pipe(options: Options) {
    let mut input = String::new();
    if let Err(e) = io::stdin().read_to_string(&mut input) {
        eprintln!("error reading stdin: {e}");
        process::exit(1);
    }

    match reflow(&input, options) {
        Ok(output) => print!("{output}"),
        Err(e) => {
            eprintln!("error: {e}");
            process::exit(1);
        }
    }
}

// ---------------------------------------------------------------------------
// Replace mode: in-place line-range editing
// ---------------------------------------------------------------------------

fn run_replace(
    options: Options,
    path: &std::path::Path,
    start: Option<isize>,
    end: Option<isize>,
) -> Result<(), String> {
    // Read the file.
    let original = fs::read_to_string(path).map_err(|e| format!("cannot read file: {e}"))?;

    // Extract the line range (1-indexed, inclusive).
    let lines: Vec<&str> = original.lines().collect();
    let total_lines = lines.len();
    if total_lines == 0 {
        return Ok(()); // no lines to reflow
    }

    // Missing bounds default to the whole file: line 1 through the last line.
    let start = resolve_line("from", start.unwrap_or(1), total_lines)?;
    let end = resolve_line("to", end.unwrap_or(-1), total_lines)?;
    if start > end {
        return Err(format!(
            "invalid line range: from={start}, to={end}; use --from <= --to"
        ));
    }
    if start > total_lines {
        return Err(format!(
            "from line {start} exceeds file length ({total_lines} lines)"
        ));
    }
    let actual_end = end.min(total_lines);

    // Join the selected lines into a markdown string.
    let input_txt = lines[start - 1..actual_end].join("\n");

    // Reflow.
    let output_txt = reflow(&input_txt, options)?;

    // Rebuild the file: lines before the range + reflowed output + lines after.
    let before: Vec<&str> = lines[..start - 1].to_vec();
    let after: Vec<&str> = if actual_end < total_lines {
        lines[actual_end..].to_vec()
    } else {
        vec![]
    };

    // Strip trailing newline from reflow output (the renderer always adds one).
    let output_trimmed = output_txt.strip_suffix('\n').unwrap_or(&output_txt);

    let before_str = before.join("\n");
    let after_str = after.join("\n");

    let mut parts: Vec<&str> = Vec::new();
    if !before.is_empty() {
        parts.push(&before_str);
    }
    parts.push(output_trimmed);
    if !after.is_empty() {
        parts.push(&after_str);
    }

    let mut result = parts.join("\n");
    if original.ends_with('\n') {
        result.push('\n');
    }

    fs::write(path, &result).map_err(|e| format!("cannot write file: {e}"))
}

/// Resolve a parsed line number against the file length: positive values are
/// 1-indexed from the start, negative values count from the end (-1 = last
/// line). Fails if the index points before line 1; callers clamp an end that
/// runs past the last line.
fn resolve_line(which: &str, n: isize, total_lines: usize) -> Result<usize, String> {
    let resolved = if n > 0 {
        n
    } else {
        total_lines as isize + n + 1
    };
    if resolved < 1 {
        return Err(format!(
            "{which} line {n} is out of range (file has {total_lines} lines)"
        ));
    }
    Ok(resolved as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn width_cases() -> Vec<(Option<String>, Option<usize>)> {
        vec![
            (None, Some(100)),
            (Some("80".into()), Some(80)),
            (Some("1".into()), Some(1)),
            (Some("+80".into()), Some(80)),
            (Some(usize::MAX.to_string()), Some(usize::MAX)),
            (Some("0".into()), None),
            (Some("+0".into()), None),
            (Some("-0".into()), None),
            (Some("-1".into()), None),
            (Some("-100".into()), None),
            (
                Some("-9999999999999999999999999999999999999999".into()),
                None,
            ),
        ]
    }

    #[test]
    fn pipe_widths() {
        for (arg, expected) in width_cases() {
            let mut args = vec!["markdown-reflow".to_string()];
            if let Some(arg) = arg {
                args.extend(["--width".into(), arg]);
            }
            let cli = Cli::try_parse_from(&args).unwrap();
            assert_eq!(cli.width.0, expected, "args: {args:?}");
            assert!(cli.files.is_empty());
            assert_eq!((cli.from, cli.to), (None, None));
        }
    }

    #[test]
    fn multiple_files_and_flags_in_any_order() {
        let cli = Cli::try_parse_from([
            "markdown-reflow",
            "one.md",
            "--to",
            "-1",
            "--width",
            "80",
            "two.md",
            "--from",
            "-3",
        ])
        .unwrap();
        assert_eq!(cli.width.0, Some(80));
        assert_eq!((cli.from, cli.to), (Some(-3), Some(-1)));
        assert_eq!(
            cli.files,
            [PathBuf::from("one.md"), PathBuf::from("two.md")]
        );
    }

    #[test]
    fn numeric_and_dash_prefixed_filenames() {
        let cli =
            Cli::try_parse_from(["markdown-reflow", "--", "80", "-notes.md", "--from"]).unwrap();
        assert_eq!(cli.files, ["80", "-notes.md", "--from"].map(PathBuf::from));
        assert_eq!(cli.width.0, Some(100));
    }

    #[test]
    fn range_bounds_are_independent() {
        for (flags, expected) in [
            (vec![], (None, None)),
            (vec!["--from", "3"], (Some(3), None)),
            (vec!["--from", "-3"], (Some(-3), None)),
            (vec!["--to", "5"], (None, Some(5))),
            (vec!["--from", "1", "--to", "-1"], (Some(1), Some(-1))),
        ] {
            let mut args = vec!["markdown-reflow", "document.md"];
            args.extend(flags);
            let cli = Cli::try_parse_from(args).unwrap();
            assert_eq!((cli.from, cli.to), expected);
        }
    }

    #[test]
    fn invalid_flags_and_ranges_are_rejected() {
        for args in [
            vec!["--width", "wide"],
            vec!["--width"],
            vec!["--from", "0", "document.md"],
            vec!["--to", "-0", "document.md"],
            vec!["--from", "one", "document.md"],
            vec!["--to", "999999999999999999999999", "document.md"],
            vec!["--from", "1"],
            vec!["--to", "-1"],
            vec!["--unknown"],
        ] {
            assert!(
                Cli::try_parse_from(std::iter::once("markdown-reflow").chain(args.iter().copied()))
                    .is_err(),
                "args: {args:?}"
            );
        }
    }

    #[test]
    fn resolving_line_numbers() {
        // Positive values are 1-indexed from the start.
        assert_eq!(resolve_line("start", 1, 10).unwrap(), 1);
        assert_eq!(resolve_line("end", 7, 10).unwrap(), 7);
        // Negative values count from the end (-1 = last line).
        assert_eq!(resolve_line("end", -1, 10).unwrap(), 10);
        assert_eq!(resolve_line("start", -10, 10).unwrap(), 1);
        // Indices pointing before the first line are errors.
        assert!(resolve_line("start", -11, 10).is_err());
        assert!(resolve_line("end", -20, 10).is_err());
    }

    #[test]
    fn replace_whole_file_by_default() {
        let dir = env::temp_dir().join(format!("markdown-reflow-test-whole-{}", process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.md");
        let original = "one two three\n\nfour five\n";
        fs::write(&path, original).unwrap();

        run_replace(options_for_width(Some(5)), &path, None, None).unwrap();

        // Defaulting to 1..=max must be equivalent to reflowing every line.
        let body = original.strip_suffix('\n').unwrap();
        let expected = reflow(body, options_for_width(Some(5))).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), expected);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replace_negative_line_range() {
        let dir = env::temp_dir().join(format!("markdown-reflow-test-neg-{}", process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.md");
        // Three lines; -1 selects just the last one.
        fs::write(&path, "aaa bbb\n\nccc ddd\n").unwrap();

        run_replace(options_for_width(Some(3)), &path, Some(-1), None).unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "aaa bbb\n\nccc\nddd\n");
        fs::remove_dir_all(&dir).unwrap();
    }
}
