// Страница «Результаты»: история завершённых сессий, бейджи доказательности,
// расширенные данные прогонов, экспорт JSON/CSV, пометка старых записей.

import { useCallback, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  commands,
  fmtNs,
  onTestFinished,
  type HistoryRow,
  type IdentityDto,
  type SessionJson,
} from "../api";
import { Badge, Button, Glass, Seg } from "../components/ui";
import { pushToast } from "../store";

type SortKey = "started" | "level";
type ExportFormat = "json" | "csv";

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

export default function ResultsPage() {
  const [rows, setRows] = useState<HistoryRow[]>([]);
  const [sort, setSort] = useState<SortKey>("started");
  const [selected, setSelected] = useState<SessionJson | null>(null);
  const [selectedGuid, setSelectedGuid] = useState<string | null>(null);
  const [currentIdentity, setCurrentIdentity] = useState<IdentityDto | null>(null);
  const busy = useRef(false);

  const refresh = useCallback(() => {
    commands
      .historyList()
      .then(setRows)
      .catch((e) => pushToast("err", String(e)));
  }, []);

  useEffect(() => {
    refresh();
    commands.identityInfo().then(setCurrentIdentity).catch(() => undefined);
    const unfor = onTestFinished(() => refresh());
    return () => {
      unfor.then((f) => f());
    };
  }, [refresh]);

  const openSession = (guid: string) => {
    setSelectedGuid(guid);
    commands
      .historyOpen(guid)
      .then(setSelected)
      .catch((e) => {
        pushToast("err", String(e));
        setSelected(null);
      });
  };

  const sorted = [...rows].sort((a, b) => {
    if (sort === "started") return a.started_label.localeCompare(b.started_label);
    return a.level.localeCompare(b.level);
  }).reverse();

  const exportSessions = async (format: ExportFormat) => {
    if (busy.current) return;
    busy.current = true;
    try {
      const dir = await open({ directory: true, title: "Каталог для экспорта" });
      if (!dir) return;
      const guid = selectedGuid ?? "all";
      const paths = await commands.historyExportTo(guid, format, dir);
      const note = paths.find((p) => p.startsWith("| "));
      pushToast("okk", `Экспортировано файлов: ${paths.length - (note ? 1 : 0)}`);
      if (note) pushToast("err", note.slice(2));
    } catch (e) {
      pushToast("err", String(e));
    } finally {
      busy.current = false;
    }
  };

  const legacyBadge = (s: SessionJson): React.ReactNode | null => {
    if (!currentIdentity) return null;
    const id = s.identity;
    const compat =
      id.workload_version === currentIdentity.workload_version &&
      id.config_hash === currentIdentity.config_hash &&
      id.seed_hex === currentIdentity.seed_hex;
    if (compat) return null;
    return <Badge kind="warn" title={`Версия ${id.workload_version}, хэш ${id.config_hash}, seed ${id.seed_hex}`}>другая версия нагрузки</Badge>;
  };

  return (
    <div className="page">
      <div className="page-head">
        <h1>Результаты</h1>
        <span className="sub">{rows.length} записей</span>
        <div className="grow" />
        <div className="row">
          <Seg
            options={[
              { value: "started", label: "По дате" },
              { value: "level", label: "По уровню" },
            ]}
            value={sort}
            onChange={setSort}
          />
          <Button onClick={() => openSession(selectedGuid ?? "")} disabled={!selectedGuid}>
            Открыть
          </Button>
          <Button onClick={() => exportSessions("json")}>Экспорт JSON</Button>
          <Button onClick={() => exportSessions("csv")}>Экспорт CSV</Button>
          <Button onClick={refresh}>Обновить</Button>
        </div>
      </div>

      <Glass>
        <table className="grid">
          <thead>
            <tr>
              <th>Старт (UTC)</th>
              <th>План</th>
              <th className="num">Схем</th>
              <th>Уровень</th>
              <th>Совместимость</th>
            </tr>
          </thead>
          <tbody>
            {sorted.map((r) => {
              const isSel = r.plan_guid !== "" && r.plan_guid === selectedGuid;
              return (
                <tr key={r.file_name} className="clickable" onClick={() => r.readable && openSession(r.plan_guid)}>
                  <td>{r.readable ? r.started_label : r.file_name}</td>
                  <td>
                    <span className="num">{r.plan_guid}</span>
                    {isSel ? <span> </span> : null}
                  </td>
                  <td className="num">{r.readable ? r.schemes : "—"}</td>
                  <td>
                    {r.readable ? (
                      <Badge kind={levelKind(r.level)}>{r.level_label}</Badge>
                    ) : (
                      <Badge kind="danger" title={r.error ?? ""}>не читается</Badge>
                    )}
                  </td>
                  <td>{isSel && selected ? legacyBadge(selected) : null}</td>
                </tr>
              );
            })}
            {sorted.length === 0 ? (
              <tr>
                <td colSpan={5} style={{ color: "var(--text-3)" }}>
                  История пуста. Завершённые сессии появляются здесь автоматически.
                </td>
              </tr>
            ) : null}
          </tbody>
        </table>
      </Glass>

      {selected ? (
        <Glass className="fade-in">
          <div className="row between wrap">
            <div>
              <div className="card-title">
                {selected.plan_guid} <Badge kind={levelKind(selected.recommendation.level)} big>{selected.recommendation.level_label}</Badge> {legacyBadge(selected)}
              </div>
              <div className="sub" style={{ color: "var(--text-3)" }}>
                {selected.recommendation.reason}
              </div>
            </div>
            <div className="sub num" style={{ color: "var(--text-3)" }}>
              исходная схема: <span className="num">{selected.original_scheme_guid ?? "—"}</span>
            </div>
          </div>
          <table className="grid" style={{ marginTop: 12 }}>
            <thead>
              <tr>
                <th>Схема</th>
                <th className="num">Прогоны</th>
                <th className="num">Средний throughput</th>
                <th className="num">CV</th>
                <th className="num">медиана Отклик p1</th>
                <th className="num">стабильность</th>
                <th>Статус</th>
              </tr>
            </thead>
            <tbody>
              {selected.schemes.map((s) => (
                <tr key={s.scheme_id} title="Клик для прогонов">
                  <td style={{ maxWidth: 260, overflow: "hidden", textOverflow: "ellipsis" }}>
                    {s.name ?? s.scheme_id}
                    <div className="sub num" style={{ color: "var(--text-3)", fontSize: 11 }}>{s.scheme_id}</div>
                  </td>
                  <td className="num">{s.runs}</td>
                  <td className="num">{Number.isFinite(s.mean_average_throughput) ? s.mean_average_throughput.toFixed(1) : "—"}</td>
                  <td className="num">{Number.isFinite(s.run_variation_percent) ? `${s.run_variation_percent.toFixed(1)}%` : "—"}</td>
                  <td className="num">{s.median_p1_throughput.toFixed(1)}</td>
                  <td className="num">{`${s.median_consistency_percent.toFixed(1)}%`}</td>
                  <td>
                    {s.rejected ? <Badge kind="danger">{s.rejection_reason ?? "забракована"}</Badge> : <Badge kind="ok">допущена</Badge>}
                    {s.cv_warning ? <span> </span> : null}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          <PerRunGrid session={selected} />
        </Glass>
      ) : null}
    </div>
  );
}

function PerRunGrid({ session }: { session: SessionJson }) {
  const runs = session.schemes.flatMap((s) => s.per_run.map((r) => ({ ...r, schemeName: s.name ?? s.scheme_id })));
  if (runs.length === 0) return null;
  return (
    <>
      <div className="card-title" style={{ marginTop: 16 }}>Прогоны ({runs.length})</div>
      <table className="grid">
        <thead>
          <tr>
            <th>Схема</th>
            <th className="num">раунд</th>
            <th className="num">тики</th>
            <th className="num">длит., мс</th>
            <th className="num">арифм. среднее</th>
            <th className="num">Отклик p01</th>
            <th className="num">консистентность</th>
            <th className="num">cкачки</th>
          </tr>
        </thead>
        <tbody>
          {runs.map((r) => (
            <tr key={`${r.key}:${r.scheme_id}`}>
              <td style={{ maxWidth: 220, overflow: "hidden", textOverflow: "ellipsis" }}>{r.schemeName}</td>
              <td className="num">{r.round + 1}</td>
              <td className="num">{r.ticks}</td>
              <td className="num">{r.duration_ms}</td>
              <td className="num">{r.combined.mean.toFixed(1)}</td>
              <td className="num">{r.combined.p01.toFixed(1)}</td>
              <td className="num">{`${r.cross_phase_consistency.toFixed(1)}%`}</td>
              <td className="num">{r.spike_windows}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <div className="sub" style={{ color: "var(--text-3)", marginTop: 8 }}>
        Идентичность нагрузки: {session.identity.workload_version} · хэш {session.identity.config_hash} · seed{" "}
        {session.identity.seed_hex} · воркеров {session.identity.worker_count} · старт {fmtNs(session.schemes[0]?.started_at_min_ns ?? 0)}
      </div>
    </>
  );
}