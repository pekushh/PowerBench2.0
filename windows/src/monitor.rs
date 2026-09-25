//! Фоновый мониторинг процессов (sysinfo/PDH-эквивалент) и корреляция
//! «окон скачков» латентности с процессами, активными в те же секунды.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

/// Минимальная занятость процесса (в % одного ядра), при которой секунда
/// считается «активной» для этого процесса в целях корреляции.
pub const ACTIVE_CPU_PERCENT: f64 = 0.5;

/// Имена системных процессов, исключаемых из мониторинга (регистронезависимо).
pub const EXCLUDED_NAMES: &[&str] = &[
    "idle",
    "system",
    "_total",
    "system idle process",
    "system process",
    "idle process",
];

/// Замер одного процесса в конкретную секунду.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcessSample {
    pub name: String,
    pub path: String,
    pub cpu_percent: f64,
    pub memory_bytes: u64,
}

/// Окно скачка латентности: интервал настенных секунд, включая концы.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpikeWindow {
    pub start_second: u64,
    pub end_second: u64,
    pub phase_label: String,
}

/// Коррелированный процесс (результат «топ-5»).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CorrelatedProcess {
    pub name: String,
    pub path: String,
    pub median_cpu_percent: f64,
    pub peak_cpu_percent: f64,
    pub correlated_spike_windows: usize,
    pub peak_memory_bytes: u64,
    pub phases: Vec<String>,
}

/// Системный ли процесс по имени (Idle/System/_Total и их варианты).
pub fn is_excluded_name(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    EXCLUDED_NAMES.contains(&lower.as_str())
}

/// Сэмплер процессов: раз в секунду снимает загрузку CPU и рабочий набор.
pub struct ProcessSampler {
    #[cfg(windows)]
    system: sysinfo::System,
    self_pid: u32,
}

impl Default for ProcessSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessSampler {
    /// Создать сэмплер. На не-Windows платформах выборки пустые.
    pub fn new() -> Self {
        #[cfg(windows)]
        {
            let mut system = sysinfo::System::new();
            system.refresh_processes();
            let self_pid = sysinfo::get_current_pid().map(|p| p.as_u32()).unwrap_or(0);
            Self { system, self_pid }
        }
        #[cfg(not(windows))]
        {
            Self { self_pid: 0 }
        }
    }

    /// Снять очередную выборку процессов (исключая Idle/System/_Total/себя).
    pub fn sample(&mut self) -> Vec<ProcessSample> {
        #[cfg(windows)]
        {
            self.system.refresh_cpu_usage();
            self.system.refresh_processes();
            let mut out: Vec<ProcessSample> = Vec::new();
            for (&pid, process) in self.system.processes() {
                let pid_u32 = pid.as_u32();
                let name = process.name().to_string();
                if is_excluded_name(&name) || pid_u32 == self.self_pid {
                    continue;
                }
                let cpu = f64::from(process.cpu_usage());
                let memory = process.memory();
                // Пустые «тени» системных PID с нулевой загрузкой не интересны,
                // но остаёмся честными: пропускаем только нулевую пару.
                if cpu == 0.0 && memory == 0 {
                    continue;
                }
                out.push(ProcessSample {
                    name,
                    path: process
                        .exe()
                        .map(|e| e.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    cpu_percent: cpu,
                    memory_bytes: memory,
                });
            }
            out
        }
        #[cfg(not(windows))]
        {
            let _ = self;
            Vec::new()
        }
    }
}

/// Медиана чисел (для отчёта); пустой список → 0.
fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut v: Vec<f64> = values.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// Корреляция окон скачков с процессами, активными в те же секунды.
///
/// Окно считается «покрытым» процессом, если в какой-то его секунде процесс
/// имел выборку с `cpu_percent >= ACTIVE_CPU_PERCENT`. Медианная и пиковая
/// загрузки, а также пик памяти считаются по всем выборкам процесса.
/// Результат — топ-5: сначала по числу покрытых окон, затем по пиковой загрузке;
/// при равенстве — по имени. Пусто, если корреляций нет.
pub fn correlate(
    spike_windows: &[SpikeWindow],
    samples_by_second: &BTreeMap<u64, Vec<ProcessSample>>,
) -> Vec<CorrelatedProcess> {
    if spike_windows.is_empty() {
        return Vec::new();
    }
    // Секунды, принадлежащие хотя бы одному окну.
    let mut covered: BTreeSet<u64> = BTreeSet::new();
    for w in spike_windows {
        for s in w.start_second..=w.end_second {
            covered.insert(s);
        }
    }

    // По процессу: число покрытых окон и множество фаз, чьи окна он побил.
    let mut window_hits: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
    let mut phase_hits: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut all_samples: BTreeMap<String, Vec<ProcessSample>> = BTreeMap::new();

    for (second, samples) in samples_by_second {
        for s in samples {
            all_samples
                .entry(s.name.clone())
                .or_default()
                .push(s.clone());
            if !covered.contains(second) {
                continue;
            }
            if s.cpu_percent < ACTIVE_CPU_PERCENT {
                continue;
            }
            for (wi, w) in spike_windows.iter().enumerate() {
                if *second >= w.start_second && *second <= w.end_second {
                    window_hits.entry(s.name.clone()).or_default().insert(wi);
                    phase_hits
                        .entry(s.name.clone())
                        .or_default()
                        .insert(w.phase_label.clone());
                }
            }
        }
    }

    let mut list: Vec<CorrelatedProcess> = Vec::new();
    for name in window_hits.keys() {
        let samples = all_samples.get(name).cloned().unwrap_or_default();
        let cpus: Vec<f64> = samples.iter().map(|s| s.cpu_percent).collect();
        let peak = cpus.iter().cloned().fold(0.0f64, f64::max);
        let mut phases: Vec<String> = phase_hits
            .get(name)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default();
        phases.sort();
        list.push(CorrelatedProcess {
            name: name.clone(),
            path: samples.first().map(|s| s.path.clone()).unwrap_or_default(),
            median_cpu_percent: median(&cpus),
            peak_cpu_percent: peak,
            correlated_spike_windows: window_hits.get(name).map(|s| s.len()).unwrap_or(0),
            peak_memory_bytes: samples.iter().map(|s| s.memory_bytes).max().unwrap_or(0),
            phases,
        });
    }

    list.sort_by(|a, b| {
        b.correlated_spike_windows
            .cmp(&a.correlated_spike_windows)
            .then_with(|| {
                b.peak_cpu_percent
                    .partial_cmp(&a.peak_cpu_percent)
                    .unwrap_or(Ordering::Equal)
            })
            .then_with(|| a.name.cmp(&b.name))
    });
    list.truncate(5);
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(name: &str, cpu: f64, mem: u64) -> ProcessSample {
        ProcessSample {
            name: name.to_string(),
            path: format!("C:\\{name}.exe"),
            cpu_percent: cpu,
            memory_bytes: mem,
        }
    }

    #[test]
    fn excluded_names_detected_case_insensitively() {
        assert!(is_excluded_name("Idle"));
        assert!(is_excluded_name("_Total"));
        assert!(is_excluded_name("System"));
        assert!(is_excluded_name("  System Idle Process "));
        assert!(!is_excluded_name("chrome"));
    }

    #[test]
    fn correlation_matches_window_to_busy_process() {
        let windows = vec![SpikeWindow {
            start_second: 1,
            end_second: 1,
            phase_label: "Тяжёлая".to_string(),
        }];
        let mut map: BTreeMap<u64, Vec<ProcessSample>> = BTreeMap::new();
        map.insert(
            1,
            vec![sample("antivirus", 40.0, 100), sample("explorer", 0.1, 5)],
        );
        map.insert(2, vec![sample("antivirus", 40.0, 100)]);
        let result = correlate(&windows, &map);
        assert_eq!(result.len(), 1);
        let p = &result[0];
        assert_eq!(p.name, "antivirus");
        assert_eq!(p.correlated_spike_windows, 1);
        assert_eq!(p.phases, vec!["Тяжёлая"]);
        // Пиковая загрузка по всем выборкам процесса = 40.
        assert_eq!(p.peak_cpu_percent, 40.0);
    }

    #[test]
    fn idle_processes_in_window_do_not_correlate() {
        let windows = vec![SpikeWindow {
            start_second: 5,
            end_second: 5,
            phase_label: "Лёгкая".to_string(),
        }];
        let mut map: BTreeMap<u64, Vec<ProcessSample>> = BTreeMap::new();
        map.insert(5, vec![sample("explorer", 0.2, 5)]);
        let result = correlate(&windows, &map);
        assert!(result.is_empty());
    }

    #[test]
    fn no_spikes_means_empty_result() {
        let map: BTreeMap<u64, Vec<ProcessSample>> = BTreeMap::new();
        assert!(correlate(&[], &map).is_empty());
    }

    #[test]
    fn median_of_odd_and_even() {
        assert_eq!(median(&[]), 0.0);
        assert_eq!(median(&[1.0, 3.0, 2.0]), 2.0);
        assert_eq!(median(&[1.0, 2.0, 3.0, 4.0]), 2.5);
    }

    #[test]
    fn multi_window_counting_and_phase_collection() {
        let windows = vec![
            SpikeWindow {
                start_second: 1,
                end_second: 1,
                phase_label: "Лёгкая".to_string(),
            },
            SpikeWindow {
                start_second: 9,
                end_second: 9,
                phase_label: "Отклик".to_string(),
            },
        ];
        let mut map: BTreeMap<u64, Vec<ProcessSample>> = BTreeMap::new();
        map.insert(1, vec![sample("busy", 50.0, 7)]);
        map.insert(9, vec![sample("busy", 50.0, 7)]);
        let result = correlate(&windows, &map);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].correlated_spike_windows, 2);
        assert_eq!(result[0].phases, vec!["Лёгкая", "Отклик"]);
    }

    #[test]
    fn top_five_limit_with_ranking() {
        let windows = vec![SpikeWindow {
            start_second: 1,
            end_second: 1,
            phase_label: "X".to_string(),
        }];
        let mut map: BTreeMap<u64, Vec<ProcessSample>> = BTreeMap::new();
        let mut jobs: Vec<ProcessSample> = Vec::new();
        for i in 0..8 {
            jobs.push(sample(&format!("proc{i}"), 10.0 + i as f64, 1));
        }
        map.insert(1, jobs);
        let result = correlate(&windows, &map);
        assert_eq!(result.len(), 5);
        assert!(result[0].peak_cpu_percent > result[4].peak_cpu_percent);
    }
}
