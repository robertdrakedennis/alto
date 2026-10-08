use anyhow::Result;
use clap::Parser;
use native910::cli::{Cli, run};

fn main() -> Result<()> {
    run(&Cli::parse())
}
