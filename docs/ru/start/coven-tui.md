---
summary: "Интерактивное prompt-first меню, запускаемое `coven` или `coven tui`."
read_when:
  - Просмотр того, что может делать интерактивное меню Coven
  - Изучение slash-команд и сокращений
  - Решение, использовать ли TUI или низкоуровневые глаголы CLI
title: "TUI Coven"
description: "Используй prompt-first TUI Coven для запуска harness-сессий, просмотра выполняющейся работы, подключения к сессиям и срабатывания ритуалов из одного меню."
---

`coven` (или явное `coven tui`) открывает **prompt-first TUI**: интерфейс, основанный на Ratatui, где можно вводить задачи в свободной форме, запускать slash-команды или навигировать по меню ритуалов стрелками. Это рекомендуемая отправная точка для новых пользователей, и она работает через SSH или в локальном терминале.

## Когда её использовать

| Ситуация | Лучшая поверхность |
|---|---|
| Свежая установка, изучение того, что Coven может делать | **TUI** (`coven`) |
| Разовая задача в знакомом проекте | **TUI** или `coven run <harness> "<task>"` |
| Скриптинг, пайпинг, читаемый машиной вывод | `coven sessions --json`, `--plain` |
| Долгий attach/replay | Браузер сессий TUI или `coven attach <id>` |
| Быстрая проверка здоровья | `coven doctor` |

TUI — это тонкий слой представления. Каждое действие, которое она предлагает, отображается на базовый глагол CLI или вызов socket API — демон на Rust остаётся авторитетом.

## Анатомия

```mermaid
flowchart TB
  subgraph TUI["coven TUI"]
    Input["Prompt-first input bar\n(free text + slash commands)"]
    Browser["Session browser pane"]
    Help["Help / shortcuts overlay"]
  end
  Input -->|free text| LaunchPath["coven run <harness> <text>"]
  Input -->|slash command| Dispatch["/run, /sessions, /archive, ..."]
  Browser -->|Rejoin| Attach["coven attach"]
  Browser -->|Archive| Archive["coven archive"]
  Browser -->|Summon| Summon["coven summon"]
  Browser -->|Sacrifice| Sacrifice["coven sacrifice"]
  LaunchPath --> Daemon[Coven daemon]
  Dispatch --> Daemon
  Attach --> Daemon
  Archive --> Daemon
  Summon --> Daemon
  Sacrifice --> Daemon
```

TUI никогда не обходит демон. Корень проекта, cwd и id harness'а перепроверяются на стороне сервера при каждом запуске.

## Режимы input

Строка prompt принимает три формы input взаимозаменяемо:

1. **Свободный текст задачи** — всё, что **не** начинается с `/`. Нажатие `Enter` запускает harness по умолчанию против текущего проекта.

   ```text
   fix the failing tests
   review the diff in packages/cli
   ```

2. **Slash-команды** — начинаются с `/` и маршрутизируются к конкретному глаголу.

   ```text
   /run codex "audit this repo"
   /run claude "polish the help text" --title "Help polish"
   /sessions
   /archive session-1
   /help
   ```

3. **Навигация меню стрелками** — `↑` / `↓` циклически проходят через карты ритуалов (Rejoin, View Log, Summon, Archive, Sacrifice) для выбранной в данный момент сессии. `Enter` подтверждает. `Esc` отменяет.

## Справочник slash-команд

| Команда | Что делает |
|---|---|
| `/help` | Показывает overlay помощи со всеми сокращениями и примерами. |
| `/run <harness> "<task>"` | Запускает сессию, ограниченную проектом. Так же, как `coven run`. |
| `/sessions` | Открывает браузер сессий. Так же, как `coven sessions`. |
| `/attach <session-id>` | Подключиться к (или воспроизвести) сессии. |
| `/archive <session-id>` | Скрыть не выполняющуюся сессию, сохраняя события. |
| `/summon <session-id>` | Восстановить архивную сессию. |
| `/sacrifice <session-id>` | Навсегда удалить не выполняющуюся сессию. Просит ввести `sacrifice` для подтверждения. |
| `/doctor` | Запустить `coven doctor` и отрендерить результат inline. |
| `/clear` | Очистить строку input и любой inline-вывод. |
| `/export` | Скопировать запись текущей выбранной сессии как JSON в буфер обмена. |
| `/agent <harness>` | Установить harness по умолчанию для свободного input в этой сессии TUI. |
| `/exit` | Чисто закрыть TUI. Эквивалент `Ctrl+C` или `Esc` в корне. |

## Клавиатурные сокращения

| Клавиши | Действие |
|---|---|
| `h` (корень) | Открывает overlay `/help` |
| `↑ / ↓` | Перемещает выбор в браузере сессий или меню |
| `Enter` | Подтверждает выбор / отправляет prompt |
| `Esc` | Возврат из меню или выход в корне |
| `Ctrl+C` | Немедленный выход |
| `Tab` | Циклическое переключение фокуса между строкой input и браузером сессий |
| `Ctrl+L` | Перерисовка (полезно через нестабильный SSH) |

TUI безопасно изменяет размер. Терминалы, такие маленькие, как 80×24, остаются пригодными к использованию; более широкие терминалы автоматически расширяют список сессий, предпросмотр логов и overlay помощи.

## Действия браузера сессий

Выбор сессии и нажатие `Enter` показывает контекстные действия. Каждое ограничено состоянием сессии — действия, которые небезопасны для текущего состояния, скрываются, а не делаются серыми, поэтому меню никогда не предлагает разрушительный глагол, который ты не можешь выполнить.

| Действие | Доступно когда | Эффект |
|---|---|---|
| **Rejoin** | сессия `running` | Подключение к живому PTY; input пересылается в harness. |
| **View Log** | сессия не `running` | Воспроизведение журнала событий (только для чтения). |
| **Summon** | `archived_at` установлен | Восстановить в активный список и воспроизвести/следить. |
| **Archive** | сессия не `running` и не архивирована | Скрыть из активного списка; события сохраняются. |
| **Sacrifice** | сессия не `running` | Постоянное удаление; требует ввод подтверждения. |

Отображение между действиями и глаголами CLI задокументировано в [Жизненный цикл сессии](/SESSION-LIFECYCLE).

## SSH и удалённое использование

TUI основана на Ratatui и переживает обычные враждебные окружения:

- Терминалы через SSH (без локальных зависимостей мыши/шрифта).
- Изменение размера во время сессии (перерисовывает на `SIGWINCH`).
- `TERM=xterm-256color` или `screen-256color`.

Она **не** требует графического терминала, бэкенда буфера обмена или `tmux`. Если ты внутри `tmux` или `screen`, TUI ведёт себя как любое другое приложение Ratatui — splits панелей и detach по-прежнему работают.

## Fallback в plain-text

Если предпочитаешь неинтерактивный поток (CI, скриптинг, логи аудита), пропусти TUI полностью:

```bash
coven run codex "fix the failing tests"
coven sessions --plain
coven attach <session-id>
```

Эти глаголы производят стабильный, скриптуемый вывод, и это те же глаголы, к которым TUI в конечном итоге маршрутизирует.

> **Image asset prompt (to be generated and dropped into `docs/images/coven-tui-hero.png`):** Render a 1920×1080 hero screenshot of the Coven TUI on a dark OpenCoven-themed terminal (`#1A1825` background, `#9A8ECD` accent). Frame: top header `coven · interactive` with OpenCoven mark, middle pane shows two example sessions (`session-1 · codex · running · "Fix failing tests"` and `session-2 · claude · completed · "Polish help text"`), bottom shows the prompt input bar with `>` cursor and the text `/run codex "describe this repo"`. Sidebar lists ritual shortcuts: `r Rejoin · v View Log · s Summon · a Archive · x Sacrifice`. Use `Fragment Mono` throughout.

> **Image asset prompt (to be generated and dropped into `docs/images/coven-tui-ritual-menu.png`):** Render a 1600×900 close-up of the ritual menu overlay. Selected session header reads `session-2 · claude · archived · "Polish help text"`. Below, four cards in a vertical list: **View Log**, **Summon**, **Archive** (greyed/hidden), **Sacrifice**. The **Summon** card is highlighted with the OpenCoven accent (`#D4B5FF`). Caption strip at the bottom reads `Enter to confirm · Esc to cancel`.

## Связанное

- [Начать работу с Coven](/GETTING-STARTED)
- [Жизненный цикл сессии](/SESSION-LIFECYCLE)
- [Справочник CLI](/reference/cli)
- [Решение проблем](/TROUBLESHOOTING)
