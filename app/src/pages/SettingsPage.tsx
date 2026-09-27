// Страница «Настройки»: оформление с живым превью, скоринг с донатом,
// статус окружения, каталоги данных.

import { useEffect, useState, type CSSProperties } from "react";
import { commands, type SettingsDto } from "../api";
import { Badge, Button, Glass, Seg, Switch } from "../components/ui";
import { ShieldIcon, SocketIcon } from "../components/icons";
import { pushToast } from "../store";

type ModeName = "Dark" | "Light" | "Auto";

const WEIGHT_COLORS = ["#5aa2f2", "#a78bfa", "#e4b46f"];

function WSlider({
  label,
  value,
  color,
  onChange,
}: {
  label: string;
  value: number;
  color: string;
  onChange: (v: number) => void;
}) {
  return (
    <label className="wslider" style={{ "--c": color } as CSSProperties}>
      <span className="wslider-head">
        <span>{label}</span>
        <b>{Math.round(value)}%</b>
      </span>
      <input
        type="range"
        min={0}
        max={100}
        step={5}
        value={value}
        onChange={(e) => onChange(Number(e.target.value))}
        style={{
          background: `linear-gradient(90deg, ${color} 0%, ${color} ${value}%, var(--bg-3) ${value}%, var(--bg-3) 100%)`,
        }}
      />
    </label>
  );
}

function Donut({ values }: { values: { label: string; value: number; color: string }[] }) {
  const total = values.reduce((s, v) => s + v.value, 0);
  const r = 40;
  const c = 2 * Math.PI * r;
  let acc = 0;
  return (
    <div className="donut-wrap">
      <div className="donut">
        <svg viewBox="0 0 100 100" width="100" height="100">
          <circle cx="50" cy="50" r={r} fill="none" stroke="var(--bg-3)" strokeWidth="13" />
          {total > 0
            ? values.map((v, i) => {
                const len = (v.value / total) * c;
                const off = -acc;
                acc += len;
                if (len <= 0) return null;
                return (
                  <circle
                    key={i}
                    cx="50"
                    cy="50"
                    r={r}
                    fill="none"
                    stroke={v.color}
                    strokeWidth="13"
                    strokeDasharray={`${len} ${c - len}`}
                    strokeDashoffset={off}
                    transform="rotate(-90 50 50)"
                    strokeLinecap="butt"
                  />
                );
              })
            : null}
        </svg>
        <div className="donut-center">
          <b>100%</b>
          <span>всего</span>
        </div>
      </div>
      <div className="donut-legend">
        {values.map((v, i) => (
          <div className="row" key={i}>
            <i style={{ background: v.color }} />
            <span>{v.label}</span>
            <b>{Math.round(v.value)}%</b>
          </div>
        ))}
      </div>
    </div>
  );
}

export default function SettingsPage({ onAppearance }: { onAppearance: (s: SettingsDto) => void }) {
  const [st, setSt] = useState<SettingsDto | null>(null);
  const [adm, setAdm] = useState<boolean | null>(null);
  const [ac, setAc] = useState<boolean | null>(null);
  const [dataDir, setDataDir] = useState("");
  const [cfgDir, setCfgDir] = useState("");

  useEffect(() => {
    commands.getSettings().then(setSt).catch(() => undefined);
    commands.isAdmin().then(setAdm).catch(() => undefined);
    commands.acPowerOnline().then(setAc).catch(() => undefined);
    commands.resultsDir().then(setDataDir).catch(() => undefined);
    commands.appsettingsPath().then(setCfgDir).catch(() => undefined);
  }, []);

  if (!st) return <div className="page">Загрузка настроек…</div>;

  const patch = (p: Partial<SettingsDto>) => {
    const next = { ...st, ...p };
    setSt(next);
    commands.setSettings(next).then(() => onAppearance(next)).catch((e) => pushToast("err", String(e)));
  };

  const weightsSum = st.score_performance + st.score_stability + st.score_worst_second;

  return (
    <div className="page tight">
      <div className="page-head">
        <h1>Настройки</h1>
        <div className="actions">
          <Button
            variant="ghost"
            onClick={() => dataDir && commands.openFolder(dataDir).catch((e) => pushToast("err", String(e)))}
          >
            Папка результатов
          </Button>
          <Button
            variant="ghost"
            disabled={!cfgDir}
            title={cfgDir || undefined}
            onClick={() => cfgDir && commands.openFolder(cfgDir).catch((e) => pushToast("err", String(e)))}
          >
            Папка настроек
          </Button>
        </div>
      </div>

      <div className="section-head">
        <h2 className="section-title">Внешний вид</h2>
        <span className="hint">Спокойный интерфейс, который не отвлекает от результата.</span>
      </div>
      <Glass className="appearance-card">
        <div className="preview-pane">
          <div className="preview-orb" />
          <div className="preview-window">
            <div className="preview-dots">
              <i />
              <i />
              <i />
            </div>
            <div className="preview-bar long" />
            <div className="preview-bar" />
            <div className="preview-bar" />
            <div className="preview-bar short" />
          </div>
        </div>
        <div className="appearance-controls">
          <div className="field">
            <label>Тема</label>
            <Seg
              options={[
                { value: "Dark", label: "Тёмная" },
                { value: "Light", label: "Светлая" },
                { value: "Auto", label: "Авто" },
              ]}
              value={st.mode as ModeName}
              onChange={(v) => patch({ mode: v })}
            />
          </div>
          <div className="reduce-row">
            <div>
              <div className="reduce-title">Уменьшить движение</div>
              <div className="hint">Мягкие переходы и анимации</div>
            </div>
            <Switch checked={st.reduce_motion} onChange={(v) => patch({ reduce_motion: v })} />
          </div>
        </div>
      </Glass>

      <div className="section-head">
        <h2 className="section-title">Скоринг схем</h2>
        <span className="hint">Вес показателей влияет на итоговый балл. Сумма нормализуется автоматически.</span>
      </div>
      <Glass className="scoring-card">
        <div className="sliders">
          <WSlider
            label="Производительность"
            color={WEIGHT_COLORS[0]}
            value={st.score_performance}
            onChange={(v) => patch({ score_performance: v })}
          />
          <WSlider
            label="Стабильность"
            color={WEIGHT_COLORS[1]}
            value={st.score_stability}
            onChange={(v) => patch({ score_stability: v })}
          />
          <WSlider
            label="Худшая секунда"
            color={WEIGHT_COLORS[2]}
            value={st.score_worst_second}
            onChange={(v) => patch({ score_worst_second: v })}
          />
          {weightsSum <= 0 ? (
            <div style={{ paddingTop: 12 }}>
              <Badge kind="danger">Сумма весов должна быть больше 0.</Badge>
            </div>
          ) : null}
          <button
            className="reset-link"
            title="Вернуть веса по умолчанию: 50/30/20"
            onClick={() => patch({ score_performance: 50, score_stability: 30, score_worst_second: 20 })}
          >
            Сбросить к 50 / 30 / 20
          </button>
        </div>
        <div className="donut-pane">
          <div className="donut-title">Распределение веса</div>
          <Donut
            values={[
              { label: "Производительность", value: st.score_performance, color: WEIGHT_COLORS[0] },
              { label: "Стабильность", value: st.score_stability, color: WEIGHT_COLORS[1] },
              { label: "Худшая секунда", value: st.score_worst_second, color: WEIGHT_COLORS[2] },
            ]}
          />
          <div className="donut-chips">
            <span className={`status-chip ${adm ? "is-ok" : "is-bad"}`} title={adm ? "Запуск от имени администратора" : "Нет прав администратора"}>
              <ShieldIcon />
              {adm == null ? "…" : adm ? "Администратор" : "Нет прав"}
            </span>
            <span className={`status-chip ${ac ? "is-ok" : "is-bad"}`} title={ac ? "Питание от сети" : "Работа от батареи"}>
              <SocketIcon />
              {ac == null ? "…" : ac ? "Сеть" : "Батарея"}
            </span>
          </div>
        </div>
      </Glass>
    </div>
  );
}
