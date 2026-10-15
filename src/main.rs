use markdown_reflow::{EdgeWidths, Options, ReflowWidth, reflow};

use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();

    // Determine mode and parse arguments.
    match parse_args(&args) {
        Ok(Mode::Pipe { width }) => run_pipe(options_for_width(width)),
        Ok(Mode::Replace {
            width,
            path,
            start,
            end,
        }) => run_replace(options_for_width(width), path, start, end),
        Err(e) => {
            eprintln!("error: {e}");
            eprintln!();
            eprintln!("usage:");
            eprintln!("  markdown-reflow [width]                      — reflow stdin → stdout");
            eprintln!(
                "  markdown-reflow [width] <file> [start] [end] — reflow line range in-place"
            );
            eprintln!();
            eprintln!("  width defaults to infinite (join soft breaks without wrapping).");
            eprintln!(
                "  positive values wrap at that width; zero or negative values use infinite."
            );
            eprintln!("  line numbers are 1-indexed and inclusive; negative values count from");
            eprintln!("  the end of the file (-1 = last line). start defaults to 1 and end to");
            eprintln!("  the last line, so `<file>` alone reflows the whole file.");
            process::exit(1);
        }
    }
}

/// Translate the CLI's uniform width into the library configuration.
fn options_for_width(width: ReflowWidth) -> Options {
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

enum Mode {
    /// Read from stdin, write to stdout.
    Pipe { width: ReflowWidth },
    /// Edit a file in-place: reflow lines `start..=end` (1-indexed, inclusive).
    /// Missing bounds default to the whole file; negative indices count from
    /// the end (-1 = last line).
    Replace {
        width: ReflowWidth,
        path: PathBuf,
        start: Option<isize>,
        end: Option<isize>,
    },
}

fn parse_args(args: &[String]) -> Result<Mode, String> {
    let args = &args[1..]; // skip program name

    if args.is_empty() {
        return Ok(Mode::Pipe {
            width: ReflowWidth::default(),
        });
    }

    // First positional arg may be a width (integer) or a file path.
    let (width, rest) = if let Ok(w) = args[0].parse::<usize>() {
        (
            if w == 0 {
                ReflowWidth::Infinite
            } else {
                ReflowWidth::Finite(w)
            },
            &args[1..],
        )
    } else if args[0]
        .strip_prefix('-')
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
    {
        // Only the sign matters for negative widths, so avoid integer range limits.
        (ReflowWidth::Infinite, &args[1..])
    } else {
        (ReflowWidth::default(), args)
    };

    if rest.is_empty() {
        return Ok(Mode::Pipe { width });
    }

    // A file path plus up to two line numbers is replace mode; omitted line
    // numbers default to the first and last line respectively.
    let line_args = &rest[1..];
    let (start, end) = match line_args.len() {
        0 => (None, None),
        1 => (Some(parse_line_arg(&line_args[0], "start")?), None),
        2 => (
            Some(parse_line_arg(&line_args[0], "start")?),
            Some(parse_line_arg(&line_args[1], "end")?),
        ),
        _ => {
            return Err(format!(
                "unexpected arguments: {}",
                rest.iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            ));
        }
    };

    Ok(Mode::Replace {
        width,
        path: PathBuf::from(&rest[0]),
        start,
        end,
    })
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

fn run_replace(options: Options, path: PathBuf, start: Option<isize>, end: Option<isize>) {
    // Read the file.
    let original = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error reading {}: {e}", path.display());
            process::exit(1);
        }
    };

    // Extract the line range (1-indexed, inclusive).
    let lines: Vec<&str> = original.lines().collect();
    let total_lines = lines.len();
    if total_lines == 0 {
        return; // no lines to reflow
    }

    // Missing bounds default to the whole file: line 1 through the last line.
    let start = match resolve_line("start", start.unwrap_or(1), total_lines) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("error: {e}");
            process::exit(1);
        }
    };
    let end = match resolve_line("end", end.unwrap_or(-1), total_lines) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("error: {e}");
            process::exit(1);
        }
    };

    if start > end {
        eprintln!("error: invalid line range: start={start}, end={end} (must be 1 ≤ start ≤ end)");
        process::exit(1);
    }
    if start > total_lines {
        eprintln!("error: start line {start} exceeds file length ({total_lines} lines)");
        process::exit(1);
    }
    let actual_end = end.min(total_lines);

    // Join the selected lines into a markdown string.
    let input_txt = lines[start - 1..actual_end].join("\n");

    // Reflow.
    let output_txt = match reflow(&input_txt, options) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            process::exit(1);
        }
    };

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

    // Write back.
    if let Err(e) = fs::write(&path, &result) {
        eprintln!("error writing {}: {e}", path.display());
        process::exit(1);
    }
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

    fn width_cases() -> Vec<(Option<String>, ReflowWidth)> {
        vec![
            (None, ReflowWidth::Infinite),
            (Some("80".into()), ReflowWidth::Finite(80)),
            (Some("1".into()), ReflowWidth::Finite(1)),
            (Some("+80".into()), ReflowWidth::Finite(80)),
            (
                Some(usize::MAX.to_string()),
                ReflowWidth::Finite(usize::MAX),
            ),
            (Some("0".into()), ReflowWidth::Infinite),
            (Some("+0".into()), ReflowWidth::Infinite),
            (Some("-0".into()), ReflowWidth::Infinite),
            (Some("-1".into()), ReflowWidth::Infinite),
            (Some("-100".into()), ReflowWidth::Infinite),
            (
                Some("-9999999999999999999999999999999999999999".into()),
                ReflowWidth::Infinite,
            ),
        ]
    }

    #[test]
    fn pipe_widths() {
        for (arg, expected) in width_cases() {
            let mut args = vec!["markdown-reflow".into()];
            args.extend(arg);
            match parse_args(&args).unwrap() {
                Mode::Pipe { width } => assert_eq!(width, expected, "args: {args:?}"),
                Mode::Replace { .. } => panic!("expected pipe mode for {args:?}"),
            }
        }
    }

    #[test]
    fn default_cli_width_matches_library_and_joins_long_paragraphs() {
        let input = format!("{}\n{}\n", "word ".repeat(30).trim_end(), "more words");
        let expected = format!("{} more words\n", "word ".repeat(30).trim_end());
        for args in [
            vec!["markdown-reflow".to_string()],
            vec!["markdown-reflow".to_string(), "document.md".to_string()],
        ] {
            let width = match parse_args(&args).unwrap() {
                Mode::Pipe { width } | Mode::Replace { width, .. } => width,
            };
            let options = options_for_width(width);
            assert_eq!(options, Options::default());
            assert_eq!(reflow(&input, options).unwrap(), expected);
        }
    }

    #[test]
    fn replace_widths() {
        for (arg, expected) in width_cases() {
            let mut args = vec!["markdown-reflow".into()];
            args.extend(arg);
            args.extend(["document.md".into(), "2".into(), "5".into()]);
            match parse_args(&args).unwrap() {
                Mode::Replace {
                    width,
                    path,
                    start,
                    end,
                } => {
                    assert_eq!(width, expected, "args: {args:?}");
                    assert_eq!(path, PathBuf::from("document.md"));
                    assert_eq!((start, end), (Some(2), Some(5)));
                }
                Mode::Pipe { .. } => panic!("expected replace mode for {args:?}"),
            }
        }
    }

    #[test]
    fn non_numeric_negative_prefix_is_a_path() {
        for path in ["-", "-notes.md", "-1.md", "--1", "-+1"] {
            let args = ["markdown-reflow", path, "1", "2"].map(String::from);
            match parse_args(&args).unwrap() {
                Mode::Replace {
                    width,
                    path: actual,
                    ..
                } => {
                    assert_eq!(width, ReflowWidth::Infinite);
                    assert_eq!(actual, PathBuf::from(path));
                }
                Mode::Pipe { .. } => panic!("expected replace mode for {args:?}"),
            }
        }
    }

    #[test]
    fn replace_without_line_numbers_defaults_to_whole_file() {
        let args = ["markdown-reflow", "document.md"].map(String::from);
        match parse_args(&args).unwrap() {
            Mode::Replace {
                width, start, end, ..
            } => {
                assert_eq!(width, ReflowWidth::Infinite);
                assert_eq!((start, end), (None, None));
            }
            Mode::Pipe { .. } => panic!("expected replace mode for {args:?}"),
        }
    }

    #[test]
    fn replace_ranges_with_negative_indices() {
        for (line_args, expected) in [
            (vec!["3"], (Some(3), None)),
            (vec!["-3"], (Some(-3), None)),
            (vec!["1", "-1"], (Some(1), Some(-1))),
            (vec!["-2", "-1"], (Some(-2), Some(-1))),
            (vec!["-1", "5"], (Some(-1), Some(5))),
        ] {
            let mut args = vec!["markdown-reflow".to_string(), "document.md".to_string()];
            args.extend(line_args.iter().map(|s| s.to_string()));
            match parse_args(&args).unwrap() {
                Mode::Replace { start, end, .. } => {
                    assert_eq!((start, end), expected, "args: {args:?}")
                }
                Mode::Pipe { .. } => panic!("expected replace mode for {args:?}"),
            }
        }
    }

    #[test]
    fn zero_line_numbers_are_rejected() {
        for line_args in [
            vec!["0"],
            vec!["0", "5"],
            vec!["5", "0"],
            vec!["-0", "1"],
            vec!["+0", "1"],
        ] {
            let mut args = vec!["markdown-reflow".to_string(), "document.md".to_string()];
            args.extend(line_args.iter().map(|s| s.to_string()));
            assert!(parse_args(&args).is_err(), "expected error for {args:?}");
        }
    }

    #[test]
    fn too_many_line_numbers_are_rejected() {
        let args = ["markdown-reflow", "document.md", "1", "2", "3"].map(String::from);
        assert!(parse_args(&args).is_err());
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

        run_replace(
            options_for_width(ReflowWidth::Finite(5)),
            path.clone(),
            None,
            None,
        );

        // Defaulting to 1..=max must be equivalent to reflowing every line.
        let body = original.strip_suffix('\n').unwrap();
        let expected = reflow(body, options_for_width(ReflowWidth::Finite(5))).unwrap();
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

        run_replace(
            options_for_width(ReflowWidth::Finite(3)),
            path.clone(),
            Some(-1),
            None,
        );

        assert_eq!(fs::read_to_string(&path).unwrap(), "aaa bbb\n\nccc\nddd\n");
        fs::remove_dir_all(&dir).unwrap();
    }
}
