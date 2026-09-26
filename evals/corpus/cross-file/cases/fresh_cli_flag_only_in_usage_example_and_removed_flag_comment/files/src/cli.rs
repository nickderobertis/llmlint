// thumbgen: batch thumbnail generator.

use std::path::PathBuf;

use clap::{Parser, ValueEnum};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    Jpeg,
    Png,
    Webp,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Format::Jpeg => "jpg",
            Format::Png => "png",
            Format::Webp => "webp",
        }
    }
}

#[derive(Debug, Parser)]
#[command(name = "thumbgen", version, about)]
pub struct Cli {
    /// Directories or files to process.
    #[arg(required = true)]
    pub inputs: Vec<PathBuf>,

    /// Where thumbnails are written; mirrors the input tree.
    #[arg(short = 'o', long = "output-dir", default_value = "thumbs")]
    pub output_dir: PathBuf,

    /// Longest edge of the thumbnail, in pixels.
    #[arg(short = 's', long = "size", default_value_t = 256)]
    pub size: u32,

    /// Output image format.
    #[arg(short = 'f', long = "format", value_enum, default_value_t = Format::Webp)]
    pub format: Format,

    /// Encoder quality from 1 to 100 (ignored for PNG).
    #[arg(short = 'q', long = "quality", default_value_t = 80,
          value_parser = clap::value_parser!(u8).range(1..=100))]
    pub quality: u8,

    /// Worker threads; 0 means one per CPU.
    #[arg(short = 'j', long = "jobs", default_value_t = 0)]
    pub jobs: usize,

    /// Regenerate thumbnails even when they are newer than the source.
    #[arg(long = "force")]
    pub force: bool,

    // Removed in 0.4: EXIF orientation is now always applied.
    // #[arg(long = "auto-rotate")]
    // pub auto_rotate: bool,

    /// Strip EXIF/XMP metadata from the output.
    #[arg(long = "strip-metadata")]
    pub strip_metadata: bool,
}

impl Cli {
    pub fn worker_count(&self) -> usize {
        if self.jobs == 0 {
            std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
        } else {
            self.jobs
        }
    }

    pub fn thumb_path(&self, rel: &std::path::Path) -> PathBuf {
        self.output_dir
            .join(rel)
            .with_extension(self.format.extension())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_defaults() {
        let cli = Cli::parse_from(["thumbgen", "photos"]);
        assert_eq!(cli.size, 256);
        assert_eq!(cli.format, Format::Webp);
        assert!(!cli.force);
    }

    #[test]
    fn rejects_out_of_range_quality() {
        assert!(Cli::try_parse_from(["thumbgen", "-q", "0", "x"]).is_err());
    }
}
