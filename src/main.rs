mod reflow;

use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;
use std::process;

const DEFAULT_WIDTH: usize = 100;

fn main() {
    let args: Vec<String> = env::args().collect();

    // Determine mode and parse arguments.
    match parse_args(&args) {
        Ok(Mode::Pipe { width }) => run_pipe(width),
        Ok(Mode::Replace {
            width,
            path,
            start,
            end,
        }) => run_replace(width, path, start, end),
        Err(e) => {
            eprintln!("error: {e}");
            eprintln!();
            eprintln!("usage:");
            eprintln!("  markdown-reflow [width]                  — reflow stdin → stdout");
            eprintln!(
                "  markdown-reflow [width] <file> <start> <end>  — reflow line range in-place"
            );
            process::exit(1);
        }
    }
}

enum Mode {
    /// Read from stdin, write to stdout.
    Pipe { width: usize },
    /// Edit a file in-place: reflow lines `start..=end` (1-indexed, inclusive).
    Replace {
        width: usize,
        path: PathBuf,
        start: usize,
        end: usize,
    },
}

fn parse_args(args: &[String]) -> Result<Mode, String> {
    let args = &args[1..]; // skip program name

    if args.is_empty() {
        return Ok(Mode::Pipe {
            width: DEFAULT_WIDTH,
        });
    }

    // First positional arg may be a width (integer) or a file path.
    let (width, rest) = if let Ok(w) = args[0].parse::<usize>() {
        (w, &args[1..])
    } else {
        (DEFAULT_WIDTH, args)
    };

    if rest.is_empty() {
        return Ok(Mode::Pipe { width });
    }

    // If we have a file path and two line numbers, it's replace mode.
    if rest.len() >= 3 {
        let path = PathBuf::from(&rest[0]);
        let start: usize = rest[1]
            .parse()
            .map_err(|_| format!("invalid start line: {}", rest[1]))?;
        let end: usize = rest[2]
            .parse()
            .map_err(|_| format!("invalid end line: {}", rest[2]))?;

        if start < 1 || end < start {
            return Err(format!(
                "invalid line range: start={start}, end={end} (must be 1 ≤ start ≤ end)"
            ));
        }

        return Ok(Mode::Replace {
            width,
            path,
            start,
            end,
        });
    }

    Err(format!(
        "unexpected arguments: {}",
        rest.iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    ))
}

// ---------------------------------------------------------------------------
// Pipe mode: stdin → stdout
// ---------------------------------------------------------------------------

fn run_pipe(width: usize) {
    let mut input = String::new();
    if let Err(e) = io::stdin().read_to_string(&mut input) {
        eprintln!("error reading stdin: {e}");
        process::exit(1);
    }

    match reflow::reflow(&input, width) {
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

fn run_replace(width: usize, path: PathBuf, start: usize, end: usize) {
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

    if start > total_lines {
        eprintln!("error: start line {start} exceeds file length ({total_lines} lines)");
        process::exit(1);
    }
    let actual_end = end.min(total_lines);

    // Join the selected lines into a markdown string.
    let input_txt = lines[start - 1..actual_end].join("\n");

    // Reflow.
    let output_txt = match reflow::reflow(&input_txt, width) {
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
