# -*- coding: utf-8 -*-
"""Сборка THIRD-PARTY-NOTICES.md: что уходит в сборку приложения и тексты
лицензий этих пакетов.

Rust: обычные рёбра `cargo tree` под целевую платформу — build-зависимости
(генераторы кода) в бинарник не попадают. Дерево берётся от окна: оно тянет
ядро и декодеры, так что покрывает и командную строку. Фронтенд: рантайм-
зависимости приложения и Svelte с её зависимостями — что из этого останется
после tree-shaking, решает vite, и лишнее в списке безопаснее недостачи.

Скрипт — родня такому же в соседнем проекте star-trails, намеренно: один
формат уведомления на оба, чтобы читать их можно было одинаково.
"""
import glob
import hashlib
import io
import json
import os
import re
import subprocess
from collections import defaultdict

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__))).replace('\\', '/') + '/'
NODE = ROOT + 'apps/desktop/node_modules'
CACHE = glob.glob(os.path.expanduser('~/.cargo/registry/src/*'))

# --- Rust -------------------------------------------------------------------
tree = subprocess.run(
    ['cargo', 'tree', '--edges', 'normal', '--target', 'x86_64-pc-windows-msvc',
     '--prefix', 'none', '--no-dedupe'],
    cwd=ROOT + 'apps/desktop/src-tauri', capture_output=True, text=True, encoding='utf-8',
)
OWN = {'astro-stacker-desktop', 'astro-core', 'astro-format-canon', 'astro-cli'}
pkgs = set()
for line in tree.stdout.splitlines():
    m = re.match(r'^([A-Za-z0-9_.-]+) v([0-9][^ ]*)', line.strip())
    if m and m.group(1) not in OWN:
        pkgs.add((m.group(1), m.group(2)))
assert len(pkgs) > 100, tree.stderr[:500]


def pkg_dir(name, ver):
    for root in CACHE:
        p = os.path.join(root, f'{name}-{ver}')
        if os.path.isdir(p):
            return p
    return None


def read_license_files(d):
    out = []
    if not d:
        return out
    for pattern in ('LICENSE*', 'COPYING*', 'licenses/*', 'LICENCE*'):
        for f in sorted(glob.glob(os.path.join(d, pattern))):
            if os.path.isfile(f) and os.path.getsize(f) < 200_000:
                try:
                    out.append(io.open(f, encoding='utf-8', errors='replace').read().strip())
                except OSError:
                    pass
    return out


def spdx(d):
    if not d:
        return None
    toml = os.path.join(d, 'Cargo.toml')
    if not os.path.exists(toml):
        return None
    for line in io.open(toml, encoding='utf-8', errors='replace'):
        m = re.match(r'license\s*=\s*"([^"]+)"', line)
        if m:
            return m.group(1)
    return None


texts = defaultdict(list)
bodies = {}


def collect(name, ver, d):
    for body in read_license_files(d):
        key = hashlib.sha1(body.encode('utf-8', 'replace')).hexdigest()
        texts[key].append(f'{name} {ver}')
        bodies[key] = body


rust = []
for name, ver in sorted(pkgs):
    d = pkg_dir(name, ver)
    rust.append((name, ver, spdx(d) or 'не указана в манифесте'))
    collect(name, ver, d)

# --- npm --------------------------------------------------------------------
def closure(roots):
    seen, queue = set(), list(roots)
    while queue:
        name = queue.pop()
        if name in seen:
            continue
        pj = os.path.join(NODE, *name.split('/'), 'package.json')
        if not os.path.isfile(pj):
            continue
        seen.add(name)
        try:
            meta = json.load(io.open(pj, encoding='utf-8'))
        except Exception:
            continue
        queue.extend((meta.get('dependencies') or {}).keys())
    return sorted(seen)


npm = []
for pkg in closure(['@tauri-apps/api', '@tauri-apps/plugin-dialog', '@tauri-apps/plugin-log',
                    '@tauri-apps/plugin-opener', 'svelte']):
    d = os.path.join(NODE, *pkg.split('/'))
    try:
        meta = json.load(io.open(os.path.join(d, 'package.json'), encoding='utf-8'))
    except Exception:
        continue
    name = meta.get('name', pkg)
    ver = meta.get('version', '?')
    lic = meta.get('license') or meta.get('licenses') or 'не указана в манифесте'
    if isinstance(lic, list):
        lic = ' / '.join(x.get('type', '?') for x in lic)
    npm.append((name, ver, lic))
    collect(name, ver, d)

# Копилефт называется поимённо, а не тонет в таблице: у этих лицензий есть
# условия, которые надо выполнить, а не просто упомянуть.
mpl = sorted(name for name, _, lic in rust if 'MPL' in lic)

# --- файл -------------------------------------------------------------------
out = io.StringIO()
w = out.write
w('# Сторонние компоненты astro-stacker\n\n')
w('Само приложение — **MIT OR Apache-2.0**: тексты в `LICENSE-MIT` и\n')
w('`LICENSE-APACHE` в корне репозитория, исходники целиком —\n')
w('https://github.com/pbelov/astro-stacker.\n\n')
w('Ниже — всё, что уходит в сборку: библиотеки Rust, которые линкуются в\n')
w('бинарник (обычные рёбра `cargo tree` под x86_64-pc-windows-msvc, без\n')
w('build-зависимостей), и пакеты фронтенда, чей код vite вкомпилирует в бандл.\n')
w('Дальше — тексты лицензий; одинаковые собраны в один блок со списком\n')
w('пакетов. Где выбор оставлен за нами («MIT OR Apache-2.0» и подобное), мы\n')
w('берём любой из перечисленных вариантов.\n\n')
w('Файл собирается скриптом `tools/gen-notices.py` по `cargo tree` и\n')
w('`node_modules` — при смене зависимостей его надо пересобрать.\n\n')
w('## Особые условия\n\n')
w('**rawler (LGPL-2.1)** — декодер RAW, статически связан с приложением.\n')
w('Библиотека под GNU LGPL версии 2.1; авторы — Daniel Vogelbacher и\n')
w('Pedro Côrte-Real, исходники и история: https://github.com/dnglab/dnglab,\n')
w('пакет использованной версии: https://crates.io/crates/rawler. Полный текст\n')
w('LGPL-2.1 — ниже в этом файле.\n\n')
w('LGPL-2.1 §6 требует дать возможность заменить библиотеку своей версией и\n')
w('перелинковать программу. У нас это закрыто §6(a): исходники astro-stacker\n')
w('опубликованы целиком (ссылка выше), порядок сборки описан в `README.md`,\n')
w('версия rawler закреплена точным пином в корневом `Cargo.toml`. Изменений в\n')
w('саму rawler мы не вносили; обратная разработка ради таких изменений не\n')
w('ограничивается.\n\n')
if mpl:
    w('**' + ', '.join(mpl) + ' (MPL-2.0)** приходят с Tauri и WebView-слоем.\n')
    w('MPL-2.0 — файловый копилефт: эти файлы мы не меняли, их исходники\n')
    w('доступны на crates.io по имени и версии из таблицы ниже. Текст MPL-2.0 —\n')
    w('в этом же файле.\n\n')
w(f'## Библиотеки Rust ({len(rust)})\n\n')
w('| пакет | версия | лицензия |\n|---|---|---|\n')
for name, ver, lic in rust:
    w(f'| {name} | {ver} | {lic} |\n')
w(f'\n## Пакеты фронтенда ({len(npm)})\n\n')
w('| пакет | версия | лицензия |\n|---|---|---|\n')
for name, ver, lic in sorted(npm):
    w(f'| {name} | {ver} | {lic} |\n')
w('\n## Тексты лицензий\n\n')
order = sorted(texts.items(), key=lambda kv: (-len(kv[1]), bodies[kv[0]][:40]))
for key, users in order:
    body = bodies[key]
    head = (body.splitlines() or [''])[0][:70].strip() or 'без заголовка'
    users = sorted(set(users))
    shown = ', '.join(users[:12]) + (f' и ещё {len(users) - 12}' if len(users) > 12 else '')
    w(f'### {head}\n\n')
    w(f'Относится к: {shown}.\n\n')
    w('```\n' + body.replace('```', "'''") + '\n```\n\n')

data = out.getvalue()
io.open(ROOT + 'THIRD-PARTY-NOTICES.md', 'w', encoding='utf-8', newline='\n').write(data)
print('пакетов Rust:', len(rust), '· фронтенда:', len(npm), '· блоков текстов:', len(order))
print('размер:', round(len(data.encode('utf-8')) / 1024), 'КБ')
print('копилефт MPL:', mpl or 'нет')
