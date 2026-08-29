<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { open } from "@tauri-apps/plugin-dialog";
  import { onDestroy, onMount } from "svelte";

  import DropZone from "./ui/DropZone.svelte";
  import { i18n, LOCALES, type Locale } from "./i18n.svelte";

  type Step = "frames" | "quality" | "stack";
  const STEPS: { id: Step; key: "stepFrames" | "stepQuality" | "stepStack"; ready: boolean }[] = [
    { id: "frames", key: "stepFrames", ready: true },
    { id: "quality", key: "stepQuality", ready: false },
    { id: "stack", key: "stepStack", ready: false },
  ];

  type Role = "lights" | "darks" | "flats" | "biases" | "darkFlats";
  const ROLES: { id: Role; label: string }[] = [
    { id: "lights", label: "roleLights" },
    { id: "darks", label: "roleDarks" },
    { id: "flats", label: "roleFlats" },
    { id: "biases", label: "roleBiases" },
    { id: "darkFlats", label: "roleDarkFlats" },
  ];

  type MismatchDto = {
    kind: string;
    severity: string;
    expected: number | null;
    found: number | null;
    expectedText: string | null;
    foundText: string | null;
    english: string;
  };
  type RoleDto = {
    kind: string;
    set: number;
    frames: number;
    quality: string;
    alternatives: number;
    mismatches: MismatchDto[];
  };
  type ReasonDto = {
    kind: string;
    expected: number | null;
    found: number | null;
    expectedText: string | null;
    foundText: string | null;
    english: string;
  };
  type PlanDto = {
    lights: number;
    lightFrames: number;
    roles: RoleDto[];
    blocked: { kind: string; candidates: number; reasons: ReasonDto[] }[];
  };
  type SetDto = {
    id: number;
    kind: string;
    frames: number;
    active: number;
    body: string;
    exposure: number | null;
    iso: number | null;
    width: number;
    height: number;
  };
  type SuspicionDto = { kind: string; set: number; numbers: number[]; text: string | null };
  type SessionDto = {
    frames: number;
    seconds: number;
    sets: SetDto[];
    plans: PlanDto[];
    unassigned: { name: string; path: string; exposure: number | null; iso: number | null }[];
    suspicions: SuspicionDto[];
    rejected: { name: string; reason: string }[];
  };

  let step = $state<Step>("frames");
  // Три состояния, как у соседних проектов, и системное среди них: без него
  // человек со светлой системой получает тёмное окно и решает, что выбора нет.
  // Код ниже совпадает с ними построчно - см. ответ про общую папку.
  type Theme = "system" | "dark" | "light";
  const saved = localStorage.getItem("theme");
  let theme = $state<Theme>(
    saved === "light" || saved === "dark" || saved === "system" ? saved : "system",
  );
  const THEME_KEYS = {
    system: "themeSystem",
    dark: "themeDark",
    light: "themeLight",
  } as const;
  let aboutOpen = $state(false);
  let version = $state("");
  let formats = $state<string[]>([]);

  let roots = $state<Record<Role, string[]>>({
    lights: [],
    darks: [],
    flats: [],
    biases: [],
    darkFlats: [],
  });
  let hot = $state<Role | null>(null);
  let zones: Partial<Record<Role, HTMLElement>> = {};

  let scanning = $state(false);
  let session = $state<SessionDto | null>(null);
  let error = $state("");

  const chosen = $derived(Object.values(roots).some((paths) => paths.length > 0));

  $effect(() => {
    const root = document.documentElement;
    if (theme === "system") delete root.dataset.theme;
    else root.dataset.theme = theme;
    localStorage.setItem("theme", theme);
  });

  // onMount не может быть async и вернуть уборку одновременно, поэтому подписка
  // складывается сюда, а снимается в onDestroy.
  let unlisten: (() => void) | null = null;

  onMount(() => {
    document.documentElement.lang = i18n.locale;
    void (async () => {
      version = await invoke<string>("app_version");
      formats = await invoke<string[]>("formats");

      // Перетаскивание в Tauri приходит событием окна с координатами, а не в
      // DOM, поэтому попадание в корзину считается здесь по её прямоугольнику.
      unlisten = await getCurrentWebview().onDragDropEvent((event) => {
        if (event.payload.type === "over") {
          hot = zoneAt(event.payload.position.x, event.payload.position.y);
        } else if (event.payload.type === "drop") {
          const target = zoneAt(event.payload.position.x, event.payload.position.y);
          hot = null;
          if (target) add(target, event.payload.paths);
        } else {
          hot = null;
        }
      });
    })();
  });

  onDestroy(() => unlisten?.());

  function zoneAt(x: number, y: number): Role | null {
    const scale = window.devicePixelRatio || 1;
    const [px, py] = [x / scale, y / scale];
    for (const [role, element] of Object.entries(zones)) {
      if (!element) continue;
      const box = element.getBoundingClientRect();
      if (px >= box.left && px <= box.right && py >= box.top && py <= box.bottom) {
        return role as Role;
      }
    }
    return null;
  }

  function add(role: Role, paths: string[]) {
    const merged = new Set([...roots[role], ...paths]);
    roots[role] = [...merged];
    session = null;
  }

  async function browse(role: Role) {
    const picked = await open({ directory: true, multiple: true });
    if (!picked) return;
    add(role, Array.isArray(picked) ? picked : [picked]);
  }

  async function scan() {
    if (!chosen || scanning) return;
    scanning = true;
    error = "";
    try {
      session = await invoke<SessionDto>("scan_session", { roots });
    } catch (thrown) {
      error = String(thrown);
      session = null;
    } finally {
      scanning = false;
    }
  }

  // --- то, как читаются числа --------------------------------------------

  function exposure(seconds: number | null): string {
    if (seconds === null || !Number.isFinite(seconds)) return "—";
    if (seconds >= 1) return `${trim(seconds)} ${i18n.t("seconds")}`;
    return `1/${Math.round(1 / seconds)}`;
  }

  function trim(value: number, decimals = 2): string {
    return Number(value.toFixed(decimals)).toString();
  }

  function span(seconds: number): string {
    const abs = Math.abs(seconds);
    if (abs >= 86400) return `${trim(abs / 86400, 1)} ${i18n.t("days")}`;
    if (abs >= 3600) return `${trim(abs / 3600, 1)} ${i18n.t("hours")}`;
    return `${Math.round(abs / 60)} ${i18n.t("minutes")}`;
  }

  const KIND_KEYS: Record<string, string> = {
    light: "roleLights",
    dark: "roleDarks",
    flat: "roleFlats",
    bias: "roleBiases",
    "dark flat": "roleDarkFlats",
    darkflat: "roleDarkFlats",
  };

  function kindName(kind: string): string {
    const key = KIND_KEYS[kind.toLowerCase()];
    return key ? i18n.t(key as never) : kind;
  }

  const QUALITY_KEYS: Record<string, string> = {
    exact: "qualityExact",
    only: "qualityOnly",
    exposure: "qualityExposure",
    gain: "qualityGain",
  };

  /**
   * Расхождение словами. Ключа нет — показывается английская строка ядра, и
   * видно, что это пропуск в переводе, а не перевод.
   */
  function mismatchText(m: MismatchDto): string {
    const a = m.expectedText ?? (m.expected === null ? "—" : trim(m.expected, 3));
    const b = m.foundText ?? (m.found === null ? "—" : trim(m.found, 3));
    switch (m.kind) {
      case "exposure":
        return i18n.t("mExposure", { a: exposure(m.expected), b: exposure(m.found) });
      case "gain":
        return i18n.t("mGain", { a, b });
      case "temperature":
        return i18n.t("mTemperature", { a, b });
      case "black":
        return i18n.t("mBlack", { a, b });
      case "white":
        return i18n.t("mWhite", { a, b });
      case "orientation":
        return i18n.t("mOrientation", { a, b });
      case "body":
        return i18n.t("mBody", { a, b });
      case "lens":
        return i18n.t("mLens", { a, b });
      case "optics":
        return i18n.t("mOptics", { a, b });
      case "elapsed":
        return i18n.t("mElapsed", { a: span(m.expected ?? 0) });
      case "unrecorded":
        return i18n.t("mUnrecorded");
      default:
        return m.english;
    }
  }

  /** Отказ словами: почему набор не подошёл. */
  function reasonText(r: ReasonDto): string {
    const a = r.expectedText ?? (r.expected === null ? "—" : trim(r.expected, 3));
    const b = r.foundText ?? (r.found === null ? "—" : trim(r.found, 3));
    const key = {
      dimensions: "rDimensions",
      cfa: "rCfa",
      body: "rBody",
      gain: "rGain",
      depth: "rDepth",
      scale: "rScale",
      unrecorded: "rUnrecorded",
    }[r.kind];
    return key ? i18n.t(key as never, { a, b }) : r.english;
  }

  function suspicionText(s: SuspicionDto): string {
    const [a = 0, b = 0] = s.numbers;
    switch (s.kind) {
      case "minority":
        return i18n.t("sMinority", { set: s.set, a, b });
      case "biasNotShortest":
        return i18n.t("sBiasNotShortest", { set: s.set, a: trim(a, 3), b: trim(b, 3) });
      case "flatNeedsDarkFlats":
        return i18n.t("sFlatNeedsDarkFlats", { set: s.set, a: trim(a, 2) });
      case "inCameraDark":
        return i18n.t("sInCameraDark", { set: s.set, a: trim(a), b: trim(b) });
      case "spansNights":
        return i18n.t("sSpansNights", {
          set: s.set,
          kind: kindName(s.text ?? ""),
          a: span(a),
        });
      default:
        return s.kind;
    }
  }
</script>

<div class="shell">
  <nav class="rail">
    <div class="logo">{i18n.t("appName")}</div>

    {#each STEPS as s (s.id)}
      <button
        class="step"
        class:active={step === s.id}
        disabled={!s.ready}
        onclick={() => (step = s.id)}
      >
        {i18n.t(s.key)}
        {#if !s.ready}<span class="soon">{i18n.t("soon")}</span>{/if}
      </button>
    {/each}

    <div class="railfoot">
      <button
        class="ghost"
        onclick={() =>
          (theme = theme === "system" ? "dark" : theme === "dark" ? "light" : "system")}
      >
        {i18n.t(THEME_KEYS[theme])}
      </button>
      <button class="ghost" onclick={() => (aboutOpen = true)}>{i18n.t("about")}</button>
      <label>
        <span>{i18n.t("language")}</span>
        <select
          value={i18n.locale}
          onchange={(e) => i18n.set(e.currentTarget.value as Locale)}
        >
          {#each LOCALES as l (l.id)}
            <option value={l.id}>{l.label}</option>
          {/each}
        </select>
      </label>
    </div>
  </nav>

  <main class="content">
    {#if step === "frames"}
      <header class="bar">
        <h1>{i18n.t("stepFrames")}</h1>
        <button class="primary" disabled={!chosen || scanning} onclick={scan}>
          {scanning ? i18n.t("scanning") : session ? i18n.t("rescan") : i18n.t("scan")}
        </button>
      </header>

      <div class="zones">
        {#each ROLES as role (role.id)}
          <div bind:this={zones[role.id]}>
            <DropZone
              label={i18n.t(role.label as never)}
              hint={i18n.t("dropHere")}
              bind:paths={roots[role.id]}
              hot={hot === role.id}
              browseLabel={i18n.t("browse")}
              clearLabel={i18n.t("clear")}
              countLabel={(n) => (n === 1 ? i18n.t("pathChosen") : i18n.t("pathsChosen", { n }))}
              onbrowse={() => browse(role.id)}
            />
          </div>
        {/each}
      </div>

      {#if error}
        <section class="card err">
          <h2>{i18n.t("error")}</h2>
          <p>{error}</p>
        </section>
      {/if}

      {#if !chosen && !error}
        <p class="empty">{i18n.t("nothingToScan")}</p>
      {/if}

      {#if session}
        <section class="card">
          <h2>{i18n.t("summary")}</h2>
          <p class="num">
            {i18n.t("framesRead", { n: session.frames, s: trim(session.seconds) })} ·
            {i18n.t("setsFound", { n: session.sets.length })} ·
            {i18n.t("plansFound", { n: session.plans.length })}
          </p>
        </section>

        {#each session.plans as plan (plan.lights)}
          <section class="card">
            <h2>{i18n.t("plan")}</h2>
            <p class="num">
              {i18n.t("planLights", { set: plan.lights, n: plan.lightFrames })}
            </p>

            {#if plan.roles.length === 0}
              <p class="muted">{i18n.t("noCalibration")}</p>
            {:else}
              <h3>{i18n.t("calibration")}</h3>
              <ul class="roles">
                {#each plan.roles as role (role.kind)}
                  <li>
                    <div class="roleline num">
                      <strong>{kindName(role.kind)}</strong>
                      <span class="muted">
                        {role.frames} · {i18n.t("quality")}:
                        {i18n.t((QUALITY_KEYS[role.quality] ?? role.quality) as never)}
                        {#if role.alternatives > 0}
                          · {i18n.t("alternatives", { n: role.alternatives })}
                        {/if}
                      </span>
                    </div>
                    {#if role.mismatches.length > 0}
                      <ul class="mismatches">
                        {#each role.mismatches as m (m.kind + m.english)}
                          <li class={m.severity}>{mismatchText(m)}</li>
                        {/each}
                      </ul>
                    {/if}
                  </li>
                {/each}
              </ul>
            {/if}

            {#each plan.blocked as blocked (blocked.kind)}
              <p class="warning">
                {i18n.t("blocked", {
                  kind: kindName(blocked.kind),
                  n: blocked.candidates,
                })}
              </p>
              <ul class="mismatches">
                {#each blocked.reasons as r, index (r.kind + index)}
                  <li class="note">{reasonText(r)}</li>
                {/each}
              </ul>
            {/each}
          </section>
        {/each}

        <section class="card">
          <h2>{i18n.t("sets")}</h2>
          <table class="num">
            <tbody>
              {#each session.sets as set (set.id)}
                <tr>
                  <td class="dim">#{set.id}</td>
                  <td>{kindName(set.kind)}</td>
                  <td>{set.frames}</td>
                  <td class="muted">
                    {#if set.active < set.frames}
                      {i18n.t("excludedOf", { active: set.active, total: set.frames })}
                    {/if}
                  </td>
                  <td class="muted">{exposure(set.exposure)}</td>
                  <td class="muted">{set.iso ? `ISO ${set.iso}` : "—"}</td>
                  <td class="dim">{set.body}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </section>

        {#if session.suspicions.length > 0}
          <section class="card">
            <h2>{i18n.t("suspicions")}</h2>
            <ul class="plain">
              {#each session.suspicions as s, index (s.kind + index)}
                <li class="warning">{suspicionText(s)}</li>
              {/each}
            </ul>
          </section>
        {/if}

        {#if session.unassigned.length > 0}
          <section class="card">
            <h2>{i18n.t("unassigned")} ({session.unassigned.length})</h2>
            <p class="muted">{i18n.t("unassignedHint")}</p>
            <ul class="plain scroll">
              {#each session.unassigned as frame (frame.path)}
                <li title={frame.path}>{frame.name}</li>
              {/each}
            </ul>
          </section>
        {/if}

        {#if session.rejected.length > 0}
          <section class="card">
            <h2>{i18n.t("rejected")} ({session.rejected.length})</h2>
            <ul class="plain scroll">
              {#each session.rejected as file (file.name)}
                <li><span class="dim">{file.name}</span> — {file.reason}</li>
              {/each}
            </ul>
          </section>
        {/if}
      {/if}
    {/if}
  </main>
</div>

{#if aboutOpen}
  <div
    class="scrim"
    role="button"
    tabindex="0"
    onclick={() => (aboutOpen = false)}
    onkeydown={(e) => e.key === "Escape" && (aboutOpen = false)}
  >
    <div class="modal" role="dialog" tabindex="-1" onclick={(e) => e.stopPropagation()} onkeydown={() => {}}>
      <h2>{i18n.t("appName")}</h2>
      <p class="muted">{i18n.t("version", { v: version })}</p>
      <p>{i18n.t("aboutText")}</p>
      {#if formats.length > 0}
        <p class="muted">{i18n.t("formatsRead", { list: formats.join(", ") })}</p>
      {/if}
      <button class="primary" onclick={() => (aboutOpen = false)}>{i18n.t("close")}</button>
    </div>
  </div>
{/if}

<style>
  .shell {
    display: grid;
    grid-template-columns: 210px 1fr;
    height: 100%;
  }

  .rail {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 16px 12px;
    background: var(--surface);
    border-right: 1px solid var(--line);
    overflow-y: auto;
  }
  .logo {
    font-weight: 700;
    letter-spacing: 0.02em;
    color: var(--accent);
    padding: 4px 8px 14px;
  }
  .step {
    text-align: left;
    padding: 8px 10px;
    border: 1px solid transparent;
    border-radius: 8px;
    background: none;
    color: var(--muted);
    font: inherit;
    cursor: pointer;
    display: flex;
    align-items: baseline;
    gap: 8px;
  }
  .step:hover:not(:disabled) {
    color: var(--text);
    background: var(--chipbg);
  }
  .step.active {
    color: var(--text);
    background: var(--chipbg);
    border-color: var(--line);
  }
  .step:disabled {
    color: var(--faint);
    cursor: default;
  }
  .soon {
    margin-left: auto;
    font-size: 11px;
    color: var(--faint);
  }

  .railfoot {
    margin-top: auto;
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding-top: 16px;
    font-size: 12px;
  }
  .railfoot label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    color: var(--dim);
  }
  .railfoot select {
    font: inherit;
    padding: 4px 6px;
    border-radius: 7px;
    border: 1px solid var(--line);
    background: var(--chipbg);
    color: var(--text);
  }

  .content {
    padding: 20px 24px 40px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: var(--gap);
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 16px;
  }
  h1 {
    margin: 0;
    font-size: 20px;
  }
  h2 {
    margin: 0 0 6px;
    font-size: 14px;
    color: var(--muted);
    font-weight: 600;
  }
  h3 {
    margin: 12px 0 4px;
    font-size: 12px;
    color: var(--dim);
    font-weight: 600;
  }

  .zones {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(230px, 1fr));
    gap: var(--gap);
  }

  .card {
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    padding: 14px 16px;
  }
  .card.err {
    border-color: var(--err);
    background: var(--err-bg);
  }
  .card p {
    margin: 0 0 4px;
  }

  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 13px;
  }
  td {
    padding: 3px 10px 3px 0;
    border-bottom: 1px solid var(--line);
  }
  tr:last-child td {
    border-bottom: none;
  }

  .roles,
  .plain,
  .mismatches {
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .roles > li {
    padding: 5px 0;
    border-bottom: 1px solid var(--line);
  }
  .roles > li:last-child {
    border-bottom: none;
  }
  .roleline {
    display: flex;
    gap: 10px;
    align-items: baseline;
  }
  .mismatches {
    margin-top: 3px;
    padding-left: 14px;
    font-size: 12px;
  }
  .mismatches li.note {
    color: var(--dim);
  }
  .mismatches li.warning,
  .warning {
    color: var(--warn);
  }
  .plain {
    font-size: 13px;
  }
  .scroll {
    max-height: 190px;
    overflow-y: auto;
  }

  .muted {
    color: var(--muted);
  }
  .dim {
    color: var(--dim);
  }
  .empty {
    color: var(--dim);
    margin: 0;
  }

  button.primary {
    font: inherit;
    cursor: pointer;
    padding: 7px 16px;
    border-radius: 8px;
    border: 1px solid var(--accent);
    background: var(--accent);
    color: var(--accent-contrast);
    font-weight: 600;
  }
  button.primary:disabled {
    opacity: 0.45;
    cursor: default;
  }
  button.ghost {
    font: inherit;
    font-size: 12px;
    cursor: pointer;
    text-align: left;
    padding: 5px 8px;
    border-radius: 7px;
    border: 1px solid var(--line);
    background: none;
    color: var(--muted);
  }
  button.ghost:hover {
    color: var(--text);
    border-color: var(--accent);
  }

  .scrim {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.55);
    display: grid;
    place-items: center;
  }
  .modal {
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    padding: 20px 22px;
    max-width: 460px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .modal button {
    align-self: flex-end;
    margin-top: 8px;
  }
</style>
