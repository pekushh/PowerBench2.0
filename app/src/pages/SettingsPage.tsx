// Страница «Настройки»: оформление с живым превью, статус окружения,
// каталоги данных.

import { useEffect, useRef, useState } from "react";
import { commands, type SettingsDto } from "../api";
import { Button, Glass, Seg, Switch } from "../components/ui";
import { ShieldIcon, SocketIcon } from "../components/icons";
import { pushToast } from "../store";

type ModeName = "Dark" | "Light" | "Auto";

/** Отсечение неизвестного значения: без него `Seg` остался бы без выбора. */
function isMode(v: string): v is ModeName {
  return v === "Dark" || v === "Light" || v === "Auto";
}

// Ползунки весов и кольцо их распределения убраны вместе с
// весовым баллом: в отчёте теперь категории по throughput, P1 и
// стабильности, и настраивать нечего.
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
        <h2 className="section-title">Состояние системы</h2>
        <span className="hint">Схемы сравниваются по средней throughput, худшей секунде (P1) и стабильности — настраивать веса не нужно.</span>
      </div>
      <Glass className="inset">
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
