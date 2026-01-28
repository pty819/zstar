use anyhow::{Context, Result};
use std::fs::File;
use std::path::Path;

pub fn execute(input: &Path, output: &Path) -> Result<()> {
    let file = File::open(input).context("Failed to open input file")?;
    let decoder = zstd::Decoder::new(file)?;
    let mut archive = tar::Archive::new(decoder);

    archive.unpack(output).context("Failed to unpack archive")?;
    Ok(())
}
