mod scenes;
mod session;
mod svg;

use std::{env, path::PathBuf, process::ExitCode};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let [binary, demo, out] = args.as_slice() else {
        eprintln!("usage: capture <ccview-rs binary> <demo dir> <output dir>");
        return ExitCode::FAILURE;
    };
    match scenes::run(&PathBuf::from(binary), &PathBuf::from(demo), &PathBuf::from(out)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}
