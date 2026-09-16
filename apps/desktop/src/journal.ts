import { error } from "@tauri-apps/plugin-log";

// Ошибки окна пишутся туда же, куда пишет Rust.
//
// Иначе их не видит никто: упавший скрипт или отклонённый промис остаются в
// консоли webview, которой у собранного приложения нет. Для человека это
// выглядит как «нажал и ничего не произошло», и после перезапуска не остаётся
// вообще ничего.
//
// Подписка ставится до монтирования: ошибка при самом монтировании — ровно тот
// случай, когда окно пустое и сказать больше нечему.

/**
 * Записать, ничего не подняв в ответ.
 *
 * Сам вызов логгера — это обращение к бэкенду, и оно может не получиться. Если
 * дать ему отклониться, это придёт сюда же обработчиком `unhandledrejection`,
 * который снова вызовет логгер: одна неудача превращается в бесконечный цикл.
 * Логгер, усиливающий поломку, хуже отсутствующего.
 */
function write(line: string): void {
  void error(line).catch(() => {});
}

/** Стек, если он есть: без него остаётся только текст сообщения. */
function detail(value: unknown): string {
  if (value instanceof Error) return `${value.name}: ${value.message}\n${value.stack ?? ""}`;
  return String(value);
}

export function catchWindowErrors(): void {
  window.addEventListener("error", (event) => {
    // Промах по ресурсу приходит сюда же, но у него нет `error`, и путать его
    // с исключением не надо: это разные поломки.
    const what = event.error ? detail(event.error) : `${event.message} (${event.filename}:${event.lineno})`;
    write(`window: ${what}`);
  });

  window.addEventListener("unhandledrejection", (event) => {
    write(`unhandled rejection: ${detail(event.reason)}`);
  });
}
