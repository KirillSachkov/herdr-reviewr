# Свой reviewr

Форк herdr-reviewr для владельца: ревью изменений сессии, артефакты, отчёты и сводка прямо в Herdr.
Решения владельца — [decisions.md](decisions.md), план — [plan.md](plan.md), исследования — [research/](research/).

## Устройство форка

- `origin` — [KirillSachkov/herdr-reviewr](https://github.com/KirillSachkov/herdr-reviewr), `upstream` —
  persiyanov/herdr-reviewr (push отключён). Обновление: `git fetch upstream && git merge upstream/main`.
- Свой код живёт в отдельных модулях; правки в `src/app.rs` и `src/ui.rs` — короткие вызовы в них,
  чтобы merge upstream ложился без конфликтов.
- Ядро понимания сессии — библиотека agent-desk (`../agent-desk`, приватный репозиторий
  KirillSachkov/agent-desk). Сборка требует этот checkout рядом.
- Plugin `kirill.reviewr` (`own/herdr-plugin/`) подключён через `herdr plugin link` рядом с
  `persiyanov.reviewr`. Конфиг — ссылка на тот же `~/Work/Tools/workspace/herdr/reviewr.toml`.

## Установка своей сборки

```
just own-install
```

Скрипт собирает release, кладёт бинарь в `own/herdr-plugin/bin/` через новый inode с подписью и при
первом запуске выполняет `herdr plugin link`. Открытые панели держат старый бинарь: закрой и открой их.
Открыть панель: `herdr plugin action invoke toggle --plugin kirill.reviewr` (клавишу назначает сессия
`workspace` или владелец). Обе версии ищут панели по имени бинаря, поэтому toggle одной версии видит и
закрывает панель другой.

## Вкладка «Сессия»

| Клавиша | Что делает |
|---|---|
| `4` | вкладка «Сессия»: сессия агента этой вкладки Herdr |
| `A` | построить AI-сводку (codex exec, 5–30 с; кэш agent-desk) |
| `o` | открыть выбранный артефакт в браузере или системном приложении |
| `r` | перечитать сессию |
| `→` `←` | раскрыть или свернуть группу |

Сверху строка «Нужно от вас». В списке первой стоит сводка, за ней артефакты по ярусам (решение 11);
код и конфигурация свёрнуты. Вкладка видна в строке вкладок, только пока открыта: так строка upstream
сохраняет ширину. `cargo run --example session_probe -- <repo>` с `HERDR_PANE_ID`/`HERDR_TAB_ID`
печатает, что вкладка нашла бы.
