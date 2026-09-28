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
  favorite_schemes: string[];
  excluded_schemes: string[];
  score_performance: number;
  score_stability: number;
  score_worst_second: number;
  max_sessions: number;
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
  export_raw_samples?: boolean;
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
  historyDelete: (fileName: string) => invoke<void>("history_delete", { fileName }),
  historyOpenFolder: () => invoke<void>("history_open_folder"),
  openFolder: (path: string) => invoke<void>("open_folder", { path }),
  openFile: (path: string) => invoke<void>("open_file", { path }),
  storageStats: () => invoke<StorageStats>("storage_stats"),
  logHistory: () => invoke<LoggerEntry[]>("log_history"),
  systemReady: (requestedSchemes?: number | null) =>
    invoke<Readiness>("system_ready", { requestedSchemes: requestedSchemes ?? null }),
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