use client910::draw_trace::Trace;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: drawdiff <recorded.bin> <rust.bin>");
        return ExitCode::from(2);
    }
    let load = |path: &str| -> Result<Trace, String> {
        Trace::decode(&std::fs::read(path).map_err(|e| format!("{path}: {e}"))?)
            .map_err(|e| format!("{path}: {e}"))
    };
    match (load(&args[1]), load(&args[2])) {
        (Ok(a), Ok(b)) => match a.compare(&b) {
            Ok(()) => {
                println!("IDENTICAL ({} sections)", a.0.len());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{e}");
                ExitCode::from(1)
            }
        },
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("{e}");
            ExitCode::from(2)
        }
    }
}
