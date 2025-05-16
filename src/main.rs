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

mod player;

const EAGAIN: i32 = 11;

// TODO: url for audio/video in args
// TODO: use video (.mp4) for -a/--audio
// TODO: photo convert
// TODO: GIF -g/--gif (animation&audio loop)

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

#[derive(Parser, Debug)]
struct ConvertArgs {
    /// Input video file path
    #[arg(short, long)]
    input: String,

    /// Output directory for ASCII frames (will contain subdirectories for seconds)
    #[arg(short, long, default_value = "output")]
    output_dir: String,

    /// Target FPS for ASCII conversion (sampling rate)
    #[arg(short, long, default_value_t = 15.0)]
    fps: f64,

    /// Output ASCII art width
    #[arg(short = 'W', long, default_value_t = 100)]
    width: usize,

    /// Output ASCII art height (maximum)
    #[arg(short = 'H', long, default_value_t = 50)]
    height: usize,
}

#[derive(Parser, Debug)]
struct PlayArgs {
    /// Directory containing ASCII frames (organized in second subdirectories)
    #[arg(short, long, default_value = "output")]
    frames_dir: PathBuf,

    /// Playback FPS
    #[arg(short, long, default_value_t = 30.0)]
    fps: f64,

    /// Optional path to audio file or video file containing audio track
    #[arg(
        short,
        long,
        help = "Path to audio file or video file with audio track"
    )]
    audio: Option<PathBuf>,

    /// Loop the animation and audio like a GIF
    #[arg(short = 'g', long = "gif", default_value_t = false)]
    loop_gif: bool,
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
            let player_options = player::PlayerOptions {
                frames_dir: args.frames_dir,
                fps: args.fps,
                audio_path: args.audio,
                loop_gif: args.loop_gif,
            };
            player::play_animation(player_options)?;
        }
    }

    let duration = start_time.elapsed();
    println!("\nCommand completed in: {duration:.2?}");
    Ok(())
}

struct CleanupGuard(PathBuf);

impl CleanupGuard {
    fn new(path: PathBuf) -> Self {
        Self(path)
    }
}

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        if self.0.exists() {
            let _ = fs::remove_file(&self.0);
        }
    }
}


fn run_conversion(args: ConvertArgs) -> Result<()> {
    ffmpeg::init().context("Failed to initialize FFmpeg")?;

    let input_path = Path::new(&args.input);
    if !input_path.exists() {
        return Err(anyhow!("Input file not found: {}", args.input));
    }

    let main_output_dir_path = Path::new(&args.output_dir);
    fs::create_dir_all(main_output_dir_path).with_context(|| {
        format!("Failed to create main output directory: {main_output_dir_path:?}")
    })?;

    let temp_frame_path = main_output_dir_path.join("_temp_frame.png");
    let _cleanup_guard = CleanupGuard::new(temp_frame_path.clone()); // Use the guard for cleanup


    let mut ictx = ffmpeg::format::input(&input_path)
        .with_context(|| format!("Failed to open input file: {}", args.input))?;

    let input_stream = ictx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or_else(|| anyhow!("Could not find video stream in input file"))?;
    let video_stream_index = input_stream.index();
    let codec_parameters = input_stream.parameters();

    let mut decoder = ffmpeg::codec::context::Context::from_parameters(codec_parameters)?
        .decoder()
        .video()?;
    let frame_time_base = input_stream.time_base();
    let video_fps: f64 = input_stream.rate().into();
    let target_fps = args.fps;
    let min_pts_difference = (video_fps / target_fps).round() as i64;

    let mut scaler = ffmpeg::software::scaling::Context::get(
        decoder.format(),
        decoder.width(),
        decoder.height(),
        ffmpeg::format::Pixel::RGB24,
        decoder.width(),
        decoder.height(),
        ffmpeg::software::scaling::Flags::BILINEAR,
    )?;
    let mut video_frame_count = 0;
    let mut total_output_frames = 0;
    let mut last_processed_time_pts = -1;
    let mut last_processed_second: Option<u64> = None;
    let mut current_second_dir: Option<PathBuf> = None;
    let mut frame_count_in_second = 0;

    let ascii_width: u32 = args.width.try_into().context("Width value too large")?;
    let ascii_height: u32 = args.height.try_into().context("Height value too large")?;
    let ascii_config = AsciiArtConfig {
        width: ascii_width,
        height: ascii_height,
        char_set: CHAR_SET_VERY_DETAILED.chars().collect(),
        ..Default::default()
    };

    for (stream, packet) in ictx.packets() {
        if stream.index() == video_stream_index {
            match decoder.send_packet(&packet) {
                Ok(()) => (),
                Err(e) if matches!(e, ffmpeg::Error::Other { .. }) => {
                    eprintln!("\nWarning: Non-fatal error when sending packet: {e}");
                }
                Err(e) => {
                    return Err(anyhow!("Failed to send packet to decoder: {}", e));
            }
                                    }

            let mut decoded_frame = ffmpeg::frame::Video::empty();
            loop {
                match decoder.receive_frame(&mut decoded_frame) {
                    Ok(()) => {
                        video_frame_count += 1;
                        let current_pts = decoded_frame.pts().unwrap_or(0);

                        if current_pts >= 0
                            && (last_processed_time_pts == -1
                                || (current_pts - last_processed_time_pts) >= min_pts_difference)
                        {
                            last_processed_time_pts = current_pts;

                            let current_second = (current_pts as f64
                                * frame_time_base.numerator() as f64
                                / frame_time_base.denominator() as f64)
                                .floor() as u64;

                            if last_processed_second != Some(current_second) {
                                let second_dir =
                                    main_output_dir_path.join(current_second.to_string());
                                fs::create_dir_all(&second_dir).with_context(|| {
                                    format!("Failed to create directory for second {current_second}: {second_dir:?}")
                                })?;
                                current_second_dir = Some(second_dir);
                                frame_count_in_second = 1;
                                last_processed_second = Some(current_second);
                            } else {
                                frame_count_in_second += 1;
                            }

                            let output_dir = match &current_second_dir {
                                Some(dir) => dir,
                                None => {
                                    eprintln!(
                                        "Error: Current second directory not set for frame {video_frame_count}. Skipping."
                                    );
                                    continue;
                                }
                            };

                            let mut rgb_frame = ffmpeg::frame::Video::empty();
                            if scaler.run(&decoded_frame, &mut rgb_frame).is_err() {
                                eprintln!(
                                    "Warning: Scaling failed for frame {video_frame_count}. Skipping."
                                );
                                continue;
                            }

                            let img_buf: ImageBuffer<Rgb<u8>, Vec<u8>> = match ImageBuffer::from_raw(
                                rgb_frame.width(),
                                rgb_frame.height(),
                                rgb_frame.data(0).to_vec(),
                            ) {
                                Some(buf) => buf,
                                None => {
                                    eprintln!(
                                        "Warning: Failed to create image buffer for frame {video_frame_count}. Skipping."
                                    );
                                    continue;
                                }
                            };

                            if img_buf.save(&temp_frame_path).is_err() {
                                eprintln!(
                                    "Warning: Failed to save temporary frame {video_frame_count}. Skipping."
                                );
                                continue;
                            }

                            match image_to_ascii_configurable(&temp_frame_path, &ascii_config) {
                                Ok(ascii_art) => {
                                    total_output_frames += 1;
                                    let output_filename =
                                        output_dir.join(format!("{frame_count_in_second}.txt"));
                                    match fs::File::create(&output_filename) {
                                        Ok(mut file) => {
                                            if file.write_all(ascii_art.as_bytes()).is_err() {
                                                eprintln!(
                                                    "\nWarning: Failed to write ASCII art to file: {output_filename:?}"
                                                );
                                            }
                                        }
                                        Err(e) => {
                                            eprintln!(
                                                "\nWarning: Failed to create output file {output_filename:?}: {e}"
                                            );
                                        }
                                    }

                                    if total_output_frames % 10 == 0 {
                                        print!("\rProcessed ASCII frames: {total_output_frames}");
                                        std::io::stdout().flush().unwrap_or_default();
                                    }
                                }
                                Err(e) => {
                                    eprintln!(
                                        "\nWarning: Failed to convert frame {video_frame_count} (sec {current_second}, frame {frame_count_in_second}) to ASCII: {e}"
                                    );
                                }
                            }
                        }
                    }
                    Err(ffmpeg::Error::Eof) => {
                        break;
                    }
                    Err(ffmpeg::Error::Other { errno }) if errno == EAGAIN => {
                        break;
                    }
                    Err(e) => {
                        eprintln!("\nWarning: Error receiving frame: {e}");
                        break;
                    }
                }
            }
        }
    }

    if let Err(e) = decoder.send_eof() {
        if e != ffmpeg::Error::Eof {
            eprintln!("\nWarning: Failed to send final EOF to decoder: {e}");
        }
    }

    let mut decoded_frame = ffmpeg::frame::Video::empty();
    while decoder.receive_frame(&mut decoded_frame).is_ok() {
        video_frame_count += 1;
    }
    Ok(())
}
