use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand};
use ffmpeg_next as ffmpeg;
use image::{ImageBuffer, Rgb};
use std::{
    convert::TryInto,
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::Instant,
};
use sysx::utils::ascii::{AsciiArtConfig, CHAR_SET_VERY_DETAILED, image_to_ascii_configurable};

mod play;
mod convert;

use play::*;
use convert::*;

// TODO: url for audio/video in args
// TODO: use video (.mp4) for -a/--audio
// TODO: photo convert
// TODO: terminal auto size (convert)
// TODO: reverse video

#[derive(Parser, Debug)]
#[command(author, version, about = "Converts video to ASCII art and plays it", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Convert video file to ASCII frames organized by seconds
    Convert(ConvertArgs),
    /// Play ASCII animation from frames directory
    Play(PlayArgs),
}

fn main() -> Result<()> {
    ctrlc::set_handler(|| {
        use crossterm::{cursor, execute, terminal};
        use std::io::stdout;
        use std::process::exit;

        let mut stdout = stdout();

        let _ = execute!(stdout, cursor::Show);
        let _ = execute!(stdout, terminal::LeaveAlternateScreen);
        let _ = stdout.flush();

        exit(0);
    })
    .with_context(|| "Failed to set Ctrl+C handler")?;

    let cli = Cli::parse();
    let start_time = Instant::now();

    match cli.command {
        Commands::Convert(args) => {
            println!("Starting conversion...");
            run_conversion(args)?;
        }
        Commands::Play(args) => {
            println!("Starting player...");
            play_animation(args)?;
        }
    }

    let duration = start_time.elapsed();
    println!("\nCommand completed in: {duration:.2?}");

    Ok(())
}
