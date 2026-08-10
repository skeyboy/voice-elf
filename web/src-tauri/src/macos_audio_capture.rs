use std::{
    ffi::{CStr, c_char, c_int},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use tauri::{AppHandle, Manager};

static CAPTURE_APP: OnceLock<Mutex<Option<AppHandle>>> = OnceLock::new();
static CAPTURE_ACTIVE: AtomicBool = AtomicBool::new(false);
static CAPTURE_FRAMES: AtomicU64 = AtomicU64::new(0);
static CAPTURE_SAMPLE_RATE: AtomicU32 = AtomicU32::new(0);
static CAPTURE_ERROR: OnceLock<Mutex<Option<String>>> = OnceLock::new();

unsafe extern "C" {
    fn voice_elf_macos_audio_supported() -> bool;
    fn voice_elf_macos_audio_start(
        samples_callback: extern "C" fn(*const i16, usize, u32),
        event_callback: extern "C" fn(c_int, *const c_char),
    );
    fn voice_elf_macos_audio_stop();
}

pub(crate) fn supported() -> bool {
    // SAFETY: The Objective-C implementation has no preconditions and only checks OS availability.
    unsafe { voice_elf_macos_audio_supported() }
}

pub(crate) fn start(app: AppHandle) -> Result<(), &'static str> {
    if !supported() {
        return Err("系统内录需要 macOS 13 或更高版本");
    }
    if CAPTURE_ACTIVE.swap(true, Ordering::AcqRel) {
        return Err("系统内录已在运行");
    }
    CAPTURE_FRAMES.store(0, Ordering::Release);
    CAPTURE_SAMPLE_RATE.store(0, Ordering::Release);
    *capture_error()
        .lock()
        .expect("capture error mutex poisoned") = None;
    *capture_app().lock().expect("capture app mutex poisoned") = Some(app);
    // SAFETY: Both callbacks remain valid for the process lifetime and copy callback data immediately.
    unsafe { voice_elf_macos_audio_start(handle_samples, handle_event) };
    Ok(())
}

pub(crate) fn stop() {
    // SAFETY: Stopping is idempotent in the Objective-C implementation.
    unsafe { voice_elf_macos_audio_stop() };
}

pub(crate) fn status() -> serde_json::Value {
    serde_json::json!({
        "supported": supported(),
        "active": CAPTURE_ACTIVE.load(Ordering::Acquire),
        "frames": CAPTURE_FRAMES.load(Ordering::Acquire),
        "sample_rate": CAPTURE_SAMPLE_RATE.load(Ordering::Acquire),
        "error": capture_error().lock().expect("capture error mutex poisoned").clone(),
    })
}

fn capture_app() -> &'static Mutex<Option<AppHandle>> {
    CAPTURE_APP.get_or_init(|| Mutex::new(None))
}

fn capture_error() -> &'static Mutex<Option<String>> {
    CAPTURE_ERROR.get_or_init(|| Mutex::new(None))
}

extern "C" fn handle_samples(samples: *const i16, count: usize, sample_rate: u32) {
    if samples.is_null() || count == 0 {
        return;
    }
    // SAFETY: The native callback guarantees `count` valid i16 samples for this call.
    let bytes = unsafe { std::slice::from_raw_parts(samples.cast::<u8>(), count * 2) };
    CAPTURE_FRAMES.fetch_add(count as u64, Ordering::AcqRel);
    CAPTURE_SAMPLE_RATE.store(sample_rate, Ordering::Release);
    dispatch(serde_json::json!({
        "type": "audio-pcm",
        "data": STANDARD.encode(bytes),
        "sampleRate": sample_rate,
    }));
}

extern "C" fn handle_event(event: c_int, message: *const c_char) {
    let message = if message.is_null() {
        None
    } else {
        // SAFETY: Native event messages are valid UTF-8-compatible C strings for this callback.
        Some(
            unsafe { CStr::from_ptr(message) }
                .to_string_lossy()
                .into_owned(),
        )
    };
    let payload = match event {
        1 => serde_json::json!({ "type": "capture-started", "sampleRate": 48_000 }),
        2 => serde_json::json!({ "type": "capture-stopped" }),
        _ => serde_json::json!({
            "type": "capture-error",
            "message": message.clone().unwrap_or_else(|| "macOS 系统内录发生异常".to_owned()),
        }),
    };
    if event != 1 && event != 2 {
        *capture_error()
            .lock()
            .expect("capture error mutex poisoned") = message.clone();
    }
    dispatch(payload);
    if event != 1 {
        CAPTURE_ACTIVE.store(false, Ordering::Release);
        *capture_app().lock().expect("capture app mutex poisoned") = None;
    }
}

fn dispatch(detail: serde_json::Value) {
    let app = capture_app()
        .lock()
        .expect("capture app mutex poisoned")
        .clone();
    let Some(window) = app.and_then(|app| app.get_webview_window("main")) else {
        return;
    };
    let script = format!(
        "window.dispatchEvent(new CustomEvent('voice-elf:mac-native',{{detail:{detail}}}));"
    );
    if let Err(error) = window.eval(script) {
        eprintln!("failed to emit macOS audio event: {error}");
    }
}
