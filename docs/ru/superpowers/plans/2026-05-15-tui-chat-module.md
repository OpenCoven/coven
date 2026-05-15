# План внедрения выделения модуля чата TUI

> **Для агентных работников:** ТРЕБУЕМЫЙ ПОД-НАВЫК: Используйте superpowers:subagent-driven-development (рекомендуется) или superpowers:executing-plans для реализации этого плана задача за задачей. Шаги используют синтаксис чекбоксов (`- [ ]`) для отслеживания.

**Цель:** Разделить `crates/coven-cli/src/chat.rs` (1111 строк) на 4-файловый модуль под новым пространством имён `tui/`, с нулевыми изменениями поведения.

**Архитектура:** Чистое перемещение кода. Три последовательных коммита: (1) каркас пустых новых файлов + подключение `mod tui;` в main.rs, (2) перемещение всего содержимого из `chat.rs` в новые файлы, превращая `chat.rs` в шим реэкспорта, (3) удаление `chat.rs` + стража и обновление точки вызова `main.rs`.

**Технический стек:** Rust edition 2021. Без новых зависимостей. Те же ratatui 0.30 / crossterm 0.29, что и в Фазе 1.

**Спецификация:** [`docs/superpowers/specs/2026-05-15-tui-chat-module-design.md`](../specs/2026-05-15-tui-chat-module-design.md)

**Ветка:** `feat/tui-chat-module`, накладывается на `feat/tui-theme-module`. PR не может быть слит, пока не приземлится #56.

**Worktree:** `/Users/buns/Documents/GitHub/OpenCoven/coven/.worktrees/feat-tui-chat-module`

---

## Карта файлов

| Файл | Действие | Заметки |
|---|---|---|
| `crates/coven-cli/src/tui/mod.rs` | **Создать** (~10 строк) | Док-комментарий уровня модуля + `pub mod chat;` |
| `crates/coven-cli/src/tui/chat/mod.rs` | **Создать** (~40 строк) | `pub fn run_chat` + реэкспорты `MessageRole`/`ChatMessage`/`AgentInfo` |
| `crates/coven-cli/src/tui/chat/app.rs` | **Создать** (~530 строк) | Всё состояние, поведение, хелперы, тесты |
| `crates/coven-cli/src/tui/chat/render.rs` | **Создать** (~380 строк) | Все 7 функций `render_*` |
| `crates/coven-cli/src/tui/chat/events.rs` | **Создать** (~150 строк) | `run_event_loop` |
| `crates/coven-cli/src/chat.rs` | **Удалить** (сейчас 1111 строк) | Заменяется модулем выше |
| `crates/coven-cli/src/main.rs` | **Изменить** (~2 строки) | `mod chat;` → `mod tui;` (алфавитное переупорядочивание); `chat::run_chat()` → `tui::chat::run_chat()` |

Никакие другие файлы не меняются. Тесты не добавляются; один тест (страж) удаляется.

---

## Критическая заметка о рабочем каталоге

ВСЕ команды `cd`, `cargo` и `git` в этом плане выполняются из:

```
/Users/buns/Documents/GitHub/OpenCoven/coven/.worktrees/feat-tui-chat-module
```

Первое действие в каждой задаче — сделать `cd` туда и убедиться, что `git rev-parse --abbrev-ref HEAD` равно `feat/tui-chat-module`. В противном случае ОСТАНОВИТЬСЯ и сообщить ЗАБЛОКИРОВАНО. (Это урок из Фазы 1, когда некоторые субагенты-реализаторы случайно писали в основной checkout.)

---

## Задача 1: Каркас новой структуры модуля

Создать пустые/скелетные файлы для нового модуля и подключить его к `main.rs`. После этой задачи и `mod chat;` (указывающий на старый `chat.rs`), и `mod tui;` (указывающий на новый, почти пустой модуль) сосуществуют. Сборка проходит с предупреждениями о неиспользуемых элементах в новых файлах.

**Файлы:**
- Создать: `crates/coven-cli/src/tui/mod.rs`
- Создать: `crates/coven-cli/src/tui/chat/mod.rs`
- Создать: `crates/coven-cli/src/tui/chat/app.rs`
- Создать: `crates/coven-cli/src/tui/chat/render.rs`
- Создать: `crates/coven-cli/src/tui/chat/events.rs`
- Изменить: `crates/coven-cli/src/main.rs` (добавить объявление `mod tui;`)

- [ ] **Шаг 1: cd в worktree и проверка ветки**

```bash
cd /Users/buns/Documents/GitHub/OpenCoven/coven/.worktrees/feat-tui-chat-module
pwd
git rev-parse --abbrev-ref HEAD
```

Ожидается:
```
/Users/buns/Documents/GitHub/OpenCoven/coven/.worktrees/feat-tui-chat-module
feat/tui-chat-module
```

Если что-то отличается, ОСТАНОВИТЬСЯ и сообщить ЗАБЛОКИРОВАНО. Не модифицировать файлы вне этого worktree.

- [ ] **Шаг 2: Создать `crates/coven-cli/src/tui/mod.rs`**

Записать точно это содержимое:

```rust
//! TUI surfaces for the coven CLI. Currently hosts the chat module; Phases 3–4
//! will land the launcher and session-browser carve-outs from main.rs here.

pub mod chat;
```

- [ ] **Шаг 3: Создать `crates/coven-cli/src/tui/chat/mod.rs` как временную заглушку**

Этот файл — заглушка для Задачи 1. Он будет наполнен `run_chat` и реэкспортами в Задаче 2. Пока он должен компилироваться без предупреждений, хотя ничто ещё не ссылается на его подмодули.

Записать точно это содержимое:

```rust
//! Ratatui-based chat TUI. State lives in `app`, view in `render`, event loop
//! in `events`. The entry point `run_chat` here manages the raw-terminal
//! lifecycle.

#![allow(dead_code)]

mod app;
mod events;
mod render;
```

`#![allow(dead_code)]` временный — он удаляется на Шаге 5 Задачи 2, когда `run_chat` приземляется здесь и потребляет подмодули. Подмодули объявлены приватными (`mod`, а не `pub mod`), потому что ни одному коду вне `tui::chat` не нужно лезть в `tui::chat::app::*`.

- [ ] **Шаг 4: Создать три пустых файла подмодулей**

Каждый должен быть валидным Rust, который компилируется самостоятельно. Запишите каждый файл только с док-комментарием и строкой `// placeholder` (заменяется в Задаче 2):

**`crates/coven-cli/src/tui/chat/app.rs`:**

```rust
//! Chat application state, behavior, and tests. Populated in Task 2 of the
//! chat-module carve-out (see plans/2026-05-15-tui-chat-module.md).

// placeholder — content lands in Task 2
```

**`crates/coven-cli/src/tui/chat/render.rs`:**

```rust
//! Chat TUI render functions. Populated in Task 2 of the chat-module
//! carve-out (see plans/2026-05-15-tui-chat-module.md).

// placeholder — content lands in Task 2
```

**`crates/coven-cli/src/tui/chat/events.rs`:**

```rust
//! Chat TUI event loop. Populated in Task 2 of the chat-module
//! carve-out (see plans/2026-05-15-tui-chat-module.md).

// placeholder — content lands in Task 2
```

- [ ] **Шаг 5: Добавить `mod tui;` в main.rs**

Найти этот блок в `crates/coven-cli/src/main.rs` (около строк 31–33 после вставки `mod theme;` из Фазы 1):

```rust
mod store;
mod theme;
mod verification;
```

Вставить `mod tui;` алфавитно между `theme` и `verification`:

```rust
mod store;
mod theme;
mod tui;
mod verification;
```

НЕ удалять `mod chat;` пока (этим занимается Задача 3). Оба модуля сосуществуют после Задачи 1.

- [ ] **Шаг 6: Проверить, что крейт собирается**

```bash
cargo build -p coven-cli 2>&1 | tail -20
```

Ожидается: собирается чисто. Несколько предупреждений «unused import» о `crate::tui::chat` или его подмодулях допустимы в Задаче 1 — они будут потреблены в Задаче 3.

Если вы видите реальные ошибки (не предупреждения), ОСТАНОВИТЬСЯ и сообщить ЗАБЛОКИРОВАНО с текстом ошибки.

- [ ] **Шаг 7: Запустить все тесты**

```bash
cargo test -p coven-cli 2>&1 | tail -10
```

Ожидается: все существующие тесты проходят. Модуль chat (всё ещё в `src/chat.rs`) и его тесты не тронуты. Количество тестов: 172 unit + 4 smoke = 176 (как в конце Фазы 1).

- [ ] **Шаг 8: Коммит**

```bash
git add crates/coven-cli/src/tui/ crates/coven-cli/src/main.rs
git commit -m "refactor(tui): scaffold tui/chat module structure

Empty submodule skeleton for the chat carve-out. Old chat.rs remains
the active implementation; this commit only adds the new file tree and
wires mod tui; into main.rs. Task 2 of the chat-module plan moves the
content; Task 3 deletes the old file.
"
```

- [ ] **Шаг 9: Убедиться, что коммит приземлился на правильной ветке**

```bash
git log --oneline -2
git rev-parse --abbrev-ref HEAD
```

Ожидается: новый коммит сверху, и HEAD на `feat/tui-chat-module`. Если нет, ОСТАНОВИТЬСЯ и сообщить.

---

## Задача 2: Переместить всё содержимое из `chat.rs` в новые файлы модуля

Это основная часть работы. Стратегия: копировать каждый раздел старого `chat.rs` в его целевой новый файл, исправить импорты + видимость, затем заменить `chat.rs` на шим реэкспорта (`pub use crate::tui::chat::*;`), чтобы старая точка вызова `chat::run_chat()` в `main.rs` продолжала работать в течение Задачи 2. Задача 3 удаляет шим и обновляет точку вызова.

**Файлы:**
- Изменить: `crates/coven-cli/src/tui/chat/mod.rs` (заменить заглушку на run_chat + реэкспорты)
- Изменить: `crates/coven-cli/src/tui/chat/app.rs` (заменить placeholder кодом состояния)
- Изменить: `crates/coven-cli/src/tui/chat/render.rs` (заменить placeholder рендерерами)
- Изменить: `crates/coven-cli/src/tui/chat/events.rs` (заменить placeholder циклом событий)
- Изменить: `crates/coven-cli/src/chat.rs` (свести к шиму реэкспорта)

- [ ] **Шаг 1: cd в worktree и проверка ветки**

```bash
cd /Users/buns/Documents/GitHub/OpenCoven/coven/.worktrees/feat-tui-chat-module
git rev-parse --abbrev-ref HEAD
```

Ожидается: `feat/tui-chat-module`. Иначе ОСТАНОВИТЬСЯ.

- [ ] **Шаг 2: Наполнить `tui/chat/app.rs`**

Открыть `crates/coven-cli/src/chat.rs` и скопировать следующие диапазоны (номера строк относятся к **текущему** chat.rs на момент коммита `9bcb69a`):

- Строки 33–85 (типы данных: `MessageRole`, `ChatMessage`, `AgentInfo`, `InputMode`, `SlashCommandResult`, `App`)
- Строка 86 (константа `SPINNER_FRAMES`)
- Строки 88–457 (блок `impl App`)
- Строки 459–471 (`fn discover_agents`)
- Строки 990–992 (`fn timestamp_now`)
- Строки 994–1002 (`fn truncate_str`)
- Строки 1004–1111 (весь блок `#[cfg(test)] mod tests`)

Заменить placeholder в `crates/coven-cli/src/tui/chat/app.rs` этим содержимым в таком порядке:

1. Док-комментарий в начале файла + операторы use (заменить импорты из chat.rs только тем, что нужно app.rs):

```rust
//! Chat application state, behavior, and helpers. Owns `App` and all of its
//! methods; provides `discover_agents` and the spinner-frame data.

use crate::harness;
```

2. Типы данных из строк 33–69 файла chat.rs. **Изменения видимости (из спецификации):**
   - `pub enum MessageRole` → оставить `pub` (реэкспортирован через mod.rs на следующем шаге)
   - `pub struct ChatMessage` → оставить `pub`
   - `pub struct AgentInfo` → оставить `pub`
   - `enum InputMode` → без изменений (приватный, остаётся `enum`)
   - `enum SlashCommandResult` → без изменений (приватный)

3. `struct App` (строки 71–85): изменить видимость с приватной на `pub(super)`:

```rust
pub(super) struct App {
    // ... unchanged fields ...
}
```

4. `const SPINNER_FRAMES: &[&str] = ...` (строка 86): изменить на `pub(super)`:

```rust
pub(super) const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
```
(Или скопировать точные глифы из chat.rs:86 — кадры спиннера — это те же символы шаблона Брайля.)

5. `impl App` (строки 88–457): вставить без изменений.

6. `fn discover_agents` (строки 459–471): изменить на `pub(super)`:

```rust
pub(super) fn discover_agents() -> Vec<AgentInfo> {
    // ... unchanged body ...
}
```

7. `fn timestamp_now` (строки 990–992): оставить приватной:

```rust
fn timestamp_now() -> String {
    // ... unchanged body ...
}
```

8. `fn truncate_str` (строки 994–1002): оставить приватной:

```rust
fn truncate_str(s: &str, max: usize) -> &str {
    // ... unchanged body ...
}
```

9. Модуль тестов (строки 1004–1111) — вставить с такими точечными изменениями:
   - **Удалить** тест `chat_module_stays_single_file_to_avoid_rust_module_ambiguity` (строки 1035–1059).
   - **Удалить** импорт `use std::path::Path;` внутри `mod tests` (только этот тест его использовал; другие нет).
   - Сохранить все четыре поведенческих теста (`unknown_slash_command_returns_command_name_for_feedback`, `handle_input_clears_unknown_slash_command_and_reports_it`, `agent_command_without_argument_opens_picker_on_active_agent`, `unavailable_agent_selection_keeps_current_active_agent`) и оба хелпера (`app_with_agents`, `agent`) без изменений.

- [ ] **Шаг 3: Наполнить `tui/chat/render.rs`**

Скопировать следующие диапазоны из `chat.rs` в новый `render.rs`:

- Строки 473–510 (`fn render_ui`)
- Строки 512–538 (`fn render_status_bar`)
- Строки 540–636 (`fn render_messages`)
- Строки 638–672 (`fn render_input`)
- Строки 674–700 (`fn render_hint_bar`)
- Строки 702–779 (`fn render_help_overlay`)
- Строки 781–838 (`fn render_agent_select`)

Заменить placeholder в `render.rs` на:

1. Док-комментарий в начале файла + импорты. Рендерерам нужны типы ratatui и модуль theme:

```rust
//! Chat TUI render functions. Pure view code; reads `App` state and emits
//! ratatui widgets. The entry point is `render_ui`; the other render_* fns
//! are private helpers it composes.

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

2. Изменить `fn render_ui` на `pub(super) fn render_ui` (вызывается `events.rs` далее):

```rust
pub(super) fn render_ui(f: &mut Frame, app: &mut App) {
    // ... unchanged body ...
}
```

3. Все остальные функции `render_*` остаются приватными (`fn`, не `pub`). Вставить их без изменений.

- [ ] **Шаг 4: Наполнить `tui/chat/events.rs`**

Скопировать строки 863–988 из `chat.rs` (функцию `run_event_loop`) в `events.rs`.

Заменить placeholder на:

```rust
//! Chat TUI event loop. Reads keyboard events via crossterm and dispatches
//! to `App` methods; calls `render_ui` between events.

use std::io::Stdout;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::{Terminal, backend::CrosstermBackend};

use super::app::{App, SlashCommandResult};
use super::render::render_ui;
```

Затем вставить `run_event_loop` с таким изменением сигнатуры:

```rust
pub(super) fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
) -> Result<()> {
    // ... unchanged body ...
}
```

(Сегодняшняя сигнатура в chat.rs строка 863 начинает тело с параметров `terminal:` и `app:` — сохранить их.)

- [ ] **Шаг 5: Наполнить `tui/chat/mod.rs`**

Заменить заглушку, созданную в Задаче 1, реальным содержимым. mod.rs содержит `run_chat` и публичные реэкспорты.

```rust
//! Ratatui-based chat TUI. State lives in `app`, view in `render`, event loop
//! in `events`. The entry point `run_chat` manages the raw-terminal lifecycle.

mod app;
mod events;
mod render;

// Re-export the public types so callers see them at `tui::chat::*` instead of
// having to reach into `tui::chat::app::*`. Matches the surface of the old
// `chat::*` module from before the carve-out.
pub use app::{AgentInfo, ChatMessage, MessageRole};

use std::io::stdout;

use anyhow::Result;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

use app::App;
use events::run_event_loop;
```

Затем вставить тело `pub fn run_chat()` из chat.rs строк 840–861 без изменений. Тело вызывает `App::new()` и `run_event_loop(...)` — оба импортированы в блоке `use` выше, поэтому правки тела не требуются.

Удалить `#![allow(dead_code)]` из верха mod.rs, который был добавлен в Задаче 1.

- [ ] **Шаг 6: Заменить `chat.rs` шимом реэкспорта**

Заменить всё содержимое `crates/coven-cli/src/chat.rs` (1111 строк) на эти 3 строки:

```rust
//! Temporary re-export shim during the Phase 2 carve-out. Removed in Task 3
//! of the chat-module plan; do not add new content here.

pub use crate::tui::chat::*;
```

Это сохраняет работоспособность точки вызова `chat::run_chat()` в `main.rs` (теперь она разрешается в `tui::chat::run_chat` через глоб-реэкспорт). Шим удаляется в Задаче 3.

- [ ] **Шаг 7: Проверить, что крейт собирается**

```bash
cargo build -p coven-cli 2>&1 | tail -30
```

Ожидается: собирается чисто без ошибок. Могут остаться некоторые предупреждения (например, «unused import», если `use` теперь избыточен). Если вы видите ошибки, наиболее вероятная причина:

- Элемент `pub(super)`, которому нужен `pub` для реэкспорта через шим. Шим chat.rs `pub use crate::tui::chat::*;` реэкспортирует только `pub` элементы, не `pub(super)`. Элементы `run_chat`, `MessageRole`, `ChatMessage`, `AgentInfo` должны быть `pub` в `tui::chat::*`, чтобы шим их нашёл.
- Отсутствующий импорт в одном из новых файлов. Сверьте раздел импортов в каждом файле с тем, что перечислено в спецификации.

- [ ] **Шаг 8: Запустить все тесты**

```bash
cargo test -p coven-cli 2>&1 | tail -10
```

Ожидается: **175 unit-тестов** + 4 smoke-теста проходят (на один unit-тест меньше, чем в начале Задачи 2 — удалённый страж).

Если тест падает, наиболее вероятная причина — файл тестов больше не компилируется (приватный доступ к полям `App` раньше был легален, но теперь требует, чтобы тест был в том же файле, что и `App` — он там, в `app.rs`, так что это должно работать).

- [ ] **Шаг 9: Коммит**

```bash
git add crates/coven-cli/src/tui/ crates/coven-cli/src/chat.rs
git commit -m "refactor(tui): move chat.rs content into tui/chat/* submodule

Pure code motion. chat.rs becomes a re-export shim that points at
crate::tui::chat::* so main.rs's existing chat::run_chat() call keeps
working. The shim and the old mod chat; declaration get deleted in
Task 3 along with the guardrail test (which fails as soon as chat.rs
is removed).
"
```

- [ ] **Шаг 10: Проверить коммит на правильной ветке**

```bash
git log --oneline -3
git rev-parse --abbrev-ref HEAD
```

Ожидается: новый коммит сверху, HEAD = `feat/tui-chat-module`.

---

## Задача 3: Удалить `chat.rs` и обновить `main.rs`

После Задачи 2 `chat.rs` — просто шим реэкспорта. Эта задача удаляет его, убирает `mod chat;` из main.rs, обновляет точку вызова на `tui::chat::run_chat()` и проверяет финальные критерии приёмки.

**Файлы:**
- Удалить: `crates/coven-cli/src/chat.rs`
- Изменить: `crates/coven-cli/src/main.rs` (удалить `mod chat;`, обновить строку 150)

- [ ] **Шаг 1: cd в worktree, проверка ветки**

```bash
cd /Users/buns/Documents/GitHub/OpenCoven/coven/.worktrees/feat-tui-chat-module
git rev-parse --abbrev-ref HEAD
```

Ожидается: `feat/tui-chat-module`.

- [ ] **Шаг 2: Удалить `crates/coven-cli/src/chat.rs`**

```bash
rm crates/coven-cli/src/chat.rs
git status --short
```

Ожидается: показывает `D  crates/coven-cli/src/chat.rs`.

- [ ] **Шаг 3: Обновить `main.rs` — удалить `mod chat;`**

В `crates/coven-cli/src/main.rs` найти блок объявлений `mod` (около строк 21–35). Удалить строку `mod chat;`. Оставшийся блок mod должен выглядеть так:

```rust
mod api;
mod control_plane;
mod daemon;
mod harness;
mod openclaw_repo;
mod patch;
mod pc;
mod project;
mod pty_runner;
mod store;
mod theme;
mod tui;
mod verification;
```

(Заметка: `mod chat;` изначально находился между `mod api;` и `mod control_plane;`.)

- [ ] **Шаг 4: Обновить `main.rs` — изменить точку вызова chat**

В `main.rs` найти строку 150 (приблизительно — точная строка смещается при удалении `mod chat;`):

```rust
Some(Command::Chat) => chat::run_chat(),
```

Заменить на:

```rust
Some(Command::Chat) => tui::chat::run_chat(),
```

Это **единственная** точка вызова в main.rs, использующая модуль chat. Подтвердить через grep:

```bash
grep -nE '\bchat::' crates/coven-cli/src/main.rs
```

Ожидаемый вывод: одна строка, новый `tui::chat::run_chat()`. Если вы видите дополнительные совпадения, их тоже нужно заменить.

- [ ] **Шаг 5: Проверить, что крейт собирается**

```bash
cargo build -p coven-cli 2>&1 | tail -20
```

Ожидается: собирается чисто с нулём предупреждений.

Если вы видите:
- «unresolved module `chat`» — вы пропустили Шаг 3 (удаление `mod chat;`) или Шаг 4 (обновление точки вызова). Повторно сделать grep.
- «file not found: chat.rs» — сборка всё ещё ищет chat.rs. Подтвердите, что `mod chat;` ушёл из main.rs.
- «function `run_chat` is private» — `run_chat` в `tui/chat/mod.rs` не `pub`. Проверьте содержимое `mod.rs` из Задачи 2; требуется сигнатура `pub fn run_chat`.

- [ ] **Шаг 6: Запустить все тесты**

```bash
cargo test -p coven-cli 2>&1 | tail -10
```

Ожидается: **175 unit-тестов + 4 smoke-теста проходят** (тот же счёт, что в Задаче 2).

- [ ] **Шаг 7: Запустить clippy**

```bash
cargo clippy -p coven-cli --no-deps 2>&1 | tail -10
```

Ожидается: ноль предупреждений (без регрессии относительно чистого состояния Фазы 1).

- [ ] **Шаг 8: Проверить критерии приёмки через проверки файловой системы**

```bash
# Criterion 1: chat.rs is gone
test -e crates/coven-cli/src/chat.rs && echo "FAIL: chat.rs still exists" || echo "ok: chat.rs deleted"

# Criterion 2: tui/mod.rs exists with the expected content
cat crates/coven-cli/src/tui/mod.rs

# Criterion 3: tui/chat/ has exactly 4 .rs files
ls crates/coven-cli/src/tui/chat/

# Criterion 4: main.rs uses tui::chat::run_chat
grep -nE 'tui::chat::run_chat|chat::run_chat' crates/coven-cli/src/main.rs
```

Ожидается:
- `ok: chat.rs deleted`
- `tui/mod.rs` показывает док-комментарий + `pub mod chat;`
- `ls` показывает ровно `mod.rs  app.rs  render.rs  events.rs` (4 файла, без лишних)
- Последний grep показывает одну строку с `tui::chat::run_chat()`

- [ ] **Шаг 9: Проверить, что удалённый тест-страж исчез**

```bash
grep -rn 'chat_module_stays_single_file' crates/coven-cli/src/ 2>&1 || echo "ok: guardrail test deleted"
```

Ожидается: `ok: guardrail test deleted`. Если что-то совпадает, страж всё ещё где-то существует (он должен был быть удалён при копировании тестов в `app.rs` на Шаге 2 Задачи 2). Удалите его сейчас и перезапустите.

- [ ] **Шаг 10: Коммит**

```bash
git add crates/coven-cli/src/main.rs crates/coven-cli/src/chat.rs
git commit -m "refactor(tui): delete chat.rs shim and finalize chat carve-out

Removes the re-export shim from Task 2, drops mod chat; from main.rs,
and points the Chat command at tui::chat::run_chat() directly. The
guardrail test (which previously prevented this split) was removed in
Task 2 when its containing module file was rewritten.

Acceptance criteria from the design spec all met:
- src/chat.rs deleted
- src/tui/chat/ has exactly mod.rs, app.rs, render.rs, events.rs
- 175 unit + 4 smoke tests pass
- cargo clippy clean
"
```

- [ ] **Шаг 11: Проверить финальное состояние**

```bash
git log --oneline -4
git rev-parse --abbrev-ref HEAD
git status --short
```

Ожидается: 3 новых коммита поверх вершины Фазы 1 (`9bcb69a`):
```
<sha3> refactor(tui): delete chat.rs shim and finalize chat carve-out
<sha2> refactor(tui): move chat.rs content into tui/chat/* submodule
<sha1> refactor(tui): scaffold tui/chat module structure
9bcb69a chore(theme): silence dead-code warnings for future-use tokens
```

Ветка `feat/tui-chat-module`. Статус чистый (без незакоммиченных изменений).

---

## Готово

Когда Задача 3 завершена, выполнен каждый критерий приёмки из спецификации:

1. ✅ `src/chat.rs` больше не существует — Задача 3 Шаг 2.
2. ✅ `src/tui/mod.rs` существует с `pub mod chat;` — Задача 1 Шаг 2.
3. ✅ `src/tui/chat/` содержит ровно `mod.rs`, `app.rs`, `render.rs`, `events.rs` — Задачи 1–2.
4. ✅ `src/main.rs` имеет `mod tui;` и `tui::chat::run_chat()` — Задачи 1 + 3.
5. ✅ `cargo build -p coven-cli` успешно завершается чисто — Задача 3 Шаг 5.
6. ✅ `cargo test -p coven-cli` проходит; счётчик unit падает ровно на один — Задача 3 Шаг 6.
7. ✅ `cargo clippy -p coven-cli --no-deps` выдаёт ноль предупреждений — Задача 3 Шаг 7.
8. ✅ Ни один элемент не получил новой экспозиции за пределами сегодняшней поверхности — правила видимости Задач 2–3.
9. ⏳ Вручную: запуск `coven chat` показывает ту же TUI, что и раньше. Не автоматизируется; проверить на глаз, если удобно.

После Задачи 3 запушить в origin и открыть PR, накладывающийся на #56.
