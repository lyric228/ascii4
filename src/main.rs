use anyhow::{anyhow, Context, Result};
use clap::Parser;
use ffmpeg_next as ffmpeg;
use image::{ImageBuffer, Rgb};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::Instant,
    convert::TryInto,
};
use sysx::utils::ascii::{image_to_ascii_configurable, AsciiArtConfig, CHAR_SET_DETAILED};

const EAGAIN: i32 = 11;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    input: String,

    #[arg(short, long, default_value = "output")]
    output_dir: String,

    #[arg(short, long, default_value_t = 30.0)]
    fps: f64,

    // Keep these as usize for clap parsing flexibility
    #[arg(short = 'W', long, default_value_t = 100)]
    width: usize,

    #[arg(short = 'H', long, default_value_t = 50)]
    height: usize,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let start_time = Instant::now();

    ffmpeg::init().context("Failed to initialize FFmpeg")?;

    let input_path = Path::new(&args.input);
    if !input_path.exists() {
        return Err(anyhow!("Input file not found: {}", args.input));
    }

    let main_output_dir_path = Path::new(&args.output_dir);
    fs::create_dir_all(main_output_dir_path)
        .with_context(|| format!("Failed to create main output directory: {:?}", main_output_dir_path))?;

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

    if frame_time_base.denominator() == 0 {
         return Err(anyhow!("Invalid video time base (denominator is zero)"));
    }

    // --- Using unsafe pointer access as requested ---
    // WARNING: This bypasses Rust safety guarantees.
    let (input_width, input_height) = unsafe {
        let params_ptr = codec_parameters.as_ptr();
        // Check for null pointer before dereferencing
        if params_ptr.is_null() {
             return Err(anyhow!("Codec parameters pointer is null"));
        }
        // Read width and height from the raw pointer
        ((*params_ptr).width, (*params_ptr).height)
    };

    println!(
        "Input video: {}x{} @ {:.2} FPS",
        input_width, // Use the values read via unsafe
        input_height, // Use the values read via unsafe
        original_fps
    );
    // --- End unsafe block ---

    println!("Target ASCII FPS: {:.2}", args.fps);
    println!("Main output directory: {}", args.output_dir);

    let context = ffmpeg::codec::context::Context::from_parameters(codec_parameters)
        .context("Failed to create codec context from parameters")?;
    let mut decoder = context.decoder().video()
        .context("Failed to create video decoder from context")?;

    // It's generally safer to use the decoder's reported dimensions after initialization
    let decoder_width = decoder.width();
    let decoder_height = decoder.height();

    let mut scaler = ffmpeg::software::scaling::context::Context::get(
        decoder.format(),
        decoder_width,  // Use decoder's width
        decoder_height, // Use decoder's height
        ffmpeg::format::Pixel::RGB24,
        decoder_width,  // Target dimensions for scaler
        decoder_height,
        ffmpeg::software::scaling::flag::Flags::BILINEAR,
    )
    .context("Failed to create scaler context")?;

    let mut total_output_frames: u64 = 0;
    let mut video_frame_count: u64 = 0;
    let mut last_processed_time_pts: i64 = -1;

    let mut last_processed_second: Option<u64> = None;
    let mut frame_count_in_second: u64 = 0;
    let mut current_second_dir: Option<PathBuf> = None;
    let min_pts_difference = if args.fps > 0.0 && args.fps < original_fps && args.fps != 0.0 {
         (frame_time_base.denominator() as f64 / (args.fps * frame_time_base.numerator() as f64)).round() as i64
    } else {
        1
    }.max(1);
    println!("Minimum PTS difference for target FPS: {}", min_pts_difference);


    // --- Cast usize to u32 for AsciiArtConfig ---
    // Add error handling for potential overflow, though unlikely for dimensions
    let ascii_width: u32 = args.width.try_into().context("Width value too large")?;
    let ascii_height: u32 = args.height.try_into().context("Height value too large")?;

    let ascii_config = AsciiArtConfig {
        width: ascii_width, // Use the converted u32 value
        height: ascii_height, // Use the converted u32 value
        char_set: CHAR_SET_DETAILED.chars().collect(),
        ..Default::default()
    };
    // --- End cast ---

    let temp_frame_path = main_output_dir_path.join("_temp_frame.png");

    'packet_loop: for (stream, packet) in ictx.packets() {
        if stream.index() == video_stream_index {
            if let Err(e) = decoder.send_packet(&packet) {
                // --- Fix Error::Other check using matches! ---
                 if e == ffmpeg::Error::Eof {
                    println!("\nDecoder signalled EOF while sending packet."); // More info
                    break 'packet_loop;
                 } else if matches!(e, ffmpeg::Error::Other { .. }) {
                     // EAGAIN or other non-fatal "try again" errors might appear as Error::Other
                     // Depending on the specific errno, might need different handling.
                     // For now, just print a warning and continue processing output frames.
                     eprintln!("\nWarning: Non-fatal error sending packet: {}", e);
                     // The loop below will try to receive frames, potentially clearing the state
                 } else {
                     // For other errors, return the error
                     return Err(anyhow!("Failed to send packet to decoder: {}", e));
                 }
                 // --- End Error::Other fix ---
            }

            let mut decoded_frame = ffmpeg::frame::Video::empty();
            // Use loop instead of while to handle potential EAGAIN from receive_frame
            loop {
                 match decoder.receive_frame(&mut decoded_frame) {
                     Ok(()) => {
                         // --- Frame processing logic (moved inside Ok arm) ---
                         video_frame_count += 1;
                         let current_pts = decoded_frame.pts().unwrap_or(0);

                         if current_pts >= 0 &&
                            (last_processed_time_pts == -1 || (current_pts - last_processed_time_pts) >= min_pts_difference)
                         {
                             last_processed_time_pts = current_pts;

                    let current_second = (current_pts as f64 * frame_time_base.numerator() as f64 / frame_time_base.denominator() as f64).floor() as u64;

                             if last_processed_second != Some(current_second) {
                                 let second_dir = main_output_dir_path.join(current_second.to_string());
                                 fs::create_dir_all(&second_dir).with_context(|| {
                                     format!("Failed to create directory for second {}: {:?}", current_second, second_dir)
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
                                     eprintln!("Error: Current second directory not set for frame {}. Skipping.", video_frame_count);
                                     continue; // Continue the inner loop (receive_frame)
                                 }
                             };

                             let mut rgb_frame = ffmpeg::frame::Video::empty();
                             if scaler.run(&decoded_frame, &mut rgb_frame).is_err() {
                                 eprintln!("Warning: Scaler failed for frame {}. Skipping.", video_frame_count);
                                 continue; // Continue the inner loop
                             }

                             let img_buf: ImageBuffer<Rgb<u8>, Vec<u8>> =
                                 match ImageBuffer::from_raw(
                                     rgb_frame.width(),
                                     rgb_frame.height(),
                                     rgb_frame.data(0).to_vec(),
                                 ) {
                                     Some(buf) => buf,
                                     None => {
                                         eprintln!("Warning: Failed to create image buffer for frame {}. Skipping.", video_frame_count);
                                         continue; // Continue the inner loop
                                     }
                                 };

                             if img_buf.save(&temp_frame_path).is_err() {
                                 eprintln!("Warning: Failed to save temporary frame {}. Skipping.", video_frame_count);
                                 continue; // Continue the inner loop
                             }

                             match image_to_ascii_configurable(&temp_frame_path, &ascii_config) {
                                 Ok(ascii_art) => {
                                     total_output_frames += 1;
                                     let output_filename = output_dir.join(format!("{}.txt", frame_count_in_second));
                                     match fs::File::create(&output_filename) {
                                         Ok(mut file) => {
                                             if file.write_all(ascii_art.as_bytes()).is_err() {
                                                 eprintln!("\nWarning: Failed to write ASCII art to file: {:?}", output_filename);
                                             }
                                         }
                                         Err(e) => {
                                             eprintln!("\nWarning: Failed to create output file {:?}: {}", output_filename, e);
                                         }
                                     }

                                     if total_output_frames % 10 == 0 {
                                         print!("\rProcessed ASCII frames: {}", total_output_frames);
                                         std::io::stdout().flush().unwrap_or_default();
                                     }
                                 }
                                 Err(e) => {
                                     eprintln!(
                                         "\nWarning: Failed to convert frame {} (sec {}, frame {}) to ASCII: {}",
                                         video_frame_count,
                                         current_second,
                                         frame_count_in_second,
                                         e
                                     );
                                 }
                             }
                         }
                         // --- End Frame processing logic ---
                     }
                     Err(ffmpeg::Error::Eof) => {
                         println!("\nDecoder signalled EOF while receiving frames.");
                         break; // Exit the inner receive_frame loop
                     }
                     Err(ffmpeg::Error::Other { errno }) if errno == EAGAIN => {
                         // Decoder needs more input packets, exit inner loop and get next packet
                         // println!("\nDecoder needs more input (EAGAIN)."); // Debugging
                         break; // Exit the inner receive_frame loop
                     }
                     Err(e) => {
                         // An unexpected error occurred during decoding
                         eprintln!("\nWarning: Error receiving frame: {}", e);
                         // Decide whether to break or continue; breaking is safer
                         break; // Exit the inner receive_frame loop
                     }
                 }
            } // End inner loop (receive_frame)
        } // End if stream_index
    } // End packet_loop ('packet_loop)

    // --- Cleanup and Final Flush ---
    if temp_frame_path.exists() {
        let _ = fs::remove_file(&temp_frame_path);
    }

    // Send EOF to the decoder *again* here to ensure it's flushed,
    // even if we broke out of the loop early.
    if let Err(e) = decoder.send_eof() {
        // Log EOF sending error but don't necessarily stop the program
        eprintln!("\nWarning: Failed to send EOF to decoder: {}", e);
    }

    // Drain any remaining frames from the decoder after EOF
    let mut decoded_frame = ffmpeg::frame::Video::empty();
    while decoder.receive_frame(&mut decoded_frame).is_ok() {
        // Note: We are *not* processing these flushed frames into ASCII here.
        // If precise handling of the very last frames is critical,
        // the processing logic (calculating second, saving ASCII) would need
        // to be duplicated or refactored into a function to be called here too.
        video_frame_count += 1; // Still count them as scanned
    }
    // --- End of Cleanup ---

    let duration = start_time.elapsed();
    println!("\n\nFinished processing.");
    println!("Total video frames scanned: {}", video_frame_count);
    println!("Total ASCII frames generated: {}", total_output_frames); // Use the total count
    println!("Output saved to subdirectories within: {}", args.output_dir); // Updated message
    println!("Total time: {:.2?}", duration);

    Ok(())
} // End of main function
