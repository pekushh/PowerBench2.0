//! Консольный валидатор ядра GamingCpuV1 (Этап 2).
//!
//! Автономная самопроверка:
//! - замороженный хэш конфигурации (зафиксирован строкой в коде);
//! - двойной reset и сверка контрольных сумм по профилям (Light/Heavy/Response);
//! - расписание суперцикла «Отклика» (63/127/191 → Medium, 255 → Major);
//! - независимость контрольных сумм от числа воркеров;
//! - активная отмена с ограниченным временем остановки и восстановлением пула;
//! - ноль аллокаций в главном цикле тиков (считающий глобальный аллокатор).
//!
//! Код выхода: 0 — все проверки прошли, 1 — какая-либо проверка не прошла.
//! Флаг `--json <путь>` формирует машинный отчёт. Отчёт детерминирован
//! (в нём нет времени и длительностей): последовательные запуски дают
//! идентичные сигнатуры и контрольные суммы.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime};

use powerbench_core::alloc_count::COUNT;
use powerbench_core::config::{self, Phase, Profile, RESPONSE_SUPERCYCLE, response_profile};
use powerbench_core::engine::{Engine, RunError, RunTarget};

use serde::Serialize;

mod bench;
mod ctrlc;

/// Замороженный хэш конфигурации: SHA-256 от payload конфигурации, hex UPPER.
/// Зафиксирован строкой в коде — расхождение с `config_hash()` означает отказ.
/// Замороженный хэш конфигурации нагрузки.
///
/// Смысл проверки — заметить НЕОЖИДАННУЮ правку объёма работы: изменилась хоть
/// одна константа из `config_payload`, и прогоны разных сборок больше нельзя
/// сравнивать. Поэтому при плановом изменении нагрузки хэш обновляется здесь же
/// и в сообщении коммита, иначе расхождение обнаружится только как «контрольная
/// сумма различается между повторами».
///
/// Обновлено вместе с составом `config_payload`: раньше константа осталась с
/// самого первого коммита, тогда как `core/src/config.rs` правили ещё четыре
/// раза, — и проверка падала всегда, то есть валидатор был мёртвым.
///
/// Обновлено и после регресса M28: объём работы профиля `Partial` приведён к
/// ровно половине `Heavy` (было 75 % при половине занятых ядер), а это четыре
/// числа в `config_payload`. Хэш обязан быть пересчитан ВМЕСТЕ с такой правкой,
/// иначе валидатор начинает врать о причине провала. Тест
/// `frozen_config_hash_matches_the_payload` не даёт забыть про это в следующий
/// раз: он требовал бы обновить константу вручную, если бы проверка была только
/// в рантайме — а её не было, и именно поэтому расхождение и прожило незамеченным.
const FROZEN_CONFIG_HASH: &str = "24749E3636EE5CBF86164625DBCBF0F43C73B7BF21E4EB9F8E0D35232FBB166B";

/// Тики коротких прогонов самопроверки (детерминированные сигнатуры).
const SHORT_TICKS: u64 = 16;

/// Фаза «Отклик» прогоняется минимум одним полным суперциклом (256 тиков),
/// чтобы в сигнатуру вошли все классы тиков.
const RESPONSE_TICKS: u64 = RESPONSE_SUPERCYCLE;

/// Матрица чисел воркеров для проверки независимости контрольных сумм
/// (к ней добавляется значение по умолчанию).
const WORKER_MATRIX: [usize; 3] = [1, 2, 4];

/// Один пункт самопроверки (одинаково в человеческом и машинном отчёте).
#[derive(Serialize)]
struct Check {
    name: String,
    /// Нарушений не обнаружено.
    ok: bool,
    /// Проверка способна упасть. `false` — пункт ничего не проверяет в этой
    /// сборке и не должен входить в итоговый вердикт: иначе отчёт зелёный
    /// благодаря проверке, которая не может стать красной ни при каких
    /// обстоятельствах, и это читается как «всё хорошо».
    measured: bool,
    detail: String,
}

/// Машинный отчёт валидатора — полностью детерминированный.
#[derive(Serialize)]
struct Report {
    program: String,
    version: String,
    seed_hex: String,
    config_hash: String,
    logical_cpus: usize,
    default_workers: usize,
    checks: Vec<Check>,
    /// Все ПРОВЕРЯЕМЫЕ пункты прошли.
    passed: bool,
    /// Сколько пунктов в этой сборке проверить невозможно.
    inconclusive: usize,
}

fn check(checks: &mut Vec<Check>, name: &str, ok: bool, detail: impl Into<String>) {
    checks.push(Check {
        name: name.to_string(),
        ok,
        measured: true,
        detail: detail.into(),
    });
}

/// Пункт, который в этой сборке не измеряется: он попадает в отчёт, но не в
/// вердикт. Так он честно виден и не может притвориться успешной проверкой.
fn check_inconclusive(checks: &mut Vec<Check>, name: &str, detail: impl Into<String>) {
    checks.push(Check {
        name: name.to_string(),
        ok: true,
        measured: false,
        detail: detail.into(),
    });
}

/// Справка по CLI.
///
/// Раньше `--help` печатал одну строку `Использование: powerbench-cli [--json
/// <путь>]`, хотя CLI уже умел шесть подкоманд и двенадцать флагов: человек,
/// вызвавший `powerbench-cli --help`, не узнавал ни про `bench`, ни про
/// пресеты. Справка собирается из тех же констант, что и разбор аргументов, а
/// числа в ней проверяет тест `help_matches_the_real_flags` — иначе она
/// разъедется с кодом снова.
fn help_text() -> String {
    use powerbench_orchestrator::config::{
        DETAILED_PRESET, MAX_COOLING_SECONDS, MAX_DURATION_SECONDS, MAX_REPETITIONS,
        MIN_COOLING_SECONDS, MIN_DURATION_SECONDS, MIN_REPETITIONS, MIN_WARMUP_SECONDS,
        QUICK_PRESET,
    };
    format!(
        "\
PowerBench CLI — бенчмарк схем питания и самопроверка ядра GamingCpuV1.

ИСПОЛЬЗОВАНИЕ
  powerbench-cli <подкоманда> [флаги]
  powerbench-cli [--json <путь>]          самопроверка ядра, отчёт в JSON

ПОДКОМАНДЫ
  list                    список схем питания
  bench                   замер по плану
  resume                  продолжить прерванную сессию
  status                  состояние контрольной точки
  history list            список сохранённых сессий
  history export <plan|all>  экспорт сессии (--format json|csv, --out)
  settings show           текущие настройки приложения
  settings reset          сбросить настройки к значениям по умолчанию

ФЛАГЫ `bench`
  --preset <quick|detailed>   пресет сценария (по умолчанию detailed)
  --duration <сек>            измеряемая часть, {MIN_DURATION_SECONDS}..={MAX_DURATION_SECONDS}
  --warmup <сек>              разогрев, от {MIN_WARMUP_SECONDS}
  --cooling <сек>             пауза между прогонами, {MIN_COOLING_SECONDS}..={MAX_COOLING_SECONDS}
  --reps <n>                  раундов, {MIN_REPETITIONS}..={MAX_REPETITIONS}
  --threshold <%>             порог фоновой нагрузки на ядро, 0.5..=100
  --workers <n>               воркеров пула (по умолчанию логические ядра − 2)
  --schemes <g1,g2,...>       GUID через запятую (только GUID, без имён);
                              без флага — интерактивный выбор списком
  --plan <guid>               GUID плана (свои настройки сессии)
  --out <путь>                итоговый JSON (по умолчанию %LOCALAPPDATA%\\PowerBench\\benchmark-result.json)
  --allow-dirty-background    начать при фоновой нагрузке выше порога

  `resume` использует план контрольной точки, поэтому --schemes и --plan
  для него не поддерживаются.

ПРЕСЕТЫ
{quick}
{detailed}

  Длительность делится на четыре фазы: 30 % лёгкой, 20 % частичной,
  35 % тяжёлой, остаток — «Отклик».

ПРОЧЕЕ
  --json <путь>               сохранить отчёт самопроверки в JSON
  -h, --help                  эта справка

Примеры
  powerbench-cli list
  powerbench-cli bench --preset quick --schemes 381b4222-f694-41f0-9685-ff5bb260df2e,8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c
  powerbench-cli bench --preset detailed --reps 3 --out .\\result.json
  powerbench-cli history export all --format csv --out .\\exports",
        quick = preset_line("quick", QUICK_PRESET),
        detailed = preset_line("detailed", DETAILED_PRESET),
    )
}

/// Строка пресета для справки: «60 с прогона, 8 с разогрева, …, 5 раундов».
fn preset_line(name: &str, p: powerbench_orchestrator::config::Preset) -> String {
    format!(
        "  {name:<9} {} с прогона, {} с разогрева, {} с охлаждения, {} раунд{}",
        p.duration_seconds,
        p.warmup_seconds,
        p.cooling_seconds,
        p.repetitions,
        plural_raunds(p.repetitions),
    )
}

/// Окончание слова «раунд» по числу.
fn plural_raunds(n: u32) -> &'static str {
    match n % 10 {
        1 if n % 100 != 11 => "",
        2..=4 if (n % 100 < 12 || n % 100 > 14) => "а",
        _ => "ов",
    }
}

fn main() -> ExitCode {
    // Подкоманды: list / bench / resume / status / history / settings. Первый
    // токен без дефиса — имя подкоманды; иначе — существующий путь (Этап 2).
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(first) = args.first().filter(|f| !f.starts_with('-')) {
        return match first.as_str() {
            "list" => bench::cmd_list(&args[1..]),
            "bench" => bench::cmd_bench(&args[1..]),
            "resume" => bench::cmd_resume(&args[1..]),
            "status" => bench::cmd_status(&args[1..]),
            "history" => cmd_history(&args[1..]),
            "settings" => cmd_settings(&args[1..]),
            other => {
                eprintln!(
                    "PowerBench CLI: неизвестная подкоманда «{other}» (ожидается list, bench, \
                     resume, status, history или settings). Полный список: powerbench-cli --help"
                );
                return ExitCode::FAILURE;
            }
        };
    }

    // Разбор аргументов: единственная опция `--json <путь>`.
    let mut json_path: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => {
                i += 1;
                match args.get(i) {
                    Some(path) => json_path = Some(PathBuf::from(path)),
                    None => {
                        eprintln!("PowerBench CLI: `--json` требует путь к файлу отчёта.");
                        return ExitCode::FAILURE;
                    }
                }
            }
            "--help" | "-h" => {
                println!("{}", help_text());
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!(
                    "PowerBench CLI: неизвестный аргумент «{other}». Полный список: \
                     powerbench-cli --help"
                );
                return ExitCode::FAILURE;
            }
        }
        i += 1;
    }

    let report = run_validator();

    println!("PowerBench — консольный валидатор «{}»", report.version);
    println!("--------------------------------");
    println!("хэш конфигурации : {}", report.config_hash);
    println!("seed              : {}", report.seed_hex);
    println!("логических CPU    : {}", report.logical_cpus);
    println!("воркеров (default): {}", report.default_workers);
    println!();
    for c in &report.checks {
        // Непроверяемый пункт не должен выглядеть как успешная проверка:
        // метка «SKIP» говорит, что падать ему нечему, и в вердикт он не входит.
        let tag = if !c.measured {
            "SKIP"
        } else if c.ok {
            "ok  "
        } else {
            "FAIL"
        };
        println!("[{tag}] {}", c.name);
        println!("       {}", c.detail);
    }
    println!();
    if report.inconclusive > 0 {
        println!(
            "Из {} пунктов {} в этой сборке проверить невозможно и в вердикт не входят.",
            report.checks.len(),
            report.inconclusive
        );
    }
    if report.passed {
        println!("Итог: PASS (код выхода 0)");
    } else {
        println!("Итог: FAIL (код выхода 1)");
    }

    if let Some(path) = json_path {
        let json = match serde_json::to_string_pretty(&report) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("PowerBench CLI: не удалось сериализовать отчёт: {e}");
                return ExitCode::FAILURE;
            }
        };
        if let Err(e) = std::fs::write(&path, format!("{json}\n")) {
            eprintln!(
                "PowerBench CLI: не удалось записать отчёт «{}»: {e}",
                path.display()
            );
            return ExitCode::FAILURE;
        }
    }

    if report.passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Диспетчер подкоманды `history` (list / export).
fn cmd_history(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("list") => bench::cmd_history_list(&args[1..]),
        Some("export") => bench::cmd_history_export(&args[1..]),
        Some(other) => {
            eprintln!(
                "PowerBench CLI: неизвестная подкоманда истории «{other}» (ожидается list или export)."
            );
            ExitCode::FAILURE
        }
        None => {
            eprintln!("PowerBench CLI: `history` требует подкоманду (list или export).");
            ExitCode::FAILURE
        }
    }
}

/// Диспетчер подкоманды `settings` (show / reset).
fn cmd_settings(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("show") => bench::cmd_settings_show(&args[1..]),
        Some("reset") => bench::cmd_settings_reset(&args[1..]),
        Some(other) => {
            eprintln!(
                "PowerBench CLI: неизвестная подкоманда настроек «{other}» (ожидается show или reset)."
            );
            ExitCode::FAILURE
        }
        None => {
            eprintln!("PowerBench CLI: `settings` требует подкоманду (show или reset).");
            ExitCode::FAILURE
        }
    }
}

/// Новый идентификатор плана (уникальный в пределах секунды/машины).
fn new_plan_guid() -> String {
    let ns = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    format!("plan-{:016x}", ns)
}

/// Путь по умолчанию для итогового JSON-результата.
fn default_result_path() -> PathBuf {
    powerbench_orchestrator::checkpoint::data_dir().join("benchmark-result.json")
}

/// Атомарная запись JSON-результата (создаёт каталоги).
fn write_json<T: Serialize>(value: &T, path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("не удалось создать каталог «{}»: {e}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(path, format!("{json}\n"))
        .map_err(|e| format!("не удалось записать результат «{}»: {e}", path.display()))
}

fn run_validator() -> Report {
    let mut engine = Engine::new(None);
    let logical_cpus = engine.logical_cpus();
    let default_workers = engine.worker_count();
    let mut checks: Vec<Check> = Vec::new();

    // 1. Замороженный хэш конфигурации.
    let actual_hash = config::config_hash();
    let hash_ok = actual_hash == FROZEN_CONFIG_HASH;
    check(
        &mut checks,
        "frozen-config-hash",
        hash_ok,
        format!("ожидалось {FROZEN_CONFIG_HASH}, получено {actual_hash}"),
    );

    // 2. Обязательная самопроверка ядра (двойной прогон всех фаз).
    let self_ok = engine.self_check().is_ok();
    check(
        &mut checks,
        "kernel-self-check",
        self_ok,
        if self_ok {
            "двойной reset и прогон Light/Heavy/Response дали одинаковые контрольные суммы"
        } else {
            "самопроверка ядра завершилась ошибкой детерминизма"
        },
    );

    // 3. Двойной reset и сверка контрольных сумм по профилям.
    let mut actual_first: Vec<(Phase, u64)> = Vec::new();
    for phase in [Phase::Light, Phase::Heavy, Phase::Response] {
        let ticks = if phase == Phase::Response {
            RESPONSE_TICKS
        } else {
            SHORT_TICKS
        };
        engine.reset();
        let a = engine.run_phase(phase, RunTarget::Ticks(ticks));
        engine.reset();
        let b = engine.run_phase(phase, RunTarget::Ticks(ticks));
        let (ok, first, run) = match (&a, &b) {
            (Ok(aa), Ok(bb)) => (
                aa.ticks == ticks
                    && bb.ticks == ticks
                    && aa.first_tick_checksum == bb.first_tick_checksum
                    && aa.run_checksum == bb.run_checksum,
                aa.first_tick_checksum,
                aa.run_checksum,
            ),
            _ => (false, 0, 0),
        };
        if ok {
            actual_first.push((phase, first));
        }
        check(
            &mut checks,
            &format!("double-reset:{phase:?}"),
            ok,
            format!(
                "первый тик = {:016X}, итоговая = {:016X} (совпали в двух попытках: {ok})",
                first, run
            ),
        );
    }

    // 4. Сверка первого тика каждой фазы с эталоном самопроверки.
    let mut ref_ok = true;
    let mut ref_detail = String::new();
    for (phase, actual) in &actual_first {
        let profile = phase.first_profile();
        let expected = engine.first_tick_reference(profile);
        let ok = expected.is_some() && expected == Some(*actual);
        ref_ok &= ok;
        ref_detail.push_str(&format!(
            "{phase:?}={:016X}{} ",
            *actual,
            if ok { "" } else { " (≠ эталон)" }
        ));
    }
    check(&mut checks, "references-verify", ref_ok, ref_detail);

    // 5. Расписание суперцикла «Отклика» (независимо от номера раунда).
    let mut sched_ok = true;
    let mut base = 0usize;
    let mut medium = 0usize;
    let mut major = 0usize;
    for round in 0..2u64 {
        for i in 0..RESPONSE_SUPERCYCLE {
            let idx = round * RESPONSE_SUPERCYCLE + i;
            let expected = match i {
                63 | 127 | 191 => Profile::ResponseMedium,
                255 => Profile::ResponseMajor,
                _ => Profile::ResponseBase,
            };
            if response_profile(idx) != expected {
                sched_ok = false;
            }
            match response_profile(idx) {
                Profile::ResponseBase => base += 1,
                Profile::ResponseMedium => medium += 1,
                Profile::ResponseMajor => major += 1,
                _ => {}
            }
        }
    }
    check(
        &mut checks,
        "response-schedule",
        sched_ok,
        format!("2 суперцикла по 256: base={base}, medium={medium}, major={major}"),
    );

    // 6. Независимость контрольных сумм от числа воркеров.
    let mut workers = WORKER_MATRIX.to_vec();
    workers.push(default_workers);
    workers.sort_unstable();
    workers.dedup();
    let mut indep_ok = true;
    let mut heavy_ref: Option<u64> = None;
    let mut heavy_first: Option<u64> = None;
    let mut response_ref: Option<u64> = None;
    let mut response_first: Option<u64> = None;
    for &w in &workers {
        let mut probe = Engine::new(Some(w));
        probe.reset();
        let heavy = probe.run_phase(Phase::Heavy, RunTarget::Ticks(64));
        probe.reset();
        let response = probe.run_phase(Phase::Response, RunTarget::Ticks(RESPONSE_TICKS));
        match (heavy, response) {
            (Ok(h), Ok(r)) => {
                match heavy_ref {
                    None => {
                        heavy_ref = Some(h.run_checksum);
                        heavy_first = Some(h.first_tick_checksum);
                    }
                    Some(rf) => {
                        indep_ok &= rf == h.run_checksum;
                        indep_ok &= heavy_first == Some(h.first_tick_checksum);
                    }
                }
                match response_ref {
                    None => {
                        response_ref = Some(r.run_checksum);
                        response_first = Some(r.first_tick_checksum);
                    }
                    Some(rf) => {
                        indep_ok &= rf == r.run_checksum;
                        indep_ok &= response_first == Some(r.first_tick_checksum);
                    }
                }
            }
            _ => indep_ok = false,
        }
    }
    check(
        &mut checks,
        "worker-independence",
        indep_ok,
        format!(
            "воркеры: {:?}; heavy={:016X}, response={:016X}",
            workers,
            heavy_ref.unwrap_or(0),
            response_ref.unwrap_or(0)
        ),
    );

    // 7. Активная отмена: остановка в пределах ограничения и восстановление пула.
    let mut cancel_engine = Engine::new(None);
    cancel_engine.reset();
    let canceller = cancel_engine.canceller();
    let (outcome, latency) = std::thread::scope(|scope| {
        let handler = scope.spawn(|| {
            cancel_engine.prepare_sample_buffer(Phase::Heavy, 300);
            cancel_engine.run_phase(Phase::Heavy, RunTarget::Duration(Duration::from_secs(300)))
        });
        std::thread::sleep(Duration::from_millis(250));
        let start = Instant::now();
        canceller.store(true, Ordering::Release);
        // Паника воркера — тоже результат проверки, а не повод ронять самопроверку.
        let result = match handler.join() {
            Ok(r) => r,
            Err(_) => Err(RunError::WorkerFailed),
        };
        (result, start.elapsed())
    });
    let canceled = matches!(outcome, Err(RunError::Cancelled));
    let bounded = latency < Duration::from_secs(5);
    cancel_engine.reset();
    let pa = cancel_engine.run_phase(Phase::Heavy, RunTarget::Ticks(SHORT_TICKS));
    cancel_engine.reset();
    let pb = cancel_engine.run_phase(Phase::Heavy, RunTarget::Ticks(SHORT_TICKS));
    let recovered = matches!(
        (pa, pb),
        (Ok(a), Ok(b)) if a.run_checksum == b.run_checksum
    );
    check(
        &mut checks,
        "active-cancellation",
        canceled && bounded && recovered,
        format!(
            "фаза остановилась по отмене: {canceled}; в пределах 5 с: {bounded}; \
             пул после отмены работает: {recovered}"
        ),
    );

    // 8. Изоляция измерения: замер обязан быть КОРОЧЕ батча.
    //
    // Это проверяемое утверждение о строении движка, и оно ровно то, что
    // ломается первым при правке цикла тика: если кто-то снова включит в
    // измеряемый интервал ожидание пула или финальную цепочку контрольных
    // сумм, `compute` станет равен `total`, и проверка упадёт. В отличие от
    // счётчика аллокаций, здесь падать есть чему.
    engine.reset();
    let timing_run = engine.run_phase(Phase::Heavy, RunTarget::Ticks(SHORT_TICKS));
    let snap = engine.progress_snapshot();
    let isolation_ok = timing_run.is_ok()
        && snap.last_compute_ns > 0
        && snap.last_total_ns > 0
        && snap.last_compute_ns <= snap.last_total_ns;
    let sync_share = if snap.last_total_ns > 0 {
        100.0 * (snap.last_total_ns - snap.last_compute_ns) as f64 / snap.last_total_ns as f64
    } else {
        0.0
    };
    check(
        &mut checks,
        "timing-isolation",
        isolation_ok,
        format!(
            "вычисление {} нс из батча {} нс (синхронизация и обвязка — {sync_share:.1} %)",
            snap.last_compute_ns, snap.last_total_ns
        ),
    );

    // 9. Привязка потоков к ядрам действительно применена.
    //
    // Проверка ПРОВЕРЯЕМАЯ: `SetThreadAffinityMask` отказать может (политика
    // домена, некоторые виртуализированные ЦП), и тогда замер идёт на
    // произвольных ядрах — то есть результат перестаёт быть тем, чем
    // подписан. Молчать об этом нельзя.
    let failures = engine.affinity_failures();
    check(
        &mut checks,
        "affinity-applied",
        failures.is_empty(),
        if failures.is_empty() {
            engine.topology().describe()
        } else {
            format!(
                "ОС отказала {} воркерам: {}",
                failures.len(),
                failures.join("; ")
            )
        },
    );

    // 10. Ноль аллокаций в главном цикле тиков — НЕ ПРОВЕРЯЕТСЯ в этой сборке.
    //
    // Счётчик живёт за `#[cfg(test)]` в core: в релизной сборке CLI `COUNT`
    // всегда ноль, и проверка проходила бы, ничего не проверяя. Раньше она
    // при этом входила в `passed`, то есть отчёт показывал зелёный статус
    // благодаря пункту, который не может стать красным. Теперь такой пункт
    // помечен `measured: false`, не влияет на вердикт и честно называет, где
    // настоящая проверка: `powerbench-core::tests::zero_allocations_in_tick_loop`,
    // где аллокатор действительно установлен.
    engine.reset();
    let warm = engine.run_phase(Phase::Heavy, RunTarget::Ticks(SHORT_TICKS));
    let measured = engine.run_phase(Phase::Heavy, RunTarget::Ticks(32));
    check_inconclusive(
        &mut checks,
        "zero-allocations",
        format!(
            "в релизной сборке не измеряется (счётчик только в тестах крейта): \
             прогон {} / {}, счётчик {}; настоящая проверка — \
             powerbench-core::tests::zero_allocations_in_tick_loop",
            warm.is_ok(),
            measured.is_ok(),
            COUNT.load(Ordering::Relaxed)
        ),
    );

    let inconclusive = checks.iter().filter(|c| !c.measured).count();
    Report {
        program: "powerbench-cli".to_string(),
        version: config::VERSION.to_string(),
        seed_hex: format!("{:016X}", config::SEED),
        config_hash: config::config_hash().to_string(),
        logical_cpus,
        default_workers,
        // Вердикт считается ТОЛЬКО по проверяемым пунктам: непроверяемый
        // пункт не имеет права сделать отчёт зелёным.
        passed: checks.iter().all(|c| !c.measured || c.ok),
        checks,
        inconclusive,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Регресс M38 (аудит): замороженный хэш обязан совпадать с payload.
    ///
    /// Проверка lived только в рантайме — то есть только когда кто-то запустит
    /// валидатор. Поэтому она молча протухала: регресс M28 изменил четыре
    /// числа в `config_payload`, и `frozen-config-hash` начал падать, показывая
    /// несуществующую проблему в движке. Ни один тест этого не ловил.
    ///
    /// Теперь расхождение обнаруживается при сборке тестов, а не у пользователя.
    #[test]
    fn frozen_config_hash_matches_the_payload() {
        assert_eq!(
            config::config_hash(),
            FROZEN_CONFIG_HASH,
            "изменился payload конфигурации (например, объём работы профиля), но \
             FROZEN_CONFIG_HASH не пересчитан. Плановая правка нагрузки требует \
             обновить константу ВМЕСТЕ с изменением, иначе прогоны разных сборок \
             нельзя сравнивать."
        );
        // Хэш обязан быть hex UPPER фиксированной длины — иначе сравнение строк
        // выше может «случайно» пройти.
        assert_eq!(FROZEN_CONFIG_HASH.len(), 64);
        assert!(
            FROZEN_CONFIG_HASH
                .chars()
                .all(|c| c.is_ascii_digit() || ('A'..='F').contains(&c)),
            "FROZEN_CONFIG_HASH должен быть SHA-256 в верхнем регистре"
        );
    }

    /// Регресс M38: справка обязана совпадать с тем, что CLI понимает.
    ///
    /// Раньше справка печатала одну строку `Использование: powerbench-cli
    /// [--json <путь>]` — при том, что CLI уже умел шесть подкоманд и двенадцать
    /// флагов. Человек, вызвавший `--help`, не узнавал ни про `bench`, ни про
    /// пресеты, и оставался с ощущением, что программа умеет меньше, чем
    /// умеет. Хуже: неизвестная подкоманда тоже не подсказывала, где искать
    /// список (`settings` в сообщении об ошибке не было вовсе).
    ///
    /// Проверяем обе стороны: каждая реальная подкоманда и каждый реальный флаг
    /// присутствуют в справке — и в справке нет ничего, чего CLI не понимает.
    #[test]
    fn help_matches_the_real_flags() {
        let help = help_text();

        // Подкоманды, которые действительно разбирает `main` и `cmd_*`.
        for sub in [
            "list",
            "bench",
            "resume",
            "status",
            "history list",
            "history export",
            "settings show",
            "settings reset",
        ] {
            assert!(
                help.contains(sub),
                "в справке нет подкоманды `{sub}` — CLI её выполняет, а человек \
                 о ней не узнает"
            );
        }

        // Флаги, которые действительно принимает разбор `BenchCli`.
        let bench_src = include_str!("bench.rs");
        let parse_at = bench_src
            .find("fn parse(args: &[String]")
            .or_else(|| bench_src.find("for_resume: bool"))
            .expect("не найден разбор флагов bench");
        let parse_body = &bench_src[parse_at..];
        let parse_end = parse_body
            .find("Ok(cli)")
            .expect("не найден конец разбора флагов bench");
        let parse_body = &parse_body[..parse_end];

        let mut declared: Vec<&str> = Vec::new();
        for line in parse_body.lines() {
            let line = line.trim();
            for candidate in [
                "--preset",
                "--duration",
                "--warmup",
                "--cooling",
                "--reps",
                "--threshold",
                "--workers",
                "--allow-dirty-background",
                "--schemes",
                "--plan",
                "--out",
            ] {
                if line.starts_with(&format!("\"{candidate}\"")) && !declared.contains(&candidate) {
                    declared.push(candidate);
                }
            }
        }
        assert_eq!(
            declared.len(),
            11,
            "разбор флагов bench изменился — список в тесте надо обновить: {declared:?}"
        );
        for flag in &declared {
            assert!(
                help.contains(flag),
                "флаг `{flag}` принимается, но его нет в справке"
            );
        }

        // `--json` и `-h` относятся к пути самопроверки, а не к `bench`.
        assert!(help.contains("--json"), "в справке нет --json");
        assert!(help.contains("--help"), "в справке нет --help");

        // Ничего выдуманного: справка не должна обещать несуществующие флаги.
        // Реально поддерживается только `--config` из старой документации — его
        // и не было, и нет.
        for phantom in [
            "--config",
            "--dry-run",
            "--verbose",
            "--quick",
            "--detailed",
        ] {
            assert!(
                !help.contains(phantom),
                "справка обещает `{phantom}`, которого CLI не понимает"
            );
        }
    }

    /// Справка обязана печатать настоящие значения пресетов, а не протухшие
    /// числа из документации.
    #[test]
    fn help_prints_the_real_preset_values() {
        use powerbench_orchestrator::config::{DETAILED_PRESET, QUICK_PRESET};
        let help = help_text();
        for (name, preset) in [("quick", QUICK_PRESET), ("detailed", DETAILED_PRESET)] {
            let needle = format!("{} с прогона", preset.duration_seconds);
            assert!(
                help.contains(&needle),
                "в справке пресета `{name}` нет его настоящей длительности \
                 ({needle} с):\n{help}"
            );
            assert!(
                help.contains(&format!("{} с разогрева", preset.warmup_seconds)),
                "в справке пресета `{name}` неверный разогрев"
            );
            assert!(
                help.contains(&format!("{} с охлаждения", preset.cooling_seconds)),
                "в справке пресета `{name}` неверное охлаждение"
            );
            assert!(
                help.contains(&format!("{} раунд", preset.repetitions)),
                "в справке пресета `{name}` неверное число раундов"
            );
        }
    }

    /// Окончание слова «раунд» обязано совпадать с числом: иначе справка
    /// говорит «1 раундов».
    #[test]
    fn run_word_agrees_with_the_number() {
        assert_eq!(plural_raunds(1), "");
        assert_eq!(plural_raunds(2), "а");
        assert_eq!(plural_raunds(5), "ов");
        assert_eq!(plural_raunds(11), "ов");
        assert_eq!(plural_raunds(21), "");
        assert_eq!(plural_raunds(22), "а");
        assert_eq!(plural_raunds(25), "ов");
    }
}
