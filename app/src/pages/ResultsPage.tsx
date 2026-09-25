// Страница «Результаты»: история сессий, сравнение по истории, отчёты.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  commands,
  onTestFinished,
  type HistoryRow,
  type SessionJson,
} from "../api";
import { Badge, Button, Dropdown, Modal, Panel } from "../components/ui";
import { pushToast } from "../store";

type SortKey = "started" | "level" | "score" | "stability";

const LEVEL_ORDER: Record<string, number> = {
  Confirmed: 0,
  Probable: 1,
  StabilityTieBreak: 2,
  Preliminary: 3,
  KeepCurrent: 4,
};

function levelKind(level: string): "ok" | "warn" | "plain" | "danger" {
  switch (level) {
    case "Confirmed":
      return "ok";
    case "Probable":
    case "StabilityTieBreak":
      return "danger";
    case "Preliminary":
    case "KeepCurrent":
      return "warn";
    default:
      return "plain";
  }
}

function scoreKind(score: number | null): "ok" | "warn" | "danger" {
  if (score == null) return "danger";
  if (score >= 90) return "ok";
  if (score >= 75) return "warn";
  return "danger";
}

export function fmtBytes(n: number): string {
  if (!n) return "0 Б";
  if (n >= 1 << 30) return `${(n / (1 << 30)).toFixed(1)} ГБ`;
  if (n >= 1 << 20) return `${(n / (1 << 20)).toFixed(1)} МБ`;
  if (n >= 1 << 10) return `${(n / (1 << 10)).toFixed(1)} КБ`;
  return `${n} Б`;
}

interface SeriesPoint {
  col: number;
  score: number;
  lo: number;
  hi: number;
  file: string;
}

interface Series {
  id: string;
  name: string;
  pts: SeriesPoint[];
}

const PALETTE = ["#5aa2f2", "#6fd0a0", "#e4b46f", "#e57979", "#b8acff", "#58c7ee"];

export default function ResultsPage({ active = true }: { active?: boolean }) {
  const [rows, setRows] = useState<HistoryRow[]>([]);
  const [sort, setSort] = useState<SortKey>("started");
  const [stats, setStats] = useState<{
    free_bytes: number;
    history_bytes: number;
    max_sessions: number;
  } | null>(null);
  const [detail, setDetail] = useState<SessionJson | null>(null);
  const [series, setSeries] = useState<Series[]>([]);
  const [hidden, setHidden] = useState<Set<string>>(new Set());
  const [chartSessions, setChartSessions] = useState<HistoryRow[]>([]);
  const busy = useRef(false);

  const refresh = useCallback(() => {
    commands.historyList().then(setRows).catch((e) => pushToast("err", String(e)));
    commands
      .storageStats()
      .then((s) => setStats({ free_bytes: s.free_bytes, history_bytes: s.history_bytes, max_sessions: s.max_sessions }))
      .catch(() => undefined);
  }, []);

  useEffect(() => {
    refresh();
    const unfor = onTestFinished((m) => {
      refresh();
      if (m.ok && m.plan_guid) {
        commands.historyOpen(m.plan_guid).then(setDetail).catch(() => undefined);
      }
    });
    return () => {
      unfor.then((f) => f());
    };
  }, [refresh]);

  // График сравнения: медианный throughput схем по последним сессиям.
  // Строится только на активной вкладке, чтобы не дёргать историю зря.
  useEffect(() => {
    if (!active) return;
    const recent = [...rows]
      .filter((r) => r.readable)
      .sort((a, b) => (a.started_at_ns < b.started_at_ns ? 1 : -1))
      .slice(0, 24)
      .reverse();
    setChartSessions(recent);
    let alive = true;
    Promise.all(
      recent.map((r) => commands.historyOpen(r.plan_guid).then((s) => ({ row: r, s })).catch(() => null)),
    ).then((all) => {
      if (!alive) return;
      const byScheme = new Map<string, Series>();
      all.forEach((item, col) => {
        if (!item) return;
        for (const sch of item.s.schemes) {
          if (sch.rejected) continue;
          const name = sch.name ?? sch.scheme_id;
          let se = byScheme.get(name);
          if (!se) {
            se = { id: name, name, pts: [] };
            byScheme.set(name, se);
          }
          se.pts.push({
            col,
            score: sch.median_throughput,
            lo: sch.ci_95[0],
            hi: sch.ci_95[1],
            file: item.row.plan_guid,
          });
        }
      });
      setSeries([...byScheme.values()]);
    });
    return () => {
      alive = false;
    };
  }, [rows, active]);

  const sorted = useMemo(() => {
    const arr = [...rows];
    switch (sort) {
      case "started":
        arr.sort((a, b) => {
          const x = a.readable ? a.started_at_ns : -Infinity;
          const y = b.readable ? b.started_at_ns : -Infinity;
          return y - x;
        });
        break;
      case "level":
        arr.sort((a, b) => (a.readable ? (LEVEL_ORDER[a.level] ?? 99) : 99) - (b.readable ? (LEVEL_ORDER[b.level] ?? 99) : 99));
        break;
      case "score":
        arr.sort((a, b) => (b.score ?? -1) - (a.score ?? -1));
        break;
      case "stability":
        arr.sort((a, b) => (b.stability ?? -1) - (a.stability ?? -1));
        break;
    }
    return arr;
  }, [rows, sort]);

  const openSession = (guid: string) => {
    commands.historyOpen(guid).then(setDetail).catch((e) => pushToast("err", String(e)));
  };

  const openReport = (guid: string) => {
    commands
      .sessionReport(guid)
      .then((p) => pushToast("okk", `Отчёт открыт: ${p}`))
      .catch((e) => pushToast("err", String(e)));
  };

  const deleteRow = async (r: HistoryRow) => {
    if (r.readable) {
      if (!window.confirm("Удалить запись?")) return;
    }
    try {
      await commands.historyDelete(r.file_name);
      pushToast("okk", "Запись удалена");
      if (detail?.plan_guid === r.plan_guid) setDetail(null);
      refresh();
    } catch (e) {
      pushToast("err", String(e));
    }
  };

  const exportAll = async (format: "json" | "csv") => {
    if (busy.current) return;
    busy.current = true;
    try {
      const dir = await open({ directory: true, title: "Каталог для экспорта" });
      if (!dir) return;
      const paths = await commands.historyExportTo("all", format, dir as string);
      const note = paths.find((p) => p.startsWith("| "));
      pushToast("okk", `Экспортировано файлов: ${paths.length - (note ? 1 : 0)}`);
      if (note) pushToast("err", note.slice(2));
    } catch (e) {
      pushToast("err", String(e));
    } finally {
      busy.current = false;
    }
  };

  const lowDisk =
    stats != null && (stats.free_bytes < 250 * 1024 * 1024);

  return (
    <div className="page">
      <div className="page-head">
        <h1>Результаты</h1>
        <span className="sub">{rows.length} записей</span>
        <div className="page-head-line">
          <span className="sub storage-line">
            {lowDisk ? <Badge kind="warn">Место на диске заканчивается</Badge> : null}
            Свободно {fmtBytes(stats?.free_bytes ?? 0)} · История {fmtBytes(stats?.history_bytes ?? 0)}
            {stats && stats.max_sessions > 0
              ? ` · хранится до ${stats.max_sessions}`
              : stats
                ? " · хранится без ограничений"
                : ""}
          </span>
        </div>
        <div className="actions">
          <Dropdown
            title="Сортировка"
            value={sort}
            onChange={setSort}
            options={[
              { value: "started", label: "По дате" },
              { value: "level", label: "По уровню" },
              { value: "score", label: "По баллу" },
              { value: "stability", label: "По стабильности" },
            ]}
          />
          <Button variant="ghost" onClick={() => void exportAll("json")}>
            Экспорт…
          </Button>
          <Button
            variant="ghost"
            title="Открыть папку с результатами"
            onClick={() => commands.historyOpenFolder().catch((e) => pushToast("err", String(e)))}
          >
            Папка
          </Button>
          <Button variant="ghost" title="Обновить список" onClick={refresh}>
            Обновить
          </Button>
        </div>
      </div>

      <div className="page-toolbar">
        <Panel
          title="Сравнение производительности схем по истории"
          hint="Медианный throughput каждой схемы по сессиям; полосы — ДИ 95%; клик по точке — HTML-отчёт сессии"
        >
          <Chart series={series} sessions={chartSessions} hidden={hidden} onToggleHide={(id) =>
            setHidden((prev) => {
              const next = new Set(prev);
              if (next.has(id)) next.delete(id);
              else next.add(id);
              return next;
            })
          } onDot={(file) => openReport(file)} />
        </Panel>
      </div>

      <div className="results-list">
        {sorted.length === 0 ? (
          <Panel title="Нет сопоставимых сессий">
            <div className="muted">Завершённые сессии с данными по схемам появятся здесь.</div>
          </Panel>
        ) : (
          sorted.map((r) => (
            <div key={r.file_name} className="glass lift rc-row" onClick={() => r.readable && openSession(r.plan_guid)}>
              <div className="rc-main">
                <div className="rc-line1">
                  <span className="rc-name">{r.readable ? r.started_label : r.file_name}</span>
                  {r.readable ? <Badge kind={levelKind(r.level)}>{r.level_label}</Badge> : <Badge kind="plain">Не завершено</Badge>}
                  {r.early_stopped ? <Badge kind="plain">ранняя остановка</Badge> : null}
                </div>
                <div className="rc-line2">
                  <span>
                    {r.schemes} сх. {r.scheme_name ? `· ${r.scheme_name}` : ""}
                  </span>
                  {r.score != null ? <span className="num">балл {r.score.toFixed(1)}</span> : null}
                  {r.margin != null ? <span className="num">ДИ ±{r.margin.toFixed(1)}</span> : null}
                  {!r.readable && r.error ? <span>{r.error}</span> : null}
                </div>
              </div>
              <div className="rc-actions">
                {r.score != null ? <Badge kind={scoreKind(r.score)}>{r.score.toFixed(0)}</Badge> : null}
                <Button
                  sm
                  variant="ghost"
                  onClick={(e) => {
                    e.stopPropagation();
                    void deleteRow(r);
                  }}
                >
                  Удалить
                </Button>
              </div>
            </div>
          ))
        )}
      </div>

      <Modal open={detail !== null} title="Результат сессии" onClose={() => setDetail(null)}>
        {detail ? (
          <SessionDetail
            s={detail}
            fileName={rows.find((r) => r.plan_guid === detail.plan_guid)?.file_name ?? ""}
            onDelete={() => {
              setDetail(null);
              refresh();
            }}
          />
        ) : null}
      </Modal>
    </div>
  );
}

function Chart({
  series,
  sessions,
  hidden,
  onToggleHide,
  onDot,
}: {
  series: Series[];
  sessions: HistoryRow[];
  hidden: Set<string>;
  onToggleHide: (id: string) => void;
  onDot: (planGuid: string) => void;
}) {
  const W = 720;
  const H = 240;
  const pad = { l: 56, r: 12, t: 12, b: 28 };
  const shown = series.filter((s) => !hidden.has(s.id) && s.pts.length > 0);
  if (sessions.length < 1 || shown.length === 0) {
    return <div className="chart-empty muted">Нет данных для сравнения.</div>;
  }
  const vals = shown.flatMap((s) => s.pts.flatMap((p) => [p.lo, p.hi]));
  const min = Math.min(...vals);
  const max = Math.max(...vals);
  const span = Math.max(1, max - min);
  const y = (v: number) => pad.t + (1 - (v - min) / span) * (H - pad.t - pad.b);
  const x = (col: number) =>
    sessions.length <= 1
      ? (W - pad.l - pad.r) / 2 + pad.l
      : pad.l + (col / (sessions.length - 1)) * (W - pad.l - pad.r);
  const gridVals = [0, 1, 2, 3, 4].map((i) => min + (span * i) / 4);
  return (
    <div>
      <div className="chart-legend">
        {series.map((s, i) => (
          <button
            key={s.id}
            className="chart-chip"
            title="Скрыть/показать"
            onClick={() => onToggleHide(s.id)}
          >
            <i style={{ background: PALETTE[i % PALETTE.length] }} />
            {s.name || "—"}
          </button>
        ))}
        <span className="chart-note">полосы — ДИ 95%</span>
      </div>
      <div className="chart-wrap">
        <svg width="100%" height={H} viewBox={`0 0 ${W} ${H}`} role="img" aria-label="Сравнение производительности схем по истории">
          {gridVals.map((v) => (
            <g key={v}>
              <line className="chart-grid" x1={pad.l} x2={W - pad.r} y1={y(v)} y2={y(v)} />
              <text className="chart-ylab" x={pad.l - 8} y={y(v) + 3.5} textAnchor="end">
                {v.toFixed(0)}
              </text>
            </g>
          ))}
          <line
            className="chart-grid chart-axis"
            x1={pad.l}
            x2={W - pad.r}
            y1={H - pad.b}
            y2={H - pad.b}
          />
          {sessions.map((s, i) =>
            i % Math.max(1, Math.ceil(sessions.length / 5)) === 0 ? (
              <text key={s.file_name} className="chart-xlab" x={x(i)} y={H - 12} textAnchor="middle">
                {s.started_label}
              </text>
            ) : null,
          )}
          {shown.map((s) => {
            const color = PALETTE[series.indexOf(s) % PALETTE.length];
            const d = s.pts.map((p, k) => `${k === 0 ? "M" : "L"}${x(p.col).toFixed(1)} ${y(p.score).toFixed(1)}`).join(" ");
            return (
              <g key={s.id}>
                {s.pts.map((p) => (
                  <line
                    key={p.col}
                    className="chart-ci"
                    x1={x(p.col)}
                    x2={x(p.col)}
                    y1={y(p.hi)}
                    y2={y(p.lo)}
                    stroke={color}
                  />
                ))}
                <path className="chart-line" d={d} fill="none" stroke={color} strokeWidth={1.8} strokeLinejoin="round" />
                {s.pts.map((p) => (
                  <circle
                    key={p.col}
                    className="chart-dot"
                    cx={x(p.col)}
                    cy={y(p.score)}
                    r={4}
                    fill={color}
                    onClick={() => onDot(p.file)}
                  >
                    <title>{`${s.name}: ${p.score.toFixed(1)} тик/с (ДИ 95% ±${((p.hi - p.lo) / 2).toFixed(1)})`}</title>
                  </circle>
                ))}
              </g>
            );
          })}
        </svg>
      </div>
    </div>
  );
}

function SessionDetail({
  s,
  fileName,
  onDelete,
}: {
  s: SessionJson;
  fileName: string;
  onDelete: () => void;
}) {
  const rec = s.recommendation;
  const probs = rec.probabilities;
  const del = async () => {
    if (!fileName) {
      pushToast("err", "Файл записи не найден");
      return;
    }
    if (!window.confirm("Удалить запись?")) return;
    try {
      await commands.historyDelete(fileName);
      pushToast("okk", "Запись удалена");
      onDelete();
    } catch (e) {
      pushToast("err", String(e));
    }
  };
  return (
    <div className="result-body">
      <div className="result-hero">
        <div className="verdict">{rec.level_label ?? "—"}</div>
        <Badge kind={levelKind(rec.level)} big>
          уровень подтверждён
        </Badge>
      </div>
      {s.early_stop_reason ? (
        <div className="row wrap gap-3">
          <Badge kind="accent" big>
            ранняя остановка
          </Badge>
          <span className="hint">{s.early_stop_reason}</span>
        </div>
      ) : null}
      {rec.recommended_scheme ? (
        <div className="result-rec">
          <span className="rl">Рекомендуемая схема</span>
          <span className="rv">«{rec.recommended_scheme}»</span>
        </div>
      ) : null}
      {probs ? (
        <div className="evid-probs">
          <span>P(лучший)={probs[0].toFixed(2)}</span>
          <span>P(перевес&gt;0)={probs[1].toFixed(2)}</span>
          <span>P(перевес&gt;1%)={probs[2].toFixed(2)}</span>
        </div>
      ) : null}
      {s.warnings.length > 0 ? (
        <div className="hint">{s.warnings.join(" ")}</div>
      ) : null}
      <div className="rec-schemes-narrow">
        {s.schemes.map((sch) => (
          <div key={sch.scheme_id} className="glass inset">
            <div className="rc-name">{sch.name ?? sch.scheme_id}</div>
            <div className="hint">
              медиана {sch.median_throughput.toFixed(1)} тик/с · ДИ [{sch.ci_95[0].toFixed(1)};{" "}
              {sch.ci_95[1].toFixed(1)}] · прогонов: {sch.runs}
            </div>
          </div>
        ))}
      </div>
      <div className="results-table-wrap">
        <table className="grid evid-table">
          <thead>
            <tr>
              <th>Схема</th>
              <th className="num">Медиана, тик/с</th>
              <th className="num">ДИ 95%</th>
              <th className="num">Прогоны</th>
            </tr>
          </thead>
          <tbody>
            {s.schemes.map((sch) => (
              <tr key={sch.scheme_id}>
                <td>{sch.name ?? sch.scheme_id}</td>
                <td className="num">{sch.median_throughput.toFixed(1)}</td>
                <td className="num">
                  [{sch.ci_95[0].toFixed(1)}; {sch.ci_95[1].toFixed(1)}]
                </td>
                <td className="num">{sch.runs}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <div className="result-actions">
        <Button
          variant="ghost"
          onClick={() =>
            commands
              .sessionReport(s.plan_guid)
              .then((p) => pushToast("okk", `Отчёт открыт: ${p}`))
              .catch((e) => pushToast("err", String(e)))
          }
        >
          Открыть отчёт
        </Button>
        <Button variant="danger" onClick={() => void del()}>
          Удалить
        </Button>
      </div>
    </div>
  );
}
