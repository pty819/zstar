use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;

mod cli;
mod commands;
mod utils;

use cli::{Cli, Commands};

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Pack {
            input,
            output,
            level,
            threads,
            no_long,
            ignore_failed_read,
        } => {
            let output_path = match output {
                Some(p) => p,
                None => {
                    let file_stem = input
                        .file_name()
                        .context("Invalid input path")?
                        .to_string_lossy();
                    PathBuf::from(format!("{}.tar.zst", file_stem))
                }
            };

            let threads_count = threads.unwrap_or_else(|| num_cpus::get() as u32);
            let long_distance = !no_long;

            commands::pack::execute(
                &input,
                &output_path,
                commands::pack::PackOptions {
                    level,
                    threads: threads_count,
                    long_distance,
                    ignore_errors: ignore_failed_read,
                },
            )?;
        }
        Commands::Unpack {
            input,
            output,
            threads,
        } => {
            let output_path = output.unwrap_or_else(|| PathBuf::from("."));
            let threads_count = threads.unwrap_or_else(|| num_cpus::get() as u32);
            commands::unpack::execute(&input, &output_path, threads_count)?;
            println!("Successfully unpacked {:?} to {:?}", input, output_path);
        }
    }

    Ok(())
}
