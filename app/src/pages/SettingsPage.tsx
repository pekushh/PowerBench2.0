// Страница «Настройки»: оформление, скоринг, данные, система.

import { useEffect, useState } from "react";
import { commands, type SettingsDto } from "../api";
import { Badge, Button, Panel, Seg, Switch } from "../components/ui";
import { pushToast } from "../store";

type ThemeName = "Graphite" | "Ocean" | "Violet";
type ModeName = "Dark" | "Light";

function WeightSlider({
  label,
  value,
  onChange,
}: {
  label: string;
  value: number;
  onChange: (v: number) => void;
}) {
  return (
    <label className="weight-slider">
      <span>{label}</span>
      <input
        type="range"
        min={0}
        max={100}
        step={5}
        value={value}
        onChange={(e) => onChange(Number(e.target.value))}
      />
      <b>{Math.round(value)}%</b>
    </label>
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
    <div className="page">
      <div className="page-head">
        <h1>Настройки</h1>
        <span className="sub">изменения сохраняются автоматически</span>
      </div>

      <Panel title="Оформление">
        <div className="row wrap gap-3">
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
          <Switch
            checked={st.reduce_motion}
            onChange={(v) => patch({ reduce_motion: v })}
            label="уменьшить движение"
          />
        </div>
      </Panel>

      <Panel title="Скоринг схем">
        <div className="hint mb-2">
          Веса трёх метрик влияют на балл в отчёте «Балл по вашим весам» (100 = лучшая среди
          допущенных). Сумма весов нормируется автоматически.
        </div>
        <div className="row wrap gap-3">
          <WeightSlider
            label="Производительность"
            value={st.score_performance}
            onChange={(v) => patch({ score_performance: v })}
          />
          <WeightSlider
            label="Стабильность"
            value={st.score_stability}
            onChange={(v) => patch({ score_stability: v })}
          />
          <WeightSlider
            label="Худшая секунда"
            value={st.score_worst_second}
            onChange={(v) => patch({ score_worst_second: v })}
          />
        </div>
        {weightsSum <= 0 ? <Badge kind="danger">Сумма весов должна быть больше 0.</Badge> : null}
        <div className="row mt-3">
          <Button
            sm
            variant="ghost"
            title="Вернуть веса по умолчанию: 50/30/20"
            onClick={() =>
              patch({ score_performance: 50, score_stability: 30, score_worst_second: 20 })
            }
          >
            Сброс весов (50/30/20)
          </Button>
        </div>
      </Panel>

      <Panel title="Данные">
        <div className="row wrap gap-3">
          <label className="retention-field">
            <span>Хранить завершённых сессий</span>
            <input
              type="number"
              min={0}
              max={100000}
              step={10}
              value={st.max_sessions}
              onChange={(e) => {
                const v = Math.max(0, Math.min(100000, Math.trunc(Number(e.target.value)) || 0));
                patch({ max_sessions: v });
              }}
            />
          </label>
        </div>
        <div className="hint mt-2">
          Старые сессии удаляются автоматически при превышении лимита.
        </div>
        <div className="row wrap gap-3 mt-3">
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
      </Panel>

      <Panel title="Система">
        <div className="row wrap gap-3">
          <span>
            {adm === null ? "…" : adm ? <Badge kind="ok">администратор</Badge> : <Badge kind="danger">не админ</Badge>}
          </span>
          <span>
            {ac === null ? "…" : ac ? <Badge kind="ok">Питание от сети</Badge> : <Badge kind="warn">батарея</Badge>}
          </span>
        </div>
        {adm === false ? (
          <div className="hint mt-2">
            Запустите от имени администратора, чтобы включать запрет сна и менять схемы питания.
          </div>
        ) : null}
      </Panel>
    </div>
  );
}
