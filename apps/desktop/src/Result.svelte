<script lang="ts">
  // Шаг «Результат»: что получилось и что из этого стоит оставить.
  //
  // Отдельный шаг, а не хвост сложения, по той же причине, по которой файлы
  // называются теперь после прогона, а не до него: пока имена спрашивались
  // заранее, человек решал, что сохранить, не видя, что получилось.

  import { invoke } from "@tauri-apps/api/core";
  import { save } from "@tauri-apps/plugin-dialog";

  import { i18n } from "./i18n.svelte";
  import { folderOf, joined, lastFolder, remember, shortened } from "./places";
  import { OUTPUTS, stacking, type StackResult } from "./stacking.svelte";

  let { trim }: { trim: (value: number, decimals?: number) => string } = $props();

  let canvas = $state<HTMLCanvasElement | null>(null);
  const result = $derived(stacking.result);

  // Рисуется при каждом появлении холста и при каждой смене результата: шаг
  // можно покинуть и вернуться, и тогда холст новый, а результат прежний.
  $effect(() => {
    const produced = result;
    if (produced && canvas) void draw(produced, canvas);
  });

  /**
   * Превью приходит сырыми байтами RGBA отдельной командой: та же картинка
   * массивом чисел в JSON вышла бы на порядок больше и разбиралась бы по байту.
   */
  async function draw(produced: StackResult, target: HTMLCanvasElement) {
    const bytes = await invoke<ArrayBuffer>("stack_preview");
    target.width = produced.previewWidth;
    target.height = produced.previewHeight;
    const context = target.getContext("2d");
    if (!context) return;
    const data = new Uint8ClampedArray(bytes);
    if (data.length < produced.previewWidth * produced.previewHeight * 4) return;
    context.putImageData(new ImageData(data, produced.previewWidth, produced.previewHeight), 0, 0);
  }

  /**
   * Накопленный свет так, как его произносят вслух: «3 ч 20 мин», а не 12000 с.
   *
   * Это первое, что говорят про ночь, и число в секундах требует от читателя
   * деления в уме ровно в тот момент, когда он хочет сравнить два вечера.
   */
  function light(seconds: number): string {
    if (!Number.isFinite(seconds) || seconds <= 0) return "—";
    const hours = Math.floor(seconds / 3600);
    const minutes = Math.round((seconds - hours * 3600) / 60);
    if (hours > 0) return i18n.t("hoursMinutes", { h: hours, m: minutes });
    if (minutes > 0) return i18n.t("minutesOnly", { m: minutes });
    return i18n.t("secondsOnly", { s: Math.round(seconds) });
  }

  /** Папка и имя, от которых начинается диалог сохранения. */
  function startsFrom(): { folder: string; stem: string } {
    for (const spec of OUTPUTS) {
      const already = stacking.written[spec.id];
      if (already) {
        const name = already.split(/[\\/]/).pop() ?? "";
        return {
          folder: folderOf(already),
          stem: name.replace(/\.[^.]*$/, "").replace(/-view$/, ""),
        };
      }
    }
    return { folder: lastFolder("result") ?? "", stem: "stack" };
  }

  async function keep(spec: (typeof OUTPUTS)[number]) {
    const { folder, stem } = startsFrom();
    const picked = await save({
      title: i18n.t(spec.label),
      defaultPath: joined(folder, stem + spec.suffix),
      filters: [{ name: spec.ext.toUpperCase(), extensions: [spec.ext] }],
    });
    if (typeof picked !== "string") return;
    remember("result", folderOf(picked));
    await stacking.save(spec.id, picked);
  }

  const number = (value: number, decimals = 2) =>
    Number.isFinite(value) ? trim(value, decimals) : "—";

  /** Что видно, пока раздел свёрнут: иначе сворачивание прячет и ответ тоже. */
  const summary = $derived(
    result
      ? i18n.t("stackedSummary", {
          light: light(result.keptSeconds),
          frames: result.stacked,
        })
      : "",
  );

  const worst = $derived(
    result ? [...result.frames].sort((a, b) => a.weight - b.weight).slice(0, 8) : [],
  );
</script>

<header class="bar">
  <h1>{i18n.t("stepResult")}</h1>
</header>

{#if !result}
  <section class="card">
    <p class="dim">{i18n.t("noResultYet")}</p>
  </section>
{:else}
  <section class="card preview">
    <canvas bind:this={canvas}></canvas>
    <p class="dim">{result.note}</p>
  </section>

  <!-- Сохранение — первым после картинки: посмотрел и решил. -->
  <section class="card">
    <h2>{i18n.t("keepIt")}</h2>
    <p class="muted">{i18n.t("keepHint")}</p>
    <div class="keeps">
      {#each OUTPUTS as spec (spec.id)}
        <div class="keep">
          <button
            class="primary"
            disabled={stacking.saving !== null}
            onclick={() => void keep(spec)}
          >
            {stacking.saving === spec.id ? i18n.t("saving") : i18n.t(spec.label)}
          </button>
          {#if stacking.written[spec.id]}
            <span class="path num" title={stacking.written[spec.id]}>
              {shortened(stacking.written[spec.id] ?? "")}
            </span>
          {:else}
            <span class="path dim">{i18n.t("outNotSet")}</span>
          {/if}
        </div>
      {/each}
    </div>
    {#if stacking.saveError}
      <p class="warning">{stacking.saveError}</p>
    {/if}
  </section>

  <details class="card fold" bind:open={stacking.opened.stacked}>
    <summary><h2>{i18n.t("whatWasStacked")}</h2><span class="dim">{summary}</span></summary>
    <table class="num stats">
      <tbody>
        <tr>
          <td class="dim">{i18n.t("statLight")}</td>
          <td><strong>{light(result.keptSeconds)}</strong></td>
          <td class="muted">
            {i18n.t("lightBreakdown", {
              shot: light(result.shotSeconds),
              effective: light(result.effectiveSeconds),
            })}
          </td>
        </tr>
        <tr>
          <td class="dim">{i18n.t("statFrames")}</td>
          <td><strong>{result.stacked}</strong></td>
          <td class="muted">{i18n.t("effectiveDepth", { n: result.effective })}</td>
        </tr>
        <tr>
          <td class="dim">{i18n.t("statCanvas")}</td>
          <td class="num">{result.width} × {result.height}</td>
          <td class="muted">{trim((result.width * result.height) / 1e6, 1)} Mpx</td>
        </tr>
        <tr>
          <td class="dim">{i18n.t("statTook")}</td>
          <td>{trim(result.elapsed, 1)} {i18n.t("seconds")}</td>
          <td></td>
        </tr>
      </tbody>
    </table>
    {#if result.exposuresUnrecorded > 0}
      <p class="warning">
        {i18n.t("exposureMissing", { n: result.exposuresUnrecorded })}
      </p>
    {/if}

    <!-- По плоскостям, а не одной цифрой: зелёных фотосайтов вдвое больше, и
         средняя по всем трём польстила бы результату. Красная показывает,
         хватило ли дизеринга. -->
    <h3>{i18n.t("coverage")}</h3>
    <p class="muted">{i18n.t("coverageHint")}</p>
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
  </details>

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

  <details class="card fold" bind:open={stacking.opened.weights}>
    <summary>
      <h2>{i18n.t("lightestWeights")}</h2>
      <span class="dim">{i18n.t("lightestHint")}</span>
    </summary>
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
  </details>
{/if}

<style>
  /* Раздел сворачивается, и свёрнутым остаётся строкой: две подробные таблицы
     раскрытыми выталкивали за край окна то, ради чего на этот шаг пришли —
     картинку и кнопки сохранения. */
  .fold summary {
    display: flex;
    align-items: baseline;
    gap: 12px;
    cursor: pointer;
    list-style: none;
  }
  .fold summary::-webkit-details-marker {
    display: none;
  }
  .fold summary h2 {
    margin: 0;
    /* Заголовок не переносится: подпись рядом бывает длинной, и разорванное
       надвое название раздела читается как две строки, а не как одна. */
    white-space: nowrap;
    flex: none;
  }
  /* Своя стрелка, а не браузерная: та стоит вплотную к тексту и в разных
     сборках WebView2 рисуется по-разному. */
  .fold summary h2::before {
    content: "▸";
    display: inline-block;
    width: 1em;
    color: var(--dim);
    transition: transform 0.12s ease;
  }
  .fold[open] summary h2::before {
    transform: rotate(90deg);
  }
  .fold summary span {
    font-size: 12px;
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .fold[open] summary {
    margin-bottom: 10px;
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

  /* Три кнопки одной ширины, путь рядом с каждой: что сохранено и куда видно
     не заглядывая в проводник. */
  .keeps {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-top: 10px;
  }
  .keep {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .keep button {
    width: 15em;
    flex: none;
  }
  .path {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
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
