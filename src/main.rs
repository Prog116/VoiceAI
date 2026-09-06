#![cfg_attr(not(test), windows_subsystem = "windows")]

use arboard::Clipboard;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::StreamConfig;
use eframe::egui;
use flacenc::component::BitRepr;
use flacenc::error::Verify;
use rdev::{simulate, EventType, Key};
use reqwest::multipart;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

struct AppState {
    pub is_recording: AtomicBool,
    pub status: Mutex<String>,
    pub history: Mutex<Vec<String>>,
    pub api_key: Mutex<String>,
    pub filler_words: Mutex<String>,
}

#[derive(PartialEq)]
enum AppTab {
    Main,
    ApiKey,
    FillerWords,
}

fn record_audio_until_stopped(state: Arc<AppState>) -> Vec<u8> {
    let host = cpal::default_host();
    let device = match host.default_input_device() {
        Some(d) => d,
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
    let state_for_stream = state.clone();

    let stream = match device.build_input_stream(
        &config,
        move |data: &[f32], _: &cpal::InputCallbackInfo| {
            if state_for_stream.is_recording.load(Ordering::SeqCst) {
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

    while state.is_recording.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(50));
    }

    drop(stream);

    let raw_samples = audio_data.lock().unwrap();
    if raw_samples.is_empty() {
        return Vec::new();
    }

    let mut mono_samples = Vec::new();
    if native_channels > 1 {
        for chunk in raw_samples.chunks(native_channels) {
            let sum: f32 = chunk.iter().sum();
            mono_samples.push(sum / native_channels as f32);
        }
    } else {
        mono_samples = raw_samples.clone();
    }

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

    let samples_i32: Vec<i32> = resampled_samples
        .iter()
        .map(|&s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i32)
        .collect();

    let config = flacenc::config::Encoder::default()
        .into_verified()
        .expect("Ошибка конфигурации FLAC-энкодера");

    let source = flacenc::source::MemSource::from_samples(
        &samples_i32,
        1,
        16,
        target_sample_rate as usize,
    );

    let flac_stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
        .expect("Ошибка кодирования FLAC");

    let mut sink = flacenc::bitsink::ByteSink::new();
    flac_stream.write(&mut sink).expect("Ошибка записи FLAC");

    sink.as_slice().to_vec()
}

async fn process_audio_pipeline(
    client: &reqwest::Client,
    audio_bytes: Vec<u8>,
    api_key: &str,
    filler_words: &[String],
) -> Result<String, Box<dyn std::error::Error>> {
    let part = multipart::Part::bytes(audio_bytes)
        .file_name("speech.flac")
        .mime_str("audio/flac")?;

    let form = multipart::Form::new()
        .text("model", "whisper-large-v3-turbo")
        .text("language", "ru")
        .part("file", part);

    let res = client
        .post("https://api.groq.com/openai/v1/audio/transcriptions")
        .header("Authorization", format!("Bearer {}", api_key))
        .multipart(form)
        .send()
        .await?
        .json::<Value>()
        .await?;

    let raw_text = match res["text"].as_str() {
        Some(t) if !t.trim().is_empty() => t.to_string(),
        _ => return Ok(String::new()),
    };

    let cleaned_text = clean_text_locally(&raw_text, filler_words);
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

fn parse_filler_words(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
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

struct VoiceGUI {
    state: Arc<AppState>,
    active_tab: AppTab,
    show_widget: Arc<AtomicBool>,
}

impl VoiceGUI {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        dotenvy::dotenv().ok();
        let api_key = std::env::var("GROQ_API_KEY").unwrap_or_default();
        let filler_words = std::env::var("FILLER_WORDS").unwrap_or_else(|_| "эм,ну,как бы,типа,короче".to_string());

        let state = Arc::new(AppState {
            is_recording: AtomicBool::new(false),
            status: Mutex::new("Ожидание (Зажмите F9)".to_string()),
            history: Mutex::new(Vec::new()),
            api_key: Mutex::new(api_key),
            filler_words: Mutex::new(filler_words),
        });

        start_background_workers(state.clone(), cc.egui_ctx.clone());

        Self {
            state,
            active_tab: AppTab::Main,
            show_widget: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl eframe::App for VoiceGUI {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("🎙 VoiceAI");
            ui.separator();

            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.active_tab, AppTab::Main, "Главная");
                ui.selectable_value(&mut self.active_tab, AppTab::ApiKey, "API Ключ");
                ui.selectable_value(&mut self.active_tab, AppTab::FillerWords, "Слова-паразиты");
            });
            ui.separator();
            ui.add_space(5.0);

            match self.active_tab {
                AppTab::Main => {
                    let status = self.state.status.lock().unwrap().clone();
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Статус:").strong());
                        let color = if status.contains("Запись") {
                            egui::Color32::RED
                        } else if status.contains("Обработка") {
                            egui::Color32::YELLOW
                        } else {
                            egui::Color32::GREEN
                        };
                        ui.label(egui::RichText::new(status).color(color));
                    });

                    ui.add_space(5.0);

                    let mut show_w = self.show_widget.load(Ordering::Relaxed);
                    if ui.checkbox(&mut show_w, "🪟 Включить мини-виджет поверх всех окон").changed() {
                        self.show_widget.store(show_w, Ordering::Relaxed);
                    }

                    ui.add_space(10.0);
                    ui.separator();

                    ui.label(egui::RichText::new("История распознавания:").strong());
                    egui::ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
                        let history = self.state.history.lock().unwrap();
                        for entry in history.iter() {
                            ui.label(entry);
                        }
                    });
                }
                AppTab::ApiKey => {
                    ui.label(egui::RichText::new("Настройка API Ключа (Groq):").strong());
                    ui.add_space(5.0);
                    let mut api_key = self.state.api_key.lock().unwrap();
                    ui.add(egui::TextEdit::singleline(&mut *api_key).password(true).desired_width(f32::INFINITY));
                }
                AppTab::FillerWords => {
                    ui.label(egui::RichText::new("Список слов-паразитов:").strong());
                    ui.label(egui::RichText::new("(через запятую, без пробелов после запятой)").small().color(egui::Color32::GRAY));
                    ui.add_space(5.0);
                    let mut filler = self.state.filler_words.lock().unwrap();
                    ui.add(egui::TextEdit::multiline(&mut *filler).desired_width(f32::INFINITY));
                }
            }
        });

        if self.show_widget.load(Ordering::Relaxed) {
            let status = self.state.status.lock().unwrap().clone();
            let is_rec = status.contains("Запись");
            let is_proc = status.contains("Обработка");
            let show_widget_clone = self.show_widget.clone();

            ctx.show_viewport_immediate(
                egui::ViewportId::from_hash_of("floating_status_widget"),
                egui::ViewportBuilder::default()
                    .with_inner_size([190.0, 42.0])
                    .with_always_on_top()
                    .with_decorations(false)
                    .with_transparent(true),
                |ctx, _class| {
                    let mut style = (*ctx.style()).clone();
                    style.visuals.window_fill = egui::Color32::from_black_alpha(220);
                    ctx.set_style(style);

                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;

                            if is_rec {
                                ui.label("🔴");
                                ui.label(egui::RichText::new("Запись...").color(egui::Color32::RED).strong());
                            } else if is_proc {
                                ui.label("⚙️");
                                ui.label(egui::RichText::new("Обработка...").color(egui::Color32::YELLOW).strong());
                            } else {
                                ui.label("🟢");
                                ui.label(egui::RichText::new("Готов (F9)").color(egui::Color32::GREEN));
                            }

                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button("✖").clicked() {
                                    show_widget_clone.store(false, Ordering::Relaxed);
                                }
                            });
                        });
                    });
                },
            );
        }
    }
}

fn start_background_workers(state: Arc<AppState>, ctx: egui::Context) {
    let state_for_keys = state.clone();
    let ctx_for_keys = ctx.clone();

    thread::spawn(move || {
        if let Err(e) = rdev::listen(move |event| match event.event_type {
            EventType::KeyPress(Key::F9) => {
                if !state_for_keys.is_recording.load(Ordering::SeqCst) {
                    state_for_keys.is_recording.store(true, Ordering::SeqCst);
                    *state_for_keys.status.lock().unwrap() = "🔴 Запись...".to_string();
                    ctx_for_keys.request_repaint();
                }
            }
            EventType::KeyRelease(Key::F9) => {
                if state_for_keys.is_recording.load(Ordering::SeqCst) {
                    state_for_keys.is_recording.store(false, Ordering::SeqCst);
                    *state_for_keys.status.lock().unwrap() = "⚙ Обработка...".to_string();
                    ctx_for_keys.request_repaint();
                }
            }
            EventType::KeyPress(Key::F12) => {
                std::process::exit(0);
            }
            _ => {}
        }) {
            eprintln!("Ошибка слушателя клавиатуры: {:?}", e);
        }
    });

    thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let client = reqwest::Client::new();

            loop {
                tokio::time::sleep(Duration::from_millis(100)).await;

                if state.is_recording.load(Ordering::SeqCst) {
                    let audio_bytes = tokio::task::spawn_blocking({
                        let state_clone = state.clone();
                        move || record_audio_until_stopped(state_clone)
                    })
                    .await
                    .unwrap_or_default();

                    let api_key = state.api_key.lock().unwrap().clone();
                    let words_raw = state.filler_words.lock().unwrap().clone();

                    if !audio_bytes.is_empty() && !api_key.is_empty() {
                        let filler_words = parse_filler_words(&words_raw);

                        match process_audio_pipeline(&client, audio_bytes, &api_key, &filler_words).await {
                            Ok(final_text) => {
                                if !final_text.is_empty() {
                                    paste_text(&final_text);
                                    state.history.lock().unwrap().push(final_text);
                                }
                            }
                            Err(e) => {
                                state.history.lock().unwrap().push(format!("❌ Ошибка: {}", e));
                            }
                        }
                    }

                    *state.status.lock().unwrap() = "🟢 Ожидание (Зажмите F9)".to_string();
                    ctx.request_repaint();
                }
            }
        });
    });
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([450.0, 400.0])
            .with_title("VoiceAI"),
        ..Default::default()
    };

    eframe::run_native(
        "VoiceAI",
        options,
        Box::new(|cc| Box::new(VoiceGUI::new(cc))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_single_filler_word() {
        let filler_words = vec!["эм".to_string()];
        let result = clean_text_locally("привет эм как дела", &filler_words);
        assert_eq!(result, "Привет как дела.");
    }

    #[test]
    fn removes_multiple_filler_words() {
        let filler_words = vec!["эм".to_string(), "ну".to_string()];
        let result = clean_text_locally("привет эм как ну дела", &filler_words);
        assert_eq!(result, "Привет как дела.");
    }

    #[test]
    fn leaves_text_without_filler_words_unchanged_but_punctuated() {
        let filler_words = vec!["эм".to_string()];
        let result = clean_text_locally("привет как дела", &filler_words);
        assert_eq!(result, "Привет как дела.");
    }

    #[test]
    fn does_not_duplicate_punctuation_if_already_present() {
        let filler_words: Vec<String> = vec![];
        let result = clean_text_locally("привет!", &filler_words);
        assert_eq!(result, "Привет!");
    }

    #[test]
    fn empty_input_stays_empty() {
        let filler_words = vec!["эм".to_string()];
        let result = clean_text_locally("", &filler_words);
        assert_eq!(result, "");
    }

    #[test]
    fn parses_comma_separated_filler_words() {
        let result = parse_filler_words("эм,ну,как бы");
        assert_eq!(result, vec!["эм", "ну", "как бы"]);
    }

    #[test]
    fn trims_spaces_around_filler_words() {
        let result = parse_filler_words("эм, ну , как бы");
        assert_eq!(result, vec!["эм", "ну", "как бы"]);
    }

    #[test]
    fn empty_filler_words_string_gives_empty_list() {
        let result = parse_filler_words("");
        assert_eq!(result, Vec::<String>::new());
    }
}