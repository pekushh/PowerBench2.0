// Страница «Бенчмарк»: визард Режим → Схемы → Запуск.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  commands,
  onTelemetry,
  onTestFinished,
  type CheckpointDto,
  type QuarantineEntry,
  type Readiness,
  type SchemeRow,
  type SettingsDto,
  type TelemetryMsg,
} from "../api";
import { Badge, Button, FadeScroll, Field, Glass, NumInput, Panel, Progress, Spot, Stat } from "../components/ui";
import { GearIcon } from "../components/icons";
import { pushToast, setRunning, useSession } from "../store";
import { SchemePicker, filterEligible, sortSchemes } from "../components/SchemeTiles";

type Stage = "mode" | "schemes" | "run";
type PresetKey = "quick" | "detailed" | "custom";

interface PresetDef {
  title: string;
  desc: string;
  expand: string;
  reps: string;
  accuracy: string;
  preset: { duration: number; warmup: number; cooling: number; reps: number };
}

const PRESETS: Record<"quick" | "detailed", PresetDef> = {
  quick: {
    title: "Быстро",
    desc: "Быстрая проверка производительности системы с минимальным временем тестирования.",
    expand:
      "Подходит для быстрого сравнения схем питания. Минимальное время выполнения, меньше повторов и упрощённая методика — ответ за несколько минут, без долгих ожиданий.",
    reps: "1",
    accuracy: "Стандартная",
    preset: { duration: 9, warmup: 2, cooling: 1, reps: 1 },
  },
  detailed: {
    title: "Детально",
    desc: "Расширенное тестирование для точного сравнения схем питания и стабильных результатов.",
    expand:
      "Расширенный сценарий с увеличенным количеством повторов и более продолжительными фазами измерения — точнее и надёжнее. Подходит, когда схема выбирается на длительный срок.",
    reps: "3",
    accuracy: "Повышенная",
    preset: { duration: 30, warmup: 6, cooling: 5, reps: 3 },
  },
};

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
 * Длительности фаз — та же формула, что в `orchestrator::config::phase_durations`:
 * `side = 3 + (total - 9) * 3 / 10`, лёгкая и отклик по `side`,
 * тяжёлая — остаток. Нужна для подписей в интерфейсе.
 */
function phaseSeconds(total: number): { name: string; seconds: number }[] {
  const t = Math.max(9, total);
  const side = 3 + Math.floor((t - 9) * 3 / 10);
  return [
    { name: "Лёгкая", seconds: side },
    { name: "Тяжёлая", seconds: Math.max(1, t - 2 * side) },
    { name: "Отклик", seconds: side },
  ];
}

/** Порядковый номер фазы по имени (для «готово»/«текущая»). */
function phaseIndex(name: string | undefined): number {
  if (!name) return -1;
  return ["Лёгкая", "Тяжёлая", "Отклик"].indexOf(name);
}

/** Число из любых данных: нефинитное заменяется запасным. */
function num(v: unknown, fallback: number): number {
  return typeof v === "number" && Number.isFinite(v) ? v : fallback;
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
  const [duration, setDuration] = useState(30);
  const [warmup, setWarmup] = useState(6);
  const [cooling, setCooling] = useState(5);
  const [reps, setReps] = useState(3);
  const [rawSamples, setRawSamples] = useState(false);
  const [checkpoint, setCheckpoint] = useState<CheckpointDto | null>(null);
  const [telemetry, setTelemetry] = useState<TelemetryMsg | null>(null);
  const [starting, setStarting] = useState(false);
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
  const [oneScheme, setOneScheme] = useState("—");
  useEffect(() => {
    let alive = true;
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
    const perPreset = (Object.keys(PRESETS) as ("quick" | "detailed")[]).map(async (
      key,
    ) => {
      const p = PRESETS[key].preset;
      const v = await commands.estimateSession(p.duration, p.warmup, p.cooling, p.reps, 1);
      return [key, v.label] as const;
    });
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
  }, [duration, warmup, cooling, reps, selected.size]);

  const loadQuarantine = useCallback(() => {
    commands.quarantineList().then(setQuarantine).catch(() => undefined);
  }, []);

  // Настройки нужны плиткам (избранное/исключения) и после их правок.
  const loadAll = useCallback(() => {
    commands
      .getSettings()
      .then((st) => {
        setSettings(st);
        setDuration(num(st.duration_seconds, 30));
        setWarmup(num(st.warmup_seconds, 6));
        setCooling(num(st.cooling_seconds, 5));
        setReps(num(st.repetitions, 3));
      })
      .catch(() => undefined);
    loadQuarantine();
  }, [loadQuarantine]);

  const activePreset: PresetKey =
    duration === PRESETS.quick.preset.duration &&
    warmup === PRESETS.quick.preset.warmup &&
    cooling === PRESETS.quick.preset.cooling &&
    reps === PRESETS.quick.preset.reps
      ? "quick"
      : duration === PRESETS.detailed.preset.duration &&
          warmup === PRESETS.detailed.preset.warmup &&
          cooling === PRESETS.detailed.preset.cooling &&
          reps === PRESETS.detailed.preset.reps
        ? "detailed"
        : "custom";

  // «Осталось» = полная оценка сессии минус уже прошедшее время.
  const [remainingEstimate, setRemainingEstimate] = useState("—");

  const applyPreset = (p: "quick" | "detailed") => {
    const def = PRESETS[p].preset;
    setPreset(p);
    setDuration(def.duration);
    setWarmup(def.warmup);
    setCooling(def.cooling);
    setReps(def.reps);
  };


  useEffect(() => {
    loadQuarantine();
  }, [loadQuarantine, stage, finished]);

  useEffect(() => {
    let alive = true;
    commands
      .checkpointStatus()
      .then((cp) => alive && setCheckpoint(cp))
      .catch(() => undefined);
    // Настройки и карантин нужны до выбора схем: без них исключённые и
    // карантинные схемы попадали бы в выбор молча, хотя код ниже обещает
    // обратное. Раньше обе выборки шли параллельно, и `listSchemes` успевал
    // раньше — с пустыми `settings` и `quarantine`.
    Promise.all([commands.getSettings(), commands.quarantineList()])
      .then(([st, quarantineRows]) => {
        if (!alive) return;
        setSettings(st);
        setQuarantine(quarantineRows);
        setDuration(num(st.duration_seconds, 30));
        setWarmup(num(st.warmup_seconds, 6));
        setCooling(num(st.cooling_seconds, 5));
        setReps(num(st.repetitions, 3));
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
  }, []);

  useEffect(() => {
    const untele = onTelemetry((m) => {
      setTelemetry(m);
    });
    const unfin = onTestFinished((m) => {
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
      untele.then((f) => f());
      unfin.then((f) => f());
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
  const eligible = useCallback(
    (list: SchemeRow[]): string[] =>
      filterEligible(list, settings?.excluded_schemes ?? [], quarantine).map((x) => x.guid),
    [settings, quarantine],
  );

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
          background_threshold_percent: resume ? null : (settings?.background_threshold_percent ?? 5),
          worker_count: null,
          scheme_ids: resume ? [] : chosen,
          export_raw_samples: resume ? false : rawSamples,
          resume,
        })
        .then(() => {
          pushToast("info", "Сессия запущена");
          setRunning(true);
          setCheckpoint(null);
        })
        .catch((e) => {
          setStarting(false);
          pushToast("err", String(e));
        });
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [selected, preset, duration, warmup, cooling, reps, settings, rawSamples, quarantine],
  );

  const stop = () => {
    setStopping(true);
    commands
      .stopTest()
      .then((was) => pushToast("info", was ? "отправлен запрос остановки" : "сессия не выполняется"))
      .catch((e) => pushToast("err", String(e)))
      .finally(() => setStopping(false));
  };

  const subtitle =
    stage === "mode"
      ? "выберите режим «Быстро» или «Детально» — точные параметры откроются рядом"
      : stage === "schemes"
        ? "отметьте схемы питания, которые войдут в сравнение"
        : running
          ? "идёт измерение — статистика обновляется в реальном времени"
          : finished
            ? "сессия завершена — можно начать новую"
            : "проверьте параметры и запустите тест";

  const stepState = (id: Stage) => {
    if (id === "mode") return stage === "mode" ? "active" : "done";
    if (id === "schemes") return stage === "schemes" ? "active" : stage === "run" ? "done" : "pending";
    return stage === "run" ? "active" : "pending";
  };

  const nav =
    stage === "mode" ? (
      <Button variant="primary" onClick={() => go("schemes", "fwd")}>
        Далее →
      </Button>
    ) : stage === "schemes" ? (
      <>
        <Button variant="ghost" onClick={() => go("mode", "back")}>
          ← Назад
        </Button>
        <Button variant="primary" disabled={selected.size === 0} onClick={() => go("run", "fwd")}>
          Далее →
        </Button>
      </>
    ) : (
      <>
        <Button variant="ghost" onClick={() => go("schemes", "back")}>
          ← Назад
        </Button>
        <Button
          variant="primary"
          disabled={starting || running || selected.size === 0}
          title={selected.size === 0 ? "Выберите хотя бы одну схему питания" : undefined}
          onClick={() => start(false)}
        >
          {starting ? "Запуск…" : "Запустить тест"}
        </Button>
      </>
    );

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

  return (
    <div className="page">
      <div className="page-head">
        <h1>Бенчмарк</h1>
        <span className="sub">{subtitle}</span>
        <div className="actions">
          {running ? (
            <Button variant="danger" disabled={stopping} onClick={stop}>
              {stopping ? "Остановка…" : "Остановить"}
            </Button>
          ) : null}
        </div>
      </div>

      <div className="steps">
        {STAGES.map((s, idx) => {
          const st = stepState(s.id);
          return (
            <div className="steps-group" key={s.id}>
              <div className={`step ${st}`}>
                <span className="dot">{st === "done" ? "✓" : idx + 1}</span>
                <span className="label">{s.label}</span>
              </div>
              {idx < STAGES.length - 1 ? <span className="step-link" /> : null}
            </div>
          );
        })}
        <div className="steps-spacer" />
        <div className="steps-nav">{nav}</div>
      </div>

      <div className="wizard">
        <div className={`wizard-stage${dir === "back" ? " back" : ""}`}>
          {stage === "mode" ? (
            <>
              <div className="mode-area">
                <div className={`mode-cards${expanded ? " expanded" : ""}`}>
                  {(Object.keys(PRESETS) as ("quick" | "detailed")[]).map((key, cardIdx) => {
                    const def = PRESETS[key];
                    const open = preset === key;
                    return (
                      <Spot
                        key={key}
                        className={`mode-card ${open ? "open" : ""}`}
                        role="button"
                        tabIndex={0}
                        aria-pressed={open}
                        aria-label={`Режим «${def.title}»`}
                        onClick={() => {
                          if (!expanded) applyPreset(key);
                        }}
                        onKeyDown={(e) => {
                          // Space тоже выбирает режим: на Enter-only карточка
                          // недоступна с клавиатуры привычным образом.
                          if ((e.key === "Enter" || e.key === " ") && !expanded) {
                            e.preventDefault();
                            applyPreset(key);
                          }
                        }}
                      >
                        <div className="face" style={{ textAlign: "center" }}>
                          <h3>{def.title}</h3>
                          <p className="desc">{def.desc}</p>
                          <p className="desc sub">{def.expand}</p>
                          <div className="spacer" />
                          <div className="mode-stats">
                            <div className="mode-stat">
                              <div className="ms-label">Повторов</div>
                              <div className="ms-value">{def.reps}</div>
                            </div>
                            <div className="mode-stat">
                              <div className="ms-label">Точность</div>
                              <div className="ms-value">{def.accuracy}</div>
                            </div>
                          </div>
                          <div className="mode-time">
                            Одна схема целиком: <b>{presetEstimates[key] ?? "—"}</b>
                          </div>
                        </div>
                        {cardIdx === 0 ? (
                          <div className="settings-pane">
                            <h3 className="pane-title">Настройки теста</h3>
                            <span className="est">
                              Одна схема: <b>{oneScheme}</b>
                            </span>
                            <p className="desc">{PRESETS[preset].desc}</p>
                            <div className="spacer" />
                            <label className="check">
                              <input
                                type="checkbox"
                                checked={rawSamples}
                                onChange={(e) => setRawSamples(e.target.checked)}
                              />
                              <span>Подробные замеры (JSON)</span>
                            </label>
                          </div>
                        ) : (
                          <div className="settings-pane">
                            <h3 className="pane-title">Параметры</h3>
                            <div className="grid2">
                              <Field label="Длительность теста, с">
                                <NumInput
                                  value={duration}
                                  min={9}
                                  unit="с"
                                  onChange={(v) => setDuration(v)}
                                />
                              </Field>
                              <Field label="Разогрев, с">
                                <NumInput
                                  value={warmup}
                                  min={2}
                                  unit="с"
                                  onChange={(v) => setWarmup(v)}
                                />
                              </Field>
                              <Field label="Охлаждение, с">
                                <NumInput
                                  value={cooling}
                                  min={0}
                                  max={60}
                                  unit="с"
                                  onChange={(v) => setCooling(v)}
                                />
                              </Field>
                              <Field label="Повторов, раз">
                                <NumInput
                                  value={reps}
                                  min={1}
                                  max={9}
                                  onChange={(v) => setReps(v)}
                                />
                              </Field>
                            </div>
                            <div className="hint mt-3">{earlyStopHint(reps)}</div>
                          </div>
                        )}
                      </Spot>
                    );
                  })}
                </div>
              </div>
              <Button
                variant="ghost"
                className="toggle-settings"
                onClick={() => setExpanded((v) => !v)}
              >
                <GearIcon className="nav-i" />
                {expanded ? "Скрыть настройки" : "Настройки теста"}
              </Button>
              {activePreset === "custom" ? (
                <div className="hint" style={{ textAlign: "center" }}>
                  Режим: {PRESET_SHORT.custom} (параметры изменены вручную)
                </div>
              ) : null}
            </>
          ) : null}

          {stage === "schemes" ? (
            <>
              <div className="wizard-toolbar">
                <span className="ttl">Схемы питания</span>
                <span className="cnt">
                  выбрано {selected.size} из {schemes.length}
                </span>
                <div className="spacer" />
                <Button sm variant="ghost" onClick={() => setSelected(new Set(eligible(schemes)))}>
                  Выбрать все
                </Button>
                <Button
                  sm
                  variant="ghost"
                  title="Оставить только активную схему"
                  onClick={() =>
                    setSelected((prev) => {
                      const active = schemes.find((s) => s.active);
                      const next = new Set<string>();
                      if (active) next.add(active.guid);
                      else if (prev.size > 0) next.add([...prev][0]);
                      return next;
                    })
                  }
                >
                  Только активная
                </Button>
              </div>
              <SchemePicker
                schemes={schemes}
                selected={selected}
                settings={settings}
                quarantine={quarantine}
                onChanged={loadAll}
                onToggle={(guid, active) => {
                  if (active && selected.has(guid)) {
                    pushToast("err", "Эта схема уже выбрана для сравнения — снимите галочку.");
                    return;
                  }
                  // Исключённую пользователем схему нельзя вернуть в сравнение
                  // щелчком по плитке: исключение — это явное «не мерить её».
                  if (active) {
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
                    const next = new Set(prev);
                    if (next.has(guid)) next.delete(guid);
                    else next.add(guid);
                    return next;
                  });
                }}
              />
              {schemes.length === 0 ? (
                <div className="glass inset">
                  <div className="muted">
                    Схемы питания не найдены. Проверьте, что вы запустили
                    PowerBench от имени администратора.
                  </div>
                </div>
              ) : null}
              {checkpoint && !checkpoint.original_restored ? (
                <Glass className="inset">
                  <div className="card-title">Незавершённая сессия</div>
                  <div className="hint" style={{ marginBottom: 10 }}>
                    <span className="mono break">{checkpoint.plan_guid}</span> · схем{" "}
                    {checkpoint.scheme_ids.length} · повторов {checkpoint.repetitions} ·{" "}
                    {checkpoint.completed_keys.length}{" "}
                    {plural(checkpoint.completed_keys.length, "раунд выполнен", "раунда выполнено", "раундов выполнено")}
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
            running || telemetry ? (
              <>
                <div className="run-hero">
                  <div className="rh-stat grow">
                    <div className="rh-label">Текущая скорость</div>
                    <div className="rh-value">
                      {telemetry ? tf(telemetry.ticks_per_sec, 1) : "—"}
                      <span className="rh-suffix">тик/с</span>
                    </div>
                    <div className="rh-sub">
                      {telemetry
                        ? `прогон ${telemetry.run_index}/${telemetry.run_total} · раунд ${telemetry.round + 1}`
                        : "подготовка к сессии…"}
                    </div>
                  </div>
                  <div className="run-ring">
                    <svg viewBox="0 0 88 88" width="88" height="88">
                      <circle className="ring-bg" cx="44" cy="44" r="38" />
                      <circle
                        className="ring-fg"
                        cx="44"
                        cy="44"
                        r="38"
                        strokeDasharray={2 * Math.PI * 38}
                        strokeDashoffset={2 * Math.PI * 38 * (1 - phaseProgress / 100)}
                      />
                    </svg>
                    <div className="ring-center">
                      <div className="ring-pct">
                        {telemetry ? `${phaseProgress.toFixed(0)}%` : "—"}
                      </div>
                      <div className="ring-label">{telemetry?.phase ?? "фаза"}</div>
                    </div>
                  </div>
                </div>
                <div className="run-grid">
                  <Panel title="Фазы замера" hint={`по ${duration} с на прогон · фазы повторяются в каждом раунде`}>
                    <div className="run-phases">
                      {phaseSeconds(duration).map((p) => {
                        const active = telemetry?.phase === p.name;
                        const done =
                          telemetry != null && phaseIndex(telemetry.phase) > phaseIndex(p.name);
                        const pct = active ? phaseProgress : done ? 100 : 0;
                        return (
                          <div
                            key={p.name}
                            className={`run-phase${active ? " active" : ""}${done ? " done" : ""}`}
                          >
                            <div className="rp-head">
                              <span className="rp-name">{p.name}</span>
                              <span className="rp-time">
                                {active
                                  ? `${phaseProgress.toFixed(0)}%`
                                  : done
                                    ? "готово"
                                    : `${p.seconds} с`}
                              </span>
                            </div>
                            <Progress value={pct} />
                          </div>
                        );
                      })}
                    </div>
                    {running || telemetry ? (
                      <div className="hint" style={{ marginTop: 10 }}>
                        Прошло {fmtDuration(elapsed)} · осталось примерно{" "}
                        {remainingEstimate}
                      </div>
                    ) : null}
                  </Panel>
                  <div className="grid2">
                    <Stat
                      label="Время тика"
                      value={telemetry ? tf(telemetry.ms_per_tick, 3) : "—"}
                      suffix="мс"
                    />
                    <Stat label="Тиков" value={telemetry ? `${telemetry.ticks_done}` : "—"} />
                    <Stat
                      label="Фон"
                      value={
                        telemetry?.background_percent != null
                          ? tf(telemetry.background_percent, 1)
                          : "—"
                      }
                      suffix={telemetry?.background_noisy ? "загружен" : "%"}
                    />
                  </div>
                </div>
                {telemetry?.background_noisy && telemetry.background_percent != null ? (
                  <div className="run-warn">
                    <b>Фон загружен — {tf(telemetry.background_percent, 1)}% CPU.</b>{" "}
                    Измерение идёт с посторонней нагрузкой: закройте тяжёлые программы
                    и повторите тест, иначе результат может быть занижен.
                  </div>
                ) : null}
              </>
            ) : (
              <>
                <div className="run-grid">
                  <Panel title="Схемы" hint={`в сравнении · ${selected.size}`}>
                    <FadeScroll className="run-scroll">
                      {selSchemes.map((s) => (
                        <div key={s.guid} className={`run-row${s.active ? " active" : ""}`}>
                          <span className="run-row-name">
                            <span className="run-dot" />
                            {s.name || "Без названия"}
                          </span>
                          <Badge kind={s.active ? "ok" : "plain"}>
                            {s.active ? "активна" : "участвует"}
                          </Badge>
                        </div>
                      ))}
                    </FadeScroll>
                  </Panel>
                  <Panel title="Параметры">
                    <div className="run-row">
                      <span className="run-k">Режим</span>
                      <Badge kind="accent" big>
                        {PRESET_SHORT[activePreset]}
                      </Badge>
                    </div>
                    <div className="run-row">
                      <span className="run-k">Длительность теста</span>
                      <b>{duration} с</b>
                    </div>
                    <div className="run-row">
                      <span className="run-k">Разогрев / Охлаждение</span>
                      <b>
                        {warmup} с / {cooling} с
                      </b>
                    </div>
                    <div className="run-row">
                      <span className="run-k">Повторов (раундов)</span>
                      <b>{reps}</b>
                    </div>
                    <div className="run-row">
                      <span className="run-k">Подробные замеры</span>
                      <b>{rawSamples ? "да" : "нет"}</b>
                    </div>
                  </Panel>
                </div>
                <div className="run-summary">
                  <div className="grow">
                    <div className="run-total">
                      Длительность теста: <b>{estimate}</b>
                    </div>
                    <div className="hint">
                      {selected.size}{" "}
                      {plural(selected.size, "схема", "схемы", "схем")} × {reps}{" "}
                      {plural(reps, "раунд", "раунда", "раундов")}. Фазы: Лёгкая / Тяжёлая /
                      Отклик. По окончании исходная схема питания восстанавливается автоматически.
                    </div>
                  </div>
                  <Button
                    variant="primary"
                    big
                    disabled={starting || running || selected.size === 0}
                    title={selected.size === 0 ? "Выберите хотя бы одну схему питания" : undefined}
                    onClick={() => start(false)}
                  >
                    {starting ? "Запуск…" : "Запустить тест"}
                  </Button>
                </div>
                {readiness && !readiness.ok && !running ? (
                  <div className="ready-warn inset warn">
                    <div className="ready-title">Окружение не готово к замеру</div>
                    {readiness.issues.map((t, i) => (
                      <div key={i} className="ready-item">
                        {t}
                      </div>
                    ))}
                  </div>
                ) : null}
                {finished ? (
                  <Glass className="inset">
                    <div className="card-title">Тест завершён</div>
                    <div className="hint">{finishMsg || "сессия завершена — можно начать новую"}</div>
                  </Glass>
                ) : null}
              </>
            )
          ) : null}
        </div>
      </div>
    </div>
  );
}
