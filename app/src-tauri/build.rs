fn main() -> tauri_build::Result<()> {
    // Требование прав администратора: задаём собственный манифест exe
    // через официальный механизм tauri-build (WindowsAttributes::app_manifest).
    // Внутри манифеста должен быть Common Controls v6 — без него у Tauri
    // не работают нативные диалоги.
    let windows =
        tauri_build::WindowsAttributes::new().app_manifest(include_str!("app-manifest.xml"));
    let attributes = tauri_build::Attributes::new().windows_attributes(windows);
    tauri_build::try_build(attributes)
}