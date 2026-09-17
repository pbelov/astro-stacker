import { invoke, Channel } from "@tauri-apps/api/core";

// Замер живёт дольше вкладки, на которой он показан.
//
// Раньше он жил внутри компонента, а шаги отрисованы через `{#if}`, так что
// шаг назад к кадрам компонент уничтожал — вместе с каналом и обещанием от
// `measure_quality`. Сам проход при этом продолжал считать, докладывать ему
// было некуда, и вернувшись человек видел пустой шаг, как будто ничего и не
// запускал. А причина шагнуть назад обычно в том, чтобы посмотреть на то, что
// замер только что показал.
//
// Поэтому состояние прогона здесь, а не в компоненте: компонент — это вид на
// него, и его можно уничтожать и создавать сколько угодно.

export type FrameQuality = {
  name: string;
  stars: number;
  fwhm: number;
  trail: number;
  angle: number;
  agreement: number;
  sky: number;
  noise: number;
  saturated: number;
  oversized: number;
  seconds: number;
};

export type Quality = {
  frames: FrameQuality[];
  failed: { name: string; reason: string }[];
  seconds: number;
  stopped: boolean;
  /** Осталось ли что продолжать. Не то же, что `stopped`: проход доводит уже
   *  начатые кадры, и остановка под конец оставляет прогон целым. */
  continuable: boolean;
  direction: number;
  directionAgreement: number;
  starCap: number;
};

export type Progress =
  | { stage: "master"; kind: string; done: number; total: number }
  | { stage: "frame"; done: number; total: number; name: string };

export type Roots = Record<string, string[]>;

/** Чем набор кадров отличается от другого набора. */
const signature = (roots: Roots) => JSON.stringify(roots);

class Measurement {
  running = $state(false);
  progress = $state<Progress | null>(null);
  result = $state<Quality | null>(null);
  error = $state("");
  /** null = порог не задан, кадры взвешиваются, ничего не выбрасывается. */
  limit = $state<number | null>(null);
  /** Набор, который был измерен: результат от другого набора — не результат. */
  private measured = $state("");

  /**
   * Относится ли то, что показано, к тому, что сейчас выбрано.
   *
   * Пока замер жил в компоненте, устаревший результат был невозможен: уход со
   * вкладки его стирал. Раз он теперь сохраняется, это надо проверять явно —
   * иначе сохранение прогона куплено ценой показа чужих чисел.
   */
  isOf(roots: Roots): boolean {
    return this.measured === signature(roots);
  }

  /**
   * Можно ли продолжить: прогон был остановлен, и с тех пор ничего не менялось.
   *
   * Отвечает на это та сторона: она держит то, из чего продолжать, и знает,
   * осталось ли что. Здесь сверяется только набор кадров - если он сменился,
   * показанное относится к другому вопросу.
   */
  canContinue(roots: Roots): boolean {
    return !this.running && (this.result?.continuable ?? false) && this.isOf(roots);
  }

  async start(
    roots: Roots,
    sigma: number,
    maxStars: number,
    raw: boolean,
    resume = false,
  ): Promise<void> {
    // Второй замер поверх первого — не то, чего кто-либо хочет, и раньше это
    // было возможно: флаг жил в компоненте, а компонентов за прогон могло
    // смениться несколько.
    if (this.running) return;
    this.running = true;
    this.error = "";
    // Продолжение возвращает прогон целиком, вместе с уже измеренным, поэтому
    // старое стирается в обоих случаях — но только тогда, когда новое придёт
    // на его место.
    this.result = null;
    this.progress = null;
    this.measured = signature(roots);
    const channel = new Channel<Progress>();
    channel.onmessage = (message) => (this.progress = message);
    try {
      this.result = await invoke<Quality>("measure_quality", {
        roots,
        sigma,
        maxStars,
        raw,
        resume,
        on: channel,
      });
      this.limit = null;
    } catch (thrown) {
      this.error = String(thrown);
    } finally {
      this.running = false;
      this.progress = null;
    }
  }

  stop(): void {
    void invoke("cancel");
  }
}

export const measurement = new Measurement();
