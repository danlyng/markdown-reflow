use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

fn run_in(args: &[&str], input: &str, dir: Option<&Path>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_markdown-reflow"));
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn run(args: &[&str], input: &str) -> Output {
    run_in(args, input, None)
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "markdown-reflow-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn write(&self, name: &str, content: &str) {
        fs::write(self.0.join(name), content).unwrap();
    }
    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.0.join(name)).unwrap()
    }
    fn run(&self, args: &[&str]) -> Output {
        run_in(args, "", Some(&self.0))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn help_and_version_exit_successfully_on_stdout() {
    for args in [vec!["--help"], vec!["-h"], vec!["--width", "80", "--help"]] {
        let output = run(&args, "");
        assert!(output.status.success(), "{args:?}: {output:?}");
        assert!(output.stderr.is_empty());
        let help = String::from_utf8(output.stdout).unwrap();
        for text in [
            "[FILE]...",
            "--width",
            "--from",
            "--to",
            "default: infinite",
            "default: 1",
            "default: -1",
            "Examples:",
        ] {
            assert!(help.contains(text), "missing {text}: {help}");
        }
    }
    for arg in ["--version", "-V"] {
        let output = run(&[arg], "");
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!("markdown-reflow {}\n", env!("CARGO_PKG_VERSION"))
        );
    }
}

#[test]
fn width_flags_reflow_stdin() {
    for (args, expected) in [
        (vec![], "aaa bbb ccc\n"),
        (vec!["--width", "3"], "aaa\nbbb\nccc\n"),
        (vec!["--width", "+3"], "aaa\nbbb\nccc\n"),
        (vec!["--width=0"], "aaa bbb ccc\n"),
        (
            vec!["--width", "-999999999999999999999999999999"],
            "aaa bbb ccc\n",
        ),
    ] {
        let output = run(&args, "aaa bbb\nccc\n");
        assert!(output.status.success(), "{args:?}: {output:?}");
        assert!(output.stderr.is_empty());
        assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
    }
    let input = format!("{}\nmore words\n", "word ".repeat(30).trim_end());
    let expected = format!("{} more words\n", "word ".repeat(30).trim_end());
    let output = run(&[], &input);
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
}

#[test]
fn clap_reports_invalid_flags_and_ranges_before_editing() {
    let dir = Fixture::new();
    dir.write("document.md", "aaa bbb\n");
    for args in [
        vec!["document.md", "--from", "0"],
        vec!["--width", "80", "document.md", "--from", "one"],
        vec!["document.md", "--to", "0"],
        vec!["document.md", "--width", "wide"],
        vec!["document.md", "--to", "999999999999999999999999999"],
        vec!["document.md", "--unknown"],
        vec!["--from", "1"],
        vec!["--to", "-1"],
    ] {
        let output = dir.run(&args);
        assert!(!output.status.success(), "{args:?}");
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains("error:"), "{args:?}: {error}");
        assert!(!error.contains("error reading"), "{args:?}: {error}");
        assert!(!error.contains('\u{1b}'));
        assert_eq!(dir.read("document.md"), "aaa bbb\n");
    }
}

#[test]
fn multiple_files_default_to_their_entire_contents() {
    let dir = Fixture::new();
    dir.write("one.md", "aaa bbb\n\nccc ddd\n");
    dir.write("two.md", "eee fff\n");
    dir.write("empty.md", "");
    let output = dir.run(&["one.md", "--width", "3", "two.md", "empty.md"]);
    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    assert_eq!(dir.read("one.md"), "aaa\nbbb\n\nccc\nddd\n");
    assert_eq!(dir.read("two.md"), "eee\nfff\n");
    assert_eq!(dir.read("empty.md"), "");
}

#[test]
fn omitted_width_joins_long_paragraphs_in_files() {
    let dir = Fixture::new();
    let input = format!("{}\nmore words\n", "word ".repeat(30).trim_end());
    let expected = format!("{} more words\n", "word ".repeat(30).trim_end());
    for name in ["one.md", "two.md"] {
        dir.write(name, &input);
    }
    let output = dir.run(&["one.md", "two.md"]);
    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    for name in ["one.md", "two.md"] {
        assert_eq!(dir.read(name), expected);
    }
}

#[test]
fn negative_ranges_resolve_independently_for_each_file() {
    let dir = Fixture::new();
    dir.write("one.md", "keep this\naaa bbb\nkeep that\n");
    dir.write("two.md", "keep this\nkeep too\nccc ddd\nkeep that");
    let output = dir.run(&[
        "--width", "3", "--from", "-2", "--to", "-2", "one.md", "two.md",
    ]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(dir.read("one.md"), "keep this\naaa\nbbb\nkeep that\n");
    assert_eq!(
        dir.read("two.md"),
        "keep this\nkeep too\nccc\nddd\nkeep that"
    );
}

#[test]
fn each_bound_can_be_omitted() {
    let dir = Fixture::new();
    for (flags, expected) in [
        (vec!["--from", "2"], "keep this\naaa\nbbb\nccc\nddd\n"),
        (vec!["--to", "2"], "keep\nthis\naaa\nbbb\nccc ddd\n"),
    ] {
        dir.write("doc.md", "keep this\naaa bbb\nccc ddd\n");
        let mut args = vec!["--width", "3", "doc.md"];
        args.extend(flags);
        let output = dir.run(&args);
        assert!(output.status.success(), "{output:?}");
        assert_eq!(dir.read("doc.md"), expected);
    }
}

#[test]
fn batch_errors_name_the_file_and_continue_processing() {
    let dir = Fixture::new();
    dir.write("short.md", "aaa bbb\n");
    dir.write("good.md", "keep this\nccc ddd\n");
    let output = dir.run(&[
        "--width",
        "3",
        "--from",
        "2",
        "missing.md",
        "short.md",
        "good.md",
    ]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("missing.md"));
    assert!(error.contains("short.md"));
    assert_eq!(dir.read("short.md"), "aaa bbb\n");
    assert_eq!(dir.read("good.md"), "keep this\nccc\nddd\n");
}

#[test]
fn numeric_and_dash_prefixed_paths_can_be_edited_together() {
    let dir = Fixture::new();
    for name in ["80", "-notes.md", "--from", "has spaces.md"] {
        dir.write(name, "aaa bbb\n");
    }
    let output = dir.run(&[
        "--width",
        "3",
        "80",
        "--",
        "-notes.md",
        "--from",
        "has spaces.md",
    ]);
    assert!(output.status.success(), "{output:?}");
    for name in ["80", "-notes.md", "--from", "has spaces.md"] {
        assert_eq!(dir.read(name), "aaa\nbbb\n");
    }
}
