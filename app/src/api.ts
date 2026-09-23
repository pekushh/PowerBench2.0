// Типизированный мост к командам Tauri и событиям телеметрии.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

// ---------- Типы (зеркала Rust-DTO) ----------

export interface SchemeRow {
  guid: string;
  name: string;
  active: boolean;
}

export interface SettingsDto {
  duration_seconds: number;
  warmup_seconds: number;
  cooling_seconds: number;
  repetitions: number;
  background_threshold_percent: number;
  theme: string;
  mode: string;
  reduce_motion: boolean;
  sidebar_collapsed: boolean;
  favorite_schemes: string[];
  excluded_schemes: string[];
  scoring_performance: number;
  scoring_stability: number;
  scoring_worst_second: number;
  retention_max_sessions: number;
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
  schemes: number;
  level: string;
  level_label: string;
  readable: boolean;
  error: string | null;
}

export interface StoredRun {
  key: string;
  round: number;
  scheme_id: string;
  scheme_name: string | null;
  started_at_ns: string;
  duration_ms: number;
  ticks: number;
  supercycles: number;
  first_tick_checksums: number[];
  run_checksums: number[];
  phases: {
    phase_index: number;
    stats: {
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
    };
  }[];
  combined: {
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
  };
  cross_phase_consistency: number;
  burst_retention_percent: number;
  background: unknown[];
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
  run_duration_ms: number;
  started_at_min_ns: string;
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
}

export interface LogMsg {
  level: "info" | "warn" | "success";
  text: string;
  ts_ms: number;
}

export interface FinishedPayload {
  ok: boolean;
  error: string | null;
  plan_guid: string | null;
  cancelled: boolean | null;
  result_path: string | null;
  level: string | null;
  level_label: string | null;
  recommended_scheme: string | null;
  recommended_name: string | null;
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
  identityInfo: () => invoke<IdentityDto>("identity_info"),
  startTest: (req: TestRequestDto) => invoke<string>("start_test", { req }),
  stopTest: () => invoke<boolean>("stop_test"),
  testRunning: () => invoke<boolean>("test_running"),
  historyList: () => invoke<HistoryRow[]>("history_list"),
  historyOpen: (planGuid: string) => invoke<SessionJson>("history_open", { planGuid }),
  historyExportTo: (planGuid: string, format: "json" | "csv", outDir: string) =>
    invoke<string[]>("history_export_to", { planGuid, format, outDir }),
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

export function fmtTime(ts: number): string {
  const d = new Date(ts);
  const p = (n: number, l = 2) => n.toString().padStart(l, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

export function fmtNs(ns: string | number): string {
  const n = Number(ns);
  if (!n) return "—";
  const d = new Date(Math.floor(n / 1_000_000));
  const p = (x: number, l = 2) => x.toString().padStart(l, "0");
  return `${p(d.getDate())}.${p(d.getMonth() + 1)}.${d.getFullYear()} ${p(d.getHours())}:${p(d.getMinutes())}`;
}