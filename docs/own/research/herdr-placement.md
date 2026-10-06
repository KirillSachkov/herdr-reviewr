# Herdr 0.9.3: placement панели reviewr, баг «Z», кликабельные пути

Исследование только на чтение. Конфиги, панели и plugin actions не трогались.
Источники: `herdr 0.9.3` CLI, исходники `herdrdev/herdr` тега `v0.9.3` (клон в
`scratchpad/src/herdr`), `~/.config/herdr/herdr-server.log`, код форка
`herdr-reviewr`, GitHub issues herdr, Ghostty 1.3.1, Claude Code 2.1.292,
Codex 0.160.1.

Обозначения: **[Ф]** проверенный факт с источником. **[В]** вывод или предположение, его надо
проверить.

---

## 1. Какие placement есть у plugin pane

[Ф] Enum в `src/api/schema/plugins.rs:445`: `Overlay` (по умолчанию), `Popup`, `Split`, `Tab`,
`Zoomed`. Других вариантов нет: ни `float`, ни `fullscreen`, ни `sidebar`. Запрос на `sidebar`
закрыт как feature request (issue #4312).

[Ф] `herdr plugin pane open --help` показывает только `overlay, split, tab, zoomed`. Но строка
usage и docs (`plugins.mdx`) называют и `popup` с `--width/--height`. Это известная неточность help
(issues #1756, #1757).

| Placement | Как устроен (код) | Что видно | Сайдбар | Pane ID |
|---|---|---|---|---|
| `split` | Новая панель рядом с target pane (`panes.rs:82`) | Половина вкладки рядом с агентом | Виден | Есть |
| `zoomed` | Тот же split, затем `tab.zoomed = true` (`panes.rs:155-163`) | Вся область вкладки | Виден | Есть |
| `overlay` | Split от фокусной панели, `tab.zoomed = true`, запись в `overlay_panes` с прежним фокусом и zoom (`custom_commands.rs:438-519`) | Вся область вкладки; при выходе процесса herdr возвращает прежний фокус и zoom | Виден | Есть |
| `tab` | Новая вкладка с одной панелью (`panes.rs:180`) | Вся область вкладки, своя вкладка в tab bar | Виден | Есть |
| `popup` | Отдельный терминал вне раскладки, singleton на сессию (`app/popup.rs`) | Окно с рамкой по центру; `width/height` в ячейках или `%` | Виден | **Нет** |

### Как `toggle_placement` reviewr ложится на herdr

[Ф] `src/config.rs:393-410` принимает `split | overlay | zoomed | tab`. `popup` reviewr не знает.

[Ф] `src/actions.rs:365-371` и `src/herdr.rs:311-324` передают в CLI:

| `toggle_placement` | Аргументы `herdr plugin pane open` |
|---|---|
| `split` | `--placement split --target-pane <фокус> --direction <right/down>` |
| `zoomed` | `--placement zoomed --target-pane <фокус>` |
| `tab` | `--placement tab --workspace <ws>`, затем `tab rename` |
| `overlay` | `--placement overlay` (herdr сам берёт активную панель) |

[Ф] Auto-open в новом worktree работает только для `split` и `tab` (`actions.rs:112`).

### Можно ли показать панель на весь терминал, включая сайдбар

[Ф] Нет, ни один placement этого не умеет. Все четыре tiled-варианта живут внутри области
вкладки. Popup тоже: клиент считает геометрию от `layout.pane_surface`
(`src/client/shell/composition.rs:566`). `pane_surface` начинается справа от сайдбара и под
tab bar (`src/client/shell/config.rs:392-420`). Даже `width = "100%"` даёт область вкладки минус
рамку popup.

[Ф] API для сворачивания сайдбара нет. Есть клавиша `toggle_sidebar` (у владельца `prefix+b`) и
настройки `ui.sidebar_collapsed_mode = "hidden"` (ширина 0), `ui.sidebar_start_collapsed`,
`ui.hide_tab_bar_when_single_tab` (config reference v0.9.3).

Варианты «на максимум»:

| Вариант | Площадь | Цена |
|---|---|---|
| `overlay`/`zoomed`/`tab` | Вся вкладка | Сайдбар и tab bar остаются |
| `overlay` + `prefix+b` с `sidebar_collapsed_mode = "hidden"` | Почти весь терминал | Ручное нажатие; сайдбар скрыт для всех вкладок |
| `popup` 100%×100% | Вся вкладка минус рамка 1 ячейка | См. минусы popup в разделе 2 |
| Отдельное окно Ghostty со standalone reviewr | Весь экран | [В] Нет `HERDR_PANE_ID`: вкладка «Сессия» и отправка агенту не работают без доработки |

---

## 2. Баг: после смены workspace вкладка показывает «Z», overlay пропал

### Что значит «Z»

[Ф] `src/client/shell/tabs.rs:374-379`: если `tab.zoomed`, подпись вкладки получает суффикс ` Z`.
Значит, вкладка осталась в режиме zoom.

### Механизм

[Ф] Overlay в herdr не отдельный слой. Это обычная split-панель плюс zoom всей вкладки
(`custom_commands.rs:479-501`). Zoom хранится на вкладке, а не на панели.

[Ф] Zoomed-вкладка рисует только фокусную панель вкладки: `tab.layout.focused()`
(`src/ui/panes.rs:218, 286`).

[Ф] Любой переход «к агенту» вызывает `focus_pane_in_workspace` (`src/app/actions.rs:226`). Эта
функция переключает workspace и вкладку и ставит фокус на панель агента. Zoom она не снимает и
запись overlay не трогает. Так работают `agent.focus` из API (`src/app/agents.rs:75-88`), клик по
агенту в сайдбаре, `focus_agent`, `previous_agent`/`next_agent`, `open_notification_target`,
herdr-navigator (`herdr agent focus`, `src/app.rs:267` плагина) и, вероятно, agent-desk `--agents`.

[Ф] Обычное переключение workspace (`switch_workspace`, `actions.rs:387`) фокус внутри вкладки не
меняет. Само по себе оно overlay не прячет.

Итог: вкладка остаётся zoomed, но фокус уже на агенте. На экране агент во весь размер и « Z».
Процесс reviewr жив, его панель скрыта за zoom.

### Подтверждение по логу сервера

[Ф] `~/.config/herdr/herdr-server.log`, 06.10.2026:

| Pane | Событие | Что произошло дальше |
|---|---|---|
| 72 (wC8) | 19:03:12 overlay открыт; 19:03:50 `agent.focus` в той же вкладке | 19:03:56 toggle → `pane.close` скрытой панели; 19:03:58 новый toggle открыл pane 73 |
| 75 (wBD) | 19:06:57 overlay открыт; 19:07:30 переход в wC8; 19:07:59 возврат в wBD (два `tab.focus` подряд, как при переходе к агенту) | 19:08:00 toggle → `pane.close`; 19:08:01 новый toggle открыл pane 76 |

[В] Владелец видит агента с « Z» и жмёт `Ctrl+B D`. Toggle reviewr находит скрытую панель в
workspace и **закрывает** её. Второе нажатие открывает новый reviewr. Комментарии в закрытом
экземпляре пропадают, потому что store в памяти.

### Чья это ошибка

[Ф] Для herdr это ожидаемое поведение. Maintainer в issue #2659: «Zoom is intentionally tab-level
rather than pane-owned… changing the focused pane while a tab is zoomed keeps zoom active; selecting
an agent in the sidebar uses that same focus behavior». Issue закрыт как feature request.

[Ф] Overlays не дедуплицируются и не toggle-ятся самим herdr (issue #3199, feature request).

[В] Ошибка на стороне reviewr: toggle закрывает панель, которую пользователь не видит. Для
«tool window» правильная семантика у herdr-navigator `open-side` (`src/main.rs:88-113` плагина):
панели нет → открыть; панель есть, но не в фокусе → `herdr plugin pane focus <id>`; панель в
фокусе → закрыть.

[В] `plugin pane focus` вызывает тот же `focus_pane_in_workspace` (`plugins/mod.rs:560`). Во
вкладке, которая всё ещё zoomed, это сразу покажет reviewr на весь размер. Если пользователь снял
zoom вручную, нужен ещё `herdr pane zoom <id> --on`.

[Ф] Связанный открытый баг herdr #3799: в 0.9.0 overlay в многопанельной вкладке иногда рисуется не
на всю ширину. Для alt-screen приложений исправлено в #4016. reviewr использует alt-screen, поэтому
[В] его это, скорее всего, не задевает.

### Какой placement надёжнее для частого toggle

| Вариант | Прячется при переходе к агенту | Комментарии | Вход/выход по `Ctrl+B D` | Работа нужна |
|---|---|---|---|---|
| `overlay` + toggle «show-or-close» | Да, но toggle его вернёт одним нажатием | Сохраняются, пока панель не закрыта | Да | Малая: `actions.rs`, ветка «есть, но не в фокусе → focus (+zoom on)» |
| `tab` + toggle «перейти/вернуться» | Нет: своя вкладка, zoom не участвует | Сохраняются | Да; надо помнить прошлую вкладку | Малая-средняя |
| `zoomed` | Да, как overlay, и zoom не снимается при закрытии | Как overlay | Да | Не лучше overlay |
| `popup` 100% | Не прячется: popup модальный, пока он открыт, workspace не переключить | Теряются при каждом закрытии | **Нет**: при открытом popup все клавиши идут в popup раньше prefix (`src/client/shell/input.rs:525-527`), закрывать только `q` | Большая: нет `HERDR_PANE_ID`, ломаются вкладка «Сессия» (`session.rs:615`) и herdr connection (`herdr.rs:359`); Ctrl+click по ссылкам в popup отключён (`mouse.rs:916`) |

Рекомендация [В]: оставить `overlay` и исправить toggle reviewr на «show-or-close». Это лечит баг
«Z» одним нажатием и не теряет комментарии. Если владелец хочет, чтобы reviewr вообще не прятался,
подойдёт `tab`. Popup для окна с комментариями не подходит.

---

## 3. Кликабельные пути из вывода агента → reviewr

### Что умеет herdr

[Ф] В манифесте есть `[[link_handlers]]`: `id`, `pattern` (Rust regex по URL), `action` того же
плагина (`plugins.mdx`, раздел «Link handlers»). Клик с **Control** на любой платформе, включая
macOS. Action получает `HERDR_PLUGIN_CLICKED_URL`, `HERDR_PLUGIN_LINK_HANDLER_ID` и
`invocation_source = "link_click"` в `HERDR_PLUGIN_CONTEXT_JSON`. Контекст строится для кликнутой
панели (`plugins/mod.rs:356-405`).

[Ф] Что считается ссылкой (`src/app/actions.rs:1026-1033, 1157-1177`):
- OSC 8 hyperlink с **любой** схемой: `file://`, `vscode://`, свои схемы;
- в обычном тексте только `http://` и `https://`.

Обычный путь вида `src/app.rs` без OSC 8 herdr ссылкой не считает.

[Ф] OSC 8 `file://` доходит до link handlers с 0.9.0 (CHANGELOG 0.9.0: «Plugin link handlers now receive
matching OSC 8 `file://` clicks; unmatched file links still do not launch the system URL opener»,
#2941). Без handler herdr открывает в браузере только `http(s)`, остальное отдаёт приложению
(`src/client/shell/actions.rs:684-700`).

[Ф] Других hooks на клик нет. Есть `selected_text`: если клавиша объявлена как
`[[keys.command]] type = "plugin_action"`, herdr передаёт выделенный мышью текст в
`HERDR_PLUGIN_CONTEXT_JSON.selected_text` (`src/app/custom_commands.rs:95-115`). Двойной клик
выделяет токен и предпочитает URL и путь в кавычках (`actions.rs:1063-1080`).

[Ф] Аргументы у action. CLI `herdr plugin action invoke` принимает только `<ACTION_ID>` и
`--plugin` (`src/cli/plugin.rs:455-495`). Raw socket `plugin.action.invoke` принимает `context`, и
herdr подставляет из него `clicked_url`, `selected_text` и другие поля (`context.rs:5-30`). Через
сокет это рабочий канал для аргумента. `plugin pane open` принимает `--env KEY=VALUE`.

[Ф] Текущая клавиша `Ctrl+B D` у владельца имеет `type = "shell"`, а не `plugin_action`. Поэтому
`selected_text` она не передаёт.

### Ghostty

[Ф] Ghostty 1.3.1, `ghostty +show-config --default --docs`: опция `link` (regex + action) описана,
но помечена «TODO: This can't currently be set!». Работает только `link-url` для URL по
Cmd+hover на macOS. Свой обработчик путей в Ghostty сейчас не настроить.

[Ф] herdr пересылает OSC 8 ссылки панелей во внешний терминал (`src/protocol/render_ansi.rs:939-949`).

[В] Cmd+click в Ghostty по такой ссылке открыл бы `file://` в приложении по умолчанию, а не в
reviewr. Срабатывает ли Cmd+click при включённом mouse capture herdr, не проверено. Docs herdr
говорят, что Cmd в захваченных mouse reports не различим, поэтому herdr использует Ctrl.

### Claude Code

[Ф] Claude Code выводит OSC 8 для путей в tool output с 2.1.2 и `file://` ссылки в ответах
(CHANGELOG Claude Code: 2.1.2, 2.1.153, 2.1.274).

[Ф] Ссылки включаются только при поддержке терминала. Логика в бинаре 2.1.292: `FORCE_HYPERLINK`,
затем `TERM_PROGRAM` из списка `ghostty, iTerm.app, WezTerm, vscode, Hyper, kitty, alacritty,
WarpTerminal`, `VTE_VERSION`, `WT_SESSION`, `TERM=alacritty`.

[Ф] С herdr 0.9.2 новые панели ставят `TERM_PROGRAM=herdr` (CHANGELOG 0.9.2, #4104). В панели
сейчас `TERM_PROGRAM=herdr`, `TERM=xterm-256color`, `FORCE_HYPERLINK` не задан, `LC_TERMINAL` не
задан.

[В] Поэтому Claude Code внутри herdr 0.9.3 OSC 8 **не выводит**. Включить: `FORCE_HYPERLINK=1` в
`env` файла `~/.claude/settings.json`. Чтение `env` из settings исправлено в Claude Code (CHANGELOG:
«Fixed `FORCE_HYPERLINK` … ignored when set via settings.json env»). Проверка `pane read --format
ansi` по 6 панелям нашла 0 OSC 8. Это согласуется с выводом, но [В] не доказывает его: неизвестно,
сохраняет ли `pane read` OSC 8.

### Codex

[Ф] В Codex 0.160.1 есть `file_opener` с фиксированными значениями `vscode` (по умолчанию),
`vscode-insiders`, `windsurf`, `cursor`, `none`. Своей схемы задать нельзя.

[В] Если Codex выводит цитаты файлов как OSC 8 `vscode://file/<abs>:<line>`, их можно ловить
handler-ом `^vscode://file/`. Выводит ли Codex OSC 8 внутри herdr, не проверено.

### herdr-navigator и другие примеры

[Ф] herdr-navigator (v0.3.3, установлен) link handlers не использует. Он показывает полезный приём
«launch-or-focus»: ищет свою панель в `pane list` по label и делает `plugin pane focus` или `close`.

[Ф] `ChmaraX/herdr-nvim` объявляет handler `pattern = '^file://'` для OSC 8 ссылок Claude Code и
handler для обычных путей. [В] Второй в herdr 0.9.3 не сработает: обычный текст herdr распознаёт
только как `http(s)`.

### Предлагаемые механизмы

| # | Механизм | Как работает | Реализуемость | Объём |
|---|---|---|---|---|
| 1 | OSC 8 + `[[link_handlers]]` | `FORCE_HYPERLINK=1` для Claude Code. В манифест `kirill.reviewr` добавить handler `^file://` (и `^vscode://file/` для Codex после проверки) с action `open-link`. Action берёт `HERDR_PLUGIN_CLICKED_URL` и показывает файл в reviewr. Клик: **Ctrl+click** | Высокая для Claude Code после одной проверки `FORCE_HYPERLINK`. Codex под вопросом | Средний: handler + action (полдня) + доставка пути в живой reviewr (см. ниже) |
| 2 | Выделение + клавиша | Двойной клик по пути, затем новая клавиша `type = "plugin_action"` (например `prefix+shift+d`). Action читает `selected_text`, относительный путь разрешает от cwd агента | Высокая: работает для любого агента и обычного текста, без OSC 8 | Малый-средний; та же доставка пути |
| 3 | Без клика: список в reviewr | Вкладка «Сессия» форка уже собирает файлы, которые агент назвал или записал. Добавить Enter → открыть файл во вкладке «All files» | Высокая, всё внутри reviewr | Малый; но это не клик по выводу агента |

Доставка пути в уже открытый reviewr (нужна для 1 и 2). Закрыть и открыть заново с
`--env REVIEWR_OPEN=<path>` нельзя: пропадут комментарии (инвариант «Comments survive»).

| Вариант IPC | Плюсы | Минусы |
|---|---|---|
| Файл-запрос в `HERDR_PLUGIN_STATE_DIR`, который видит watcher reviewr | Событийно, в духе «nothing runs because time passed» | Новый путь в watcher |
| `herdr pane send-text` с меткой в bracketed paste, reviewr ловит `Event::Paste` | Без новых каналов | Хрупко: зависит от режима ввода reviewr |
| Свой unix socket на панель | Явный протокол | Больше кода |

Если reviewr не открыт, action открывает его через `plugin pane open … --env REVIEWR_OPEN=<path>`.
Затем `plugin pane focus <id>`, чтобы показать панель даже за zoom.

Своя URL-схема macOS (`reviewr://open?path=…`) [В] даёт мало. Внутри herdr link handler и так ловит
любую схему OSC 8. Claude Code выводит только `file://`, поэтому `reviewr://` появится, только если
агент сам печатает такие markdown-ссылки. Схема нужна лишь для кликов вне herdr. Тогда понадобится
маленькое .app с `CFBundleURLTypes` или Hammerspoon. Аргумент в action придётся передавать через
raw socket `context` или файл-запрос: CLI аргументов не принимает.

---

## 4. Что проверить руками (не делалось: владелец работает в Herdr)

1. Запустить Claude Code в новой панели с `FORCE_HYPERLINK=1` и убедиться, что пути в tool output
   подсвечиваются по Ctrl+hover в herdr.
2. Проверить, выводит ли Codex OSC 8 `vscode://file/...` внутри herdr.
3. Проверить, что выделение после двойного клика сохраняется до нажатия клавиши `plugin_action`.
4. Воспроизвести баг «Z»: открыть overlay, перейти к агенту этой вкладки через сайдбар или
   `prefix+alt+N`, убедиться, что `herdr pane list` всё ещё показывает панель reviewr.
