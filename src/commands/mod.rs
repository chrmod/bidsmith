pub mod adapt;
pub mod apply;
pub mod export;
pub mod fmt;
pub mod plan;
pub mod query;
pub mod validate;

use std::process::ExitCode;

pub fn stub(name: &str, message: &str) -> ExitCode {
    println!("{name}: not yet implemented.");
    println!("{message}");
    ExitCode::from(1)
}
