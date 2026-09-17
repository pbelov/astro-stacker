// Сколько ещё идти — из того, что уже прошло.
//
// Считается по текущей стадии и только по ней. Сложить стадии в одно число
// хочется, но кадр в них стоит по-разному и разница не в процентах: мастер
// собирается из кадров, которые держатся в памяти все сразу, а лайты читаются
// в несколько потоков. Оценка лайтов по темпу мастера была бы уверенной и
// неверной в разы, а уверенное неверное число хуже отсутствующего — по нему
// принимают решение уйти или ждать.
//
// Поэтому у каждой стадии своё «осталось», и на экране рядом стоит, чего оно
// касается.

/** Пока кадров меньше, темп - это не темп, а случайность первого кадра. */
const ENOUGH = 3;

export class Remaining {
  private origin: { key: string; at: number; done: number } | null = null;
  private left: number | null = null;

  /** Забыть всё: начался новый прогон. */
  reset(): void {
    this.origin = null;
    this.left = null;
  }

  /**
   * Очередное сообщение о ходе дела.
   *
   * `key` различает стадии, и мастеров разных видов в том числе: у каждого свой
   * счёт с нуля, и сквозной отсчёт принял бы этот возврат к нулю за время,
   * потраченное впустую.
   */
  saw(key: string, done: number, total: number, now = Date.now()): void {
    if (!this.origin || this.origin.key !== key || done < this.origin.done) {
      // Начало отсчёта - первое сообщение стадии, а не её старт: между ними
      // лежит открытие файлов, которое к темпу чтения отношения не имеет.
      this.origin = { key, at: now, done };
      this.left = null;
      return;
    }
    const passed = done - this.origin.done;
    if (passed < ENOUGH) return;
    const perFrame = (now - this.origin.at) / 1000 / passed;
    this.left = Math.max(0, total - done) * perFrame;
  }

  /** Секунды до конца текущей стадии, или null, пока сказать нечего. */
  get seconds(): number | null {
    return this.left;
  }
}

export type Rough = { value: number; unit: "seconds" | "minutes" | "hours" };

/**
 * Огрубление, а не округление.
 *
 * Точность оценки - десятки процентов, и показывать её посекундно значит и
 * врать о точности, и мельтешить: число, пересчитываемое на каждом кадре, не
 * успевают прочитать. Огрублённое меняется только на переходе через шаг, и
 * само по себе говорит, насколько ему верить.
 */
export function roughly(seconds: number): Rough {
  if (seconds < 45) return { value: Math.max(5, Math.round(seconds / 5) * 5), unit: "seconds" };
  if (seconds < 90 * 60) return { value: Math.max(1, Math.round(seconds / 60)), unit: "minutes" };
  return { value: Math.round(seconds / 360) / 10, unit: "hours" };
}
