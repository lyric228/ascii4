use anyhow::{anyhow, Context, Result};
use crossterm::{cursor, execute, terminal, ExecutableCommand};
use rodio::{Decoder, OutputStream, Sink, Source};
use std::{
    fs::{self, File},
    io::{stdout, BufReader, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use sysx::time::safe_sleep;

/// Player options for ASCII animation
#[derive(Debug)]
pub struct PlayerOptions {
    pub frames_dir: PathBuf,
    pub fps: f64,
    pub audio_path: Option<PathBuf>,
    pub loop_gif: bool,
}

/// Structure for frame path and number
#[derive(Debug)]
struct FrameInfo {
    path: PathBuf,
    number: u64,
}

/// Structure for second information and its frames
#[derive(Debug)]
struct SecondInfo {
    number: u64,
    frames: Vec<FrameInfo>,
}

/// Main animation playback function
pub fn play_animation(options: PlayerOptions) -> Result<()> {
    if options.fps <= 0.0 {
        return Err(anyhow!("FPS must be positive"));
    }

    println!("Scanning frames directory: {:?}", options.frames_dir);
    let ordered_frames = discover_and_sort_frames(&options.frames_dir)?;

    if ordered_frames.is_empty() {
        return Err(anyhow!(
            "No valid frame files found in directory structure: {:?}",
            options.frames_dir
        ));
    }
    println!(
        "Found {} frames. Target FPS: {}",
        ordered_frames.len(),
        options.fps
    );

    // Audio initialization
    let (_stream, stream_handle) = OutputStream::try_default()
        .map_err(|e| anyhow!("Failed to get default audio output device: {}", e))?;
    let sink = Sink::try_new(&stream_handle)
        .map_err(|e| anyhow!("Failed to create audio sink: {}", e))?;

    let mut audio_source: Option<Box<dyn rodio::Source<Item = f32> + Send>> = None;

    if let Some(audio_path) = &options.audio_path {
        println!("Loading audio from: {audio_path:?}");
        if !audio_path.exists() {
            eprintln!("Warning: Audio/video file not found at: {audio_path:?}");
        } else {
            let file = BufReader::new(File::open(audio_path)
                .with_context(|| format!("Failed to open file for audio stream: {audio_path:?}"))?);

            match Decoder::new(file) {
                Ok(source) => {
                    audio_source = Some(Box::new(source));
                    println!("Audio stream loaded.");
                },
                Err(e) => {
                    eprintln!("Warning: Failed to decode audio stream from {audio_path:?}: {e}. Audio will not play.");
                }
            }
        }
    }


    // Terminal preparation
    let mut stdout = stdout();
    stdout.execute(terminal::EnterAlternateScreen)?;
    stdout.execute(cursor::Hide)?;
    stdout.flush()?;

    // Playback loop
    let frame_duration = Duration::from_secs_f64(1.0 / options.fps);

    let mut playback_loop = || -> Result<()> {
        if options.loop_gif && audio_source.is_some() {
    sink.stop();
            let audio_path = options.audio_path.as_ref().unwrap();
            let file = BufReader::new(File::open(audio_path)
                .with_context(|| format!("Failed to re-open file for audio stream: {audio_path:?}"))?);

            match Decoder::new(file) {
                Ok(source) => {
                    sink.append(source);
                    println!("Audio stream reloaded for loop.");
                },
                Err(e) => {
                    eprintln!("Warning: Failed to re-decode audio stream for loop: {e}. Audio will not play during this loop.");
                }
            }
        } else if audio_source.is_some() && sink.empty() && !options.loop_gif {
            let audio_path = options.audio_path.as_ref().unwrap();
            let file = BufReader::new(File::open(audio_path).with_context(|| format!("Failed to open file for audio stream: {audio_path:?}"))?);
            
            match Decoder::new(file) {
                Ok(source) => {
                    sink.append(source);
                    println!("Audio stream appended.");
                },
                Err(e) => {
                        eprintln!("Warning: Failed to decode audio stream: {e}. Audio will not play.");
                }
            }
        }

        for frame_path in &ordered_frames {
            let start_time = Instant::now();
            let frame_content = match fs::read_to_string(&frame_path) {
                 Ok(content) => content,
                 Err(e) => {
                    eprintln!("\nError reading frame file {frame_path:?}: {e}. Stopping playback.");
                    return Err(anyhow!("Failed to read frame: {:?}", frame_path).context(e));
                }
            };

            execute!(
                stdout,
                terminal::Clear(terminal::ClearType::All),
                cursor::MoveTo(0, 0),
            )?;

            write!(stdout, "{frame_content}")?;
            stdout.flush()?;

            let elapsed = start_time.elapsed();
            let sleep_duration = frame_duration.saturating_sub(elapsed);

            safe_sleep(sleep_duration)?;
        }

        Ok(())
    };

    if options.loop_gif {
        loop {
            if let Err(e) = playback_loop() {
                eprintln!("Error during playback loop: {}", e);
                break;
            }
            safe_sleep("10ms");
        }
                                } else {
        if let Err(e) = playback_loop() {
            eprintln!("Error during playback: {}", e);
            return Err(e);
    }
                            }

    // Terminal cleanup
    let _ = stdout.execute(cursor::Show);
    let _ = stdout.execute(terminal::LeaveAlternateScreen);
    let _ = stdout.flush();

    sink.stop();
    println!("\nPlayback finished.");

    Ok(())
}

/// Discovers and sorts frame files in directory
fn discover_and_sort_frames(base_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut seconds: Vec<SecondInfo> = Vec::new();

    for entry_res in fs::read_dir(base_dir).with_context(|| format!("Failed to read base directory: {base_dir:?}"))? {
        let entry = entry_res?;
        let path = entry.path();

        if path.is_dir() {
            if let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) {
                if let Ok(second_num) = dir_name.parse::<u64>() {
                    let mut current_second = SecondInfo {
                        number: second_num,
                        frames: Vec::new(),
                    };

                    for frame_entry_res in fs::read_dir(&path).with_context(|| format!("Failed to read second directory: {path:?}"))? {
                        let frame_entry = frame_entry_res?;
                        let frame_path = frame_entry.path();

                        if frame_path.is_file() && frame_path.extension().is_some_and(|ext| ext == "txt") {
                            if let Some(frame_stem) = frame_path.file_stem().and_then(|s| s.to_str()) {
                                if let Ok(frame_num) = frame_stem.parse::<u64>() {
                                    current_second.frames.push(FrameInfo {
                                        path: frame_path,
                                        number: frame_num,
                                    });
                                } else {
                                    eprintln!("Warning: Could not parse frame number from file name: {frame_path:?}");
                }
            }
        }
    }
                    current_second.frames.sort_by_key(|f| f.number);

                    if !current_second.frames.is_empty() {
                        seconds.push(current_second);
                    }

                } else {
                    eprintln!("Warning: Directory name is not a valid second number: {path:?}");
                }
            }
        }
    }
    seconds.sort_by_key(|s| s.number);

    let ordered_frame_paths: Vec<PathBuf> = seconds
        .into_iter()
        .flat_map(|s| s.frames.into_iter().map(|f| f.path))
        .collect();

    Ok(ordered_frame_paths)
}
