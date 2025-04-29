use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand}; // Добавили Subcommand
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

// --- Объявляем модуль player ---
mod player;
// ---

const EAGAIN: i32 = 11; // Или используем libc::EAGAIN, если libc добавлен как зависимость

// --- Основная структура CLI с подкомандами ---
#[derive(Parser, Debug)]
#[command(author, version, about = "Convert videos to ASCII art and play them", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

// --- Перечисление возможных подкоманд ---
#[derive(Subcommand, Debug)]
enum Commands {
    /// Convert a video file into ASCII frames organized by second
    Convert(ConvertArgs),
    /// Play an ASCII animation from a directory of frames
    Play(PlayArgs),
}

// --- Аргументы для команды convert ---
#[derive(Parser, Debug)]
struct ConvertArgs {
    /// Path to the input video file
    #[arg(short, long)]
    input: String,

    /// Output directory for ASCII frames (will contain subdirs for seconds)
    #[arg(short, long, default_value = "output")]
    output_dir: String,

    /// Target frames per second for ASCII conversion (sampling rate)
    #[arg(short, long, default_value_t = 15.0)] // Увеличил дефолтное значение для конвертации
    fps: f64,

    /// Width of the output ASCII art
    #[arg(short = 'W', long, default_value_t = 100)]
    width: usize,

    /// Height of the output ASCII art (maximum)
    #[arg(short = 'H', long, default_value_t = 50)]
    height: usize,
}

// --- Аргументы для команды play ---
#[derive(Parser, Debug)]
struct PlayArgs {
    /// Directory containing the ASCII frames (organized by second subdirs)
    #[arg(short, long, default_value = "output")]
    frames_dir: PathBuf, // Используем PathBuf для clap

    /// Frames per second for playback
    #[arg(short, long, default_value_t = 30.0)] // Дефолтное значение для воспроизведения
    fps: f64,

    /// Optional path to an audio file (e.g., mp3, wav) to play alongside the animation
    #[arg(short, long)]
    audio: Option<PathBuf>, // Используем PathBuf для clap
}


// --- Основная функция ---
fn main() -> Result<()> {
    let cli = Cli::parse();
    let start_time = Instant::now();

    // --- Диспетчеризация команд ---
    match cli.command {
        Commands::Convert(args) => {
            println!("Running conversion...");
            run_conversion(args)?;
        }
        Commands::Play(args) => {
            println!("Running player...");
            // --- Создаем опции для плеера ---
            let player_options = player::PlayerOptions {
                frames_dir: args.frames_dir,
                fps: args.fps,
                audio_path: args.audio,
            };
            player::play_animation(player_options)?;
        }
    }

    let duration = start_time.elapsed();
    println!("\nCommand finished in: {:.2?}", duration);

    Ok(())
}

// --- Логика конвертации вынесена в отдельную функцию ---
fn run_conversion(args: ConvertArgs) -> Result<()> {
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

    let (input_width, input_height) = unsafe {
        let params_ptr = codec_parameters.as_ptr();
        if params_ptr.is_null() {
             return Err(anyhow!("Codec parameters pointer is null"));
        }
        ((*params_ptr).width, (*params_ptr).height)
    };

    println!(
        "Input video: {}x{} @ {:.2} FPS",
        input_width, input_height, original_fps
    );
    println!("Target ASCII FPS for conversion: {:.2}", args.fps); // Уточнили
    println!("Output directory: {}", args.output_dir);

    let context = ffmpeg::codec::context::Context::from_parameters(codec_parameters)
        .context("Failed to create codec context from parameters")?;
    let mut decoder = context.decoder().video()
        .context("Failed to create video decoder from context")?;

    let decoder_width = decoder.width();
    let decoder_height = decoder.height();

    let mut scaler = ffmpeg::software::scaling::context::Context::get(
        decoder.format(),
        decoder_width,
        decoder_height,
        ffmpeg::format::Pixel::RGB24,
        decoder_width,
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
    println!("Minimum PTS difference for conversion sampling: {}", min_pts_difference); // Уточнили

    let ascii_width: u32 = args.width.try_into().context("Width value too large")?;
    let ascii_height: u32 = args.height.try_into().context("Height value too large")?;

    let ascii_config = AsciiArtConfig {
        width: ascii_width,
        height: ascii_height,
        char_set: CHAR_SET_DETAILED.chars().collect(),
        ..Default::default()
    };

    let temp_frame_path = main_output_dir_path.join("_temp_frame.png");

    'packet_loop: for (stream, packet) in ictx.packets() {
        if stream.index() == video_stream_index {
            if let Err(e) = decoder.send_packet(&packet) {
                 if e == ffmpeg::Error::Eof {
                    println!("\nDecoder signalled EOF while sending packet.");
                    break 'packet_loop;
                 } else if matches!(e, ffmpeg::Error::Other { .. }) {
                     eprintln!("\nWarning: Non-fatal error sending packet: {}", e);
                 } else {
                     return Err(anyhow!("Failed to send packet to decoder: {}", e));
                 }
            }

            let mut decoded_frame = ffmpeg::frame::Video::empty();
            loop {
                 match decoder.receive_frame(&mut decoded_frame) {
                     Ok(()) => {
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
                                     continue;
                                 }
                             };

                             let mut rgb_frame = ffmpeg::frame::Video::empty();
                             if scaler.run(&decoded_frame, &mut rgb_frame).is_err() {
                                 eprintln!("Warning: Scaler failed for frame {}. Skipping.", video_frame_count);
                                 continue;
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
                                         continue;
                                     }
                                 };

                             if img_buf.save(&temp_frame_path).is_err() {
                                 eprintln!("Warning: Failed to save temporary frame {}. Skipping.", video_frame_count);
                                 continue;
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
                     }
                     Err(ffmpeg::Error::Eof) => {
                         // EOF при получении кадра - это нормально, просто выходим из внутреннего цикла
                         break;
                     }
                     Err(ffmpeg::Error::Other { errno }) if errno == EAGAIN => {
                         // Нужно больше пакетов
                         break;
                     }
                     Err(e) => {
                         eprintln!("\nWarning: Error receiving frame: {}", e);
                         break;
                     }
                 }
            } // End inner loop
        } // End if stream_index
    } // End packet_loop

    if temp_frame_path.exists() {
        let _ = fs::remove_file(&temp_frame_path);
    }

    if let Err(e) = decoder.send_eof() {
        if e != ffmpeg::Error::Eof {
            eprintln!("\nWarning: Failed to send final EOF to decoder: {}", e);
        }
    }

    let mut decoded_frame = ffmpeg::frame::Video::empty();
    while decoder.receive_frame(&mut decoded_frame).is_ok() {
        video_frame_count += 1;
    }

    // --- Перенес вывод статистики в конец основной функции ---
    // println!("\n\nFinished conversion.");
    // println!("Total video frames scanned: {}", video_frame_count);
    // println!("Total ASCII frames generated: {}", total_output_frames);
    // println!("Output saved to subdirectories within: {}", args.output_dir);

    Ok(())
} // --- Конец функции run_conversion ---