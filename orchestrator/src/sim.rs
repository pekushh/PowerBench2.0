//! Стохастический симулятор сессий (только для тестов, компилируется
//! под `#[cfg(test)]`).
//!
//! Вместо реального CPU-движка, powercfg и дискового хранилища использует:
//! * `VirtualEngine` — эмулирует тики фаз с производительностью активной
//!   схемы, шумом и редкими скачками; делит общий `Scoreboard`, которым
//!   пользуются сторожевой таймер и телеметрия;
//! * `MockSchemeDriver` — in-memory список схем и активная схема;
//! * `MemoryCheckpointStore` — контрольная точка в памяти.
//!
//! Прогон всего планировщика сессии без реального железа даёт E2E-покрытие
//! ключевых сценариев (отмена, рестарт, браковка, досрочная остановка).
//! Паузы планировщика отключаются через `set_sleep_scale(0)`, фоновый
//! замер через `set_background_check_fn`.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use powerbench_core::config::{Phase, Profile, RESPONSE_SUPERCYCLE};
use powerbench_core::engine::{ProgressSnapshot, RunError, RunReport, RunTarget, Scoreboard};
use powerbench_windows::powercfg::PowerScheme;

use crate::checkpoint::Checkpoint;
use crate::config::SessionConfig;
use crate::session::{
    CheckpointStore, SessionEngine, SessionOutcome, SessionError, SessionEvent, SchemeDriver,
};

/// Детерминированный генератор splitmix64.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Равномерное [0, 1).
    pub fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Эталонная контрольная сумма первого тика виртуального движка.
const VIRTUAL_FIRST_TICK: u64 = 0x0f1e_2d3c_4b5a_6978;

/// Виртуальный движок: NULL-нагрузка с детерминированным шумом.
///
/// Производительность (медианное время тика) зависит от активной схемы:
/// значение берётся из общей с драйвером карты `perf[active_guid]`.
/// Генерация укладывается в разумный бюджет — `max_ticks` на фазу.
pub struct VirtualEngine {
    cpus: usize,
    seed: u64,
    median_tick_ms: f64,
    noise_cv: f64,
    spike_rate: f64,
    max_ticks: u64,
    rng: Rng,
    times: Vec<f64>,
    scoreboard: Arc<Scoreboard>,
    cancel: Arc<AtomicBool>,
    prepared: bool,
    perf: Arc<Mutex<BTreeMap<String, f64>>>,
    active: Arc<Mutex<String>>,
}

impl VirtualEngine {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cpus: usize,
        seed: u64,
        median_tick_ms: f64,
        noise_cv: f64,
        spike_rate: f64,
        max_ticks: u64,
        perf: Arc<Mutex<BTreeMap<String, f64>>>,
        active: Arc<Mutex<String>>,
    ) -> Self {
        Self {
            cpus,
            seed,
            median_tick_ms,
            noise_cv,
            spike_rate,
            max_ticks,
            rng: Rng::new(seed.wrapping_add(0x9e37_79b9_7f4a_7c15)),
            times: Vec::new(),
            scoreboard: Arc::new(Scoreboard::default()),
            cancel: Arc::new(AtomicBool::new(false)),
            prepared: false,
            perf,
            active,
        }
    }
}

impl SessionEngine for VirtualEngine {
    fn version(&self) -> &'static str {
        "virtual-1.0.0"
    }

    fn config_hash(&self) -> &str {
        "virtual-config"
    }

    fn seed(&self) -> u64 {
        self.seed
    }

    fn worker_count(&self) -> usize {
        self.cpus
    }

    fn logical_cpus(&self) -> usize {
        self.cpus
    }

    fn reset(&mut self) {
        self.cancel.store(false, Ordering::Relaxed);
        self.times.clear();
        self.prepared = false;
    }

    fn prepare_sample_buffer(&mut self, _phase: Phase, _duration_secs: u64) {
        self.prepared = true;
    }

    fn canceller(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }

    fn scoreboard_arc(&self) -> Arc<Scoreboard> {
        self.scoreboard.clone()
    }

    fn progress_snapshot(&self) -> ProgressSnapshot {
        self.scoreboard.snapshot()
    }

    fn run_phase(
        &mut self,
        phase: Phase,
        target: RunTarget,
    ) -> Result<RunReport, RunError> {
        if !self.prepared && floor_ticks(target) == 0 {
            return Err(RunError::Cancelled);
        }
        let perf = {
            let a = self.active.lock().unwrap();
            self.perf
                .lock()
                .unwrap()
                .get(&*a)
                .copied()
                .unwrap_or(self.median_tick_ms)
        };
        let planned = floor_ticks(target);
        let n = planned.min(self.max_ticks).max(1);
        let mut times: Vec<f64> = Vec::with_capacity(n as usize);
        self.scoreboard.set_running(true);
        for i in 0..n {
            if self.cancel.load(Ordering::Relaxed) {
                self.scoreboard.set_running(false);
                return Err(RunError::Cancelled);
            }
            let noise = 1.0 + self.noise_cv * (self.rng.unit() * 2.0 - 1.0);
            let mut t = perf * noise;
            if self.rng.unit() < self.spike_rate {
                t *= 1.7;
            }
            times.push(t);
            self.scoreboard.set_progress(
                i + 1,
                ((i as f64 * t) / 1000.0) as u64,
                (t * 1.0e6) as u64,
            );
        }
        self.scoreboard.set_running(false);
        let supercycles = if phase == Phase::Response {
            n / RESPONSE_SUPERCYCLE
        } else {
            0
        };
        let report = RunReport {
            phase,
            ticks: n,
            supercycles_completed: supercycles,
            first_tick_checksum: VIRTUAL_FIRST_TICK,
            run_checksum: times
                .iter()
                .enumerate()
                .fold(self.seed, |acc, (idx, &t)| {
                    acc.wrapping_mul(31).wrapping_add((t * 1.0e6) as u64 + idx as u64)
                }),
            samples_written: times.len(),
        };
        self.times = times;
        Ok(report)
    }

    fn samples(&self) -> &[f64] {
        &self.times
    }

    fn first_tick_reference(&self, _profile: Profile) -> Option<u64> {
        Some(VIRTUAL_FIRST_TICK)
    }
}

/// Число тиков по цели фазы (под виртуальное время тика).
fn floor_ticks(target: RunTarget) -> u64 {
    match target {
        RunTarget::Ticks(n) => n,
        RunTarget::Duration(d) => (d.as_secs_f64() / 0.00025).round() as u64,
    }
}

/// Контрольная сумма первого тика для теста авто-браковки заеданий.
const STUTTER_FIRST_TICK: u64 = 0x55aa_55aa_55aa_55aa;

/// Движок с реальным течением времени (для E2E авто-браковки заеданий).
///
/// Публикует тики в реальном времени, поэтому детектор заеданий видит
/// фактическую скорость. Номинальная длительность фаз здоровых схем сжимается
/// в 10 раз (скорость становится выше эталона — ложных срабатываний нет),
/// а фазы «заедающей» схемы (`stall_scheme`) идут без публикации тиков в
/// реальном масштабе: детектор успевает отменить прогон.
pub struct StutterEngine {
    scoreboard: Arc<Scoreboard>,
    cancel: Arc<AtomicBool>,
    prepared: bool,
    stall_scheme: String,
    active: Arc<Mutex<String>>,
    ticks_per_sec: u64,
    times: Vec<f64>,
}

impl StutterEngine {
    pub fn new(stall_scheme: String, active: Arc<Mutex<String>>) -> Self {
        Self {
            scoreboard: Arc::new(Scoreboard::default()),
            cancel: Arc::new(AtomicBool::new(false)),
            prepared: false,
            stall_scheme,
            active,
            ticks_per_sec: 20_000,
            times: Vec::new(),
        }
    }

    fn is_stall(&self) -> bool {
        *self.active.lock().unwrap() == self.stall_scheme
    }
}

impl SessionEngine for StutterEngine {
    fn version(&self) -> &'static str {
        "stutter-1.0.0"
    }

    fn config_hash(&self) -> &str {
        "stutter-config"
    }

    fn seed(&self) -> u64 {
        0x5EED
    }

    fn worker_count(&self) -> usize {
        4
    }

    fn logical_cpus(&self) -> usize {
        4
    }

    fn reset(&mut self) {
        self.cancel.store(false, Ordering::Relaxed);
        self.prepared = false;
        self.times.clear();
        self.scoreboard.set_progress(0, 0, 0);
        self.scoreboard.set_running(false);
    }

    fn prepare_sample_buffer(&mut self, _phase: Phase, _duration_secs: u64) {
        self.prepared = true;
    }

    fn canceller(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }

    fn scoreboard_arc(&self) -> Arc<Scoreboard> {
        self.scoreboard.clone()
    }

    fn progress_snapshot(&self) -> ProgressSnapshot {
        self.scoreboard.snapshot()
    }

    fn run_phase(&mut self, phase: Phase, target: RunTarget) -> Result<RunReport, RunError> {
        let secs = match target {
            RunTarget::Duration(d) => d.as_secs_f64(),
            RunTarget::Ticks(_) => return Err(RunError::Cancelled),
        };
        if !self.prepared && secs > 0.0 {
            return Err(RunError::Cancelled);
        }
        let stall = self.is_stall();
        let nominal_ticks = (secs * self.ticks_per_sec as f64) as u64;
        // Фазы «заедающей» схемы идут в реальном масштабе без публикации тиков,
        // здоровые — в 10 раз быстрее с высокой реальной скоростью.
        let wall_secs = if stall { secs } else { (secs / 10.0).max(0.05) };
        let end = Instant::now() + Duration::from_secs_f64(wall_secs);
        let step = Duration::from_millis(5);
        let batch = (nominal_ticks / 20).max(1);
        let mut ticks = 0u64;
        let mut written = 0usize;
        self.scoreboard.set_running(true);
        while Instant::now() < end {
            if self.cancel.load(Ordering::Relaxed) {
                self.scoreboard.set_running(false);
                return Err(RunError::Cancelled);
            }
            if !stall {
                ticks = ticks.saturating_add(batch);
                for _ in 0..batch {
                    self.times.push(0.25);
                    written += 1;
                }
                self.scoreboard.set_progress(ticks, ticks / self.ticks_per_sec, 250_000);
            }
            std::thread::sleep(step);
        }
        self.scoreboard.set_running(false);
        let supercycles = if phase == Phase::Response {
            ticks / RESPONSE_SUPERCYCLE
        } else {
            0
        };
        Ok(RunReport {
            phase,
            ticks,
            supercycles_completed: supercycles,
            first_tick_checksum: STUTTER_FIRST_TICK,
            run_checksum: ticks.wrapping_mul(31).wrapping_add(0xBADC0DE),
            samples_written: written,
        })
    }

    fn samples(&self) -> &[f64] {
        &self.times
    }

    fn first_tick_reference(&self, _profile: Profile) -> Option<u64> {
        Some(STUTTER_FIRST_TICK)
    }
}

/// Мок-драйвер схем: вся логика в памяти, с возможностью имитировать отказ
/// применения/восстановления.
pub struct MockSchemeDriver {
    pub active_guid: Arc<Mutex<String>>,
    pub schemes: Vec<PowerScheme>,
    pub original: String,
    pub admin: bool,
    pub ac_power: bool,
    /// Применение схемы с этим guid приводит к ошибке.
    pub fail_apply: Option<String>,
    /// Восстановление схемы с этим guid приводит к ошибке.
    pub fail_restore: Option<String>,
    /// Карта «guid → медианное время тика» общей для движков и тестов.
    perf: Arc<Mutex<BTreeMap<String, f64>>>,
}

impl MockSchemeDriver {
    /// Схемы: перечисленные guids; активная — `active`.
    pub fn new(guids: &[&str], active: &str) -> Self {
        let active_guid = Arc::new(Mutex::new(active.to_string()));
        let schemes = guids
            .iter()
            .map(|g| PowerScheme {
                guid: g.to_string(),
                name: format!("Схема {g}"),
                active: *g == active,
            })
            .collect();
        Self {
            active_guid,
            schemes,
            original: active.to_string(),
            admin: true,
            ac_power: true,
            fail_apply: None,
            fail_restore: None,
            perf: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub fn active(&self) -> String {
        self.active_guid.lock().unwrap().clone()
    }

    /// Общая с `VirtualEngine` карта производительности по guids.
    pub fn perf(&self) -> Arc<Mutex<BTreeMap<String, f64>>> {
        self.perf.clone()
    }
}

impl SchemeDriver for MockSchemeDriver {
    fn list_schemes(&self) -> Result<Vec<PowerScheme>, String> {
        let active = self.active_guid.lock().unwrap().clone();
        Ok(self
            .schemes
            .iter()
            .map(|p| PowerScheme {
                guid: p.guid.clone(),
                name: p.name.clone(),
                active: p.guid == active,
            })
            .collect())
    }

    fn set_active(&self, guid: &str) -> Result<(), String> {
        if let Some(f) = &self.fail_apply
            && f == guid && guid != self.original {
                return Err(format!("применение {guid} заблокировано (тест)"));
            }
        if let Some(f) = &self.fail_restore
            && f == guid && guid == self.original {
                return Err(format!("восстановление {guid} заблокировано (тест)"));
            }
        *self.active_guid.lock().unwrap() = guid.to_string();
        Ok(())
    }

    fn ac_power_online(&self) -> Result<bool, String> {
        Ok(self.ac_power)
    }

    fn is_admin(&self) -> bool {
        self.admin
    }
}

/// Хранилище контрольной точки в памяти.
pub struct MemoryCheckpointStore(pub Option<Checkpoint>);

impl MemoryCheckpointStore {
    pub fn new() -> Self {
        Self(None)
    }
}

impl CheckpointStore for MemoryCheckpointStore {
    fn load(&self) -> Option<Checkpoint> {
        self.0.clone()
    }

    fn save(&mut self, checkpoint: &Checkpoint) -> Result<(), String> {
        self.0 = Some(checkpoint.clone());
        Ok(())
    }

    fn clear(&mut self) -> Result<(), String> {
        self.0 = None;
        Ok(())
    }
}

/// План с разумными значениями по умолчанию при 9-секундной длительности.
pub fn plan(schemes: &[&str], reps: u32, guid: &str) -> SessionConfig {
    SessionConfig {
        duration_seconds: 9,
        warmup_seconds: 2,
        cooling_seconds: 0,
        repetitions: reps,
        background_threshold_percent: 5.0,
        worker_count: None,
        scheme_ids: schemes.iter().map(|s| s.to_string()).collect(),
        export_raw_samples: false,
        plan_guid: guid.to_string(),
    }
}

/// Прогнать `f` при выключенных паузах и чистом фоне; глобальные тестовые
/// швы обязательно сбрасываются после прогона.
pub fn with_fast<T>(f: impl FnOnce() -> T) -> T {
    crate::session::set_sleep_scale(0.0);
    crate::session::set_background_check_fn(Some(|_, _| (true, 1.0)));
    let out = f();
    crate::session::set_background_check_fn(None);
    crate::session::set_sleep_scale(1.0);
    out
}

/// Есть ли в событиях событие данного варианта (без аргументов).
pub fn has_event(events: &[SessionEvent], wanted: &SessionEvent) -> bool {
    events.iter().any(|e| std::mem::discriminant(e) == std::mem::discriminant(wanted))
}

/// Прогнать сессию и поднять панику с сообщением об ошибке.
pub fn expect_ok<E: SessionEngine>(
    engine: &mut E,
    driver: &MockSchemeDriver,
    plan: SessionConfig,
    store: &mut MemoryCheckpointStore,
) -> SessionOutcome {
    let cancel = Arc::new(AtomicBool::new(false));
    crate::session::run_session(engine, driver, plan, cancel, store, None).expect("сессия успешна")
}

// --- Сценарии E2E ----------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// E2E-сценарии переключают глобальные тестовые швы (паузы, фон). Чтобы
    /// параллельные тесты не крали их друг у друга, сценарии сериализуются.
    static E2E_LOCK: Mutex<()> = Mutex::new(());

    /// Сериализованный E2E-прогон с выключенными паузами и чистым фоном.
    fn e2e(f: impl FnOnce()) {
        let _guard = E2E_LOCK.lock().unwrap();
        with_fast(f);
    }

    /// Общий харнесс: две тестовые схемы плюс исходная «orig».
    /// Производительность a=0.25 мс/тик, b=0.26 мс/тик, orig=0.25 мс/тик.
    fn harness() -> (VirtualEngine, MockSchemeDriver, MemoryCheckpointStore) {
        let driver = MockSchemeDriver::new(&["orig", "a", "b"], "orig");
        {
            let perf = driver.perf();
            let mut p = perf.lock().unwrap();
            p.insert("orig".to_string(), 0.25);
            p.insert("a".to_string(), 0.25);
            p.insert("b".to_string(), 0.26);
        }
        let perf = driver.perf();
        let active = driver.active_guid.clone();
        let engine = VirtualEngine::new(4, 0xABCD, 0.25, 0.002, 0.0005, 60_000, perf, active);
        let store = MemoryCheckpointStore::new();
        (engine, driver, store)
    }

    #[test]
    fn full_session_completes_and_recommends_faster() {
        e2e(|| {
            let (mut engine, driver, mut store) = harness();
            let plan = plan(&["a", "b"], 2, "e2e-full");
            let out = expect_ok(&mut engine, &driver, plan, &mut store);

            assert!(!out.cancelled);
            assert!(has_event(&out.events, &SessionEvent::Finished));
            assert!(has_event(&out.events, &SessionEvent::Restored {
                scheme_id: "orig".into(),
                label: String::new(),
            }));
            // Контрольная точка успешной сессии удаляется.
            assert!(store.load().is_none());
            // Все прогоны отработаны: 2 схемы * 2 раунда.
            assert_eq!(out.checkpoint.runs.len(), 4);
            assert!(out.rejection_reasons.is_empty());
            let rec = out.recommendation.expect("рекомендация есть");
            assert_eq!(rec.recommended_scheme.as_deref(), Some("a"));
            // Исходная схема восстановлена.
            assert!(out.checkpoint.original_restored);
            assert_eq!(out.checkpoint.original_scheme_guid.as_deref(), Some("orig"));
        });
    }

    #[test]
    fn adaptive_early_stop_discovered() {
        e2e(|| {
            // Перевес 13.6% (0.22 против 0.25) при 6 раундах и малом шуме.
            let driver = MockSchemeDriver::new(&["orig", "a", "b"], "orig");
            {
                let perf = driver.perf();
                let mut p = perf.lock().unwrap();
                p.insert("orig".to_string(), 0.25);
                p.insert("a".to_string(), 0.22);
                p.insert("b".to_string(), 0.25);
            }
            let perf = driver.perf();
            let active = driver.active_guid.clone();
            let mut engine =
                VirtualEngine::new(4, 0x1234, 0.22, 0.001, 0.0, 20_000, perf, active);
            let mut store = MemoryCheckpointStore::new();
            let plan = plan(&["a", "b"], 6, "e2e-early");
            let out = crate::session::run_session(
                &mut engine,
                &driver,
                plan,
                Arc::new(AtomicBool::new(false)),
                &mut store,
                None,
            )
            .expect("сессия успешна");
            assert!(!out.cancelled);
            assert!(
                out.checkpoint.early_stop_reason.is_some(),
                "ожидали досрочную остановку"
            );
            assert!(has_event(&out.events, &SessionEvent::StoppedEarly { reason: String::new() }));
            // Отработано меньше раундов, чем запланировано, но не меньше двух.
            let run_rounds = out.checkpoint.runs.len() / 2;
            assert!(
                (2..6).contains(&run_rounds),
                "выполнено раундов должно быть 2..5, было {run_rounds}"
            );
            // Оставшиеся раунды помечаются выполненными (чтобы resume не
            // повторял их), но прогонов по ним нет.
            assert_eq!(out.checkpoint.completed_keys.len(), 6);
        });
    }

    #[test]
    fn cancel_preserves_checkpoint_and_resume_finishes() {
        e2e(|| {
            // Равные схемы: адаптивная остановка не должна вмешиваться,
            // сессия обязана дойти до всех 6 прогонов.
            let driver = MockSchemeDriver::new(&["orig", "a", "b"], "orig");
            {
                let perf = driver.perf();
                let mut p = perf.lock().unwrap();
                p.insert("orig".to_string(), 0.25);
                p.insert("a".to_string(), 0.25);
                p.insert("b".to_string(), 0.25);
            }
            let perf = driver.perf();
            let active = driver.active_guid.clone();
            let mut engine =
                VirtualEngine::new(4, 0xABCD, 0.25, 0.002, 0.0, 60_000, perf, active);
            let mut store = MemoryCheckpointStore::new();
            let cancel = Arc::new(AtomicBool::new(false));
            // Сессия «длинная» на виртуальном движке, чтобы отмена попала
            // внутрь прогонов.
            let plan = plan(&["a", "b"], 3, "e2e-cancel");
            let uc = cancel.clone();
            let canceller = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(40));
                uc.store(true, Ordering::Relaxed);
            });

            let first = crate::session::run_session(
                &mut engine,
                &driver,
                plan.clone(),
                cancel.clone(),
                &mut store,
                None,
            );
            canceller.join().unwrap();
            let out = first.expect("отменённая сессия — Ok(cancelled)");
            assert!(out.cancelled, "сессия должна быть отменена");
            assert!(out.checkpoint.original_restored);
            // Контрольная точка остаётся для возобновления.
            assert!(store.load().is_some());

            // Возобновление с того же плана и хранилища добивает оставшееся.
            let second_cancel = Arc::new(AtomicBool::new(false));
            let out2 = crate::session::run_session(
                &mut engine,
                &driver,
                plan.clone(),
                second_cancel,
                &mut store,
                None,
            )
            .expect("возобновление успешно");
            assert!(!out2.cancelled);
            assert!(out2.checkpoint.early_stop_reason.is_none());
            assert_eq!(out2.checkpoint.runs.len(), 2 * 3);
            assert!(store.load().is_none(), "точка удаляется после успеха");
            assert!(has_event(&out2.events, &SessionEvent::Finished));
        });
    }

    #[test]
    fn apply_failure_restores_original_and_fails() {
        e2e(|| {
            let (mut engine, mut driver, mut store) = harness();
            driver.fail_apply = Some("b".to_string());
            let plan = plan(&["a", "b"], 2, "e2e-apply-fail");
            let cancel = Arc::new(AtomicBool::new(false));
            let err = crate::session::run_session(
                &mut engine,
                &driver,
                plan,
                cancel,
                &mut store,
                None,
            )
            .expect_err("ожидали отказ применения");
            match err {
                SessionError::ApplyScheme { scheme_id, .. } => {
                    assert!(scheme_id.eq_ignore_ascii_case("b"))
                }
                other => panic!("не тот отказ: {other:?}"),
            }
            // Исходная схема активна после ошибки.
            assert_eq!(driver.active(), "orig");
        });
    }

    #[test]
    fn restore_failure_is_reported() {
        e2e(|| {
            let (mut engine, mut driver, mut store) = harness();
            driver.fail_restore = Some("orig".to_string());
            let plan = plan(&["a", "b"], 2, "e2e-restore-fail");
            let cancel = Arc::new(AtomicBool::new(false));
            let err = crate::session::run_session(
                &mut engine,
                &driver,
                plan,
                cancel,
                &mut store,
                None,
            )
            .expect_err("ожидали отказ восстановления");
            assert!(matches!(err, SessionError::RestoreScheme(_)));
            // Схемы не тронуты восстановлением: последняя применённая осталась.
            assert_ne!(driver.active(), "orig");
        });
    }

    #[test]
    fn not_admin_and_no_ac_block() {
        e2e(|| {
            let (mut engine, mut driver, mut store) = harness();
            let plan = plan(&["a", "b"], 1, "e2e-guard-1");
            driver.admin = false;
            let e1 = crate::session::run_session(
                &mut engine,
                &driver,
                plan.clone(),
                Arc::new(AtomicBool::new(false)),
                &mut store,
                None,
            )
            .expect_err("должно быть запрещено без админа");
            assert!(matches!(e1, SessionError::NotAdmin));

            driver.admin = true;
            driver.ac_power = false;
            let e2 = crate::session::run_session(
                &mut engine,
                &driver,
                plan,
                Arc::new(AtomicBool::new(false)),
                &mut store,
                None,
            )
            .expect_err("должно быть запрещено без сети");
            assert!(matches!(e2, SessionError::NoAcPower));
        });
    }

    #[test]
    fn clean_background_is_reported() {
        e2e(|| {
            // Хук «чистого фона» из with_fast должен породить BackgroundClean.
            let (mut engine, driver, mut store) = harness();
            let plan = plan(&["a"], 1, "e2e-clean-bg");
            let out = expect_ok(&mut engine, &driver, plan, &mut store);
            assert!(has_event(&out.events, &SessionEvent::BackgroundClean {
                measured_total_percent: f64::NAN,
                threshold_percent: f64::NAN,
            }));
        });
    }

    #[test]
    fn stuttering_scheme_is_rejected_and_test_continues() {
        e2e(|| {
            let guid = "e2e-stut";
            let plan = plan(&["aa", "bb"], 1, guid);
            let order0 = crate::config::round_order(&plan.scheme_ids, 0, guid);
            let stall = order0[1].clone(); // вторая по порядку схема «заедает».
            let good = order0[0].clone();
            let driver = MockSchemeDriver::new(&["orig", "aa", "bb"], "orig");
            let active = driver.active_guid.clone();
            let mut engine = StutterEngine::new(stall.clone(), active);
            let mut store = MemoryCheckpointStore::new();
            let out = expect_ok(&mut engine, &driver, plan, &mut store);

            assert!(!out.cancelled);
            let reason = out
                .rejection_reasons
                .get(&stall)
                .expect("заедающая схема должна быть забракована");
            assert!(reason.contains("заедания"), "reason = {reason}");
            assert!(has_event(&out.events, &SessionEvent::SchemeRejected {
                scheme_id: stall.clone(),
                reason: String::new(),
            }));
            assert!(out.checkpoint.has_run(0, &good), "здоровая схема не выполнена");
            assert!(
                !out.checkpoint.has_run(0, &stall),
                "заблокированная схема не должна иметь прогон"
            );
            // Сессия завершилась штатно: контрольная точка удалена.
            assert!(store.load().is_none());
        });
    }

    #[test]
    fn stall_as_first_scheme_is_rejected_via_canonical() {
        // «Максимально плохая» схема идёт ПЕРВОЙ в ротации. Собственной
        // (низкой) базовой скорости у неё ещё нет, а сессионных эталонов —
        // тем более: без canonical-эталона она не была бы распознана, и её
        // нагрузка молотила бы «сломанную» систему весь план. Эталон из
        // короткого замера под исходной схемой бракует её уже в разогреве.
        e2e(|| {
            let guid = "e2e-stut-first";
            let plan = plan(&["aa", "bb"], 1, guid);
            let order0 = crate::config::round_order(&plan.scheme_ids, 0, guid);
            let stall = order0[0].clone(); // первая по порядку схема «заедает».
            let good = order0[1].clone();
            let driver = MockSchemeDriver::new(&["orig", "aa", "bb"], "orig");
            let active = driver.active_guid.clone();
            let mut engine = StutterEngine::new(stall.clone(), active);
            let mut store = MemoryCheckpointStore::new();
            let out = expect_ok(&mut engine, &driver, plan, &mut store);

            assert!(!out.cancelled);
            let reason = out
                .rejection_reasons
                .get(&stall)
                .expect("заедающая (первая) схема должна быть забракована");
            assert!(reason.contains("заедания"), "reason = {reason}");
            assert!(has_event(&out.events, &SessionEvent::SchemeRejected {
                scheme_id: stall.clone(),
                reason: String::new(),
            }));
            assert!(out.checkpoint.has_run(0, &good), "здоровая схема не выполнена");
            assert!(
                !out.checkpoint.has_run(0, &stall),
                "заблокированная схема не должна иметь прогон"
            );
            assert!(store.load().is_none());
        });
    }
}
