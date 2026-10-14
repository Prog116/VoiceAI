#![cfg_attr(not(test), windows_subsystem = "windows")]

use arboard::Clipboard;
use chrono::{Datelike, Local, NaiveDate, TimeZone};
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

#[link(name = "user32")]
extern "system" {
    fn GetAsyncKeyState(vkey: i32) -> i16;
}

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

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Hotkey {
    F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,
    LeftCtrl, RightCtrl, LeftAlt, RightAlt, LeftShift, RightShift,
    Space, Enter, Tab, Backspace, Delete, Insert,
    Home, End, PageUp, PageDown,
    Up, Down, Left, Right,
    Num0, Num1, Num2, Num3, Num4, Num5, Num6, Num7, Num8, Num9,
    A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P, Q, R, S, T, U, V, W, X, Y, Z,
}

impl Hotkey {
    pub fn all() -> &'static [Hotkey] {
        &[
            Hotkey::F1, Hotkey::F2, Hotkey::F3, Hotkey::F4, Hotkey::F5, Hotkey::F6,
            Hotkey::F7, Hotkey::F8, Hotkey::F9, Hotkey::F10, Hotkey::F11, Hotkey::F12,
            Hotkey::LeftCtrl, Hotkey::RightCtrl, Hotkey::LeftAlt, Hotkey::RightAlt,
            Hotkey::LeftShift, Hotkey::RightShift,
            Hotkey::Space, Hotkey::Enter, Hotkey::Tab, Hotkey::Backspace, Hotkey::Delete, Hotkey::Insert,
            Hotkey::Home, Hotkey::End, Hotkey::PageUp, Hotkey::PageDown,
            Hotkey::Up, Hotkey::Down, Hotkey::Left, Hotkey::Right,
            Hotkey::Num0, Hotkey::Num1, Hotkey::Num2, Hotkey::Num3, Hotkey::Num4,
            Hotkey::Num5, Hotkey::Num6, Hotkey::Num7, Hotkey::Num8, Hotkey::Num9,
            Hotkey::A, Hotkey::B, Hotkey::C, Hotkey::D, Hotkey::E, Hotkey::F, Hotkey::G,
            Hotkey::H, Hotkey::I, Hotkey::J, Hotkey::K, Hotkey::L, Hotkey::M, Hotkey::N,
            Hotkey::O, Hotkey::P, Hotkey::Q, Hotkey::R, Hotkey::S, Hotkey::T, Hotkey::U,
            Hotkey::V, Hotkey::W, Hotkey::X, Hotkey::Y, Hotkey::Z,
        ]
    }

    pub fn to_string(&self) -> &'static str {
        match self {
            Hotkey::F1 => "F1", Hotkey::F2 => "F2", Hotkey::F3 => "F3", Hotkey::F4 => "F4",
            Hotkey::F5 => "F5", Hotkey::F6 => "F6", Hotkey::F7 => "F7", Hotkey::F8 => "F8",
            Hotkey::F9 => "F9", Hotkey::F10 => "F10", Hotkey::F11 => "F11", Hotkey::F12 => "F12",
            Hotkey::LeftCtrl => "L_Ctrl", Hotkey::RightCtrl => "R_Ctrl",
            Hotkey::LeftAlt => "L_Alt", Hotkey::RightAlt => "R_Alt",
            Hotkey::LeftShift => "L_Shift", Hotkey::RightShift => "R_Shift",
            Hotkey::Space => "Space", Hotkey::Enter => "Enter", Hotkey::Tab => "Tab",
            Hotkey::Backspace => "Backspace", Hotkey::Delete => "Del", Hotkey::Insert => "Ins",
            Hotkey::Home => "Home", Hotkey::End => "End", Hotkey::PageUp => "PgUp", Hotkey::PageDown => "PgDn",
            Hotkey::Up => "Up", Hotkey::Down => "Down", Hotkey::Left => "Left", Hotkey::Right => "Right",
            Hotkey::Num0 => "0", Hotkey::Num1 => "1", Hotkey::Num2 => "2", Hotkey::Num3 => "3",
            Hotkey::Num4 => "4", Hotkey::Num5 => "5", Hotkey::Num6 => "6", Hotkey::Num7 => "7",
            Hotkey::Num8 => "8", Hotkey::Num9 => "9",
            Hotkey::A => "A", Hotkey::B => "B", Hotkey::C => "C", Hotkey::D => "D",
            Hotkey::E => "E", Hotkey::F => "F", Hotkey::G => "G", Hotkey::H => "H",
            Hotkey::I => "I", Hotkey::J => "J", Hotkey::K => "K", Hotkey::L => "L",
            Hotkey::M => "M", Hotkey::N => "N", Hotkey::O => "O", Hotkey::P => "P",
            Hotkey::Q => "Q", Hotkey::R => "R", Hotkey::S => "S", Hotkey::T => "T",
            Hotkey::U => "U", Hotkey::V => "V", Hotkey::W => "W", Hotkey::X => "X",
            Hotkey::Y => "Y", Hotkey::Z => "Z",
        }
    }

    pub fn to_vk(&self) -> i32 {
        match self {
            Hotkey::F1 => 0x70, Hotkey::F2 => 0x71, Hotkey::F3 => 0x72, Hotkey::F4 => 0x73,
            Hotkey::F5 => 0x74, Hotkey::F6 => 0x75, Hotkey::F7 => 0x76, Hotkey::F8 => 0x77,
            Hotkey::F9 => 0x78, Hotkey::F10 => 0x79, Hotkey::F11 => 0x7A, Hotkey::F12 => 0x7B,
            Hotkey::LeftCtrl => 0xA2, Hotkey::RightCtrl => 0xA3,
            Hotkey::LeftAlt => 0xA4, Hotkey::RightAlt => 0xA5,
            Hotkey::LeftShift => 0xA0, Hotkey::RightShift => 0xA1,
            Hotkey::Space => 0x20, Hotkey::Enter => 0x0D, Hotkey::Tab => 0x09,
            Hotkey::Backspace => 0x08, Hotkey::Delete => 0x2E, Hotkey::Insert => 0x2D,
            Hotkey::Home => 0x24, Hotkey::End => 0x23, Hotkey::PageUp => 0x21, Hotkey::PageDown => 0x22,
            Hotkey::Up => 0x26, Hotkey::Down => 0x28, Hotkey::Left => 0x25, Hotkey::Right => 0x27,
            Hotkey::Num0 => 0x30, Hotkey::Num1 => 0x31, Hotkey::Num2 => 0x32, Hotkey::Num3 => 0x33,
            Hotkey::Num4 => 0x34, Hotkey::Num5 => 0x35, Hotkey::Num6 => 0x36, Hotkey::Num7 => 0x37,
            Hotkey::Num8 => 0x38, Hotkey::Num9 => 0x39,
            Hotkey::A => 0x41, Hotkey::B => 0x42, Hotkey::C => 0x43, Hotkey::D => 0x44,
            Hotkey::E => 0x45, Hotkey::F => 0x46, Hotkey::G => 0x47, Hotkey::H => 0x48,
            Hotkey::I => 0x49, Hotkey::J => 0x4A, Hotkey::K => 0x4B, Hotkey::L => 0x4C,
            Hotkey::M => 0x4D, Hotkey::N => 0x4E, Hotkey::O => 0x4F, Hotkey::P => 0x50,
            Hotkey::Q => 0x51, Hotkey::R => 0x52, Hotkey::S => 0x53, Hotkey::T => 0x54,
            Hotkey::U => 0x55, Hotkey::V => 0x56, Hotkey::W => 0x57, Hotkey::X => 0x58,
            Hotkey::Y => 0x59, Hotkey::Z => 0x5A,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ModifierKey {
    Ctrl,
    Alt,
    Shift,
}

impl ModifierKey {
    pub fn to_vk(&self) -> i32 {
        match self {
            ModifierKey::Ctrl => 0x11,
            ModifierKey::Alt => 0x12,
            ModifierKey::Shift => 0x10,
        }
    }

    pub fn to_string(&self) -> &'static str {
        match self {
            ModifierKey::Ctrl => "Ctrl",
            ModifierKey::Alt => "Alt",
            ModifierKey::Shift => "Shift",
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct HotkeyBinding {
    pub modifiers: Vec<ModifierKey>,
    pub key: Hotkey,
}

impl HotkeyBinding {
    pub fn new(key: Hotkey) -> Self {
        HotkeyBinding {
            modifiers: Vec::new(),
            key,
        }
    }

    pub fn from_string(s: &str) -> Self {
        let mut modifiers: Vec<ModifierKey> = Vec::new();
        let mut key = Hotkey::F9;
        for part in s.split('+') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            match part.to_uppercase().as_str() {
                "CTRL" | "CONTROL" => modifiers.push(ModifierKey::Ctrl),
                "ALT" => modifiers.push(ModifierKey::Alt),
                "SHIFT" => modifiers.push(ModifierKey::Shift),
                _ => key = hotkey_from_string(part),
            }
        }
        modifiers.dedup();
        HotkeyBinding { modifiers, key }
    }

    pub fn is_pressed(&self) -> bool {
        for m in &self.modifiers {
            if (unsafe { GetAsyncKeyState(m.to_vk()) } as u16 & 0x8000) == 0 {
                return false;
            }
        }
        (unsafe { GetAsyncKeyState(self.key.to_vk()) } as u16 & 0x8000) != 0
    }
}

impl std::fmt::Display for HotkeyBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts: Vec<String> = self
            .modifiers
            .iter()
            .map(|m| m.to_string().to_string())
            .collect();
        parts.push(self.key.to_string().to_string());
        write!(f, "{}", parts.join("+"))
    }
}

fn egui_key_to_hotkey(key: egui::Key) -> Option<Hotkey> {
    match key {
        egui::Key::F1 => Some(Hotkey::F1), egui::Key::F2 => Some(Hotkey::F2),
        egui::Key::F3 => Some(Hotkey::F3), egui::Key::F4 => Some(Hotkey::F4),
        egui::Key::F5 => Some(Hotkey::F5), egui::Key::F6 => Some(Hotkey::F6),
        egui::Key::F7 => Some(Hotkey::F7), egui::Key::F8 => Some(Hotkey::F8),
        egui::Key::F9 => Some(Hotkey::F9), egui::Key::F10 => Some(Hotkey::F10),
        egui::Key::F11 => Some(Hotkey::F11), egui::Key::F12 => Some(Hotkey::F12),
        egui::Key::Space => Some(Hotkey::Space),
        egui::Key::Enter => Some(Hotkey::Enter),
        egui::Key::Tab => Some(Hotkey::Tab),
        egui::Key::Backspace => Some(Hotkey::Backspace),
        egui::Key::Delete => Some(Hotkey::Delete),
        egui::Key::Insert => Some(Hotkey::Insert),
        egui::Key::Home => Some(Hotkey::Home),
        egui::Key::End => Some(Hotkey::End),
        egui::Key::PageUp => Some(Hotkey::PageUp),
        egui::Key::PageDown => Some(Hotkey::PageDown),
        egui::Key::ArrowUp => Some(Hotkey::Up),
        egui::Key::ArrowDown => Some(Hotkey::Down),
        egui::Key::ArrowLeft => Some(Hotkey::Left),
        egui::Key::ArrowRight => Some(Hotkey::Right),
        egui::Key::Num0 => Some(Hotkey::Num0), egui::Key::Num1 => Some(Hotkey::Num1),
        egui::Key::Num2 => Some(Hotkey::Num2), egui::Key::Num3 => Some(Hotkey::Num3),
        egui::Key::Num4 => Some(Hotkey::Num4), egui::Key::Num5 => Some(Hotkey::Num5),
        egui::Key::Num6 => Some(Hotkey::Num6), egui::Key::Num7 => Some(Hotkey::Num7),
        egui::Key::Num8 => Some(Hotkey::Num8), egui::Key::Num9 => Some(Hotkey::Num9),
        egui::Key::A => Some(Hotkey::A), egui::Key::B => Some(Hotkey::B),
        egui::Key::C => Some(Hotkey::C), egui::Key::D => Some(Hotkey::D),
        egui::Key::E => Some(Hotkey::E), egui::Key::F => Some(Hotkey::F),
        egui::Key::G => Some(Hotkey::G), egui::Key::H => Some(Hotkey::H),
        egui::Key::I => Some(Hotkey::I), egui::Key::J => Some(Hotkey::J),
        egui::Key::K => Some(Hotkey::K), egui::Key::L => Some(Hotkey::L),
        egui::Key::M => Some(Hotkey::M), egui::Key::N => Some(Hotkey::N),
        egui::Key::O => Some(Hotkey::O), egui::Key::P => Some(Hotkey::P),
        egui::Key::Q => Some(Hotkey::Q), egui::Key::R => Some(Hotkey::R),
        egui::Key::S => Some(Hotkey::S), egui::Key::T => Some(Hotkey::T),
        egui::Key::U => Some(Hotkey::U), egui::Key::V => Some(Hotkey::V),
        egui::Key::W => Some(Hotkey::W), egui::Key::X => Some(Hotkey::X),
        egui::Key::Y => Some(Hotkey::Y), egui::Key::Z => Some(Hotkey::Z),
        _ => None,
    }
}

fn hotkey_from_string(s: &str) -> Hotkey {
    match s.trim().to_uppercase().as_str() {
        "F1" => Hotkey::F1, "F2" => Hotkey::F2, "F3" => Hotkey::F3, "F4" => Hotkey::F4,
        "F5" => Hotkey::F5, "F6" => Hotkey::F6, "F7" => Hotkey::F7, "F8" => Hotkey::F8,
        "F9" => Hotkey::F9, "F10" => Hotkey::F10, "F11" => Hotkey::F11, "F12" => Hotkey::F12,
        "L_CTRL" | "LCONTROL" | "LEFTCTRL" => Hotkey::LeftCtrl,
        "R_CTRL" | "RCONTROL" | "RIGHTCTRL" => Hotkey::RightCtrl,
        "L_ALT" | "LALT" | "LEFTALT" => Hotkey::LeftAlt,
        "R_ALT" | "RALT" | "RIGHTALT" | "ALTGR" => Hotkey::RightAlt,
        "L_SHIFT" | "LSHIFT" | "LEFTSHIFT" => Hotkey::LeftShift,
        "R_SHIFT" | "RSHIFT" | "RIGHTSHIFT" => Hotkey::RightShift,
        "SPACE" => Hotkey::Space,
        "ENTER" | "RETURN" => Hotkey::Enter,
        "TAB" => Hotkey::Tab,
        "BACKSPACE" => Hotkey::Backspace,
        "DELETE" | "DEL" => Hotkey::Delete,
        "INSERT" | "INS" => Hotkey::Insert,
        "HOME" => Hotkey::Home,
        "END" => Hotkey::End,
        "PAGEUP" | "PGUP" => Hotkey::PageUp,
        "PAGEDOWN" | "PGDN" => Hotkey::PageDown,
        "UP" | "ARROWUP" => Hotkey::Up,
        "DOWN" | "ARROWDOWN" => Hotkey::Down,
        "LEFT" | "ARROWLEFT" => Hotkey::Left,
        "RIGHT" | "ARROWRIGHT" => Hotkey::Right,
        "0" | "NUM0" => Hotkey::Num0,
        "1" | "NUM1" => Hotkey::Num1,
        "2" | "NUM2" => Hotkey::Num2,
        "3" | "NUM3" => Hotkey::Num3,
        "4" | "NUM4" => Hotkey::Num4,
        "5" | "NUM5" => Hotkey::Num5,
        "6" | "NUM6" => Hotkey::Num6,
        "7" | "NUM7" => Hotkey::Num7,
        "8" | "NUM8" => Hotkey::Num8,
        "9" | "NUM9" => Hotkey::Num9,
        "A" => Hotkey::A, "B" => Hotkey::B, "C" => Hotkey::C, "D" => Hotkey::D,
        "E" => Hotkey::E, "F" => Hotkey::F, "G" => Hotkey::G, "H" => Hotkey::H,
        "I" => Hotkey::I, "J" => Hotkey::J, "K" => Hotkey::K, "L" => Hotkey::L,
        "M" => Hotkey::M, "N" => Hotkey::N, "O" => Hotkey::O, "P" => Hotkey::P,
        "Q" => Hotkey::Q, "R" => Hotkey::R, "S" => Hotkey::S, "T" => Hotkey::T,
        "U" => Hotkey::U, "V" => Hotkey::V, "W" => Hotkey::W, "X" => Hotkey::X,
        "Y" => Hotkey::Y, "Z" => Hotkey::Z,
        _ => Hotkey::F9,
    }
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

fn month_name_ru(month: u32) -> &'static str {
    match month {
        1 => "января",
        2 => "февраля",
        3 => "марта",
        4 => "апреля",
        5 => "мая",
        6 => "июня",
        7 => "июля",
        8 => "августа",
        9 => "сентября",
        10 => "октября",
        11 => "ноября",
        12 => "декабря",
        _ => "",
    }
}

/// Возвращает относительный заголовок группы по числу прошедших дней.
/// `None` означает, что запись старше описанных групп и её нужно
/// подписать конкретной датой.
fn group_label_for_days(days_ago: i64) -> Option<&'static str> {
    if days_ago <= 0 {
        Some("Сегодня")
    } else if days_ago == 1 {
        Some("Вчера")
    } else if days_ago <= 6 {
        Some("На прошлой неделе")
    } else {
        None
    }
}

fn date_group_label(timestamp: u64, today: NaiveDate) -> String {
    let dt = match Local.timestamp_opt(timestamp as i64, 0).single() {
        Some(dt) => dt,
        None => return "Дата неизвестна".to_string(),
    };
    let days_ago = (today - dt.date_naive()).num_days();
    if let Some(label) = group_label_for_days(days_ago) {
        return label.to_string();
    }
    format!("{} {} {}", dt.day(), month_name_ru(dt.month()), dt.year())
}

fn entry_time_label(timestamp: u64) -> String {
    match Local.timestamp_opt(timestamp as i64, 0).single() {
        Some(dt) => dt.format("%H:%M").to_string(),
        None => String::new(),
    }
}

fn parse_mic_gain(raw: &str) -> f32 {
    raw.trim()
        .parse::<f32>()
        .ok()
        .unwrap_or(10.0)
        .clamp(1.0, 50.0)
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

#[derive(Clone, Copy, PartialEq)]
enum ModelPhase {
    Idle,
    Downloading,
    Loading,
    Ready,
    Error,
}

#[derive(Clone)]
struct ModelLoadState {
    phase: ModelPhase,
    downloaded: u64,
    total: u64,
    error: Option<String>,
}

impl Default for ModelLoadState {
    fn default() -> Self {
        Self {
            phase: ModelPhase::Idle,
            downloaded: 0,
            total: 0,
            error: None,
        }
    }
}

struct AppState {
    pub is_recording: AtomicBool,
    pub status: Mutex<String>,
    pub history: Mutex<Vec<HistoryEntry>>,
    pub model_path: Mutex<String>,
    pub filler_words: Mutex<String>,
    pub word_replacements: Mutex<String>,
    pub whisper_ctx: Mutex<Option<WhisperContext>>,
    pub model_status: Mutex<ModelLoadState>,
    pub mic_gain: Mutex<f32>,
    pub hotkey_record: Mutex<HotkeyBinding>,
    pub hotkey_exit: Mutex<HotkeyBinding>,
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

fn download_model(
    model_path: &std::path::Path,
    state: &AppState,
    ctx: &egui::Context,
) -> Result<(), Box<dyn std::error::Error>> {
    // Create parent directory if needed
    if let Some(parent) = model_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let client = blocking::Client::new();
    let mut response = client.get(MODEL_URL).send()?;

    if !response.status().is_success() {
        return Err(format!("Ошибка скачивания: HTTP {}", response.status()).into());
    }

    let total_size = response.content_length().unwrap_or(0);
    {
        let mut st = state.model_status.lock().unwrap();
        st.phase = ModelPhase::Downloading;
        st.downloaded = 0;
        st.total = total_size;
        st.error = None;
    }
    ctx.request_repaint();

    // Пишем во временный файл, а по завершении переименовываем —
    // так оборванное скачивание не оставит «битую» модель.
    let tmp_path = model_path.with_extension("download");
    let mut file = std::fs::File::create(&tmp_path)?;
    let mut downloaded: u64 = 0;

    const CHUNK_SIZE: usize = 1024 * 1024; // 1MB chunks
    let mut buffer = vec![0u8; CHUNK_SIZE];

    loop {
        let bytes_read = std::io::Read::read(&mut response, &mut buffer)?;
        if bytes_read == 0 {
            break; // EOF
        }
        std::io::Write::write_all(&mut file, &buffer[..bytes_read])?;
        downloaded += bytes_read as u64;
        {
            let mut st = state.model_status.lock().unwrap();
            st.downloaded = downloaded;
        }
        ctx.request_repaint();
    }
    drop(file);

    std::fs::rename(&tmp_path, model_path)?;
    Ok(())
}

fn start_model_loader(state: Arc<AppState>, ctx: egui::Context, model_path: std::path::PathBuf) {
    thread::spawn(move || {
        if !model_path.exists() {
            if let Err(e) = download_model(&model_path, &state, &ctx) {
                let mut st = state.model_status.lock().unwrap();
                st.phase = ModelPhase::Error;
                st.error = Some(e.to_string());
                drop(st);
                ctx.request_repaint();
                return;
            }
        }

        {
            let mut st = state.model_status.lock().unwrap();
            st.phase = ModelPhase::Loading;
            drop(st);
        }
        ctx.request_repaint();

        match WhisperContext::new_with_params(
            model_path.to_string_lossy().as_ref(),
            WhisperContextParameters::default(),
        ) {
            Ok(c) => {
                *state.whisper_ctx.lock().unwrap() = Some(c);
                let mut st = state.model_status.lock().unwrap();
                st.phase = ModelPhase::Ready;
                st.error = None;
                drop(st);
            }
            Err(e) => {
                let mut st = state.model_status.lock().unwrap();
                st.phase = ModelPhase::Error;
                st.error = Some(format!("Не удалось загрузить модель: {}", e));
                drop(st);
            }
        }
        ctx.request_repaint();
    });
}

#[derive(PartialEq, Clone, Copy)]
enum HotkeyAction { Record, Exit }

fn configure_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    // egui поставляется с ограниченным набором эмодзи, из-за чего часть
    // символов рисуется «квадратиками». Добавляем системный шрифт символов
    // Windows (содержит таблицу glyf, поэтому egui умеет его растеризовать).
    let candidates = [
        "C:\\Windows\\Fonts\\seguiemj.ttf",
        "C:\\Windows\\Fonts\\seguisym.ttf",
    ];
    if let Some(bytes) = candidates.iter().find_map(|p| std::fs::read(p).ok()) {
        let name = "system_symbols".to_owned();
        fonts
            .font_data
            .insert(name.clone(), egui::FontData::from_owned(bytes));
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .push(name.clone());
        }
    }

    ctx.set_fonts(fonts);
}

struct VoiceGUI {
    state: Arc<AppState>,
    active_tab: AppTab,
    show_widget: Arc<AtomicBool>,
    search_query: String,
    dark_mode: bool,
    waiting_for_hotkey: Option<HotkeyAction>,
    collapsed_groups: std::collections::HashSet<String>,
}

impl VoiceGUI {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        dotenvy::dotenv().ok();
        configure_fonts(&cc.egui_ctx);
        let model_path = std::env::var("WHISPER_MODEL_PATH")
            .unwrap_or_else(|_| "models/ggml-large-v3-turbo.bin".to_string());
        let model_path_buf = std::path::PathBuf::from(&model_path);

        let filler_words = std::env::var("FILLER_WORDS")
            .unwrap_or_else(|_| "эм,ну,как бы,типа,короче".to_string());
        let word_replacements = std::env::var("WORD_REPLACEMENTS")
            .unwrap_or_else(|_| "дефис=-\nдвоеточие=:\nточка с запятой=;\nоткрывающая скобка=(\nзакрывающая скобка=)".to_string());

        // Load hotkeys from .env
        let record_hotkey = std::env::var("HOTKEY_RECORD")
            .map(|s| HotkeyBinding::from_string(&s))
            .unwrap_or_else(|_| HotkeyBinding::new(Hotkey::F9));
        let exit_hotkey = std::env::var("HOTKEY_EXIT")
            .map(|s| HotkeyBinding::from_string(&s))
            .unwrap_or_else(|_| HotkeyBinding::new(Hotkey::F12));

        let mic_gain = std::env::var("MIC_GAIN")
            .map(|s| parse_mic_gain(&s))
            .unwrap_or(10.0);

        let state = Arc::new(AppState {
            is_recording: AtomicBool::new(false),
            status: Mutex::new("Ожидание (Зажмите F9)".to_string()),
            history: Mutex::new(load_history()),
            model_path: Mutex::new(model_path),
            filler_words: Mutex::new(filler_words),
            word_replacements: Mutex::new(word_replacements),
            whisper_ctx: Mutex::new(None),
            model_status: Mutex::new(ModelLoadState::default()),
            mic_gain: Mutex::new(mic_gain),
            hotkey_record: Mutex::new(record_hotkey),
            hotkey_exit: Mutex::new(exit_hotkey),
        });

        // Скачивание (при необходимости) и загрузка модели идут в фоне,
        // чтобы не блокировать запуск интерфейса.
        start_model_loader(state.clone(), cc.egui_ctx.clone(), model_path_buf);

        start_background_workers(state.clone(), cc.egui_ctx.clone());
        start_widget_repaint_loop(cc.egui_ctx.clone());

        Self {
            state,
            active_tab: AppTab::Main,
            show_widget: Arc::new(AtomicBool::new(false)),
            search_query: String::new(),
            dark_mode: true,
            waiting_for_hotkey: None,
            collapsed_groups: std::collections::HashSet::new(),
        }
    }
}

fn encode_env_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '$' => out.push_str("\\$"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

fn upsert_env_line(key: &str, value: &str) {
    let env_path = std::path::Path::new(".env");
    let mut lines: Vec<String> = if env_path.exists() {
        std::fs::read_to_string(env_path)
            .unwrap_or_default()
            .lines()
            .map(|s| s.to_string())
            .collect()
    } else {
        Vec::new()
    };
    let prefix = format!("{}=", key);
    lines.retain(|line| !line.starts_with(&prefix));
    lines.push(format!("{}={}", key, encode_env_value(value)));
    let _ = std::fs::write(env_path, lines.join("\n"));
}

impl VoiceGUI {
    fn save_hotkey_to_env(&self, key_name: &str, binding: &HotkeyBinding) {
        upsert_env_line(key_name, &binding.to_string());
    }

    fn save_filler_words(&self) {
        let value = self.state.filler_words.lock().unwrap().clone();
        upsert_env_line("FILLER_WORDS", &value);
    }

    fn save_word_replacements(&self) {
        let value = self.state.word_replacements.lock().unwrap().clone();
        upsert_env_line("WORD_REPLACEMENTS", &value);
    }

    fn save_mic_gain(&self) {
        let value = *self.state.mic_gain.lock().unwrap();
        upsert_env_line("MIC_GAIN", &format!("{:.0}", value));
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

        // Handle hotkey capture
        if let Some(action) = self.waiting_for_hotkey {
            ctx.input(|i| {
                for event in &i.events {
                    if let egui::Event::Key { key, pressed: true, modifiers, .. } = event {
                        if *key == egui::Key::Escape && modifiers.is_none() {
                            self.waiting_for_hotkey = None;
                            return;
                        }
                        if let Some(hk) = egui_key_to_hotkey(*key) {
                            let mut mods: Vec<ModifierKey> = Vec::new();
                            if modifiers.ctrl {
                                mods.push(ModifierKey::Ctrl);
                            }
                            if modifiers.alt {
                                mods.push(ModifierKey::Alt);
                            }
                            if modifiers.shift {
                                mods.push(ModifierKey::Shift);
                            }
                            let binding = HotkeyBinding { modifiers: mods, key: hk };
                            match action {
                                HotkeyAction::Record => {
                                    *self.state.hotkey_record.lock().unwrap() = binding.clone();
                                    self.save_hotkey_to_env("HOTKEY_RECORD", &binding);
                                }
                                HotkeyAction::Exit => {
                                    *self.state.hotkey_exit.lock().unwrap() = binding.clone();
                                    self.save_hotkey_to_env("HOTKEY_EXIT", &binding);
                                }
                            }
                            self.waiting_for_hotkey = None;
                            return;
                        }
                    }
                }
            });
            ctx.request_repaint();
        }

        // Применяем тема
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
                    let record_hk = self.state.hotkey_record.lock().unwrap().clone();
                    let exit_hk = self.state.hotkey_exit.lock().unwrap().clone();
                    ui.label(egui::RichText::new(format!("{} — Запись (зажмите)", record_hk)).size(11.0).color(egui::Color32::GRAY));
                    ui.label(egui::RichText::new(format!("{} — Выход", exit_hk)).size(11.0).color(egui::Color32::GRAY));
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

                            if history.is_empty() {
                                ui.horizontal(|ui| {
                                    ui.add_space(15.0);
                                    ui.label(
                                        egui::RichText::new("История пуста — запишите первую фразу")
                                            .size(13.0)
                                            .italics()
                                            .color(egui::Color32::GRAY),
                                    );
                                });
                            }

                            // Группируем по дате: заголовки «Сегодня», «Вчера» и т.д.
                            let today = Local::now().date_naive();
                            let filtered: Vec<(usize, String)> = history
                                .iter()
                                .enumerate()
                                .filter(|(_, e)| {
                                    query.is_empty() || e.text.to_lowercase().contains(&query)
                                })
                                .map(|(i, e)| (i, date_group_label(e.timestamp, today)))
                                .collect();

                            if !history.is_empty() && filtered.is_empty() {
                                ui.horizontal(|ui| {
                                    ui.add_space(15.0);
                                    ui.label(
                                        egui::RichText::new("Ничего не найдено")
                                            .size(13.0)
                                            .italics()
                                            .color(egui::Color32::GRAY),
                                    );
                                });
                            }

                            // Количество записей в каждой группе — для бейджа в заголовке.
                            let mut group_counts: Vec<(String, usize)> = Vec::new();
                            for (_, label) in &filtered {
                                match group_counts.last_mut() {
                                    Some(last) if &last.0 == label => last.1 += 1,
                                    _ => group_counts.push((label.clone(), 1)),
                                }
                            }

                            let mut group_index = 0usize;
                            let mut current_group = String::new();
                            let mut current_collapsed = false;
                            let mut toggle_group: Option<String> = None;

                            for (i, label) in &filtered {
                                let i = *i;
                                let entry = &history[i];

                                if *label != current_group {
                                    current_group = label.clone();
                                    current_collapsed =
                                        self.collapsed_groups.contains(&current_group);
                                    let count = group_counts
                                        .get(group_index)
                                        .map(|g| g.1)
                                        .unwrap_or(0);
                                    group_index += 1;

                                    ui.add_space(12.0);
                                    let header = ui.horizontal(|ui| {
                                        ui.add_space(15.0);
                                        let arrow = if current_collapsed { "▶" } else { "▼" };
                                        ui.label(
                                            egui::RichText::new(arrow)
                                                .size(9.0)
                                                .color(egui::Color32::from_rgb(120, 170, 220)),
                                        );
                                        ui.add_space(2.0);
                                        ui.label(
                                            egui::RichText::new(&current_group)
                                                .size(12.0)
                                                .strong()
                                                .color(egui::Color32::from_rgb(120, 170, 220)),
                                        );
                                        ui.add_space(6.0);
                                        ui.label(
                                            egui::RichText::new(count.to_string())
                                                .size(11.0)
                                                .color(egui::Color32::from_rgb(130, 130, 140)),
                                        );
                                    });
                                    let mut header_rect = header.response.rect;
                                    header_rect.max.x = ui.max_rect().right();
                                    let header = ui.interact(
                                        header_rect,
                                        ui.make_persistent_id((
                                            "history_group_toggle",
                                            &current_group,
                                        )),
                                        egui::Sense::click(),
                                    );
                                    if header.clicked() {
                                        toggle_group = Some(current_group.clone());
                                    }
                                    if header.hovered() {
                                        ui.ctx().output_mut(|o| {
                                            o.cursor_icon = egui::CursorIcon::PointingHand
                                        });
                                    }
                                    ui.add_space(3.0);
                                    ui.horizontal(|ui| {
                                        ui.add_space(12.0);
                                        let width = (ui.available_width() - 12.0).max(0.0);
                                        let (rect, _) = ui.allocate_exact_size(
                                            egui::vec2(width, 1.0),
                                            egui::Sense::hover(),
                                        );
                                        ui.painter().rect_filled(
                                            rect,
                                            0.0,
                                            egui::Color32::from_rgb(58, 58, 66),
                                        );
                                    });
                                    ui.add_space(if current_collapsed { 2.0 } else { 6.0 });
                                }

                                if current_collapsed {
                                    continue;
                                }

                                ui.horizontal(|ui| {
                                    ui.add_space(15.0);
                                    ui.label(
                                        egui::RichText::new(entry_time_label(entry.timestamp))
                                            .size(11.0)
                                            .monospace()
                                            .color(egui::Color32::from_rgb(120, 120, 130)),
                                    );
                                    ui.add_space(6.0);
                                    let response = ui.add(
                                        egui::Label::new(egui::RichText::new(&entry.text).size(13.0))
                                            .sense(egui::Sense::click()),
                                    );
                                    if response.clicked() {
                                        let mut clipboard = Clipboard::new().ok();
                                        if let Some(cb) = clipboard.as_mut() {
                                            let _ = cb.set_text(entry.text.clone());
                                        }
                                    }
                                    if response.hovered() {
                                        ui.ctx().output_mut(|o| {
                                            o.cursor_icon = egui::CursorIcon::PointingHand
                                        });
                                    }
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui
                                                .small_button("✕")
                                                .on_hover_text("Удалить")
                                                .clicked()
                                            {
                                                to_delete = Some(i);
                                            }
                                            if ui
                                                .small_button("📋")
                                                .on_hover_text("Копировать")
                                                .clicked()
                                            {
                                                let mut clipboard = Clipboard::new().ok();
                                                if let Some(cb) = clipboard.as_mut() {
                                                    let _ = cb.set_text(entry.text.clone());
                                                }
                                            }
                                        },
                                    );
                                });
                                ui.add_space(4.0);
                            }

                            if let Some(g) = toggle_group {
                                if self.collapsed_groups.contains(&g) {
                                    self.collapsed_groups.remove(&g);
                                } else {
                                    self.collapsed_groups.insert(g);
                                }
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
                            let changed = {
                                let mut gain = self.state.mic_gain.lock().unwrap();
                                let changed = ui
                                    .add(
                                        egui::Slider::new(&mut *gain, 1.0..=50.0)
                                            .text("x")
                                            .suffix("x")
                                            .step_by(1.0),
                                    )
                                    .changed();
                                ui.label(
                                    egui::RichText::new(format!("{:.0}x", *gain))
                                        .size(13.0)
                                        .color(egui::Color32::GRAY),
                                );
                                changed
                            };
                            if changed {
                                self.save_mic_gain();
                            }
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

                        // Горячие клавиши
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            ui.label(egui::RichText::new("⌨ Горячие клавиши").strong().size(16.0));
                        });
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            ui.label(egui::RichText::new("Комбинация записи (зажать):").size(13.0));
                        });
                        ui.add_space(5.0);
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            let record_hk = self.state.hotkey_record.lock().unwrap().clone();
                            
                            ui.label(egui::RichText::new(format!("Текущая: {}", record_hk)).size(13.0).monospace().color(egui::Color32::from_rgb(100, 180, 255)));
                            ui.add_space(10.0);
                            
                            let waiting_record = matches!(self.waiting_for_hotkey, Some(HotkeyAction::Record));
                            let btn_text = if waiting_record { "⌨ Жду нажатия... (ESC для отмены)" } else { "🎮 Назначить комбинацию" };
                            let btn = ui.add_enabled(!waiting_record, egui::Button::new(btn_text)
                                .fill(if waiting_record { egui::Color32::YELLOW } else { egui::Color32::from_rgb(70, 130, 180) })
                                .min_size(egui::vec2(200.0, 30.0)));
                            
                            if btn.clicked() {
                                self.waiting_for_hotkey = Some(HotkeyAction::Record);
                            }
                        });
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            ui.label(egui::RichText::new("Комбинация выхода:").size(13.0));
                        });
                        ui.add_space(5.0);
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            let exit_hk = self.state.hotkey_exit.lock().unwrap().clone();
                            
                            ui.label(egui::RichText::new(format!("Текущая: {}", exit_hk)).size(13.0).monospace().color(egui::Color32::from_rgb(100, 180, 255)));
                            ui.add_space(10.0);
                            
                            let waiting_exit = matches!(self.waiting_for_hotkey, Some(HotkeyAction::Exit));
                            let btn_text = if waiting_exit { "⌨ Жду нажатия... (ESC для отмены)" } else { "🎮 Назначить комбинацию" };
                            let btn = ui.add_enabled(!waiting_exit, egui::Button::new(btn_text)
                                .fill(if waiting_exit { egui::Color32::YELLOW } else { egui::Color32::from_rgb(70, 130, 180) })
                                .min_size(egui::vec2(200.0, 30.0)));
                            
                            if btn.clicked() {
                                self.waiting_for_hotkey = Some(HotkeyAction::Exit);
                            }
                        });
                        ui.add_space(5.0);
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            ui.label(egui::RichText::new("💾 Комбинация сохраняется в .env и применяется сразу").small().color(egui::Color32::GRAY));
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

                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            let ms = self.state.model_status.lock().unwrap().clone();
                            match ms.phase {
                                ModelPhase::Idle => {
                                    ui.label(egui::RichText::new("⬇ Модель ещё не загружена").size(13.0).color(egui::Color32::GRAY));
                                }
                                ModelPhase::Downloading => {
                                    let frac = if ms.total > 0 {
                                        ms.downloaded as f32 / ms.total as f32
                                    } else {
                                        0.0
                                    };
                                    ui.add(
                                        egui::ProgressBar::new(frac)
                                            .desired_width(300.0)
                                            .show_percentage(),
                                    );
                                    let text = if ms.total > 0 {
                                        format!(
                                            "{:.0} / {:.0} МБ",
                                            ms.downloaded as f64 / 1_048_576.0,
                                            ms.total as f64 / 1_048_576.0
                                        )
                                    } else {
                                        format!("{:.0} МБ", ms.downloaded as f64 / 1_048_576.0)
                                    };
                                    ui.label(egui::RichText::new(text).size(12.0).color(egui::Color32::GRAY));
                                }
                                ModelPhase::Loading => {
                                    ui.label(egui::RichText::new("⏳ Загрузка модели в память...").size(13.0).color(egui::Color32::YELLOW));
                                }
                                ModelPhase::Ready => {
                                    ui.label(egui::RichText::new("✅ Модель загружена").size(13.0).color(egui::Color32::from_rgb(80, 255, 120)));
                                }
                                ModelPhase::Error => {
                                    let err = ms.error.clone().unwrap_or_default();
                                    ui.label(egui::RichText::new(format!("❌ {}", err)).size(13.0).color(egui::Color32::from_rgb(255, 80, 80)));
                                }
                            }
                        });
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            let path_str = self.state.model_path.lock().unwrap().clone();
                            let busy = matches!(
                                self.state.model_status.lock().unwrap().phase,
                                ModelPhase::Downloading | ModelPhase::Loading
                            );
                            let btn_text = if path_str.trim().is_empty() {
                                "⬇ Скачать модель"
                            } else {
                                "⬇ Скачать / перезагрузить модель"
                            };
                            let btn = ui.add_enabled(
                                !busy,
                                egui::Button::new(btn_text).min_size(egui::vec2(230.0, 28.0)),
                            );
                            if btn.clicked() {
                                let path_buf = std::path::PathBuf::from(path_str.trim());
                                start_model_loader(self.state.clone(), ui.ctx().clone(), path_buf);
                            }
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
                            let resp = {
                                let mut filler = self.state.filler_words.lock().unwrap();
                                ui.add(
                                    egui::TextEdit::multiline(&mut *filler)
                                        .desired_width(500.0)
                                        .desired_rows(6)
                                        .hint_text("эм,ну,как бы,типа,короче")
                                )
                            };
                            if resp.changed() {
                                self.save_filler_words();
                            }
                        });
                        ui.add_space(5.0);
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            ui.label(egui::RichText::new("💾 Сохраняется автоматически").small().color(egui::Color32::GRAY));
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
                            let resp = {
                                let mut replacements = self.state.word_replacements.lock().unwrap();
                                ui.add(
                                    egui::TextEdit::multiline(&mut *replacements)
                                        .desired_width(500.0)
                                        .desired_rows(10)
                                        .hint_text("дефис=-\nдвоеточие=:\nточка с запятой=;\nоткрывающая скобка=(\nзакрывающая скобка=)")
                                )
                            };
                            if resp.changed() {
                                self.save_word_replacements();
                            }
                        });
                        ui.add_space(5.0);
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            ui.label(egui::RichText::new("Пример: скажете \"дефис\" → вставится \"-\"").small().color(egui::Color32::from_rgb(100, 180, 255)));
                        });
                        ui.add_space(5.0);
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            ui.label(egui::RichText::new("💾 Сохраняется автоматически").small().color(egui::Color32::GRAY));
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
            let record_name = self.state.hotkey_record.lock().unwrap().to_string();

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
                                ui.label(egui::RichText::new(format!("ГОТОВ ({})", record_name)).color(egui::Color32::from_rgb(80, 255, 120)).size(13.0).monospace());
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

    // Отслеживаем горячие клавиши через прямой опрос состояния клавиш.
    // Это надёжнее низкоуровневого хука: опрос не зависит от активного окна
    // и его не перехватывают сторонние хуки (драйверы GPU и т.п.).
    thread::spawn(move || {
        let mut prev_record_down = false;
        let mut prev_exit_down = false;

        loop {
            thread::sleep(Duration::from_millis(15));

            let record_down = state_for_keys.hotkey_record.lock().unwrap().is_pressed();
            let exit_down = state_for_keys.hotkey_exit.lock().unwrap().is_pressed();

            if record_down && !prev_record_down && !state_for_keys.is_recording.load(Ordering::SeqCst) {
                state_for_keys.is_recording.store(true, Ordering::SeqCst);
                *state_for_keys.status.lock().unwrap() = "🔴 Запись...".to_string();
                ctx_for_keys.request_repaint();
            }

            if !record_down && prev_record_down && state_for_keys.is_recording.load(Ordering::SeqCst) {
                state_for_keys.is_recording.store(false, Ordering::SeqCst);
                *state_for_keys.status.lock().unwrap() = "⚙ Обработка...".to_string();
                ctx_for_keys.request_repaint();
            }

            if exit_down && !prev_exit_down {
                std::process::exit(0);
            }

            prev_record_down = record_down;
            prev_exit_down = exit_down;
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
                        if state.whisper_ctx.lock().unwrap().is_none() {
                            *state.status.lock().unwrap() = "⏳ Модель ещё загружается, повторите позже".to_string();
                            ctx.request_repaint();
                            continue;
                        }

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

                    *state.status.lock().unwrap() = format!(
                        "🟢 Ожидание (Зажмите {})",
                        state.hotkey_record.lock().unwrap()
                    );
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

    #[test]
    fn groups_today() {
        assert_eq!(group_label_for_days(0), Some("Сегодня"));
        assert_eq!(group_label_for_days(-1), Some("Сегодня"));
    }

    #[test]
    fn groups_yesterday() {
        assert_eq!(group_label_for_days(1), Some("Вчера"));
    }

    #[test]
    fn groups_last_week() {
        assert_eq!(group_label_for_days(2), Some("На прошлой неделе"));
        assert_eq!(group_label_for_days(6), Some("На прошлой неделе"));
    }

    #[test]
    fn older_entries_have_no_relative_group() {
        assert_eq!(group_label_for_days(7), None);
        assert_eq!(group_label_for_days(30), None);
    }

    #[test]
    fn month_names_in_genitive() {
        assert_eq!(month_name_ru(1), "января");
        assert_eq!(month_name_ru(10), "октября");
        assert_eq!(month_name_ru(12), "декабря");
    }

    #[test]
    fn parses_mic_gain() {
        assert_eq!(parse_mic_gain("15"), 15.0);
        assert_eq!(parse_mic_gain(" 12 "), 12.0);
    }

    #[test]
    fn mic_gain_falls_back_to_default_on_garbage() {
        assert_eq!(parse_mic_gain("abc"), 10.0);
        assert_eq!(parse_mic_gain(""), 10.0);
    }

    #[test]
    fn mic_gain_is_clamped() {
        assert_eq!(parse_mic_gain("0"), 1.0);
        assert_eq!(parse_mic_gain("100"), 50.0);
        assert_eq!(parse_mic_gain("-5"), 1.0);
    }

    #[test]
    fn encode_env_value_quotes_plain() {
        assert_eq!(encode_env_value("F9"), "\"F9\"");
    }

    #[test]
    fn encode_env_value_escapes_newline() {
        assert_eq!(encode_env_value("a=b\nc=d"), "\"a=b\\nc=d\"");
    }

    #[test]
    fn encode_env_value_escapes_dollar_and_quotes() {
        assert_eq!(encode_env_value("a$b\"c"), "\"a\\$b\\\"c\"");
    }

    #[test]
    fn encode_env_value_drops_carriage_return() {
        assert_eq!(encode_env_value("a\r\nb"), "\"a\\nb\"");
    }

    #[test]
    fn env_values_survive_dotenvy_roundtrip() {
        let filler = "эм,ну,как бы";
        let repl = "дефис=-\nдвоеточие=:$";
        let content = format!(
            "FILLER_WORDS={}\nWORD_REPLACEMENTS={}\n",
            encode_env_value(filler),
            encode_env_value(repl)
        );
        let parsed: Vec<(String, String)> = dotenvy::from_read_iter(content.as_bytes())
            .filter_map(|r| r.ok())
            .collect();
        assert!(parsed.contains(&("FILLER_WORDS".to_string(), filler.to_string())));
        assert!(parsed.contains(&("WORD_REPLACEMENTS".to_string(), repl.to_string())));
    }

    #[test]
    fn horizontal_header_interact_registers_click() {
        use std::cell::Cell;
        let ctx = egui::Context::default();
        let clicked = Cell::new(false);

        let make_input = |events: Vec<egui::Event>| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400.0, 300.0),
            )),
            events,
            ..Default::default()
        };

        let run = |events: Vec<egui::Event>| {
            let _ = ctx.run(make_input(events), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let header = ui.horizontal(|ui| {
                        ui.label("Сегодня");
                    });
                    let header = ui.interact(
                        header.response.rect,
                        ui.make_persistent_id("header_toggle_test"),
                        egui::Sense::click(),
                    );
                    if header.clicked() {
                        clicked.set(true);
                    }
                });
            });
        };

        let pos = egui::pos2(30.0, 16.0);
        run(vec![egui::Event::PointerMoved(pos)]);
        run(vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
        ]);
        run(vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }]);

        assert!(clicked.get(), "клик по заголовку не зарегистрирован");
    }
}