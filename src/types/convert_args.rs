use clap::Parser;

#[derive(Parser, Debug)]
pub struct ConvertArgs {
    /// Input video file path
    #[arg(short, long)]
    pub input: String,

    /// Output directory for ASCII frames
    #[arg(short, long, default_value = "output")]
    pub output_dir: String,

    /// Target FPS for ASCII conversion
    #[arg(short, long, default_value_t = 15.0)]
    pub fps: f64,

    /// Output ASCII art width
    #[arg(short = 'W', long, default_value_t = 100)]
    pub width: usize,

    /// Output ASCII art height
    #[arg(short = 'H', long, default_value_t = 50)]
    pub height: usize,

    /// Automatically set output size based on terminal size if width/height not specified
    #[arg(short = 'A', long)]
    pub auto_size: bool,
}
