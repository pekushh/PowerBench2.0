//! Топология процессора и привязка потоков к ядрам.
//!
//! Замер схемы питания имеет смысл, только если известно, **на каких ядрах**
//! шла нагрузка. Без привязки планировщик Windows раскладывает воркеров по
//! ядрам произвольно и перераспределяет их по ходу прогона, поэтому на
//! гибридной архитектуре (P- и E-ядра в одном кластоне) измеряется не разница
//! между схемами питания, а случайная раскладка по кластерам.
//!
//! Модуль решает две задачи:
//!
//! 1. **Разведка** через `GetLogicalProcessorInformationEx(RelationProcessorCore)`:
//!    физические ядра, их класс эффективности (0 — производительные, старше —
//!    экономные) и группы процессоров.
//! 2. **Привязка**: [`CpuTopology::bind_current_thread`] ставит маску текущего
//!    потока.
//!
//! Маска ставится **на поток**, а не на процесс: маска процесса конфликтует с
//! антивирусом и некоторыми сетевыми фильтрами, которые ставят свою, и
//! Windows возвращает её к прежнему виду — тихо, без ошибки. Маска потока
//! переживает это, потому что принадлежит конкретному потоку.
//!
//! Режимы ([`AffinityMode`]):
//!
//! * [`AffinityMode::POnly`] — по одному потоку на физическое производительное
//!   ядро. Режим по умолчанию: измеряется однородная нагрузка.
//! * [`AffinityMode::AllLogical`] — по одному потоку на логический процессор,
//!   SMT-партнёры могут достаться разным воркерам.
//! * [`AffinityMode::AffinityOff`] — привязки нет вовсе, планировщик свободен.
//!   Нужен там, где маски запрещены (жёсткие политики домена, некоторые
//!   виртуализированные ЦП).
//!
//! Режим переопределяется переменной окружения `POWERBENCH_AFFINITY`
//! (`off` | `all` | `p`), чтобы его можно было выбрать без пересборки.

use std::fmt::Write as _;

#[cfg(windows)]
mod imp {
    //! Реализация поверх Win32. На не-Windows платформах — заглушка.

    use super::Placement;
    use std::mem::offset_of;
    use std::ptr;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::SystemInformation::{
        GROUP_AFFINITY, GetLogicalProcessorInformationEx, PROCESSOR_RELATIONSHIP,
        RelationProcessorCore, SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, SetThreadAffinityMask, SetThreadGroupAffinity,
    };

    /// Последний код ошибки Windows.
    fn last_error() -> u32 {
        unsafe { windows_sys::Win32::Foundation::GetLastError() }
    }

    /// Признак «на ядре включён SMT» из `PROCESSOR_RELATIONSHIP::Flags`.
    const LTP_PC_SMT: u8 = 0x01;

    /// Одно физическое ядро, как его сообщила ОС.
    #[derive(Debug, Clone)]
    pub struct CoreInfo {
        /// Класс эффективности: меньше — производительнее.
        pub efficiency_class: u8,
        /// SMT включён на ядре (признак `LTP_PC_SMT`).
        pub smt: bool,
        /// Логические процессоры ядра: (группа, индекс бита).
        pub threads: Vec<Placement>,
    }

    pub(crate) fn query_cores() -> Result<Vec<CoreInfo>, String> {
        // Двухпроходный вызов: сначала узнаём нужный размер, потом читаем.
        let mut needed: u32 = 0;
        unsafe {
            GetLogicalProcessorInformationEx(RelationProcessorCore, ptr::null_mut(), &mut needed);
        }
        if needed == 0 {
            return Err(format!(
                "GetLogicalProcessorInformationEx не вернул размер (код {})",
                last_error()
            ));
        }
        // Размер округляем вверх с запасом: структуры ОС на разных сборках
        // различаются, и усечение буфера здесь — порча памяти.
        let alloc = (needed as usize).saturating_add(4096);
        let mut buf = vec![0u8; alloc];
        let mut len = alloc as u32;
        let ok = unsafe {
            GetLogicalProcessorInformationEx(
                RelationProcessorCore,
                buf.as_mut_ptr()
                    .cast::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>(),
                &mut len,
            )
        };
        if ok == 0 {
            return Err(format!(
                "GetLogicalProcessorInformationEx: код {} ({} байт)",
                last_error(),
                len
            ));
        }

        let mut out = Vec::with_capacity(256);
        let total = (len as usize).min(buf.len());
        // Запись начинается с заголовка { Relationship: i32, Size: u32 }.
        let header = size_of::<usize>() * 2;
        let union_off = offset_of!(SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX, Anonymous);
        let mut offset = 0usize;
        while offset + header <= total {
            let rec = unsafe { buf.as_ptr().add(offset) };
            let relationship = unsafe { ptr::read_unaligned(rec as *const i32) };
            let size =
                unsafe { ptr::read_unaligned(rec.add(size_of::<i32>()) as *const u32) } as usize;
            if size < union_off || offset + size > total {
                break;
            }
            if relationship == RelationProcessorCore {
                // Объединение лежит ПОСЛЕ заголовка. Приведение к
                // PROCESSOR_RELATIONSHIP без этого смещения читает поля из
                // Relationship/Size и Reserved — GroupCount тогда всегда ноль,
                // то есть ни одного ядра не находится НИКОГДА. Ровно такая
                // ошибка выглядит снаружи невинно: разведка сообщает «0 ядер».
                let rel_ptr = unsafe { rec.add(union_off) } as *const PROCESSOR_RELATIONSHIP;
                let rel = unsafe { &*rel_ptr };
                let group_count = rel.GroupCount as usize;
                // GroupMask идёт сразу за телом PROCESSOR_RELATIONSHIP, то
                // есть тоже со смещением объединения — иначе маска читалась бы
                // из Reserved и выглядела бы как «старшие биты номера ядра».
                let mask_base =
                    unsafe { rec.add(union_off + offset_of!(PROCESSOR_RELATIONSHIP, GroupMask)) }
                        as *const GROUP_AFFINITY;
                let mut threads = Vec::with_capacity(group_count);
                for gi in 0..group_count {
                    let ga = unsafe { &*mask_base.add(gi) };
                    let mask = ga.Mask as u64;
                    for bit in 0..64u32 {
                        if mask & (1u64 << bit) != 0 {
                            threads.push(Placement {
                                group: ga.Group,
                                bit: bit as u8,
                            });
                        }
                    }
                }
                if !threads.is_empty() {
                    out.push(CoreInfo {
                        efficiency_class: rel.EfficiencyClass,
                        smt: rel.Flags & LTP_PC_SMT != 0,
                        threads,
                    });
                }
            }
            offset += size;
        }
        if out.is_empty() {
            return Err("ОС не сообщила ни одного физического ядра".to_string());
        }
        Ok(out)
    }

    /// Поставить маску текущего потока. `false` — ОС отказала.
    ///
    /// Сначала `SetThreadGroupAffinity`: он учитывает группы процессоров и
    /// корректен на машинах с более чем 64 логическими процессорами.
    /// `SetThreadAffinityMask` — путь для группы 0, где маска задаётся в
    /// терминах самой группы.
    pub fn apply_current_thread(p: &Placement) -> bool {
        let thread: HANDLE = unsafe { GetCurrentThread() };
        let mask: usize = 1usize << p.bit.min(63);
        if p.group == 0 {
            let prev = unsafe { SetThreadAffinityMask(thread, mask) };
            return prev != 0;
        }
        let ga = GROUP_AFFINITY {
            Mask: mask,
            Group: p.group,
            Reserved: [0; 3],
        };
        let mut previous = GROUP_AFFINITY {
            Mask: 0,
            Group: 0,
            Reserved: [0; 3],
        };
        unsafe { SetThreadGroupAffinity(thread, &ga, &mut previous) != 0 }
    }

    /// Причина, по которой ОС отказала в маске (для журнала).
    pub(crate) fn last_error_text() -> String {
        format!("код {}", last_error())
    }
}

#[cfg(not(windows))]
mod imp {
    use super::Placement;

    #[derive(Debug, Clone)]
    pub struct CoreInfo {
        pub efficiency_class: u8,
        pub smt: bool,
        pub threads: Vec<Placement>,
    }

    pub(crate) fn query_cores() -> Result<Vec<CoreInfo>, String> {
        Err("топология доступна только на Windows".to_string())
    }
    pub fn apply_current_thread(_p: &Placement) -> bool {
        false
    }
    pub(crate) fn last_error_text() -> String {
        "платформа не Windows".to_string()
    }
}

/// Режим привязки потоков к ядрам.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AffinityMode {
    /// По одному потоку на физическое производительное ядро. По умолчанию.
    #[default]
    POnly,
    /// По одному потоку на логический процессор.
    AllLogical,
    /// Привязки нет.
    AffinityOff,
}

impl AffinityMode {
    /// Имя режима для подписи совместимости и отчёта.
    pub const fn as_str(self) -> &'static str {
        match self {
            AffinityMode::POnly => "p-only",
            AffinityMode::AllLogical => "all-logical",
            AffinityMode::AffinityOff => "off",
        }
    }

    /// Разбор имени режима (из переменной окружения или настроек).
    pub fn parse(text: &str) -> Option<Self> {
        match text
            .trim()
            .to_ascii_lowercase()
            .replace(['_', ' '], "-")
            .as_str()
        {
            "p" | "p-only" | "ponly" | "p-cores" => Some(AffinityMode::POnly),
            "all" | "all-logical" | "alllogical" | "logical" => Some(AffinityMode::AllLogical),
            "off" | "none" | "no" => Some(AffinityMode::AffinityOff),
            _ => None,
        }
    }

    /// Режим по умолчанию: `POWERBENCH_AFFINITY`, иначе P-ядра.
    ///
    /// Переменная окружения нужна для машин, где маски запрещены политикой:
    /// там ОС откажет в `SetThreadAffinityMask`, и без возможности выключить
    /// привязку бенчмарк был бы непригоден.
    pub fn resolve_default() -> Self {
        std::env::var("POWERBENCH_AFFINITY")
            .ok()
            .and_then(|v| Self::parse(&v))
            .unwrap_or_default()
    }
}

/// Место для одного потока: логический процессор `(группа, индекс бита)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Placement {
    pub group: u16,
    pub bit: u8,
}

impl Placement {
    pub const fn new(group: u16, bit: u8) -> Self {
        Self { group, bit }
    }
}

/// Снимок топологии под выбранный режим привязки.
#[derive(Debug, Clone)]
pub struct CpuTopology {
    mode: AffinityMode,
    /// Физические производительные ядра: первый логический процессор каждого.
    p_cores: Vec<Placement>,
    /// Все логические процессоры по порядку (группа, затем бит).
    logical: Vec<Placement>,
    /// Классы эффективности, которые сообщила ОС (по возрастанию).
    efficiency_classes: Vec<u8>,
    /// ОС сообщила больше одного класса эффективности — процессор гибридный.
    hybrid: bool,
    /// Включён ли SMT (по признаку ядер).
    smt_enabled: bool,
    /// Сколько физических ядер всего (включая экономные).
    physical_cores: usize,
    /// Причина, по которой разведка не удалась (пусто — успех).
    error: Option<String>,
}

impl CpuTopology {
    /// Разведать топологию под режим. Ошибка не прерывает работу: пустой
    /// список слотов означает «привязку не применять».
    pub fn detect(mode: AffinityMode) -> Self {
        match imp::query_cores() {
            Ok(mut cores) => {
                // Порядок ядер из API не гарантирован, а подпись совместимости
                // обязана быть устойчивой к порядку: сортируем по месту.
                for c in cores.iter_mut() {
                    c.threads.sort();
                }
                cores.sort_by_key(|c| c.threads[0]);

                let min_eff = cores.iter().map(|c| c.efficiency_class).min().unwrap_or(0);
                let max_eff = cores.iter().map(|c| c.efficiency_class).max().unwrap_or(0);
                let mut classes: Vec<u8> = cores.iter().map(|c| c.efficiency_class).collect();
                classes.sort_unstable();
                classes.dedup();

                let smt_enabled = cores.iter().any(|c| c.smt);
                let physical_cores = cores.len();

                let mut p_cores: Vec<Placement> = cores
                    .iter()
                    // Класс эффективности — по минимуму, а не по нулю: на
                    // некоторых ЦО отсчёт начинается не с нуля, и сравнение с
                    // нулём дало бы ложный признак гибридности.
                    .filter(|c| c.efficiency_class == min_eff)
                    // По одному потоку на физическое ядро: берём младший
                    // логический процессор ядра, SMT-партнёра не трогаем.
                    .map(|c| c.threads[0])
                    .collect();
                p_cores.sort();
                p_cores.dedup();

                let mut logical: Vec<Placement> = cores
                    .iter()
                    .flat_map(|c| c.threads.iter().copied())
                    .collect();
                logical.sort();
                logical.dedup();

                Self {
                    mode,
                    p_cores,
                    logical,
                    efficiency_classes: classes,
                    hybrid: min_eff != max_eff,
                    smt_enabled,
                    physical_cores,
                    error: None,
                }
            }
            Err(e) => Self {
                mode,
                p_cores: Vec::new(),
                logical: Vec::new(),
                efficiency_classes: Vec::new(),
                hybrid: false,
                smt_enabled: false,
                physical_cores: 0,
                error: Some(e),
            },
        }
    }

    pub fn mode(&self) -> AffinityMode {
        self.mode
    }

    /// Гибридный ли процессор (есть и производительные, и экономные ядра).
    pub fn is_hybrid(&self) -> bool {
        self.hybrid
    }

    pub fn smt_enabled(&self) -> bool {
        self.smt_enabled
    }

    /// Сколько физических ядер сообщила ОС.
    pub fn physical_cores(&self) -> usize {
        self.physical_cores
    }

    /// Сколько производительных физических ядер.
    pub fn p_cores(&self) -> usize {
        self.p_cores.len()
    }

    /// Причина неудачной разведки, если она была.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Все места текущего режима (для подписи и отчёта).
    pub fn slots_list(&self) -> &[Placement] {
        match self.mode {
            AffinityMode::AffinityOff => &[],
            AffinityMode::POnly => &self.p_cores,
            AffinityMode::AllLogical => &self.logical,
        }
    }

    /// Сколько «рабочих мест» (потоков с разными ядрами) даёт режим.
    ///
    /// В режиме POnly мест столько же, сколько производительных физических
    /// ядер: второй поток на то же ядро не берём, иначе замер снова зависит
    /// от того, как Windows разложила SMT-пару.
    pub fn slots(&self) -> usize {
        self.slots_list().len()
    }

    /// Место для потока с номером `index` (0-based).
    pub fn slot(&self, index: usize) -> Option<Placement> {
        self.slots_list().get(index).copied()
    }

    /// Применить маску текущего потока по номеру `index`.
    ///
    /// `Ok(())` — маска поставлена либо ставить не нужно. `Err` — ОС отказала;
    /// это не фатально: замер продолжается, но в подпись попадает факт, что
    /// привязка не сработала, иначе результаты двух режимов склеились бы.
    pub fn bind_current_thread(&self, index: usize) -> Result<(), String> {
        let Some(p) = self.slot(index) else {
            // Привязка выключена или мест нет — привязывать нечего.
            return Ok(());
        };
        if imp::apply_current_thread(&p) {
            Ok(())
        } else {
            Err(imp::last_error_text())
        }
    }

    /// Подпись размещения для `CompatibilitySignature`.
    ///
    /// Одинаковая подпись означает, что прогоны выполнялись на одном и том же
    /// наборе ядер; разная — что их нельзя агрегировать (замер на P-ядрах и
    /// замер на «всех логических» — разные измерения).
    pub fn signature(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut header = String::with_capacity(96);
        let _ = write!(
            header,
            "affinity|{}|{}|{}|{}|{}|{}",
            self.mode.as_str(),
            self.hybrid as u8,
            self.smt_enabled as u8,
            self.p_cores.len(),
            self.logical.len(),
            self.physical_cores
        );
        let mut slots = String::with_capacity(self.slots().max(1) * 8);
        for p in self.slots_list() {
            let _ = write!(slots, "|{}:{}", p.group, p.bit);
        }
        let mut hasher = Sha256::new();
        hasher.update(header.as_bytes());
        // Раскладка входит в подпись: тот же режим на другом наборе ядер —
        // это другое измерение, и агрегировать их нельзя.
        hasher.update(slots.as_bytes());
        let digest = hasher.finalize();
        let mut hex = String::with_capacity(16);
        for b in digest.iter().take(8) {
            let _ = write!(hex, "{b:02x}");
        }
        format!("{}:{}", self.mode.as_str(), hex)
    }

    /// Человекочитаемое описание для журнала и отчёта.
    pub fn describe(&self) -> String {
        let mut s = String::new();
        let _ = write!(
            s,
            "{}: физических ядер {}, производительных {}, логических {}, слотов {}",
            self.mode.as_str(),
            self.physical_cores,
            self.p_cores.len(),
            self.logical.len(),
            self.slots()
        );
        if self.hybrid {
            let classes: Vec<String> = self
                .efficiency_classes
                .iter()
                .map(|c| c.to_string())
                .collect();
            let _ = write!(s, ", гибридный (классы {})", classes.join("/"));
        }
        if self.smt_enabled {
            s.push_str(", SMT включён");
        }
        if let Some(e) = &self.error {
            let _ = write!(s, "; разведка не удалась: {e}");
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_parsing_is_forgiving() {
        assert_eq!(AffinityMode::parse("p"), Some(AffinityMode::POnly));
        assert_eq!(AffinityMode::parse("P-Only"), Some(AffinityMode::POnly));
        assert_eq!(AffinityMode::parse(" PONLY "), Some(AffinityMode::POnly));
        assert_eq!(
            AffinityMode::parse("all_logical"),
            Some(AffinityMode::AllLogical)
        );
        assert_eq!(AffinityMode::parse("OFF"), Some(AffinityMode::AffinityOff));
        assert_eq!(AffinityMode::parse("none"), Some(AffinityMode::AffinityOff));
        assert_eq!(AffinityMode::parse("nonsense"), None);
        assert_eq!(AffinityMode::parse(""), None);
    }

    #[test]
    fn default_mode_is_p_only() {
        assert_eq!(AffinityMode::default(), AffinityMode::POnly);
        assert_eq!(AffinityMode::default().as_str(), "p-only");
    }

    /// Имя режима попадает в подпись совместимости и в отчёт: переименование
    /// делает все старые результаты несопоставимыми с новыми.
    #[test]
    fn mode_names_are_stable() {
        assert_eq!(AffinityMode::POnly.as_str(), "p-only");
        assert_eq!(AffinityMode::AllLogical.as_str(), "all-logical");
        assert_eq!(AffinityMode::AffinityOff.as_str(), "off");
    }

    /// Топология на любой машине обязана разведываться без паники; при
    /// неудаче разведки — пустые списки и записанная причина.
    #[test]
    fn detection_never_panics() {
        for mode in [
            AffinityMode::POnly,
            AffinityMode::AllLogical,
            AffinityMode::AffinityOff,
        ] {
            let topo = CpuTopology::detect(mode);
            // Подпись непуста в любом случае — иначе агрегация не отличит
            // «разведка не удалась» от «маска не применялась».
            assert!(!topo.signature().is_empty(), "{mode:?}: пустая подпись");
            assert!(!topo.describe().is_empty(), "{mode:?}: пустое описание");
            if topo.error().is_some() {
                assert_eq!(topo.slots(), 0, "{mode:?}: слоты при ошибке разведки");
                assert!(topo.slot(0).is_none());
            }
        }
    }

    /// Плейсменты отсортированы и уникальны: подпись строится по ним, а
    /// дубликаты означали бы, что два воркера получили одно ядро.
    #[test]
    fn placements_are_sorted_and_unique() {
        for mode in [AffinityMode::POnly, AffinityMode::AllLogical] {
            let topo = CpuTopology::detect(mode);
            let mut slots = topo.slots_list().to_vec();
            let before = slots.len();
            slots.sort();
            slots.dedup();
            assert_eq!(slots.len(), before, "{mode:?}: в слотах есть дубликаты");
            assert!(
                slots.windows(2).all(|w| w[0] <= w[1]),
                "{mode:?}: слоты не отсортированы"
            );
            assert_eq!(topo.slots(), before);
        }
    }

    /// Подпись одинакова при повторной разведке той же машины и различается
    /// для разных режимов: иначе агрегация склеила бы несопоставимые прогоны.
    #[test]
    fn signature_is_stable_per_mode() {
        let a = CpuTopology::detect(AffinityMode::POnly);
        let b = CpuTopology::detect(AffinityMode::POnly);
        assert_eq!(a.signature(), b.signature(), "подпись неустойчива");
        let c = CpuTopology::detect(AffinityMode::AffinityOff);
        assert_ne!(
            a.signature(),
            c.signature(),
            "режим привязки обязан входить в подпись"
        );
        assert!(a.signature().starts_with("p-only:"));
        assert!(c.signature().starts_with("off:"));
    }

    /// Слот с индексом за пределами списка — «нет места», а не паника.
    #[test]
    fn out_of_range_slot_is_none() {
        let topo = CpuTopology::detect(AffinityMode::POnly);
        assert!(topo.slot(topo.slots()).is_none());
        assert!(topo.slot(usize::MAX).is_none());
        // Выключенная привязка не предлагает мест вовсе.
        let off = CpuTopology::detect(AffinityMode::AffinityOff);
        assert_eq!(off.slots(), 0);
        assert!(off.slot(0).is_none());
        assert!(off.slots_list().is_empty());
    }

    /// Привязка применяется и читается обратно тем же API.
    ///
    /// Без такой проверки ошибка в FFI (смещение объединения, тип маски, группа)
    /// выглядит снаружи совершенно невинно: разведка сообщает о ядрах, а маска
    /// молча не ставится — и замер снова зависит от планировщика.
    #[cfg(windows)]
    #[test]
    fn binding_takes_effect_and_reads_back() {
        use windows_sys::Win32::System::SystemInformation::GROUP_AFFINITY;
        use windows_sys::Win32::System::Threading::{GetCurrentThread, GetThreadGroupAffinity};

        let topo = CpuTopology::detect(AffinityMode::AllLogical);
        let Some(target) = topo.slot(0) else {
            // Разведка не удалась — привязывать нечего, проверить нечего.
            assert!(
                topo.error().is_some(),
                "слотов нет, но и ошибки разведки нет"
            );
            return;
        };
        let thread = unsafe { GetCurrentThread() };
        assert!(
            imp::apply_current_thread(&target),
            "ОС отказала в маске для {target:?}"
        );
        let mut current = GROUP_AFFINITY {
            Mask: 0,
            Group: 0,
            Reserved: [0; 3],
        };
        let ok = unsafe { GetThreadGroupAffinity(thread, &mut current) };
        assert_ne!(ok, 0, "прочитать маску текущего потока не удалось");
        assert_eq!(current.Group, target.group, "маска попала не в ту группу");
        let expected: u64 = 1u64 << target.bit.min(63);
        assert_eq!(
            current.Mask as u64, expected,
            "маска установилась не на запрошенный логический процессор"
        );
    }

    /// Мест для воркеров не может быть больше, чем физических ядер: иначе
    /// часть воркеров останется без привязки и вернёт разброс планировщика.
    #[test]
    fn p_only_offers_at_most_one_slot_per_physical_core() {
        let topo = CpuTopology::detect(AffinityMode::POnly);
        if topo.error().is_some() {
            return;
        }
        assert!(topo.slots() <= topo.physical_cores());
        assert!(topo.slots() <= topo.p_cores());
        assert!(
            topo.slots() >= 1,
            "на любой машине есть хотя бы одно P-ядро"
        );
    }
}
