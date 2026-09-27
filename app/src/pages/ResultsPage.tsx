// Страница «Результаты»: история сессий, сравнение по истории, HTML-отчёты.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  commands,
  onTestFinished,
  type HistoryRow,
  type SessionJson,
} from "../api";
import { Badge, Button, Dropdown, FadeScroll, Modal, Panel } from "../components/ui";
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

/** Число для показа: нефинитное/отсутствующее → прочерк. */
function f1(v: number | null | undefined, digits = 1): string {
  return typeof v === "number" && Number.isFinite(v) ? v.toFixed(digits) : "—";
}

/** `20260927T020233Z307` → `27.09.2026 · 02:02`. */
export function fmtStamp(stamp: string): string {
  const m = /^(\d{4})(\d{2})(\d{2})T(\d{2})(\d{2})/.exec(stamp);
  if (!m) return stamp;
  return `${m[3]}.${m[2]}.${m[1]} · ${m[4]}:${m[5]}`;
}

export default function ResultsPage() {
  const [rows, setRows] = useState<HistoryRow[]>([]);
  const [sort, setSort] = useState<SortKey>("started");
  const [stats, setStats] = useState<{
    free_bytes: number;
    history_bytes: number;
    max_sessions: number;
  } | null>(null);
  const [detail, setDetail] = useState<SessionJson | null>(null);
  const busy = useRef(false);
  const reportBusy = useRef(false);

  const refresh = useCallback(() => {
    commands.historyList().then(setRows).catch((e) => pushToast("err", String(e)));
    commands
      .storageStats()
      .then((s) =>
        setStats({
          free_bytes: s.free_bytes,
          history_bytes: s.history_bytes,
          max_sessions: s.max_sessions,
        }),
      )
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

  const sorted = useMemo(() => {
    const arr = [...rows];
    switch (sort) {
      case "started":
        arr.sort((a, b) => (b.readable ? b.started_at_ns : -Infinity) - (a.readable ? a.started_at_ns : -Infinity));
        break;
      case "level":
        arr.sort(
          (a, b) =>
            (a.readable ? (LEVEL_ORDER[a.level] ?? 99) : 99) -
            (b.readable ? (LEVEL_ORDER[b.level] ?? 99) : 99),
        );
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

  // HTML-отчёт сессии: бэкенд сохраняет .html рядом с историей и открывает
  // его в браузере, сюда возвращается путь — показываем только имя файла.
  const openReport = (guid: string) => {
    if (reportBusy.current) return;
    reportBusy.current = true;
    commands
      .sessionReport(guid)
      .then((p) => {
        const name = p.split(/[/\\]/).pop() ?? p;
        pushToast("okk", `HTML-отчёт открыт в браузере: ${name}`);
      })
      .catch((e) => pushToast("err", String(e)))
      .finally(() => {
        reportBusy.current = false;
      });
  };

  const deleteRow = async (r: HistoryRow) => {
    if (r.readable && !window.confirm("Удалить запись?")) return;
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

  const lowDisk = stats != null && stats.free_bytes < 250 * 1024 * 1024;

  return (
    <div className="page fill">
      <div className="page-head">
        <h1>Результаты</h1>
        <span className="sub">{rows.length} записей</span>
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
        <div className="page-head-line">
          <span className="storage-line">
            {lowDisk ? <Badge kind="warn">Место на диске заканчивается</Badge> : null}
            Свободно {fmtBytes(stats?.free_bytes ?? 0)} · история {fmtBytes(stats?.history_bytes ?? 0)}
            {stats && stats.max_sessions > 0 ? ` · хранится до ${stats.max_sessions}` : ""}
          </span>
        </div>
      </div>

      <FadeScroll className="results-list fill-list">
        {sorted.length === 0 ? (
          <Panel title="Нет сопоставимых сессий">
            <div className="muted">Завершённые сессии с данными по схемам появятся здесь.</div>
          </Panel>
        ) : (
          sorted.map((r) => (
            <div
              key={r.file_name}
              className="glass lift res-row"
              onClick={() => r.readable && openSession(r.plan_guid)}
            >
              <div style={{ minWidth: 0 }}>
                <div className="res-name">{r.readable ? r.scheme_name || "Сессия без схемы" : r.file_name}</div>
                <div className="res-meta">
                  <span className="res-when">{r.readable ? fmtStamp(r.started_label) : "—"}</span>
                  {r.readable ? <Badge kind={levelKind(r.level)}>{r.level_label}</Badge> : <Badge kind="plain">Не завершено</Badge>}
                  <span className="num">{r.schemes} сх.</span>
                  {r.throughput != null ? (
                    <span className="num">{f1(r.throughput, 0)} тик/с</span>
                  ) : null}
                  {r.margin != null && Number.isFinite(r.margin) ? (
                    <span className="num">ДИ ±{r.margin.toFixed(1)}</span>
                  ) : null}
                  {r.early_stopped ? <Badge kind="plain">ранняя остановка</Badge> : null}
                  {!r.readable && r.error ? <span className="num">{r.error}</span> : null}
                </div>
              </div>
              <div className="res-acts">
                {r.score != null && Number.isFinite(r.score) ? (
                  <div className="res-score">
                    <b style={{ color: `var(--${scoreKind(r.score) === "ok" ? "ok" : scoreKind(r.score) === "warn" ? "warn" : "err"})` }}>
                      {r.score.toFixed(0)}
                    </b>
                    <span>балл</span>
                  </div>
                ) : null}
                {r.readable ? (
                  <Button sm variant="ghost" title="Открыть HTML-отчёт в браузере" onClick={(e) => {
                    e.stopPropagation();
                    openReport(r.plan_guid);
                  }}>
                    HTML
                  </Button>
                ) : null}
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
      </FadeScroll>

      <Modal open={detail !== null} title="Результат сессии" wide onClose={() => setDetail(null)}>
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
  const [opening, setOpening] = useState(false);

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

  const openHtml = () => {
    if (opening) return;
    setOpening(true);
    commands
      .sessionReport(s.plan_guid)
      .then((p) => {
        const name = p.split(/[/\\]/).pop() ?? p;
        pushToast("okk", `HTML-отчёт открыт в браузере: ${name}`);
      })
      .catch((e) => pushToast("err", String(e)))
      .finally(() => setOpening(false));
  };

  // Лидер: рекомендованная схема, иначе лучшая по медиане среди допущенных.
  const tie = rec.level === "Equivalent" || rec.level === "KeepCurrent";
  const admitted = s.schemes.filter((x) => !x.rejected);
  const leader =
    (!tie && rec.recommended_scheme
      ? s.schemes.find((x) => x.scheme_id === rec.recommended_scheme)
      : undefined) ??
    [...admitted].sort((a, b) => b.median_throughput - a.median_throughput)[0] ??
    null;
  const leaderName = leader ? (leader.name ?? leader.scheme_id) : null;
  const margin = rec.expected_margin_percent;
  const marginText =
    margin != null && Number.isFinite(margin) ? `${margin >= 0 ? "+" : ""}${margin.toFixed(2)}%` : "—";
  const marginKind = margin == null || !Number.isFinite(margin) ? "" : margin >= 1 ? "ok" : "warn";

  // Таблица: лидер первым, затем остальные по убыванию медианы; забракованные — в конце.
  const tableRows = [...s.schemes].sort((a, b) => {
    if (a.rejected !== b.rejected) return a.rejected ? 1 : -1;
    return b.median_throughput - a.median_throughput;
  });
  const rejectedCount = s.schemes.length - admitted.length;
  const totalRuns = s.schemes.reduce((n, x) => n + x.runs, 0);

  // Доверие к лидерству: при <2 прогонов вердикт предварительный.
  const trustNote = !leader
    ? { kind: "warn" as const, title: "Сравнивать нечего", items: ["Все схемы забракованы."] }
    : (leader.runs < 2
        ? {
            kind: "warn" as const,
            title: "Вердикт предварительный",
            items: [
              `У лидера ${leader.runs} прогон(ов) — повторите сессию в режиме «Детально» (3 повтора).`,
            ],
          }
        : { kind: "ok" as const, title: "Лидеру можно верить", items: ["Прогонов достаточно, выбросов не видно."] });

  return (
    <div className="rd">
      <div className="rd-hero">
        <div className="rd-kicker">{tie ? "ничья" : "лидер сессии"}</div>
        <div className="rd-lead ok">{leaderName ? `«${leaderName}»` : (rec.level_label ?? "—")}</div>
        <div className="row wrap gap-2" style={{ justifyContent: "center" }}>
          <Badge kind={levelKind(rec.level)} big>
            {rec.level_label ?? rec.level}
          </Badge>
          <Badge kind="plain">раундов {s.rounds_completed}/{s.rounds_planned}</Badge>
        </div>
      </div>

      <div className="rd-stats">
        <div className="rd-stat">
          <div className="k">Медиана лидера</div>
          <div className="v ok">{f1(leader?.median_throughput)}</div>
          <div className="s">тик/с</div>
        </div>
        <div className="rd-stat">
          <div className="k">Перевес</div>
          <div className={`v ${marginKind}`}>{marginText}</div>
          <div className="s">ожидаемый</div>
        </div>
        <div className="rd-stat">
          <div className="k">Схем</div>
          <div className="v">{admitted.length}</div>
          <div className="s">{rejectedCount ? `брак: ${rejectedCount}` : "допущено"}</div>
        </div>
        <div className="rd-stat">
          <div className="k">Прогонов</div>
          <div className="v">{totalRuns}</div>
          <div className="s">всего</div>
        </div>
      </div>

      <div className={`rd-note ${trustNote.kind}`}>
        <b>{trustNote.title}</b>
        <ul>
          {trustNote.items.map((t, i) => (
            <li key={i}>{t}</li>
          ))}
        </ul>
        {rec.reason ? <span className="why">{rec.reason}</span> : null}
      </div>

      {s.early_stop_reason ? (
        <div className="rd-note warn">
          <b>Ранняя остановка:</b> {s.early_stop_reason}
        </div>
      ) : null}

      {probs ? (
        <div className="rd-note">
          <b>Уверенность:</b> P(лучший) = {f1(probs[0], 2)} · P(перевес &gt; 0) ={" "}
          {f1(probs[1], 2)} · P(перевес &gt; 1%) = {f1(probs[2], 2)}
        </div>
      ) : null}

      {s.warnings.length > 0 ? (
        <div className="rd-note warn">{s.warnings.join(" ")}</div>
      ) : null}

      <div className="modal-scrollx">
        <table className="grid evid-table rd-table">
          <thead>
            <tr>
              <th>Схема</th>
              <th className="num">Медиана, тик/с</th>
              <th className="num">ДИ 95%</th>
              <th className="num">Прогонов</th>
              <th>Статус</th>
            </tr>
          </thead>
          <tbody>
            {tableRows.map((sch) => {
              const isLeader = leader != null && sch.scheme_id === leader.scheme_id;
              return (
                <tr key={sch.scheme_id} className={isLeader ? "lead" : sch.rejected ? "dead" : undefined}>
                  <td className="nm">
                    {isLeader ? "★ " : ""}
                    {sch.name ?? sch.scheme_id}
                  </td>
                  <td className="num">{f1(sch.median_throughput)}</td>
                  <td className="num">
                    {sch.ci_95[0] > 0 ? `[${f1(sch.ci_95[0])}; ${f1(sch.ci_95[1])}]` : "—"}
                  </td>
                  <td className="num">{sch.runs}</td>
                  <td>
                    {sch.rejected ? (
                      <Badge kind="danger">забракована</Badge>
                    ) : isLeader ? (
                      <Badge kind="ok">лидер</Badge>
                    ) : (
                      <Badge kind="plain">допущена</Badge>
                    )}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>

      <div className="rd-foot">
        <Button variant="primary" disabled={opening} onClick={openHtml}>
          {opening ? "Открываю…" : "Открыть HTML-отчёт"}
        </Button>
        <span className="spacer" />
        <Button variant="danger" onClick={() => void del()}>
          Удалить запись
        </Button>
      </div>
    </div>
  );
}
