// Страница «Настройки»: параметры сценария по умолчанию, оформление (тема,
// режим, reduce motion, sidebar), идентичность нагрузки, системные сведения.

import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import type { IdentityDto, SettingsDto } from "../api";
import { commands } from "../api";
import { Badge, Field, Glass, Seg, Stat } from "../components/ui";
import { pushToast } from "../store";

type ThemeName = "Graphite" | "Ocean" | "Violet";
type ModeName = "Dark" | "Light";

export default function SettingsPage({ onAppearance }: { onAppearance: (s: SettingsDto) => void }) {
  const [st, setSt] = useState<SettingsDto | null>(null);
  const [ident, setIdent] = useState<IdentityDto | null>(null);
  const [adm, setAdm] = useState<boolean | null>(null);
  const [ac, setAc] = useState<boolean | null>(null);
  const [dataDir, setDataDir] = useState("");
  const [cfgDir, setCfgDir] = useState("");
  const [version, setVersion] = useState("");

  useEffect(() => {
    commands.getSettings().then((s) => setSt(s)).catch(() => undefined);
    commands.identityInfo().then(setIdent).catch(() => undefined);
    commands.isAdmin().then(setAdm).catch(() => undefined);
    commands.acPowerOnline().then(setAc).catch(() => undefined);
    commands.resultsDir().then(setDataDir).catch(() => undefined);
    commands.appsettingsPath().then(setCfgDir).catch(() => undefined);
    getVersion().then(setVersion).catch(() => undefined);
  }, []);

  if (!st) return <div className="page">Загрузка настроек…</div>;

  const patch = (p: Partial<SettingsDto>) => {
    const next = { ...st, ...p };
    setSt(next);
    commands.setSettings(next).then(() => onAppearance(next)).catch((e) => pushToast("err", String(e)));
  };

  return (
    <div className="page">
      <div className="page-head">
        <h1>Настройки</h1>
        <span className="sub">изменения сохраняются автоматически</span>
      </div>

      <Glass>
        <div className="card-title">Оформление</div>
        <div className="row wrap">
          <div className="row" style={{ flexWrap: "wrap", gap: 12 }}>
            <Seg
              options={[
                { value: "Graphite", label: "Графит" },
                { value: "Ocean", label: "Океан" },
                { value: "Violet", label: "Фиолет" },
              ]}
              value={st.theme as ThemeName}
              onChange={(v) => patch({ theme: v })}
            />
            <Seg
              options={[
                { value: "Dark", label: "Тёмная" },
                { value: "Light", label: "Светлая" },
              ]}
              value={st.mode as ModeName}
              onChange={(v) => patch({ mode: v })}
            />
          </div>
          <div className="grow" />
          <label className="row" style={{ color: "var(--text-2)" }}>
            <input
              type="checkbox"
              checked={st.reduce_motion}
              onChange={(e) => patch({ reduce_motion: e.target.checked })}
            />
            уменьшить движение
          </label>
        </div>
      </Glass>

      <Glass>
        <div className="card-title">Сценарий по умолчанию</div>
        <div className="grid2">
          <Field label={`Длительность, с (≥9) — сейчас ${st.duration_seconds}`}>
            <input
              type="number"
              min={9}
              value={st.duration_seconds}
              onChange={(e) => patch({ duration_seconds: +e.target.value })}
            />
          </Field>
          <Field label={`Разогрев, с (≥2) — сейчас ${st.warmup_seconds}`}>
            <input
              type="number"
              min={2}
              value={st.warmup_seconds}
              onChange={(e) => patch({ warmup_seconds: +e.target.value })}
            />
          </Field>
          <Field label={`Охлаждение, с (0–60) — сейчас ${st.cooling_seconds}`}>
            <input
              type="number"
              min={0}
              max={60}
              value={st.cooling_seconds}
              onChange={(e) => patch({ cooling_seconds: +e.target.value })}
            />
          </Field>
          <Field label={`Повторов, 1–9 — сейчас ${st.repetitions}`}>
            <input
              type="number"
              min={1}
              max={9}
              value={st.repetitions}
              onChange={(e) => patch({ repetitions: +e.target.value })}
            />
          </Field>
          <Field label={`Порог фона, % на ядро — сейчас ${st.background_threshold_percent}`}>
            <input
              type="number"
              step={0.5}
              min={0.5}
              max={100}
              value={st.background_threshold_percent}
              onChange={(e) => patch({ background_threshold_percent: +e.target.value })}
            />
          </Field>
        </div>
      </Glass>

      <Glass>
        <div className="card-title">Скоринг (веса)</div>
        <div className="grid2">
          <Field label={`Performance — сейчас ${st.scoring_performance}`}>
            <input
              type="number"
              step={0.1}
              min={0}
              max={100}
              value={st.scoring_performance}
              onChange={(e) => patch({ scoring_performance: +e.target.value })}
            />
          </Field>
          <Field label={`Stability — сейчас ${st.scoring_stability}`}>
            <input
              type="number"
              step={0.1}
              min={0}
              max={100}
              value={st.scoring_stability}
              onChange={(e) => patch({ scoring_stability: +e.target.value })}
            />
          </Field>
          <Field label={`Worst second — сейчас ${st.scoring_worst_second}`}>
            <input
              type="number"
              step={0.1}
              min={0}
              max={100}
              value={st.scoring_worst_second}
              onChange={(e) => patch({ scoring_worst_second: +e.target.value })}
            />
          </Field>
        </div>
      </Glass>

      <Glass>
        <div className="card-title">Удержание данных</div>
        <div className="grid2">
          <Field label={`Макс. сессий — сейчас ${st.retention_max_sessions}`}>
            <input
              type="number"
              min={1}
              max={10000}
              value={st.retention_max_sessions}
              onChange={(e) => patch({ retention_max_sessions: +e.target.value })}
            />
          </Field>
        </div>
      </Glass>

      <Glass>
        <div className="card-title">Идентичность нагрузки</div>
        <div className="grid2">
          <Stat label="Версия нагрузка" value={ident?.workload_version ?? "—"} />
          <Stat label="Воркеров / ядер" value={ident ? `${ident.worker_count} / ${ident.logical_cpus}` : "—"} />
          <Stat label="Таймер" value={ident ? `${ident.timer_hz}` : "—"} suffix="Гц" />
          <Stat label="seed" value={ident?.seed_hex ?? "—"} />
        </div>
        {ident ? (
          <div className="sub" style={{ color: "var(--text-3)", marginTop: 8 }}>
            Хэш конфигурации: <span className="num">{ident.config_hash}</span> · CPU: {ident.cpu_identifier} ·
            диагностика: <span className="num">{ident.diagnostics_version}</span>
          </div>
        ) : null}
      </Glass>

      <Glass>
        <div className="card-title">О приложении</div>
        <div className="row wrap between">
          <div>
            PowerBench
            {version ? <Badge kind="plain" big>версия {version}</Badge> : null}
          </div>
          <div className="sub" style={{ color: "var(--text-3)" }}>
            Идентичность нагрузки и каталоги данных указаны выше.
          </div>
        </div>
      </Glass>

      <Glass>
        <div className="card-title">Система</div>
        <div className="grid2">
          <Stat
            label="Права администратора"
            value={adm === null ? "…" : adm ? "есть" : "нет"}
          />
          <Stat label="Питание от сети" value={ac === null ? "…" : ac ? "в сети" : "батарея"} />
        </div>
        <div className="sub" style={{ color: "var(--text-3)", marginTop: 8 }}>
          Результаты: <span className="num">{dataDir}</span>
          <br />
          Настройки: <span className="num">{cfgDir}</span>
        </div>
        {adm === false ? (
          <div style={{ marginTop: 8 }}>
            <Badge kind="warn">
              Запустите от имени администратора, чтобы включать запрет сна и менять схемы питания.
            </Badge>
          </div>
        ) : null}
      </Glass>
    </div>
  );
}