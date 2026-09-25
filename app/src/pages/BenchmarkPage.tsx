// Страница «Бенчмарк»: визард Режим → Схемы → Запуск.

import { useCallback, useEffect, useRef, useState } from "react";
import {
  commands,
  onLog,
  onTelemetry,
  onTestFinished,
  type CheckpointDto,
  type LogMsg,
  type Readiness,
  type SchemeRow,
  type SettingsDto,
  type TelemetryMsg,
} from "../api";
import { Badge, Button, Field, Glass, NumInput, Panel, Progress, Stat } from "../components/ui";
import { GearIcon } from "../components/icons";
import { pushToast, setRunning, useSession } from "../store";
import { SchemeCards } from "../components/SchemeTiles";

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
  quick: "Быстро",
  detailed: "Детально",
  custom: "Пользовательская",
};

const STAGES: { id: Stage; label: string }[] = [
  { id: "mode", label: "Режим и настройки" },
  { id: "schemes", label: "Схемы питания" },
  { id: "run", label: "Запуск" },
];

/** ~1 мин 41 с */
export function fmtEst(totalSeconds: number): string {
  if (!Number.isFinite(totalSeconds)) return "~—";
  const t = Math.max(0, Math.round(totalSeconds));
  if (t >= 3600) return `~${Math.floor(t / 3600)} ч ${Math.round((t % 3600) / 60)} мин`;
  if (t >= 60) return `~${Math.floor(t / 60)} мин ${Math.round(t % 60)} с`;
  return `~${t} с`;
}

const T_TABLE = [12.706, 4.303, 3.182, 2.776, 2.571, 2.447, 2.365, 2.306, 2.262];

function earlyStopNeed(reps: number, cv = 0.05): number | null {
  if (reps < 2) return null;
  const n = T_TABLE[Math.min(reps - 2, T_TABLE.length - 1)];
  return ((2 * Math.SQRT2 * n * cv * 100) / Math.sqrt(reps));
}

function earlyStopHint(reps: number): string {
  if (!Number.isFinite(reps) || reps < 2)
    return "Адаптивная остановка невозможна при одном повторе — нужен перевес в данных минимум двух прогонов.";
  const need = earlyStopNeed(reps) ?? Infinity;
  if (need >= 100)
    return `При ${reps} повторах ранняя остановка практически недостижима (нужен перевес > 100% при разбросе прогонов ~5%). Рекомендуем 5+ повторов.`;
  return `Ранняя остановка возможна, если перевес ≈ ${need.toFixed(0)}% и более (при разбросе прогонов ~5%). Для практичного срабатывания рекомендуем 5+ повторов.`;
}

const CHART_POINTS = 180;

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
  const [feed, setFeed] = useState<LogMsg[]>([]);
  const [starting, setStarting] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [finished, setFinished] = useState(false);
  const [finishMsg, setFinishMsg] = useState("");
  const [elapsed, setElapsed] = useState(0);
  const [chart, setChart] = useState<number[]>([]);
  const [readiness, setReadiness] = useState<Readiness | null>(null);
  const chartRef = useRef<number[]>([]);
  const t0 = useRef(0);

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

  const estSeconds = duration * reps + warmup + cooling;

  const applyPreset = (p: "quick" | "detailed") => {
    const def = PRESETS[p].preset;
    setPreset(p);
    setDuration(def.duration);
    setWarmup(def.warmup);
    setCooling(def.cooling);
    setReps(def.reps);
  };


  useEffect(() => {
    let alive = true;
    commands
      .checkpointStatus()
      .then((cp) => alive && setCheckpoint(cp))
      .catch(() => undefined);
    commands
      .listSchemes()
      .then((s) => {
        if (!alive) return;
        setSchemes(s);
        setSelected(new Set(s.map((x) => x.guid)));
      })
      .catch(() => undefined);
    commands
      .getSettings()
      .then((st) => {
        if (!alive) return;
        setSettings(st);
        setDuration(num(st.duration_seconds, 30));
        setWarmup(num(st.warmup_seconds, 6));
        setCooling(num(st.cooling_seconds, 5));
        setReps(num(st.repetitions, 3));
      })
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, []);

  useEffect(() => {
    const untele = onTelemetry((m) => {
      chartRef.current = [...chartRef.current.slice(-(CHART_POINTS - 1)), m.ticks_per_sec];
      setChart(chartRef.current);
      setTelemetry(m);
    });
    const unlog = onLog((m) => {
      setFeed((f) => [...f.slice(-60), m]);
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
      unlog.then((f) => f());
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

  const selSchemes = schemes.filter((s) => selected.has(s.guid));

  useEffect(() => {
    if (!running) return;
    if (!t0.current) t0.current = Date.now();
    const id = window.setInterval(() => setElapsed((Date.now() - t0.current) / 1000), 500);
    return () => window.clearInterval(id);
  }, [running]);

  const go = (s: Stage, d: "fwd" | "back") => {
    setDir(d);
    setStage(s);
  };

  const start = useCallback(
    (resume: boolean) => {
      const chosen = [...selected];
      if (!resume && chosen.length === 0) {
        pushToast("err", "выберите хотя бы одну схему питания");
        return;
      }
      setStarting(true);
      setFinished(false);
      setFinishMsg("");
      t0.current = 0;
      setElapsed(0);
      commands
        .startTest({
          preset: preset ?? "detailed",
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
          chartRef.current = [];
          setChart([]);
          setFeed([]);
          setCheckpoint(null);
        })
        .catch((e) => {
          setStarting(false);
          pushToast("err", String(e));
        });
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [selected, preset, duration, warmup, cooling, reps, settings, rawSamples],
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
      ? "выберите пресет — Быстрый или Детальный; настройки открываются кнопкой ниже"
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
        <Button variant="primary" disabled={starting || running} onClick={() => start(false)}>
          {starting ? "Запуск…" : "Запустить тест"}
        </Button>
      </>
    );

  const phaseProgress =
    telemetry && telemetry.phase_seconds > 0
      ? (telemetry.phase_elapsed_ms / (telemetry.phase_seconds * 1000)) * 100
      : 0;

  const chartSvg = (() => {
    if (chart.length < 2) return null;
    const w = 720;
    const h = 120;
    const max = Math.max(1, ...chart);
    const step = w / (CHART_POINTS - 1);
    const pts = chart
      .map((v, i) => `${(i * step).toFixed(1)},${(h - (v / max) * (h - 8) - 4).toFixed(1)}`)
      .join(" ");
    return (
      <svg viewBox={`0 0 ${w} ${h}`} className="chart" preserveAspectRatio="none">
        <polyline
          points={pts}
          fill="none"
          stroke="var(--accent)"
          strokeWidth={2}
          strokeLinejoin="round"
        />
      </svg>
    );
  })();



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
                      <div
                        key={key}
                        className={`mode-card ${open ? "open" : ""}`}
                        role="button"
                        tabIndex={0}
                        onClick={() => {
                          if (!expanded) applyPreset(key);
                        }}
                        onKeyDown={(e) => {
                          if (e.key === "Enter" && !expanded) applyPreset(key);
                        }}
                      >
                        <div className="face">
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
                            Примерное время 1 плана ≈{" "}
                            <b>
                              {fmtEst(
                                def.preset.duration * def.preset.reps +
                                  def.preset.warmup +
                                  def.preset.cooling,
                              )}
                            </b>
                          </div>
                        </div>
                        {cardIdx === 0 ? (
                          <div className="settings-pane">
                            <h3 className="pane-title">Настройки теста</h3>
                            <span className="est">
                              Одна схема ≈ <b>{fmtEst(estSeconds)}</b>
                            </span>
                            <p className="desc">{PRESETS[preset].desc}</p>
                            <div className="spacer" />
                            <label className="check">
                              <input
                                type="checkbox"
                                checked={rawSamples}
                                onChange={(e) => setRawSamples(e.target.checked)}
                              />
                              <span>Сырые сэмплы (JSON)</span>
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
                      </div>
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
                <Button sm variant="ghost" onClick={() => setSelected(new Set(schemes.map((s) => s.guid)))}>
                  Выбрать все
                </Button>
                <Button
                  sm
                  variant="ghost"
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
                  Снять выбор
                </Button>
              </div>
              <SchemeCards
                schemes={schemes}
                selected={selected}
                excluded={
                  new Set((settings?.excluded_schemes ?? []).map((g) => g.toLowerCase()))
                }
                onToggle={(guid, active) => {
                  if (active && selected.has(guid)) {
                    pushToast("err", "Активная схема питания должна участвовать в тестировании — её нельзя снять.");
                    return;
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
                  <div className="muted">Ничего не найдено.</div>
                </div>
              ) : null}
              {checkpoint && !checkpoint.original_restored ? (
                <Glass className="inset">
                  <div className="card-title">Прерванная сессия</div>
                  <div className="hint" style={{ marginBottom: 10 }}>
                    {checkpoint.plan_guid} · схем {checkpoint.scheme_ids.length} · повторов{" "}
                    {checkpoint.repetitions} · {checkpoint.completed_keys.length} выполненных раундов
                  </div>
                  <Button variant="primary" onClick={() => start(true)}>
                    Продолжить из контрольной точки
                  </Button>
                </Glass>
              ) : null}
            </>
          ) : null}

          {stage === "run" ? (
            running || telemetry ? (
              <>
                <div className="run-hero">
                  <div className="rh-stat">
                    <div className="rh-label">Текущая скорость</div>
                    <div className="rh-value">
                      {telemetry ? telemetry.ticks_per_sec.toFixed(1) : "—"}
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
                <div className="grid2">
                  <Stat
                    label="Время тика"
                    value={telemetry ? `${telemetry.ms_per_tick.toFixed(3)}` : "—"}
                    suffix="мс"
                  />
                  <Stat label="Тиков" value={telemetry ? `${telemetry.ticks_done}` : "—"} />
                  <Stat label="Раунд" value={telemetry ? `${telemetry.round + 1}` : "—"} />
                  <Stat label="Фаза" value={telemetry?.phase ?? "—"} />
                </div>
                <Panel
                  title="Общий ход теста"
                  hint={`прошло ~${fmtEst(elapsed)} · осталось ~${fmtEst(
                    Math.max(0, estSeconds * Math.max(1, selected.size) - elapsed),
                  )}`}
                >
                  <Progress value={phaseProgress} />
                </Panel>
                <Panel title="Ход сессии" hint="живой журнал" className="inset">
                  <div className="log run-feed fade-bottom">
                    {feed.length === 0 ? (
                      <div className="muted">Сессия стартует…</div>
                    ) : (
                      feed.map((l, i) => (
                        <div key={i} className={`log-line ${l.level}`}>
                          <span className="ts">{l.ts_ms % 100000}</span>
                          <span className="tx">{l.text}</span>
                        </div>
                      ))
                    )}
                  </div>
                </Panel>
                {chartSvg ?? null}
              </>
            ) : (
              <>
                <div className="run-grid">
                  <Panel title="Схемы" hint={`в сравнении · ${selected.size}`}>
                    <div className={`run-scroll${selSchemes.length > 6 ? " fade-bottom" : ""}`}>
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
                    </div>
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
                      <span className="run-k">Сырые сэмплы</span>
                      <b>{rawSamples ? "да" : "нет"}</b>
                    </div>
                  </Panel>
                </div>
                <div className="run-summary">
                  <div className="grow">
                    <div className="run-total">
                      Примерная длительность:{" "}
                      <b>{fmtEst(estSeconds * Math.max(1, selected.size))}</b>
                    </div>
                    <div className="hint">
                      Фазы: Лёгкая / Тяжёлая / Отклик. По окончании активная схема восстанавливается
                      автоматически.
                    </div>
                  </div>
                  <Button variant="primary" big disabled={starting || running} onClick={() => start(false)}>
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
