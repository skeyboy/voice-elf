mod static_server;

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

struct LocalServer {
    _origin: String,
    task: tauri::async_runtime::JoinHandle<()>,
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let server = static_server::start(app.path().app_config_dir()?)?;
            let origin = server.origin.clone();
            app.manage(LocalServer {
                _origin: origin.clone(),
                task: server.task,
            });

            WebviewWindowBuilder::new(
                app,
                "main",
                WebviewUrl::External(origin.parse().expect("local server origin is a valid URL")),
            )
            .title("Voice Elf")
            .inner_size(1180.0, 820.0)
            .min_inner_size(360.0, 640.0)
            .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to run Voice Elf");
}
