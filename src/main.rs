use arboard::Clipboard;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::StreamConfig;
use flacenc::component::BitRepr;
use flacenc::error::Verify;
use rdev::{simulate, EventType, Key};
use reqwest::multipart;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

static IS_RECORDING: AtomicBool = AtomicBool::new(false);

fn record_audio_until_stopped() -> Vec<u8> {
    let host = cpal::default_host();
    let device = match host.default_input_device() {
        Some(d) => {
            if let Ok(name) = d.name() {
                println!("[🎙️] Микрофон: {}", name);
            }
            d
        }
        None => return Vec::new(),
    };

    let supported_config = match device.default_input_config() {
        Ok(sc) => sc,
        Err(_) => return Vec::new(),
    };

    let native_sample_rate = supported_config.sample_rate().0;
    let native_channels = supported_config.channels() as usize;
    let config: StreamConfig = supported_config.into();

    let audio_data: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let audio_data_clone = audio_data.clone();

    let stream = match device.build_input_stream(
        &config,
        move |data: &[f32], _: &cpal::InputCallbackInfo| {
            if IS_RECORDING.load(Ordering::SeqCst) {
                let mut buffer = audio_data_clone.lock().unwrap();
                buffer.extend_from_slice(data);
            }
        },
        |_| {},
        None,
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };

    if stream.play().is_err() {
        return Vec::new();
    }

    while IS_RECORDING.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(50));
    }

    drop(stream);

    let t_processing = Instant::now();

    let raw_samples = audio_data.lock().unwrap();
    if raw_samples.is_empty() {
        return Vec::new();
    }

    // 1. Моно конвертация
    let mut mono_samples = Vec::new();
    if native_channels > 1 {
        for chunk in raw_samples.chunks(native_channels) {
            let sum: f32 = chunk.iter().sum();
            mono_samples.push(sum / native_channels as f32);
        }
    } else {
        mono_samples = raw_samples.clone();
    }

    // 2. Ресемплинг в 16000 Гц
    let target_sample_rate = 16000;
    let mut resampled_samples = Vec::new();
    if native_sample_rate != target_sample_rate {
        let ratio = native_sample_rate as f64 / target_sample_rate as f64;
        let mut i = 0.0;
        while (i as usize) < mono_samples.len() {
            resampled_samples.push(mono_samples[i as usize]);
            i += ratio;
        }
    } else {
        resampled_samples = mono_samples;
    }

    // 3. Кодирование в FLAC (без потерь, но заметно легче WAV)
    let samples_i32: Vec<i32> = resampled_samples
        .iter()
        .map(|&s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i32)
        .collect();

    let config = flacenc::config::Encoder::default()
        .into_verified()
        .expect("Ошибка конфигурации FLAC-энкодера");

    let source = flacenc::source::MemSource::from_samples(
        &samples_i32,
        1, // mono
        16, // bits per sample
        target_sample_rate as usize,
    );

    let flac_stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
        .expect("Ошибка кодирования FLAC");

    let mut sink = flacenc::bitsink::ByteSink::new();
    flac_stream
        .write(&mut sink)
        .expect("Ошибка записи FLAC-потока");

    let flac_bytes = sink.as_slice().to_vec();

    println!(
        "[⏱️] Только обработка аудио (ресемплинг + FLAC): {:?}",
        t_processing.elapsed()
    );

    flac_bytes
}

async fn process_audio_pipeline(
    client: &reqwest::Client,
    audio_bytes: Vec<u8>,
    api_key: &str,
    filler_words: &[String],
) -> Result<String, Box<dyn std::error::Error>> {
    let t0 = Instant::now();

    let part = multipart::Part::bytes(audio_bytes)
        .file_name("speech.flac")
        .mime_str("audio/flac")?;

    let form = multipart::Form::new()
        .text("model", "whisper-large-v3-turbo")
        .text("language", "ru")
        .part("file", part);

    println!("[⏱️] Подготовка формы: {:?}", t0.elapsed());
    let t1 = Instant::now();

    let res = client
        .post("https://api.groq.com/openai/v1/audio/transcriptions")
        .header("Authorization", format!("Bearer {}", api_key))
        .multipart(form)
        .send()
        .await?
        .json::<Value>()
        .await?;

    println!("[⏱️] Запрос к Whisper: {:?}", t1.elapsed());

    let raw_text = match res["text"].as_str() {
        Some(t) if !t.trim().is_empty() => t.to_string(),
        _ => return Ok(String::new()),
    };

    println!("[🗣️ Распознано]: {}", raw_text);

    let t2 = Instant::now();
    let cleaned_text = clean_text_locally(&raw_text, filler_words);
    println!("[⏱️] Локальная очистка: {:?}", t2.elapsed());

    Ok(cleaned_text)
}

fn clean_text_locally(raw_text: &str, filler_words: &[String]) -> String {
    let mut cleaned = raw_text.to_string();

    for word in filler_words {
        let pattern = format!(" {} ", word);
        let lower = cleaned.to_lowercase();
        while let Some(pos) = lower.find(&pattern.to_lowercase()) {
            cleaned.replace_range(pos..pos + pattern.len(), " ");
            if !cleaned.to_lowercase().contains(&pattern.to_lowercase()) {
                break;
            }
        }
    }

    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");

    let mut result = cleaned.trim().to_string();
    if let Some(first_char) = result.chars().next() {
        result.replace_range(0..first_char.len_utf8(), &first_char.to_uppercase().to_string());
    }
    if !result.is_empty() && !result.ends_with(['.', '!', '?']) {
        result.push('.');
    }

    result
}

fn paste_text(text: &str) {
    if text.is_empty() {
        return;
    }

    if let Ok(mut clipboard) = Clipboard::new() {
        let _ = clipboard.set_text(text.to_string());
    }

    thread::sleep(Duration::from_millis(150));

    let _ = simulate(&EventType::KeyPress(Key::ControlLeft));
    let _ = simulate(&EventType::KeyPress(Key::KeyV));
    thread::sleep(Duration::from_millis(20));
    let _ = simulate(&EventType::KeyRelease(Key::KeyV));
    let _ = simulate(&EventType::KeyRelease(Key::ControlLeft));
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    let api_key = std::env::var("GROQ_API_KEY").expect("GROQ_API_KEY не установлен");
    let filler_words_raw = std::env::var("FILLER_WORDS")
        .unwrap_or_else(|_| "эм,ну,как бы,типа,короче".to_string());
    let filler_words: Vec<String> = filler_words_raw
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();
    let client = reqwest::Client::new();

    println!("==================================================");
    println!(">>> Whisper Flow запущен и ожидает F9 (удерживайте)...");
    println!(">>> F12 — выход");
    println!("==================================================");

    thread::spawn(move || {
        if let Err(e) = rdev::listen(move |event| match event.event_type {
            EventType::KeyPress(Key::F9) => {
                if !IS_RECORDING.load(Ordering::SeqCst) {
                    IS_RECORDING.store(true, Ordering::SeqCst);
                    println!("\n[🎤] Запись начата... Говорите!");
                }
            }
            EventType::KeyRelease(Key::F9) => {
                if IS_RECORDING.load(Ordering::SeqCst) {
                    IS_RECORDING.store(false, Ordering::SeqCst);
                    println!("[🛑] Запись остановлена. Обработка...");
                }
            }
            EventType::KeyPress(Key::F12) => {
                println!("\n[👋] Завершение работы Whisper Flow...");
                std::process::exit(0);
            }
            _ => {}
        }) {
            eprintln!("[❌] Ошибка слушателя клавиатуры: {:?}", e);
        }
    });

    loop {
        tokio::time::sleep(Duration::from_millis(100)).await;

        if IS_RECORDING.load(Ordering::SeqCst) {
            let t_rec = Instant::now();
            let audio_bytes = tokio::task::spawn_blocking(record_audio_until_stopped)
                .await
                .unwrap_or_default();
            println!("[⏱️] Запись (включая удержание клавиши): {:?}", t_rec.elapsed());

            if !audio_bytes.is_empty() && !api_key.is_empty() {
                match process_audio_pipeline(&client, audio_bytes, &api_key, &filler_words).await {
                    Ok(final_text) => {
                        if !final_text.is_empty() {
                            println!("[✨ Готово]: {}", final_text);
                            paste_text(&final_text);
                        } else {
                            println!("[ℹ️] Пустой результат.");
                        }
                    }
                    Err(e) => eprintln!("[❌] Ошибка конвейера: {}", e),
                }
            }
        }
    }
}