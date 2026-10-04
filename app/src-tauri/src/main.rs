// Предотвращает открытие консоли в release-сборках.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bridge;
mod diagnostics;
mod logger;
mod runner;

use crate::bridge::{
    AppState, ac_power_online, appsettings_path, background_sample, checkpoint_discard,
    checkpoint_status, diagnostics_file_name, estimate_session, get_settings, history_delete,
    history_export_to, history_list, history_open, history_open_folder, history_report,
    identity_info, is_admin, list_schemes, log_flush, log_history, open_file, open_folder,
    phase_plan, quarantine_clear, quarantine_list, results_dir, save_diagnostics, scheme_action,
    session_report, set_settings, start_test, stop_test, storage_stats, system_ready, test_presets,
    test_running,
};
use tauri::{
    Manager,
    menu::{Menu, MenuItem},
    tray::{TrayIcon, TrayIconBuilder},
};

// Хранит дескриптор трея, чтобы он не был удалён до завершения приложения.
#[allow(dead_code)]
struct TrayState(std::sync::Mutex<Option<TrayIcon>>);

/// Новый идентификатор плана (универсальный уникальный, формат UUIDv4).
pub fn new_plan_guid() -> String {
    let hi = rand_u64();
    let lo = rand_u64();
    // Версия 4 и вариант RFC 4122 задаются явно, иначе Windows может
    // отвергнуть имя плана при `powercfg /duplicatescheme`.
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        (hi >> 32) as u32,
        (hi >> 16) as u16,
        (hi & 0x0FFF) as u16,
        // Вариант 10xx.
        ((lo >> 48) as u16 & 0x3FFF) | 0x8000,
        lo & 0xFFFF_FFFF_FFFF
    )
}

/// splitmix64 с состоянием в потоке: прежний вариант брал наносекунды часов
/// заново на каждый вызов, поэтому два соседних вызова в одном тике получали
/// одно и то же значение, а формат `{:08x}` для `u64` не обрезал старшие
/// разряды — первая группа получалась 16 символов вместо 8.
fn rand_u64() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static STATE: AtomicU64 = AtomicU64::new(0);
    let mut s = STATE.load(Ordering::Relaxed);
    if s == 0 {
        s = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15)
            | 1;
    }
    s = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
    STATE.store(s, Ordering::Relaxed);
    let mut z = s;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Завершение перед выходом из приложения.
///
/// Два выхода должны быть одинаковыми. Раньше выход через трей вызывал
/// `app.exit(0)` напрямую и пропускал этот шаг: процесс завершался, не дожидаясь
/// фоновой сессии, а журнал оставался без последних строк. Сессия при этом
/// могла остаться с применённой тестовой схемой питания.
fn shutdown(app: &tauri::AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        runner::join(&state.runner);
        // Журнал пишется фоновым потоком: без явного сброса последние строки
        // остались бы только в памяти.
        state.log.flush();
        // Если запись на диск отказывала, пользователь должен узнать об этом
        // сейчас: иначе он будет искать в `AppLog.json` записи, которых там
        // не будет, и объяснить отсутствие выдуманной причиной.
        state.log.warn_if_not_persisted("warn");
    }
}

fn main() {
    // Второй экземпляр выходит сразу, ДО восстановления после прерывания.
    // Иначе он записал бы поверх чужого маркера прогона и карантинил
    // исправную схему, а потом ещё и переключил питание поверх идущего замера.
    let _instance = match powerbench_windows::instance::SingleInstance::acquire() {
        Some(g) => g,
        None => {
            powerbench_windows::instance::show_already_running();
            return;
        }
    };
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
                "quit" => {
                    shutdown(app);
                    app.exit(0);
                }
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
            background_sample,
            results_dir,
            appsettings_path,
            save_diagnostics,
            diagnostics_file_name,
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                // Ждём завершения фоновой сессии: контрольная точка и
                // восстановление схемы должны успеть сохраниться.
                shutdown(window.app_handle());
            }
        })
        .run(tauri::generate_context!())
        .expect("ошибка при запуске PowerBench");
}

#[cfg(test)]
mod tests {
    use super::new_plan_guid;

    /// `{:08x}` — минимальная, а не максимальная ширина, поэтому прежний
    /// формат давал 44-символьные «идентификаторы схемы», которые
    /// `powercfg` не принимает.
    #[test]
    fn plan_guid_is_36_chars_and_unique() {
        let a = new_plan_guid();
        let b = new_plan_guid();
        assert_eq!(
            a.len(),
            36,
            "ожидался UUIDv4 из 36 символов, получено «{a}»"
        );
        assert_eq!(
            b.len(),
            36,
            "ожидался UUIDv4 из 36 символов, получено «{b}»"
        );
        assert_ne!(a, b, "два соседних вызова дали одинаковый идентификатор");
        let groups: Vec<&str> = a.split('-').collect();
        assert_eq!(
            groups.iter().map(|g| g.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12]
        );
        assert!(groups[2].starts_with('4'), "версия UUID должна быть 4");
        assert!(
            matches!(groups[3].chars().next(), Some('8' | '9' | 'a' | 'b')),
            "вариант RFC 4122 должен быть 10xx, получено «{}»",
            groups[3]
        );
        assert!(a.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
    }
}
