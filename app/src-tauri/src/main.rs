// Предотвращает открытие консоли в release-сборках.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bridge;
mod logger;
mod runner;

use crate::bridge::{
    AppState, ac_power_online, appsettings_path, checkpoint_discard, checkpoint_status,
    estimate_session, get_settings, history_delete, history_export_to, history_list,
    history_open, history_open_folder, history_report, identity_info, is_admin, list_schemes,
    log_flush, log_history, open_file, open_folder, phase_plan, quarantine_clear,
    quarantine_list,
    results_dir,
    scheme_action, session_report, set_settings, start_test, stop_test, storage_stats,
    system_ready, test_presets, test_running,
};
use tauri::{
    Manager,
    menu::{Menu, MenuItem},
    tray::{TrayIcon, TrayIconBuilder},
};

// Хранит дескриптор трея, чтобы он не был удалён до завершения приложения.
#[allow(dead_code)]
struct TrayState(std::sync::Mutex<Option<TrayIcon>>);

/// Новый идентификатор плана (универсальный уникальный).
pub fn new_plan_guid() -> String {
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        rand_u64(),
        rand_u64() as u16,
        (rand_u64() as u16) & 0x0FFF | 0x4000,
        ((rand_u64() as u16) & 0x3FFF) | 0x8000,
        rand_u64() & 0xFFFF_FFFF_FFFF
    )
}

fn rand_u64() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let mut s = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15);
    // xorshift64* — достаточно для идентификаторов плана.
    s ^= s >> 12;
    s ^= s << 25;
    s ^= s >> 27;
    s.wrapping_mul(0x2545_F491_4F6C_DD1D)
}

fn main() {
    // Если прошлая сессия была прервана (kill/падение), исходная схема могла
    // остаться переключённой — восстанавливаем до старта интерфейса.
    {
        use powerbench_orchestrator::recovery::recover_interrupted_session;
        use powerbench_orchestrator::session::RealSchemeDriver;
        let outcome = recover_interrupted_session(&RealSchemeDriver);
        if outcome.interrupted_checkpoint && !outcome.already_ok {
            eprintln!(
                "PowerBench: восстановление после прерывания: restored={} {:?}",
                outcome.restored, outcome.error
            );
        }
    }
    let state = bridge::AppState {
        runner: std::sync::Arc::new(std::sync::Mutex::new(None)),
        log: std::sync::Arc::new(logger::Logger::new()),
    };
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .setup(|app| {
            let tray_icon = TrayIconBuilder::with_id("powerbench-tray")
                .tooltip("PowerBench")
                .icon(tauri::image::Image::from_bytes(include_bytes!(
                    "../icons/tray-icon-32.png"
                ))?)
                .build(app)?;
            let show_i = MenuItem::with_id(app, "show", "Показать окно", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Выход", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &quit_i])?;
            tray_icon.set_menu(Some(menu))?;
            tray_icon.on_menu_event(|app, event| match event.id.as_ref() {
                "show" => {
                    if let Some(w) = app.get_webview_window("main") {
                        let _ = w.show();
                        let _ = w.unminimize();
                        let _ = w.set_focus();
                    }
                }
                "quit" => app.exit(0),
                _ => {}
            });
            app.manage(TrayState(std::sync::Mutex::new(Some(tray_icon))));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_schemes,
            is_admin,
            ac_power_online,
            scheme_action,
            get_settings,
            set_settings,
            checkpoint_status,
            checkpoint_discard,
            identity_info,
            start_test,
            stop_test,
            test_running,
            history_list,
            history_open,
            history_export_to,
            history_report,
            session_report,
            history_delete,
            history_open_folder,
            quarantine_list,
            quarantine_clear,
            estimate_session,
            test_presets,
            phase_plan,
            storage_stats,
            open_folder,
            open_file,
            log_flush,
            log_history,
            system_ready,
            results_dir,
            appsettings_path
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                // Ждём завершения фоновой сессии: контрольная точка и
                // восстановление схемы должны успеть сохраниться.
                use tauri::Manager;
                if let Some(state) = window.app_handle().try_state::<AppState>() {
                    runner::join(&state.runner);
                    // Журнал пишется фоновым потоком: без явного сброса
                    // последние строки остались бы только в памяти.
                    state.log.flush();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("ошибка при запуске PowerBench");
}
