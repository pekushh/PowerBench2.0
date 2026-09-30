// Страница «Настройки»: оформление с живым превью, скоринг с донатом,
// статус окружения, каталоги данных.

import { useEffect, useRef, useState, type CSSProperties } from "react";
import { commands, type SettingsDto } from "../api";
import { Badge, Button, Glass, Seg, Switch } from "../components/ui";
import { ShieldIcon, SocketIcon } from "../components/icons";
import { pushToast } from "../store";

type ModeName = "Dark" | "Light" | "Auto";

/** Отсечение неизвестного значения: без него `Seg` остался бы без выбора. */
function isMode(v: string): v is ModeName {
  return v === "Dark" || v === "Light" || v === "Auto";
}

// Цвета весов повторяют акценты палитры приложения (info, violet, warn) и
// служат только для этого кольца.
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
          {/* Сумма весов может быть любой — при подсчёте они нормализуются.
              Показываем реальную сумму, а не жёсткие «100%»: иначе кольцо
              говорило бы «всего 100%», а легенда под ним — 50/30/20. */}
          <b>{Math.round(total)}%</b>
          <span>всего</span>
        </div>
      </div>
      <div className="donut-legend">
        {values.map((v, i) => (
          <div className="row" key={i}>
            <i style={{ background: v.color }} />
            <span>{v.label}</span>
            <b>{total > 0 ? Math.round((v.value / total) * 100) : 0}%</b>
          </div>
        ))}
      </div>
    </div>
  );
}

export default function SettingsPage({ onAppearance }: { onAppearance: (s: SettingsDto) => void }) {
  const [st, setSt] = useState<SettingsDto | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [adm, setAdm] = useState<boolean | null>(null);
  const [ac, setAc] = useState<boolean | null>(null);
  const [dataDir, setDataDir] = useState("");
  const [cfgDir, setCfgDir] = useState("");
  // Очередь записей. Хук объявлен до раннего возврата ниже: хуки нельзя
  // вызывать условно.
  const queue = useRef<Promise<unknown>>(Promise.resolve());

  useEffect(() => {
    commands
      .getSettings()
      .then(setSt)
      .catch((e) => setLoadError(String(e)));
    commands.isAdmin().then(setAdm).catch(() => undefined);
    commands.acPowerOnline().then(setAc).catch(() => undefined);
    commands.resultsDir().then(setDataDir).catch(() => undefined);
    commands.appsettingsPath().then(setCfgDir).catch(() => undefined);
  }, []);

  // Отказ в чтении настроек раньше оставлял страницу в вечном «Загрузка
  // настроек…»: ни сообщения, ни кнопки повтора. Пользователь не понимал,
  // что произошло, и думал, что приложение зависло.
  if (!st) {
    return (
      <div className="page">
        <div className="glass inset">
          <div className="card-title">Не удалось прочитать настройки</div>
          <div className="hint">{loadError ?? "Ответ приложения не получен."}</div>
          <div className="row gap-2" style={{ marginTop: 10 }}>
            <Button
              variant="primary"
              onClick={() => {
                setLoadError(null);
                commands
                  .getSettings()
                  .then(setSt)
                  .catch((e) => setLoadError(String(e)));
              }}
            >
              Повторить
            </Button>
          </div>
        </div>
      </div>
    );
  }

  // Записи настроек выстраиваются в очередь. Без неё каждый слайдер писал
  // «прочитал → изменил → записал» сам по себе, и два быстрых движения
  // ползунком затирали друг друга.
  const patch = (p: Partial<SettingsDto>) => {
    setSt((prev) => {
      if (!prev) return prev;
      // Показываем новое значение сразу (слайдеры должны реагировать), но при
      // ошибке возвращаем старое: иначе экран показывал бы настройки,
      // которых на диске нет.
      const next = { ...prev, ...p };
      queue.current = queue.current
        .then(() => commands.setSettings(next))
        .then(() => onAppearance(next))
        .catch((e: unknown) => {
          pushToast("err", `Настройка не сохранена: ${String(e)}`);
          // Перечитываем реальное состояние: иначе экран продолжал бы
          // показывать значение, которого на диске нет.
          commands
            .getSettings()
            .then(setSt)
            .catch(() => undefined);
        });
      return next;
    });
  };

  const weightsSum = st.score_performance + st.score_stability + st.score_worst_second;

  return (
    <div className="page tight">
      <div className="page-head">
        <h1>Настройки</h1>
        <div className="actions">
          <Button
            variant="ghost"
            disabled={!dataDir}
            title={dataDir || undefined}
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
            <span className="field-label">Тема</span>
            <Seg
              label="Тема оформления"
              options={[
                { value: "Dark", label: "Тёмная" },
                { value: "Light", label: "Светлая" },
                { value: "Auto", label: "Авто" },
              ]}
              // Значение приходит строкой: неизвестное отображаем как «Авто»,
              // иначе `Seg` остался бы без выбранного пункта.
              value={isMode(st.mode) ? st.mode : "Auto"}
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
              {/* Без точки — это короткая метка. */}
              <Badge kind="danger">Сумма весов должна быть больше нуля</Badge>
            </div>
          ) : null}
          <button
            className="reset-link"
            title="Вернуть веса по умолчанию: 40/30/30"
            onClick={() => patch({ score_performance: 40, score_stability: 30, score_worst_second: 30 })}
          >
            Сбросить к 40 / 30 / 30
          </button>
        </div>
        <div className="donut-pane">
          <div className="donut-title">Распределение весов</div>
          <Donut
            values={[
              { label: "Производительность", value: st.score_performance, color: WEIGHT_COLORS[0] },
              { label: "Стабильность", value: st.score_stability, color: WEIGHT_COLORS[1] },
              { label: "Худшая секунда", value: st.score_worst_second, color: WEIGHT_COLORS[2] },
            ]}
          />
          <div className="donut-chips">
            {/* Пока проверка не завершена, чип не должен мигать красным:
                `null` — это «неизвестно», а не «прав нет». */}
            <span
              className={`status-chip ${adm == null ? "" : adm ? "is-ok" : "is-bad"}`}
              title={adm ? "Запуск от имени администратора" : "Нет прав администратора"}
            >
              <ShieldIcon />
              {adm == null ? "…" : adm ? "Администратор" : "Нет прав"}
            </span>
            <span
              className={`status-chip ${ac == null ? "" : ac ? "is-ok" : "is-bad"}`}
              title={ac ? "Питание от сети" : "Работа от батареи"}
            >
              <SocketIcon />
              {ac == null ? "…" : ac ? "Сеть" : "Батарея"}
            </span>
          </div>
        </div>
      </Glass>

      <div className="section-head">
        <h2 className="section-title">Железо и BIOS</h2>
      </div>
      <Glass className="inset">
        <div className="field">
          <span className="field-label">Разгон, андерволт, отключённые функции</span>
          <textarea
            className="cpu-notes"
            value={st.cpu_notes}
            rows={3}
            maxLength={500}
            placeholder="например: PBO +200 МГц, андерволт −30, отключён SMT, 2×16 ГБ DDR5-6000"
            onChange={(e) => patch({ cpu_notes: e.target.value })}
          />
          <div className="hint">
            Попадёт в отчёт для поддержки. Заполнять нужно только если что-то
            меняли: программа не может отличить разгон от штатного буста —
            Windows показывает одну и ту же цифру в обоих случаях. На разгоне
            результат меняется сильнее, чем от схемы питания, поэтому без этой
            заметки сравнивать замеры нельзя.
          </div>
        </div>
      </Glass>
    </div>
  );
}
