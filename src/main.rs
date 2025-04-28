use anyhow::{anyhow, Context, Result};
use clap::Parser;
use ffmpeg_next as ffmpeg;
use image::{ImageBuffer, Rgb};
use std::{fs, io::Write, path::Path, time::Instant};
use sysx::utils::ascii::{image_to_ascii_configurable, AsciiArtConfig, CHAR_SET_DETAILED};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the input video file
    #[arg(short, long)]
    input: String,

    /// Target frames per second for ASCII conversion
    #[arg(short, long, default_value_t = 5.0)]
    fps: f64,

    /// Width of the output ASCII art
    #[arg(long, default_value_t = 120)]
    width: u32,

    /// Height of the output ASCII art (maximum)
    #[arg(long, default_value_t = 60)]
    height: u32,

    /// Output directory for ASCII frames
    #[arg(short, long, default_value = "frames")]
    output_dir: String,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let start_time = Instant::now();

    ffmpeg::init().context("Failed to initialize FFmpeg")?;

    let input_path = Path::new(&args.input);
    if !input_path.exists() {
        return Err(anyhow!("Input file not found: {}", args.input));
    }

    let output_dir_path = Path::new(&args.output_dir);
    fs::create_dir_all(output_dir_path)
        .with_context(|| format!("Failed to create output directory: {:?}", output_dir_path))?;

    let mut ictx = ffmpeg::format::input(&input_path)
        .with_context(|| format!("Failed to open input file: {}", args.input))?;

    let input_stream = ictx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or_else(|| anyhow!("Could not find video stream in input file"))?;

    let video_stream_index = input_stream.index();
    let codec_parameters = input_stream.parameters();

    let frame_rate = input_stream.avg_frame_rate();
    let original_fps = if frame_rate.denominator() != 0 {
        frame_rate.numerator() as f64 / frame_rate.denominator() as f64
    } else {
        0.0
    };
    let frame_time_base = input_stream.time_base();

    unsafe {
        println!(
            "Input video: {}x{} @ {:.2} FPS",
            codec_parameters.as_ptr().read().width,
            codec_parameters.as_ptr().read().height,
            original_fps
        );
    }
    
    println!("Target ASCII FPS: {:.2}", args.fps);
    println!("Output directory: {}", args.output_dir);

    let context = ffmpeg::codec::context::Context::from_parameters(codec_parameters)
        .context("Failed to create codec context from parameters")?;
    let mut decoder = context.decoder().video()
        .context("Failed to create video decoder from context")?;

    let mut scaler = ffmpeg::software::scaling::context::Context::get(
        decoder.format(),
        decoder.width(),
        decoder.height(),
        ffmpeg::format::Pixel::RGB24,
        decoder.width(),
        decoder.height(),
        ffmpeg::software::scaling::flag::Flags::BILINEAR,
    )
    .context("Failed to create scaler context")?;

    let mut output_frame_count: u64 = 0;
    let mut video_frame_count: u64 = 0;
    let mut last_processed_time_pts: i64 = -1;

    let min_pts_difference = if args.fps > 0.0 && args.fps < original_fps {
         (frame_time_base.denominator() as f64 / (args.fps * frame_time_base.numerator() as f64)) as i64
    } else {
        1
    };

    println!("Minimum PTS difference for target FPS: {}", min_pts_difference);

    let ascii_config = AsciiArtConfig {
        width: args.width,
        height: args.height,
        char_set: CHAR_SET_DETAILED.chars().collect(),
        ..Default::default()
    };

    let temp_frame_path = Path::new(&args.output_dir).join("_temp_frame.png");

    for (stream, packet) in ictx.packets() {
        if stream.index() == video_stream_index {
            decoder
                .send_packet(&packet)
                .context("Failed to send packet to decoder")?;
    let mut decoded_frame = ffmpeg::frame::Video::empty();
     while decoder.receive_frame(&mut decoded_frame).is_ok() {
                video_frame_count += 1;
                let current_pts = decoded_frame.pts().unwrap_or(0);

                if current_pts == 0 || last_processed_time_pts == -1 || (current_pts - last_processed_time_pts) >= min_pts_difference {
                    last_processed_time_pts = current_pts;

                    let mut rgb_frame = ffmpeg::frame::Video::empty();
                    scaler
                        .run(&decoded_frame, &mut rgb_frame)
                        .context("Scaler failed to convert frame")?;

                    let img_buf: ImageBuffer<Rgb<u8>, Vec<u8>> =
                        match ImageBuffer::from_raw(
                            rgb_frame.width(),
                            rgb_frame.height(),
                            rgb_frame.data(0).to_vec(),
                        ) {
                            Some(buf) => buf,
                            None => {
                                eprintln!("Warning: Failed to create image buffer for frame {}", video_frame_count);
                                continue;
     }
                        };

                    img_buf
                        .save(&temp_frame_path)
                        .with_context(|| format!("Failed to save temporary frame: {:?}", temp_frame_path))?;

                    match image_to_ascii_configurable(&temp_frame_path, &ascii_config) {
                        Ok(ascii_art) => {
                            output_frame_count += 1;
                            let output_filename =
                                output_dir_path.join(format!("{}.txt", output_frame_count));
                            let mut file = fs::File::create(&output_filename).with_context(|| {
                                format!("Failed to create output file: {:?}", output_filename)
                            })?;
                            file.write_all(ascii_art.as_bytes())
                                .context("Failed to write ASCII art to file")?;

                            if output_frame_count % 10 == 0 {
                                print!("\rProcessed ASCII frames: {}", output_frame_count);
                                std::io::stdout().flush().unwrap_or_default();
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "\nWarning: Failed to convert frame {} (output frame {}) to ASCII: {}",
                                video_frame_count, output_frame_count + 1, e
                            );
                        }
                    }
                }
            }
        }
    }

    if temp_frame_path.exists() {
        fs::remove_file(&temp_frame_path).with_context(|| {
            format!("Failed to remove temporary frame file: {:?}", temp_frame_path)
        })?;
    }

    decoder.send_eof().context("Failed to send EOF to decoder")?;
    let mut decoded_frame = ffmpeg::frame::Video::empty();
    while decoder.receive_frame(&mut decoded_frame).is_ok() {}

    let duration = start_time.elapsed();
    println!("\n\nFinished processing.");
    println!("Total video frames scanned: {}", video_frame_count);
    println!("Total ASCII frames generated: {}", output_frame_count);
    println!("Output saved to: {}", args.output_dir);
    println!("Total time: {:.2?}", duration);

    Ok(())
}
