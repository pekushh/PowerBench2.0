// Страница «Тестирование»: настройка и запуск теста, live-телеметрия 10 Гц,
// график ticks/s, прогресс фазы, остановка и продолжение из контрольной точки.

import { useCallback, useEffect, useRef, useState } from "react";
import {
  commands,
  onLog,
  onTelemetry,
  onTestFinished,
  type CheckpointDto,
  type LogMsg,
  type SchemeRow,
  type SettingsDto,
  type TelemetryMsg,
} from "../api";
import { Badge, Button, Field, Glass, Progress, Seg, Stat } from "../components/ui";
import { pushToast, setRunning, useSession } from "../store";

const CHART_POINTS = 180;

interface PresetDef {
  duration: number;
  warmup: number;
  cooling: number;
  reps: number;
}

const PRESETS: Record<string, PresetDef> = {
  quick: { duration: 9, warmup: 2, cooling: 1, reps: 1 },
  detailed: { duration: 30, warmup: 6, cooling: 5, reps: 3 },
};

export default function TestPage() {
  const { running } = useSession();
  const [schemes, setSchemes] = useState<SchemeRow[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [settings, setSettings] = useState<SettingsDto | null>(null);
  const [preset, setPreset] = useState("detailed");
  const [duration, setDuration] = useState(30);
  const [warmup, setWarmup] = useState(6);
  const [cooling, setCooling] = useState(5);
  const [reps, setReps] = useState(3);
  const [checkpoint, setCheckpoint] = useState<CheckpointDto | null>(null);
  const [telemetry, setTelemetry] = useState<TelemetryMsg | null>(null);
  const [chart, setChart] = useState<number[]>([]);
  const [feed, setFeed] = useState<LogMsg[]>([]);
  const [starting, setStarting] = useState(false);
  const [stopping, setStopping] = useState(false);
  const chartRef = useRef<number[]>([]);

  const applyPreset = (p: keyof typeof PRESETS) => {
    const def = PRESETS[p];
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
      .catch((e) => console.error(e));
    commands
      .listSchemes()
      .then((s) => {
        if (!alive) return;
        setSchemes(s);
        setSelected(new Set(s.map((x) => x.guid)));
      })
      .catch((e) => pushToast("err", String(e)));
    commands
      .getSettings()
      .then((st) => {
        if (!alive) return;
        setSettings(st);
        setDuration(st.duration_seconds);
        setWarmup(st.warmup_seconds);
        setCooling(st.cooling_seconds);
        setReps(st.repetitions);
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
      setFeed((f) => [...f.slice(-7), m]);
    });
    const unfin = onTestFinished((m) => {
      setRunning(false);
      setStarting(false);
      setStopping(false);
      setTelemetry(null);
      commands
        .checkpointStatus()
        .then((cp) => setCheckpoint(cp))
        .catch(() => undefined);
      const ok = m.ok;
      if (!ok) pushToast("err", m.error ?? "сессия завершилась с ошибкой");
      else {
        const msg = m.cancelled
          ? "Сессия остановлена; контрольная точка сохранена."
          : `Готово: ${m.level_label}${m.recommended_name ? ` · схема «${m.recommended_name}»` : ""}`;
        pushToast(ok ? "okk" : "info", msg);
      }
    });
    return () => {
      untele.then((f) => f());
      unlog.then((f) => f());
      unfin.then((f) => f());
    };
  }, []);

  useEffect(() => {
    commands.testRunning().then((r) => setRunning(r)).catch(() => undefined);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const start = useCallback(
    (resume: boolean) => {
      const chosen = [...selected];
      if (chosen.length === 0) {
        pushToast("err", "выберите хотя бы одну схему питания");
        return;
      }
      setStarting(true);
      commands
        .startTest({
          preset,
          duration_seconds: resume ? null : duration,
          warmup_seconds: resume ? null : warmup,
          cooling_seconds: resume ? null : cooling,
          repetitions: resume ? null : reps,
          background_threshold_percent: resume ? null : (settings?.background_threshold_percent ?? 5),
          worker_count: null,
          scheme_ids: resume ? [] : chosen,
          resume,
        })
        .then((guid) => {
          pushToast("info", `Сессия ${guid} запущена`);
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
    [selected, preset, duration, warmup, cooling, reps, settings],
  );

  const stop = () => {
    setStopping(true);
    commands
      .stopTest()
      .then((was) => pushToast("info", was ? "отправлен запрос остановки" : "сессия не выполняется"))
      .catch((e) => pushToast("err", String(e)))
      .finally(() => setStopping(false));
  };

  const excluded = settings?.excluded_schemes ?? [];
  const favorites = settings?.favorite_schemes ?? [];

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
    const pts = chart.map((v, i) => `${(i * step).toFixed(1)},${(h - (v / max) * (h - 8) - 4).toFixed(1)}`);
    return (
      <svg viewBox={`0 0 ${w} ${h}`} className="chart" preserveAspectRatio="none">
        <polyline
          points={pts.join(" ")}
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
      {!running ? (
        <>
          <Glass>
            <div className="row between wrap">
              <div>
                <div className="card-title">Новая сессия</div>
                <div className="sub" style={{ color: "var(--text-3)" }}>
                  Длительность делится на фазы Лёгкая / Тяжёлая / Отклик; после теста активная схема
                  восстанавливается автоматически.
                </div>
              </div>
            </div>
            <div className="row wrap" style={{ marginTop: 12 }}>
              <Seg
                options={[
                  { value: "quick", label: "Быстрый" },
                  { value: "detailed", label: "Рекомендуемый" },
                ]}
                value={preset}
                onChange={applyPreset}
              />
            </div>
            <div className="grid2" style={{ marginTop: 12 }}>
              <Field label="Длительность, с (≥9)">
                <input type="number" min={9} value={duration} onChange={(e) => setDuration(+e.target.value)} />
              </Field>
              <Field label="Разогрев, с (≥2)">
                <input type="number" min={2} value={warmup} onChange={(e) => setWarmup(+e.target.value)} />
              </Field>
              <Field label="Охлаждение, с (0–60)">
                <input type="number" min={0} max={60} value={cooling} onChange={(e) => setCooling(+e.target.value)} />
              </Field>
              <Field label="Повторов (раундов), 1–9">
                <input type="number" min={1} max={9} value={reps} onChange={(e) => setReps(+e.target.value)} />
              </Field>
            </div>
            <div className="sub" style={{ color: "var(--text-3)", marginTop: 6 }}>
              Порог фоновой нагрузки {settings?.background_threshold_percent ?? 5}% на ядро (Настройки).
            </div>
          </Glass>

          {checkpoint && !checkpoint.original_restored && (
            <Glass className="inset">
              <div className="row between wrap">
                <div>
                  <div className="card-title">Прерванная сессия</div>
                  <div className="sub" style={{ color: "var(--text-3)" }}>
                    {checkpoint.plan_guid} · схем {checkpoint.scheme_ids.length} · повторов{" "}
                    {checkpoint.repetitions} · {checkpoint.completed_keys.length} выполненных раундов
                  </div>
                </div>
                <Button variant="primary" onClick={() => start(true)}>
                  Продолжить из контрольной точки
                </Button>
              </div>
            </Glass>
          )}

          <Glass>
            <div className="card-title">Схемы питания для теста</div>
            <div className="sub" style={{ color: "var(--text-3)", marginBottom: 10 }}>
              Активная схема отмечена кружком. Исключённые настройками показаны приглушённо — галочка
              включает их для этой сессии.
            </div>
            <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
              {schemes.map((s) => {
                const isExcluded = excluded.some((g) => g.toLowerCase() === s.guid.toLowerCase());
                const isFav = favorites.some((g) => g.toLowerCase() === s.guid.toLowerCase());
                const on = selected.has(s.guid);
                return (
                  <div key={s.guid} className={`scheme-item ${on ? "on" : ""}`} onClick={() => {
                    setSelected((prev) => {
                      const next = new Set(prev);
                      if (next.has(s.guid)) next.delete(s.guid);
                      else next.add(s.guid);
                      return next;
                    });
                  }}>
                    <span>{on ? "☑" : "☐"}</span>
                    <div className="grow">
                      <div className="name">
                        {s.name} {s.active ? <Badge kind="ok">активна</Badge> : null} {isFav ? <Badge kind="warn">★</Badge> : null}
                        {isExcluded ? <Badge kind="plain">исключена</Badge> : null}
                      </div>
                      <div className="guid">{s.guid}</div>
                    </div>
                  </div>
                );
              })}
            </div>
            <div style={{ marginTop: 14 }}>
              <Button id="test-start" variant="primary" big disabled={starting || schemes.length === 0} onClick={() => start(false)}>
                {starting ? "Запуск…" : "Запустить тест"} <kbd className="kbd">Ctrl+Enter</kbd>
              </Button>
            </div>
          </Glass>
        </>
      ) : (
        <>
          <Glass>
            <div className="row between wrap">
              <div>
                <div className="card-title">
                  Идёт измерение{telemetry?.scheme_name ? ` · «${telemetry.scheme_name}»` : ""}
                </div>
                <div className="sub" style={{ color: "var(--text-3)" }}>
                  {telemetry
                    ? `Прогон ${telemetry.run_index}/${telemetry.run_total} · раунд ${telemetry.round + 1} · фаза «${telemetry.phase}»`
                    : "Подготовка…"}
                </div>
              </div>
              <Button variant="danger" disabled={stopping} onClick={stop}>
                {stopping ? "Остановка…" : "Остановить"} <kbd>Esc</kbd>
              </Button>
            </div>
          </Glass>

          <div className="grid2">
            <Stat label="Скорость" value={telemetry ? `${telemetry.ticks_per_sec.toFixed(1)}` : "—"} suffix={"тик/с"} />
            <Stat label="Время тика" value={telemetry ? `${telemetry.ms_per_tick.toFixed(3)}` : "—"} suffix="мс" />
            <Stat label="Тиков" value={telemetry ? `${telemetry.ticks_done}` : "—"} />
            <Stat label="Фаза" value={telemetry ? `${(phaseProgress).toFixed(0)}%` : "—"} />
          </div>

          <Glass>
            <div className="card-title">График скорости, тик/с</div>
            {chartSvg ?? <div className="sub" style={{ color: "var(--text-3)", height: 120, display: "flex", alignItems: "center" }}>Ожидание данных…</div>}
            <div style={{ marginTop: 10 }}>
              <Progress value={phaseProgress} />
            </div>
          </Glass>

          <Glass className="inset">
            <div className="card-title">Ход сессии</div>
            <div className="log">
              {feed.length === 0 ? (
                <div className="sub" style={{ color: "var(--text-3)" }}>Сессия стартует…</div>
              ) : (
                feed.map((l, i) => (
                  <div key={i} className={`log-line ${l.level}`}>
                    <span className="ts">{l.ts_ms % 100000}</span>
                    <span className="tx">{l.text}</span>
                  </div>
                ))
              )}
            </div>
          </Glass>
        </>
      )}
    </div>
  );
}