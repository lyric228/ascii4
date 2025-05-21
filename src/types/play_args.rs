use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
pub struct PlayArgs {
    /// Directory containing ASCII frames (organized in second subdirectories)
    #[arg(short, long, default_value = "output")]
    pub frames_dir: PathBuf,

    /// Playback FPS
    #[arg(short, long, default_value_t = 30.0)]
    pub fps: f64,

    /// Optional path to audio file or video file containing audio track
    #[arg(
        short,
        long,
        help = "Path to audio file or video file with audio track"
    )]
    pub audio: Option<PathBuf>,

    /// Loop the animation and audio like a GIF
    #[arg(short = 'g', long = "gif", default_value_t = false)]
    pub loop_gif: bool,

    /// Sync audio with animation loop (requires --gif)
    #[arg(
        short,
        long,
        requires = "loop_gif",
        help = "Restart audio with each animation loop (requires --gif)"
    )]
    pub sync: bool,
}
