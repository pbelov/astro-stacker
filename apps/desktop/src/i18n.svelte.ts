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
  appName: "AstroAccretion",

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
  browse: "Папки…",
  browseFiles: "Файлы…",
  framesFilter: "Кадры",
  clear: "Очистить",
  pathsChosen: "{n} {n:путь|пути|путей}",

  scan: "Прочитать сессию",
  scanning: "Читаю…",
  rescan: "Прочитать заново",
  nothingToScan: "Укажи хотя бы одну папку или файл с кадрами",

  summary: "Сессия",
  framesRead: "{n} {n:кадр|кадра|кадров} за {s} с",
  setsFound: "Наборов: {n}",
  plansFound: "Готовых к сложению: {n}",

  sets: "Наборы",
  setLine: "{kind}, {n} {n:кадр|кадра|кадров}",
  excludedOf: "{active} из {total} активны",
  exposure: "выдержка",
  iso: "ISO",
  seconds: "с",

  plan: "План сложения",
  planLights: "Лайты: набор {set}, {n} {n:кадр|кадра|кадров}",
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
  timeLeft: "осталось ~{n} {unit}",
  measureOn: "Продолжить",
  stoppedEarly: "Остановлено: {n:измерен|измерено|измерено} {n} {n:кадр|кадра|кадров}, это часть прогона, а не весь он.",

  runAsWhole: "Прогон целиком",
  statFwhm: "Ширина",
  statTrail: "Смаз",
  statStars: "Звёзд",
  starsCapped: "{n} {n:кадр упёрся|кадра упёрлись|кадров упёрлись} в потолок {cap}, так что это нижние оценки",
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

  stackRun: "Сложить",
  stackAgain: "Сложить заново",
  settings: "Настройки",
  sharpness: "Резкость",
  sharpnessLeft: "0 — глубина: оптимум для туманностей и всего крупнее звёзд",
  sharpnessRight: "1 — резкость: оптимум для точечных источников",
  limitTrail: "Порог по смазу, px",
  limitFwhm: "Порог по ширине, px",
  limitShift: "Порог по сдвигу, px",
  limitsHint: "Пусто — порога нет, кадры идут с весом по качеству. Порог по сдвигу отсекает кадры фазы наведения: каждый оставленный расширяет холст под себя.",
  outFiles: "Куда сохранить",
  outFits: "FITS, измерение",
  outTiff: "TIFF, линейный",
  outView: "TIFF, для просмотра",
  outNotSet: "не сохранять",
  outNothingNamed: "Назови хотя бы один файл, иначе складывать не во что",
  choose: "Выбрать…",

  aligning: "Совмещаю кадры…",
  stackingFrame: "Складываю {done} из {total} — {name}",
  writing: "Записываю…",
  stackSlow: "Четыре прохода по кадрам, два из них с декодированием. Прогон в пару сотен кадров — это минуты.",

  theResult: "Результат",
  noResultYet: "Здесь появится изображение, когда сложение пройдёт.",
  whatWasStacked: "Что сложилось",
  statFrames: "Кадров",
  effectiveDepth: "по глубине это {n} {n:кадр|кадра|кадров} медианного качества",
  statCanvas: "Холст",
  statTook: "Заняло",
  coverage: "Покрытие по плоскостям",
  depthAt: "{median} {median:кадр|кадра|кадров} вглубь по медиане, {thin} в самой тонкой десятой",
  written: "Записано",
  refused: "Отклонены",
  lightestWeights: "С наименьшим весом",
  lightestHint: "Не отброшены — просто вносят меньше всех. Если что-то тут выглядит неожиданно, стоит посмотреть на сам кадр.",
  colWeight: "Вес",
  colBrightness: "Яркость",
  colShift: "Сдвиг",

  reject: "Отбраковывать выбросы",
  rejectHint: "Убирает спутники, самолёты и следы частиц. Стоит ещё одного прохода по кадрам с декодированием, поэтому по умолчанию выключено.",
  kappa: "Порог, сигм",
  rejectedShare: "Отброшено {share}% отсчётов ({dropped} из {considered})",
  heavyLosses: "Потеряли необычно много — это не кадры в спутниках, а признак того, что порог не подходит прогону:",
  lostShare: "потерял {share}% своих отсчётов",
  passOf: "проход {pass} из {passes}",

  suspicions: "На что стоит посмотреть",
  sMinority: "Набор {set}: {a} {a:кадр|кадра|кадров} с другими настройками съёмки против {b} в основной серии — похоже на пробные снимки",
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
  // Короткие — для кнопки в подвале панели, где на подпись треть её ширины.
  // Полные остаются подсказкой при наведении: в ряду с языком и справкой сама
  // позиция говорит, что это тема, а вот какая именно — нет.
  themeSystemShort: "Авто",
  themeDarkShort: "Тёмная",
  themeLightShort: "Светлая",
  language: "Язык",
  about: "О программе",
  aboutText:
    "Складывает астрокадры: читает сессию, находит звёзды, измеряет смаз, регистрирует и складывает.",
  close: "Закрыть",
  version: "Версия {v}",
  formatsRead: "Читаются: {list}",
  // Отладочная кнопка, и названа одинаково в обоих языках: уйдёт целиком.
  logs: "Logs",
  aboutLicense: "Приложение — MIT или Apache-2.0.",
  aboutNotices: "Сторонние компоненты",
  saveAs: "Сохранить как…",

  error: "Ошибка",
  hours: "ч",
  minutes: "мин",
  days: "дн",
} as const;

/** Имя строки. Экспортируется, чтобы список ключей проверялся, а не приводился. */
export type Keys = keyof typeof ru;

const en: Record<Keys, string> = {
  appName: "AstroAccretion",

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
  browse: "Folders…",
  browseFiles: "Files…",
  framesFilter: "Frames",
  clear: "Clear",
  pathsChosen: "{n} {n:path|paths}",

  scan: "Read the session",
  scanning: "Reading…",
  rescan: "Read again",
  nothingToScan: "Point it at least at one folder or file of frames",

  summary: "Session",
  framesRead: "{n} {n:frame|frames} in {s} s",
  setsFound: "Sets: {n}",
  plansFound: "Ready to stack: {n}",

  sets: "Sets",
  setLine: "{kind}, {n} {n:frame|frames}",
  excludedOf: "{active} of {total} active",
  exposure: "exposure",
  iso: "ISO",
  seconds: "s",

  plan: "Stacking plan",
  planLights: "Lights: set {set}, {n} {n:frame|frames}",
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
  timeLeft: "~{n} {unit} left",
  measureOn: "Carry on",
  stoppedEarly: "Stopped: {n} {n:frame|frames} measured, which is part of a run and not the whole of one.",

  runAsWhole: "The run as a whole",
  statFwhm: "Width",
  statTrail: "Trail",
  statStars: "Stars",
  starsCapped: "{n} {n:frame|frames} hit the {cap} ceiling, so these are floors",
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

  stackRun: "Stack",
  stackAgain: "Stack again",
  settings: "Settings",
  sharpness: "Sharpness",
  sharpnessLeft: "0 — depth: optimal for nebulosity and anything larger than a star",
  sharpnessRight: "1 — sharpness: optimal for point sources",
  limitTrail: "Trail limit, px",
  limitFwhm: "Width limit, px",
  limitShift: "Shift limit, px",
  limitsHint: "Empty means no limit: frames contribute by weight. The shift limit is what removes the framing shots — every one kept widens the canvas to cover it.",
  outFiles: "Where to save",
  outFits: "FITS, the measurement",
  outTiff: "TIFF, linear",
  outView: "TIFF, to look at",
  outNotSet: "not saved",
  outNothingNamed: "Name at least one file, or there is nowhere for the result to go",
  choose: "Choose…",

  aligning: "Aligning the frames…",
  stackingFrame: "Stacking {done} of {total} — {name}",
  writing: "Writing…",
  stackSlow: "Four passes over the frames, two of them decoding. A run of a couple of hundred takes minutes.",

  theResult: "The result",
  noResultYet: "The image appears here once a stack has run.",
  whatWasStacked: "What was stacked",
  statFrames: "Frames",
  effectiveDepth: "worth {n} {n:frame|frames} of median quality",
  statCanvas: "Canvas",
  statTook: "Took",
  coverage: "Coverage per plane",
  depthAt: "{median} {median:frame|frames} deep at the median, {thin} at the thinnest tenth",
  written: "Written",
  refused: "Refused",
  lightestWeights: "Lightest weights",
  lightestHint: "Not dropped, just contributing least. If something here looks unexpected, the frame itself is worth a look.",
  colWeight: "Weight",
  colBrightness: "Brightness",
  colShift: "Shift",

  reject: "Reject outliers",
  rejectHint: "Removes satellites, aeroplanes and particle hits. Costs another decoding pass over the frames, so it is off by default.",
  kappa: "Threshold, sigma",
  rejectedShare: "Rejected {share}% of samples ({dropped} of {considered})",
  heavyLosses: "Lost an unusual share — not frames full of satellites, but a sign the threshold does not fit the run:",
  lostShare: "lost {share}% of its samples",
  passOf: "pass {pass} of {passes}",

  suspicions: "Worth a look",
  sMinority: "Set {set}: {a} {a:frame|frames} shot at other settings against {b} in the main series — looks like test shots",
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
  themeSystemShort: "Auto",
  themeDarkShort: "Dark",
  themeLightShort: "Light",
  language: "Language",
  about: "About",
  aboutText:
    "Stacks astrophotographs: reads a session, finds the stars, measures the trailing, registers and combines.",
  close: "Close",
  version: "Version {v}",
  formatsRead: "Reads: {list}",
  logs: "Logs",
  aboutLicense: "The app is MIT or Apache-2.0.",
  aboutNotices: "Third-party components",
  saveAs: "Save as…",

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

/**
 * Какую из форм слова требует число.
 *
 * Русский различает три: на 1 (кроме 11), на 2–4 (кроме 12–14) и остальные.
 * Дробное число, как «4,4», согласуется со второй — «4,4 кадра», — и так же
 * число, которое не удалось прочитать: из трёх это наименее странная.
 */
function agree(locale: Locale, n: number, forms: string[]): string {
  const pick = (i: number) => forms[Math.min(i, forms.length - 1)];
  if (locale !== "ru") return pick(n === 1 ? 0 : 1);
  if (!Number.isInteger(n)) return pick(1);
  const [ten, hundred] = [Math.abs(n) % 10, Math.abs(n) % 100];
  if (ten === 1 && hundred !== 11) return pick(0);
  if (ten >= 2 && ten <= 4 && (hundred < 12 || hundred > 14)) return pick(1);
  return pick(2);
}

class I18n {
  locale = $state<Locale>(initial());

  set(next: Locale) {
    this.locale = next;
    localStorage.setItem("locale", next);
    document.documentElement.lang = next;
  }

  /**
   * Одна строка с подстановками.
   *
   * `{name}` — значение как есть. `{name:форма|форма|форма}` — слово,
   * согласованное с числом `name`: три формы для русского (один кадр, два
   * кадра, пять кадров), две для английского. Согласование сделано здесь, а не
   * в каждой строке, потому что «4 кадров» не опечатка одной строки — это то,
   * что даёт любая строка, где число стоит перед словом без него.
   */
  t(key: Keys, values: Record<string, string | number> = {}): string {
    const template = TABLES[this.locale][key] ?? TABLES.en[key] ?? key;
    return template
      .replace(/\{(\w+):([^}]+)\}/g, (whole, name: string, forms: string) =>
        name in values ? agree(this.locale, Number(values[name]), forms.split("|")) : whole,
      )
      .replace(/\{(\w+)\}/g, (whole, name: string) =>
        name in values ? String(values[name]) : whole,
      );
  }
}

export const i18n = new I18n();
