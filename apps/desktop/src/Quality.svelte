<script lang="ts">
  // Шаг «Качество»: измеряет каждый лайт и показывает, что мешало.
  //
  // Ползунок здесь — не украшение. Порог по смазу выбирается вслепую, если не
  // видно, сколько кадров он унесёт: 4 px звучит строго, а на этом прогоне это
  // половина ночи. Поэтому цена считается на лету, а таблица сразу помечает,
  // что выпадет.

  import { invoke, Channel } from "@tauri-apps/api/core";
  import { i18n } from "./i18n.svelte";

  type FrameQuality = {
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
  type Quality = {
    frames: FrameQuality[];
    failed: { name: string; reason: string }[];
    seconds: number;
    stopped: boolean;
    direction: number;
    directionAgreement: number;
    starCap: number;
  };
  type Progress =
    | { stage: "master"; kind: string; done: number; total: number }
    | { stage: "frame"; done: number; total: number; name: string };

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

  let running = $state(false);
  let progress = $state<Progress | null>(null);
  let quality = $state<Quality | null>(null);
  let error = $state("");
  /** null = порог не задан, кадры взвешиваются, ничего не выбрасывается. */
  let limit = $state<number | null>(null);

  const measured = $derived(quality?.frames ?? []);
  const shaped = $derived(measured.filter((f) => Number.isFinite(f.trail)));
  const ranked = $derived([...shaped].sort((a, b) => b.trail - a.trail));
  const unshaped = $derived(measured.filter((f) => !Number.isFinite(f.trail)));

  const trails = $derived([...shaped.map((f) => f.trail)].sort((a, b) => a - b));
  const fwhms = $derived([...shaped.map((f) => f.fwhm)].sort((a, b) => a - b));
  const median = (values: number[]) =>
    values.length ? values[Math.floor((values.length - 1) / 2)] : NaN;

  const kept = $derived.by(() => {
    const at = limit;
    return at === null ? shaped.length : shaped.filter((f) => f.trail <= at).length;
  });
  const capped = $derived(measured.filter((f) => f.stars >= (quality?.starCap ?? Infinity)).length);

  // Три отметки на шкале, из самих данных: где порог оставит 90, 75 и 50%.
  const marks = $derived(
    trails.length
      ? [0.9, 0.75, 0.5].map((share) => ({
          share,
          value: trails[Math.floor((trails.length - 1) * share)],
        }))
      : [],
  );

  async function measure() {
    if (running) return;
    running = true;
    error = "";
    quality = null;
    progress = null;
    const channel = new Channel<Progress>();
    channel.onmessage = (message) => (progress = message);
    try {
      quality = await invoke<Quality>("measure_quality", {
        roots,
        sigma,
        maxStars,
        raw,
        on: channel,
      });
      limit = null;
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

  const share = $derived(
    progress === null
      ? 0
      : progress.total > 0
        ? progress.done / progress.total
        : 0,
  );

  const number = (value: number, decimals = 2) =>
    Number.isFinite(value) ? trim(value, decimals) : "—";
  const angle = (value: number) => (Number.isFinite(value) ? `${Math.round(value)}°` : "—");
</script>

<header class="bar">
  <h1>{i18n.t("stepQuality")}</h1>
  {#if running}
    <button class="ghost" onclick={stop}>{i18n.t("stop")}</button>
  {:else}
    <button class="primary" onclick={measure}>
      {quality ? i18n.t("measureAgain") : i18n.t("measure")}
    </button>
  {/if}
</header>

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
      {:else}
        {i18n.t("measuringFrame", {
          done: progress.done,
          total: progress.total,
          name: progress.name,
        })}
      {/if}
    </p>
    <p class="dim">{i18n.t("measureSlow")}</p>
  </section>
{/if}

{#if error}
  <section class="card err">
    <h2>{i18n.t("error")}</h2>
    <p>{error}</p>
  </section>
{/if}

{#if quality && shaped.length > 0}
  <section class="card">
    <h2>{i18n.t("runAsWhole")}</h2>
    {#if quality.stopped}
      <p class="warning">{i18n.t("stoppedEarly", { n: measured.length })}</p>
    {/if}
    <table class="num stats">
      <tbody>
        <tr>
          <td class="dim">{i18n.t("statFwhm")}</td>
          <td><strong>{number(median(fwhms))}</strong> px</td>
          <td class="muted">{number(fwhms[0])} – {number(fwhms[fwhms.length - 1])}</td>
        </tr>
        <tr>
          <td class="dim">{i18n.t("statTrail")}</td>
          <td><strong>{number(median(trails))}</strong> px</td>
          <td class="muted">{number(trails[0])} – {number(trails[trails.length - 1])}</td>
        </tr>
        <tr>
          <td class="dim">{i18n.t("statStars")}</td>
          <td>
            <strong>{Math.round(shaped.reduce((s, f) => s + f.stars, 0) / shaped.length)}</strong>
          </td>
          <td class="muted">
            {#if capped > 0}{i18n.t("starsCapped", { n: capped, cap: quality.starCap })}{/if}
          </td>
        </tr>
      </tbody>
    </table>

    <!-- Вывод, а не число: одно направление во всех кадрах — это скорость
         ведения, разные — ветер или толчки, и лечится это по-разному. -->
    <p class="verdict">
      {i18n.t("directionIs", {
        deg: angle(quality.direction),
        agree: number(quality.directionAgreement),
      })}
      <span class:ok={quality.directionAgreement > 0.8} class:warning={quality.directionAgreement <= 0.4}>
        {quality.directionAgreement > 0.8
          ? i18n.t("directionTracking")
          : quality.directionAgreement > 0.4
            ? i18n.t("directionMixed")
            : i18n.t("directionNone")}
      </span>
    </p>
  </section>

  <section class="card">
    <h2>{i18n.t("trailLimit")}</h2>
    <div class="slider">
      <input
        type="range"
        min={0}
        max={Math.max(1, trails[trails.length - 1])}
        step={0.05}
        value={limit ?? trails[trails.length - 1]}
        oninput={(e) => (limit = Number(e.currentTarget.value))}
      />
      <div class="readout num">
        {#if limit === null}
          <strong>{i18n.t("noLimit")}</strong>
          <span class="muted">{i18n.t("weightsDoTheWork")}</span>
        {:else}
          <strong>{trim(limit, 2)} px</strong>
          <span class="muted">
            {i18n.t("keepsOf", {
              kept,
              total: shaped.length,
              percent: Math.round((kept / shaped.length) * 100),
            })}
          </span>
        {/if}
      </div>
      {#if limit !== null}
        <button class="ghost" onclick={() => (limit = null)}>{i18n.t("clearLimit")}</button>
      {/if}
    </div>
    <ul class="marks num">
      {#each marks as mark (mark.share)}
        <li>
          <button class="link" onclick={() => (limit = mark.value)}>
            {i18n.t("markKeeps", {
              percent: Math.round(mark.share * 100),
              value: trim(mark.value, 2),
            })}
          </button>
        </li>
      {/each}
    </ul>
    <p class="dim">{i18n.t("limitCost")}</p>
  </section>

  <section class="card">
    <h2>{i18n.t("perFrame")} ({shaped.length})</h2>
    <div class="scroll">
      <table class="num frames">
        <thead>
          <tr>
            <th>{i18n.t("colFrame")}</th>
            <th>{i18n.t("colTrail")}</th>
            <th>{i18n.t("colFwhm")}</th>
            <th>{i18n.t("colAngle")}</th>
            <th>{i18n.t("colAgree")}</th>
            <th>{i18n.t("colStars")}</th>
            <th>{i18n.t("colNoise")}</th>
          </tr>
        </thead>
        <tbody>
          {#each ranked as frame (frame.name)}
            <tr class:dropped={limit !== null && frame.trail > limit}>
              <td class="name">{frame.name}</td>
              <td><strong>{number(frame.trail)}</strong></td>
              <td>{number(frame.fwhm)}</td>
              <td class="muted">{angle(frame.angle)}</td>
              <td class="muted">{number(frame.agreement)}</td>
              <td class="muted">{frame.stars}</td>
              <td class="muted">{number(frame.noise, 1)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  </section>
{/if}

{#if quality && unshaped.length > 0}
  <section class="card">
    <h2>{i18n.t("noShape")} ({unshaped.length})</h2>
    <p class="muted">{i18n.t("noShapeHint")}</p>
    <ul class="plain">
      {#each unshaped as frame (frame.name)}
        <li><span class="dim">{frame.name}</span> — {i18n.t("starsFound", { n: frame.stars })}</li>
      {/each}
    </ul>
  </section>
{/if}

{#if quality && quality.failed.length > 0}
  <section class="card">
    <h2>{i18n.t("rejected")} ({quality.failed.length})</h2>
    <ul class="plain scroll">
      {#each quality.failed as file (file.name)}
        <li><span class="dim">{file.name}</span> — {file.reason}</li>
      {/each}
    </ul>
  </section>
{/if}

{#if quality && measured.length === 0}
  <p class="empty">{i18n.t("nothingMeasured")}</p>
{/if}

<style>
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

  .stats td {
    padding: 2px 14px 2px 0;
    border: none;
  }
  .stats td:first-child {
    width: 8em;
  }

  .verdict {
    margin: 10px 0 0;
    padding-top: 8px;
    border-top: 1px solid var(--line);
  }
  .verdict .ok {
    color: var(--ok);
  }

  .slider {
    display: flex;
    align-items: center;
    gap: 14px;
  }
  input[type="range"] {
    flex: 1;
    accent-color: var(--accent);
  }
  .readout {
    min-width: 16em;
    display: flex;
    flex-direction: column;
  }

  .marks {
    display: flex;
    gap: 16px;
    margin: 10px 0 4px;
    padding: 0;
    list-style: none;
    font-size: 12px;
  }
  .link {
    font: inherit;
    background: none;
    border: none;
    padding: 0;
    color: var(--muted);
    text-decoration: underline dotted;
    cursor: pointer;
  }
  .link:hover {
    color: var(--accent);
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
    position: sticky;
    top: 0;
    background: var(--surface);
  }
  .frames td {
    padding: 2px 10px 2px 0;
    border-bottom: 1px solid var(--line);
  }
  .frames .name {
    color: var(--text);
  }
  /* Выпадающие кадры не прячутся: видно, что именно уходит, а не только
     сколько. */
  .frames tr.dropped td {
    opacity: 0.35;
    text-decoration: line-through;
  }

  .scroll {
    max-height: 420px;
    overflow-y: auto;
  }
</style>
