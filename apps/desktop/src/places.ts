// Где в прошлый раз брали кадры каждого вида.
//
// По видам отдельно, потому что лежат они отдельно: лайты в одной папке, дарки
// в соседней, флэты бывают и от другой ночи. Одна общая «последняя папка»
// промахивается тем чаще, чем аккуратнее разложены файлы, — а разложены они у
// того, кто снимает помногу, всегда.
//
// Первый выбор вида, о котором ещё ничего не известно, начинается там, где
// брали что-нибудь последним: в начале сессии это соседняя папка той же ночи, и
// это заметно ближе к цели, чем то, где диалог оставила система.

const KEY = "places";

type Place = { path: string; at: number };

/**
 * Чтение и запись не падают.
 *
 * localStorage бросает сам по себе - приватное окно, запрет на хранение данных
 * сайта, - и удобство, роняющее выбор папки, это не удобство.
 */
function read(): Record<string, Place> {
  try {
    const raw = localStorage.getItem(KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : {};
    return parsed && typeof parsed === "object" ? (parsed as Record<string, Place>) : {};
  } catch {
    return {};
  }
}

/** Папка, в которой лежит путь. Разделитель тут может быть любой из двух. */
export function folderOf(path: string): string {
  const at = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return at > 0 ? path.slice(0, at) : "";
}

/** Запомнить папку, из которой брали кадры этого вида. */
export function remember(feed: string, folder: string): void {
  if (!folder) return;
  try {
    const places = read();
    places[feed] = { path: folder, at: Date.now() };
    localStorage.setItem(KEY, JSON.stringify(places));
  } catch {
    // Не запомнилось - в следующий раз просто начнём оттуда же, откуда сейчас.
  }
}

/** Папка, где брали кадры этого вида; иначе та, где брали что-нибудь последним. */
export function lastFolder(feed: string): string | undefined {
  const places = read();
  const own = places[feed]?.path;
  if (own) return own;
  const any = Object.values(places).sort((a, b) => b.at - a.at)[0];
  return any?.path;
}

/**
 * Где открыть диалог выбора папки.
 *
 * На уровень выше запомненного: выбирают папку, а полезно видеть те, из которых
 * выбирают. Открытое внутри прошлой папки показывает её содержимое - то есть
 * кадры, - и чтобы взять соседнюю, надо сначала подняться.
 */
export function startFolderPick(feed: string): string | undefined {
  const last = lastFolder(feed);
  if (!last) return undefined;
  return folderOf(last) || last;
}

/**
 * Где открыть диалог выбора файлов.
 *
 * В самой запомненной папке: выбирают кадры, а они лежат в ней.
 */
export function startFilePick(feed: string): string | undefined {
  return lastFolder(feed);
}
