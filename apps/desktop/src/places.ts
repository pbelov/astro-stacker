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

/**
 * Путь в одном написании, для показа.
 *
 * Хранится он ровно таким, каким его дала система: его потом открывают. А на
 * экран три пути подряд должны выходить одинаковыми, иначе строки одного рода
 * выглядят пришедшими из разных программ. Сборка — единственная в Windows,
 * так что написание — обратный слэш.
 */
export function shown(path: string): string {
  return path.replace(/\//g, "\\");
}

/**
 * Папка и имя в один путь, разделителем той самой папки.
 *
 * Не жёсткой косой чертой: папка приходит от системного диалога с обратными
 * слэшами, и приклеенная к ней косая давала путь из двух написаний сразу.
 */
export function joined(folder: string, name: string): string {
  if (!folder) return name;
  const separator = folder.includes("\\") || !folder.includes("/") ? "\\" : "/";
  return folder + separator + name;
}

/**
 * Укороченный путь для узкой строки.
 *
 * Режется по разделителю, а не по числу знаков: обрезанный посередине имени
 * папки путь читается как испорченный текст, а выброшенные целиком папки — как
 * путь. Хвост сохраняется целиком, потому что имя файла и есть то, что ищут
 * глазами.
 */
export function shortened(path: string, keep = 46): string {
  const full = shown(path);
  if (full.length <= keep) return full;
  const parts = full.split("\\");
  let tail = parts[parts.length - 1] ?? full;
  for (let index = parts.length - 2; index > 0; index -= 1) {
    const wider = parts[index] + "\\" + tail;
    if (wider.length + 1 > keep) break;
    tail = wider;
  }
  return "…\\" + tail;
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
