<script lang="ts">
  // Шаг «Сложение»: настройки и прогон. Результат — на своём шаге.
  //
  // Пороги здесь те же, что на шаге «Качество», и это намеренно: там они
  // выбираются, глядя на цену, здесь применяются. Одно и то же число в двух
  // местах с разными именами — верный способ потом не понять, какое из них
  // сработало.
  //
  // Файлы больше не называются до прогона. Прогон оставляет сложенный кадр в
  // памяти, и шаг «Результат» пишет из него то, что человек решит оставить,
  // посмотрев на картинку.

  import { i18n } from "./i18n.svelte";
  import { stacking } from "./stacking.svelte";

  let {
    roots,
    sigma = 5,
    maxStars = 1000,
    raw = false,
    kindName,
    trim,
    onFinished,
  }: {
    roots: Record<string, string[]>;
    sigma?: number;
    maxStars?: number;
    raw?: boolean;
    kindName: (kind: string) => string;
    trim: (value: number, decimals?: number) => string;
    onFinished: () => void;
  } = $props();

  let sharpness = $state(0.5);
  let maxTrail = $state<number | null>(null);
  let maxFwhm = $state<number | null>(null);
  let maxShift = $state<number | null>(null);
  let pixfrac = $state(1);
  let reject = $state(false);
  let kappa = $state(3);

  const running = $derived(stacking.running);
  const progress = $derived(stacking.progress);
  const error = $derived(stacking.error);
  const done = $derived(stacking.result !== null && stacking.isOf(roots));
  const ready = $derived(Object.values(roots).some((paths) => paths.length > 0) && !running);

  async function run() {
    if (!ready) return;
    const finished = await stacking.run(roots, {
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
    });
    // Ушло минуты работы: показать результат надо самому, а не ждать, пока
    // человек догадается перейти на соседний шаг.
    if (finished) onFinished();
  }

  const share = $derived.by(() => {
    if (progress === null) return 0;
    if (progress.stage === "aligning" || progress.stage === "finishing") return 1;
    return progress.total > 0 ? progress.done / progress.total : 0;
  });
</script>

<header class="bar">
  <h1>{i18n.t("stepStack")}</h1>
  {#if running}
    <button class="ghost" onclick={() => stacking.stop()}>{i18n.t("stop")}</button>
  {:else}
    <button class="primary" disabled={!ready} onclick={run}>
      {done ? i18n.t("stackAgain") : i18n.t("stackRun")}
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
        {i18n.t("finishing")}
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

{#if done && !running}
  <section class="card">
    <p class="muted">{i18n.t("resultWaiting")}</p>
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
</style>
