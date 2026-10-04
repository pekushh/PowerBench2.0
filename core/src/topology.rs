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

    /// Прочитать поле структуры по смещению, не требуя выравнивания буфера.
    ///
    /// Буфер разведки — `Vec<u8>`, то есть `align_of == 1`, а поля
    /// `PROCESSOR_RELATIONSHIP` и `GROUP_AFFINITY` требуют выравнивания 2 и 8.
    /// Создание ссылки на такое поле (`&*ptr` с приведением к типу структуры)
    /// — мгновенный UB: компилятор вправе считать поле выровненным и полагаться
    /// на это в оптимизациях, а процессор получит доступ по невыровненному
    /// адресу. Поэтому читаем копию через `read_unaligned`.
    ///
    /// # Безопасность
    ///
    /// Вызывающий обязан обеспечить `base + offset + size_of::<T>()` в границах
    /// буфера. Все смещения и границы проверяет [`parse_cores`].
    fn read_field<T: Copy>(base: *const u8, offset: usize) -> T {
        unsafe { ptr::read_unaligned(base.add(offset) as *const T) }
    }

    /// Смещение `GroupMask` внутри `PROCESSOR_RELATIONSHIP`.
    const MASKS_OFFSET: usize = offset_of!(PROCESSOR_RELATIONSHIP, GroupMask);

    /// Разобрать буфер `GetLogicalProcessorInformationEx` в список ядер.
    ///
    /// Вынесено отдельно от вызова API, потому что именно здесь принимаются
    /// решения о доверии к содержимому буфера — и именно они должны
    /// проверяться тестом, а не глазами на живой машине.
    ///
    /// # Безопасность
    ///
    /// `buf` — буфер, заполненный ОС; `total` — сколько байт ОС признала
    /// действительными. Записи за пределами `total` не читаются.
    pub(crate) fn parse_cores(buf: &[u8], total: usize) -> Vec<CoreInfo> {
        let mut out = Vec::with_capacity(256);
        let total = total.min(buf.len());
        // Запись начинается с заголовка { Relationship: i32, Size: u32 }.
        let header = size_of::<i32>() + size_of::<u32>();
        let union_off = offset_of!(SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX, Anonymous);
        let mask_size = size_of::<GROUP_AFFINITY>();
        let mut offset = 0usize;
        while offset + header <= total {
            let rec = unsafe { buf.as_ptr().add(offset) };
            let relationship = read_field::<i32>(rec, 0);
            let size = read_field::<u32>(rec, size_of::<i32>()) as usize;
            // Запись должна целиком помещаться в буфер и содержать объединение.
            if size < union_off || size > total - offset {
                break;
            }
            if relationship == RelationProcessorCore {
                // Объединение лежит ПОСЛЕ заголовка. Приведение к
                // PROCESSOR_RELATIONSHIP без этого смещения читает поля из
                // Relationship/Size и Reserved — GroupCount тогда всегда ноль,
                // то есть ни одного ядра не находится НИКОГДА. Ровно такая
                // ошибка выглядит снаружи невинно: разведка сообщает «0 ядер».
                let rel = unsafe { rec.add(union_off) };
                let group_count =
                    read_field::<u16>(rel, offset_of!(PROCESSOR_RELATIONSHIP, GroupCount)) as usize;
                let flags = read_field::<u8>(rel, offset_of!(PROCESSOR_RELATIONSHIP, Flags));
                let efficiency_class =
                    read_field::<u8>(rel, offset_of!(PROCESSOR_RELATIONSHIP, EfficiencyClass));

                // ГРАНИЦА ДОВЕРИЯ (регресс C7).
                //
                // `GroupCount` — это тоже данные извне: раньше он брался без
                // проверки, и цикл ниже шёл по `group_count` масок, читая их
                // адресом `mask_base.add(gi)`. Если ОС (или подмена буфера)
                // сообщала `GroupCount = 0xFFFF` при `Size` в несколько десятков
                // байт, цикл уходил далеко за конец буфера и читал кучу. Проверка
                // `size` на верхнюю границу от этого не спасала: она ограничивала
                // запись, а не количество масок внутри неё.
                //
                // Требуемое условие: в записи обязано помещаться
                // `masks_offset + group_count * size_of::<GROUP_AFFINITY>()`.
                // Умножение насыщающее — при огромном `GroupCount` оно даёт
                // usize::MAX, и сравнение с `size` отбрасывает запись.
                let needed = union_off
                    .checked_add(MASKS_OFFSET)
                    .and_then(|v| v.checked_add(group_count.saturating_mul(mask_size)))
                    .unwrap_or(usize::MAX);
                if needed > size {
                    break;
                }

                let mut threads = Vec::with_capacity(group_count);
                for gi in 0..group_count {
                    let ga = unsafe { rel.add(MASKS_OFFSET + gi * mask_size) };
                    let mask = read_field::<usize>(ga, offset_of!(GROUP_AFFINITY, Mask)) as u64;
                    let group = read_field::<u16>(ga, offset_of!(GROUP_AFFINITY, Group));
                    for bit in 0..64u32 {
                        if mask & (1u64 << bit) != 0 {
                            threads.push(Placement {
                                group,
                                bit: bit as u8,
                            });
                        }
                    }
                }
                if !threads.is_empty() {
                    out.push(CoreInfo {
                        efficiency_class,
                        smt: flags & LTP_PC_SMT != 0,
                        threads,
                    });
                }
            }
            offset += size;
        }
        out
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

        let out = parse_cores(&buf, len as usize);
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
        let _g = crate::tests::lock();
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
        let _g = crate::tests::lock();
        assert_eq!(AffinityMode::default(), AffinityMode::POnly);
        assert_eq!(AffinityMode::default().as_str(), "p-only");
    }

    /// Имя режима попадает в подпись совместимости и в отчёт: переименование
    /// делает все старые результаты несопоставимыми с новыми.
    #[test]
    fn mode_names_are_stable() {
        let _g = crate::tests::lock();
        assert_eq!(AffinityMode::POnly.as_str(), "p-only");
        assert_eq!(AffinityMode::AllLogical.as_str(), "all-logical");
        assert_eq!(AffinityMode::AffinityOff.as_str(), "off");
    }

    /// Топология на любой машине обязана разведываться без паники; при
    /// неудаче разведки — пустые списки и записанная причина.
    #[test]
    fn detection_never_panics() {
        let _g = crate::tests::lock();
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
        let _g = crate::tests::lock();
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
        let _g = crate::tests::lock();
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
        let _g = crate::tests::lock();
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
        let _g = crate::tests::lock();
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
        let _g = crate::tests::lock();
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

    // ------------------------------------------------------------------
    // Регресс C7: границы доверия к буферу FFI
    // ------------------------------------------------------------------

    /// Собрать синтетическую запись `RelationProcessorCore` ровно на
    /// `masks.len()` масок и длиной `extra` байт сверх необходимого.
    #[cfg(windows)]
    fn core_record(
        masks: &[(u16, u64)],
        extra: usize,
        group_count_override: Option<u16>,
    ) -> Vec<u8> {
        use std::mem::offset_of;
        use windows_sys::Win32::System::SystemInformation::{
            GROUP_AFFINITY, PROCESSOR_RELATIONSHIP, SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
        };
        let relation_core = windows_sys::Win32::System::SystemInformation::RelationProcessorCore;
        let union_off = offset_of!(SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX, Anonymous);
        let masks_off = offset_of!(PROCESSOR_RELATIONSHIP, GroupMask);
        let mask_size = size_of::<GROUP_AFFINITY>();
        let needed = union_off + masks_off + masks.len() * mask_size + extra;
        let mut buf = vec![0u8; needed];
        // Relationship
        buf[0..4].copy_from_slice(&relation_core.to_le_bytes());
        // Size
        buf[4..8].copy_from_slice(&(needed as u32).to_le_bytes());
        let rel = union_off;
        // Flags = LTP_PC_SMT, EfficiencyClass = 0
        buf[rel + offset_of!(PROCESSOR_RELATIONSHIP, Flags)] = 0x01;
        buf[rel + offset_of!(PROCESSOR_RELATIONSHIP, EfficiencyClass)] = 0;
        buf[rel + offset_of!(PROCESSOR_RELATIONSHIP, GroupCount)
            ..rel + offset_of!(PROCESSOR_RELATIONSHIP, GroupCount) + 2]
            .copy_from_slice(
                &group_count_override
                    .unwrap_or(masks.len() as u16)
                    .to_le_bytes(),
            );
        for (i, &(group, mask)) in masks.iter().enumerate() {
            let at = rel + masks_off + i * mask_size;
            buf[at..at + size_of::<usize>()].copy_from_slice(&mask.to_le_bytes());
            buf[at + offset_of!(GROUP_AFFINITY, Group)..at + offset_of!(GROUP_AFFINITY, Group) + 2]
                .copy_from_slice(&group.to_le_bytes());
        }
        buf
    }

    /// Реальная запись разбирается: маски, группы и признаки на месте.
    #[cfg(windows)]
    #[test]
    fn a_well_formed_core_record_is_parsed() {
        let _g = crate::tests::lock();
        let buf = core_record(&[(0, 0b1011), (1, 1 << 3)], 0, None);
        let cores = imp::parse_cores(&buf, buf.len());
        assert_eq!(cores.len(), 1, "запись не разобрана: {cores:?}");
        assert!(cores[0].smt, "признак SMT не прочитан");
        assert_eq!(cores[0].efficiency_class, 0);
        assert_eq!(
            cores[0].threads,
            vec![
                Placement { group: 0, bit: 0 },
                Placement { group: 0, bit: 1 },
                Placement { group: 0, bit: 3 },
                Placement { group: 1, bit: 3 },
            ],
            "маски разобраны неверно: {:?}",
            cores[0].threads
        );
    }

    /// Регресс C7: `GroupCount` больше, чем маски реально помещаются в запись.
    ///
    /// Раньше цикл шёл по `group_count` масок и читал их за концом буфера —
    /// то есть по куче. Теперь такая запись отбрасывается целиком.
    #[cfg(windows)]
    #[test]
    fn an_oversized_group_count_is_rejected_instead_of_read_past_the_buffer() {
        let _g = crate::tests::lock();
        // Одна настоящая маска, но GroupCount враньёт про 0xFFFF.
        let buf = core_record(&[(0, 0b1)], 0, Some(0xFFFF));
        let cores = imp::parse_cores(&buf, buf.len());
        assert!(
            cores.is_empty(),
            "запись с врущующим GroupCount принята: {cores:?}"
        );
        // Ровно на одну маску больше, чем есть, — тоже отказ.
        let buf = core_record(&[(0, 0b1)], 0, Some(2));
        assert!(imp::parse_cores(&buf, buf.len()).is_empty());
        // А на единицу меньше — parses (маска читается, лишняя просто не нужна).
        let buf = core_record(&[(0, 0b1), (1, 0b10)], 0, Some(1));
        let cores = imp::parse_cores(&buf, buf.len());
        assert_eq!(cores.len(), 1, "корректная запись отвергнута: {cores:?}");
    }

    /// Регресс C7: `GroupCount = 0xFFFF` при `Size`, умещающем в буфер, —
    /// именно тот случай, который проходил проверку `size` и вёл в кучу.
    #[cfg(windows)]
    #[test]
    fn a_record_whose_size_fits_but_masks_do_not_is_rejected() {
        let _g = crate::tests::lock();
        // Запись объявлена короче, чем нужно для 0xFFFF масок, но длиннее
        // минимума, — проверка `size` её пропускает.
        let mut buf = core_record(&[(0, 0b1)], 0, Some(0xFFFF));
        let shrink = buf.len() - 8;
        buf.truncate(shrink);
        buf[4..8].copy_from_slice(&(shrink as u32).to_le_bytes());
        assert!(imp::parse_cores(&buf, buf.len()).is_empty());
    }

    /// Регресс C7: буфер выровнен по единице, и это обязано быть безопасно.
    ///
    /// `Vec<u8>` имеет `align_of == 1`, а поля структур требуют 2 и 8. Создание
    /// ссылок `&PROCESSOR_RELATIONSHIP` / `&GROUP_AFFINITY` на такой буфер —
    /// UB; вместо них используется `read_unaligned`.
    ///
    /// Буфер подложки выровнен по 8 намеренно (`Vec<u64>`), поэтому сдвиг
    /// задаёт выравнивание детерминированно, а не в зависимости от того, что
    /// вернул аллокатор. Тест обязан работать и в debug, где включены проверки
    /// выравнивания.
    #[cfg(windows)]
    #[test]
    fn parsing_works_on_an_unaligned_buffer() {
        let _g = crate::tests::lock();
        let record = core_record(&[(0, 0b1101), (2, 0b1 << 40)], 16, None);
        let words = record.len().div_ceil(8) + 2;
        let mut aligned: Vec<u64> = vec![0; words];
        let base_bytes = aligned.as_mut_ptr().cast::<u8>();
        // Размёщаем запись со сдвигом 1: адрес `base + 1` гарантированно не
        // кратен 2 и не кратен 8 — то есть ровно тот случай, где `&*ptr` был UB.
        let shifted = 1usize;
        for (i, b) in record.iter().enumerate() {
            unsafe { base_bytes.add(shifted + i).write(*b) };
        }
        let start = unsafe { base_bytes.add(shifted) };
        assert_eq!(
            start as usize % align_of::<u64>(),
            shifted % align_of::<u64>()
        );
        assert_ne!(start as usize % align_of::<u16>(), 0);
        assert_ne!(start as usize % align_of::<u64>(), 0);

        let window = unsafe { std::slice::from_raw_parts(start, record.len()) };
        let cores = imp::parse_cores(window, record.len());
        assert_eq!(
            cores.len(),
            1,
            "запись не разобрана на невыровненном буфере"
        );
        assert_eq!(
            cores[0].threads,
            vec![
                Placement { group: 0, bit: 0 },
                Placement { group: 0, bit: 2 },
                Placement { group: 0, bit: 3 },
                Placement { group: 2, bit: 40 },
            ],
            "маски разобраны неверно: {:?}",
            cores[0].threads
        );
    }

    /// Мусор в буфере не приводит к панике и к чтению за его пределами.
    #[cfg(windows)]
    #[test]
    fn malformed_buffers_are_survivable() {
        let _g = crate::tests::lock();
        // Запись `Size` уходит за буфер.
        let mut bad = core_record(&[(0, 0b1)], 0, None);
        bad[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(imp::parse_cores(&bad, bad.len()).is_empty());

        // `Size = 0` — бесконечный цикл невозможен, но запись не читается.
        let mut zero = core_record(&[(0, 0b1)], 0, None);
        zero[4..8].copy_from_slice(&0u32.to_le_bytes());
        assert!(imp::parse_cores(&zero, zero.len()).is_empty());

        // `total` меньше реального буфаера — хвост не читается.
        let good = core_record(&[(0, 0b1)], 0, None);
        assert!(imp::parse_cores(&good, 4).is_empty());
        assert!(imp::parse_cores(&good, 0).is_empty());
        assert!(imp::parse_cores(&[], 0).is_empty());

        // Случайный шум.
        let noise: Vec<u8> = (0..512u32).map(|i| (i * 7) as u8).collect();
        let _ = imp::parse_cores(&noise, noise.len());
    }

    /// На живой машине разведка обязана работать: регрессионная проверка, что
    /// разбор вынесен в отдельную функцию без потери результата.
    #[cfg(windows)]
    #[test]
    fn live_detection_still_finds_cores() {
        let _g = crate::tests::lock();
        let topo = CpuTopology::detect(AffinityMode::AllLogical);
        assert!(
            topo.error().is_none(),
            "разведка сломана: {:?}",
            topo.error()
        );
        assert!(topo.physical_cores() > 0);
        assert!(topo.slots() > 0);
    }
}
