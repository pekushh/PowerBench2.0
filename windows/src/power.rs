//! Низкоуровневые утилиты Windows: права администратора, запрет сна,
//! питание от сети, снимок ограничений частоты и троттлинга, объём памяти и
//! сборка ОС, идентификатор CPU, частота таймера QPC, перекодирование
//! OEM-вывода в Unicode.

#[cfg(windows)]
use std::marker::PhantomData;

#[cfg(windows)]
use windows_sys::Win32::Globalization::MultiByteToWideChar;
#[cfg(windows)]
use windows_sys::Win32::System::Performance::QueryPerformanceFrequency;
#[cfg(windows)]
use windows_sys::Win32::System::Power::{
    CallNtPowerInformation, ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED,
    EXECUTION_STATE, GetSystemPowerStatus, PROCESSOR_POWER_INFORMATION, ProcessorInformation,
    SYSTEM_POWER_INFORMATION, SYSTEM_POWER_STATUS, SetThreadExecutionState, SystemPowerInformation,
};
#[cfg(windows)]
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
};
#[cfg(windows)]
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
#[cfg(windows)]
use windows_sys::Win32::UI::Shell::IsUserAnAdmin;

/// Версия диагностики (часть CompatibilitySignature).
///
/// `0.2.0` — с этой версии измеряется **только вычислительная часть тика**
/// (`main_stage`), а общее время батча с синхронизацией пула ушло в телеметрию
/// и темп тиков. Это меняет величину результата примерно в 2-3 раза при той же
/// самой нагрузке, поэтому версия поднята: без этого прогон, сделанный до и
/// после изменения, считался бы сопоставимым, и базовая линия машины сложила
/// бы две разные шкалы. `config_hash` здесь не помогает — он отражает объём
/// работы, который не изменился, а меняется только то, что именно замеряется.
pub const DIAGNOSTICS_VERSION: &str = "0.2.0";

/// Ошибка работы с электропитанием.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PowerError {
    /// Не удалось выставить запрет сна/отключения дисплея.
    SleepPreventionFailed,
    /// Не удалось определить статус питания (система вернула «неизвестно»).
    AcStatusUnknown,
}

/// Версия диагностики.
pub fn diagnostics_version() -> &'static str {
    DIAGNOSTICS_VERSION
}

/// Частота таймера QPC в Гц.
pub fn qpc_frequency() -> u64 {
    #[cfg(windows)]
    {
        let mut freq: i64 = 0;
        // Вызов успешен: QueryPerformanceFrequency не документирует природу
        // возвращаемого BOOL — при нуле принимаем консервативное значение.
        unsafe { QueryPerformanceFrequency(&mut freq) };
        if freq > 0 { freq as u64 } else { 10_000_000 }
    }
    #[cfg(not(windows))]
    {
        10_000_000
    }
}

/// Идентификатор CPU: `PROCESSOR_IDENTIFIER`, иначе архитектура.
pub fn cpu_identifier() -> String {
    std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| std::env::consts::ARCH.to_string())
}

/// Проверка прав администратора при старте.
///
/// `IsUserAnAdmin` — shell-API: он возвращает `true` для админской учётной
/// записи, запущенной **без повышения** (split token). Тогда диагностика
/// считала окружение готовым, а `powercfg /setactive` падал с «access denied»
/// уже посреди бенчмарка. Поэтому спрашиваем настоящее состояние токена:
/// `TokenElevation == 1` означает, что процесс действительно повышен.
pub fn is_admin() -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::HANDLE;
        use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TokenElevation};
        // Псевдодескриптор текущего процесса (GetCurrentProcess()).
        const CURRENT_PROCESS: HANDLE = std::ptr::null_mut();
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut returned: u32 = 0;
        let ok = unsafe {
            GetTokenInformation(
                CURRENT_PROCESS,
                TokenElevation,
                std::ptr::addr_of_mut!(elevation).cast(),
                std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                &mut returned,
            )
        };
        if ok != 0 {
            return elevation.TokenIsElevated != 0;
        }
        // Не удалось определить — не блокируем запуск из-за проверки.
        unsafe { IsUserAnAdmin() != 0 }
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Перекодировать байты вывода системной утилиты в Unicode.
///
/// Кодировка вывода `powercfg` **не фиксирована**: она следует за кодовой
/// страницей консоли, из которой запущена программа. В одном окружении это
/// CP866, в другом — UTF-8 (например, при включённой опции «Использовать
/// UTF-8» или когда у процесса нет консоли).
///
/// Раньше вывод всегда трактовался как OEM, из-за чего схемы с кириллическими
/// именами выглядели как «╨Ь╨░╨║╤Б╨╕╨╝» — валидный UTF-8, прочитанный как
/// однобайтовая кириллица. Поэтому сначала проверяем UTF-8 (строгий разбор),
/// и только если он невалиден — откатываемся на OEM-кодировку.
pub fn decode_oem(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    // Строгий разбор: `from_utf8` отвергает и неполные последовательности, и
    // одиночные байты CP866, поэтому ложных срабатываний на кириллице нет.
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    decode_oem_strict(bytes)
}

/// Декодирование строго по OEM-кодировке, без попытки угадать UTF-8.
fn decode_oem_strict(bytes: &[u8]) -> String {
    #[cfg(windows)]
    {
        const CP_OEMCP: u32 = 1;
        let needed = unsafe {
            MultiByteToWideChar(
                CP_OEMCP,
                0,
                bytes.as_ptr(),
                bytes.len() as i32,
                std::ptr::null_mut(),
                0,
            )
        };
        if needed <= 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        let mut buf = vec![0u16; needed as usize];
        let written = unsafe {
            MultiByteToWideChar(
                CP_OEMCP,
                0,
                bytes.as_ptr(),
                bytes.len() as i32,
                buf.as_mut_ptr(),
                needed,
            )
        };
        if written <= 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        buf.truncate(written as usize);
        String::from_utf16(&buf).unwrap_or_else(|_| String::from_utf8_lossy(bytes).into_owned())
    }
    #[cfg(not(windows))]
    {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// Запрет сна и отключения дисплея на время сессии; снимается при Drop.
///
/// `SetThreadExecutionState` возвращает **предыдущее** состояние потока, и его
/// нужно вернуть при снятии guard'а. Прежний код ставил `ES_CONTINUOUS` (то
/// есть «разрешить сон»), из-за чего два вложенных guard'а ломали друг друга:
/// снятие внешнего разрешало сон, пока внутренний ещё был жив.
#[derive(Debug)]
pub struct SleepGuard {
    #[cfg(windows)]
    previous: EXECUTION_STATE,
    #[cfg(windows)]
    active: bool,
    /// Гард привязан к ПОТОКУ, а не к процессу (регресс H21).
    ///
    /// `SetThreadExecutionState` действует на вызывающий поток, и снимать
    /// состояние тоже должен он же. Если guard уехал бы в другой поток через
    /// `move`, запрет сна висел бы на чужом потоке, а исходный вернулся бы в
    /// `ES_CONTINUOUS` слишком рано — и «сон отменён на время замера» тихо
    /// перестало бы работать в самый неподходящий момент.
    ///
    /// `PhantomData<*const ()>` делает тип `!Send` и `!Sync`: это запрещено
    /// ровно то, что запрещено делать с ресурсом потока. Компилятор
    /// гарантирует, что guard не покинет поток, в котором был создан.
    ///
    /// Поле не под `cfg(windows)`: привязка к потоку — свойство типа, а не
    /// платформенная деталь, и на других платформах guard точно так же нельзя
    /// отдавать другому потоку.
    #[doc = r#"
    `SleepGuard` нельзя отправить в другой поток — тип намеренно `!Send`:

```compile_fail
use powerbench_windows::power::SleepGuard;
fn требует_send<T: Send>() {}
требует_send::<SleepGuard>();
```
"#]
    _thread_bound: PhantomData<*const ()>,
}

impl SleepGuard {
    /// Выставить запрет сна. Повторный вызов создаёт независимый guard.
    pub fn prevent() -> Result<Self, PowerError> {
        #[cfg(windows)]
        {
            let flags: EXECUTION_STATE = ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED;
            let prev = unsafe { SetThreadExecutionState(flags) };
            if prev == 0 {
                return Err(PowerError::SleepPreventionFailed);
            }
            Ok(Self {
                previous: prev,
                active: true,
                _thread_bound: PhantomData,
            })
        }
        #[cfg(not(windows))]
        {
            Ok(Self {})
        }
    }
}

impl Drop for SleepGuard {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            if self.active {
                // Возвращаем состояние, которое было до guard'а, а не «разрешить
                // сон»: так вложенные guard'а работают корректно.
                unsafe { SetThreadExecutionState(self.previous) };
                self.active = false;
            }
        }
    }
}

/// Питание от сети? `Err` — статус неизвестен (255) или вызов завершился ошибкой.
pub fn ac_power_online() -> Result<bool, PowerError> {
    #[cfg(windows)]
    {
        // SYSTEM_POWER_STATUS не реализует Default в windows-sys.
        let mut status: SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
        let ok = unsafe { GetSystemPowerStatus(&mut status) };
        if ok == 0 {
            return Err(PowerError::AcStatusUnknown);
        }
        match status.ACLineStatus {
            1 => Ok(true),
            0 => Ok(false),
            _ => Err(PowerError::AcStatusUnknown),
        }
    }
    #[cfg(not(windows))]
    {
        Ok(true)
    }
}

/// Признак того, что процессор ушёл в пассивное охлаждение (троттлинг):
/// ACPI сообщает `CoolingMode = Passive` (1), когда вентилятор не справляется и
/// система снижает частоты вместо того, чтобы охлаждать.
const COOLING_MODE_PASSIVE: u16 = 1;

/// Снимок ограничений питания и частоты на момент замера.
///
/// Читается через `CallNtPowerInformation` — штатный интерфейс Power Manager.
/// Это не «температура в градусах» (её без драйвера не достать), а то, что
/// действительно важно для достоверности замера:
///
/// * `max_mhz` / `current_mhz` — частота, которую система разрешает и которую
///   выдаёт процессор. Схема питания с пониженным максимальным состоянием
///   уменьшает `max_mhz`, а троттлинг уменьшает `current_mhz`.
///   **Важно:** `max_mhz` — это текущий разрешённый потолок *с учётом буста*,
///   а не базовая частота процессора. Базовая частота в этих данных
///   отсутствует, поэтому равенство `current_mhz == max_mhz` доказывает лишь
///   «система не держит процессор ниже разрешённого потолка» и не доказывает,
///   что буст отрабатывает полностью;
/// * `throttled` — какое-то ядро удерживается заметно ниже **собственного**
///   потолка прямо сейчас;
/// * `thermal_throttle` — пассивное охлаждение, то есть причина ограничения
///   именно температура;
/// * `policy_reason` — флаги источника ограничения (см. `throttle_cause`),
///   0 — ограничений нет.
///
/// По «тепли»: нагрев смещает все последующие прогоны сессии, а ротация
/// порядка его не компенсирует, поэтому такой признак пишется в каждый прогон
/// и попадает в отчёт.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub struct PowerState {
    /// Наибольшая разрешённая частота по всем ядрам, МГц.
    pub max_mhz: u32,
    /// Наименьшая выданная частота по всем ядрам, МГц.
    pub current_mhz: u32,
    /// Система ограничивает частоту прямо сейчас.
    pub throttled: bool,
    /// Причина ограничения — температура (пассивное охлаждение).
    pub thermal_throttle: bool,
    /// Флаги источника ограничения; 0 — ограничений нет.
    pub policy_reason: u32,
    /// Сколько система выдержит без сна, минут (0 — неизвестно/без предела).
    ///
    /// Раньше это значение (`MaxIdlenessAllowed`) выдавалось как код
    /// ACPI-ограничения; теперь оно используется по назначению — для отчёта о
    /// среде. Для длинной сессии важно знать, не уйдёт ли машина в сон посреди
    /// замера.
    pub max_idle_minutes: u32,
    /// Питание от сети на момент замера.
    pub on_ac: bool,
    /// Данные получить не удалось (вызов вернул ошибку).
    pub unavailable: bool,
}

/// Флаги источника ограничения частоты (`PowerState::policy_reason`).
///
/// Регресс H22: в это поле раньше клали `SYSTEM_POWER_INFORMATION::MaxIdlenessAllowed`
/// — «максимальное время простоя до сна в единицах по 64 с». Это НЕ код
/// ACPI-ограничения, и выводилось оно в отчёте как «ACPI-код N». То есть
/// отчёт утверждал конкретную причину ограничения, которой не знает система:
/// отказ троттлинга (`_PROCESSOR_THROTTLE`) выглядел как «ACPI-код 5».
///
/// Теперь здесь только то, что действительно известно Power Manager.
pub mod throttle_cause {
    /// Пассивное охлаждение: процессор снижает частоту сам, по температуре.
    pub const PASSIVE_COOLING: u32 = 1 << 0;
    /// Активное охлаждение: вентиляторы работают, система охлаждает принудительно.
    pub const ACTIVE_COOLING: u32 = 1 << 1;
    /// Ядро удерживается ниже собственного разрешённого потолка.
    pub const HELD_BELOW_CEILING: u32 = 1 << 2;
    /// Система сообщает об ограничении частоты процессора.
    pub const PROCESSOR_THROTTLE: u32 = 1 << 3;

    /// Человекочитаемое описание набора флагов (пусто — ограничений нет).
    pub fn describe(flags: u32) -> String {
        if flags == 0 {
            return String::new();
        }
        let mut parts: Vec<&str> = Vec::new();
        if flags & PASSIVE_COOLING != 0 {
            parts.push("пассивное охлаждение");
        }
        if flags & ACTIVE_COOLING != 0 {
            parts.push("активное охлаждение");
        }
        if flags & HELD_BELOW_CEILING != 0 {
            parts.push("ядро ниже своего потолка");
        }
        if flags & PROCESSOR_THROTTLE != 0 {
            parts.push("троттлинг процессора");
        }
        if parts.is_empty() {
            format!("неизвестные флаги {flags:#x}")
        } else {
            parts.join(", ")
        }
    }
}

/// Допуск, в пределах которого разница `max_mhz`/`current_mhz` считается
/// шумом: Windows округляет частоту до ступеней, и «на пару МГц ниже» ещё не
/// троттлинг.
const MHZ_TOLERANCE: u32 = 50;

/// Вынесено отдельно от [`power_state`], чтобы правило можно было проверить
/// на конкретных значениях: система обязана честно различать пассивное
/// охлаждение (температура) и активное (вентиляторы), и не выдавать ни то,
/// ни другое, если охлаждение выключено.
fn cooling_cause(cooling_mode: u16) -> Option<u32> {
    if cooling_mode == COOLING_MODE_PASSIVE {
        Some(throttle_cause::PASSIVE_COOLING)
    } else if cooling_mode > COOLING_MODE_PASSIVE {
        Some(throttle_cause::ACTIVE_COOLING)
    } else {
        None
    }
}

/// Сборка снимка питания из уже полученных значений.
///
/// Регресс H22: `policy_reason` собирается только из того, что Power
/// Manager действительно сообщает. `MaxIdlenessAllowed` сюда больше не
/// попадает — это «время простоя до сна в единицах по 64 с», а не код
/// ограничения (см. [`throttle_cause`]).
fn power_state_from(clocks: Clocks, sys: SystemPower, on_ac: Option<bool>) -> PowerState {
    let max_mhz = clocks.ceiling_mhz;
    let current_mhz = clocks.slowest_mhz;
    let mut policy_reason = 0u32;
    if let Some(cause) = cooling_cause(sys.cooling_mode) {
        policy_reason |= cause;
    }
    if clocks.held_below_own_ceiling {
        policy_reason |= throttle_cause::HELD_BELOW_CEILING;
    }
    PowerState {
        max_mhz,
        current_mhz,
        throttled: clocks.held_below_own_ceiling,
        thermal_throttle: sys.cooling_mode == COOLING_MODE_PASSIVE,
        policy_reason,
        // Единицы `MaxIdlenessAllowed` — по 64 секунды, приводим к минутам.
        max_idle_minutes: sys.max_idleness_units.saturating_mul(64) / 60,
        on_ac: on_ac.unwrap_or(false),
        unavailable: max_mhz == 0 && sys.cooling_mode == 0,
    }
}
/// Снимок состояния питания. На не-Windows платформах недоступен.
pub fn power_state() -> PowerState {
    #[cfg(windows)]
    {
        let clocks = processor_clocks();
        let sys = system_power_info();
        power_state_from(clocks, sys, ac_power_online().ok())
    }
    #[cfg(not(windows))]
    {
        PowerState {
            unavailable: true,
            ..PowerState::default()
        }
    }
}

/// Что удалось выяснить о частоте процессора.
///
/// Важное о смысле полей: `PROCESSOR_POWER_INFORMATION::MaxMhz` — это **не
/// базовая частота**, а тот потолок, который Windows разрешает *прямо сейчас*,
/// то есть на бусте ускорения. Поэтому равенство `current == max` означает
/// лишь «процессор не удерживается ниже разрешённого потолка», и НЕ означает
/// «буст отрабатывает полностью»: застрять на базовой частоте этот способ
/// обнаружить не может — базовой частоты в данных просто нет.
struct Clocks {
    /// Наивысший потолок среди ядер (для показа).
    ceiling_mhz: u32,
    /// Наименьшая текущая частота среди ядер (для показа).
    slowest_mhz: u32,
    /// Есть ли ядро, удерживаемое ниже **собственного** потолка.
    ///
    /// Сравнение идёт по каждому ядру отдельно. Прежняя версия брала
    /// максимум потолков и минимум текущих частот, то есть сравнивала
    /// разные ядра между собой: на процессорах с разными ядрами (P/E)
    /// это давало ложное «троттлинг» там, где его нет.
    held_below_own_ceiling: bool,
}

/// Сколько записей `PROCESSOR_POWER_INFORMATION` запрашивать у Power Manager.
///
/// Регресс H24: буфер был жёстко рассчитан на 256 процессоров «с запасом
/// больше любой реальной машины». На серверных процессорах (EPYC/Xeon на 128–512
/// потоков) этого запаса нет: `CallNtPowerInformation` возвращает по записи на
/// КАЖДЫЙ логический процессор, и запрос обрезался. Часть ядер не попадала в
/// разбор, а потолок и минимальная частота считались по случайному подмножеству
/// — то есть на такой машине значения в отчёте были просто неверными.
///
/// Берём счёт из `available_parallelism` с двойным запасом (Power Manager
/// считает и SMT-партнёров, которых `available_parallelism` иногда не видит),
/// а снизу — 1024 записи. Если ОС всё же ответит «буфер мал», запрос
/// удваивается: пусть лучше лишняя запись, чем молчаливая порча.
fn processor_buffer_len() -> usize {
    let hw = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    hw.saturating_mul(2).max(1024)
}

/// `(наибольший MaxMhz, наименьший CurrentMhz, удерживается ли кто-то ниже своего потолка)`.
#[cfg(windows)]
fn processor_clocks() -> Clocks {
    let mut want = processor_buffer_len();
    let mut buf: Vec<PROCESSOR_POWER_INFORMATION> = Vec::new();
    // Не больше пяти удвоений: 1024 → 2048 → … → 32768 записей, это ~1 МБ.
    // Дальше расти бессмысленно — столько логических процессоров не бывает.
    let mut attempts = 0;
    let status = loop {
        buf.clear();
        buf.resize(want, unsafe { std::mem::zeroed() });
        let bytes = std::mem::size_of::<PROCESSOR_POWER_INFORMATION>()
            .saturating_mul(want)
            .min(u32::MAX as usize) as u32;
        let status = unsafe {
            CallNtPowerInformation(
                ProcessorInformation,
                std::ptr::null(),
                0,
                buf.as_mut_ptr() as *mut std::ffi::c_void,
                bytes,
            )
        };
        attempts += 1;
        // Успех, либо буфер заведомо достаточен (потолок достигнут).
        if status == 0 || attempts >= 5 {
            break status;
        }
        want = want.saturating_mul(2);
    };
    // STATUS_SUCCESS == 0; NTSTATUS типа u32 в windows-sys.
    if status != 0 {
        return Clocks {
            ceiling_mhz: 0,
            slowest_mhz: 0,
            held_below_own_ceiling: false,
        };
    }
    clocks_from_buffer(&buf)
}

/// Разбор ответа `CallNtPowerInformation` — отдельно от самого вызова,
/// чтобы правило «каждое ядро сравнивается со своим потолком» можно было
/// проверить тестом на конкретных цифрах.
#[cfg(windows)]
fn clocks_from_buffer(buf: &[PROCESSOR_POWER_INFORMATION]) -> Clocks {
    let mut ceiling_mhz = 0u32;
    // Регресс H25 (продолжение): «нет ни одного ядра» раньше помечалось
    // значением `u32::MAX`, и это же значение — настоящая частота. Ядро,
    // идущее ровно на 4294967295 МГц (счётчики и виртуализаторы такое
    // отдают), терялось, и разбор молча сообщал «информации нет».
    // Поэтому «ядер нет» — это `None`, а не магическое число.
    let mut slowest: Option<u32> = None;
    let mut held = false;
    for p in buf.iter() {
        // Нулевая запись — хвост буфера, реального ядра с 0 МГц не бывает.
        if p.MaxMhz == 0 && p.CurrentMhz == 0 {
            continue;
        }
        ceiling_mhz = ceiling_mhz.max(p.MaxMhz);
        slowest = Some(match slowest {
            Some(prev) => prev.min(p.CurrentMhz),
            None => p.CurrentMhz,
        });
        // Каждое ядро сравнивается со своим же потолком.
        //
        // Регресс H25: стояло `p.CurrentMhz + MHZ_TOLERANCE < p.MaxMhz`, то есть
        // сложение `u32`. При `CurrentMhz` вблизи `u32::MAX` (а такие значения
        // отдают некоторые виртуализаторы и счётчики) сложение переполнялось и
        // в режиме release **переворачивалось**, из-за чего «держится ниже своего
        // потолка» становилось ложным на самых горячих ядрах — то есть
        // троттлинг не видел именно там, где он опаснее всего.
        //
        // Формула переписана без сложения: `current < max − tol` (для потолков
        // меньше допуска «ниже потолка» быть не может вовсе).
        //
        // `CurrentMhz == 0` при ненулевом потолке — это тоже «ниже своего
        // потолка»: ядро существует (потолок известен), но не работает.
        if p.MaxMhz > 0
            && p.MaxMhz
                .checked_sub(MHZ_TOLERANCE)
                .is_some_and(|limit| p.CurrentMhz < limit)
        {
            held = true;
        }
    }
    match slowest {
        None => Clocks {
            ceiling_mhz: 0,
            slowest_mhz: 0,
            held_below_own_ceiling: false,
        },
        Some(slowest_mhz) => Clocks {
            ceiling_mhz,
            slowest_mhz,
            held_below_own_ceiling: held,
        },
    }
}

/// Данные `SYSTEM_POWER_INFORMATION`, которые действительно что-то значат.
#[cfg(windows)]
struct SystemPower {
    /// Режим охлаждения: 0 — охлаждение выключено, 1 — пассивное,
    /// 2 и выше — активное (принудительное).
    cooling_mode: u16,
    /// Максимальное время простоя до сна, в единицах по 64 с.
    ///
    /// Регресс H22: это поле больше НЕ выдаётся как код ограничения. По
    /// документации Windows это «сколько система выдержит без сна», а не
    /// причина троттлинга; выводить его как «ACPI-код» значило утверждать
    /// причину, которой нет.
    max_idleness_units: u32,
}

/// `(режим охлаждения, время простоя до сна)`.
#[cfg(windows)]
fn system_power_info() -> SystemPower {
    let mut info: SYSTEM_POWER_INFORMATION = unsafe { std::mem::zeroed() };
    let status = unsafe {
        CallNtPowerInformation(
            SystemPowerInformation,
            std::ptr::null(),
            0,
            &mut info as *mut SYSTEM_POWER_INFORMATION as *mut std::ffi::c_void,
            std::mem::size_of::<SYSTEM_POWER_INFORMATION>() as u32,
        )
    };
    if status != 0 {
        return SystemPower {
            cooling_mode: 0,
            max_idleness_units: 0,
        };
    }
    SystemPower {
        cooling_mode: info.CoolingMode,
        max_idleness_units: info.MaxIdlenessAllowed,
    }
}

/// Объём физической памяти, ГБ (округление до сотых).
pub fn memory_gib() -> f64 {
    #[cfg(windows)]
    {
        let mut mem: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        // Без dwLength вызов возвращает FALSE: структура обязана сообщить свой
        // размер (так же работает MEMORYSTATUS, но не все знают).
        mem.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        let ok = unsafe { GlobalMemoryStatusEx(&mut mem) };
        if ok != 0 {
            return mem.ullTotalPhys as f64 / (1024.0 * 1024.0 * 1024.0);
        }
    }
    0.0
}

/// Сборка и ревизия ОС (`22631.4169` и т. п.).
///
/// Читается из реестра, а не через `GetVersionEx`: последняя начиная с
/// Windows 8.1 сообщает версию, подменённую приложением, если у манифеста нет
/// `supportedOS` (то есть почти всегда «6.2»). Настоящий номер обновления
/// хранится в `CurrentBuild`, а ревизия — в `UBR`; без него две сборки,
/// отличающиеся сотнями патчей, выглядели бы одинаково, а именно такие
/// обновления меняют поведение планировщика и планировщика питания.
pub fn os_build() -> String {
    #[cfg(windows)]
    {
        let major = registry_string(
            "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion",
            "CurrentBuild",
        );
        if major.is_empty() {
            return String::new();
        }
        // UBR хранится как DWORD, а не как строка, — иначе ревизия терялась бы.
        let ubr = registry_dword("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion", "UBR");
        if ubr == 0 {
            major
        } else {
            format!("{major}.{ubr}")
        }
    }
    #[cfg(not(windows))]
    {
        String::new()
    }
}

/// Имя процессора глазами человека («AMD Ryzen 7 5800X3D»).
///
/// `cpu_identifier` даёт семейство/модель/степпинг — этого достаточно для
/// сравнения сессий между собой, но в отчёте нечитаемо. Брендовое имя лежит в
/// реестре, и читать его оттуда надёжнее, чем разбирать CPUID.
pub fn cpu_brand() -> String {
    #[cfg(windows)]
    {
        let brand = registry_string(
            "HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0",
            "ProcessorNameString",
        );
        let brand = brand.trim().to_string();
        if !brand.is_empty() {
            return brand;
        }
    }
    String::new()
}

/// Значение строкового параметра реестра (`HKEY_LOCAL_MACHINE`), пусто при ошибке.
///
/// Буфер прежней версии был фиксированным 128 байт: длинные строки
/// (`ProcessorNameString` на некоторых сборках, пути к прошивке) давали
/// `ERROR_MORE_DATA`, функция молча возвращала пустое значение, и identity
/// машины теряла данные без единого признака ошибки.
#[cfg(windows)]
fn registry_string(subkey: &str, value: &str) -> String {
    const ERROR_MORE_DATA: u32 = 234;
    let subkey: Vec<u16> = subkey.encode_utf16().chain(std::iter::once(0)).collect();
    let value: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let mut units: usize = 64;
    for _ in 0..4 {
        let mut buf = vec![0u16; units];
        let mut len: u32 = (buf.len() * 2) as u32;
        let rc = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE as HKEY,
                subkey.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr() as *mut std::ffi::c_void,
                &mut len,
            )
        };
        if rc == 0 {
            // Без завершающего нуля обрезаем строку, а не выдаём хвост мусора.
            let mut text = String::from_utf16_lossy(&buf[..(len as usize / 2).min(buf.len())]);
            while text.ends_with('\0') {
                text.pop();
            }
            return text;
        }
        if rc != ERROR_MORE_DATA {
            return String::new();
        }
        // `len` при ERROR_MORE_DATA содержит нужный размер в байтах.
        let needed = len as usize / 2 + 1;
        if needed <= units {
            return String::new();
        }
        units = needed;
    }
    String::new()
}

/// Значение DWORD из реестра, 0 при ошибке или другом типе.
#[cfg(windows)]
fn registry_dword(subkey: &str, value: &str) -> u32 {
    let subkey: Vec<u16> = subkey.encode_utf16().chain(std::iter::once(0)).collect();
    let value: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let mut out: u32 = 0;
    let mut len: u32 = std::mem::size_of::<u32>() as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE as HKEY,
            subkey.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            &mut out as *mut u32 as *mut std::ffi::c_void,
            &mut len,
        )
    };
    if rc == 0 { out } else { 0 }
}

/// Починить строку, испорченную однобайтовым декодером.
/// Старый код читал UTF-8 вывод `powercfg` как CP866. Ошибка обратима: если
/// обратная перекодировка в OEM даёт валидный UTF-8 с кириллицей, значит
/// перед нами именно такая порча. Нужно для истории, записанной до исправления.
///
/// Возвращает `None`, если строка не похожа на испорченную.
pub fn repair_mojibake(text: &str) -> Option<String> {
    // Признак порчи — символы псевдографики CP866, попавшие на место букв.
    if !text.chars().any(|c| ('\u{2550}'..='\u{259F}').contains(&c)) {
        return None;
    }
    let bytes = encode_oem(text);
    let fixed = String::from_utf8(bytes).ok()?;
    // Починено: кириллица появилась, псевдографики не осталось.
    let has_cyrillic = fixed
        .chars()
        .any(|c| ('а'..='я').contains(&c) || ('А'..='Я').contains(&c) || c == 'ё' || c == 'Ё');
    let still_box = fixed
        .chars()
        .any(|c| ('\u{2550}'..='\u{259F}').contains(&c));
    if has_cyrillic && !still_box {
        Some(fixed)
    } else {
        None
    }
}

/// Закодировать строку в OEM-кодировку текущей системы.
fn encode_oem(text: &str) -> Vec<u8> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Globalization::WideCharToMultiByte;
        const CP_OEMCP: u32 = 1;
        let utf16: Vec<u16> = text.encode_utf16().collect();
        if utf16.is_empty() {
            return Vec::new();
        }
        let needed = unsafe {
            WideCharToMultiByte(
                CP_OEMCP,
                0,
                utf16.as_ptr(),
                utf16.len() as i32,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
                std::ptr::null_mut(),
            )
        };
        if needed <= 0 {
            return text.as_bytes().to_vec();
        }
        let mut buf = vec![0u8; needed as usize];
        let written = unsafe {
            WideCharToMultiByte(
                CP_OEMCP,
                0,
                utf16.as_ptr(),
                utf16.len() as i32,
                buf.as_mut_ptr(),
                needed,
                std::ptr::null(),
                std::ptr::null_mut(),
            )
        };
        if written <= 0 {
            return text.as_bytes().to_vec();
        }
        buf.truncate(written as usize);
        buf
    }
    #[cfg(not(windows))]
    {
        text.as_bytes().to_vec()
    }
}

/// Смещение местного времени относительно UTC в секундах на момент `epoch_secs`.
///
/// Зачем это нужно приложению. Интерфейс показывает время в местной зоне
/// (это делает JavaScript), а отчёт для поддержки должен называть метки
/// записей в том же времени — иначе «ошибка в 20:38» из журнала и «20:38» из
/// обращения пользователя разойдутся на разницу часов, и время ошибки
/// определить будет невозможно.
///
/// Смещение берётся у Windows, а не вычисляется вручную: правила перехода на
/// летнее время меняются политикой региона, и самодельный разбор `TIME_ZONE_INFORMATION`
/// рано или поздно ошибётся на переходе. Смещение вычисляется один раз для
/// текущего момента, поэтому для записей журнала из другого полугодия (когда
/// действовало другое правило) возможна ошибка в час — для разбора ошибок это
/// несущественно, о чём и сказано в документации функции.
#[cfg(windows)]
pub fn local_offset_secs(epoch_secs: u64) -> i64 {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::Time::TzSpecificLocalTimeToSystemTime;

    let secs = epoch_secs as i64;
    let (days, sod) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    let as_systemtime = |secs_of_day: i64| SYSTEMTIME {
        wYear: y as u16,
        wMonth: m as u16,
        wDayOfWeek: 0,
        wDay: d as u16,
        wHour: (secs_of_day / 3600) as u16,
        wMinute: ((secs_of_day % 3600) / 60) as u16,
        wSecond: (secs_of_day % 60) as u16,
        wMilliseconds: 0,
    };
    // Считаем текущий момент «местным» и просим Windows перевести его в
    // универсальное. Если сказать «это 18:56 по местному», Windows вернёт
    // 15:56 UTC, то есть `универсальное = местное − смещение`, откуда
    // `смещение = местное − универсальное`. Знак легко перепутать, поэтому
    // он зафиксирован тестом `local_offset_matches_the_system_clock`.
    let local = as_systemtime(sod);
    let mut universal = SYSTEMTIME {
        wYear: 0,
        wMonth: 0,
        wDayOfWeek: 0,
        wDay: 0,
        wHour: 0,
        wMinute: 0,
        wSecond: 0,
        wMilliseconds: 0,
    };
    let ok = unsafe { TzSpecificLocalTimeToSystemTime(std::ptr::null(), &local, &mut universal) };
    if ok == 0 {
        return 0;
    }
    let universal_days = days_from_civil(
        universal.wYear as i64,
        universal.wMonth as i64,
        universal.wDay as i64,
    );
    let universal_sod = (universal.wHour as i64) * 3600
        + (universal.wMinute as i64) * 60
        + universal.wSecond as i64;
    secs - (universal_days * 86_400 + universal_sod)
}

/// Дни от Unix-эпохи до календарной даты (обратная к `civil_from_days`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Дата из дней от Unix-эпохи.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Текущее местное время в виде `ДД.ММ.ГГГГ ЧЧ:ММ:СС` (сдвиг зоны учтён).
pub fn local_time_string(epoch_secs: u64) -> String {
    let local = epoch_secs as i64 + local_offset_secs(epoch_secs);
    let days = local.div_euclid(86_400);
    let sod = local.rem_euclid(86_400);
    let (y, mo, d) = civil_from_days(days);
    format!(
        "{d:02}.{mo:02}.{y} {:02}:{:02}:{:02}",
        sod / 3600,
        (sod % 3600) / 60,
        sod % 60
    )
}

/// Смещение местной зоны текстом: «UTC+03:00». Для отчёта: без него
/// сравнивать метки записей с метками в интерфейсе приходится на глаз.
pub fn local_offset_label(epoch_secs: u64) -> String {
    let off = local_offset_secs(epoch_secs);
    let sign = if off < 0 { '-' } else { '+' };
    let a = off.abs();
    format!("UTC{sign}{:02}:{:02}", a / 3600, (a % 3600) / 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core(max: u32, current: u32) -> PROCESSOR_POWER_INFORMATION {
        PROCESSOR_POWER_INFORMATION {
            Number: 0,
            MaxMhz: max,
            CurrentMhz: current,
            MhzLimit: 0,
            MaxIdleState: 0,
            CurrentIdleState: 0,
        }
    }

    /// Ядра с разными потолками (например, P/E) не должны давать ложного
    /// «троттлинга»: каждое ядро сравнивается со своим же потолком.
    ///
    /// Прежняя версия брала максимум потолков и минимум текущих частот, то
    /// есть сравнивала 3000 МГц с чужим потолком 5200 МГц и объявляла
    /// ограничение там, где его нет.
    #[test]
    fn asymmetric_cores_are_not_called_throttled() {
        let buf = [core(5200, 5190), core(3000, 2990)];
        let c = clocks_from_buffer(&buf);
        assert_eq!(c.ceiling_mhz, 5200);
        assert_eq!(c.slowest_mhz, 2990);
        assert!(
            !c.held_below_own_ceiling,
            "оба ядра идут на своих потолках — ограничения нет"
        );
    }

    /// Настоящее ограничение ловится: ядро заметно ниже своего потолка.
    #[test]
    fn core_below_its_own_ceiling_is_throttled() {
        let buf = [core(5200, 5190), core(5200, 3100)];
        let c = clocks_from_buffer(&buf);
        assert!(
            c.held_below_own_ceiling,
            "ядро на 3100 при потолке 5200 — это ограничение"
        );
    }

    /// Хвост буфера (нулевые записи) не должен считаться ядром с 0 МГц.
    #[test]
    fn zero_tail_is_ignored() {
        let buf = [core(5200, 5190), core(0, 0), core(0, 0)];
        let c = clocks_from_buffer(&buf);
        assert_eq!(c.ceiling_mhz, 5200);
        assert_eq!(
            c.slowest_mhz, 5190,
            "нулевой хвост не должен понижать частоту"
        );
        assert!(!c.held_below_own_ceiling);
    }

    /// Полностью пустой ответ — «информации нет», а не «троттлинг».
    #[test]
    fn empty_buffer_means_no_information() {
        let c = clocks_from_buffer(&[]);
        assert_eq!(c.ceiling_mhz, 0);
        assert_eq!(c.slowest_mhz, 0);
        assert!(!c.held_below_own_ceiling);
    }

    /// Регресс H25: сравнение не переполняется при частотах у потолка `u32`.
    ///
    /// Раньше стояло `CurrentMhz + MHZ_TOLERANCE < MaxMhz` — сложение `u32`.
    /// При `CurrentMhz = u32::MAX` (такие значения отдают некоторые
    /// виртуализаторы и счётчики) сложение переворачивалось в release, и «ядро
    /// держится ниже потолка» становилось ложным именно на самых горячих
    /// ядрах, где троттлинг опаснее всего.
    #[test]
    fn extreme_frequency_does_not_wrap_around() {
        // Потолок — максимум u32: `max - tol` уже почти равен самому потолку,
        // но переполнения быть не должно.
        let buf = [core(u32::MAX, u32::MAX)];
        let c = clocks_from_buffer(&buf);
        assert_eq!(c.ceiling_mhz, u32::MAX);
        assert_eq!(c.slowest_mhz, u32::MAX);
        assert!(
            !c.held_below_own_ceiling,
            "ядро на своём потолке не может быть «ниже потолка»"
        );

        // А вот настоящее ограничение при тех же экстремальных числах обязано
        // быть замечено: 4294960000 против 4294967295 — это выше допуска.
        let held = clocks_from_buffer(&[core(u32::MAX, u32::MAX - 10)]);
        assert!(
            !held.held_below_own_ceiling,
            "разница меньше допуска — ограничения нет"
        );
        let held = clocks_from_buffer(&[core(u32::MAX, u32::MAX - 1000)]);
        assert!(
            held.held_below_own_ceiling,
            "ядро на 1000 МГц ниже потолка — это ограничение"
        );
    }

    /// Потолок меньше допуска: `max - tol` не существует, и переполнение при
    /// вычитании не должно выглядеть как «очень большой предел».
    #[test]
    fn a_ceiling_below_the_tolerance_is_never_throttled() {
        let c = clocks_from_buffer(&[core(1, 1)]);
        assert!(
            !c.held_below_own_ceiling,
            "при потолке 1 МГц ядро не может быть ниже потолка"
        );
        // И ненулевой потолок при нулевой текущей частоте — это ограничение.
        let c = clocks_from_buffer(&[core(5000, 0)]);
        assert!(
            c.held_below_own_ceiling,
            "частота упала до нуля при потолке 5000 — это ограничение"
        );
    }

    /// Регресс H24: размера буфера хватает любой реальной машине.
    ///
    /// Буфер был жёстко рассчитан на 256 процессоров. На серверных CPU
    /// (EPYC/Xeon на 128–512 потоков) этого запаса нет: запрос обрезался, и
    /// потолок с минимальной частотой считались по случайному подмножеству
    /// ядер — то есть в отчёте были просто неверные значения.
    #[test]
    fn the_power_buffer_covers_every_logical_processor() {
        let want = processor_buffer_len();
        assert!(
            want >= 1024,
            "буфер меньше 1024 записей: на серверных процессорах запрос снова \
             обрежется, текущее значение {want}"
        );
        let hw = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        assert!(
            want >= hw.saturating_mul(2),
            "буфер {want} не перекрывает {hw} потоков с запасом для SMT-партнёров"
        );
        // И размер не должен переполняться при передаче в FFI (u32 байт).
        let bytes = std::mem::size_of::<PROCESSOR_POWER_INFORMATION>()
            .saturating_mul(want)
            .min(u32::MAX as usize) as u32;
        assert!(
            bytes > 0,
            "размер запроса нулевой — CallNtPowerInformation не вызовется"
        );
        // Старый фиксированный размер обязан быть позади.
        assert!(want > 256, "вернулся фиксированный буфер на 256 записей");
    }

    /// Регресс H22: `policy_reason` содержит только настоящие причины.
    ///
    /// Вынесено отдельно от [`power_state`], чтобы правило можно было проверить
    /// на конкретных значениях.
    ///
    /// Раньше в это поле клали `SYSTEM_POWER_INFORMATION::MaxIdlenessAllowed` —
    /// «максимальное время простоя до сна в единицах по 64 с». Это НЕ код
    /// ACPI-ограничения, но в отчёте выводилось как «ACPI-код N»: отчёт
    /// утверждал конкретную причину ограничения, которой система не знает.
    /// Отказ троттлинга (`_PROCESSOR_THROTTLE`) при `MaxIdlenessAllowed == 5`
    /// выглядел как «ACPI-код 5».
    #[test]
    fn policy_reason_never_contains_the_idleness_limit() {
        // `MaxIdlenessAllowed` меняется от 0 до 255 единиц по 64 с — то есть
        // практически любое число. Ни одно из них не должно попасть в
        // `policy_reason` как «код ограничения».
        for idleness_units in [0u32, 1, 5, 42, 100, 255] {
            let state = power_state_from(
                Clocks {
                    ceiling_mhz: 5200,
                    slowest_mhz: 5190,
                    held_below_own_ceiling: false,
                },
                SystemPower {
                    cooling_mode: 0,
                    max_idleness_units: idleness_units,
                },
                Some(true),
            );
            assert_eq!(
                state.policy_reason, 0,
                "при охлаждении выключено и полном потолке причина ограничения \
                 должна быть нулевой, но MaxIdlenessAllowed={idleness_units} дал \
                 policy_reason={}",
                state.policy_reason
            );
            // А сам лимит времени простоя обязан сохраниться как есть.
            assert_eq!(
                state.max_idle_minutes,
                idleness_units * 64 / 60,
                "время простоя до сна посчитано неверно"
            );
            assert!(!state.throttled);
        }
        // И наоборот: лимит простоя не должен маскировать настоящую причину.
        let state = power_state_from(
            Clocks {
                ceiling_mhz: 5200,
                slowest_mhz: 3000,
                held_below_own_ceiling: true,
            },
            SystemPower {
                cooling_mode: 0,
                max_idleness_units: 255,
            },
            Some(true),
        );
        assert!(
            state.throttled && state.policy_reason != 0,
            "настоящее ограничение потерялось из-за значения времени простоя"
        );
        assert_ne!(
            state.policy_reason, 255,
            "в policy_reason снова лежит MaxIdlenessAllowed"
        );
    }

    /// Причины ограничения различаются, а не схлопываются в одно число.
    #[test]
    fn cooling_modes_map_to_distinct_causes() {
        assert_eq!(
            cooling_cause(COOLING_MODE_PASSIVE),
            Some(throttle_cause::PASSIVE_COOLING),
            "пассивное охлаждение — это температура, а не вентиляторы"
        );
        assert_eq!(
            cooling_cause(COOLING_MODE_PASSIVE + 1),
            Some(throttle_cause::ACTIVE_COOLING)
        );
        assert_eq!(
            cooling_cause(COOLING_MODE_PASSIVE + 2),
            Some(throttle_cause::ACTIVE_COOLING)
        );
        assert_eq!(cooling_cause(0), None, "охлаждение выключено — причины нет");
        // Тепловой троттлинг обязан совпадать с пассивным охлаждением.
        let passive = power_state_from(
            Clocks {
                ceiling_mhz: 5200,
                slowest_mhz: 5190,
                held_below_own_ceiling: false,
            },
            SystemPower {
                cooling_mode: COOLING_MODE_PASSIVE,
                max_idleness_units: 5,
            },
            Some(true),
        );
        assert!(
            passive.thermal_throttle,
            "тепловой признак не выставлен при пассивном охлаждении"
        );
        let active = power_state_from(
            Clocks {
                ceiling_mhz: 5200,
                slowest_mhz: 5190,
                held_below_own_ceiling: false,
            },
            SystemPower {
                cooling_mode: COOLING_MODE_PASSIVE + 1,
                max_idleness_units: 5,
            },
            Some(true),
        );
        assert!(
            !active.thermal_throttle,
            "активное охлаждение не должно выдаваться за нагрев"
        );
        assert_ne!(passive.policy_reason, active.policy_reason);
    }

    /// Флаги обязаны читаться человеком, а не выдаваться шестнадцатеричным кодом.
    #[test]
    fn throttle_flags_have_a_readable_description() {
        use throttle_cause::{
            ACTIVE_COOLING, HELD_BELOW_CEILING, PASSIVE_COOLING, PROCESSOR_THROTTLE,
        };
        assert_eq!(throttle_cause::describe(0), "", "нулевой набор не описан");
        for (flags, expected) in [
            (PASSIVE_COOLING, "пассивное охлаждение"),
            (ACTIVE_COOLING, "активное охлаждение"),
            (HELD_BELOW_CEILING, "ядро ниже своего потолка"),
            (PROCESSOR_THROTTLE, "троттлинг процессора"),
        ] {
            let text = throttle_cause::describe(flags);
            assert_eq!(text, expected);
            assert!(
                !text.contains("0x") && !text.chars().any(|c| c.is_ascii_digit()),
                "в описании остался код вместо причины: {text}"
            );
        }
        // Набор флагов описывается целиком.
        let both = throttle_cause::describe(PASSIVE_COOLING | HELD_BELOW_CEILING);
        assert_eq!(
            both, "пассивное охлаждение, ядро ниже своего потолка",
            "{both}"
        );
        // Неизвестные биты не теряются молча.
        let unknown = throttle_cause::describe(1 << 20);
        assert!(unknown.contains("неизвестные"), "{unknown}");
        // Описание обязано быть пригодным для отчёта: без сырых `ACPI`-кодов.
        for flags in 0u32..64 {
            let text = throttle_cause::describe(flags);
            assert!(!text.contains("ACPI"), "ACPI-код снова в описании: {text}");
        }
    }

    /// Флаги обязаны быть непересекающимися, иначе описание врёт.
    #[test]
    fn throttle_flags_do_not_overlap() {
        use throttle_cause::{
            ACTIVE_COOLING, HELD_BELOW_CEILING, PASSIVE_COOLING, PROCESSOR_THROTTLE,
        };
        let all = [
            PASSIVE_COOLING,
            ACTIVE_COOLING,
            HELD_BELOW_CEILING,
            PROCESSOR_THROTTLE,
        ];
        for (i, a) in all.iter().enumerate() {
            assert_eq!(a.count_ones(), 1, "флаг {a:#x} не один бит");
            for b in &all[i + 1..] {
                assert_eq!(a & b, 0, "флаги {a:#x} и {b:#x} пересекаются");
            }
        }
    }

    /// Регресс H21: `SleepGuard` обязан быть привязан к потоку.
    ///
    /// `WaitForSingleObject`/`Sleep` ждут конкретный поток, но сам guard можно
    /// уронить в другом потоке — тогда ожидание останется висеть, а отмена
    /// будет потеряна. Тип обязан запрещать `Send`/`Sync` статически.
    ///
    /// Отрицательное ограничение проверяется doctest'ом `compile_fail` на самом
    /// типе (`SleepGuard` нельзя отправить в другой поток), а здесь мы
    /// проверяем, что маркер привязки вообще есть и не под `cfg(windows)`:
    /// иначе на не-Windows платформах ограничение молча исчезает.
    #[test]
    fn sleep_guard_is_bound_to_its_thread() {
        let src = include_str!("power.rs");
        let start = src
            .find("pub struct SleepGuard")
            .expect("не найден SleepGuard");
        let end = src[start..]
            .find("impl SleepGuard")
            .map(|i| start + i)
            .expect("не найден impl SleepGuard");
        let body = &src[start..end];
        assert!(
            body.contains("PhantomData<*const ()>"),
            "у SleepGuard нет PhantomData<*const ()>: guard можно уронить в чужом \
             потоке и потерять отмену ожидания. Поле:\n{body}"
        );
        assert!(
            !body.contains("#[cfg(windows)]\n    _thread_bound"),
            "маркер привязки к потоку спрятан под cfg(windows): на других \
             платформах тип снова станет Send"
        );
        // И конструктор обязан его инициализировать: иначе поле не существует
        // в рантайме, даже если объявлено.
        let ctor_start = src
            .find("impl SleepGuard")
            .expect("не найден impl SleepGuard");
        let ctor_end = src[ctor_start..]
            .find("impl Drop for SleepGuard")
            .map(|i| ctor_start + i)
            .unwrap_or(src.len());
        assert!(
            src[ctor_start..ctor_end].contains("_thread_bound: PhantomData"),
            "SleepGuard не инициализирует маркер привязки"
        );
        // Ненулевой размер и осмысленный отпечаток: «пустой» тип на Windows
        // выглядел бы подозрительно.
        assert!(
            std::mem::size_of::<SleepGuard>() >= std::mem::size_of::<usize>(),
            "SleepGuard не занимает места — маркер привязки не тот"
        );
    }

    #[test]
    fn cpu_identifier_falls_back_to_architecture() {
        let id = cpu_identifier();
        assert!(!id.trim().is_empty());
    }

    #[test]
    fn oem_decoding_is_stable_for_ascii() {
        // ASCII-вывод в любом кодовом стиле даёт ту же строку.
        let decoded = decode_oem(b"GUID scheme: abc");
        assert_eq!(decoded, "GUID scheme: abc");
    }

    /// Регресс: `powercfg` на этой машине отдаёт UTF-8, и раньше вывод всегда
    /// читался как CP866 — кириллические имена схем превращались в «╨Ь╨░╨║».
    #[test]
    fn utf8_output_survives_decoding() {
        let src = "Максимальная производительность";
        let bytes = src.as_bytes();
        assert!(std::str::from_utf8(bytes).is_ok());
        assert_eq!(decode_oem(bytes), src);
    }

    /// ASCII-путь не должен ломаться, даже если весь вывод валидный UTF-8.
    #[test]
    fn mixed_ascii_and_cyrillic_is_decoded_as_utf8() {
        let src = "GUID схемы питания: 07d147ca-d013-40ae-80ee-ae3c98650195";
        assert_eq!(decode_oem(src.as_bytes()), src);
    }

    /// Уже испорченные имена в истории должны чиниться: «╨Ь╨░╨║╤Б╨╕╨╝» —
    /// это UTF-8, прочитанный как CP866, и операция обратима.
    #[test]
    fn corrupted_cyrillic_is_repairable() {
        let src = "Высокая производительность";
        // Воспроизводим ровно то, что делал старый декодер.
        let wrong = decode_oem_strict(src.as_bytes());
        assert_ne!(wrong, src, "имитация порчи не сработала");
        assert_eq!(repair_mojibake(&wrong).as_deref(), Some(src));
    }

    /// Текст без порчи чинить нечего.
    #[test]
    fn valid_text_is_left_alone() {
        assert_eq!(repair_mojibake("Обычная схема"), None);
        assert_eq!(repair_mojibake("Velo's Power Plan"), None);
        assert_eq!(repair_mojibake(""), None);
    }

    #[test]
    fn diagnostics_version_is_fixed() {
        assert_eq!(diagnostics_version(), "0.2.0");
        // Версия диагностики входит в CompatibilitySignature: её смена
        // обязана запрещать агрегацию прогонов, сделанных по разным
        // правилам измерения. Поэтому она непуста и осмысленна.
        assert!(diagnostics_version().split('.').count() == 3);
    }

    /// Смещение зоны обязано быть целым числом минут и правдоподобным по
    /// величине: это единственная защита от молчаливого мусора, если FFI
    /// вернёт не то (например, если структура SYSTEMTIME соберётся неверно).
    #[test]
    fn local_offset_is_plausible() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let off = local_offset_secs(now);
        assert!(
            (-14 * 3600..=14 * 3600).contains(&off),
            "смещение {off} с не похоже на часовой пояс"
        );
        assert_eq!(off % 60, 0, "смещение {off} с не кратно минуте");
    }

    /// Дата в местном времени и календарная арифметика обязаны быть
    /// согласованы: иначе отчёт напишет «31.02» или «29.02» не того года, и
    /// это будет выглядеть как «приложение сошло с ума».
    /// Смещение обязано совпадать с тем, что показывает сама система: иначе
    /// отчёт напишет время на несколько часов раньше или позже интерфейса,
    /// и это будет выглядеть как «в отчёте ерунда».
    #[test]
    fn local_offset_matches_the_system_clock() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let off = local_offset_secs(now);
        let local = now as i64 + off;
        // Сравниваем с системными часами через ту же WinAPI: независимый
        // источник времени (DateTimeOffset в PowerShell) в тест не пустить,
        // но сам факт «локальное = UTC + смещение» проверяется кругом.
        let (y, mo, d) = civil_from_days(local.div_euclid(86_400));
        let sod = local.rem_euclid(86_400);
        let text = local_time_string(now);
        assert_eq!(
            text,
            format!(
                "{d:02}.{mo:02}.{y} {:02}:{:02}:{:02}",
                sod / 3600,
                (sod % 3600) / 60,
                sod % 60
            ),
            "местное время не сходится с самим собой: {text}"
        );
        // Метка зоны обязана соответствовать знаку смещения.
        let label = local_offset_label(now);
        assert_eq!(
            label.starts_with("UTC-"),
            off < 0,
            "метка {label} не соответствует смещению {off} с"
        );
    }

    #[test]
    fn local_time_string_is_a_valid_calendar_date() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let text = local_time_string(now);
        assert!(
            text.len() == 19,
            "неожиданный формат местного времени: {text}"
        );
        let (date, time) = text.split_once(' ').expect("нет пробела в дате");
        let d: Vec<&str> = date.split('.').collect();
        let t: Vec<&str> = time.split(':').collect();
        assert_eq!(d.len(), 3, "дата должна быть ДД.ММ.ГГГГ: {text}");
        assert_eq!(t.len(), 3, "время должно быть ЧЧ:ММ:СС: {text}");
        let (day, month, year) = (
            d[0].parse::<u32>().unwrap(),
            d[1].parse::<u32>().unwrap(),
            d[2].parse::<i32>().unwrap(),
        );
        assert!((1..=31).contains(&day), "неверный день: {text}");
        assert!((1..=12).contains(&month), "неверный месяц: {text}");
        assert!(year >= 2020, "неверный год: {text}");
        assert!(t[0].parse::<u32>().unwrap() <= 23, "неверный час: {text}");
        assert!(
            t[1].parse::<u32>().unwrap() <= 59,
            "неверная минута: {text}"
        );
        assert!(
            t[2].parse::<u32>().unwrap() <= 60,
            "неверные секунды: {text}"
        );
    }

    /// `days_from_civil` и `civil_from_days` обязаны быть обратными: всё
    /// местное время считается через эту пару.
    #[test]
    fn civil_date_conversions_round_trip() {
        for days in [-25_000i64, 0, 1, 19_723, 20_725, 40_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days, "не круг для {days}");
        }
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_725), (2026, 9, 29));
    }

    #[test]
    fn local_offset_label_is_readable() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let label = local_offset_label(now);
        assert!(
            label.starts_with("UTC+") || label.starts_with("UTC-"),
            "{label}"
        );
        assert_eq!(label.len(), 9, "ожидался вид UTC+03:00, получено {label}");
    }
}
