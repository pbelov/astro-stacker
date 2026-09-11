<script lang="ts">
  // Одна корзина путей: лайты, дарки, флэты и так далее.
  //
  // Компонент ничего не знает про перетаскивание: в Tauri оно приходит не в
  // webview, а событием окна с координатами, поэтому попадание считает
  // родитель, а сюда приходит уже готовым флагом. Так же устроено в соседних
  // проектах, и это единственная часть, которую нельзя написать «как в вебе».

  let {
    label,
    hint,
    paths = $bindable([]),
    hot = false,
    browseLabel,
    filesLabel,
    clearLabel,
    countLabel,
    onbrowse,
    onfiles,
  }: {
    label: string;
    hint: string;
    paths: string[];
    hot?: boolean;
    browseLabel: string;
    filesLabel: string;
    clearLabel: string;
    countLabel: (n: number) => string;
    onbrowse: () => void;
    onfiles: () => void;
  } = $props();

  const tail = (path: string) => path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? path;
</script>

<div class="zone" class:hot class:filled={paths.length > 0}>
  <div class="head">
    <span class="label">{label}</span>
    {#if paths.length > 0}
      <span class="count num">{countLabel(paths.length)}</span>
      <button class="ghost" onclick={() => (paths = [])}>{clearLabel}</button>
    {/if}
  </div>

  {#if paths.length === 0}
    <p class="hint">{hint}</p>
  {:else}
    <ul class="paths">
      {#each paths as path (path)}
        <li title={path}>{tail(path)}</li>
      {/each}
    </ul>
  {/if}

  <!-- Папки и отдельные файлы. Папка читается без подпапок, так что кадр,
       отложенный в подпапку, туда и не вернётся; нужный кадр оттуда можно
       добавить поштучно. -->
  <div class="browse">
    <button onclick={onbrowse}>{browseLabel}</button>
    <button onclick={onfiles}>{filesLabel}</button>
  </div>
</div>

<style>
  .zone {
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-height: 132px;
    padding: 12px 14px;
    border: 1px dashed var(--line);
    border-radius: var(--radius);
    background: var(--surface);
    transition: border-color 0.12s, background 0.12s;
  }
  .zone.filled {
    border-style: solid;
  }
  /* Подсветка именно той корзины, над которой курсор: без неё перетаскивание
     в окно с пятью мишенями — угадайка. */
  .zone.hot {
    border-color: var(--accent);
    background: var(--raised);
  }

  .head {
    display: flex;
    align-items: baseline;
    gap: 8px;
  }
  .label {
    font-weight: 600;
  }
  .count {
    color: var(--muted);
    font-size: 12px;
  }

  .hint {
    margin: 0;
    color: var(--dim);
    font-size: 12px;
    flex: 1;
  }

  .paths {
    margin: 0;
    padding: 0;
    list-style: none;
    flex: 1;
    overflow-y: auto;
    max-height: 76px;
    font-size: 12px;
    color: var(--muted);
  }
  .paths li {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  button {
    font: inherit;
    cursor: pointer;
    border-radius: 7px;
    border: 1px solid var(--line);
    background: var(--chipbg);
    color: var(--text);
    padding: 5px 10px;
  }
  button:hover {
    border-color: var(--accent);
  }
  .browse {
    display: flex;
    gap: 6px;
    align-self: flex-start;
  }
  .browse button {
    font-size: 12px;
  }
  .ghost {
    margin-left: auto;
    background: none;
    border-color: transparent;
    color: var(--dim);
    font-size: 12px;
    padding: 2px 6px;
  }
  .ghost:hover {
    color: var(--err);
    border-color: var(--err);
  }
</style>
