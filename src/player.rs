use anyhow::{anyhow, Context, Result};
use crossterm::{cursor, execute, terminal, ExecutableCommand};
use rodio::{Decoder, OutputStream, Sink, Source};
use std::{
    fs::{self, File},
    io::{stdout, BufReader, Read, Write},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

/// Опции для плеера ASCII анимации
#[derive(Debug)]
pub struct PlayerOptions {
    pub frames_dir: PathBuf,
    pub fps: f64,
    pub audio_path: Option<PathBuf>,
    // Пока не будем извлекать из видео, только прямые аудиофайлы
    // pub audio_source_is_video: bool,
}

// Структура для хранения пути к кадру и его номера
#[derive(Debug)]
struct FrameInfo {
    path: PathBuf,
    number: u64,
}

// Структура для хранения информации о секунде и ее кадрах
#[derive(Debug)]
struct SecondInfo {
    number: u64,
    frames: Vec<FrameInfo>,
}

/// Основная функция воспроизведения анимации
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

    // --- Инициализация аудио ---
    let (_stream, stream_handle) = OutputStream::try_default()
         .map_err(|e| anyhow!("Failed to get default audio output device: {}", e))?;
    // Sink должен оставаться в области видимости, чтобы аудио играло
    let sink = Sink::try_new(&stream_handle)
        .map_err(|e| anyhow!("Failed to create audio sink: {}", e))?;

    if let Some(audio_path) = &options.audio_path {
        println!("Loading audio from: {:?}", audio_path);
        if !audio_path.exists() {
            eprintln!("Warning: Audio file not found: {:?}", audio_path);
        } else {
            let file = BufReader::new(File::open(audio_path).with_context(|| format!("Failed to open audio file: {:?}", audio_path))?);
             // Пытаемся декодировать аудио
            match Decoder::new(file) {
                Ok(source) => {
                    // Добавляем источник в Sink. Воспроизведение начнется немедленно.
                    sink.append(source);
                    //sink.play(); // Аудио начнет играть
                    println!("Audio playback started.");
                },
                Err(e) => {
                     eprintln!("Warning: Failed to decode audio file {:?}: {}. Audio will not play.", audio_path, e);
                     // Можно вернуть ошибку, если аудио критично:
                     // return Err(anyhow!("Failed to decode audio file {:?}: {}", audio_path, e));
                }
            }
        }
    } else {
        // Если аудио не указано, можно "приостановить" Sink, чтобы он не занимал ресурсы.
        // sink.pause(); // Не обязательно, но может быть полезно
    }
    // --- Конец инициализации аудио ---


    // --- Подготовка терминала ---
    let mut stdout = stdout();
    // Включаем альтернативный буфер и скрываем курсор (восстановится при выходе)
    stdout.execute(terminal::EnterAlternateScreen)?;
    stdout.execute(cursor::Hide)?;
    // Убедимся, что очистка и скрытие произошли до первого кадра
    stdout.flush()?;

    // --- Цикл воспроизведения ---
    let frame_duration = Duration::from_secs_f64(1.0 / options.fps);
    let mut result: Result<()> = Ok(()); // Для хранения результата внутри `scope`

    // Используем `scope`, чтобы гарантировать восстановление терминала
    // даже если внутри цикла произойдет паника (хотя с anyhow это менее вероятно)
    let scope_result = std::panic::catch_unwind(|| {
        for frame_path in ordered_frames {
            let start_time = Instant::now();

            // Читаем содержимое кадра
            let frame_content = match fs::read_to_string(&frame_path) {
                 Ok(content) => content,
                 Err(e) => {
                     // Log error and potentially break or continue
                     eprintln!( "\nError reading frame file {:?}: {}. Stopping playback.", frame_path, e);
                     result = Err(anyhow!("Failed to read frame: {:?}", frame_path).context(e));
                     break; // Выходим из цикла при ошибке чтения кадра
                 }
            };


            // Очищаем экран и перемещаем курсор в начало
            // Игнорируем ошибки терминала во время воспроизведения, чтобы не прерывать из-за мелочей
            let _ = execute!(
                stdout,
                terminal::Clear(terminal::ClearType::All),
                cursor::MoveTo(0, 0),
            );

            // Печатаем кадр
            // Игнорируем ошибки печати, чтобы не прерывать
            let _ = write!(stdout, "{}", frame_content);

            // Убедимся, что кадр отобразился
            let _ = stdout.flush();


            // Рассчитываем время ожидания
            let elapsed = start_time.elapsed();
            let sleep_duration = frame_duration.saturating_sub(elapsed);

            // Спим до следующего кадра
            thread::sleep(sleep_duration);

             // Проверяем, играет ли еще аудио (если оно вообще было)
             // Если аудио остановилось (закончилось), а кадры еще есть, можно решить, что делать
             // if options.audio_path.is_some() && sink.empty() {
             //     println!("\nAudio finished before video. Stopping playback.");
             //     break; // Останавливаем видео, если аудио закончилось
             // }
        }
        Ok(()) // Возвращаем Ok, если цикл завершился нормально
    });

    // --- Восстановление терминала ---
    // Восстанавливаем курсор и выходим из альтернативного буфера
    // Делаем это *после* цикла, даже если была ошибка или паника
    let _ = stdout.execute(cursor::Show);
    let _ = stdout.execute(terminal::LeaveAlternateScreen);
    // Еще раз flush на всякий случай
    let _ = stdout.flush();


    // Останавливаем аудио явно (хотя оно и так остановится при выходе sink из области видимости)
    sink.stop();
    println!("\nPlayback finished.");

    // Обрабатываем результат из catch_unwind и из цикла
    match scope_result {
        Ok(cycle_result) => cycle_result.and(result), // Возвращаем ошибку, если она была в цикле
        Err(panic_payload) => {
            // Если была паника, пытаемся ее обработать
            eprintln!("\nPlayback panicked!");
            std::panic::resume_unwind(panic_payload); // Передаем панику дальше
        }
    }
}


/// Находит и сортирует файлы кадров в директории
fn discover_and_sort_frames(base_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut seconds: Vec<SecondInfo> = Vec::new();

    for entry_res in fs::read_dir(base_dir).with_context(|| format!("Failed to read base directory: {:?}", base_dir))? {
        let entry = entry_res?;
        let path = entry.path();

        if path.is_dir() {
            // Пытаемся получить имя директории (секунды) и распарсить его как число
            if let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) {
                if let Ok(second_num) = dir_name.parse::<u64>() {
                    let mut current_second = SecondInfo {
                        number: second_num,
                        frames: Vec::new(),
                    };

                    // Сканируем файлы внутри директории секунды
                    for frame_entry_res in fs::read_dir(&path).with_context(|| format!("Failed to read second directory: {:?}", path))? {
                        let frame_entry = frame_entry_res?;
                        let frame_path = frame_entry.path();

                        // Проверяем, что это файл с расширением .txt
                        if frame_path.is_file() && frame_path.extension().map_or(false, |ext| ext == "txt") {
                            // Пытаемся получить имя файла без расширения и распарсить его как номер кадра
                            if let Some(frame_stem) = frame_path.file_stem().and_then(|s| s.to_str()) {
                                if let Ok(frame_num) = frame_stem.parse::<u64>() {
                                    current_second.frames.push(FrameInfo {
                                        path: frame_path,
                                        number: frame_num,
                                    });
                                } else {
                                    eprintln!("Warning: Could not parse frame number from file name: {:?}", frame_path);
                                }
                            }
                        }
                    } // Конец цикла по файлам в директории секунды

                    // Сортируем кадры внутри секунды по их номеру
                    current_second.frames.sort_by_key(|f| f.number);

                    // Добавляем информацию о секунде, если в ней есть кадры
                    if !current_second.frames.is_empty() {
                        seconds.push(current_second);
                    }

                } else {
                    // Игнорируем директории, имя которых не парсится как номер секунды
                    eprintln!("Warning: Directory name is not a valid second number: {:?}", path);
                }
            }
        } // Конец if path.is_dir()
    } // Конец цикла по элементам в базовой директории

    // Сортируем секунды по их номеру
    seconds.sort_by_key(|s| s.number);

    // Собираем все пути к кадрам в один упорядоченный вектор
    let ordered_frame_paths: Vec<PathBuf> = seconds
        .into_iter()
        .flat_map(|s| s.frames.into_iter().map(|f| f.path))
        .collect();

    Ok(ordered_frame_paths)
}
