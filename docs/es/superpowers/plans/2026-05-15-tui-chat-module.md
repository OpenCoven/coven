# Plan de implementación de la extracción del módulo de chat TUI

> **Para trabajadores agénticos:** SUBHABILIDAD REQUERIDA: Usa superpowers:subagent-driven-development (recomendado) o superpowers:executing-plans para implementar este plan tarea por tarea. Los pasos usan sintaxis de checkbox (`- [ ]`) para el seguimiento.

**Objetivo:** Dividir `crates/coven-cli/src/chat.rs` (1111 líneas) en un módulo de 4 archivos bajo un nuevo espacio de nombres `tui/`, con cero cambios de comportamiento.

**Arquitectura:** Movimiento puro de código. Tres commits secuenciales: (1) andamiar archivos nuevos vacíos + cablear `mod tui;` en main.rs, (2) mover todo el contenido desde `chat.rs` hacia los nuevos archivos mientras se hace de `chat.rs` un shim de reexportación, (3) eliminar `chat.rs` + la salvaguarda y actualizar el callsite de `main.rs`.

**Stack tecnológico:** Rust edición 2021. Sin nuevas dependencias. Las mismas ratatui 0.30 / crossterm 0.29 que en la Fase 1.

**Especificación:** [`docs/superpowers/specs/2026-05-15-tui-chat-module-design.md`](../specs/2026-05-15-tui-chat-module-design.md)

**Rama:** `feat/tui-chat-module`, apilada sobre `feat/tui-theme-module`. El PR no puede fusionarse hasta que aterrice #56.

**Worktree:** `/Users/buns/Documents/GitHub/OpenCoven/coven/.worktrees/feat-tui-chat-module`

---

## Mapa de archivos

| Archivo | Acción | Notas |
|---|---|---|
| `crates/coven-cli/src/tui/mod.rs` | **Crear** (~10 líneas) | Doc a nivel de módulo + `pub mod chat;` |
| `crates/coven-cli/src/tui/chat/mod.rs` | **Crear** (~40 líneas) | `pub fn run_chat` + reexportaciones de `MessageRole`/`ChatMessage`/`AgentInfo` |
| `crates/coven-cli/src/tui/chat/app.rs` | **Crear** (~530 líneas) | Todo el estado, comportamiento, helpers, tests |
| `crates/coven-cli/src/tui/chat/render.rs` | **Crear** (~380 líneas) | Las 7 funciones `render_*` |
| `crates/coven-cli/src/tui/chat/events.rs` | **Crear** (~150 líneas) | `run_event_loop` |
| `crates/coven-cli/src/chat.rs` | **Eliminar** (actualmente 1111 líneas) | Reemplazado por el módulo de arriba |
| `crates/coven-cli/src/main.rs` | **Modificar** (~2 líneas) | `mod chat;` → `mod tui;` (reordenamiento alfabético); `chat::run_chat()` → `tui::chat::run_chat()` |

Ningún otro archivo cambia. No se añaden tests; se elimina uno (la salvaguarda).

---

## Nota crítica sobre el directorio de trabajo

TODOS los comandos `cd`, `cargo` y `git` de este plan se ejecutan desde:

```
/Users/buns/Documents/GitHub/OpenCoven/coven/.worktrees/feat-tui-chat-module
```

La primera acción de cada tarea es hacer `cd` allí y verificar que `git rev-parse --abbrev-ref HEAD` es `feat/tui-chat-module`. De lo contrario PARAR y reportar BLOQUEADO. (Esta es la lección de la Fase 1, donde algunos subagentes implementadores escribieron en el checkout principal por accidente.)

---

## Tarea 1: Andamiar la nueva estructura del módulo

Crear archivos vacíos/esqueleto para el nuevo módulo y cablearlo en `main.rs`. Tras esta tarea, tanto `mod chat;` (apuntando al viejo `chat.rs`) como `mod tui;` (apuntando al nuevo módulo casi vacío) coexisten. La build pasa con advertencias sobre elementos no usados en los archivos nuevos.

**Archivos:**
- Crear: `crates/coven-cli/src/tui/mod.rs`
- Crear: `crates/coven-cli/src/tui/chat/mod.rs`
- Crear: `crates/coven-cli/src/tui/chat/app.rs`
- Crear: `crates/coven-cli/src/tui/chat/render.rs`
- Crear: `crates/coven-cli/src/tui/chat/events.rs`
- Modificar: `crates/coven-cli/src/main.rs` (añadir la declaración `mod tui;`)

- [ ] **Paso 1: cd al worktree y verificar la rama**

```bash
cd /Users/buns/Documents/GitHub/OpenCoven/coven/.worktrees/feat-tui-chat-module
pwd
git rev-parse --abbrev-ref HEAD
```

Esperado:
```
/Users/buns/Documents/GitHub/OpenCoven/coven/.worktrees/feat-tui-chat-module
feat/tui-chat-module
```

Si alguno difiere, PARAR y reportar BLOQUEADO. No modificar archivos fuera de este worktree.

- [ ] **Paso 2: Crear `crates/coven-cli/src/tui/mod.rs`**

Escribir este contenido exacto:

```rust
//! TUI surfaces for the coven CLI. Currently hosts the chat module; Phases 3–4
//! will land the launcher and session-browser carve-outs from main.rs here.

pub mod chat;
```

- [ ] **Paso 3: Crear `crates/coven-cli/src/tui/chat/mod.rs` como un stub temporal**

Este archivo es un stub para la Tarea 1. Se rellenará con `run_chat` y las reexportaciones en la Tarea 2. Por ahora debe compilar sin advertencias aunque nada referencie sus submódulos todavía.

Escribir este contenido exacto:

```rust
//! Ratatui-based chat TUI. State lives in `app`, view in `render`, event loop
//! in `events`. The entry point `run_chat` here manages the raw-terminal
//! lifecycle.

#![allow(dead_code)]

mod app;
mod events;
mod render;
```

El `#![allow(dead_code)]` es temporal — se elimina en el Paso 5 de la Tarea 2 cuando `run_chat` aterrice aquí y consuma los submódulos. Los submódulos se declaran privados (`mod`, no `pub mod`) porque ningún código fuera de `tui::chat` necesita acceder a `tui::chat::app::*`.

- [ ] **Paso 4: Crear tres archivos de submódulo vacíos**

Cada uno debe ser Rust válido que compile por sí solo. Escribir cada archivo con solo un comentario de documentación y una línea `// placeholder` (reemplazada en la Tarea 2):

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

- [ ] **Paso 5: Añadir `mod tui;` a main.rs**

Encontrar este bloque en `crates/coven-cli/src/main.rs` (alrededor de las líneas 31–33 tras la inserción de `mod theme;` de la Fase 1):

```rust
mod store;
mod theme;
mod verification;
```

Insertar `mod tui;` alfabéticamente entre `theme` y `verification`:

```rust
mod store;
mod theme;
mod tui;
mod verification;
```

NO eliminar `mod chat;` todavía (la Tarea 3 se encarga de eso). Ambos módulos coexisten tras la Tarea 1.

- [ ] **Paso 6: Verificar que el crate compila**

```bash
cargo build -p coven-cli 2>&1 | tail -20
```

Esperado: compila limpiamente. Algunas advertencias "unused import" sobre `crate::tui::chat` o sus submódulos son aceptables en la Tarea 1 — esas se consumirán en la Tarea 3.

Si ves errores reales (no advertencias), PARAR y reportar BLOQUEADO con el texto del error.

- [ ] **Paso 7: Ejecutar todos los tests**

```bash
cargo test -p coven-cli 2>&1 | tail -10
```

Esperado: todos los tests existentes pasan. El módulo de chat (todavía en `src/chat.rs`) y sus tests están intactos. Conteo de tests: 172 unitarios + 4 smoke = 176 (igual que al final de la Fase 1).

- [ ] **Paso 8: Commit**

```bash
git add crates/coven-cli/src/tui/ crates/coven-cli/src/main.rs
git commit -m "refactor(tui): scaffold tui/chat module structure

Empty submodule skeleton for the chat carve-out. Old chat.rs remains
the active implementation; this commit only adds the new file tree and
wires mod tui; into main.rs. Task 2 of the chat-module plan moves the
content; Task 3 deletes the old file.
"
```

- [ ] **Paso 9: Verificar que el commit aterrizó en la rama correcta**

```bash
git log --oneline -2
git rev-parse --abbrev-ref HEAD
```

Esperado: el nuevo commit está encima, y HEAD está en `feat/tui-chat-module`. Si no, PARAR y reportar.

---

## Tarea 2: Mover todo el contenido de `chat.rs` a los nuevos archivos del módulo

Esta es la mayor parte del trabajo. La estrategia: copiar cada sección del viejo `chat.rs` a su archivo nuevo de destino, arreglar imports + visibilidad, luego reemplazar `chat.rs` con un shim de reexportación (`pub use crate::tui::chat::*;`) para que el viejo callsite `chat::run_chat()` en `main.rs` siga funcionando durante la Tarea 2. La Tarea 3 elimina el shim y actualiza el callsite.

**Archivos:**
- Modificar: `crates/coven-cli/src/tui/chat/mod.rs` (reemplazar stub con run_chat + reexportaciones)
- Modificar: `crates/coven-cli/src/tui/chat/app.rs` (reemplazar placeholder con código de estado)
- Modificar: `crates/coven-cli/src/tui/chat/render.rs` (reemplazar placeholder con renderizadores)
- Modificar: `crates/coven-cli/src/tui/chat/events.rs` (reemplazar placeholder con bucle de eventos)
- Modificar: `crates/coven-cli/src/chat.rs` (reducir a un shim de reexportación)

- [ ] **Paso 1: cd al worktree y verificar la rama**

```bash
cd /Users/buns/Documents/GitHub/OpenCoven/coven/.worktrees/feat-tui-chat-module
git rev-parse --abbrev-ref HEAD
```

Esperado: `feat/tui-chat-module`. Si no, PARAR.

- [ ] **Paso 2: Poblar `tui/chat/app.rs`**

Abrir `crates/coven-cli/src/chat.rs` y copiar los siguientes rangos (los números de línea se refieren al chat.rs **actual** al momento del commit `9bcb69a`):

- Líneas 33–85 (los tipos de datos: `MessageRole`, `ChatMessage`, `AgentInfo`, `InputMode`, `SlashCommandResult`, `App`)
- Línea 86 (la constante `SPINNER_FRAMES`)
- Líneas 88–457 (el bloque `impl App`)
- Líneas 459–471 (`fn discover_agents`)
- Líneas 990–992 (`fn timestamp_now`)
- Líneas 994–1002 (`fn truncate_str`)
- Líneas 1004–1111 (todo el bloque `#[cfg(test)] mod tests`)

Reemplazar el placeholder en `crates/coven-cli/src/tui/chat/app.rs` con este contenido, en este orden:

1. Comentario de documentación al inicio del archivo + sentencias use (reemplazar los imports de chat.rs por solo lo que app.rs necesita):

```rust
//! Chat application state, behavior, and helpers. Owns `App` and all of its
//! methods; provides `discover_agents` and the spinner-frame data.

use crate::harness;
```

2. Los tipos de datos de las líneas 33–69 de chat.rs. **Cambios de visibilidad (según la especificación):**
   - `pub enum MessageRole` → mantener `pub` (reexportado vía mod.rs en el siguiente paso)
   - `pub struct ChatMessage` → mantener `pub`
   - `pub struct AgentInfo` → mantener `pub`
   - `enum InputMode` → sin cambios (privado, permanece como `enum`)
   - `enum SlashCommandResult` → sin cambios (privado)

3. `struct App` (líneas 71–85): cambiar la visibilidad de privada a `pub(super)`:

```rust
pub(super) struct App {
    // ... unchanged fields ...
}
```

4. `const SPINNER_FRAMES: &[&str] = ...` (línea 86): cambiar a `pub(super)`:

```rust
pub(super) const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
```
(O copiar los glifos exactos de chat.rs:86 — los frames del spinner son los mismos caracteres del patrón Braille.)

5. `impl App` (líneas 88–457): pegar sin cambios.

6. `fn discover_agents` (líneas 459–471): cambiar a `pub(super)`:

```rust
pub(super) fn discover_agents() -> Vec<AgentInfo> {
    // ... unchanged body ...
}
```

7. `fn timestamp_now` (líneas 990–992): mantener privado:

```rust
fn timestamp_now() -> String {
    // ... unchanged body ...
}
```

8. `fn truncate_str` (líneas 994–1002): mantener privado:

```rust
fn truncate_str(s: &str, max: usize) -> &str {
    // ... unchanged body ...
}
```

9. El módulo de tests (líneas 1004–1111) — pegar con estos cambios quirúrgicos:
   - **Eliminar** el test `chat_module_stays_single_file_to_avoid_rust_module_ambiguity` (líneas 1035–1059).
   - **Eliminar** el import `use std::path::Path;` dentro de `mod tests` (solo ese test lo usaba; los demás no).
   - Mantener los cuatro tests de comportamiento (`unknown_slash_command_returns_command_name_for_feedback`, `handle_input_clears_unknown_slash_command_and_reports_it`, `agent_command_without_argument_opens_picker_on_active_agent`, `unavailable_agent_selection_keeps_current_active_agent`) y ambos helpers (`app_with_agents`, `agent`) sin cambios.

- [ ] **Paso 3: Poblar `tui/chat/render.rs`**

Copiar los siguientes rangos desde `chat.rs` al nuevo `render.rs`:

- Líneas 473–510 (`fn render_ui`)
- Líneas 512–538 (`fn render_status_bar`)
- Líneas 540–636 (`fn render_messages`)
- Líneas 638–672 (`fn render_input`)
- Líneas 674–700 (`fn render_hint_bar`)
- Líneas 702–779 (`fn render_help_overlay`)
- Líneas 781–838 (`fn render_agent_select`)

Reemplazar el placeholder en `render.rs` con:

1. Comentario de documentación al inicio del archivo + imports. Los renderizadores necesitan tipos de ratatui y el módulo theme:

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

2. Cambiar `fn render_ui` a `pub(super) fn render_ui` (llamado por `events.rs` a continuación):

```rust
pub(super) fn render_ui(f: &mut Frame, app: &mut App) {
    // ... unchanged body ...
}
```

3. Todas las demás funciones `render_*` permanecen privadas (`fn`, no `pub`). Pegarlas sin cambios.

- [ ] **Paso 4: Poblar `tui/chat/events.rs`**

Copiar las líneas 863–988 desde `chat.rs` (la función `run_event_loop`) a `events.rs`.

Reemplazar el placeholder con:

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

Luego pegar `run_event_loop` con este cambio de firma:

```rust
pub(super) fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
) -> Result<()> {
    // ... unchanged body ...
}
```

(La firma actual en chat.rs línea 863 empieza el cuerpo con los parámetros `terminal:` y `app:` — mantenerlos.)

- [ ] **Paso 5: Poblar `tui/chat/mod.rs`**

Reemplazar el stub creado en la Tarea 1 con el contenido real. El mod.rs contiene `run_chat` y las reexportaciones públicas.

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

Luego pegar el cuerpo de `pub fn run_chat()` desde chat.rs líneas 840–861 sin cambios. El cuerpo llama a `App::new()` y `run_event_loop(...)` — ambos están importados en el bloque `use` de arriba, por lo que no se necesitan ediciones del cuerpo.

Eliminar el `#![allow(dead_code)]` de la parte superior de mod.rs que se añadió en la Tarea 1.

- [ ] **Paso 6: Reemplazar `chat.rs` con un shim de reexportación**

Reemplazar todo el contenido de `crates/coven-cli/src/chat.rs` (1111 líneas) por estas 3 líneas:

```rust
//! Temporary re-export shim during the Phase 2 carve-out. Removed in Task 3
//! of the chat-module plan; do not add new content here.

pub use crate::tui::chat::*;
```

Esto mantiene funcionando el callsite `chat::run_chat()` de `main.rs` (ahora resuelve a `tui::chat::run_chat` a través de la reexportación con glob). El shim se elimina en la Tarea 3.

- [ ] **Paso 7: Verificar que el crate compila**

```bash
cargo build -p coven-cli 2>&1 | tail -30
```

Esperado: compila limpiamente sin errores. Pueden quedar algunas advertencias (p. ej., "unused import" si un `use` ahora es redundante). Si ves errores, la causa más probable es:

- Un elemento `pub(super)` que necesita `pub` para la reexportación del shim. El shim de chat.rs `pub use crate::tui::chat::*;` reexporta solo elementos `pub`, no `pub(super)`. Los elementos `run_chat`, `MessageRole`, `ChatMessage`, `AgentInfo` deben ser `pub` en `tui::chat::*` para que el shim los encuentre.
- Un import faltante en uno de los archivos nuevos. Contrastar la sección de imports en cada archivo con lo que lista la especificación.

- [ ] **Paso 8: Ejecutar todos los tests**

```bash
cargo test -p coven-cli 2>&1 | tail -10
```

Esperado: **175 tests unitarios** + 4 smoke tests pasan (un test unitario menos que al inicio de la Tarea 2 — la salvaguarda eliminada).

Si un test falla, la causa más probable es que el archivo de tests ya no compila (el acceso privado a campos de `App` antes era legal pero ahora requiere que el test esté en el mismo archivo que `App` — lo está, en `app.rs`, así que esto debería funcionar).

- [ ] **Paso 9: Commit**

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

- [ ] **Paso 10: Verificar el commit en la rama correcta**

```bash
git log --oneline -3
git rev-parse --abbrev-ref HEAD
```

Esperado: nuevo commit encima, HEAD = `feat/tui-chat-module`.

---

## Tarea 3: Eliminar `chat.rs` y actualizar `main.rs`

Tras la Tarea 2, `chat.rs` es solo un shim de reexportación. Esta tarea lo elimina, quita `mod chat;` de main.rs, actualiza el callsite a `tui::chat::run_chat()` y verifica los criterios de aceptación finales.

**Archivos:**
- Eliminar: `crates/coven-cli/src/chat.rs`
- Modificar: `crates/coven-cli/src/main.rs` (eliminar `mod chat;`, actualizar línea 150)

- [ ] **Paso 1: cd al worktree, verificar rama**

```bash
cd /Users/buns/Documents/GitHub/OpenCoven/coven/.worktrees/feat-tui-chat-module
git rev-parse --abbrev-ref HEAD
```

Esperado: `feat/tui-chat-module`.

- [ ] **Paso 2: Eliminar `crates/coven-cli/src/chat.rs`**

```bash
rm crates/coven-cli/src/chat.rs
git status --short
```

Esperado: muestra `D  crates/coven-cli/src/chat.rs`.

- [ ] **Paso 3: Actualizar `main.rs` — eliminar `mod chat;`**

En `crates/coven-cli/src/main.rs`, encontrar el bloque de declaraciones `mod` (alrededor de las líneas 21–35). Eliminar la línea `mod chat;`. El bloque mod restante debería verse así:

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

(Nota: `mod chat;` estaba originalmente entre `mod api;` y `mod control_plane;`.)

- [ ] **Paso 4: Actualizar `main.rs` — cambiar el callsite del chat**

En `main.rs` encontrar la línea 150 (aproximada — la línea exacta se desplaza cuando se elimina `mod chat;`):

```rust
Some(Command::Chat) => chat::run_chat(),
```

Reemplazar con:

```rust
Some(Command::Chat) => tui::chat::run_chat(),
```

Este es el **único** callsite en main.rs que usa el módulo de chat. Grep para confirmar:

```bash
grep -nE '\bchat::' crates/coven-cli/src/main.rs
```

Salida esperada: una línea, el nuevo `tui::chat::run_chat()`. Si ves coincidencias adicionales, también necesitan ser reemplazadas.

- [ ] **Paso 5: Verificar que el crate compila**

```bash
cargo build -p coven-cli 2>&1 | tail -20
```

Esperado: compila limpiamente con cero advertencias.

Si ves:
- "unresolved module `chat`" — te saltaste el Paso 3 (la eliminación de `mod chat;`) o el Paso 4 (la actualización del callsite). Volver a hacer grep.
- "file not found: chat.rs" — la build todavía está buscando chat.rs. Confirmar que `mod chat;` ya no está en main.rs.
- "function `run_chat` is private" — `run_chat` en `tui/chat/mod.rs` no es `pub`. Revisar el contenido de `mod.rs` de la Tarea 2; se requiere la firma `pub fn run_chat`.

- [ ] **Paso 6: Ejecutar todos los tests**

```bash
cargo test -p coven-cli 2>&1 | tail -10
```

Esperado: **175 tests unitarios + 4 smoke tests pasan** (igual que el conteo de la Tarea 2).

- [ ] **Paso 7: Ejecutar clippy**

```bash
cargo clippy -p coven-cli --no-deps 2>&1 | tail -10
```

Esperado: cero advertencias (sin regresión del estado limpio de la Fase 1).

- [ ] **Paso 8: Verificar los criterios de aceptación mediante comprobaciones del sistema de archivos**

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

Esperado:
- `ok: chat.rs deleted`
- `tui/mod.rs` muestra el comentario de documentación + `pub mod chat;`
- `ls` muestra exactamente `mod.rs  app.rs  render.rs  events.rs` (4 archivos, sin extras)
- El último grep muestra una línea, con `tui::chat::run_chat()`

- [ ] **Paso 9: Verificar que el test de salvaguarda eliminado se haya ido**

```bash
grep -rn 'chat_module_stays_single_file' crates/coven-cli/src/ 2>&1 || echo "ok: guardrail test deleted"
```

Esperado: `ok: guardrail test deleted`. Si algo coincide, la salvaguarda todavía existe en algún lugar (debería haber sido eliminada al copiar los tests a `app.rs` en el Paso 2 de la Tarea 2). Eliminarla ahora y volver a ejecutar.

- [ ] **Paso 10: Commit**

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

- [ ] **Paso 11: Verificar el estado final**

```bash
git log --oneline -4
git rev-parse --abbrev-ref HEAD
git status --short
```

Esperado: 3 commits nuevos encima del tip de la Fase 1 (`9bcb69a`):
```
<sha3> refactor(tui): delete chat.rs shim and finalize chat carve-out
<sha2> refactor(tui): move chat.rs content into tui/chat/* submodule
<sha1> refactor(tui): scaffold tui/chat module structure
9bcb69a chore(theme): silence dead-code warnings for future-use tokens
```

La rama es `feat/tui-chat-module`. El status está limpio (sin cambios sin commitear).

---

## Hecho

Cuando se complete la Tarea 3, cada criterio de aceptación de la especificación se cumple:

1. ✅ `src/chat.rs` ya no existe — Tarea 3 Paso 2.
2. ✅ `src/tui/mod.rs` existe con `pub mod chat;` — Tarea 1 Paso 2.
3. ✅ `src/tui/chat/` contiene exactamente `mod.rs`, `app.rs`, `render.rs`, `events.rs` — Tareas 1–2.
4. ✅ `src/main.rs` tiene `mod tui;` y `tui::chat::run_chat()` — Tareas 1 + 3.
5. ✅ `cargo build -p coven-cli` tiene éxito limpiamente — Tarea 3 Paso 5.
6. ✅ `cargo test -p coven-cli` pasa; el conteo unitario baja exactamente en uno — Tarea 3 Paso 6.
7. ✅ `cargo clippy -p coven-cli --no-deps` produce cero advertencias — Tarea 3 Paso 7.
8. ✅ Ningún elemento expuesto de forma nueva más allá de la superficie de hoy — Reglas de visibilidad de las Tareas 2–3.
9. ⏳ Manual: lanzar `coven chat` muestra la misma TUI que antes. No automatizable; verificar a ojo si es conveniente.

Tras la Tarea 3, hacer push a origin y abrir un PR apilado sobre #56.
