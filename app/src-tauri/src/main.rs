// Предотвращает открытие консоли в release-сборках.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bridge;
mod runner;

use crate::bridge::{
    ac_power_online, appsettings_path, checkpoint_status, get_settings, history_export_to,
    history_list, history_open, identity_info, is_admin, list_schemes, results_dir, scheme_action,
    set_settings, start_test, stop_test, test_running, AppState,
};

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
    let state = bridge::AppState {
        runner: std::sync::Arc::new(std::sync::Mutex::new(None)),
    };
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            list_schemes,
            is_admin,
            ac_power_online,
            scheme_action,
            get_settings,
            set_settings,
            checkpoint_status,
            identity_info,
            start_test,
            stop_test,
            test_running,
            history_list,
            history_open,
            history_export_to,
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
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("ошибка при запуске PowerBench");
}