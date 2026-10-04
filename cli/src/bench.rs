//! Консольный бенч (Этап 5): подкоманды `list`, `bench`, `resume`, `status`.
//!
//! `list` — список схем питания (активная помечается `*`);
//! `bench` — полный тест: самопроверка ядра, выбор схем, сессия с контрольными
//! точками, итоговый JSON;
//! `resume` — продолжение прерванной сессии из контрольной точки;
//! `status` — состояние сохранённой контрольной точки.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use powerbench_core::engine::Engine;
use powerbench_metrics::AggregateResult;
use powerbench_orchestrator::checkpoint::StoredRun;
use powerbench_orchestrator::config::{Preset, SessionConfig, validate_config};
use powerbench_orchestrator::result::{IdentityJson, RecommendationScheme, build_session_json};
use powerbench_orchestrator::session::{
    DiskCheckpointStore, RealSchemeDriver, SessionError, SessionEvent, run_session,
    session_signature,
};
use powerbench_recommend::{EvidenceLevel, Recommendation};
use powerbench_windows::power::{SleepGuard, is_admin};

/// Пресет сценария по имени.
fn preset(preset_name: &str) -> Option<Preset> {
    match preset_name {
        "quick" => Some(powerbench_orchestrator::config::QUICK_PRESET),
        "detailed" => Some(powerbench_orchestrator::config::DETAILED_PRESET),
        _ => None,
    }
}

/// Простой курсор по списку аргументов.
struct ArgCursor {
    args: Vec<String>,
    i: usize,
}

impl ArgCursor {
    fn new(args: &[String]) -> Self {
        Self {
            args: args.to_vec(),
            i: 0,
        }
    }

    fn next(&mut self) -> Option<String> {
        let v = self.args.get(self.i).cloned();
        self.i += 1;
        v
    }

    fn value(&mut self, option: &str) -> Result<String, String> {
        match self.next() {
            Some(v) if !v.starts_with("--") => Ok(v),
            _ => Err(format!("опция {option} требует значение")),
        }
    }
}

/// Параметры команд `bench` и `resume`.
struct BenchCli {
    preset: Option<Preset>,
    duration: Option<u64>,
    warmup: Option<u64>,
    cooling: Option<u64>,
    reps: Option<u32>,
    threshold: Option<f64>,
    allow_dirty: bool,
    workers: Option<usize>,
    schemes: Option<Vec<String>>,
    plan: Option<String>,
    out: PathBuf,
}

impl BenchCli {
    fn parse(args: &[String], for_resume: bool) -> Result<BenchCli, String> {
        let mut cursor = ArgCursor::new(args);
        let mut cli = BenchCli {
            preset: None,
            duration: None,
            warmup: None,
            cooling: None,
            reps: None,
            threshold: None,
            allow_dirty: false,
            workers: None,
            schemes: None,
            plan: None,
            out: crate::default_result_path(),
        };
        while let Some(arg) = cursor.next() {
            match arg.as_str() {
                "--preset" => {
                    let name = cursor.value("--preset")?;
                    match preset(&name) {
                        Some(p) => cli.preset = Some(p),
                        None => {
                            return Err(format!(
                                "неизвестный пресет «{name}» (ожидается quick или detailed)"
                            ));
                        }
                    }
                }
                "--duration" => {
                    cli.duration = Some(parse_u64("--duration", &cursor.value("--duration")?)?)
                }
                "--warmup" => cli.warmup = Some(parse_u64("--warmup", &cursor.value("--warmup")?)?),
                "--cooling" => {
                    cli.cooling = Some(parse_u64("--cooling", &cursor.value("--cooling")?)?)
                }
                "--reps" => cli.reps = Some(parse_u32("--reps", &cursor.value("--reps")?)?),
                "--threshold" => {
                    cli.threshold = Some(parse_f64("--threshold", &cursor.value("--threshold")?)?)
                }
                "--workers" => {
                    cli.workers = Some(parse_usize("--workers", &cursor.value("--workers")?)?)
                }
                // Аналог кнопки «продолжить с риском»: гейт по фону выключен,
                // замер идёт на загруженной машине. Остальные проверки (админ,
                // сеть, целостность схемы) флаг не отменяет.
                "--allow-dirty-background" => cli.allow_dirty = true,
                "--schemes" => {
                    let raw = cursor.value("--schemes")?;
                    let ids: Vec<String> = raw
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    if ids.is_empty() {
                        return Err("--schemes требует хотя бы одну схему".to_string());
                    }
                    cli.schemes = Some(ids);
                }
                "--plan" => cli.plan = Some(cursor.value("--plan")?),
                "--out" => cli.out = PathBuf::from(cursor.value("--out")?),
                other => return Err(format!("неизвестная опция «{other}»")),
            }
        }
        if for_resume && (cli.schemes.is_some() || cli.plan.is_some()) {
            return Err(
                "resume использует план контрольной точки: опции --schemes и --plan не поддерживаются"
                    .to_string(),
            );
        }
        Ok(cli)
    }
}

/// «да»/«нет» для вывода состояния.
fn yes_no(v: bool) -> &'static str {
    if v { "да" } else { "нет" }
}

fn parse_u64(option: &str, v: &str) -> Result<u64, String> {
    v.parse()
        .map_err(|_| format!("{option}: ожидалось целое число, получено «{v}»"))
}

fn parse_u32(option: &str, v: &str) -> Result<u32, String> {
    v.parse().map_err(|_| {
        format!(
            "{option}: ожидалось целое число (0–{}), получено «{v}»",
            u32::MAX
        )
    })
}

fn parse_usize(option: &str, v: &str) -> Result<usize, String> {
    v.parse()
        .map_err(|_| format!("{option}: ожидалось целое число, получено «{v}»"))
}

fn parse_f64(option: &str, v: &str) -> Result<f64, String> {
    v.parse()
        .map_err(|_| format!("{option}: ожидалось число, получено «{v}»"))
}

/// План для команды `bench`.
fn build_plan(cli: &BenchCli, scheme_ids: &[String]) -> Result<SessionConfig, String> {
    let p = cli
        .preset
        .unwrap_or(powerbench_orchestrator::config::DETAILED_PRESET);
    // Эталон по умолчанию — активная схема: она и так мерится в каждом
    // раунде, и по её прогонам видно, насколько «плывёт» машина за сессию.
    //
    // Раньше CLI оставлял эталон пустым, и дрейф не оценивался вовсе: на тех
    // же данных приложение и CLI выдавали разные вердикты. Берём активную
    // схему, но только если она реально участвует в сравнении — иначе
    // эталон был бы прогоном со схемы, которой нет в плане.
    let active = powerbench_windows::powercfg::active_scheme().ok();
    let reference_scheme_id = active.as_ref().and_then(|guid| {
        scheme_ids
            .iter()
            .any(|s| s.eq_ignore_ascii_case(guid))
            .then(|| guid.clone())
    });
    if reference_scheme_id.is_none()
        && let Some(active) = active.as_ref()
    {
        eprintln!(
            "PowerBench CLI: активная схема {active} не участвует в сравнении, \
             дрейф машины оцениваться не будет."
        );
    }
    let plan = SessionConfig {
        duration_seconds: cli.duration.unwrap_or(p.duration_seconds),
        warmup_seconds: cli.warmup.unwrap_or(p.warmup_seconds),
        cooling_seconds: cli.cooling.unwrap_or(p.cooling_seconds),
        repetitions: cli.reps.unwrap_or(p.repetitions),
        background_threshold_percent: cli
            .threshold
            .unwrap_or(powerbench_orchestrator::config::DEFAULT_BACKGROUND_PERCENT),
        accept_dirty_background: cli.allow_dirty,
        worker_count: cli.workers,
        scheme_ids: scheme_ids.to_vec(),
        plan_guid: cli.plan.clone().unwrap_or_else(crate::new_plan_guid),
        reference_scheme_id,
    };
    match validate_config(&plan) {
        Some(reason) => Err(reason),
        None => Ok(plan),
    }
}

/// Интерактивный выбор схем питания (если --schemes не задан).
fn select_schemes_interactive() -> Result<Vec<String>, String> {
    let schemes = powerbench_windows::powercfg::list_schemes().map_err(|e| e.message)?;
    if schemes.is_empty() {
        return Err("не найдено ни одной схемы управления электропитанием".to_string());
    }
    let active_index = schemes.iter().position(|s| s.active).unwrap_or(0);
    println!("Доступные схемы питания:");
    for (i, s) in schemes.iter().enumerate() {
        let marker = if s.active { " *" } else { "" };
        println!("  {}. {} ({}){marker}", i + 1, s.name, s.guid);
    }
    print!(
        "Выберите номера схем через запятую (Enter — активная «{}»): ",
        schemes[active_index].name
    );
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| format!("ошибка чтения ввода: {e}"))?;
    let chosen: Vec<usize> = if line.trim().is_empty() {
        vec![active_index + 1]
    } else {
        line.split(',')
            .map(|s| {
                s.trim()
                    .parse::<usize>()
                    .map_err(|_| format!("«{s}» не является номером"))
            })
            .collect::<Result<_, _>>()?
    };
    let mut ids: Vec<String> = Vec::new();
    for idx in chosen {
        let i = idx
            .checked_sub(1)
            .ok_or_else(|| format!("номер схемы должен быть ≥ 1, получено {idx}"))?;
        let s = schemes
            .get(i)
            .ok_or_else(|| format!("нет схемы с номером {idx}"))?;
        if !ids.contains(&s.guid) {
            ids.push(s.guid.clone());
        }
    }
    Ok(ids)
}

/// Движок с обязательной самопроверкой ядра перед измерением.
fn prepare_engine(workers: Option<usize>) -> Result<Engine, String> {
    let mut engine = Engine::new(workers);
    engine
        .self_check()
        .map_err(|e| format!("самопроверка ядра не прошла: {e:?}"))?;
    Ok(engine)
}

/// Псевдо-агрегат для схемы без прогонов (для вывода в таблице).
fn empty_aggregate() -> AggregateResult {
    AggregateResult {
        runs: 0,
        mean_average_throughput: f64::NAN,
        sample_std: 0.0,
        t_value: 0.0,
        margin: 0.0,
        ci_95: [0.0, 0.0],
        run_variation_percent: 0.0,
        cv_warning: false,
        median_throughput: 0.0,
        median_p1_throughput: 0.0,
        median_p01_throughput: 0.0,
        median_p95_execution_time_ms: 0.0,
        median_p99_execution_time_ms: 0.0,
        median_consistency_percent: 0.0,
        median_burst_retention_percent: 0.0,
        median_jitter_p99_ms: 0.0,
        median_worst_window_throughput: 0.0,
        median_background_purity: None,
        run_duration_ms: 0,
        started_at_min_ns: 0,
    }
}

/// Снимать ли контрольную точку по итогам сессии.
///
/// Только завершённая: её результат уже записан, и точка обязана исчезнуть,
/// иначе следующий `powerbench-cli bench` упрётся в `CheckpointPlanMismatch` —
/// на диске лежит точка прошлого плана, и новый считается чужим. Раньше CLI
/// не удалял её никогда, и повторный замер был невозможен.
///
/// Прерванная сессия, наоборот, сохраняется: её есть смысл продолжить
/// (`resume`) или сбросить вручную.
fn checkpoint_should_be_cleared(cancelled: bool) -> bool {
    !cancelled
}

/// Единый финал сессии: протокол, сводная таблица, рекомендация, JSON.
fn finish_session(
    mut engine: Engine,
    plan: SessionConfig,
    store: &mut DiskCheckpointStore,
    out: &Path,
) -> ExitCode {
    let outcome = match run_session(
        &mut engine,
        &RealSchemeDriver,
        plan,
        crate::ctrlc::stop(),
        store,
        None,
    ) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("PowerBench CLI: ошибка сессии: {e}");
            // Точка от другого плана — самая частая причина отказа, и
            // общий совет «продолжи сессию» тут бесполезен: продолжать нечего,
            // чужой план уже отработан. Сказать, что делать конкретно.
            if let SessionError::CheckpointPlanMismatch { found, .. } = &e {
                eprintln!(
                    "PowerBench CLI: на диске лежит контрольная точка плана {found}. \
                     Если он не дополнен — продолжить: powerbench-cli resume. \
                     Если он завершён (точка осталась с прошлого успешного прогона) — \
                     удалите файл {} в каталоге данных PowerBench и запустите заново",
                    powerbench_orchestrator::checkpoint::CHECKPOINT_FILE_NAME,
                );
            } else {
                eprintln!(
                    "PowerBench CLI: сессию при необходимости можно продолжить: \
                     powerbench-cli resume"
                );
            }
            return ExitCode::FAILURE;
        }
    };

    for event in &outcome.events {
        println!("  {event}");
    }
    if outcome.cancelled {
        println!("Сессия остановлена пользователем; контрольная точка сохранена.");
    }

    println!();
    println!("Схема | Имя | Прогоны | Средний throughput | CV | Примечание");
    for (scheme_id, agg) in &outcome.aggregates {
        let note = if outcome.rejection_reasons.contains_key(scheme_id) {
            "забракована"
        } else {
            ""
        };
        let name = outcome
            .checkpoint
            .runs
            .iter()
            .find(|r| r.scheme_id.eq_ignore_ascii_case(scheme_id))
            .and_then(|r| r.scheme_name.clone());
        println!(
            "{scheme_id} | {} | {} | {:.2} | {:.2}% | {note}",
            name.as_deref().unwrap_or("—"),
            agg.runs,
            agg.mean_average_throughput,
            agg.run_variation_percent
        );
    }

    let rec = outcome.recommendation.clone().unwrap_or(Recommendation {
        level: EvidenceLevel::None,
        recommended_scheme: None,
        runner_up_scheme: None,
        reason: "рекомендация не сформирована".to_string(),
        three_probabilities: None,
        expected_margin_percent: None,
        bootstrap_mode: None,
        tie_criterion: None,
    });
    println!();
    // Уровень до понижения здесь намеренно не печатается: он не учитывает
    // дрейф, фон и число прогонов, поэтому рядом с итоговым вердиктом два
    // разных уровня выглядели бы как противоречие.
    println!("Рекомендация: {}", rec.reason);
    if let Some(id) = &rec.recommended_scheme {
        let rname = outcome
            .checkpoint
            .runs
            .iter()
            .find(|r| r.scheme_id.eq_ignore_ascii_case(id))
            .and_then(|r| r.scheme_name.clone());
        println!(
            "  рекомендованная схема: {id} ({})",
            rname.as_deref().unwrap_or("—")
        );
    }
    if let Some(p) = rec.three_probabilities {
        println!(
            "  P(лучший)={:.2} P(перевес>0)={:.2} P(перевес>1%)={:.2}",
            p[0], p[1], p[2]
        );
    }

    // Рекомендация → описание схем для JSON (включая забракованные).
    let recommendation_schemes: Vec<RecommendationScheme> = outcome
        .checkpoint
        .plan
        .scheme_ids
        .iter()
        .map(|id| {
            let agg = outcome
                .aggregates
                .iter()
                .find(|(sid, _)| sid.eq_ignore_ascii_case(id))
                .map(|(_, a)| a.clone())
                .unwrap_or_else(empty_aggregate);
            // GUID сравниваем регистронезависимо, как Checkpoint::is_rejected.
            let rejected = outcome
                .rejection_reasons
                .keys()
                .any(|k| k.eq_ignore_ascii_case(id));
            let reason = outcome
                .rejection_reasons
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(id))
                .map(|(_, v)| v.clone());
            let per_run: Vec<StoredRun> = outcome
                .checkpoint
                .runs
                .iter()
                .filter(|r| r.scheme_id.eq_ignore_ascii_case(id))
                .cloned()
                .collect();
            (id.clone(), rejected, reason, agg, per_run)
        })
        .collect();

    let warnings: Vec<String> = outcome
        .events
        .iter()
        .filter_map(|e| match e {
            SessionEvent::Warn(w) => Some(w.clone()),
            _ => None,
        })
        .collect();
    let json = build_session_json(
        &outcome.checkpoint,
        identity_of(&engine),
        outcome
            .aggregates
            .iter()
            .map(|(id, a)| (id.clone(), a.clone()))
            .collect(),
        &recommendation_schemes,
        &rec,
        warnings,
        outcome.cancelled,
        powerbench_orchestrator::result::default_score_weights(),
        outcome.early_stop_reason,
    );
    if let Err(e) = crate::write_json(&json, out) {
        eprintln!("PowerBench CLI: {e}");
        eprintln!(
            "PowerBench CLI: контрольная точка сохранена — прогон можно повторить \
             командой powerbench-cli resume, либо удалить файл {} и начать заново",
            powerbench_orchestrator::checkpoint::CHECKPOINT_FILE_NAME,
        );
        return ExitCode::FAILURE;
    }
    // Итоговые условия замера печатаем из готового JSON: раньше CLI показывал
    // уровень до понижения (дрейф, фон, скрининг), и на экране и в файле
    // получались разные вердикты.
    println!(
        "Итоговый вердикт [{}]{}",
        json.recommendation.level_label,
        if json.screening {
            " — режим скрининга"
        } else {
            ""
        }
    );
    if let Some(r) = &json.reference {
        println!("{}", r.note());
    }
    for w in json
        .warnings
        .iter()
        .filter(|w| w.contains("фон") || w.contains("троттлинг") || w.contains("нестабильна"))
    {
        println!("Предупреждение: {w}");
    }
    println!("Итоговый результат: {}", out.display());
    // История (Этап 6): завершённая сессия сохраняется в `Results\`
    // автоматически; неудача здесь не отменяет успех основной записи.
    match powerbench_orchestrator::history::save_result(&json) {
        Ok(path) => println!("История: {}", path.display()),
        Err(e) => eprintln!("PowerBench CLI: предупреждение: не удалось сохранить в историю: {e}"),
    }
    // HTML-отчёт пишется рядом с JSON. Раньше CLI оставлял после замера только
    // цифры в консоли, и посмотреть разбор сессии можно было лишь в приложении.
    match write_report_next_to(&json) {
        Ok(path) => println!("HTML-отчёт: {path}"),
        Err(e) => eprintln!("PowerBench CLI: предупреждение: не удалось сохранить HTML-отчёт: {e}"),
    }
    // Точка долела своё: сессия записана в историю, результат на диске.
    //
    // Раньше CLI её не удалял, и повторный `powerbench-cli bench` падал с
    // `CheckpointPlanMismatch`: на диске лежала точка прошлого плана, и новый
    // план считался «чужим». Приложение давно ведёт себя так же.
    //
    // Незавершённая (отменённая) сессия — наоборот: её точку сохраняем, и
    // пользователю нужна подсказка, как её продолжить или сбросить.
    if checkpoint_should_be_cleared(outcome.cancelled) {
        match powerbench_orchestrator::checkpoint::clear_checkpoint() {
            Ok(()) => {}
            // Неудача удаления не отменяет успех основной записи, но сказать
            // о ней обязаны: иначе следующий запуск упрётся ровно в ту же
            // ошибку, ради которой точку снимали.
            Err(e) => eprintln!(
                "PowerBench CLI: предупреждение: не удалось очистить контрольную точку: {e}. \
                 Следующий запуск может остановиться с «контрольная точка другого плана» — \
                 удалите файл {} вручную",
                powerbench_orchestrator::checkpoint::CHECKPOINT_FILE_NAME,
            ),
        }
    } else {
        println!(
            "Контрольная точка сохранена. Продолжить: powerbench-cli resume. \
             Начать заново: удалите файл {} в каталоге данных PowerBench.",
            powerbench_orchestrator::checkpoint::CHECKPOINT_FILE_NAME,
        );
    }
    ExitCode::SUCCESS
}

/// Записать HTML-отчёт сессии рядом с её JSON.
fn write_report_next_to(
    json: &powerbench_orchestrator::result::SessionJson,
) -> Result<String, String> {
    use powerbench_orchestrator::history;
    let dir = history::results_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("не удалось создать {}: {e}", dir.display()))?;
    let start = history::session_started_at_ns(json).unwrap_or(0);
    let stamp = history::date_time_stamp(start);
    let plan = history::sanitize(&json.plan_guid);
    let path = dir.join(format!("PowerBench-Session_{plan}_{stamp}.html"));
    let html = powerbench_orchestrator::report::build_session_report(json);
    std::fs::write(&path, html).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path.display().to_string())
}

fn identity_of(engine: &Engine) -> IdentityJson {
    let sig = session_signature(engine);
    IdentityJson {
        workload_version: sig.workload_version,
        config_hash: sig.config_hash,
        seed_hex: format!("{:016X}", sig.seed),
        worker_count: sig.worker_count,
        logical_cpus: sig.logical_cpus,
        timer_hz: sig.timer_hz,
        cpu_identifier: sig.cpu_identifier,
        diagnostics_version: sig.diagnostics_version,
        os_build: powerbench_windows::power::os_build(),
        memory_gib: powerbench_windows::power::memory_gib(),
        cpu_brand: powerbench_windows::power::cpu_brand(),
        affinity_mode: sig.affinity_mode,
        affinity_signature: sig.affinity_signature,
    }
}

/// `powerbench-cli list`.
pub fn cmd_list(_args: &[String]) -> ExitCode {
    match powerbench_windows::powercfg::list_schemes() {
        Ok(schemes) => {
            for s in schemes {
                println!("{} {} {}", if s.active { "*" } else { " " }, s.guid, s.name);
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!(
                "PowerBench CLI: не удалось получить список схем: {}",
                e.message
            );
            ExitCode::FAILURE
        }
    }
}

/// Нужен ли интерактивный выбор схем питания.
///
/// Явный `--schemes` пустым быть не может (парсер отвергает), но пустой срез
/// приходит из тестов и из `unwrap_or(&[])` в вызывающем коде — такой случай
/// обязан доходить до выбора, а не до ошибки валидации.
fn needs_interactive_selection(explicit: Option<&[String]>) -> bool {
    explicit.is_none_or(<[String]>::is_empty)
}

/// `powerbench-cli bench [...опции]`.
pub fn cmd_bench(args: &[String]) -> ExitCode {
    let cli = match BenchCli::parse(args, false) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("PowerBench CLI: {e}");
            return ExitCode::FAILURE;
        }
    };
    if !is_admin() {
        eprintln!(
            "PowerBench CLI: требуются права администратора (запустите от имени администратора)."
        );
        return ExitCode::FAILURE;
    }
    // Схемы выбираем ДО `build_plan`.
    //
    // `validate_config` отвергает план с пустым списком схем («выберите хотя бы
    // одну схему питания»), а прежний порядок звал `build_plan` первым. В
    // результате `bench` без `--schemes` всегда падал, и интерактивный выбор
    // был недостижим вовсе — единственный способ указать схему был явный флаг.
    //
    // Побочная польза порядка: `build_plan` сам выбирает эталон дрейфа из
    // переданного списка, поэтому при интерактивном выборе эталон тоже
    // определяется сразу — раньше его приходилось досочинять вручную.
    let scheme_ids: Vec<String> = if needs_interactive_selection(cli.schemes.as_deref()) {
        match select_schemes_interactive() {
            Ok(ids) => ids,
            Err(e) => {
                eprintln!("PowerBench CLI: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        cli.schemes.clone().unwrap_or_default()
    };
    let plan = match build_plan(&cli, &scheme_ids) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("PowerBench CLI: {e}");
            return ExitCode::FAILURE;
        }
    };
    crate::ctrlc::install();
    // Восстановление исходной схемы после прошлого прерывания (если было).
    {
        use powerbench_orchestrator::recovery::recover_interrupted_session;
        let outcome = recover_interrupted_session(&RealSchemeDriver);
        if outcome.interrupted_checkpoint && !outcome.already_ok {
            eprintln!(
                "PowerBench CLI: восстановление после прерывания: restored={} {:?}",
                outcome.restored, outcome.error
            );
        }
    }
    let engine = match prepare_engine(cli.workers) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("PowerBench CLI: {e}");
            return ExitCode::FAILURE;
        }
    };
    let sleep_guard = match SleepGuard::prevent() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("PowerBench CLI: не удалось запретить сон и отключение дисплея: {e:?}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "PowerBench CLI: сессия {guid}: {schemes} схем, раундов {reps}",
        guid = plan.plan_guid,
        schemes = plan.scheme_ids.len(),
        reps = plan.repetitions
    );
    println!(
        "  длительность {} с (фазы: Лёгкая/Частичная/Тяжёлая/Отклик), разогрев {} с, охлаждение {} с",
        plan.duration_seconds, plan.warmup_seconds, plan.cooling_seconds
    );
    println!("  запрет сна включён; активная схема будет восстановлена после теста.");
    let _guard = sleep_guard;
    let mut store = DiskCheckpointStore;
    finish_session(engine, plan, &mut store, &cli.out)
}

/// `powerbench-cli resume [--out путь]`.
pub fn cmd_resume(args: &[String]) -> ExitCode {
    let cli = match BenchCli::parse(args, true) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("PowerBench CLI: {e}");
            return ExitCode::FAILURE;
        }
    };
    if !is_admin() {
        eprintln!("PowerBench CLI: требуются права администратора.");
        return ExitCode::FAILURE;
    }
    // План берётся из контрольной точки — иначе нечего продолжать.
    // Нечитаемый план — это ошибка, а не «сессии нет»: раньше битый файл
    // выглядел как отсутствующая точка, и resume стартовал с нуля молча.
    let plan = match powerbench_orchestrator::checkpoint::load_checkpoint() {
        Ok(Some(cp)) => cp.plan,
        Ok(None) => {
            eprintln!("PowerBench CLI: нет сохранённой контрольной точки (запустите bench).");
            return ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("PowerBench CLI: контрольная точка не читается: {e}");
            return ExitCode::FAILURE;
        }
    };
    crate::ctrlc::install();
    let engine = match prepare_engine(cli.workers) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("PowerBench CLI: {e}");
            return ExitCode::FAILURE;
        }
    };
    let sleep_guard = match SleepGuard::prevent() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("PowerBench CLI: не удалось запретить сон: {e:?}");
            return ExitCode::FAILURE;
        }
    };
    let _guard = sleep_guard;
    let mut store = DiskCheckpointStore;
    finish_session(engine, plan, &mut store, &cli.out)
}

/// `powerbench-cli status`.
pub fn cmd_status(_args: &[String]) -> ExitCode {
    let cp = match powerbench_orchestrator::checkpoint::load_checkpoint() {
        Ok(cp) => cp,
        Err(e) => {
            eprintln!("PowerBench CLI: контрольная точка не читается: {e}");
            return ExitCode::FAILURE;
        }
    };
    match cp {
        None => {
            println!("Контрольная точка отсутствует.");
            ExitCode::SUCCESS
        }
        Some(cp) => {
            println!("План: {}", cp.plan.plan_guid);
            println!(
                "Схемы: {}; повторений до: {}; длительность {} с",
                cp.plan.scheme_ids.len(),
                cp.plan.repetitions,
                cp.plan.duration_seconds
            );
            println!("Выполнено прогонов: {}", cp.runs.len());
            for (sid, reason) in &cp.rejections {
                println!("Забракована схема {sid}: {reason}");
            }
            println!(
                "Исходная схема: {}",
                cp.original_scheme_guid.as_deref().unwrap_or("-")
            );
            println!(
                "Исходная схема восстановлена: {}",
                if cp.original_restored {
                    "да"
                } else {
                    "нет"
                }
            );
            ExitCode::SUCCESS
        }
    }
}

/// `powerbench-cli settings show` — текущие настройки (дефолт, если файла нет).
pub fn cmd_settings_show(_args: &[String]) -> ExitCode {
    use powerbench_orchestrator::appsettings::{self, AppSettings};
    let s = AppSettings::load();
    let path = appsettings::appsettings_path();
    let src = if path.exists() {
        path.display().to_string()
    } else {
        "<дефолт, файла нет>".to_string()
    };
    println!("Файл: {src}");
    println!(
        "benchmark: порог фона {}",
        s.benchmark.background_threshold_percent
    );
    // Состояние машины: без него нельзя понять, сопоставимы ли две сессии,
    // и не объяснить, почему замер вдруг замедлился.
    let p = powerbench_windows::power::power_state();
    let brand = powerbench_windows::power::cpu_brand();
    let id = powerbench_windows::power::cpu_identifier();
    // Бренд — для человека, идентификатор — для сравнения сессий между собой.
    let cpu = if brand.is_empty() {
        id.clone()
    } else {
        format!("{brand} ({id})")
    };
    println!(
        "система: ОС {}, память {:.1} ГБ, CPU {}",
        {
            let b = powerbench_windows::power::os_build();
            if b.is_empty() { "—".to_string() } else { b }
        },
        powerbench_windows::power::memory_gib(),
        cpu
    );
    println!(
        "питание: {}, частота {}/{} МГц, троттлинг {}, термолимит {}, ACPI-причина {}",
        if p.on_ac {
            "от сети"
        } else {
            "от батареи"
        },
        p.current_mhz,
        p.max_mhz,
        yes_no(p.throttled),
        yes_no(p.thermal_throttle),
        p.policy_reason
    );
    println!(
        "appearance: тема «{}», режим «{}», reduceMotion {}, sidebarCollapsed {}",
        s.appearance.theme,
        s.appearance.mode,
        s.appearance.reduce_motion,
        s.appearance.sidebar_collapsed
    );
    let fav = if s.favorite_schemes.is_empty() {
        "-".to_string()
    } else {
        s.favorite_schemes.join(", ")
    };
    let excl = if s.excluded_schemes.is_empty() {
        "-".to_string()
    } else {
        s.excluded_schemes.join(", ")
    };
    println!("favorite_schemes: {fav}");
    println!("excluded_schemes: {excl}");
    ExitCode::SUCCESS
}

/// `powerbench-cli settings reset` — записать дефолтный appsettings.json (атомарно),
/// если его ещё нет (существующий не трогаем).
pub fn cmd_settings_reset(_args: &[String]) -> ExitCode {
    use powerbench_orchestrator::appsettings::{self, AppSettings};
    if appsettings::appsettings_path().exists() {
        println!(
            "Настройки уже существуют: {}",
            appsettings::appsettings_path().display()
        );
        return ExitCode::SUCCESS;
    }
    match AppSettings::default().save() {
        Ok(()) => {
            println!(
                "Записан дефолтный appsettings.json: {}",
                appsettings::appsettings_path().display()
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("PowerBench CLI: не удалось записать настройки: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `powerbench-cli history list` — история завершённых сессий.
pub fn cmd_history_list(_args: &[String]) -> ExitCode {
    let entries = match powerbench_orchestrator::history::list_results() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("PowerBench CLI: не удалось прочитать историю: {e}");
            return ExitCode::FAILURE;
        }
    };
    if entries.is_empty() {
        println!("История пуста.");
        return ExitCode::SUCCESS;
    }
    println!("Старт (UTC)               | План                | Схем | Уровень");
    for entry in entries {
        match powerbench_orchestrator::history::load_result(&entry.path) {
            Ok(s) => {
                let start = powerbench_orchestrator::history::session_started_at_ns(&s)
                    .map(powerbench_orchestrator::history::date_time_stamp)
                    .unwrap_or_default();
                println!(
                    "{start} | {} | {} | {}",
                    s.plan_guid,
                    s.schemes.len(),
                    s.recommendation.level_label
                );
            }
            Err(e) => {
                println!("{} | <не читается: {e}>", entry.file_name);
            }
        }
    }
    ExitCode::SUCCESS
}

/// `powerbench-cli history export <plan-guid|all> [--format json|csv] [--out путь]`.
pub fn cmd_history_export(args: &[String]) -> ExitCode {
    let mut cursor = ArgCursor::new(args);
    let what = match cursor.next() {
        Some(v) if !v.starts_with("--") => v,
        _ => {
            eprintln!("PowerBench CLI: укажите plan-guid или all.");
            return ExitCode::FAILURE;
        }
    };
    let mut format = "json".to_string();
    let mut out: Option<PathBuf> = None;
    while let Some(arg) = cursor.next() {
        match arg.as_str() {
            "--format" => match cursor.value("--format") {
                Ok(v) if v == "json" || v == "csv" => format = v,
                Ok(v) => {
                    eprintln!("PowerBench CLI: неизвестный формат «{v}» (ожидается json или csv).");
                    return ExitCode::FAILURE;
                }
                Err(e) => {
                    eprintln!("PowerBench CLI: {e}");
                    return ExitCode::FAILURE;
                }
            },
            "--out" => match cursor.value("--out") {
                Ok(v) => out = Some(PathBuf::from(v)),
                Err(e) => {
                    eprintln!("PowerBench CLI: {e}");
                    return ExitCode::FAILURE;
                }
            },
            other => {
                eprintln!("PowerBench CLI: неизвестная опция «{other}».");
                return ExitCode::FAILURE;
            }
        }
    }

    use powerbench_orchestrator::history::{export_csv_to, export_json, list_results, load_result};
    let entries = match list_results() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("PowerBench CLI: не удалось прочитать историю: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut sessions: Vec<(String, powerbench_orchestrator::result::SessionJson)> = Vec::new();
    for entry in entries {
        let s = match load_result(&entry.path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "PowerBench CLI: пропускаю «{}» (не читается: {e})",
                    entry.file_name
                );
                continue;
            }
        };
        if what == "all" || s.plan_guid.eq_ignore_ascii_case(&what) {
            sessions.push((entry.file_name, s));
        }
    }
    if sessions.is_empty() {
        eprintln!("PowerBench CLI: записей для «{what}» не найдено.");
        return ExitCode::FAILURE;
    }

    if what == "all" {
        // Все записи — в каталог `--out` (по файлу на сессию).
        let dir = out.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        if let Err(e) = std::fs::create_dir_all(&dir) {
            eprintln!(
                "PowerBench CLI: не удалось создать «{}»: {e}",
                dir.display()
            );
            return ExitCode::FAILURE;
        }
        for (_, s) in &sessions {
            // plan_guid — из содержимого файла: без sanitize возможен traversal.
            let stem = powerbench_orchestrator::history::sanitize(&s.plan_guid);
            let file = dir.join(format!("{stem}.{format}"));
            let r: Result<(), String> = if format == "json" {
                export_json(s, &file).map_err(|e| e.to_string())
            } else {
                export_csv_to(s, &file).map(|_| ())
            };
            match r {
                Ok(()) => println!("Экспорт: {}", file.display()),
                Err(e) => {
                    eprintln!(
                        "PowerBench CLI: не удалось экспортировать «{}»: {e}",
                        file.display()
                    );
                    return ExitCode::FAILURE;
                }
            }
        }
        return ExitCode::SUCCESS;
    }

    let (_, s) = &sessions[0];
    let path = out.unwrap_or_else(|| PathBuf::from(format!("export.{format}")));
    let r: Result<(), String> = if format == "json" {
        export_json(s, &path).map_err(|e| e.to_string())
    } else {
        export_csv_to(s, &path).map(|_| ())
    };
    match r {
        Ok(()) => {
            println!("Экспорт: {}", path.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("PowerBench CLI: не удалось экспортировать: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{checkpoint_should_be_cleared, needs_interactive_selection};

    /// Регресс H42: успешная сессия обязана снимать свою контрольную точку.
    /// CLI её не удалял, и повторный `powerbench-cli bench` падал с
    /// `CheckpointPlanMismatch` — на диске лежала точка прошлого плана, и новый
    /// план считался чужим. Второй замер на этой машине был невозможен.
    #[test]
    fn a_finished_session_clears_its_checkpoint() {
        assert!(
            checkpoint_should_be_cleared(false),
            "точка завершённой сессии остаётся на диске и блокирует следующий запуск"
        );
    }

    /// Прерванная сессия — наоборот: её точку сохраняем, иначе `resume` будет
    /// нечего продолжать.
    #[test]
    fn an_unfinished_session_keeps_its_checkpoint() {
        assert!(
            !checkpoint_should_be_cleared(true),
            "точка отменённой сессии удалена: продолжить прогон нечем"
        );
    }

    /// Правило обязано быть именно тем, о котором говорит H42, и подсказка
    /// про `--resume` / ручной сброс — на месте.
    ///
    /// Проверяется по исходнику: `finish_session` требует живого движка и прав
    /// администратора, а регресс здесь молчаливый — код компилируется и
    /// работает, пока не встретит именно этот случай.
    #[test]
    fn finish_session_clears_the_checkpoint_and_explains_the_alternative() {
        let src = include_str!("bench.rs");
        let start = src
            .find("fn finish_session(")
            .expect("не найдена finish_session");
        let body = &src[start..];
        let end = body
            .find("\n/// Записать HTML-отчёт сессии")
            .expect("не найден конец finish_session");
        let body = &body[..end];

        assert!(
            body.contains("checkpoint::clear_checkpoint()"),
            "finish_session не снимает контрольную точку: повторный bench упадёт \
             с CheckpointPlanMismatch"
        );
        assert!(
            body.contains("checkpoint_should_be_cleared(outcome.cancelled)"),
            "снятие точки не связано с признаком незавершённости сессии"
        );
        assert!(
            body.contains("powerbench-cli resume"),
            "для незавершённой сессии нужна подсказка, как её продолжить"
        );
        assert!(
            body.contains("CHECKPOINT_FILE_NAME"),
            "пользователю нужно сказать, какой файл удалить при ручном сбросе"
        );
        // Сначала результат, потом точка: неудача записи JSON возвращается
        // раньше, и данные не должны пропасть вместе с точкой.
        let json_at = body
            .find("write_json(&json, out)")
            .expect("запись JSON в finish_session");
        let clear_at = body
            .find("checkpoint::clear_checkpoint()")
            .expect("снятие точки в finish_session");
        assert!(
            json_at < clear_at,
            "точка снимается раньше записи результата: неудача записи JSON \
             приведёт к потере и данных, и точки"
        );
    }

    // ------------------------------------------------------------------
    // Регресс C1: интерактивный выбор схем обязан быть достижим
    // ------------------------------------------------------------------

    /// `--schemes` без флага — интерактивный выбор; явный непустой список — нет.
    #[test]
    fn interactive_selection_is_chosen_only_without_an_explicit_list() {
        let explicit = vec!["381b4222-f694-41f0-9685-ff5bb260df2e".to_string()];
        assert!(!needs_interactive_selection(Some(&explicit)));
        // Ни флага, ни пустой список — тоже выбор (именно так приходил
        // `unwrap_or(&[])` из прежнего кода).
        assert!(needs_interactive_selection(None));
        assert!(needs_interactive_selection(Some(&[])));
    }

    /// Регресс C1: выбор схем обязан происходить ДО сборки плана.
    ///
    /// `validate_config` отвергает план с пустым списком схем, поэтому при
    /// прежнем порядке `bench` без `--schemes` всегда падал с «выберите хотя бы
    /// одну схему питания», и интерактивный выбор был недостижим вовсе.
    ///
    /// Проверяется по исходнику: `cmd_bench` требует прав администратора и
    /// живого `powercfg`, а регресс молчаливый — код компилируется и работает,
    /// пока не встретит именно этот случай.
    #[test]
    fn schemes_are_selected_before_the_plan_is_built() {
        let src = include_str!("bench.rs");
        let start = src
            .find("pub fn cmd_bench(args: &[String]) -> ExitCode {")
            .expect("не найдена cmd_bench");
        let body = &src[start..];
        let end = body
            .find("\n/// `powerbench-cli resume")
            .expect("не найден конец cmd_bench");
        let body = &body[..end];

        let select_at = body
            .find("select_schemes_interactive()")
            .expect("cmd_bench не предлагает интерактивный выбор схем");
        let build_at = body
            .find("build_plan(&cli, &scheme_ids)")
            .expect("cmd_bench не собирает план");
        assert!(
            select_at < build_at,
            "план собирается раньше выбора схем: validate_config отвергнет пустой \
             список, и выбор станет недостижим"
        );
    }
}
