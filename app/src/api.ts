// Типизированный мост к командам Tauri и событиям телеметрии.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

// ---------- Типы (зеркала Rust-DTO) ----------

export interface SchemeRow {
  guid: string;
  name: string;
  active: boolean;
}

export type QuarantineKind = "HardFreeze" | "NoProgress" | "Unstable" | "Degraded";

/** Параметры режима теста (зеркало `orchestrator::config::Preset`). */
export interface PresetDto {
  key: "quick" | "detailed";
  duration_seconds: number;
  warmup_seconds: number;
  cooling_seconds: number;
  repetitions: number;
}

/** Одна измеряемая фаза (зеркало `core::config::Phase`). */
export interface PhasePlanRow {
  index: number;
  name: string;
  seconds: number;
  active_worker_percent: number;
}

/** Сводка по одной фазе в результате сессии. */
export interface PhaseSummaryJson {
  name: string;
  median_throughput: number;
  p1_throughput: number;
  consistency_percent: number;
  /** Насколько частота в этой фазе просела относительно лучшей частоты
   *  сессии, %. 0 — снижения не замечено. */
  frequency_drop_percent: number;
  /** Медианная частота CPU в этой фазе, МГц; 0 — не сообщалась. */
  frequency_mhz: number;
}

/** Оценка дрейфа машины по опорной схеме. */
export interface ReferenceSummary {
  scheme_id: string;
  scheme_name: string | null;
  per_round: number[];
  span_percent: number;
  trend_percent_per_round: number;
  span_limit_percent: number;
  unstable: boolean;
  note: string;
}

export interface QuarantineEntry {
  scheme_id: string;
  scheme_name: string | null;
  kind: QuarantineKind;
  reason: string;
  at_ns: number;
  plan_guid: string;
}

export const QUARANTINE_LABELS: Record<QuarantineKind, string> = {
  HardFreeze: "зависание",
  NoProgress: "нет прогресса",
  Unstable: "нестабильна",
  Degraded: "деградация",
};

export interface SettingsDto {
  /** Порог фоновой нагрузки, % на ядро. Параметры режима — см. `PresetDto`. */
  background_threshold_percent: number;
  theme: string;
  mode: string;
  reduce_motion: boolean;
  sidebar_collapsed: boolean;
  /** Плотность интерфейса: «compact» · «normal» · «roomy» (CSS `--k`). */
  density: string;
  /** Масштаб текста: «s» · «m» · «l» (CSS `--kt`). */
  text_scale: string;
  favorite_schemes: string[];
  excluded_schemes: string[];
  // Веса убраны из интерфейса вместе с весовым баллом: категории в отчёте
  // считаются по throughput, P1 и стабильности. Поля остаются в DTO, чтобы
  // не ломать чтение сохранённых настроек.
  /** @deprecated весовой балл больше не показывается. */
  score_performance: number;
  /** @deprecated весовой балл больше не показывается. */
  score_stability: number;
  /** @deprecated весовой балл больше не показывается. */
  score_worst_second: number;
  max_sessions: number;
  /**
   * Заметка о настройках CPU и BIOS (разгон, андерволт, отключённые
   * функции). Попадает в отчёт для поддержки: программа не может выяснить
   * это сама, потому что Windows одинаково показывает и штатный буст, и
   * разгон одной и той же цифрой.
   */
  cpu_notes: string;
}

export interface CheckpointDto {
  plan_guid: string;
  scheme_ids: string[];
  repetitions: number;
  duration_seconds: number;
  completed_keys: string[];
  original_scheme_guid: string | null;
  original_restored: boolean;
  has_runs: boolean;
}

export interface IdentityDto {
  workload_version: string;
  config_hash: string;
  seed_hex: string;
  worker_count: number;
  logical_cpus: number;
  timer_hz: number;
  cpu_identifier: string;
  diagnostics_version: string;
  /** Сборка и ревизия ОС — обновление Windows меняет сопоставимость. */
  os_build: string;
  memory_gib: number;
  cpu_brand: string;
}

export interface HistoryRow {
  file_name: string;
  plan_guid: string;
  started_label: string;
  started_at_ns: number;
  schemes: number;
  level: string;
  level_label: string;
  readable: boolean;
  error: string | null;
  scheme_name: string;
  /** Медианный throughput лидера, тик/с. */
  throughput: number | null;
  /** Балл лидера 0..=100 по весам из настроек. */
  score: number | null;
  margin: number | null;
  stability: number | null;
  early_stopped: boolean;
  rounds_planned: number;
  rounds_completed: number;
  /** Время изменения файла записи (Unix ns), 0 если недоступно. У прерванной
   *  сессии метка старта нулевая, и дата берётся отсюда. */
  file_modified_at_ns: number;
  /** GUID лидера сессии — нужен, чтобы показать название схемы, если в файле
   *  сохранился только идентификатор. */
  leader_scheme_guid: string;
}

/**
 * Сколько прогонов одной схемы нужно, чтобы показывать доверительный
 * интервал, «уверенность» и перевес.
 *
 * При меньшем числе движок отдаёт `margin: 0.0`, а на экране это читалось как
 * идеальная точность: «Погрешность ±0.0» и «Уверенность 100 %» при одном
 * прогоне. Тот же порог стоит в тексте предупреждения о сессии-скрининге
 * («нужно от 3 прогонов»), поэтому здесь ровно три.
 */
export const MARGIN_MIN_RUNS = 3;

/** Хватает ли прогонов одной схеме, чтобы показать доверительный интервал. */
export function marginAvailable(runs: number, margin: number | null): boolean {
  return runs >= MARGIN_MIN_RUNS && margin != null && Number.isFinite(margin);
}

/** То же для сессии: интервал показывается, когда его есть у всех замерянных схем. */
export function sessionMarginAvailable(
  schemes: ReadonlyArray<{ runs: number }>,
  margin: number | null,
): boolean {
  const measured = schemes.filter((s) => s.runs > 0);
  if (measured.length === 0) return false;
  return marginAvailable(Math.min(...measured.map((s) => s.runs)), margin);
}

/** Статистика прогона в одном диапазоне (зеркало `checkpoint::RunStats`). */
export interface RunStatsJson {
  mean: number;
  median: number;
  p1: number;
  p01: number;
  p95_ms: number;
  p99_ms: number;
  std_dev: number;
  consistency_percent: number;
  burst_retention_percent: number;
  jitter_p99_ms: number;
}

/** Связанный фоновый процесс, измеренный во время прогона. */
export interface CorrelatedProcessJson {
  name: string;
  path: string;
  median_cpu_percent: number;
  peak_cpu_percent: number;
  correlated_spike_windows: number;
  peak_memory_bytes: number;
  phases: string[];
}

/** Один прогон одной схемы (зеркало `checkpoint::StoredRun`). */
export interface StoredRun {
  key: string;
  round: number;
  scheme_id: string;
  scheme_name: string | null;
  started_at_ns: number;
  duration_ms: number;
  ticks: number;
  supercycles: number;
  /** Ровно три контрольные суммы: загрузка, тяжёлая, отклик. */
  first_tick_checksums: [number, number, number];
  run_checksums: [number, number, number];
  phases: { phase_index: number; stats: RunStatsJson }[];
  combined: RunStatsJson;
  cross_phase_consistency: number;
  burst_retention_percent: number;
  background: CorrelatedProcessJson[];
  spike_windows: number;
  /** Фоновая нагрузка. В записях, сделанных до их появления, полей нет. */
  background_cpu_p50?: number;
  background_cpu_p95?: number;
  background_sample_seconds?: number;
}

export interface SchemeJson {
  scheme_id: string;
  name: string | null;
  rejected: boolean;
  rejection_reason: string | null;
  runs: number;
  mean_average_throughput: number;
  sample_std: number;
  t_value: number;
  margin: number;
  ci_95: [number, number];
  run_variation_percent: number;
  cv_warning: boolean;
  median_throughput: number;
  median_p1_throughput: number;
  median_p01_throughput: number;
  median_p95_execution_time_ms: number;
  median_p99_execution_time_ms: number;
  median_consistency_percent: number;
  median_burst_retention_percent: number;
  median_jitter_p99_ms: number;
  /** Медиана прохода фоновой нагрузки, %; `null`, если фона не было. */
  median_background_purity: number | null;
  /** Метрики по фазам (медиана, P1, стабильность, троттлинг). */
  phases: PhaseSummaryJson[];
  run_duration_ms: number;
  started_at_min_ns: number;
  per_run: StoredRun[];
}

export interface RecommendationJson {
  level: string;
  level_label: string;
  recommended_scheme: string | null;
  runner_up_scheme: string | null;
  reason: string;
  probabilities: [number, number, number] | null;
  expected_margin_percent: number | null;
  bootstrap_mode: string | null;
  tie_criterion: string | null;
}

export interface SessionJson {
  plan_guid: string;
  original_scheme_guid: string | null;
  original_restored: boolean;
  identity: IdentityDto;
  schemes: SchemeJson[];
  recommendation: RecommendationJson;
  warnings: string[];
  rounds_planned: number;
  rounds_completed: number;
  early_stop_reason: string | null;
  score_weights: [number, number, number];
  /** Сессия-скрининг: прогона на схему недостаточно для вердикта. */
  screening: boolean;
  /** Оценка дрейфа машины по опорной схеме. */
  reference: ReferenceSummary | null;
}

export interface TestRequestDto {
  preset: string;
  duration_seconds: number | null;
  warmup_seconds: number | null;
  cooling_seconds: number | null;
  repetitions: number | null;
  background_threshold_percent: number | null;
  worker_count: number | null;
  scheme_ids: string[];
  resume: boolean;
  /** Активная схема — эталон для оценки дрейфа машины. */
  active_scheme_id: string | null;
}

export interface StorageStats {
  free_bytes: number;
  total_bytes: number;
  history_bytes: number;
  max_sessions: number;
}

export interface Readiness {
  ok: boolean;
  issues: string[];
}

export interface LoggerEntry {
  level: string;
  text: string;
  ts_ms: number;
}

// ---------- Телеметрия ----------

export interface TelemetryMsg {
  running: boolean;
  phase: string;
  phase_seconds: number;
  ticks_done: number;
  ticks_per_sec: number;
  ms_per_tick: number;
  run_index: number;
  run_total: number;
  round: number;
  scheme_id: string;
  scheme_name: string;
  phase_elapsed_ms: number;
  /** Фоновая нагрузка, % CPU; `null`, пока не измерена. */
  background_percent: number | null;
  /** Фон превысил порог — замеру стоит доверять с оговоркой. */
  background_noisy: boolean;
}

/**
 * Уровень записи журнала.
 *
 * Раньше здесь стоял союз `"info" | "warn" | "success"`, но бэкенд шлёт ещё
 * и `error` (ошибки движка, сохранения, отчёта), а потребитель приводил
 * незнакомые значения вручную. Теперь тип совпадает с тем, что реально
 * приходит, и `normLevel` на странице логов отвечает за отображение.
 */
export type LogLevel = "info" | "warn" | "success" | "error";

export interface LogMsg {
  level: LogLevel;
  text: string;
  ts_ms: number;
}

export interface FinishedPayload {
  ok: boolean;
  error: string | null;
  plan_guid: string | null;
  cancelled: boolean | null;
  early_stopped: boolean;
  result_path: string | null;
  report_path: string | null;
  level: string | null;
  level_label: string | null;
  recommended_scheme: string | null;
  recommended_name: string | null;
  expected_margin_percent: number | null;
  winner_margin_percent: number | null;
}

// ---------- Команды ----------

export const commands = {
  listSchemes: () => invoke<SchemeRow[]>("list_schemes"),
  isAdmin: () => invoke<boolean>("is_admin"),
  acPowerOnline: () => invoke<boolean>("ac_power_online"),
  schemeAction: (action: string, guid?: string | null, path?: string | null) =>
    invoke<string | null>("scheme_action", { action, guid: guid ?? null, path: path ?? null }),
  getSettings: () => invoke<SettingsDto>("get_settings"),
  setSettings: (settings: SettingsDto) => invoke<void>("set_settings", { settings }),
  checkpointStatus: () => invoke<CheckpointDto | null>("checkpoint_status"),
  checkpointDiscard: () => invoke<void>("checkpoint_discard"),
  startTest: (req: TestRequestDto) => invoke<string>("start_test", { req }),
  stopTest: () => invoke<boolean>("stop_test"),
  testRunning: () => invoke<boolean>("test_running"),
  historyList: () => invoke<HistoryRow[]>("history_list"),
  historyOpen: (planGuid: string) => invoke<SessionJson>("history_open", { planGuid }),
  historyExportTo: (planGuid: string, format: "json" | "csv", outDir: string) =>
    invoke<string[]>("history_export_to", { planGuid, format, outDir }),
  historyReport: (outDir: string) => invoke<string>("history_report", { outDir }),
  sessionReport: (planGuid: string) => invoke<string>("session_report", { planGuid }),
  quarantineList: () => invoke<QuarantineEntry[]>("quarantine_list"),
  quarantineClear: (schemeId: string) => invoke<boolean>("quarantine_clear", { schemeId }),
  /** Оценка длительности сессии, посчитанная бэкендом. */
  /** Сбросить журнал на диск (вызывается перед выходом и в тестах). */
  logFlush: () => invoke<void>("log_flush"),
  /** Имя файла отчёта по умолчанию, чтобы диалог сохранения был осмысленным. */
  diagnosticsFileName: () => invoke<string>("diagnostics_file_name"),
  /** Состав машины для чипа в шапке: берётся тот же `identity`, что и в отчёте. */
  identityInfo: () => invoke<IdentityDto>("identity_info"),
  /**
   * Сохранить отчёт для поддержки: журнал, окружение, идентичность замера,
   * состояние контрольной точки, карантина, настроек и последней сессии.
   */
  saveDiagnostics: (path: string, redact: boolean) =>
    invoke<{ path: string; lines: number; findings: number; suggested_name: string }>(
      "save_diagnostics",
      { path, redact },
    ),
  estimateSession: (
    durationSeconds: number,
    warmupSeconds: number,
    coolingSeconds: number,
    repetitions: number,
    schemeCount: number,
  ) =>
    invoke<{ seconds: number; label: string }>("estimate_session", {
      durationSeconds,
      warmupSeconds,
      coolingSeconds,
      repetitions,
      schemeCount,
    }),
  /** Значения обоих пресетов — единственный источник для карточек режимов. */
  testPresets: () => invoke<PresetDto[]>("test_presets"),
  /** Длительности фаз для заданной длительности — единственный источник. */
  phasePlan: (durationSeconds: number) =>
    invoke<PhasePlanRow[]>("phase_plan", { durationSeconds }),
  historyDelete: (fileName: string) => invoke<void>("history_delete", { fileName }),
  historyOpenFolder: () => invoke<void>("history_open_folder"),
  openFolder: (path: string) => invoke<void>("open_folder", { path }),
  openFile: (path: string) => invoke<void>("open_file", { path }),
  storageStats: () => invoke<StorageStats>("storage_stats"),
  logHistory: () => invoke<LoggerEntry[]>("log_history"),
  systemReady: (requestedSchemes?: number | null) =>
    invoke<Readiness>("system_ready", { requestedSchemes: requestedSchemes ?? null }),
  /** Текущая фоновая нагрузка CPU, % — для предстартовой проверки. */
  backgroundSample: () => invoke<number>("background_sample"),
  resultsDir: () => invoke<string>("results_dir"),
  appsettingsPath: () => invoke<string>("appsettings_path"),
};

// ---------- События ----------

export function onTelemetry(cb: (m: TelemetryMsg) => void): Promise<UnlistenFn> {
  return listen<TelemetryMsg>("telemetry", (e) => cb(e.payload));
}

export function onLog(cb: (m: LogMsg) => void): Promise<UnlistenFn> {
  return listen<LogMsg>("log", (e) => cb(e.payload));
}

export function onTestFinished(cb: (m: FinishedPayload) => void): Promise<UnlistenFn> {
  return listen<FinishedPayload>("test-finished", (e) => cb(e.payload));
}

// ---------- Прочее ----------

/**
 * Время в формате `ЧЧ:ММ:СС`.
 *
 * На вход приходит миллисекунды epoch. Нечисловое или отрицательное значение
 * даёт `Invalid Date`, из которого получалось «NaN:NaN:NaN», поэтому
 * невалидные значения показываются прочерком.
 */
export function fmtTime(ts: number): string {
  if (!Number.isFinite(ts) || ts < 0) return "--:--:--";
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return "--:--:--";
  const p = (n: number, l = 2) => n.toString().padStart(l, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}