use anyhow::{Context, Result, anyhow};
use clap::Parser;
use crossterm::{ExecutableCommand, cursor, execute, terminal};
use rodio::{Decoder, OutputStream, Sink, Source};
use std::{
    fs::{self, File},
    io::{BufReader, Write, stdout},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use sysx::time::safe_sleep;
use crate::play_args::PlayArgs;
use crate::terminal_guard::TerminalGuard;
use crate::info::{FrameInfo, SecondInfo};

fn initialize_audio(options: &PlayArgs) -> Result<(Sink, OutputStream)> {
    let (stream, stream_handle) = OutputStream::try_default()?;
    let sink = Sink::try_new(&stream_handle)?;

    if let Some(path) = &options.audio {
        load_audio_file(&sink, path).unwrap_or_else(|e| eprintln!("Audio loading error: {e}"));
    }

    Ok((sink, stream))
}

fn load_audio_file(sink: &Sink, path: &Path) -> Result<()> {
    let file = BufReader::new(File::open(path)?);
    let source = Decoder::new(file)?.convert_samples::<f32>();
    sink.append(source);
    Ok(())
}

fn render_frame(content: &str) -> Result<()> {
    execute!(
        stdout(),
        terminal::Clear(terminal::ClearType::All),
        cursor::MoveTo(0, 0),
    )?;
    write!(stdout(), "{content}")?;
    stdout().flush()?;
    Ok(())
}

/// Main animation playback function
pub fn play_animation(options: PlayArgs) -> Result<()> {
    if options.fps <= 0.0 {
        return Err(anyhow!("FPS must be positive"));
    }

    let _terminal_guard = TerminalGuard::new();

    let (sink, _stream) = initialize_audio(&options)?;

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

    let frame_contents: Vec<String> = ordered_frames
        .iter()
        .map(|path| {
            fs::read_to_string(path).context(format!("Failed to read frame file: {path:?}"))
        })
        .collect::<Result<_, _>>()?;

    let playback_loop = || -> Result<()> {
        if options.sync {
            sink.stop();
            if let Some(path) = &options.audio {
                load_audio_file(&sink, path)
                    .unwrap_or_else(|e| eprintln!("Audio reload error: {e}"));
            }
        }

        let frame_duration = Duration::from_secs_f64(1.0 / options.fps);
        let mut last_frame_time = Instant::now();

        for content in &frame_contents {
            render_frame(content)?;

            let elapsed = last_frame_time.elapsed();
            let sleep_duration = frame_duration.saturating_sub(elapsed);
            safe_sleep(sleep_duration)?;

            last_frame_time = Instant::now();
        }
        Ok(())
    };

    if options.loop_gif {
        loop {
            playback_loop()?;
            safe_sleep("10ms")?;
        }
    } else {
        playback_loop()?;
    }

    sink.stop();
    println!("\nPlayback finished.");

    Ok(())
}

/// Discovers and sorts frame files in directory
fn discover_and_sort_frames(base_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut seconds: Vec<SecondInfo> = Vec::new();

    let entries = fs::read_dir(base_dir)
        .with_context(|| format!("Failed to read base directory: {base_dir:?}"))?;

    for entry_res in entries {
        let entry = entry_res?;
        let path = entry.path();

        if path.is_dir() {
            if let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) {
                if let Ok(second_num) = dir_name.parse::<u64>() {
                    let mut current_second = SecondInfo {
                        number: second_num,
                        frames: Vec::new(),
                    };

                    for frame_entry_res in fs::read_dir(&path)
                        .with_context(|| format!("Failed to read second directory: {path:?}"))?
                    {
                        let frame_entry = frame_entry_res?;
                        let frame_path = frame_entry.path();

                        if frame_path.is_file()
                            && frame_path.extension().is_some_and(|ext| ext == "txt")
                        {
                            if let Some(frame_stem) =
                                frame_path.file_stem().and_then(|s| s.to_str())
                            {
                                if let Ok(frame_num) = frame_stem.parse::<u64>() {
                                    current_second.frames.push(FrameInfo {
                                        path: frame_path,
                                        number: frame_num,
                                    });
                                } else {
                                    eprintln!(
                                        "Warning: Could not parse frame number from file name: {frame_path:?}"
                                    );
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
        } else if path.is_file() && path.extension().is_some_and(|ext| ext == "txt") {
            if let Some(file_stem) = path.file_stem().and_then(|s| s.to_str()) {
                if let Ok(frame_num) = file_stem.parse::<u64>() {
                    // Collect root files into a single SecondInfo { number: 0 } entry
                    // Find or create the SecondInfo for number 0
                    let second_0 = seconds.iter_mut().find(|s| s.number == 0);
                    if let Some(second) = second_0 {
                        second.frames.push(FrameInfo {
                            path,
                            number: frame_num,
                        });
                    } else {
                        seconds.push(SecondInfo {
                            number: 0,
                            frames: vec![FrameInfo {
                                path,
                                number: frame_num,
                            }],
                        });
                    }
                } else {
                    eprintln!(
                        "Warning: Could not parse frame number from root file name: {path:?}"
                    );
                }
            }
        }
    }

    // Sort frames within the root (second 0) if it exists
    if let Some(second_0) = seconds.iter_mut().find(|s| s.number == 0) {
        second_0.frames.sort_by_key(|f| f.number);
    }

    seconds.sort_by_key(|s| s.number);

    let ordered_frame_paths: Vec<PathBuf> = seconds
        .into_iter()
        .flat_map(|s| s.frames.into_iter().map(|f| f.path))
        .collect();

    Ok(ordered_frame_paths)
}
