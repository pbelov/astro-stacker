import { invoke, Channel } from "@tauri-apps/api/core";

import { type Roots, signature } from "./measurement.svelte";
import type { Keys } from "./i18n.svelte";

// Сложение живёт дольше вкладки, на которой его запустили, — и теперь обязано:
// результат показывается на отдельном шаге, то есть заведомо не там, где была
// нажата кнопка. Пока состояние жило в компоненте, уход со вкладки уничтожал
// канал и обещание, а прогон продолжал считать в никуда.
//
// Второе, что здесь живёт, — сам сложенный кадр на той стороне. Прогон его не
// пишет; он остаётся в памяти окна, и кнопки сохранения пишут из него. Пока
// файлы назывались до прогона, иначе было нельзя: программа отказывалась
// начинать, не зная, куда класть, потому что узнать в конце, что минуты работы
// ушли в никуда, — худший момент для такой новости.

export type Coverage = { filled: number; medianDepth: number; thinnest: number };

export type StackedFrame = {
  name: string;
  weight: number;
  brightness: number;
  trail: number;
  fwhm: number;
  shift: number;
};

export type StackResult = {
  frames: StackedFrame[];
  refused: { name: string; reason: string }[];
  effective: number;
  stacked: number;
  width: number;
  height: number;
  /** Сколько шёл прогон. Не путать с тремя следующими: те — про свет. */
  elapsed: number;
  shotSeconds: number;
  keptSeconds: number;
  effectiveSeconds: number;
  exposuresUnrecorded: number;
  coverage: Coverage[];
  rejected: [number, number] | null;
  heavyLosses: [string, number][];
  previewWidth: number;
  previewHeight: number;
  note: string;
};

export type Progress =
  | { stage: "master"; kind: string; done: number; total: number }
  | { stage: "frame"; done: number; total: number; name: string }
  | { stage: "aligning" }
  | { stage: "stacking"; pass: number; passes: number; done: number; total: number; name: string }
  | { stage: "finishing" };

export type Output = "fits" | "tiff" | "view";

/** Три вида результата, каждый со своим именем и своим расширением. */
export const OUTPUTS: { id: Output; label: Keys; suffix: string; ext: string }[] = [
  { id: "fits", label: "outFits", suffix: ".fits", ext: "fits" },
  { id: "tiff", label: "outTiff", suffix: ".tif", ext: "tif" },
  { id: "view", label: "outView", suffix: "-view.tif", ext: "tif" },
];

export type StackSettings = {
  sigma: number;
  maxStars: number;
  raw: boolean;
  sharpness: number;
  maxTrail: number | null;
  maxFwhm: number | null;
  maxShift: number | null;
  pixfrac: number;
  reject: boolean;
  kappa: number;
};

class Stacking {
  running = $state(false);
  progress = $state<Progress | null>(null);
  result = $state<StackResult | null>(null);
  error = $state("");
  /** Куда уже записан каждый вид, если записан. */
  written = $state<Partial<Record<Output, string>>>({});
  saving = $state<Output | null>(null);
  saveError = $state("");
  /** Набор, который сложен: результат от другого набора — не результат. */
  private stackedOf = $state("");
  /**
   * Раскрыты ли подробные таблицы на шаге результата.
   *
   * Вид, а не прогон, и жить бы ему в компоненте — но шаги нарисованы через
   * `{#if}`, так что уход на соседний шаг компонент уничтожает. Раскрытая
   * таблица схлопывалась бы каждый раз, когда человек отошёл посмотреть на
   * кадры и вернулся, то есть ровно тогда, когда он её и открыл.
   */
  opened = $state({ stacked: false, weights: false });

  /** Относится ли показанное к тому, что сейчас выбрано. */
  isOf(roots: Roots): boolean {
    return this.stackedOf === signature(roots);
  }

  async run(roots: Roots, settings: StackSettings): Promise<boolean> {
    if (this.running) return false;
    this.running = true;
    this.error = "";
    this.saveError = "";
    // Прежний результат уходит вместе с прогоном, который его сделал: та
    // сторона его тоже отпускает, и держать здесь имена файлов от него
    // значило бы показывать, будто новый стек уже где-то лежит.
    this.result = null;
    this.written = {};
    this.progress = null;
    this.stackedOf = signature(roots);
    const channel = new Channel<Progress>();
    channel.onmessage = (message) => (this.progress = message);
    try {
      this.result = await invoke<StackResult>("stack_run", {
        roots,
        ...settings,
        on: channel,
      });
      return true;
    } catch (thrown) {
      this.error = String(thrown);
      return false;
    } finally {
      this.running = false;
      this.progress = null;
    }
  }

  stop(): void {
    void invoke("cancel");
  }

  /**
   * Пишет один вид из того, что держит та сторона.
   *
   * По одному, а не всё сразу: каждый вид — это свой проход по всем пикселям,
   * и прогон, которому нужен только FITS, не должен платить за два TIFF.
   */
  async save(kind: Output, path: string): Promise<void> {
    if (this.saving !== null) return;
    this.saving = kind;
    this.saveError = "";
    try {
      const out = { fits: null, tiff: null, view: null, [kind]: path };
      const written = await invoke<string[]>("stack_write", { out });
      this.written = { ...this.written, [kind]: written[0] ?? path };
    } catch (thrown) {
      this.saveError = String(thrown);
    } finally {
      this.saving = null;
    }
  }
}

export const stacking = new Stacking();
