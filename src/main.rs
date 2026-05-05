mod reflow;

use std::env;
use std::io::{self, Read};
use std::process;

const DEFAULT_WIDTH: usize = 100;

fn main() {
    let args: Vec<String> = env::args().collect();
    let target_width: usize = args
        .get(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_WIDTH);

    let mut input = String::new();
    if let Err(e) = io::stdin().read_to_string(&mut input) {
        eprintln!("error reading stdin: {e}");
        process::exit(1);
    }

    match reflow::reflow(&input, target_width) {
        Ok(output) => print!("{output}"),
        Err(e) => {
            eprintln!("error: {e}");
            process::exit(1);
        }
    }
}
