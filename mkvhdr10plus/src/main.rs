//! `mkvhdr10plus` — generate HDR10+ dynamic metadata (Profile B) from an HDR10
//! source and inject it into the original HEVC stream without re-encoding.
//!
//! See `docs/HDR10plus_writer_spec.md` for the measurement and JSON contract.

use clap::Parser;
use colored::Colorize;

mod cli;
mod external;
mod json;
mod measure;
mod pipeline;
mod scene;

use cli::Cli;

fn main() {
    let cli = Cli::parse();

    if let Err(e) = external::check_dependencies(cli.json_only) {
        eprintln!("{}", format!("Dependency check failed: {e}").red());
        std::process::exit(1);
    }

    if let Err(e) = pipeline::run(&cli) {
        eprintln!("{}", format!("Error: {e:#}").red());
        std::process::exit(1);
    }
}
