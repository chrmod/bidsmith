use std::fs::File;
use std::io::{self, Write};
use std::process::ExitCode;

use crate::schema::dump_schema;

pub fn run(output: Option<&str>) -> ExitCode {
    let doc = dump_schema();
    let json = match serde_json::to_string_pretty(&doc) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to serialize schema: {e}");
            return ExitCode::from(1);
        }
    };

    match output {
        None => {
            let stdout = io::stdout();
            let mut handle = stdout.lock();
            if let Err(e) = handle.write_all(json.as_bytes()).and_then(|_| handle.write_all(b"\n"))
            {
                eprintln!("failed to write schema: {e}");
                return ExitCode::from(1);
            }
        }
        Some(path) => {
            let result = File::create(path).and_then(|mut f| {
                f.write_all(json.as_bytes())?;
                f.write_all(b"\n")
            });
            if let Err(e) = result {
                eprintln!("failed to write {path}: {e}");
                return ExitCode::from(1);
            }
        }
    }

    ExitCode::SUCCESS
}
