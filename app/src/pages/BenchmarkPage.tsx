// Страница «Бенчмарк»: визард Режим → Схемы → Запуск.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  commands,
  onTelemetry,
  onTestFinished,
  type CheckpointDto,
  type PhasePlanRow,
  type PresetDto,
  type QuarantineEntry,
  type Readiness,
  type SchemeRow,
  type SettingsDto,
  type TelemetryMsg,
} from "../api";
import { Button, Glass, Spot } from "../components/ui";
import { GearIcon } from "../components/icons";
import { pushToast, setRunning, useSession } from "../store";
import { SchemePicker, filterEligible, sortSchemes } from "../components/SchemeTiles";

type Stage = "mode" | "schemes" | "run";
type PresetKey = "quick" | "detailed" | "custom";

interface PresetDef {
  title: string;
  desc: string;
  expand: string;
}

const PRESETS: Record<"quick" | "detailed", PresetDef> = {
  quick: {
    title: "Быстро",
    desc: "Быстрая проверка производительности системы с минимальным временем тестирования.",
    expand:
      "Один повтор и короткие фазы — ответ за пару минут. Разница между схемами видна, но это режим скрининга: доверительного интервала здесь не существует, поэтому итог не будет обещать подтверждённый выбор.",
  },
  detailed: {
    title: "Детально",
    desc: "Расширенное тестирование для точного сравнения схем питания и стабильных результатов.",
    expand:
      "Длинные фазы и пять повторов — точнее и надёжнее, срабатывает адаптивная остановка. Подходит, когда схема выбирается на длительный срок.",
  },
};

type PresetParams = { duration: number; warmup: number; cooling: number; reps: number };

/** Числа пресета в виде полей мастера. */
function paramsOf(p: PresetDto): PresetParams {
  return {
    duration: p.duration_seconds,
    warmup: p.warmup_seconds,
    cooling: p.cooling_seconds,
    reps: p.repetitions,
  };
}

function sameParams(a: PresetParams, b: PresetParams): boolean {
  return (
    a.duration === b.duration &&
    a.warmup === b.warmup &&
    a.cooling === b.cooling &&
    a.reps === b.reps
  );
}

const PRESET_SHORT: Record<PresetKey, string> = {
  quick: "Быстрый",
  detailed: "Детальный",
  custom: "Пользовательские параметры",
};

const STAGES: { id: Stage; label: string }[] = [
  { id: "mode", label: "Режим и настройки" },
  { id: "schemes", label: "Схемы питания" },
  { id: "run", label: "Запуск" },
];

/**
 * Форматирование уже посчитанного времени (прошедшего или оставшегося).
 * Слово «около» и «прошло» задаёт вызывающий: здесь только единицы.
 */
export function fmtDuration(totalSeconds: number): string {
  if (!Number.isFinite(totalSeconds) || totalSeconds < 0) return "—";
  const t = Math.round(totalSeconds);
  if (t < 60) return `${t} с`;
  const m = Math.floor(t / 60);
  const s = t % 60;
  if (m < 60) return s === 0 ? `${m} мин` : `${m} мин ${s} с`;
  const h = Math.floor(m / 60);
  const rm = m % 60;
  return rm === 0 ? `${h} ч` : `${h} ч ${rm} мин`;
}

/** Телеметрия для показа: нефинитное значение — прочерк, а не «NaN». */
function tf(v: number | null | undefined, digits: number): string {
  return typeof v === "number" && Number.isFinite(v) ? v.toFixed(digits) : "—";
}

const T_TABLE = [12.706, 4.303, 3.182, 2.776, 2.571, 2.447, 2.365, 2.306, 2.262];

function earlyStopNeed(reps: number, cv = 0.05): number | null {
  if (reps < 2) return null;
  const n = T_TABLE[Math.min(reps - 2, T_TABLE.length - 1)];
  return ((2 * Math.SQRT2 * n * cv * 100) / Math.sqrt(reps));
}

function earlyStopHint(reps: number): string {
  if (!Number.isFinite(reps) || reps < 2)
    return "Адаптивная остановка невозможна при одном повторе: нужны минимум два прогона, чтобы оценить разброс.";
  const need = earlyStopNeed(reps) ?? Infinity;
  if (need >= 100)
    return `При ${reps} ${plural(reps, "повторе", "повторах", "повторах")} ранняя остановка практически недостижима (нужен перевес > 100% при разбросе прогонов ~5%). Рекомендуем 5+ повторов.`;
  return `Ранняя остановка возможна, если перевес ≈ ${need.toFixed(0)}% и более (при разбросе прогонов ~5%). Для практичного срабатывания рекомендуем 5+ повторов.`;
}

/** Русские склонения: 1 повтор / 2 повтора / 5 повторов. */
function plural(n: number, one: string, few: string, many: string): string {
  const a = Math.abs(Math.trunc(n)) % 100;
  const b = a % 10;
  if (a > 10 && a < 20) return many;
  if (b > 1 && b < 5) return few;
  if (b === 1) return one;
  return many;
}

/**
 * Порядковый номер текущей фазы в плане замера (для «готово» / «текущая»).
 *
 * Список фаз приходит из бэкенда (`phasePlan`): там же живёт формула деления
 * длительности. Раньше её копия стояла здесь, и после появления четвёртой фазы
 * интерфейс молча показывал бы три фазы вместо четырёх.
 */
function phaseIndex(plan: PhasePlanRow[], name: string | undefined): number {
  if (!name) return -1;
  return plan.findIndex((p) => p.name === name);
}

export default function BenchmarkPage() {
  const { running } = useSession();
  const [stage, setStage] = useState<Stage>("mode");
  const [dir, setDir] = useState<"fwd" | "back">("fwd");
  const [expanded, setExpanded] = useState(false);
  const [schemes, setSchemes] = useState<SchemeRow[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [settings, setSettings] = useState<SettingsDto | null>(null);
  const [preset, setPreset] = useState<"quick" | "detailed">("detailed");
  // Числа режимов приходят из `orchestrator::config` одной командой. Пока они
  // не пришли, мастер не применяет пресет: иначе карточка обещала бы одни
  // значения, а запустила бы другие (собственная копия чисел в интерфейсе уже
  // расходилась с бэкендом после правки пресета).
  const [presets, setPresets] = useState<Record<"quick" | "detailed", PresetParams> | null>(
    null,
  );
  const [duration, setDuration] = useState(0);
  const [warmup, setWarmup] = useState(0);
  const [cooling, setCooling] = useState(0);
  const [reps, setReps] = useState(0);
  const [backgroundThreshold, setBackgroundThresholdState] = useState(5);
  const [checkpoint, setCheckpoint] = useState<CheckpointDto | null>(null);
  const [checkpointError, setCheckpointError] = useState<string | null>(null);
  const [telemetry, setTelemetry] = useState<TelemetryMsg | null>(null);
  const [starting, setStarting] = useState(false);
  // Событие `test-finished` может прийти раньше ответа команды запуска: поток
  // сессии падает ещё до того, как команда вернёт pid. Флаг нужен, чтобы
  // оптимистичное `running = true` не залипло навсегда.
  const finishedRef = useRef(false);
  const [stopping, setStopping] = useState(false);
  const [finished, setFinished] = useState(false);
  const [finishMsg, setFinishMsg] = useState("");
  const [elapsed, setElapsed] = useState(0);
  const [readiness, setReadiness] = useState<Readiness | null>(null);
  const [quarantine, setQuarantine] = useState<QuarantineEntry[]>([]);
  const [estimate, setEstimate] = useState<string>("—");
  const t0 = useRef(0);
  // Полная оценка сессии в секундах — для расчёта «осталось» во время замера.
  const totalSecondsRef = useRef<number | null>(null);

  // Оценку времени для каждого пресета считает бэкенд из тех же констант,
  // что и планировщик: интерфейс не должен переписывать формулу, иначе цифры
  // в карточке режима и реальная длительность разойдутся.
  const [presetEstimates, setPresetEstimates] = useState<Record<string, string>>({});
  // План фаз приходит из бэкенда: формула деления длительности живёт там
  // одна, и копия в интерфейсе рано или поздно разошлась бы с планировщиком.
  const [phasePlan, setPhasePlan] = useState<PhasePlanRow[]>([]);
  const [oneScheme, setOneScheme] = useState("—");
  // Фоновая нагрузка CPU перед стартом, % (сэмплится бэкендом).
  const [bgSample, setBgSample] = useState<number | null>(null);

  // План фаз перечитывается при смене длительности: это подписи на экране
  // запуска, и брать их надо у того, кто действительно делит время.
  useEffect(() => {
    let alive = true;
    commands
      .phasePlan(duration)
      .then((rows) => alive && setPhasePlan(rows))
      .catch(() => alive && setPhasePlan([]));
    return () => {
      alive = false;
    };
  }, [duration]);

  useEffect(() => {
    let alive = true;
    // Параметров ещё нет — считать нечего, и нули в `estimateSession` вернули
    // бы «около 0 с», что хуже прочерка.
    if (duration <= 0 || reps <= 0) {
      setEstimate("—");
      setOneScheme("—");
      return;
    }
    const n = Math.max(1, selected.size);
    commands
      .estimateSession(duration, warmup, cooling, reps, n)
      .then((v) => {
        if (!alive) return;
        setEstimate(v.label);
        totalSecondsRef.current = v.seconds > 0 ? v.seconds : null;
      })
      .catch(() => alive && setEstimate("—"));
    // «Одна схема» для текущих параметров — панель настроек показывает его
    // рядом с полями, которые пользователь только что двигал.
    const one = commands.estimateSession(duration, warmup, cooling, reps, 1);
    // По одной схеме каждого пресета: карточки режима обещают «сколько займёт
    // тест», и раньше там стояла формула без стабилизации и фоновых проб.
    const perPreset = presets
      ? (Object.keys(presets) as ("quick" | "detailed")[]).map(async (key) => {
          const p = presets[key];
          const v = await commands.estimateSession(p.duration, p.warmup, p.cooling, p.reps, 1);
          return [key, v.label] as const;
        })
      : [];
    Promise.all([one, Promise.all(perPreset)])
      .then(([oneV, pairs]) => {
        if (!alive) return;
        setOneScheme(oneV.label);
        setPresetEstimates(Object.fromEntries(pairs));
      })
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, [duration, warmup, cooling, reps, selected.size, presets]);

  const loadQuarantine = useCallback(() => {
    commands.quarantineList().then(setQuarantine).catch(() => undefined);
  }, []);

  // Настройки нужны плиткам (избранное/исключения) и после их правок.
  const loadAll = useCallback(() => {
    commands.getSettings().then(setSettings).catch(() => undefined);
    loadQuarantine();
  }, [loadQuarantine]);

  // Пресеты режимов — из бэкенда, вместе с первым набором настроек: мастер
  // стартует с детального режима, и пока числа не пришли, поля пусты.
  const applyPresets = useCallback((rows: PresetDto[]) => {
    const map: Record<"quick" | "detailed", PresetParams> = {
      quick: paramsOf(rows.find((r) => r.key === "quick") ?? rows[0]),
      detailed: paramsOf(rows.find((r) => r.key === "detailed") ?? rows[1] ?? rows[0]),
    };
    setPresets(map);
    const start = map.detailed;
    setDuration(start.duration);
    setWarmup(start.warmup);
    setCooling(start.cooling);
    setReps(start.reps);
  }, []);

  const current: PresetParams = { duration, warmup, cooling, reps };
  const activePreset: PresetKey = !presets
    ? "custom"
    : sameParams(current, presets.quick)
      ? "quick"
      : sameParams(current, presets.detailed)
        ? "detailed"
        : "custom";

  // Проверка готовности не декоративная: она перечисляет ровно те условия,
  // при которых заведомо не получится замер (нет прав, нет сети, недоступен
  // powercfg, мало места). Показывать баннер и давать кнопку — значит
  // отправлять пользователя в заведомо падающую сессию.
  const readinessBlocksStart = !!readiness && !readiness.ok && !running;
  const startBlockedReason =
    selected.size === 0
      ? "Выберите хотя бы одну схему питания"
      : readiness && !readiness.ok
        ? "Окружение не готово к замеру — исправьте условия ниже"
        : undefined;

  // «Осталось» = полная оценка сессии минус уже прошедшее время.
  const [remainingEstimate, setRemainingEstimate] = useState("—");

  const applyPreset = (p: "quick" | "detailed") => {
    const def = presets?.[p];
    if (!def) return;
    setPreset(p);
    setDuration(def.duration);
    setWarmup(def.warmup);
    setCooling(def.cooling);
    setReps(def.reps);
  };

  /** Сохранить порог фоновой нагрузки. Значение остаётся и до следующего
   * запуска: настройки перечитываются целиком, чтобы не затереть правки,
   * сделанные параллельно (например, отметку «избранное» на плитке схем). */
  const setBackgroundThreshold = (v: number) => {
    setBackgroundThresholdState(v);
    commands
      .getSettings()
      .then((st) => commands.setSettings({ ...st, background_threshold_percent: v }))
      .catch((e) => pushToast("err", `Порог фона не сохранён: ${String(e)}`));
  };


  useEffect(() => {
    loadQuarantine();
  }, [loadQuarantine, stage, finished]);

  useEffect(() => {
    let alive = true;
    commands
      .checkpointStatus()
      .then((cp) => {
        if (!alive) return;
        setCheckpointError(null);
        setCheckpoint(cp);
      })
      .catch((e) => {
        // Раньше ошибка молча проглатывалась, и интерфейс просто не показывал
        // панель продолжения — выглядело так, будто незавершённой сессии нет.
        if (alive) setCheckpointError(String(e));
      });
    // Настройки и карантин нужны до выбора схем: без них исключённые и
    // карантинные схемы попадали бы в выбор молча, хотя код ниже обещает
    // обратное. Раньше обе выборки шли параллельно, и `listSchemes` успевал
    // раньше — с пустыми `settings` и `quarantine`.
    Promise.all([commands.getSettings(), commands.quarantineList(), commands.testPresets()])
      .then(([st, quarantineRows, presetRows]) => {
        if (!alive) return;
        setSettings(st);
        setQuarantine(quarantineRows);
        setBackgroundThresholdState(st.background_threshold_percent);
        if (presetRows.length >= 2) applyPresets(presetRows);
        return commands.listSchemes().then((s) => {
          if (!alive) return;
          setSchemes(sortSchemes(s));
          setSelected(
            new Set(
              filterEligible(sortSchemes(s), st.excluded_schemes, quarantineRows).map(
                (x) => x.guid,
              ),
            ),
          );
        });
      })
      .catch((e) => {
        if (alive) pushToast("err", `не удалось загрузить схемы: ${String(e)}`);
      });
    return () => {
      alive = false;
    };
  }, [applyPresets]);

  useEffect(() => {
    const untele = onTelemetry((m) => {
      setTelemetry(m);
    });
    const unfin = onTestFinished((m) => {
      finishedRef.current = true;
      setRunning(false);
      setStarting(false);
      setStopping(false);
      setTelemetry(null);
      setFinished(true);
      commands
        .checkpointStatus()
        .then((cp) => setCheckpoint(cp))
        .catch(() => undefined);
      // Карантин мог пополниться во время сессии — подтягиваем сразу.
      loadQuarantine();
      if (!m.ok) {
        setFinishMsg("сессия завершилась с ошибкой");
        pushToast("err", m.error ?? "сессия завершилась с ошибкой");
      } else {
        const msg = m.cancelled
          ? "Сессия остановлена."
          : `Готово: ${m.level_label}${m.recommended_name ? ` · схема «${m.recommended_name}»` : ""}`;
        setFinishMsg(msg);
        pushToast("okk", msg);
      }
    });
    return () => {
      untele.then((f) => f()).catch(() => undefined);
      unfin.then((f) => f()).catch(() => undefined);
    };
  }, []);

  useEffect(() => {
    commands.testRunning().then(setRunning).catch(() => undefined);
  }, []);

  useEffect(() => {
    if (stage !== "run") return;
    let alive = true;
    commands
      .systemReady(selected.size || 2)
      .then((r) => alive && setReadiness(r))
      .catch(() => alive && setReadiness(null));
    return () => {
      alive = false;
    };
  }, [stage, selected, duration, warmup, cooling, reps, running]);

  // Порядок выполнения совпадает с показанным: активная первая, далее
  // по алфавиту (тот же порядок применяет бэкенд для прогонов).
  const selSchemes = useMemo(
    () => sortSchemes(schemes.filter((s) => selected.has(s.guid))),
    [schemes, selected],
  );

  // На время замера — один таймер раз в секунду на весь прогресс.
  //
  // Раньше их было два, причём второй пересоздавался на каждом тике первого
  // (`elapsed` стоял в зависимостях), то есть интервал пересоздавался дважды
  // в секунду. Секундной границы достаточно: счётчик показывает `мм:сс`, а
  // «осталось» округляется пятками.
  useEffect(() => {
    if (!running) {
      setElapsed(0);
      setRemainingEstimate("—");
      return;
    }
    if (!t0.current) t0.current = Date.now();
    const tick = () => {
      const passed = (Date.now() - t0.current) / 1000;
      setElapsed(passed);
      const total = totalSecondsRef.current;
      // Оценка приходит асинхронно; до неё показываем «—», а не ноль.
      if (total == null) return;
      const left = Math.max(0, total - passed);
      // Округляем: «осталось примерно 4 мин 30 с» полезнее, чем «4 мин 33 с».
      const rounded = left < 60 ? Math.round(left / 5) * 5 : Math.round(left / 15) * 15;
      setRemainingEstimate(fmtDuration(rounded));
    };
    tick();
    const id = window.setInterval(tick, 1000);
    return () => window.clearInterval(id);
  }, [running]);

  const go = (s: Stage, d: "fwd" | "back") => {
    setDir(d);
    setStage(s);
  };

  /** Схемы к бенчмарку: исключённые и карантинные не выбираются по умолчанию. */
  const start = useCallback(
    (resume: boolean) => {
      const chosen = [...selected];
      if (!resume && chosen.length === 0) {
        pushToast("err", "выберите хотя бы одну схему питания");
        return;
      }
      if (!resume) {
        // Карантинные схемы бэкенд всё равно отсечёт на префлайте —
        // предупреждаем заранее, а не в середине запуска.
        const qset = new Set(quarantine.map((q) => q.scheme_id.toLowerCase()));
        const admitted = chosen.filter((g) => !qset.has(g.toLowerCase()));
        if (admitted.length === 0) {
          pushToast("err", "все выбранные схемы в карантине — верните их кнопкой «Вернуть»");
          return;
        }
        if (admitted.length < chosen.length) {
          pushToast("info", `карантинные схемы пропущены: ${chosen.length - admitted.length}`);
        }
      }
      setStarting(true);
      finishedRef.current = false;
      setFinished(false);
      setFinishMsg("");
      t0.current = 0;
      setElapsed(0);
      commands
        .startTest({
          // Отправляем выведенный `activePreset`, а не последний нажатый ключ:
          // после ручной правки полей пресет «не тот», и в истории сессии
          // лежал бы чужой режим. «custom» бэкенд трактует как «параметры
          // переданы явно» и использует их.
          preset: activePreset,
          duration_seconds: resume ? null : duration,
          warmup_seconds: resume ? null : warmup,
          cooling_seconds: resume ? null : cooling,
          repetitions: resume ? null : reps,
          background_threshold_percent: resume ? null : backgroundThreshold,
          worker_count: null,
          scheme_ids: resume ? [] : chosen,
          // Активная схема идёт эталоном: по её прогонам оценивается дрейф
          // машины за сессию, и вердикт понижается, если машина «плывёт»
      // сильнее, чем различаются схемы.
          active_scheme_id: schemes.find((s) => s.active)?.guid ?? null,
          resume,
        })
        .then(() => {
          pushToast("info", "запуск принят");
          setCheckpoint(null);
          // Если сессия уже завершилась (мгновенная ошибка), возвращать
          // `running = true` нельзя: экран замера залипнет, схемы останутся
          // заблокированы, и лечится это только перезапуском приложения.
          if (finishedRef.current) {
            setStarting(false);
            return;
          }
          setRunning(true);
        })
        .catch((e) => {
          setStarting(false);
          pushToast("err", String(e));
        });
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [selected, preset, duration, warmup, cooling, reps, settings, quarantine],
  );

  const stop = () => {
    setStopping(true);
    commands
      .stopTest()
      .then((was) => pushToast("info", was ? "отправлен запрос остановки" : "сессия не выполняется"))
      .catch((e) => pushToast("err", String(e)))
      .finally(() => setStopping(false));
  };

  const stepState = (id: Stage) => {
    if (id === "mode") return stage === "mode" ? "active" : "done";
    if (id === "schemes") return stage === "schemes" ? "active" : stage === "run" ? "done" : "pending";
    return stage === "run" ? "active" : "pending";
  };

  // Прогресс фазы: телеметрия шлёт событие до конца фазы, поэтому значение
  // может слегка превысить 100 — клампим, иначе кольцо и подписи «ломаются».
  // Нефинитное время тоже даёт NaN, а `Math.min/max` его не убирают: на экране
  // появлялось «NaN%».
  const phaseProgress = useMemo(() => {
    const el = telemetry?.phase_elapsed_ms;
    const total = telemetry?.phase_seconds;
    if (el == null || total == null || !Number.isFinite(el) || !Number.isFinite(total)) return 0;
    if (total <= 0) return 0;
    const p = (el / (total * 1000)) * 100;
    return Number.isFinite(p) ? Math.min(100, Math.max(0, p)) : 0;
  }, [telemetry]);

  // Идёт ли замер: телеметрия может ещё не прийти (подготовка сессии), но
  // дашборд уже должен быть на месте, и кнопка остановки — в шапке.
  const live = running || telemetry != null;

  // Общий прогресс сессии по всем выбранным схемам и раундам.
  //
  // Кольцо раньше показывало процент текущей фазы, то есть на длинной сессии
  // оно снова и снова возвращалось к нулю: выглядело, будто замер начинается
  // заново. `run_index` измеряет прогоны по всем схемам (`run_total` =
  // схемы × повторы), поэтому общий процент = выполненные прогоны плюс
  // текущая фаза.
  const sessionProgress = useMemo(() => {
    const total = telemetry?.run_total;
    const idx = telemetry?.run_index;
    if (telemetry == null || total == null || total <= 0 || idx == null) return 0;
    const frac = phaseProgress / 100;
    const done = Math.max(0, idx - 1 + frac);
    const p = (done / total) * 100;
    return Number.isFinite(p) ? Math.min(100, Math.max(0, p)) : 0;
  }, [telemetry, phaseProgress]);

  const next = () => go(stage === "mode" ? "schemes" : "run", "fwd");
  const back = () => go(stage === "run" ? "schemes" : "mode", "back");

  // Фоновая нагрузка перед стартом: пользователь видит, что машина занята,
  // ещё до того, как замер начался. Сэмпл занимает ~1.2 с, поэтому берём его
  // один раз на входе на предстартовый экран, а не в цикле.
  useEffect(() => {
    if (stage !== "run" || live) {
      setBgSample(null);
      return;
    }
    let alive = true;
    setBgSample(null);
    commands
      .backgroundSample()
      .then((v) => alive && setBgSample(Number.isFinite(v) ? v : null))
      .catch(() => alive && setBgSample(null));
    return () => {
      alive = false;
    };
  }, [stage, live]);

  /** Переключить схему в списке сравнения с проверкой ограничений. */
  const toggleScheme = (guid: string, on?: boolean) => {
    // Раньше активную схему нельзя было снять с плитки, а сообщение звало
    // «снимите галочку», которой на плитке нет. Снять её можно было только
    // кнопкой «Выбрать все», которая её наоборот добавляла. Активная схема
    // нужна и как опора для оценки дрейфа, и в списке сравнения, но участие
    // в сравнении — выбор пользователя.
    const isChecked = on ?? selected.has(guid);
    if (isChecked && selected.has(guid)) {
      setSelected((prev) => {
        const nextSet = new Set(prev);
        nextSet.delete(guid);
        return nextSet;
      });
      return;
    }
    // Исключённую пользователем схему нельзя вернуть в сравнение щелчком:
    // исключение — это явное «не мерить её».
    if (isChecked) {
      const row = schemes.find((s) => s.guid === guid);
      const isExcluded = (settings?.excluded_schemes ?? []).some(
        (g) => g.toLowerCase() === guid.toLowerCase(),
      );
      const isQuarantined = quarantine.some(
        (q) => q.scheme_id.toLowerCase() === guid.toLowerCase(),
      );
      if (isExcluded) {
        pushToast("err", "Схема помечена как исключённая — снимите метку «исключить».");
        return;
      }
      if (isQuarantined) {
        pushToast(
          "err",
          `Схема в карантине${row?.name ? ` (${row.name})` : ""} — верните её из карантина.`,
        );
        return;
      }
    }
    setSelected((prev) => {
      const nextSet = new Set(prev);
      if (nextSet.has(guid)) nextSet.delete(guid);
      else nextSet.add(guid);
      return nextSet;
    });
  };

  return (
    <div className="page benchmark-view">
      {/* Единая шапка: заголовок, степпер и кнопки навигации. */}
      <div className="wizard-header">
        <div className="wh-left">
          <h1 className="page-title">Бенчмарк</h1>
          <nav className="stepper" aria-label="Этапы запуска">
            {STAGES.map((s, idx) => {
              const st = stepState(s.id);
              return (
                <span className="steps-group" key={s.id}>
                  <button
                    type="button"
                    className={`step-item ${st}`}
                    aria-current={st === "active" ? "step" : undefined}
                    disabled={live}
                    title={live ? "Во время замера шаги менять нельзя" : `К шагу «${s.label}»`}
                    onClick={() => !live && go(s.id, s.id === stage ? "fwd" : "back")}
                  >
                    <span className="step-badge">{st === "done" ? "✓" : idx + 1}</span>
                    <span>{s.label}</span>
                  </button>
                  {idx < STAGES.length - 1 ? <span className="step-line" /> : null}
                </span>
              );
            })}
          </nav>
        </div>

        <div className="wh-actions">
          {stage === "mode" ? null : (
            <button type="button" className="btn-back" disabled={live} onClick={back}>
              ← Назад
            </button>
          )}
          {stage === "mode" || stage === "schemes" ? (
            <button
              type="button"
              className="btn-next"
              disabled={stage === "schemes" && selected.size === 0}
              onClick={next}
            >
              Далее →
            </button>
          ) : null}
          {/* На предстарте кнопку запуска в шапке не дублируем: она одна,
              в панели `.launch-cta-bar`, иначе два одинаковых вызова рядом. */}
          {live ? (
            <button type="button" className="btn-stop" disabled={stopping} onClick={stop}>
              {stopping ? "■ Остановка…" : "■ Остановить тест"}
            </button>
          ) : null}
        </div>
      </div>

      <div className="wizard">
        <div
          className={`wizard-stage ${stage === "schemes" ? "top" : "centered"}${
            dir === "back" ? " back" : ""
          }`}
        >
          {stage === "mode" ? (
            <>
              {/* Шаг 1: два режима рядом и выдвижные параметры под ними. */}
              <div className="mode-grid">
                {(Object.keys(PRESETS) as ("quick" | "detailed")[]).map((key) => {
                  const def = PRESETS[key];
                  const open = preset === key;
                  const p = presets?.[key];
                  return (
                    <Spot
                      key={key}
                      className={`mode-card${open ? " selected" : ""}`}
                      role="radio"
                      aria-checked={open}
                      tabIndex={0}
                      aria-label={`Режим «${def.title}»`}
                      onClick={() => applyPreset(key)}
                      onKeyDown={(e) => {
                        // Space тоже выбирает режим: на Enter-only карточка
                        // недоступна с клавиатуры привычным образом.
                        if (e.key === "Enter" || e.key === " ") {
                          e.preventDefault();
                          applyPreset(key);
                        }
                      }}
                    >
                      <div className="mc-top">
                        <div>
                          <div className="mc-title-row">
                            <h2 className="mc-title">{def.title}</h2>
                            <span className="mc-tag">{key === "quick" ? "Скрининг" : "Рекомендуется"}</span>
                          </div>
                          <p className="mc-desc">{def.desc}</p>
                        </div>
                        <span className="mc-radio">✓</span>
                      </div>
                      <div className="mc-stats">
                        <div className="mc-pill">
                          <small>Повторов</small>
                          <b>{p ? `${p.reps} ${plural(p.reps, "раунд", "раунда", "раундов")}` : "—"}</b>
                        </div>
                        <div className="mc-pill">
                          <small>Прогон / Разогрев</small>
                          <b>{p ? `${p.duration} с / ${p.warmup} с` : "—"}</b>
                        </div>
                        <div className="mc-pill time">
                          <small>Одна схема</small>
                          <b>{presetEstimates[key] ?? "—"}</b>
                        </div>
                      </div>
                    </Spot>
                  );
                })}
              </div>
              {activePreset === "custom" ? (
                <div className="hint center-hint">
                  Режим: {PRESET_SHORT.custom} — параметры изменены вручную
                </div>
              ) : null}

              <div className={`adv-wrap${expanded ? " open" : ""}`}>
                <button
                  type="button"
                  className="adv-toggle"
                  aria-expanded={expanded}
                  onClick={() => setExpanded((v) => !v)}
                >
                  <span className="adv-toggle-left">
                    <GearIcon className="nav-i" />
                    <span>Параметры теста</span>
                  </span>
                  <span className="adv-toggle-right">
                    <span className="adv-summary">
                      Прогон {duration} с · Разогрев {warmup} с · Охлаждение {cooling} с ·
                      Повторов {reps} · Фон ≤ {tf(backgroundThreshold, 1)}%
                    </span>
                    <span className="adv-chevron">
                      {expanded ? "Свернуть" : "Настроить"} <i>▾</i>
                    </span>
                  </span>
                </button>

                <div className="adv-body">
                  {/* Обёртка с `overflow: hidden`: без неё `grid-template-rows`
                      не даёт плавного раскрытия — содержимое вылезает наружу. */}
                  <div className="adv-clip">
                      <div className="param-grid">
                      <div className="p-field">
                        <label htmlFor="pDur">Длительность прогона</label>
                        <div className="p-input-wrap">
                          <input
                            id="pDur"
                            type="number"
                            value={duration}
                            min={16}
                            max={3600}
                            onChange={(e) => setDuration(Number(e.target.value) || 0)}
                          />
                          <span className="p-unit">с</span>
                        </div>
                      </div>
                      <div className="p-field">
                        <label htmlFor="pWarm">Разогрев</label>
                        <div className="p-input-wrap">
                          <input
                            id="pWarm"
                            type="number"
                            value={warmup}
                            min={2}
                            max={300}
                            onChange={(e) => setWarmup(Number(e.target.value) || 0)}
                          />
                          <span className="p-unit">с</span>
                        </div>
                      </div>
                      <div className="p-field">
                        <label htmlFor="pCool">Охлаждение</label>
                        <div className="p-input-wrap">
                          <input
                            id="pCool"
                            type="number"
                            value={cooling}
                            min={0}
                            max={120}
                            onChange={(e) => setCooling(Number(e.target.value) || 0)}
                          />
                          <span className="p-unit">с</span>
                        </div>
                      </div>
                      <div className="p-field">
                        <label htmlFor="pReps">Повторов (раундов)</label>
                        <div className="p-input-wrap">
                          <input
                            id="pReps"
                            type="number"
                            value={reps}
                            min={1}
                            max={9}
                            onChange={(e) => setReps(Number(e.target.value) || 1)}
                          />
                          <span className="p-unit">раз</span>
                        </div>
                      </div>
                      {/* Порог фона — параметр замера, а не оформления, поэтому
                          он живёт здесь, рядом с остальными числами теста. */}
                      <div className="p-field">
                        <label htmlFor="pBg">Порог фоновой нагрузки</label>
                        <div className="p-input-wrap">
                          <input
                            id="pBg"
                            type="number"
                            value={backgroundThreshold}
                            min={0.5}
                            max={100}
                            step={0.5}
                            onChange={(e) => setBackgroundThreshold(Number(e.target.value) || 0.5)}
                          />
                          <span className="p-unit">%</span>
                        </div>
                      </div>
                  </div>
                  <div className="adv-footer">
                    <span>{earlyStopHint(reps)}</span>
                    <button
                      type="button"
                      className="btn-reset-params"
                      disabled={!presets || running}
                      onClick={() => {
                        applyPreset(activePreset === "quick" ? "quick" : "detailed");
                      }}
                    >
                      Сбросить по умолчанию
                    </button>
                    </div>
                  </div>
                </div>
              </div>
            </>
          ) : null}

          {stage === "schemes" ? (
            <>
              <SchemePicker
                schemes={schemes}
                selected={selected}
                settings={settings}
                quarantine={quarantine}
                onChanged={loadAll}
                estimate={estimate}
                oneScheme={oneScheme}
                detailed={activePreset !== "quick"}
                onToggle={toggleScheme}
                onToggleMany={(guids) => setSelected(new Set(guids))}
              />
              {schemes.length === 0 ? (
                <div className="glass inset">
                  <div className="muted">
                    Схемы питания не найдены. Проверьте, что вы запустили
                    PowerBench от имени администратора.
                  </div>
                </div>
              ) : null}
              {checkpointError ? (
                <Glass className="inset">
                  <div className="card-title">Незавершённая сессия не читается</div>
                  <div className="hint" style={{ marginBottom: 10 }}>
                    {checkpointError} Продолжить её нельзя: отработанные раунды
                    восстановить не из чего. Файл{" "}
                    <span className="mono break">benchmark-checkpoint.json</span>{" "}
                    лежит в каталоге данных — сохраните его себе, если он
                    нужен, и запустите новый замер.
                  </div>
                </Glass>
              ) : null}
              {checkpoint && !checkpoint.original_restored ? (
                <Glass className="inset">
                  <div className="card-title">Незавершённая сессия</div>
                  <div className="hint" style={{ marginBottom: 10 }}>
                    <span className="mono break">{checkpoint.plan_guid}</span> · схем{" "}
                    {checkpoint.scheme_ids.length} · повторов {checkpoint.repetitions} ·{" "}
                    {(() => {
                      // `completed_keys` содержит по записи на СХЕМУ на раунд,
                      // а не по раунду: при 3 схемах и 2 выполненных раундах
                      // длина 6, и интерфейс писал «6 раундов выполнено» рядом с
                      // «повторов 5» — работа выглядела почти законченной.
                      const perRound = Math.max(1, checkpoint.scheme_ids.length);
                      const done = Math.floor(checkpoint.completed_keys.length / perRound);
                      return (
                        <>
                          {done}{" "}
                          {plural(
                            done,
                            "раунд выполнен",
                            "раунда выполнено",
                            "раундов выполнено",
                          )}
                        </>
                      );
                    })()}
                  </div>
                  <div className="row wrap gap-2">
                    <Button variant="primary" disabled={running} onClick={() => start(true)}>
                      Продолжить из контрольной точки
                    </Button>
                    <Button
                      variant="ghost"
                      disabled={running}
                      title="Вернуть исходную схему питания и удалить контрольную точку"
                      onClick={() => {
                        commands
                          .checkpointDiscard()
                          .then(() => {
                            pushToast("okk", "Прерванная сессия забыта");
                            setCheckpoint(null);
                          })
                          .catch((e) => pushToast("err", String(e)));
                      }}
                    >
                      Забыть
                    </Button>
                  </div>
                </Glass>
              ) : null}
            </>
          ) : null}

          {stage === "run" ? (
            live ? (
              <>
                <div className="live-hero">
                  <div className="lh-hero-left">
                    <div className="lh-label">Текущая скорость</div>
                    <div className="lh-speed">
                      {telemetry ? tf(telemetry.ticks_per_sec, 1) : "—"}
                      <small>тик/с</small>
                    </div>
                    <div className="lh-meta">
                      {telemetry ? (
                        <>
                          <span>
                            Прогон <b>{telemetry.run_index}</b> / {telemetry.run_total}
                          </span>
                          <span className="muted">·</span>
                          <span>
                            Раунд <b>{telemetry.round + 1}</b> из {reps}
                          </span>
                          <span className="muted">·</span>
                          <span>
                            Схема: <b>{telemetry.scheme_name || "—"}</b>
                          </span>
                        </>
                      ) : (
                        <span>идёт подготовка к сессии</span>
                      )}
                    </div>
                  </div>

                  <div className="ring-wrap">
                    <div className="ring-eta">
                      <span>До конца сессии</span>
                      <b>{running ? remainingEstimate : "—"}</b>
                      <span>
                        {telemetry
                          ? `${telemetry.run_index} из ${telemetry.run_total} прогонов`
                          : "оценка готовится"}
                      </span>
                    </div>
                    <div
                      className="ring-box"
                      title="Общий прогресс бенчмарка по всем выбранным схемам и раундам"
                    >
                      <svg viewBox="0 0 76 76" width="82" height="82" aria-hidden="true">
                        <circle className="ring-bg" cx="38" cy="38" r="32" />
                        <circle
                          className="ring-fg"
                          cx="38"
                          cy="38"
                          r="32"
                          strokeDasharray={2 * Math.PI * 32}
                          strokeDashoffset={2 * Math.PI * 32 * (1 - sessionProgress / 100)}
                        />
                      </svg>
                      <div className="ring-text">
                        <b>{sessionProgress < 0.05 ? "<1%" : `${sessionProgress.toFixed(0)}%`}</b>
                        <small>сессия</small>
                      </div>
                    </div>
                  </div>
                </div>

                <div className="kpi-strip">
                  <div className="kpi-card">
                    <span className="kpi-name">Время тика</span>
                    <span className="kpi-val">
                      {telemetry ? tf(telemetry.ms_per_tick, 3) : "—"}
                      <small>мс</small>
                    </span>
                  </div>
                  <div className="kpi-card">
                    <span className="kpi-name">Накоплено тиков</span>
                    <span className="kpi-val">{telemetry ? telemetry.ticks_done : "—"}</span>
                  </div>
                  <div
                    className={`kpi-card${
                      telemetry?.background_noisy ? " warn-border" : ""
                    }`}
                  >
                    <span className="kpi-name">Фон CPU</span>
                    <span
                      className={`kpi-val${
                        telemetry?.background_noisy ? " warn-txt" : ""
                      }`}
                    >
                      {tf(telemetry?.background_percent, 1)}
                      <small>{telemetry?.background_noisy ? "загружен" : "%"}</small>
                    </span>
                  </div>
                </div>

                <div className="phases-card">
                  <div className="ph-head">
                    <h2 className="ph-title">Фазы замера (по {duration} с на прогон)</h2>
                    <span className="ph-eta">
                      Прошло <b>{fmtDuration(elapsed)}</b> · осталось примерно{" "}
                      <b>{running ? remainingEstimate : "—"}</b>
                    </span>
                  </div>
                  <div className="ph-grid">
                    {phasePlan.map((p, i) => {
                      const active = telemetry?.phase === p.name;
                      const done =
                        telemetry != null &&
                        phaseIndex(phasePlan, telemetry.phase) > i;
                      const pct = active ? phaseProgress : done ? 100 : 0;
                      return (
                        <div
                          key={p.name}
                          className={`ph-item${active ? " active" : ""}${done ? " done" : ""}`}
                        >
                          <div className="phi-top">
                            <span>
                              {i + 1}. {p.name}
                            </span>
                            <span className="phi-pct">{pct.toFixed(0)}%</span>
                          </div>
                          <div className="phi-bar">
                            <i style={{ width: `${pct}%` }} />
                          </div>
                        </div>
                      );
                    })}
                  </div>
                </div>

                {telemetry?.background_noisy && telemetry.background_percent != null ? (
                  <div className="warn-banner">
                    <span>
                      <b>Фон загружен — {tf(telemetry.background_percent, 1)}% CPU.</b> Закройте
                      ресурсоёмкие программы и повторите замер, иначе результат может быть
                      занижен.
                    </span>
                  </div>
                ) : null}
              </>
            ) : (
              <>
                <div className="launch-grid">
                  <div className="summary-card">
                    <div className="sc-header">
                      <div className="sc-title-wrap">
                        <h2 className="sc-title">Очередь тестирования</h2>
                        <span className="sc-count">
                          {selected.size} {plural(selected.size, "схема", "схемы", "схем")}
                        </span>
                      </div>
                      <button
                        type="button"
                        className="sc-edit-link"
                        onClick={() => go("schemes", "back")}
                      >
                        Изменить список
                      </button>
                    </div>
                    <div className="part-list">
                      {selSchemes.map((s, i) => (
                        <div className="part-item" key={s.guid}>
                          <span className="pi-name">
                            <span className="pi-num">#{i + 1}</span>
                            <span className="pi-title">{s.name || "Без названия"}</span>
                            {s.active ? <span className="badge-active">Активна</span> : null}
                          </span>
                          <button
                            type="button"
                            className="pi-rm"
                            onClick={() => toggleScheme(s.guid, true)}
                          >
                            Убрать
                          </button>
                        </div>
                      ))}
                    </div>
                  </div>

                  <div className="summary-card">
                    <div className="sc-header">
                      <div className="sc-title-wrap">
                        <h2 className="sc-title">Параметры и готовность</h2>
                        <span className="sc-count">{phasePlan.length} фазы нагрузки</span>
                      </div>
                      <button
                        type="button"
                        className="sc-edit-link"
                        onClick={() => go("mode", "back")}
                      >
                        Настроить
                      </button>
                    </div>
                    <div className="kv-table">
                      <div className="kv-row">
                        <span className="kv-k">Режим</span>
                        <span className="kv-v kv-pill">
                          {activePreset === "quick" ? "Скрининг" : PRESET_SHORT[activePreset]}
                          {activePreset !== "quick" ? ` (${reps} раундов)` : ""}
                        </span>
                      </div>
                      <div className="kv-row">
                        <span className="kv-k">Тайминги прогона</span>
                        <span className="kv-v">
                          {duration} с замер · {warmup} с разогрев · {cooling} с охл.
                        </span>
                      </div>
                      <div className="kv-row">
                        <span className="kv-k">Всего прогонов</span>
                        <span className="kv-v">
                          {selected.size * reps} ({selected.size} схем × {reps})
                        </span>
                      </div>
                      <div className="kv-row">
                        <span className="kv-k">Фазы нагрузки</span>
                        <span className="kv-v kv-phases">
                          {phasePlan.length ? phasePlan.map((p) => p.name).join(" · ") : "—"}
                        </span>
                      </div>
                      <div className="kv-row">
                        <span className="kv-k">Фоновая нагрузка CPU</span>
                        {bgSample == null ? (
                          <span className="kv-pill">измеряется…</span>
                        ) : bgSample > backgroundThreshold ? (
                          <span className="kv-status-bad">
                            ● {tf(bgSample, 1)}% (выше порога {tf(backgroundThreshold, 1)}%)
                          </span>
                        ) : (
                          <span className="kv-status-ok">
                            ● {tf(bgSample, 1)}% (в норме ≤ {tf(backgroundThreshold, 1)}%)
                          </span>
                        )}
                      </div>
                    </div>
                  </div>
                </div>

                {readiness && !readiness.ok ? (
                  <div className="warn-banner ready-warn">
                    <span>
                      <b>Окружение не готово к замеру.</b> {readiness.issues.join(" ")}
                    </span>
                  </div>
                ) : null}
                {finished ? (
                  <div className="warn-banner ok-banner">
                    <span>
                      <b>Тест завершён.</b> {finishMsg || "сессия завершена — можно начать новую"}
                    </span>
                  </div>
                ) : null}

                <div className="launch-cta-bar">
                  <div>
                    <div className="lcb-title">
                      Расчётное время сессии: <b>около {estimate}</b>
                    </div>
                    <div className="lcb-sub">
                      {selSchemes.some((s) => s.active)
                        ? `Активная схема «${selSchemes.find((s) => s.active)?.name ?? ""}» протестируется первой и автоматически восстановится после завершения.`
                        : "Исходная схема питания восстановится автоматически после завершения."}
                    </div>
                  </div>
                  <button
                    type="button"
                    className="btn-launch-main"
                    disabled={starting || selected.size === 0 || readinessBlocksStart}
                    title={startBlockedReason}
                    onClick={() => start(false)}
                  >
                    {starting ? "Запуск…" : "▶ Запустить тест"}
                  </button>
                </div>
              </>
            )
          ) : null}
        </div>
      </div>
    </div>
  );
}
