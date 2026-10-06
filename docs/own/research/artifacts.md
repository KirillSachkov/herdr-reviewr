# Что такое артефакт сессии: как это решает индустрия и что взять во вкладку «Сессия»

Дата исследования: 2026-10-06. Контекст: свой reviewr (форк herdr-reviewr), вкладка `4 Сессия`,
решения `docs/own/decisions.md` п. 3–7 и agent-desk п. 41, 45, 120, 129, 135, 144.

Пометки достоверности:

- **[док]** — прочитано в официальной документации или спецификации, ссылка рядом.
- **[локально]** — проверено командой на этой машине (записи Claude Code и Codex владельца).
- **[вторично]** — блог, обзор, сторонний пересказ.
- **[вывод]** — мой вывод, не факт.

## 0. Короткий ответ

1. Ни один продукт не решает «артефакт или нет» эвристикой по типу файла как главным правилом.
   Есть четыре устойчивых образца: **(а) типизированные документы самого harness** (план, список задач,
   walkthrough), **(б) явное объявление агентом через tool call, протокол или вложение** (A2A Artifact,
   Claude Code `Artifact`, Codex `open_in_codex`, MCP `resource_link`, вложения Devin и Amp), **(в) тип по
   месту** (Kiro `.kiro/specs`, Factory `.factory/docs`, Cursor `/opt/cursor/artifacts/`), **(г) «все
   изменения + кликабельные пути»** в desktop-приложениях и diff-панелях, где классификации нет вообще.
   Тип файла продукты используют для выбора просмотра (Claude desktop, Media drawer Antigravity CLI),
   а не для отбора. Ближайший прецедент терминальной панели — Antigravity CLI (раздел 1.1).
2. Явные объявления уже лежат в записях сессий, без всяких договорённостей с нами: `ExitPlanMode` и
   plan-файлы Claude Code, `Artifact` tool Claude Code, item `Plan` и `open_in_codex` у Codex
   (`update_plan` в rollout не сохраняется).
   Codex CLI официально советует просить агента «report each output path» — то есть пути в ответе.
   Строка «Артефакт: …» — только один из каналов, и за 150 последних сессий Claude она встретилась 0 раз
   [локально].
3. Правило «всё, кроме кода» на данных владельца ломается часто: в 33 из 84 сессий с правками агент
   правил только документы (`.md`, `.txt`, `.html`) [локально]. Тип файла годится для группировки, но
   не для отбора.
4. Пути из финального ответа шумные: 303 из 383 упомянутых путей сессия не писала через Write/Edit
   [локально]. Упоминание поднимает приоритет написанного файла, но само артефакт не делает.
5. Рекомендация: **механизм A «Файлы сессии по ярусам с причиной»** — ничего не прятать, а
   ранжировать по сигналам (объявлен → известное место → новый документ → упомянут → прочее) и
   показывать причину у каждого пункта. Явные метки — верхний ярус, но не условие работы. AI-ранжирование
   (механизм C) — позже, по клавише, поверх A.

## 1. Продукты и протоколы

Сводная таблица. Подробности и источники ниже.

| Продукт | Что считается артефактом | Кто решает | Как показан | Обратная связь |
|---|---|---|---|---|
| Google Antigravity | план, список задач, diff, walkthrough, скриншоты, записи браузера | harness: агент создаёт типизированные артефакты в своих режимах | IDE: панель Artifacts / review pane; CLI: список «actionable» + Media drawer, approve/reject, line comments | inline-комментарии как в Google Docs, агент продолжает без перезапуска |
| Claude.ai Artifacts | «значимый самостоятельный» контент, обычно > 15 строк | модель по критериям из системного промпта | боковая панель, версии | «Edit with Claude» на выделении, новый запрос |
| Claude Code `Artifact` | HTML/MD-страница, опубликованная на claude.ai | агент вызовом tool `Artifact` (сам или по просьбе) | URL, галерея, `/artifacts`, `Ctrl+]` | комментарии на странице, агент читает и отвечает |
| Claude Code plan mode | план в Markdown-файле | harness: `ExitPlanMode`, файл в `~/.claude/plans` или `plansDirectory` | диалог одобрения; в desktop — панель Plan | «No, keep planning» с текстом; `Ctrl+G` правит план в редакторе |
| Claude Code desktop | нет понятия; все изменения + пути в чате | пользователь: клик по пути; тип файла выбирает панель | Diff, File, Browser (HTML/PDF/картинки/видео) | комментарии к diff, правка файла |
| Claude Code checkpoints | не артефакт; снимки файлов до правки | harness, только Write/Edit/NotebookEdit | `/rewind` | откат |
| Kiro | `requirements.md`/`bugfix.md`, `design.md`, `tasks.md` | harness: фиксированное место `.kiro/specs/<имя>/` | spec pane, статусы задач | одобрение фазы, правка файлов, чат |
| Spec Kit, superpowers | `specs/<NNN-feature>/{spec,plan,tasks}.md`; `docs/superpowers/plans/YYYY-MM-DD-*.md` | соглашение о месте файла | обычные файлы | правка файлов |
| A2A | `Artifact`: `artifactId`, `name`, `description`, `parts[]` (text/raw/url/data + `mediaType`, `filename`) | агент-сервер явно; «результаты SHOULD возвращаться артефактами, не сообщениями» | решает клиент | новое сообщение в задачу |
| MCP | `resource_link` и embedded `resource` в результате tool; annotations `audience`, `priority` | сервер tool явно | решает клиент | нет |
| AG-UI | нет артефактов; `ACTIVITY_SNAPSHOT` с `activityType` (`PLAN`, `SEARCH`) | агент явно | решает клиент | нет |
| Codex (приложение, CLI) | сгенерированные файлы (документы, таблицы, PDF, HTML) — в «Work with files»; код — в review pane | вывод задачи; агент может открыть файл (`open_in_codex`) | превью рядом с чатом; sidebar: план, источники, сгенерированные файлы | аннотации на области; код — line comments |
| ChatGPT Canvas | проект письма или кода в отдельной панели | модель (обучена открывать на «напиши пост», не на Q&A); пользователь — «use canvas» | панель рядом с чатом, версии | правка, выделение → точечная правка, «Suggest edits» |
| GitHub Copilot | PR, коммиты, session log; план в VS Code — `/memories/session/plan.md` | harness: Plan agent | карточка Review Plan, Markdown-редактор | inline-комментарии к плану, «Submit Feedback», `@copilot` в PR |
| Cursor | план (Markdown, в home или `.cursor/plans`); у cloud agents — видео, скриншоты, логи; canvases | harness и место (`/opt/cursor/artifacts/` — [вторично]) | web UI, side panel | Design Mode: аннотации UI |
| Devin | вложения сообщений (видео тестов, скриншоты) | агент или запрос пользователя | вложение в чате/Slack | чат |
| Factory | spec (план) в `.factory/docs` | режим + место | файл | одобрить или продолжить |
| Replit | «публикуемые» выходы: приложение, слайды, визуализация | тип проекта | отдельная сущность | чат |
| Amp, OpenHands, Conductor, Vibe Kanban, Superset, Jules | понятия артефакта нет; панель Changes/diff | — | diff, файлы | line comments → агенту |

### 1.1. Google Antigravity

- Определение: «An artifact is a structured deliverable created by the agent to accomplish its task and
  communicate its progress and thinking to you» [док: https://antigravity.google/docs/artifacts,
  прочитано 2026-10-06, даты на странице нет].
- Виды по документации: Markdown-планы (implementation plan), code diffs, архитектурные схемы, картинки,
  записи браузера [док]. Обзоры добавляют task list и walkthrough со скриншотами [вторично:
  https://blog.openreplay.com/google-antigravity-ide-guide/].
- Кто решает: harness. Артефакты рождаются в основном в planning mode; агент «останавливается на
  промежуточных вехах и просит ревью планов или правок до их выполнения» [док].
- Показ: в Antigravity 2.0 — sidebar и review pane (план, визуальный diff, проигрывание записи браузера);
  в Antigravity CLI — панель ревью с клавиатуры в терминале и уведомление в status bar, когда нужно
  одобрение [док]. Это ближайший аналог нашей вкладки: терминальная панель артефактов существует.
- Обратная связь: inline-текст к артефакту «до того, как агент тронет локальные файлы» [док];
  комментарии в стиле Google Docs [вторично].
- **Antigravity CLI — прямой прецедент терминальной панели** [док: https://antigravity.google/docs/cli/artifacts/]:
  status bar пишет «N artifacts · /artifact to review»; `ctrl+r` открывает список с отметкой «new» и
  кнопками open/approve/reject; `p` — inline-превью на 12 строк; `y`/`n` — одобрить или отклонить;
  `Shift+A` — одобрить всё. Список делится на «actionable» (код, конфигурация, план в Markdown) и
  сворачиваемый **Media drawer** (PNG/JPG/WebP/SVG/MP4/WebM), медиа открывается системным просмотрщиком.
  В просмотре `c` добавляет line comment, `m` переключает Mermaid между картинкой Kitty, ASCII и исходником,
  `Esc` отправляет одобрения и комментарии агенту одним пакетом.
- Отдельные страницы: implementation plan ревьюится до правок кода
  (https://antigravity.google/docs/implementation-plan), walkthrough создаётся по завершении и держит
  скриншоты и записи (https://antigravity.google/docs/walkthrough), каждый скриншот — image-артефакт с
  комментариями (https://antigravity.google/docs/screenshots). Политика «Request review» / «Always proceed»
  решает, останавливается ли агент (https://antigravity.google/docs/artifact-review/) [док].
- Имена файлов: `implementation_plan.md` есть в официальном примере CLI [док]; `task.md`, `walkthrough.md`
  и хранение в `~/.gemini/antigravity-cli/brain/<conv-id>/` — [вторично]. Antigravity на этой машине не
  установлен.
- Вывод для нас [вывод]: артефакты Antigravity — это типы, которые harness порождает сам. Код здесь тоже
  артефакт (diff), то есть граница «документ или код» у них проходит не по типу файла, а по роли:
  «что агент предъявляет человеку».

### 1.2. Anthropic

**Claude.ai Artifacts.** Критерии из справки [док: https://support.claude.com/en/articles/9487310,
«Updated over a week ago» на 2026-10-06]: контент «significant and self-contained, typically over 15 lines»;
его захотят редактировать, дорабатывать или использовать вне разговора; он понятен без контекста
разговора; к нему вернутся позже. Виды: документы, код, одностраничные сайты, картинки, схемы, дашборды,
небольшие интерактивные инструменты. Решает модель по этим критериям; правка — «Edit with Claude» на
выделенном тексте или новый запрос.

Эти четыре критерия полезны нам как определение «артефакта для чтения» [вывод]: самостоятельный,
значимый по объёму, к нему вернутся, используется вне разговора. Они не зависят от типа файла.

**Claude Code `Artifact` (публикация на claude.ai).** [док: https://code.claude.com/docs/en/artifacts]
Artifact — «live, interactive web page that Claude Code publishes from your session». «Claude may publish
an artifact on its own when the output suits a page, or you can ask for one directly». Если место не
названо, Claude пишет HTML или Markdown во временный каталог вне проекта и публикует его. Обновление —
снова публикация на тот же URL; каждая публикация — версия. Комментарии на странице агент читает и отвечает.
В записи сессии это обычный `tool_use` с именем `Artifact` и `file_path` [локально: 3 вызова в 150 сессиях].

**Plan mode и plan-файлы.** [док: https://code.claude.com/docs/en/permission-modes и
https://code.claude.com/docs/en/settings-reference#plansdirectory]
В plan mode Claude исследует, пишет план и не правит исходники до одобрения. Варианты ответа: одобрить
(с выбором режима) или «No, keep planning» с указаниями; `Ctrl+G` открывает план в редакторе.
`plansDirectory`: «Choose where Claude Code stores the plan files it writes in plan mode… Default: unset, so
Claude Code uses `~/.claude/plans`»; путь вне проекта игнорируется. Имена по умолчанию случайные
(`parsed-hatching-wren.md` [локально]). В записи — `tool_use` `ExitPlanMode`.

**Checkpoints.** [док: https://code.claude.com/docs/en/checkpointing]
Снимок перед каждым промптом; отслеживаются только правки Write/Edit/NotebookEdit. Не отслеживаются:
правки через Bash (`rm`, `mv`, `cp`), правки большинства subagent, внешние правки. Это ровно те же слепые
зоны, что у нашего источника «запись разговора» [вывод]. В JSONL снимки видны как записи
`file-history-snapshot` (`trackedFileBackups` с абсолютными путями, включая файлы вне репозитория) и
`file-history-delta` [локально].

**Claude Agent SDK.** Понятия артефакта нет. Есть file checkpointing с тем же ограничением: «Only changes
made through the Write, Edit, and NotebookEdit tools are tracked» [док:
https://code.claude.com/docs/en/agent-sdk/file-checkpointing]. При откате Claude Code удаляет созданные
файлы — значит, «создан, потом исчез» — нормальный случай.

**Claude Code desktop.** [док: https://code.claude.com/docs/en/desktop]
Панели: chat, diff, browser, terminal, file, plan, tasks, subagent. Классификации артефактов нет. Diff
показывает все изменённые файлы. Пути в чате кликабельны: «Click a file path in the chat or diff viewer to
open it in the file pane. HTML, PDF, image, and video paths open in the Browser pane instead». То есть
тип файла выбирает **способ показа**, а не решает, артефакт ли это. Отдельная панель Plan показывает план.
Это сильный довод за наш подход «тип файла — для группировки и просмотра, не для отбора» [вывод].

### 1.3. Codex: что видно в записях владельца

Публичная документация Codex — в разделе 1.6. Здесь проверенное на этой машине
[локально, Codex 0.160.1, rollout `~/.codex/sessions/2026/10/04/rollout-…01a105bb….jsonl`]:

- События `event_msg` / `item_completed` с типами `FileChange`, `ImageView`, `McpToolCall`, `AgentMessage`,
  `CommandExecution` и другими.
- `FileChange.changes` — словарь «абсолютный путь → `{type: add|update|delete, unified_diff}`». Это
  аналог Claude Write/Edit: авторство и вид правки (создан или изменён) видны прямо.
- `ImageView.path` — агент посмотрел картинку. Это чтение, не создание.
- `McpToolCall` сервера `codex_app`, tool `open_in_codex`, аргументы `{target: {type: file, path}, placement:
  right}` — агент явно открывает файл владельцу в панели приложения. За 30 дней 6 вызовов: 3 `.md`,
  1 `.html`, 1 `.pdf`, 1 URL. Это готовый сигнал «агент предъявил файл», без нашей договорённости.
- `CommandExecution` содержит команду, но не список записанных файлов: файлы, созданные shell-командой,
  из записи не видны (как у Claude Bash).

### 1.4. Kiro, Spec Kit, superpowers: тип по месту

- Kiro: каждая spec создаёт три файла в `.kiro/specs/<feature>/`: `requirements.md` (или `bugfix.md`),
  `design.md`, `tasks.md`. Три фазы с одобрением; Quick Spec — без одобрений. `tasks.md` получает статусы
  in-progress/completed в реальном времени; по завершении агент открывает PR [док: https://kiro.dev/docs/specs].
- GitHub Spec Kit: `specs/<feature>/` с `spec.md`, `plan.md`, `research.md`, `data-model.md`,
  `quickstart.md`, `contracts/`, `tasks.md`; служебное — в `.specify/` [вторично:
  https://docs.plannotator.ai/frameworks/github-spec-kit.md].
- superpowers (skill `writing-plans`): `docs/superpowers/plans/YYYY-MM-DD-<feature>.md`, предпочтение
  пользователя важнее [вторично: https://skillselion.com/skills/obra/superpowers/writing-plans].
- AGENTS.md не задаёт каталогов вывода: это свободный Markdown с инструкциями [вывод по
  https://agents.md и обзорам]. Общего стандарта «куда агент кладёт отчёты» нет.
- У владельца уже есть свои места: `.reports/` (agent-desk п. 129) и скретчпады Claude
  `/private/tmp/claude-*/…/scratchpad/` [локально].

### 1.5. Протоколы

**A2A 1.0.0** [док: https://a2a-protocol.org/latest/specification/, раздел 4.1.6–4.2.2, прочитано 2026-10-06]:

- «Artifact: An output (e.g., a document, image, structured data) generated by the agent as a result of a
  task, composed of Parts».
- «Messages SHOULD NOT be used to deliver task outputs. Results SHOULD BE returned using Artifacts».
- Поля Artifact: `artifactId` (обяз.), `name`, `description`, `parts[]` (≥ 1), `metadata`, `extensions`.
- Part содержит ровно одно из `text`, `raw`, `url`, `data`, плюс `filename` и `mediaType`.
- `TaskArtifactUpdateEvent`: `artifact`, `append`, `lastChunk` — артефакт можно дописывать потоком.
- Вывод [вывод]: в A2A артефакт — это **явное объявление** с именем и описанием («зачем»). Наша строка
  «Артефакт: <путь> — <зачем>» по смыслу совпадает с `name` + `description` + `url`.

**MCP 2025-11-25** [док: https://modelcontextprotocol.io/specification/2025-11-25/server/tools,
…/server/resources]:

- Tool может вернуть `resource_link` (`uri`, `name`, `description`, `mimeType`) или embedded `resource`.
- Annotations: `audience` (`user`, `assistant`), `priority` 0.0–1.0, `lastModified`. Пара
  `audience: ["user"]` + высокий `priority` — ровно наш смысл «показать человеку» [вывод].

**AG-UI** [док: https://docs.ag-ui.com/concepts/events]: понятия артефакта нет. Есть
`ACTIVITY_SNAPSHOT`/`ACTIVITY_DELTA` с `activityType` (пример `"PLAN"`, `"SEARCH"`) — структурированное
состояние между сообщениями. Для нас это аналог Codex `update_plan`: план как состояние, не файл.

### 1.6. OpenAI

**ChatGPT Canvas** [док: https://openai.com/index/introducing-canvas, 2024-10-03; справка 9930697 теперь
404, поведение 2026 может отличаться]. «Canvas opens automatically when ChatGPT detects a scenario in which
it could be helpful»; пользователь может написать «use canvas». OpenAI обучала триггер открываться на
задачах вроде «write a blog post» и не открываться на общих вопросах: точность 83% для текста и 94% для
кода, для кода триггер намеренно сдержан. Правка — прямо в тексте; выделение → точечная правка;
«Suggest edits» и «Review code» дают inline-комментарии; есть версии.

**Codex** (документация переехала на learn.chatgpt.com/docs):

- `update_plan` — «the todo/checklist tool (not plan mode)»: `{explanation?, plan: [{step, status}]}`
  [док: github.com/openai/codex `codex-rs/protocol/src/plan_tool.rs`]. Plan mode (`/plan`) — отдельный
  item `plan {id, text}` [док: learn.chatgpt.com/docs/llms-full.txt].
- Что сохраняется в rollout [док: `codex-rs/rollout/src/policy.rs`]: новые rollout хранят `ItemCompleted`
  с `FileChange`, `Plan`, `ImageGeneration`, `ImageView`; старые — `PatchApplyEnd`. **Не сохраняются**:
  `TurnDiff`, `PlanUpdate` (то есть `update_plan`), `PatchApplyBegin`. Следствие [вывод]: diff хода надо
  собирать из `FileChange`, а чек-лист `update_plan` из rollout не восстановить.
- Codex Cloud: пользователь смотрит изменённые файлы и проверки, просит доработку, делает коммит или PR
  [док: learn.chatgpt.com/docs/cloud.md].
- Desktop: Codex живёт в приложении ChatGPT (Chat/Work/Codex). «Work with files»
  (`/docs/artifacts-viewer`) показывает превью сгенерированных документов, презентаций, таблиц и PDF рядом
  с чатом, может открыть файл сам после задачи; `.html` — интерактивное превью с переключателем исходника.
  Sidebar показывает «the agent's plan, sources, generated files, and chat summary». Обратная связь —
  **аннотации** на области, тот же механизм для «code, Markdown files, and websites». Код — в отдельной
  review pane с line comments и stage/revert/commit [док: artifacts-viewer.md, code-review.md].
- В CLI превью нет; документация советует просить Codex «report each output path» [док]. Это ровно наш
  случай терминала: пути из ответа агента — официально рекомендованный канал.
- Вывод [вывод]: OpenAI делит так же, как мы хотим: сгенерированные файлы — в просмотр с аннотациями,
  код — в ревью с diff. Отбор «сгенерированных» идёт по выводу задачи, а не по договорённости.

**OpenAI Agents SDK** [док: https://openai.github.io/openai-agents-python/results/]: понятия артефакта
нет. Результат — `final_output` (строка или экземпляр `output_type`) и `new_items` (сообщения, tool calls,
handoffs).

**GitHub Copilot** [док: docs.github.com, разделы coding agent; code.visualstudio.com/docs/copilot/agents/planning, 2026-09-30]:

- Coding agent: ветка, коммиты, draft PR с описанием, session log («internal monologue» и инструменты).
  Обратная связь — `@copilot` в комментарии PR или свои коммиты.
- Plan agent в VS Code: карточка **Review Plan**, «Open Full Plan» открывает файл плана в Markdown-редакторе;
  локальные сессии хранят план в `/memories/session/plan.md`, «not as a project file». Пользователь правит
  план, ставит inline-комментарии или «Submit Feedback»; незакрытые замечания блокируют одобрение.
- Copilot Workspace закрыт 2025-05-30 [док: githubnext.com/projects/copilot-workspace].

**Cursor** [док: cursor.com/docs/agent/planning; changelog 02-24-26, 3-0 (2026-04-02), 04-15-26]:

- Plan mode: план в Markdown, «saved by default in your home directory», «Save to workspace» переносит в
  репозиторий (путь `.cursor/plans/*.md` — [вторично], форум).
- Cloud agents с computer use выдают «merge-ready PRs with artifacts (videos, screenshots, and logs)»;
  артефакты видны в web UI. Механизм по форуму: агент пишет в `/opt/cursor/artifacts/`, платформа следит
  за каталогом [вторично]. Это образец «тип по месту» в чистом виде.
- Canvases (3.1): таблицы, схемы, графики, diff, to-do — «durable artifacts» в боковой панели.

### 1.7. Остальные агенты

- **Devin**: объекта «артефакт» нет; файлы приходят **вложениями сообщений** (видео тестов, скриншоты «as
  proof of testing»); API `GET /v1/attachments/{uuid}/{name}`. Решает агент или запрос пользователя
  [док: docs.devin.ai/work-with-devin/testing-and-recordings.md, devin-session-tools.md]. DeepWiki —
  отдельная вики репозитория, управляется `.devin/wiki.json`.
- **Jules**: план с кнопкой **Approve plan** (авто-одобрение по таймеру); итог — diff только изменённых и
  добавленных файлов; картинки в diff viewer (2025-08-22), скриншот фронтенда при проверке (2025-08-07)
  [док: jules.google/docs/review-plan, /code, /changelog].
- **Factory (Droid)**: Spec Mode; при «Save spec as Markdown» одобренный план пишется в `.factory/docs`
  ближайшего `.factory`, иначе в `~/.factory/docs` [док: docs.factory.ai/cli/user-guides/specification-mode].
  Артефакт = режим + место.
- **Amp**: панели Changes (с опцией «Order Files Intelligently», приглушает сгенерированный код) и Files;
  сгенерированные файлы остаются в orb, пока Amp не **приложит** их к ответу или не **опубликует** по URL —
  по просьбе пользователя [док: ampcode.com/docs/markdown/threads, …/orbs/files-and-attachments].
- **OpenHands**: Chat, Changes, VS Code, Terminal, Browser; понятия артефакта нет
  [док: docs.openhands.dev/openhands/usage/key-features.md].
- **Replit Agent**: «Artifact» — публикуемый выход проекта (веб-приложение, слайды, визуализация, игра);
  остальные файлы «support your artifacts» [док: docs.replit.com/learn/projects-and-artifacts].
- **Manus**: карточки правок файлов по ходу и структурированные итоговые результаты; слайды в отдельном
  превью [вторично: aiuxplayground.com/teardowns/manus/output]. Официального описания не нашёл.
- **Conductor, Vibe Kanban, Superset**: diff viewer с line comments агенту, plan mode (Conductor);
  панели артефактов нет. Superset показывает Markdown отрендеренным и картинки в редакторе
  [док: conductor.build/docs/reference/diff-viewer, vibekanban.com/docs/workspaces/changes.md,
  docs.superset.sh/diff-viewer, /editor].

## 2. Образцы индустрии

| Образец | Где | Сильное | Слабое для нашего инструмента |
|---|---|---|---|
| P1. Явное объявление (tool, протокол, вложение, строка) | A2A, MCP, Claude `Artifact`, Codex `open_in_codex`, вложения Devin и Amp, скриншоты Antigravity, строка «Артефакт:» | точно; есть «зачем»; агент знает замысел | зависит от агента и harness; частичная разметка прячет остальное |
| P2. Типизированные документы harness | Antigravity, Kiro, Claude plan, Codex `update_plan` | надёжно в своём harness; понятные типы | работает только для известных harness; не покрывает отчёты и прочие файлы |
| P3. Тип по месту (соглашение) | Kiro `.kiro/specs`, Factory `.factory/docs`, Cursor `/opt/cursor/artifacts/` и `.cursor/plans`, Spec Kit `specs/`, superpowers, `~/.claude/plans`, `.reports/` | дёшево; работает без агента | у каждого проекта своё; нужен список мест |
| P4. Все изменения + клик по пути, тип выбирает просмотр | Claude Code desktop, Conductor, Vibe Kanban, Superset, OpenHands, Amp Changes, Jules | ничего не теряет; без эвристик | нет ответа на «что почитать»; шум в больших сессиях |
| P5. Эвристика по виду файла | выбор панели в Claude desktop; Media drawer в Antigravity CLI; превью docx/pptx/xlsx/pdf/html в Codex app | работает с любой сессией | ломается на сессиях «только Markdown» (33/84 у владельца) |
| P6. Модель классифицирует | Claude.ai сам решает, создать ли artifact (критерии в промпте) | понимает смысл | цена, задержка, недетерминизм; нужен кэш |

Главный вывод [вывод]: зрелые продукты **совмещают** P1/P2 (что агент предъявил) с P4 (всё остальное
доступно). Никто не прячет файлы сессии только потому, что их никто не объявил.

## 3. Что видит наш инструмент

| Источник | Что даёт | Проверено | Слепые зоны |
|---|---|---|---|
| Запись Claude (JSONL) | `Write`/`Edit`/`MultiEdit`/`NotebookEdit` с `file_path`; `toolUseResult.type` = `create`/`update`; `file-history-snapshot`; `Artifact`, `ExitPlanMode`; финальный текст | [локально] | Bash-записи; subagent пишут в `<сессия>/subagents/agent-*.jsonl` — их надо читать отдельно [локально] |
| Запись Codex (rollout) | `FileChange` (путь, add/update/delete, diff), `Plan` (plan mode), `ImageGeneration`, `open_in_codex`, финальный текст | [локально] `FileChange`, `open_in_codex`; состав rollout — [док] `policy.rs` | shell-записи; `update_plan` и `TurnDiff` в rollout не сохраняются |
| Git своего worktree | все изменения, включая shell | upstream reviewr | в общем checkout нет авторства (agent-desk п. 129, проверено) |
| Watcher reviewr во время хода агента | записи в worktree с привязкой ко времени хода | upstream: `src/watch.rs`, `src/turn.rs` | только внутри worktree |
| Известные места | типы «план», «спека», «отчёт» | [док]/[вторично] | список мест конфигурируется |

Данные по 150 последним сессиям Claude владельца [локально, скрипт по `~/.claude/projects/*/*.jsonl`]:

- 84 сессии с правками; 455 путей правок; 211 из них `.md`, 23 `.html`.
- 33 сессии правили только документы (`.md`, `.mdx`, `.txt`, `.html`).
- 195 из 455 путей вне `cwd` сессии: 127 — в других каталогах `~/Work`, 65 — в скретчпадах Claude.
  Git worktree их не видит; запись разговора видит.
- Строка «Артефакт:» в последнем ответе — 0 из 145 сессий (договорённость ещё не внедрена).
- В последних ответах 100 из 145 упоминают пути; из 383 упомянутых путей 303 сессия не писала через
  Write/Edit (файл только читали, создали через shell или просто сослались).

## 4. Кандидаты механизма

### Механизм A. «Файлы сессии по ярусам с причиной» (рекомендую)

Кандидаты — все файлы, которые сессия **записала**: запись разговора (Write/Edit/…, `FileChange`,
включая subagent) ∪ изменения Git своего worktree, если reviewr работает в worktree этой сессии.
Каждый файл получает ярус по первому сработавшему правилу и видимую причину.

| Ярус | Правило | Причина в списке |
|---|---|---|
| 0. Отчёт и сводка | отчёт сессии (skill `report`, известный путь) и AI-сводка | «отчёт» |
| 1. Объявлен | `Artifact` tool; `open_in_codex`; строка `Артефакт: <путь> — <зачем>`; MCP `resource_link` с `audience: user` | «объявлен: <зачем>» |
| 2. План или спека | Claude `ExitPlanMode` и файл плана; Codex item `Plan`; известные места: `~/.claude/plans/`, `plansDirectory`, `.kiro/specs/*/`, `.factory/docs/`, `~/.factory/docs/`, `.cursor/plans/`, `specs/*/`, `docs/superpowers/{plans,specs}/`, `docs/plans/`, `.reports/`, глобы из конфига проекта | «план», «спека», «отчёт по месту» |
| 3. Новый документ | создан в сессии (`create`/`add`) и вид «документ» или «медиа» | «новый документ» |
| 4. Упомянут в ответе | путь из финального ответа хода совпал с записанным файлом | «упомянут в ответе» |
| 5. Изменённый документ | изменён (не создан) и вид «документ» или «медиа» | «изменён» |
| 6. Код и конфигурация | всё прочее | свёрнутая группа «Код и конфигурация» |

Виды файлов (для группы и способа показа, не для отбора):

- документ: `.md`, `.mdx`, `.txt`, `.rst`, `.adoc`, `.org`;
- медиа и страницы: `.pdf`, `.png`, `.jpg`, `.gif`, `.svg`, `.webp`, `.mp4`, `.html` вне каталогов
  исходников (`src/`, `app/`, `public/`, `templates/`);
- данные: `.csv`, `.tsv`, `.xlsx`, `.json` вне корня проекта;
- исключить всегда: файлы, игнорируемые `.gitignore` внутри worktree, lock-файлы, `node_modules/`, сборка.

Показ: ярусы 0–2 сверху, затем 3–5, ярус 6 свёрнут. Внутри яруса — по времени последней записи.
Больше 10 файлов в одном каталоге — сворачивать в узел каталога. Файлы вне worktree — отдельная группа с
полным путём (как в п. 5 решений). Медиа (картинки, видео, PDF) — отдельной свёрнутой подгруппой и
открываются системным просмотрщиком по `o`, как Media drawer в Antigravity CLI. Комментарии к артефакту
уходят агенту пакетом, как в upstream reviewr (`s`/`S`) и в Antigravity CLI (`Esc`).

Сбои и ответы:

| Случай | Что делает A |
|---|---|
| Сессия правила только Markdown (уроки, 20 файлов) | Ничего не теряется. Новые документы выше изменённых; упомянутые в ответе — выше; каталоги свёрнуты. Ревью diff тех же файлов — во вкладке Changes. |
| Отчёт записан вне репозитория (скретчпад, `/tmp`) | Виден через запись разговора: 65 таких записей в 150 сессиях. Git его не видит. |
| Отчёт или картинка созданы shell-командой (`python`, `playwright screenshot`, `cat >`) | Записи Write нет. Добор: путь из финального ответа, файл существует и его mtime попал в окно хода → ярус 4 с пометкой «создан командой (по времени)». Внутри worktree дополнительно ловит watcher/Git. Остаток не виден — честное ограничение. |
| Файл создан, потом удалён (или откатан `/rewind`) | Проверять существование при показе; удалённые — серой строкой в свёрнутой группе или скрыть. |
| Агент упомянул файл, который только читал | Не артефакт (агент-desk п. 129: только сделанное сессией). Можно показать отдельной свёрнутой группой «Ссылки из ответа», по умолчанию выключенной. Данные: 303 из 383 упоминаний — такие. |
| Общий checkout, чужие файлы в `git status` | Git используется только в своём worktree сессии; в общем checkout — только запись разговора. |
| Правки subagent | Читать `<сессия>/subagents/agent-*.jsonl` у Claude и дочерние rollout у Codex. |
| Агент частично отметил артефакты | Неотмеченные не пропадают: они ниже, но в списке. |

Плюсы: работает с любой сессией без договорённостей; явные метки и harness-сигналы только улучшают
порядок; детерминировано и объяснимо. Минусы: в больших сессиях список длинный; правила видов и мест
надо поддерживать.

### Механизм B. «Явное прежде всего, остальное — обычное ревью»

Так сейчас записано в agent-desk п. 144: если агент отметил артефакты, список = отмеченные + отчёт; если
нет — владелец видит обычное ревью. Добавить к меткам готовые сигналы harness (ярусы 1–2 механизма A).

Плюсы: короткий, точный список, когда агент следует договорённости. Минусы: 0 из 145 сессий сейчас
содержат метку; частичная разметка прячет неотмеченные отчёты; сессии без меток дают пустую вкладку.
Владелец прямо назвал этот риск. Годится как режим отображения поверх A («только объявленные»), а не
как механизм.

### Механизм C. «Модель ранжирует кандидатов» (позже, поверх A)

По клавише (как `A` для сводки, решение п. 4) модель получает кандидатов механизма A: путь, вид,
создан/изменён, размер, первые строки, финальный ответ, — и возвращает ярусы с причиной по JSON-схеме.
Подходит `codex exec --output-schema --sandbox read-only --ephemeral` (agent-desk п. 137) и тот же кэш,
что у сводки. Критерии для промпта — четыре критерия Claude.ai Artifacts (раздел 1.2).

Плюсы: понимает смысл («этот `.md` — урок, а этот — отчёт»). Минусы: цена и задержка; результат может
меняться; без A ему нечего ранжировать. Использовать как перестановку внутри A, никогда как фильтр.

## 5. Рекомендация и вопрос владельцу

Рекомендация [вывод]: строить A; в первой версии ярусы 0–6 без модели. B оставить переключателем
вида «только объявленные». C добавить после пробы на реальных сессиях, если порядок A не устроит.
Каналы явного объявления принимать все сразу: `Artifact`, `open_in_codex`, `ExitPlanMode`, строку
«Артефакт:». Строку для skill оформить в духе A2A: путь плюс «зачем» (это `name`/`description`).

Вопрос владельцу: как вкладка «Сессия» отбирает артефакты?

1. **A — все файлы сессии по ярусам с причиной, код свёрнут** (рекомендую: работает с любой сессией и
   ничего не прячет).
2. B — только объявленные и отчёт; без меток вкладка показывает отчёт и ссылку на Changes.
3. A сразу с C — модель ранжирует при каждом открытии (дороже и медленнее).

## 6. Что проверено и что нет

- Проверено документацией: Antigravity (определение, виды, CLI-панель), Claude.ai criteria, Claude Code
  `Artifact`, plan mode, `plansDirectory`, checkpoints, SDK checkpointing, desktop-панели, Kiro, A2A 1.0.0,
  MCP 2025-11-25, AG-UI.
- Проверено локально: структура JSONL Claude (`Write` input, `toolUseResult.type`, `file-history-*`,
  каталог `subagents/`), rollout Codex 0.160.1 (`FileChange`, `ImageView`, `open_in_codex`), все числа
  раздела 3.
- Проверено документацией через отдельных исследователей (ссылки в разделах 1.6–1.7): ChatGPT Canvas,
  Codex (`plan_tool.rs`, `policy.rs`, artifacts-viewer), Agents SDK, Copilot, Cursor, Devin, Jules, Factory,
  Amp, OpenHands, Replit, Conductor, Vibe Kanban, Superset, Antigravity CLI. Manus — только вторично.
- Не проверено: имена `task.md`/`walkthrough.md` и путь `brain/` Antigravity на диске; путь
  `/opt/cursor/artifacts/` и `.cursor/plans/` у Cursor; поведение Bash-созданных файлов в
  записи Codex для других версий; насколько число «303 из 383» завышено кодовыми ссылками в тексте
  (регулярное выражение грубое, в том числе ловит `src/foo.ts` в объяснениях).
