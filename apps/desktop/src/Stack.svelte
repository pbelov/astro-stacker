<script lang="ts">
  // Шаг «Сложение»: настройки, прогон и результат.
  //
  // Пороги здесь те же, что на шаге «Качество», и это намеренно: там они
  // выбираются, глядя на цену, здесь применяются. Одно и то же число в двух
  // местах с разными именами — верный способ потом не понять, какое из них
  // сработало.

  import { invoke, Channel } from "@tauri-apps/api/core";
  import { open } from "@tauri-apps/plugin-dialog";
  import { i18n } from "./i18n.svelte";

  type Coverage = { filled: number; medianDepth: number; thinnest: number };
  type StackedFrame = {
    name: string;
    weight: number;
    brightness: number;
    trail: number;
    fwhm: number;
    shift: number;
  };
  type Result = {
    frames: StackedFrame[];
    refused: { name: string; reason: string }[];
    effective: number;
    stacked: number;
    width: number;
    height: number;
    seconds: number;
    coverage: Coverage[];
    written: string[];
    rejected: [number, number] | null;
    heavyLosses: [string, number][];
    previewWidth: number;
    previewHeight: number;
    note: string;
  };
  type Progress =
    | { stage: "master"; kind: string; done: number; total: number }
    | { stage: "frame"; done: number; total: number; name: string }
    | { stage: "aligning" }
    | { stage: "stacking"; pass: number; passes: number; done: number; total: number; name: string }
    | { stage: "writing" };

  let {
    roots,
    sigma = 5,
    maxStars = 1000,
    raw = false,
    kindName,
    trim,
  }: {
    roots: Record<string, string[]>;
    sigma?: number;
    maxStars?: number;
    raw?: boolean;
    kindName: (kind: string) => string;
    trim: (value: number, decimals?: number) => string;
  } = $props();

  let sharpness = $state(0.5);
  let maxTrail = $state<number | null>(null);
  let maxFwhm = $state<number | null>(null);
  let maxShift = $state<number | null>(null);
  let pixfrac = $state(1);
  let reject = $state(false);
  let kappa = $state(3);
  let out = $state("");

  let running = $state(false);
  let progress = $state<Progress | null>(null);
  let result = $state<Result | null>(null);
  let error = $state("");
  let canvas = $state<HTMLCanvasElement | null>(null);

  const ready = $derived(out.length > 0 && !running);

  async function chooseOut() {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked === "string") out = picked;
  }

  async function run() {
    if (!ready) return;
    running = true;
    error = "";
    result = null;
    progress = null;
    const channel = new Channel<Progress>();
    channel.onmessage = (message) => (progress = message);
    try {
      const produced = await invoke<Result>("stack_run", {
        roots,
        sigma,
        maxStars,
        raw,
        sharpness,
        maxTrail,
        maxFwhm,
        maxShift,
        pixfrac,
        reject,
        kappa,
        out,
        on: channel,
      });
      result = produced;
      await draw(produced);
    } catch (thrown) {
      error = String(thrown);
    } finally {
      running = false;
      progress = null;
    }
  }

  function stop() {
    void invoke("cancel");
  }

  /**
   * Превью приходит сырыми байтами RGBA отдельной командой: та же картинка в
   * виде массива чисел в JSON вышла бы на порядок больше и разбиралась бы по
   * байту.
   */
  async function draw(produced: Result) {
    const bytes = await invoke<ArrayBuffer>("stack_preview");
    const target = canvas;
    if (!target) return;
    target.width = produced.previewWidth;
    target.height = produced.previewHeight;
    const context = target.getContext("2d");
    if (!context) return;
    const data = new Uint8ClampedArray(bytes);
    if (data.length < produced.previewWidth * produced.previewHeight * 4) return;
    context.putImageData(
      new ImageData(data, produced.previewWidth, produced.previewHeight),
      0,
      0,
    );
  }

  const number = (value: number, decimals = 2) =>
    Number.isFinite(value) ? trim(value, decimals) : "—";

  const share = $derived.by(() => {
    if (progress === null) return 0;
    if (progress.stage === "aligning" || progress.stage === "writing") return 1;
    return progress.total > 0 ? progress.done / progress.total : 0;
  });

  const worst = $derived(
    result ? [...result.frames].sort((a, b) => a.weight - b.weight).slice(0, 8) : [],
  );
</script>

<header class="bar">
  <h1>{i18n.t("stepStack")}</h1>
  {#if running}
    <button class="ghost" onclick={stop}>{i18n.t("stop")}</button>
  {:else}
    <button class="primary" disabled={!ready} onclick={run}>
      {result ? i18n.t("stackAgain") : i18n.t("stackRun")}
    </button>
  {/if}
</header>

<section class="card">
  <h2>{i18n.t("settings")}</h2>

  <div class="row">
    <label for="sharp">{i18n.t("sharpness")}</label>
    <input
      id="sharp"
      type="range"
      min="0"
      max="1"
      step="0.05"
      value={sharpness}
      oninput={(e) => (sharpness = Number(e.currentTarget.value))}
    />
    <span class="readout num">{trim(sharpness, 2)}</span>
  </div>
  <!-- Оба конца точные, а не подобранные: слева оптимум для протяжённого,
       справа для точечных источников. -->
  <p class="dim ends">
    <span>{i18n.t("sharpnessLeft")}</span>
    <span>{i18n.t("sharpnessRight")}</span>
  </p>

  <div class="limits">
    {#each [{ key: "maxTrail", label: "limitTrail", get: () => maxTrail, set: (v: number | null) => (maxTrail = v) }, { key: "maxFwhm", label: "limitFwhm", get: () => maxFwhm, set: (v: number | null) => (maxFwhm = v) }, { key: "maxShift", label: "limitShift", get: () => maxShift, set: (v: number | null) => (maxShift = v) }] as limit (limit.key)}
      <label class="limit">
        <span>{i18n.t(limit.label as never)}</span>
        <input
          type="number"
          min="0"
          step="0.1"
          placeholder={i18n.t("noLimit")}
          value={limit.get() ?? ""}
          oninput={(e) => {
            const raw = e.currentTarget.value.trim();
            limit.set(raw === "" ? null : Number(raw));
          }}
        />
      </label>
    {/each}
  </div>
  <p class="dim">{i18n.t("limitsHint")}</p>

  <div class="row reject">
    <label class="check">
      <input type="checkbox" checked={reject} onchange={(e) => (reject = e.currentTarget.checked)} />
      <span>{i18n.t("reject")}</span>
    </label>
    {#if reject}
      <label class="kappa">
        <span>{i18n.t("kappa")}</span>
        <input
          type="number"
          min="1"
          max="10"
          step="0.5"
          value={kappa}
          oninput={(e) => (kappa = Number(e.currentTarget.value))}
        />
      </label>
    {/if}
  </div>
  <p class="dim">{i18n.t("rejectHint")}</p>

  <div class="row out">
    <label for="out">{i18n.t("outFolder")}</label>
    <span class="path num" class:muted={!out}>{out || i18n.t("outNotSet")}</span>
    <button class="ghost" onclick={chooseOut}>{i18n.t("browse")}</button>
  </div>
</section>

{#if running}
  <section class="card">
    <div class="track"><div class="fill" style:width={`${share * 100}%`}></div></div>
    <p class="muted num">
      {#if progress === null}
        {i18n.t("starting")}
      {:else if progress.stage === "master"}
        {i18n.t("buildingMaster", {
          kind: kindName(progress.kind),
          done: progress.done,
          total: progress.total,
        })}
      {:else if progress.stage === "frame"}
        {i18n.t("measuringFrame", {
          done: progress.done,
          total: progress.total,
          name: progress.name,
        })}
      {:else if progress.stage === "aligning"}
        {i18n.t("aligning")}
      {:else if progress.stage === "stacking"}
        {i18n.t("stackingFrame", {
          done: progress.done,
          total: progress.total,
          name: progress.name,
        })}{#if progress.passes > 1}
          · {i18n.t("passOf", { pass: progress.pass, passes: progress.passes })}
        {/if}
      {:else}
        {i18n.t("writing")}
      {/if}
    </p>
    <p class="dim">{i18n.t("stackSlow")}</p>
  </section>
{/if}

{#if error}
  <section class="card err">
    <h2>{i18n.t("error")}</h2>
    <p>{error}</p>
  </section>
{/if}

<section class="card preview" class:empty={!result}>
  <h2>{i18n.t("theResult")}</h2>
  <canvas bind:this={canvas}></canvas>
  {#if !result}
    <p class="dim">{i18n.t("noResultYet")}</p>
  {:else}
    <p class="dim">{result.note}</p>
  {/if}
</section>

{#if result}
  <section class="card">
    <h2>{i18n.t("whatWasStacked")}</h2>
    <table class="num stats">
      <tbody>
        <tr>
          <td class="dim">{i18n.t("statFrames")}</td>
          <td><strong>{result.stacked}</strong></td>
          <td class="muted">
            {i18n.t("effectiveDepth", { n: result.effective })}
          </td>
        </tr>
        <tr>
          <td class="dim">{i18n.t("statCanvas")}</td>
          <td class="num">{result.width} × {result.height}</td>
          <td class="muted">{trim((result.width * result.height) / 1e6, 1)} Mpx</td>
        </tr>
        <tr>
          <td class="dim">{i18n.t("statTook")}</td>
          <td>{trim(result.seconds, 1)} {i18n.t("seconds")}</td>
          <td></td>
        </tr>
      </tbody>
    </table>

    <!-- По плоскостям, а не одной цифрой: зелёных фотосайтов вдвое больше, и
         средняя по всем трём польстила бы результату. Красная показывает,
         хватило ли дизеринга. -->
    <h3>{i18n.t("coverage")}</h3>
    <table class="num stats">
      <tbody>
        {#each result.coverage as plane, index (index)}
          <tr>
            <td class="dim">{["R", "G", "B"][index] ?? index}</td>
            <td>{trim(plane.filled, 1)}%</td>
            <td class="muted">
              {i18n.t("depthAt", {
                median: trim(plane.medianDepth, 1),
                thin: trim(plane.thinnest, 1),
              })}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>

    {#if result.rejected}
      <h3>{i18n.t("refused")}</h3>
      <p class="num">
        {i18n.t("rejectedShare", {
          share: trim((result.rejected[0] / Math.max(1, result.rejected[1])) * 100, 3),
          dropped: result.rejected[0],
          considered: result.rejected[1],
        })}
      </p>
      {#if result.heavyLosses.length > 0}
        <p class="warning">{i18n.t("heavyLosses")}</p>
        <ul class="plain">
          {#each result.heavyLosses as [name, share] (name)}
            <li class="warning">
              <span class="dim">{name}</span>
              {i18n.t("lostShare", { share: trim(share * 100, 1) })}
            </li>
          {/each}
        </ul>
      {/if}
    {/if}

    <h3>{i18n.t("written")}</h3>
    <ul class="plain">
      {#each result.written as path (path)}
        <li class="dim">{path}</li>
      {/each}
    </ul>
  </section>

  {#if result.refused.length > 0}
    <section class="card">
      <h2>{i18n.t("refused")} ({result.refused.length})</h2>
      <ul class="plain scroll">
        {#each result.refused as frame (frame.name)}
          <li><span class="dim">{frame.name}</span> — {frame.reason}</li>
        {/each}
      </ul>
    </section>
  {/if}

  <section class="card">
    <h2>{i18n.t("lightestWeights")}</h2>
    <p class="muted">{i18n.t("lightestHint")}</p>
    <table class="num frames">
      <thead>
        <tr>
          <th>{i18n.t("colFrame")}</th>
          <th>{i18n.t("colWeight")}</th>
          <th>{i18n.t("colBrightness")}</th>
          <th>{i18n.t("colTrail")}</th>
          <th>{i18n.t("colFwhm")}</th>
          <th>{i18n.t("colShift")}</th>
        </tr>
      </thead>
      <tbody>
        {#each worst as frame (frame.name)}
          <tr>
            <td class="name">{frame.name}</td>
            <td><strong>{number(frame.weight, 3)}</strong></td>
            <td class="muted">{number(frame.brightness, 3)}</td>
            <td class="muted">{number(frame.trail)}</td>
            <td class="muted">{number(frame.fwhm)}</td>
            <td class="muted">{number(frame.shift, 0)}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </section>
{/if}

<style>
  .row {
    display: flex;
    align-items: center;
    gap: 14px;
    margin-bottom: 4px;
  }
  .row label {
    min-width: 9em;
    color: var(--muted);
  }
  input[type="range"] {
    flex: 1;
    accent-color: var(--accent);
  }
  .readout {
    min-width: 3em;
    text-align: right;
    font-weight: 600;
  }
  .ends {
    display: flex;
    justify-content: space-between;
    margin: 0 0 14px 9em;
    font-size: 12px;
  }

  .limits {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
    gap: 12px;
    margin-bottom: 6px;
  }
  .limit {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
    color: var(--muted);
  }
  input[type="number"] {
    font: inherit;
    padding: 5px 8px;
    border-radius: 7px;
    border: 1px solid var(--line);
    background: var(--chipbg);
    color: var(--text);
  }

  .check,
  .kappa {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--muted);
    font-size: 13px;
  }
  .kappa input {
    width: 5em;
  }
  .reject {
    margin-top: 12px;
  }

  .out {
    margin-top: 14px;
    padding-top: 12px;
    border-top: 1px solid var(--line);
  }
  .path {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
  }

  .track {
    height: 6px;
    border-radius: 3px;
    background: var(--chipbg);
    overflow: hidden;
    margin-bottom: 8px;
  }
  .fill {
    height: 100%;
    background: var(--accent);
    transition: width 0.15s linear;
  }

  .preview canvas {
    display: block;
    width: 100%;
    height: auto;
    border-radius: var(--radius);
    background: var(--photo-bg);
    /* Пиксели превью крупные; сглаживание браузера тут честнее, чем ступеньки. */
    image-rendering: auto;
  }
  .preview.empty canvas {
    min-height: 160px;
  }

  .stats td {
    padding: 2px 14px 2px 0;
    border: none;
  }
  .stats td:first-child {
    width: 8em;
  }

  .frames {
    width: 100%;
  }
  .frames th {
    text-align: left;
    font-weight: 600;
    color: var(--dim);
    font-size: 12px;
    padding: 0 10px 4px 0;
  }
  .frames td {
    padding: 2px 10px 2px 0;
    border-bottom: 1px solid var(--line);
  }
  .frames .name {
    color: var(--text);
  }

  .scroll {
    max-height: 190px;
    overflow-y: auto;
  }
</style>
