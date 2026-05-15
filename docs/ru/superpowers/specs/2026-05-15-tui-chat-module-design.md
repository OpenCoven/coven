# Выделение модуля чата TUI — Проектирование

**Статус:** Утверждено — готово к плану внедрения
**Дата:** 2026-05-15
**Область:** Фаза 2 работы по структурной очистке TUI. Накладывается на Фазу 1 ([`feat/tui-theme-module`](https://github.com/OpenCoven/coven/pull/56)); не может быть слита, пока та не приземлится.
**Подход:** Чистое перемещение кода. Без переименований, без изменений сигнатур, без изменений поведения.

---

## Проблема

`crates/coven-cli/src/chat.rs` содержит 1111 строк после миграции тем в Фазе 1. Это единственный файл, содержащий типы данных чат-TUI на базе ratatui, состояние приложения, ~370 строк кода отрисовки в 7 функциях render, цикл событий, публичную точку входа и тесты. Обязанности файла — модель состояния, представление, контроллер и жизненный цикл — все собраны в одном месте.

Симптомы, мотивирующие разделение:

- Файл громоздок для навигации и ревью. Код отрисовки (строки 473–838) — единый непрерывный блок.
- `impl App` (строки 88–457) сам по себе занимает 369 строк.
- Существующий регрессионный тест (`chat_module_stays_single_file_to_avoid_rust_module_ambiguity`, добавленный в `fa786f1`) активно препятствует разделению — его удаление является триггером этой работы.

Новый модуль `crate::theme`, приземлившийся в Фазе 1, уже демонстрирует паттерн, который мы хотим для поверхностей TUI: одна логическая обязанность на файл, точки вызова импортируют через `use`.

## Не-цели

Явно вне области Фазы 2 и не должны вкрадываться:

- **Изменения поведения** любого рода. Рендереры выдают тот же вывод. Цикл событий обрабатывает те же клавиши. CLI ведёт себя идентично.
- **Сужение API.** `pub enum MessageRole`, `pub struct ChatMessage`, `pub struct AgentInfo` остаются `pub`, хотя сегодня ни один вызывающий код вне модуля chat их не импортирует. Сужение до `pub(super)` — отдельная задача (кандидат на последующий PR; см. Фазу 2.1).
- **Извлечение хелперов.** `render_messages` (самый большой рендерер, ~98 строк) не рефакторится. Внутренние вспомогательные функции не выносятся.
- **Разделение `main.rs`.** Фаза 3 — вне области.
- **Выделение лаунчера / браузера сессий.** Фаза 4 — вне области. (Мы всё же создаём родительский модуль `tui/` на перспективу, но пока под ним живёт только `tui::chat`.)
- **Новые тесты.** Фаза 2 наследует существующие тесты и удаляет ограничительный страж. Новые поведенческие тесты не добавляются.

## Ограничения

- **Ни один элемент модуля chat не получает новой экспозиции за пределами `tui::chat::run_chat`.** Видимость сохраняется в точности как сегодня (чистое перемещение кода).
- **Крейт остаётся одним бинарником.** Без новых членов workspace, без библиотечной экспозиции.
- **`cargo clippy -p coven-cli --no-deps` выдаёт ноль предупреждений**, сохраняя чистое состояние после Фазы 1.

## Структура модуля

```
crates/coven-cli/src/
├── tui/
│   ├── mod.rs            (~10 строк: `pub mod chat;`)
│   └── chat/
│       ├── mod.rs        (~40 строк: pub fn run_chat + жизненный цикл сырого терминала)
│       ├── app.rs        (~530 строк: состояние, поведение, хелперы, тесты)
│       ├── render.rs     (~380 строк: 7 функций render)
│       └── events.rs     (~150 строк: цикл событий)
├── main.rs   (одна правка: `mod chat;` → `mod tui;` и `chat::run_chat()` → `tui::chat::run_chat()`)
└── ... (остальные файлы без изменений)
```

`crates/coven-cli/src/chat.rs` удаляется полностью. Компилятор Rust запрещает сосуществование `src/chat.rs` и `src/chat/mod.rs`, поэтому удаление однофайловой формы обязательно после появления формы каталога. (Мы используем форму `src/tui/chat/`, а не `src/chat/`, но принцип тот же: никакого `src/chat.rs` оставаться не должно.)

## Сопоставление содержимого по файлам

### `tui/mod.rs` (новый)

```rust
//! TUI surfaces for the coven CLI. Currently hosts the chat module; Phases 3–4
//! will land the launcher and session-browser carve-outs from main.rs here.

pub mod chat;
```

### `tui/chat/mod.rs`

Содержит публичную точку входа и жизненный цикл сырого терминала (включить raw-режим, войти в альтернативный экран, построить App, запустить цикл, восстановить терминал при drop).

| Из `chat.rs` | Новое местоположение |
|---|---|
| Строки 840–861 (`pub fn run_chat`) | `tui/chat/mod.rs` |
| (объявления модулей) | `mod app; mod events; mod render;` |

Необходимые импорты:
```rust
use std::io::stdout;
use anyhow::Result;
use crossterm::{
    execute,
    event::{DisableMouseCapture, EnableMouseCapture},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{Terminal, backend::CrosstermBackend};
```

### `tui/chat/app.rs`

Состояние, поведение, хелперы жизненного цикла и тесты. Половина модуля с «данными + методами».

| Из `chat.rs` | Новое местоположение | Видимость |
|---|---|---|
| Строки 33–38 (MessageRole) | `app.rs` | `pub` (сохранена с сегодняшнего дня) |
| Строки 40–46 (ChatMessage) | `app.rs` | `pub` (сохранена) |
| Строки 48–54 (AgentInfo) | `app.rs` | `pub` (сохранена) |
| Строки 56–60 (InputMode) | `app.rs` | приватный `enum` (сохранён) |
| Строки 62–69 (SlashCommandResult) | `app.rs` | приватный `enum` (сохранён) |
| Строки 71–85 (struct App) | `app.rs` | `pub(super)` — был приватным к модулю в chat.rs; теперь должен пересекать новую файловую границу в `render.rs` и `events.rs` |
| Строка 86 (SPINNER_FRAMES) | `app.rs` | `pub(super)` (используется и `App::tick`, и `render_status_bar`) |
| Строки 88–457 (impl App) | `app.rs` | без изменений |
| Строки 459–471 (discover_agents) | `app.rs` | `pub(super)` (вызывается `run_chat` в `mod.rs`) |
| Строки 990–992 (timestamp_now) | `app.rs` | `pub(super)` не нужен — единственные вызывающие находятся в самом `app.rs`. Оставить приватным. |
| Строки 994–1002 (truncate_str) | `app.rs` | то же — вызывается только `App::simulate_agent_response`. Оставить приватным. |
| Строки 1004–1111 (mod tests) | `app.rs` (после отбрасывания стража) | `#[cfg(test)] mod tests` |

**Заметка о видимости.** Чистое перемещение кода сохраняет наблюдаемое поведение. Но элементы `App`, `SPINNER_FRAMES`, `discover_agents`, `render_ui`, `run_event_loop`, и типы `MessageRole`/`ChatMessage`/`AgentInfo`, которые ранее были приватными к модулю (или только crate-pub, но неиспользуемыми), теперь должны иметь видимость, подходящую для пересечения новой границы подмодуля. Новая видимость — самая узкая из работающих:

- Элементы, потребляемые только внутри `app.rs`: остаются приватными (timestamp_now, truncate_str, InputMode, SlashCommandResult).
- Элементы, потребляемые между `app.rs`/`render.rs`/`events.rs`: `pub(super)` (App, SPINNER_FRAMES, MessageRole, AgentInfo, discover_agents).
- Элементы, потребляемые `mod.rs`: `pub(super)` (run_event_loop в events.rs, render_ui в render.rs, App + discover_agents).
- Ранее `pub` типы `MessageRole`, `ChatMessage`, `AgentInfo`: это единственный спорный момент. Сегодня они `pub` на уровне крейта (видны как `chat::MessageRole` и т.д.). Цель Подхода A «сохранить видимость» гласит, что они должны оставаться видимыми на уровне крейта после перемещения. **Решение:** объявить их `pub` внутри `app.rs` и реэкспортировать через `pub use app::{MessageRole, ChatMessage, AgentInfo};` в `tui/chat/mod.rs`. Видимый из крейта путь остаётся коротким (`tui::chat::ChatMessage` вместо `tui::chat::app::ChatMessage`), совпадая с сегодняшней поверхностью с точностью до префикса `tui::`.

### `tui/chat/render.rs`

Все 7 функций render и потребитель SPINNER_FRAMES. Чистый код представления.

| Из `chat.rs` | Новое местоположение | Видимость |
|---|---|---|
| Строки 473–510 (render_ui) | `render.rs` | `pub(super)` (вызывается `events.rs` через `run_event_loop`) |
| Строки 512–538 (render_status_bar) | `render.rs` | приватная `fn` (сохранена) |
| Строки 540–636 (render_messages) | `render.rs` | приватная `fn` |
| Строки 638–672 (render_input) | `render.rs` | приватная `fn` |
| Строки 674–700 (render_hint_bar) | `render.rs` | приватная `fn` |
| Строки 702–779 (render_help_overlay) | `render.rs` | приватная `fn` |
| Строки 781–838 (render_agent_select) | `render.rs` | приватная `fn` |

Необходимые импорты:
```rust
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Margin, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
};
use crate::theme::{self, AGENT_LABEL, DIM, HINT_KEY, PRIMARY, PRIMARY_STRONG, SURFACE, SURFACE_STRONG, USER_LABEL};
use super::app::{App, AgentInfo, InputMode, MessageRole, SPINNER_FRAMES};
```

### `tui/chat/events.rs`

Цикл событий.

| Из `chat.rs` | Новое местоположение | Видимость |
|---|---|---|
| Строки 863–988 (run_event_loop) | `events.rs` | `pub(super)` (вызывается из `run_chat` в `mod.rs`) |

Необходимые импорты:
```rust
use std::io::Stdout;
use std::time::{Duration, Instant};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::{Terminal, backend::CrosstermBackend};
use super::app::{App, SlashCommandResult};
use super::render::render_ui;
```

## Вид публичного API извне модуля chat

После разделения единственным видимым из крейта элементом является `tui::chat::run_chat` (и реэкспортированные типы `MessageRole`, `ChatMessage`, `AgentInfo`, которые остаются `pub` согласно цели сохранения видимости Подхода A). `main.rs` ссылается ровно на один из них:

```rust
// crates/coven-cli/src/main.rs строка 150 (до):
Some(Command::Chat) => chat::run_chat(),

// после:
Some(Command::Chat) => tui::chat::run_chat(),
```

И объявление `mod` в строке 23 (после Фазы 1):

```rust
// до:
mod chat;

// после:
mod tui;
```

Алфавитная позиция объявления `mod` смещается с `mod chat;` (между `mod api;` и `mod control_plane;`) на `mod tui;` (между `mod theme;` и `mod verification;`).

## Тесты

### Миграция

Все пять существующих тестов/хелперов (`app_with_agents`, `agent`, плюс 4 поведенческих теста, нацеленных на методы `App`) переезжают в блок `#[cfg(test)] mod tests` файла `app.rs` без изменений.

Тест-страж `chat_module_stays_single_file_to_avoid_rust_module_ambiguity` (chat.rs:1036) **удаляется**. Его цель состояла в том, чтобы предотвратить именно то разделение, которое реализует эта спецификация. Требуемый им импорт `use std::path::Path;` удаляется вместе с ним.

### Без заменяющего стража

Сам компилятор Rust отклоняет единственный по-настоящему неоднозначный случай (одновременное существование `src/tui/chat.rs` и `src/tui/chat/mod.rs`). Тест, утверждающий «эти конкретные файлы существуют в такой раскладке», был бы ограничением, поддерживаемым через каждую будущую реструктуризацию без функциональной выгоды.

## Критерии приёмки

Фаза 2 завершена, когда:

1. `crates/coven-cli/src/chat.rs` больше не существует (`git ls-files` ничего по нему не возвращает; в рабочем дереве такого файла нет).
2. `crates/coven-cli/src/tui/mod.rs` существует с единственным содержимым `pub mod chat;` (плюс комментарий документации уровня модуля).
3. `crates/coven-cli/src/tui/chat/` содержит ровно четыре файла: `mod.rs`, `app.rs`, `render.rs`, `events.rs`. Никаких других.
4. `crates/coven-cli/src/main.rs` имеет `mod tui;` (алфавитная позиция скорректирована) и `tui::chat::run_chat()` в строке 150.
5. `cargo build -p coven-cli` успешен с нулём предупреждений.
6. `cargo test -p coven-cli` проходит; счётчик модульных тестов уменьшается ровно на один (удалённый тест-страж). Smoke-тесты проходят в количестве 4.
7. `cargo clippy -p coven-cli --no-deps` выдаёт ноль предупреждений.
8. Ни один элемент модуля chat не получает новой экспозиции за пределами `tui::chat::run_chat`, `tui::chat::ChatMessage`, `tui::chat::AgentInfo`, `tui::chat::MessageRole` (три реэкспортированных типа из сегодняшней поверхности).
9. Ручная проверка: запуск `coven chat` открывает TUI и отрисовывает её без видимых регрессий (те же цвета, та же раскладка, те же сочетания клавиш).

## Оценочный масштаб diff

| Файл | Действие | Строки |
|---|---|---|
| `crates/coven-cli/src/chat.rs` | Удалить | -1111 |
| `crates/coven-cli/src/tui/mod.rs` | Создать | ~10 |
| `crates/coven-cli/src/tui/chat/mod.rs` | Создать | ~40 |
| `crates/coven-cli/src/tui/chat/app.rs` | Создать | ~530 |
| `crates/coven-cli/src/tui/chat/render.rs` | Создать | ~380 |
| `crates/coven-cli/src/tui/chat/events.rs` | Создать | ~150 |
| `crates/coven-cli/src/main.rs` | Правка в 1 строку + 1 замена mod | ±2 |

Итого: ~0 строк (файл реорганизуется, а не сокращается). Удалённый тест-страж убирает ~25 строк из текущего итога.
