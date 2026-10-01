// Страница «Настройки»: внешний вид, состояние системы, каталоги данных и
// заметка о железе.
//
// Раскладка — три секции с карточками-строками. Каждая настройка занимает
// строку «слева подпись со значком, справа управление», чтобы на узком окне
// управление просто переезжало под подпись, а не ломалось о край.

import { useEffect, useRef, useState } from "react";
import { commands, type SettingsDto } from "../api";
import { Button, Glass, Seg, Switch } from "../components/ui";
import { useCascade } from "../components/useCascade";
import { setSectionDetail } from "../store";
import { FolderIcon, GridIcon, MotionIcon, ShieldIcon, SocketIcon, ThemeIcon } from "../components/icons";
import { pushToast } from "../store";

type ModeName = "Dark" | "Light" | "Auto";
type DensityName = "compact" | "normal" | "roomy";
type TextScaleName = "s" | "m" | "l";

/** Отсечение неизвестного значения: без него `Seg` остался бы без выбора. */
function isMode(v: string): v is ModeName {
  return v === "Dark" || v === "Light" || v === "Auto";
}

/** Плотность: неизвестное значение из конфига показываем как «обычно». */
function isDensity(v: string): v is DensityName {
  return v === "compact" || v === "normal" || v === "roomy";
}

/** Масштаб текста: неизвестное значение показываем как обычный. */
function isTextScale(v: string): v is TextScaleName {
  return v === "s" || v === "m" || v === "l";
}

/**
 * Частые записи в заметку о железе.
 *
 * Поле объясняет, что писать, но объяснение в подсказке не помогает в момент,
 * когда человек вспомнил, что у него включён PBO. Теги дописывают фрагмент в
 * конец — и заметка остаётся читаемой, как обычный текст в отчёте.
 */
const QUICK_TAGS: { label: string; text: string }[] = [
  { label: "PBO", text: "PBO +200 МГц" },
  { label: "Curve Optimizer", text: "Curve Optimizer −30" },
  { label: "XMP / EXPO", text: "XMP / EXPO активен" },
  { label: "SMT выкл", text: "SMT отключён" },
  { label: "Фикс. частота", text: "Фикс. частота CPU" },
];

export default function SettingsPage({ onAppearance, active = true }: { onAppearance: (s: SettingsDto) => void; active?: boolean }) {
  /** Режим «Авто» показываем словами, а не служебным значением. */
  function resolveModeName(mode: string): string {
    if (mode === "Auto") {
      return window.matchMedia("(prefers-color-scheme: light)").matches ? "светлая" : "тёмная";
    }
    return mode === "Light" ? "светлая" : "тёмная";
  }
  const [st, setSt] = useState<SettingsDto | null>(null);
  const rootRef = useCascade<HTMLDivElement>(active);
  // Состояние для крошки в шапке: тема и режим движения.
  useEffect(() => {
    if (!st) return;
    const motion = st.reduce_motion ? "движение уменьшено" : "анимации включены";
    const mode =
      st.mode === "Auto"
        ? `тема ${resolveModeName(st.mode)}`
        : `тема ${st.mode === "Light" ? "светлая" : "тёмная"}`;
    setSectionDetail(`${mode} · ${motion}`);
    return () => setSectionDetail("");
  }, [st?.theme, st?.mode, st?.reduce_motion]);
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
        <Glass className="inset">
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
        </Glass>
      </div>
    );
  }

  // Записи настроек выстраиваются в очередь. Без неё каждое действие писало
  // «прочитал → изменил → записал» само по себе, и два быстрых переключения
  // затирали друг друга.
  const patch = (p: Partial<SettingsDto>) => {
    setSt((prev) => {
      if (!prev) return prev;
      // Показываем новое значение сразу (управление должно реагировать), но при
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

  const notes = st.cpu_notes.trim();
  const appendTag = (text: string) => {
    // Повторное нажатие не должно плодить одинаковые фрагменты: проверяем по
    // всему тексту, а не по последней части.
    if (notes.toLowerCase().includes(text.toLowerCase())) {
      pushToast("okk", `«${text}» уже записано`);
      return;
    }
    patch({ cpu_notes: notes ? `${notes}, ${text}` : text });
  };

  return (
    <div className="page tight set-page" ref={rootRef}>
      <div className="page-head">
        <h1>Настройки</h1>
        <div className="actions">
          <button
            type="button"
            className="folder-btn"
            disabled={!dataDir}
            title={dataDir || "Каталог ещё не известен"}
            onClick={() => {
              if (dataDir) commands.openFolder(dataDir).catch((e) => pushToast("err", String(e)));
            }}
          >
            <FolderIcon />
            Папка результатов
          </button>
          <button
            type="button"
            className="folder-btn"
            disabled={!cfgDir}
            title={cfgDir || "Каталог ещё не известен"}
            onClick={() => {
              if (cfgDir) commands.openFolder(cfgDir).catch((e) => pushToast("err", String(e)));
            }}
          >
            <FolderIcon />
            Папка настроек
          </button>
        </div>
      </div>

      <section className="set-sec">
        <div className="section-head">
          <h2 className="section-title">Внешний вид</h2>
        </div>
        <div className="set-group">
          <div className="set-row">
            <div className="set-left">
              <span className="set-icon">
                <ThemeIcon />
              </span>
              <div className="set-text">
                <div className="set-title">Тема оформления</div>
                <div className="set-sub">Цветовая палитра окна и генерируемых отчётов</div>
              </div>
            </div>
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

          <div className="set-row">
            <div className="set-left">
              <span className="set-icon">
                <MotionIcon />
              </span>
              <div className="set-text">
                <div className="set-title">Уменьшить движение</div>
                <div className="set-sub">Отключить плавные переходы и анимации интерфейса</div>
              </div>
            </div>
            <Switch checked={st.reduce_motion} onChange={(v) => patch({ reduce_motion: v })} />
          </div>

          {/* Плотность и масштаб текста независимы: «крупный текст +
              компактная плотность» — валидная комбинация (ТЗ II.4). */}
          <div className="set-row">
            <div className="set-left">
              <span className="set-icon">
                <GridIcon />
              </span>
              <div className="set-text">
                <div className="set-title">Плотность интерфейса</div>
                <div className="set-sub">
                  Отступы и высота контролов. На 720 px высоты окна экономит место
                </div>
              </div>
            </div>
            <Seg
              label="Плотность интерфейса"
              options={[
                { value: "compact", label: "Компактно" },
                { value: "normal", label: "Обычно" },
                { value: "roomy", label: "Свободно" },
              ]}
              value={isDensity(st.density) ? st.density : "normal"}
              onChange={(v) => patch({ density: v })}
            />
          </div>

          <div className="set-row">
            <div className="set-left">
              <span className="set-icon">
                <ThemeIcon />
              </span>
              <div className="set-text">
                <div className="set-title">Масштаб текста</div>
                <div className="set-sub">Меняет только размер шрифта, не геометрию</div>
              </div>
            </div>
            <Seg
              label="Масштаб текста"
              options={[
                { value: "s", label: "Мелкий" },
                { value: "m", label: "Обычный" },
                { value: "l", label: "Крупный" },
              ]}
              value={isTextScale(st.text_scale) ? st.text_scale : "m"}
              onChange={(v) => patch({ text_scale: v })}
            />
          </div>
        </div>
      </section>

      <section className="set-sec">
        <div className="section-head">
          <h2 className="section-title">Состояние системы</h2>
        </div>
        <div className="set-status">
          <StatusCard
            icon={<ShieldIcon />}
            title="Права администратора"
            sub="Доступ к переключению схем питания"
            state={adm}
            okLabel="Активно"
            badLabel="Нет прав"
          />
          <StatusCard
            icon={<SocketIcon />}
            title="Питание от сети"
            sub="Без ограничений от батареи"
            state={ac}
            okLabel="Подключено"
            badLabel="От батареи"
          />
        </div>
      </section>

      <section className="set-sec">
        <div className="section-head">
          <h2 className="section-title">Железо и BIOS</h2>
        </div>
        <Glass className="bios-card">
          <div className="bios-top">
            <span className="bios-label">Разгон, андервольт и память</span>
            <span className="hint">Необязательно · добавляется в отчёт</span>
          </div>
          <textarea
            className="bios-input"
            value={st.cpu_notes}
            rows={2}
            maxLength={500}
            placeholder="Например: PBO +200 МГц, Curve Optimizer −30, SMT выкл, 2×16 ГБ DDR5-6000 CL30"
            onChange={(e) => patch({ cpu_notes: e.target.value })}
          />
          <div className="qt-row">
            <span className="qt-label">Быстрая вставка:</span>
            {QUICK_TAGS.map((t) => (
              <button
                key={t.text}
                type="button"
                className="qt-chip"
                disabled={notes.toLowerCase().includes(t.text.toLowerCase())}
                onClick={() => appendTag(t.text)}
              >
                + {t.label}
              </button>
            ))}
            {notes ? (
              <button
                type="button"
                className="qt-clear"
                onClick={() => patch({ cpu_notes: "" })}
              >
                Очистить
              </button>
            ) : null}
          </div>
        </Glass>
      </section>
    </div>
  );
}

/**
 * Карточка состояния: значок, название и метка справа.
 *
 * Пока проверка не вернулась, метка показывает многоточие без зелёного цвета:
 * иначе на секунду мелькало «Активно» и тут же менялось на «Нет прав».
 */
function StatusCard({
  icon,
  title,
  sub,
  state,
  okLabel,
  badLabel,
}: {
  icon: React.ReactNode;
  title: string;
  sub: string;
  state: boolean | null;
  okLabel: string;
  badLabel: string;
}) {
  const tone = state == null ? "wait" : state ? "ok" : "bad";
  return (
    <div className={`status-card is-${tone}`}>
      <div className="st-left">
        <span className="st-icon">{icon}</span>
        <div className="set-text">
          <div className="set-title">{title}</div>
          <div className="set-sub">{sub}</div>
        </div>
      </div>
      <span className="st-badge">{state == null ? "…" : state ? okLabel : badLabel}</span>
    </div>
  );
}
