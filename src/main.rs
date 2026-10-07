#![cfg_attr(not(test), windows_subsystem = "windows")]

use arboard::Clipboard;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::StreamConfig;
use eframe::egui;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use rdev::{simulate, EventType, Key};
use rfd;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Serialize, Deserialize, Clone)]
struct HistoryEntry {
    text: String,
    timestamp: u64,
    app: String,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
struct WordReplacement {
    from: String,
    to: String,
}

fn parse_word_replacements(raw: &str) -> Vec<WordReplacement> {
    raw.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let parts: Vec<&str> = line.splitn(2, '=').collect();
            if parts.len() == 2 {
                let from = parts[0].trim().to_string();
                let to = parts[1].trim().to_string();
                if !from.is_empty() {
                    return Some(WordReplacement { from, to });
                }
            }
            None
        })
        .collect()
}

fn apply_word_replacements(text: &str, replacements: &[WordReplacement]) -> String {
    let mut result = text.to_string();
    for repl in replacements {
        result = result.replace(&repl.from, &repl.to);
    }
    result
}

const HISTORY_FILE: &str = "history.jsonl";
const HISTORY_MAX_ENTRIES: usize = 2000;

fn unix_now() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn load_history() -> Vec<HistoryEntry> {
    let Ok(file) = std::fs::File::open(HISTORY_FILE) else {
        return Vec::new();
    };
    std::io::BufReader::new(file)
        .lines()
        .filter_map(|line| line.ok())
        .filter_map(|line| serde_json::from_str::<HistoryEntry>(&line).ok())
        .collect()
}

fn append_history_entry(entry: &HistoryEntry) {
    if let Ok(json) = serde_json::to_string(entry) {
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(HISTORY_FILE)
        {
            let _ = writeln!(file, "{}", json);
        }
    }
}

fn save_history_all(entries: &[HistoryEntry]) {
    if let Ok(mut file) = std::fs::File::create(HISTORY_FILE) {
        for entry in entries {
            if let Ok(json) = serde_json::to_string(entry) {
                let _ = writeln!(file, "{}", json);
            }
        }
    }
}

fn export_history(entries: &[HistoryEntry]) -> std::io::Result<String> {
    let filename = format!("export_{}.jsonl", unix_now());
    let mut file = std::fs::File::create(&filename)?;
    for entry in entries {
        if let Ok(json) = serde_json::to_string(entry) {
            writeln!(file, "{}", json)?;
        }
    }
    Ok(filename)
}

struct AppState {
    pub is_recording: AtomicBool,
    pub status: Mutex<String>,
    pub history: Mutex<Vec<HistoryEntry>>,
    pub model_path: Mutex<String>,
    pub filler_words: Mutex<String>,
    pub word_replacements: Mutex<String>,
    pub whisper_ctx: Mutex<Option<WhisperContext>>,
    pub mic_gain: Mutex<f32>,
}

#[derive(PartialEq)]
enum AppTab {
    Main,
    Settings,
}

fn record_audio_until_stopped(state: Arc<AppState>) -> Vec<f32> {
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

    let processed_samples = process_audio_samples(&mono_samples, native_sample_rate, *state.mic_gain.lock().unwrap());
    processed_samples
}

fn process_audio_samples(samples: &[f32], sample_rate: u32, gain: f32) -> Vec<f32> {
    const TARGET_RATE: u32 = 16000;
    const NOISE_GATE_THRESHOLD: f32 = 0.005; // Lowered threshold

    let mut resampled = if sample_rate != TARGET_RATE {
        resample_linear(samples, sample_rate, TARGET_RATE)
    } else {
        samples.to_vec()
    };

    // Apply gain early to boost signal before noise gate
    if gain > 1.0 {
        for s in resampled.iter_mut() {
            *s *= gain;
        }
    }

    // Adaptive noise gate based on signal level
    let rms = calculate_rms(&resampled);
    let adaptive_threshold = (NOISE_GATE_THRESHOLD * (1.0 + rms * 2.0)).min(0.02);
    apply_noise_gate(&mut resampled, adaptive_threshold);

    // Aggressive normalization for whisper
    normalize_audio_aggressive(&mut resampled);

    resampled
}

fn resample_linear(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate {
        return input.to_vec();
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let out_len = (input.len() as f64 / ratio).ceil() as usize;
    let mut output = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_pos = i as f64 * ratio;
        let idx = src_pos as usize;
        let frac = src_pos - idx as f64;
        if idx + 1 < input.len() {
            let v = input[idx] * (1.0 - frac as f32) + input[idx + 1] * frac as f32;
            output.push(v);
        } else if idx < input.len() {
            output.push(input[idx]);
        }
    }
    output
}

fn calculate_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|&s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

fn apply_noise_gate(samples: &mut [f32], threshold: f32) {
    for s in samples.iter_mut() {
        if s.abs() < threshold {
            *s = 0.0;
        }
    }
}

fn normalize_audio_aggressive(samples: &mut [f32]) {
    let max_abs = samples.iter().map(|&s| s.abs()).fold(0.0f32, f32::max);
    if max_abs > 0.0 {
        // Target level for whisper (it works best with signals around -20dB to -6dB)
        let target_level = 0.5; // -6dB
        let gain = target_level / max_abs;
        // Cap maximum gain to avoid amplifying noise too much
        let gain = gain.clamp(0.5, 20.0);
        for s in samples.iter_mut() {
            *s *= gain;
        }
    }
}

fn process_audio_pipeline(
    state: &Arc<AppState>,
    audio_samples: &[f32],
    filler_words: &[String],
    word_replacements: &[WordReplacement],
) -> Result<String, Box<dyn std::error::Error>> {
    let ctx_guard = state.whisper_ctx.lock().unwrap();
    let ctx = match ctx_guard.as_ref() {
        Some(ctx) => ctx,
        None => return Err("Модель не загружена".into()),
    };

    if audio_samples.is_empty() {
        return Ok(String::new());
    }

    let mut whisper_state = ctx.create_state()?;
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_language(Some("ru"));
    params.set_translate(false);
    params.set_no_timestamps(true);
    params.set_single_segment(true);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);

    whisper_state.full(params, audio_samples)?;

    let num_segments = whisper_state.full_n_segments();
    let mut raw_text = String::new();
    for i in 0..num_segments {
        if let Some(segment) = whisper_state.get_segment(i) {
            raw_text.push_str(&segment.to_string());
        }
    }

    let raw_text = raw_text.trim().to_string();
    if raw_text.is_empty() {
        return Ok(String::new());
    }

    let cleaned_text = clean_text_locally(&raw_text, filler_words);
    let final_text = apply_word_replacements(&cleaned_text, word_replacements);
    Ok(final_text)
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

    let mut clipboard = match Clipboard::new() {
        Ok(c) => c,
        Err(_) => return,
    };

    let previous_clipboard = clipboard.get_text().ok();

    let _ = clipboard.set_text(text.to_string());
    thread::sleep(Duration::from_millis(150));

    let _ = simulate(&EventType::KeyPress(Key::ControlLeft));
    let _ = simulate(&EventType::KeyPress(Key::KeyV));
    thread::sleep(Duration::from_millis(20));
    let _ = simulate(&EventType::KeyRelease(Key::KeyV));
    let _ = simulate(&EventType::KeyRelease(Key::ControlLeft));

    thread::sleep(Duration::from_millis(100));
    if let Some(prev) = previous_clipboard {
        let _ = clipboard.set_text(prev);
    }
}

use reqwest::blocking;

const MODEL_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin";

fn ensure_model_exists(model_path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    if model_path.exists() {
        return Ok(());
    }

    // Create parent directory if needed
    if let Some(parent) = model_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    eprintln!("Модель не найдена, скачивание с Hugging Face...");
    eprintln!("URL: {}", MODEL_URL);
    eprintln!("Куда: {}", model_path.display());

    let client = blocking::Client::new();
    let mut response = client.get(MODEL_URL).send()?;

    if !response.status().is_success() {
        return Err(format!("Ошибка скачивания: HTTP {}", response.status()).into());
    }

    let total_size = response.content_length().unwrap_or(0);
    let mut downloaded: u64 = 0;
    let mut file = std::fs::File::create(model_path)?;

    const CHUNK_SIZE: usize = 1024 * 1024; // 1MB chunks
    let mut buffer = vec![0u8; CHUNK_SIZE];

    loop {
        let bytes_read = std::io::Read::read(&mut response, &mut buffer)?;
        if bytes_read == 0 {
            break; // EOF
        }
        std::io::Write::write_all(&mut file, &buffer[..bytes_read])?;
        downloaded += bytes_read as u64;
        if total_size > 0 {
            let percent = (downloaded as f64 / total_size as f64 * 100.0) as u32;
            eprint!("\rПрогресс: {}% ({:.1}/{:.1} MB)", percent, downloaded as f64 / 1_048_576.0, total_size as f64 / 1_048_576.0);
        }
    }
    eprintln!("\nМодель скачана успешно!");
    Ok(())
}

struct VoiceGUI {
    state: Arc<AppState>,
    active_tab: AppTab,
    show_widget: Arc<AtomicBool>,
    search_query: String,
    dark_mode: bool,
}

impl VoiceGUI {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        dotenvy::dotenv().ok();
        let model_path = std::env::var("WHISPER_MODEL_PATH")
            .unwrap_or_else(|_| "models/ggml-large-v3-turbo.bin".to_string());
        let model_path_buf = std::path::PathBuf::from(&model_path);

        // Download model if missing
        if let Err(e) = ensure_model_exists(&model_path_buf) {
            eprintln!("Ошибка скачивания модели: {}", e);
        }

        let filler_words = std::env::var("FILLER_WORDS")
            .unwrap_or_else(|_| "эм,ну,как бы,типа,короче".to_string());
        let word_replacements = std::env::var("WORD_REPLACEMENTS")
            .unwrap_or_else(|_| "дефис=-\nдвоеточие=:\nточка с запятой=;\nоткрывающая скобка=(\nзакрывающая скобка=)".to_string());

        let whisper_ctx = WhisperContext::new_with_params(
            &model_path,
            WhisperContextParameters::default(),
        ).ok();

        if whisper_ctx.is_none() {
            eprintln!("Не удалось загрузить модель: {}", model_path);
        } else {
            println!("Модель загружена успешно: {}", model_path);
        }

        let state = Arc::new(AppState {
            is_recording: AtomicBool::new(false),
            status: Mutex::new("Ожидание (Зажмите F9)".to_string()),
            history: Mutex::new(load_history()),
            model_path: Mutex::new(model_path),
            filler_words: Mutex::new(filler_words),
            word_replacements: Mutex::new(word_replacements),
            whisper_ctx: Mutex::new(whisper_ctx),
            mic_gain: Mutex::new(10.0),
        });

        start_background_workers(state.clone(), cc.egui_ctx.clone());
        start_widget_repaint_loop(cc.egui_ctx.clone());

        Self {
            state,
            active_tab: AppTab::Main,
            show_widget: Arc::new(AtomicBool::new(false)),
            search_query: String::new(),
            dark_mode: true,
        }
    }
}

fn start_widget_repaint_loop(ctx: egui::Context) {
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_millis(100));
            ctx.request_repaint();
        }
    });
}

impl eframe::App for VoiceGUI {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut style = (*ctx.style()).clone();
        style.visuals.window_fill = egui::Color32::from_rgb(28, 28, 30);
        style.visuals.panel_fill = egui::Color32::from_rgb(32, 32, 36);
        style.visuals.extreme_bg_color = egui::Color32::from_rgb(40, 40, 44);
        style.visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(40, 40, 44);
        style.visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(48, 48, 52);
        style.visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(60, 60, 68);
        style.visuals.widgets.active.bg_fill = egui::Color32::from_rgb(70, 130, 180);
        style.visuals.selection.bg_fill = egui::Color32::from_rgb(70, 130, 180);
        style.visuals.widgets.inactive.fg_stroke.color = egui::Color32::from_rgb(220, 220, 220);
        style.visuals.widgets.hovered.fg_stroke.color = egui::Color32::WHITE;
        style.visuals.widgets.active.fg_stroke.color = egui::Color32::WHITE;
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(12.0, 6.0);
        style.text_styles.insert(
            egui::TextStyle::Heading,
            egui::FontId::new(20.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Body,
            egui::FontId::new(14.0, egui::FontFamily::Proportional),
        );
        ctx.set_style(style);

        // Применяем тему
        if self.dark_mode {
            ctx.set_visuals(egui::Visuals::dark());
        } else {
            ctx.set_visuals(egui::Visuals::light());
        }

        egui::SidePanel::left("side_panel")
            .resizable(false)
            .default_width(180.0)
            .show(ctx, |ui| {
                ui.add_space(20.0);
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new("🎙 VoiceAI").size(22.0).strong().color(egui::Color32::from_rgb(100, 180, 255)));
                });
                ui.add_space(20.0);
                ui.separator();
                ui.add_space(10.0);

                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 8.0;
                    let main_btn = ui.selectable_label(self.active_tab == AppTab::Main, egui::RichText::new("🏠  Главная").size(15.0));
                    if main_btn.clicked() {
                        self.active_tab = AppTab::Main;
                    }
                    let settings_btn = ui.selectable_label(self.active_tab == AppTab::Settings, egui::RichText::new("⚙  Настройки").size(15.0));
                    if settings_btn.clicked() {
                        self.active_tab = AppTab::Settings;
                    }
                });

                ui.add_space(20.0);
                ui.separator();
                ui.add_space(10.0);

                ui.vertical(|ui| {
                    ui.label(egui::RichText::new("Горячие клавиши:").size(11.0).color(egui::Color32::GRAY));
                    ui.label(egui::RichText::new("F9 — Запись (зажмите)").size(11.0).color(egui::Color32::GRAY));
                    ui.label(egui::RichText::new("F12 — Выход").size(11.0).color(egui::Color32::GRAY));
                });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(10.0);

            match self.active_tab {
                AppTab::Main => {
                    let status = self.state.status.lock().unwrap().clone();
                    ui.horizontal(|ui| {
                        ui.add_space(15.0);
                        ui.label(egui::RichText::new("Статус:").strong().size(15.0));
                        let color = if status.contains("Запись") {
                            egui::Color32::from_rgb(255, 80, 80)
                        } else if status.contains("Обработка") {
                            egui::Color32::from_rgb(255, 200, 80)
                        } else {
                            egui::Color32::from_rgb(80, 255, 120)
                        };
                        ui.label(egui::RichText::new(status).color(color).size(14.0));
                    });

                    ui.add_space(15.0);
                    ui.horizontal(|ui| {
                        ui.add_space(15.0);
                        let mut show_w = self.show_widget.load(Ordering::Relaxed);
                        if ui.checkbox(&mut show_w, "🪟 Мини-виджет поверх окон").changed() {
                            self.show_widget.store(show_w, Ordering::Relaxed);
                        }
                    });

                    ui.add_space(15.0);
                    ui.separator();
                    ui.add_space(10.0);

                    ui.horizontal(|ui| {
                        ui.add_space(15.0);
                        ui.label(egui::RichText::new("История:").strong().size(15.0));
                        ui.add(egui::TextEdit::singleline(&mut self.search_query)
                            .hint_text("🔍 Поиск...")
                            .desired_width(200.0));
                        if ui.button("🗑 Очистить").clicked() {
                            self.state.history.lock().unwrap().clear();
                            save_history_all(&[]);
                        }
                        if ui.button("💾 Экспорт").clicked() {
                            let history = self.state.history.lock().unwrap();
                            let _ = export_history(&history);
                        }
                    });

                    ui.add_space(10.0);

                    egui::ScrollArea::vertical()
                        .stick_to_bottom(true)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.add_space(5.0);
                            let mut history = self.state.history.lock().unwrap();
                            let query = self.search_query.to_lowercase();
                            let mut to_delete: Option<usize> = None;

                            for (i, entry) in history.iter().enumerate() {
                                if !query.is_empty() && !entry.text.to_lowercase().contains(&query) {
                                    continue;
                                }
                                ui.horizontal(|ui| {
                                    ui.add_space(15.0);
                                    let response = ui.add(egui::Label::new(
                                        egui::RichText::new(&entry.text).size(13.0)
                                    ).sense(egui::Sense::click()));
                                    if response.clicked() {
                                        let mut clipboard = Clipboard::new().ok();
                                        if let Some(cb) = clipboard.as_mut() {
                                            let _ = cb.set_text(entry.text.clone());
                                        }
                                    }
                                    if response.hovered() {
                                        ui.ctx().output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
                                    }
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if ui.small_button("✕").on_hover_text("Удалить").clicked() {
                                            to_delete = Some(i);
                                        }
                                        if ui.small_button("📋").on_hover_text("Копировать").clicked() {
                                            let mut clipboard = Clipboard::new().ok();
                                            if let Some(cb) = clipboard.as_mut() {
                                                let _ = cb.set_text(entry.text.clone());
                                            }
                                        }
                                    });
                                });
                                ui.add_space(2.0);
                            }

                            if let Some(i) = to_delete {
                                history.remove(i);
                                save_history_all(&history);
                            }
                        });
                }
                AppTab::Settings => {
                    ui.add_space(10.0);
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.add_space(10.0);

                        // Тема
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            ui.label(egui::RichText::new("🎨 Тема интерфейса").strong().size(16.0));
                        });
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            let theme_text = if self.dark_mode { "🌙 Тёмная" } else { "☀️ Светлая" };
                            if ui.button(egui::RichText::new(theme_text).size(14.0)).clicked() {
                                self.dark_mode = !self.dark_mode;
                            }
                        });
                        ui.add_space(5.0);
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            ui.label(egui::RichText::new("Требуется перезапуск для полного применения").small().color(egui::Color32::GRAY));
                        });

                        ui.add_space(20.0);
                        ui.separator();
                        ui.add_space(15.0);

                        // Микрофон / Чувствительность
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            ui.label(egui::RichText::new("🎤 Микрофон / Чувствительность").strong().size(16.0));
                        });
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            ui.label(egui::RichText::new("Усиление сигнала (gain):").size(13.0));
                        });
                        ui.add_space(5.0);
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            let mut gain = self.state.mic_gain.lock().unwrap();
                            ui.add(egui::Slider::new(&mut *gain, 1.0..=50.0).text("x").suffix("x").step_by(1.0));
                            ui.label(egui::RichText::new(format!("{:.0}x", *gain)).size(13.0).color(egui::Color32::GRAY));
                        });
                        ui.add_space(5.0);
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            ui.label(egui::RichText::new("💡 Начните с 10-15x. Увеличьте, если тихо; уменьшите, если шумит").small().color(egui::Color32::GRAY));
                        });
                        ui.add_space(5.0);
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            ui.label(egui::RichText::new("🎯 Рекомендуется: говорите на расстоянии 10-30 см от микрофона").small().color(egui::Color32::GRAY));
                        });

                        ui.add_space(20.0);
                        ui.separator();
                        ui.add_space(15.0);

                        // Модель
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            ui.label(egui::RichText::new("🤖 Модель Whisper").strong().size(16.0));
                        });
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            ui.label(egui::RichText::new("Путь к модели (ggml-large-v3-turbo.bin):").size(13.0));
                        });
                        ui.add_space(5.0);
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            let mut model_path = self.state.model_path.lock().unwrap();
                            ui.add(
                                egui::TextEdit::singleline(&mut *model_path)
                                    .desired_width(400.0)
                                    .hint_text("models/ggml-large-v3-turbo.bin")
                            );
                            if ui.button("📁 Обзор").clicked() {
                                if let Some(path) = rfd::FileDialog::new()
                                    .add_filter("GGML Model", &["bin"])
                                    .pick_file()
                                {
                                    *model_path = path.to_string_lossy().to_string();
                                }
                            }
                        });
                        ui.add_space(5.0);
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            ui.label(egui::RichText::new("Скачать: https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin").small().color(egui::Color32::GRAY));
                        });
                        ui.add_space(5.0);
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            ui.label(egui::RichText::new("⚠️ После смены пути перезапустите приложение").small().color(egui::Color32::YELLOW));
                        });

                        ui.add_space(20.0);
                        ui.separator();
                        ui.add_space(15.0);

                        // Стоп-слова
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            ui.label(egui::RichText::new("🚫 Стоп-слова (удаляются из текста)").strong().size(16.0));
                        });
                        ui.add_space(5.0);
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            ui.label(egui::RichText::new("Разделяйте запятыми без пробелов: эм,ну,как бы,типа,короче").small().color(egui::Color32::GRAY));
                        });
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            let mut filler = self.state.filler_words.lock().unwrap();
                            ui.add(
                                egui::TextEdit::multiline(&mut *filler)
                                    .desired_width(500.0)
                                    .desired_rows(6)
                                    .hint_text("эм,ну,как бы,типа,короче")
                            );
                        });

                        ui.add_space(20.0);
                        ui.separator();
                        ui.add_space(15.0);

                        // Замены слов
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            ui.label(egui::RichText::new("🔄 Замена слов при распознавании").strong().size(16.0));
                        });
                        ui.add_space(5.0);
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            ui.label(egui::RichText::new("Формат: слово=замена (каждое с новой строки, # для комментариев)").small().color(egui::Color32::GRAY));
                        });
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            let mut replacements = self.state.word_replacements.lock().unwrap();
                            ui.add(
                                egui::TextEdit::multiline(&mut *replacements)
                                    .desired_width(500.0)
                                    .desired_rows(10)
                                    .hint_text("дефис=-\nдвоеточие=:\nточка с запятой=;\nоткрывающая скобка=(\nзакрывающая скобка=)")
                            );
                        });
                        ui.add_space(5.0);
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            ui.label(egui::RichText::new("Пример: скажете \"дефис\" → вставится \"-\"").small().color(egui::Color32::from_rgb(100, 180, 255)));
                        });
                    });
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
                    .with_inner_size([220.0, 52.0])
                    .with_always_on_top()
                    .with_decorations(false)
                    .with_transparent(true),
                move |ctx, _class| {
                    let mut style = (*ctx.style()).clone();
                    style.visuals.window_fill = egui::Color32::from_rgba_premultiplied(28, 28, 30, 245);
                    style.visuals.panel_fill = egui::Color32::from_rgba_premultiplied(32, 32, 36, 245);
                    style.visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(48, 48, 52);
                    style.visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(60, 60, 68);
                    style.visuals.widgets.active.bg_fill = egui::Color32::from_rgb(70, 130, 180);
                    style.spacing.item_spacing = egui::vec2(10.0, 6.0);
                    style.spacing.button_padding = egui::vec2(8.0, 4.0);
                    ctx.set_style(style);

                    let mut close_interacted = false;

                    let panel_response = egui::CentralPanel::default().show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            ui.add_space(12.0);
                            ui.spacing_mut().item_spacing.x = 10.0;

                            if is_rec {
                                ui.label(egui::RichText::new("●").size(16.0).color(egui::Color32::from_rgb(255, 80, 80)));
                                ui.label(egui::RichText::new("ЗАПИСЬ").color(egui::Color32::from_rgb(255, 80, 80)).strong().size(13.0).monospace());
                            } else if is_proc {
                                ui.label(egui::RichText::new("⚙").size(16.0).color(egui::Color32::from_rgb(255, 200, 80)));
                                ui.label(egui::RichText::new("ОБРАБОТКА").color(egui::Color32::from_rgb(255, 200, 80)).strong().size(13.0).monospace());
                            } else {
                                ui.label(egui::RichText::new("●").size(16.0).color(egui::Color32::from_rgb(80, 255, 120)));
                                ui.label(egui::RichText::new("ГОТОВ (F9)").color(egui::Color32::from_rgb(80, 255, 120)).size(13.0).monospace());
                            }

                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let btn = ui.add_sized([24.0, 24.0], egui::Button::new("✕").frame(false));
                                if btn.clicked() {
                                    show_widget_clone.store(false, Ordering::Relaxed);
                                }
                                if btn.hovered() || btn.is_pointer_button_down_on() {
                                    close_interacted = true;
                                }
                            });
                        });
                    });

                    if panel_response.response.dragged() || (panel_response.response.is_pointer_button_down_on() && !close_interacted) {
                        ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                    }
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
            loop {
                thread::sleep(Duration::from_millis(100));

                if state.is_recording.load(Ordering::SeqCst) {
                    let audio_samples = {
                        let state_clone = state.clone();
                        record_audio_until_stopped(state_clone)
                    };

                    let words_raw = state.filler_words.lock().unwrap().clone();
                    let replacements_raw = state.word_replacements.lock().unwrap().clone();

                    if !audio_samples.is_empty() {
                        let filler_words = parse_filler_words(&words_raw);
                        let word_replacements = parse_word_replacements(&replacements_raw);

                        match process_audio_pipeline(&state, &audio_samples, &filler_words, &word_replacements) {
                            Ok(final_text) => {
                                if !final_text.is_empty() {
                                    paste_text(&final_text);
                                    let entry = HistoryEntry {
                                        text: final_text.clone(),
                                        timestamp: unix_now(),
                                        app: "неизвестно".to_string(),
                                    };
                                    append_history_entry(&entry);
                                    let mut history = state.history.lock().unwrap();
                                    history.push(entry);
                                    if history.len() > HISTORY_MAX_ENTRIES {
                                        history.remove(0);
                                        save_history_all(&history);
                                    }
                                }
                            }
                            Err(e) => {
                                let entry = HistoryEntry {
                                    text: format!("❌ Ошибка: {}", e),
                                    timestamp: unix_now(),
                                    app: "system".to_string(),
                                };
                                append_history_entry(&entry);
                                state.history.lock().unwrap().push(entry);
                            }
                        }
                    }

                    *state.status.lock().unwrap() = "🟢 Ожидание (Зажмите F9)".to_string();
                    ctx.request_repaint();
                }
            }
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