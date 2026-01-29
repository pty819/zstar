use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Compress a directory into a tar.zst archive
    Pack {
        /// Input directory to compress
        input: PathBuf,

        /// Output file path (optional, defaults to directory_name.tar.zst)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Compression level (default: 3)
        #[arg(short, long, default_value_t = 3)]
        level: i32,

        /// Number of threads (default: num_cpus)
        #[arg(short, long)]
        threads: Option<u32>,

        /// Disable long distance matching (enabled by default)
        #[arg(long)]
        no_long: bool,

        /// Ignore read errors (e.g., permission denied) instead of aborting
        #[arg(long)]
        ignore_failed_read: bool,
    },
    /// Decompress a tar.zst archive
    Unpack {
        /// Input tar.zst file
        input: PathBuf,
        /// Output directory (optional, defaults to current directory)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Number of threads (default: num_cpus)
        #[arg(short, long)]
        threads: Option<u32>,
    },
}
