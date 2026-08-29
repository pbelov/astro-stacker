// Локализация: компактный реактивный словарь на рунах Svelte 5, той же формы,
// что в star-trails и video-editing-tool.
//
// Здесь два языка, а не четыре: испанский и немецкий в соседних проектах
// написаны, и добавить их сюда — механическая работа по готовым ключам. Пустой
// перевод хуже отсутствующего языка: пользователь выбирает язык и получает
// наполовину английское окно.

export type Locale = "ru" | "en";

export const LOCALES: { id: Locale; label: string }[] = [
  { id: "ru", label: "Русский" },
  { id: "en", label: "English" },
];

const ru = {
  appName: "AstroStacker",

  stepFrames: "Кадры",
  stepQuality: "Качество",
  stepStack: "Сложение",
  soon: "скоро",

  roleLights: "Лайты",
  roleDarks: "Дарки",
  roleFlats: "Флэты",
  roleBiases: "Биасы",
  roleDarkFlats: "Дарк-флэты",

  dropHere: "Перетащи папку или файлы",
  browse: "Выбрать…",
  clear: "Очистить",
  pathsChosen: "{n} путей",
  pathChosen: "1 путь",

  scan: "Прочитать сессию",
  scanning: "Читаю…",
  rescan: "Прочитать заново",
  nothingToScan: "Укажи хотя бы одну папку с кадрами",

  summary: "Сессия",
  framesRead: "{n} кадров за {s} с",
  setsFound: "Наборов: {n}",
  plansFound: "Готовых к сложению: {n}",

  sets: "Наборы",
  setLine: "{kind}, {n} кадров",
  excludedOf: "{active} из {total} активны",
  exposure: "выдержка",
  iso: "ISO",
  seconds: "с",

  plan: "План сложения",
  planLights: "Лайты: набор {set}, {n} кадров",
  calibration: "Калибровка",
  noCalibration: "Калибровка не подобрана",
  quality: "совпадение",
  qualityExact: "точное",
  qualityOnly: "единственный кандидат",
  qualityExposure: "ближайшая выдержка",
  qualityGain: "ближайшее ISO",
  alternatives: "ещё кандидатов: {n}",
  blocked: "Не подошло: {kind}, кандидатов {n}",
  rDimensions: "Размер кадра: {a} против {b}",
  rCfa: "Цветовой фильтр: {a} против {b}",
  rBody: "Другая камера: {a} против {b}",
  rGain: "ISO: {a} против {b}",
  rDepth: "Разрядность: {a} против {b}",
  rScale: "Другая шкала значений: {a} против {b}",
  rUnrecorded: "Нечего сравнить: параметр не записан",

  mismatches: "Расхождения",
  mExposure: "Выдержка: {a} против {b}",
  mGain: "ISO: {a} против {b}",
  mTemperature: "Температура сенсора: {a} против {b}",
  mBlack: "Уровень чёрного: {a} против {b}",
  mWhite: "Уровень белого: {a} против {b}",
  mOrientation: "Ориентация: {a} против {b}",
  mBody: "Камера: {a} против {b}",
  mLens: "Объектив: {a} против {b}",
  mOptics: "Оптика: {a} против {b}",
  mElapsed: "Снято с разницей {a}",
  mUnrecorded: "Не записано, поэтому сравнить не с чем",

  measure: "Измерить качество",
  measureAgain: "Измерить заново",
  stop: "Остановить",
  starting: "Готовлюсь…",
  buildingMaster: "Мастер-{kind}: {done} из {total}",
  measuringFrame: "Кадр {done} из {total} — {name}",
  measureSlow: "Каждый кадр декодируется, калибруется и просматривается на звёзды. Считай примерно полсекунды на кадр — прогон в пару сотен займёт минуты.",
  stoppedEarly: "Остановлено: измерено {n} кадров, это часть прогона, а не весь он.",

  runAsWhole: "Прогон целиком",
  statFwhm: "Ширина",
  statTrail: "Смаз",
  statStars: "Звёзд",
  starsCapped: "{n} кадров упёрлись в потолок {cap}, так что это нижние оценки",
  directionIs: "Смаз направлен на {deg}, согласие {agree} —",
  directionTracking: "одно направление во всех кадрах: это скорость ведения, а не ветер.",
  directionMixed: "в основном одно направление, но часть кадров ушла в сторону.",
  directionNone: "общего направления нет, значит вытянутость не от хода монтировки.",

  trailLimit: "Порог по смазу",
  noLimit: "Порог не задан",
  weightsDoTheWork: "ничего не отбрасывается, кадры идут с весом по качеству",
  keepsOf: "оставит {kept} из {total} ({percent}%)",
  clearLimit: "Убрать порог",
  markKeeps: "{percent}% — {value} px",
  limitCost: "Отбросив половину кадров, теряешь примерно в 1.4 раза по шуму. Ползунок показывает цену до того, как она уплачена.",

  perFrame: "По кадрам",
  colFrame: "Кадр",
  colTrail: "Смаз, px",
  colFwhm: "Ширина, px",
  colAngle: "Угол",
  colAgree: "Согласие",
  colStars: "Звёзд",
  colNoise: "Шум",

  noShape: "Форма не измерена",
  noShapeHint: "Кадры прочитались, но звёзд для оценки формы не набралось. В статистику прогона они не входят.",
  starsFound: "звёзд найдено: {n}",
  nothingMeasured: "Не удалось измерить ни одного кадра",

  suspicions: "На что стоит посмотреть",
  sMinority: "Набор {set}: {a} кадров против {b} в основной серии — похоже на пробы",
  sBiasNotShortest: "Набор {set}: биасы по {a} с, а самая короткая выдержка {b} с",
  sFlatNeedsDarkFlats: "Набор {set}: флэты по {a} с — стоит снять дарк-флэты",
  sInCameraDark: "Набор {set}: интервал {a} с при выдержке {b} с — похоже на внутрикамерное вычитание тёмного",
  sSpansNights: "Набор {set}: {kind} сняты с разбросом {a}",

  unassigned: "Не отнесены ни к чему",
  unassignedHint: "Кадры прочитаны, но ни одно правило их не забрало. Они не участвуют в сложении.",
  rejected: "Не прочитаны",

  theme: "Тема",
  themeSystem: "Тема: системная",
  themeDark: "Тема: тёмная",
  themeLight: "Тема: светлая",
  language: "Язык",
  about: "О программе",
  aboutText:
    "Складывает астрокадры: читает сессию, находит звёзды, измеряет смаз, регистрирует и складывает. Форматы кадров — подключаемые плагины.",
  close: "Закрыть",
  version: "Версия {v}",
  formatsRead: "Читаются: {list}",

  error: "Ошибка",
  hours: "ч",
  minutes: "мин",
  days: "дн",
} as const;

type Keys = keyof typeof ru;

const en: Record<Keys, string> = {
  appName: "AstroStacker",

  stepFrames: "Frames",
  stepQuality: "Quality",
  stepStack: "Stack",
  soon: "soon",

  roleLights: "Lights",
  roleDarks: "Darks",
  roleFlats: "Flats",
  roleBiases: "Biases",
  roleDarkFlats: "Dark flats",

  dropHere: "Drop a folder or files here",
  browse: "Browse…",
  clear: "Clear",
  pathsChosen: "{n} paths",
  pathChosen: "1 path",

  scan: "Read the session",
  scanning: "Reading…",
  rescan: "Read again",
  nothingToScan: "Point it at least at one folder of frames",

  summary: "Session",
  framesRead: "{n} frames in {s} s",
  setsFound: "Sets: {n}",
  plansFound: "Ready to stack: {n}",

  sets: "Sets",
  setLine: "{kind}, {n} frames",
  excludedOf: "{active} of {total} active",
  exposure: "exposure",
  iso: "ISO",
  seconds: "s",

  plan: "Stacking plan",
  planLights: "Lights: set {set}, {n} frames",
  calibration: "Calibration",
  noCalibration: "No calibration matched",
  quality: "match",
  qualityExact: "exact",
  qualityOnly: "only candidate",
  qualityExposure: "closest exposure",
  qualityGain: "closest ISO",
  alternatives: "{n} more candidates",
  blocked: "Refused: {kind}, {n} candidates",
  rDimensions: "Frame size: {a} against {b}",
  rCfa: "Colour filter: {a} against {b}",
  rBody: "Different camera: {a} against {b}",
  rGain: "ISO: {a} against {b}",
  rDepth: "Bit depth: {a} against {b}",
  rScale: "Different value scale: {a} against {b}",
  rUnrecorded: "Nothing to compare: the property was not recorded",

  mismatches: "Differences",
  mExposure: "Exposure: {a} against {b}",
  mGain: "ISO: {a} against {b}",
  mTemperature: "Sensor temperature: {a} against {b}",
  mBlack: "Black level: {a} against {b}",
  mWhite: "White level: {a} against {b}",
  mOrientation: "Orientation: {a} against {b}",
  mBody: "Camera: {a} against {b}",
  mLens: "Lens: {a} against {b}",
  mOptics: "Optics: {a} against {b}",
  mElapsed: "Shot {a} apart",
  mUnrecorded: "Not recorded, so there is nothing to compare",

  measure: "Measure quality",
  measureAgain: "Measure again",
  stop: "Stop",
  starting: "Getting ready…",
  buildingMaster: "Master {kind}: {done} of {total}",
  measuringFrame: "Frame {done} of {total} — {name}",
  measureSlow: "Every frame is decoded, calibrated and searched for stars. Reckon on about half a second each, so a run of a couple of hundred takes minutes.",
  stoppedEarly: "Stopped: {n} frames measured, which is part of a run and not the whole of one.",

  runAsWhole: "The run as a whole",
  statFwhm: "Width",
  statTrail: "Trail",
  statStars: "Stars",
  starsCapped: "{n} frames hit the {cap} ceiling, so these are floors",
  directionIs: "Trailing points at {deg}, agreement {agree} —",
  directionTracking: "one direction in every frame: a tracking rate, not the wind.",
  directionMixed: "mostly one direction, with frames that wandered.",
  directionNone: "no shared direction, so the elongation is not the mount rate.",

  trailLimit: "Trail limit",
  noLimit: "No limit set",
  weightsDoTheWork: "nothing is dropped; frames contribute by weight",
  keepsOf: "keeps {kept} of {total} ({percent}%)",
  clearLimit: "Clear the limit",
  markKeeps: "{percent}% — {value} px",
  limitCost: "Dropping half the frames costs about 1.4 times in noise. The slider shows the price before it is paid.",

  perFrame: "Frame by frame",
  colFrame: "Frame",
  colTrail: "Trail, px",
  colFwhm: "Width, px",
  colAngle: "Angle",
  colAgree: "Agreement",
  colStars: "Stars",
  colNoise: "Noise",

  noShape: "No shape measured",
  noShapeHint: "Read, but too few stars to measure a shape from. They take no part in the run statistics.",
  starsFound: "{n} stars found",
  nothingMeasured: "Not one frame could be measured",

  suspicions: "Worth a look",
  sMinority: "Set {set}: {a} frames against {b} in the main series — looks like tests",
  sBiasNotShortest: "Set {set}: biases at {a} s, but the shortest exposure is {b} s",
  sFlatNeedsDarkFlats: "Set {set}: flats at {a} s — worth shooting dark flats",
  sInCameraDark:
    "Set {set}: {a} s between frames at {b} s exposure — looks like in-camera dark subtraction",
  sSpansNights: "Set {set}: {kind} shot {a} apart",

  unassigned: "Not claimed by anything",
  unassignedHint:
    "Read, but no rule took them. They take no part in the stack.",
  rejected: "Not read",

  theme: "Theme",
  themeSystem: "Theme: system",
  themeDark: "Theme: dark",
  themeLight: "Theme: light",
  language: "Language",
  about: "About",
  aboutText:
    "Stacks astrophotographs: reads a session, finds the stars, measures the trailing, registers and combines. Frame formats arrive as loadable plugins.",
  close: "Close",
  version: "Version {v}",
  formatsRead: "Reads: {list}",

  error: "Error",
  hours: "h",
  minutes: "min",
  days: "d",
};

const TABLES: Record<Locale, Record<Keys, string>> = { ru, en };

function initial(): Locale {
  const saved = localStorage.getItem("locale");
  if (saved === "ru" || saved === "en") return saved;
  return navigator.language.startsWith("ru") ? "ru" : "en";
}

class I18n {
  locale = $state<Locale>(initial());

  set(next: Locale) {
    this.locale = next;
    localStorage.setItem("locale", next);
    document.documentElement.lang = next;
  }

  /** Один ключ со подстановками вида {name}. */
  t(key: Keys, values: Record<string, string | number> = {}): string {
    const template = TABLES[this.locale][key] ?? TABLES.en[key] ?? key;
    return template.replace(/\{(\w+)\}/g, (whole, name: string) =>
      name in values ? String(values[name]) : whole,
    );
  }
}

export const i18n = new I18n();
