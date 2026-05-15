# Extracción del módulo de chat TUI — Diseño

**Estado:** Aprobado — listo para el plan de implementación
**Fecha:** 2026-05-15
**Alcance:** Fase 2 del esfuerzo de limpieza estructural de la TUI. Apilado sobre la Fase 1 ([`feat/tui-theme-module`](https://github.com/OpenCoven/coven/pull/56)); no puede fusionarse hasta que esa aterrice.
**Enfoque:** Movimiento puro de código. Sin renombrados, sin cambios de firma, sin cambios de comportamiento.

---

## Problema

`crates/coven-cli/src/chat.rs` tiene 1111 líneas tras la migración de temas de la Fase 1. Es un único archivo que contiene los tipos de datos de la TUI de chat basada en ratatui, el estado de la aplicación, ~370 líneas de código de renderizado repartidas en 7 funciones de render, el bucle de eventos, el punto de entrada público y los tests. Las responsabilidades del archivo — modelo de estado, vista, controlador y ciclo de vida — están todas mezcladas.

Síntomas que motivan la división:

- El archivo es difícil de manejar para navegar y revisar. El código de render (líneas 473–838) es un único bloque contiguo.
- `impl App` (líneas 88–457) tiene 369 líneas por sí solo.
- Un test de regresión existente (`chat_module_stays_single_file_to_avoid_rust_module_ambiguity`, añadido en `fa786f1`) impide activamente la división — su eliminación es el disparador de este trabajo.

El nuevo módulo `crate::theme` que aterrizó en la Fase 1 ya demuestra el patrón que queremos para las superficies de TUI: una responsabilidad lógica por archivo, los puntos de llamada importan vía `use`.

## No-objetivos

Explícitamente fuera del alcance de la Fase 2 y no deben colarse:

- **Cambios de comportamiento** de cualquier tipo. Los renderizadores producen la misma salida. El bucle de eventos procesa las mismas teclas. La CLI se comporta de manera idéntica.
- **Ajuste de API.** `pub enum MessageRole`, `pub struct ChatMessage`, `pub struct AgentInfo` permanecen `pub` aunque ningún llamador fuera del módulo de chat los importe hoy. Ajustarlos a `pub(super)` es una preocupación aparte (candidato para un PR de seguimiento; ver Fase 2.1).
- **Extracción de helpers.** `render_messages` (el renderizador más grande con ~98 líneas) no se refactoriza. Las funciones helper internas no se extraen.
- **División de `main.rs`.** Fase 3 — fuera de alcance.
- **Extracción del launcher / explorador de sesiones.** Fase 4 — fuera de alcance. (Sí creamos el módulo padre `tui/` en previsión, pero solo `tui::chat` vive bajo él por ahora.)
- **Nuevos tests.** La Fase 2 hereda los tests existentes y elimina la salvaguarda. No se añaden nuevos tests de comportamiento.

## Restricciones

- **Ningún elemento del módulo de chat se expone de forma nueva más allá de `tui::chat::run_chat`.** La visibilidad se preserva exactamente como hoy (movimiento puro de código).
- **El crate sigue siendo un único binario.** Sin nuevos miembros del workspace, sin exposición de biblioteca.
- **`cargo clippy -p coven-cli --no-deps` produce cero advertencias**, preservando el estado limpio posterior a la Fase 1.

## Estructura del módulo

```
crates/coven-cli/src/
├── tui/
│   ├── mod.rs            (~10 líneas: `pub mod chat;`)
│   └── chat/
│       ├── mod.rs        (~40 líneas: pub fn run_chat + ciclo de vida de terminal en bruto)
│       ├── app.rs        (~530 líneas: estado, comportamiento, helpers, tests)
│       ├── render.rs     (~380 líneas: 7 funciones de render)
│       └── events.rs     (~150 líneas: bucle de eventos)
├── main.rs   (una edición: `mod chat;` → `mod tui;` y `chat::run_chat()` → `tui::chat::run_chat()`)
└── ... (otros archivos sin cambios)
```

`crates/coven-cli/src/chat.rs` se elimina por completo. El compilador de Rust impone la no coexistencia de `src/chat.rs` y `src/chat/mod.rs`, por lo que la eliminación de la forma de archivo único es obligatoria una vez que aterriza la forma de directorio. (Estamos usando la forma `src/tui/chat/`, no `src/chat/`, pero el principio es el mismo: no puede quedar ningún `src/chat.rs`.)

## Mapeo de contenido por archivo

### `tui/mod.rs` (nuevo)

```rust
//! TUI surfaces for the coven CLI. Currently hosts the chat module; Phases 3–4
//! will land the launcher and session-browser carve-outs from main.rs here.

pub mod chat;
```

### `tui/chat/mod.rs`

Contiene el punto de entrada público y el ciclo de vida de terminal en bruto (habilitar modo raw, entrar en pantalla alternativa, construir App, ejecutar bucle, restaurar terminal al drop).

| Desde `chat.rs` | Nueva ubicación |
|---|---|
| Líneas 840–861 (`pub fn run_chat`) | `tui/chat/mod.rs` |
| (declaraciones de módulo) | `mod app; mod events; mod render;` |

Imports necesarios:
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

Estado, comportamiento, helpers del ciclo de vida y tests. La mitad de "datos + métodos" del módulo.

| Desde `chat.rs` | Nueva ubicación | Visibilidad |
|---|---|---|
| Líneas 33–38 (MessageRole) | `app.rs` | `pub` (preservada de hoy) |
| Líneas 40–46 (ChatMessage) | `app.rs` | `pub` (preservada) |
| Líneas 48–54 (AgentInfo) | `app.rs` | `pub` (preservada) |
| Líneas 56–60 (InputMode) | `app.rs` | `enum` privado (preservado) |
| Líneas 62–69 (SlashCommandResult) | `app.rs` | `enum` privado (preservado) |
| Líneas 71–85 (struct App) | `app.rs` | `pub(super)` — era privado al módulo en chat.rs; ahora debe cruzar la nueva frontera de archivo hacia `render.rs` y `events.rs` |
| Línea 86 (SPINNER_FRAMES) | `app.rs` | `pub(super)` (usado tanto por `App::tick` como por `render_status_bar`) |
| Líneas 88–457 (impl App) | `app.rs` | sin cambios |
| Líneas 459–471 (discover_agents) | `app.rs` | `pub(super)` (llamado por `run_chat` en `mod.rs`) |
| Líneas 990–992 (timestamp_now) | `app.rs` | `pub(super)` es innecesario — los únicos llamadores están en el propio `app.rs`. Mantener privado. |
| Líneas 994–1002 (truncate_str) | `app.rs` | igual — solo lo llama `App::simulate_agent_response`. Mantener privado. |
| Líneas 1004–1111 (mod tests) | `app.rs` (tras descartar la salvaguarda) | `#[cfg(test)] mod tests` |

**Nota sobre visibilidad.** El movimiento puro de código preserva el comportamiento observable. Pero los elementos `App`, `SPINNER_FRAMES`, `discover_agents`, `render_ui`, `run_event_loop`, y los tipos `MessageRole`/`ChatMessage`/`AgentInfo` que antes eran privados al módulo (o solo crate-pub pero sin uso) deben ahora tener una visibilidad apropiada para cruzar la nueva frontera de submódulo. La nueva visibilidad es la más restrictiva que aún funciona:

- Elementos consumidos solo dentro de `app.rs`: permanecen privados (timestamp_now, truncate_str, InputMode, SlashCommandResult).
- Elementos consumidos a través de `app.rs`/`render.rs`/`events.rs`: `pub(super)` (App, SPINNER_FRAMES, MessageRole, AgentInfo, discover_agents).
- Elementos consumidos por `mod.rs`: `pub(super)` (run_event_loop en events.rs, render_ui en render.rs, App + discover_agents).
- Los tipos previamente `pub` `MessageRole`, `ChatMessage`, `AgentInfo`: este es el único juicio. Hoy son `pub` a nivel de crate (visibles como `chat::MessageRole` etc.). El objetivo del Enfoque A de "preservar la visibilidad" dice que deben permanecer visibles a nivel de crate tras el movimiento. **Decisión:** declararlos `pub` dentro de `app.rs`, y reexportarlos vía `pub use app::{MessageRole, ChatMessage, AgentInfo};` en `tui/chat/mod.rs`. La ruta visible al crate se mantiene corta (`tui::chat::ChatMessage` en lugar de `tui::chat::app::ChatMessage`), coincidiendo con la superficie de hoy módulo el prefijo `tui::`.

### `tui/chat/render.rs`

Las 7 funciones de render y el consumidor de SPINNER_FRAMES. Código de vista puro.

| Desde `chat.rs` | Nueva ubicación | Visibilidad |
|---|---|---|
| Líneas 473–510 (render_ui) | `render.rs` | `pub(super)` (llamado por `events.rs` vía `run_event_loop`) |
| Líneas 512–538 (render_status_bar) | `render.rs` | `fn` privada (preservada) |
| Líneas 540–636 (render_messages) | `render.rs` | `fn` privada |
| Líneas 638–672 (render_input) | `render.rs` | `fn` privada |
| Líneas 674–700 (render_hint_bar) | `render.rs` | `fn` privada |
| Líneas 702–779 (render_help_overlay) | `render.rs` | `fn` privada |
| Líneas 781–838 (render_agent_select) | `render.rs` | `fn` privada |

Imports necesarios:
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

El bucle de eventos.

| Desde `chat.rs` | Nueva ubicación | Visibilidad |
|---|---|---|
| Líneas 863–988 (run_event_loop) | `events.rs` | `pub(super)` (llamado desde `run_chat` en `mod.rs`) |

Imports necesarios:
```rust
use std::io::Stdout;
use std::time::{Duration, Instant};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::{Terminal, backend::CrosstermBackend};
use super::app::{App, SlashCommandResult};
use super::render::render_ui;
```

## Vista de la API pública desde fuera del módulo de chat

Tras la división, el único elemento visible a nivel de crate es `tui::chat::run_chat` (y los tipos reexportados `MessageRole`, `ChatMessage`, `AgentInfo`, que permanecen `pub` según el objetivo de preservación de visibilidad del Enfoque A). `main.rs` referencia exactamente uno de ellos:

```rust
// crates/coven-cli/src/main.rs línea 150 (antes):
Some(Command::Chat) => chat::run_chat(),

// después:
Some(Command::Chat) => tui::chat::run_chat(),
```

Y la declaración `mod` en la línea 23 (post–Fase 1):

```rust
// antes:
mod chat;

// después:
mod tui;
```

La posición alfabética de la declaración `mod` se desplaza desde `mod chat;` (entre `mod api;` y `mod control_plane;`) a `mod tui;` (entre `mod theme;` y `mod verification;`).

## Tests

### Migración

Los cinco tests/helpers existentes (`app_with_agents`, `agent`, más 4 tests de comportamiento que apuntan a métodos de `App`) se mueven intactos al bloque `#[cfg(test)] mod tests` de `app.rs`.

El test de salvaguarda `chat_module_stays_single_file_to_avoid_rust_module_ambiguity` (chat.rs:1036) **se elimina**. Su propósito era prevenir exactamente la división que implementa esta especificación. El `use std::path::Path;` que requería se elimina junto con él.

### Sin salvaguarda de reemplazo

El propio compilador de Rust rechaza el único caso verdaderamente ambiguo (que coexistan `src/tui/chat.rs` y `src/tui/chat/mod.rs` al mismo tiempo). Un test que afirme "estos archivos específicos existen con esta disposición" sería una restricción mantenida a través de cada reestructuración futura sin beneficio funcional.

## Criterios de aceptación

La Fase 2 está completa cuando:

1. `crates/coven-cli/src/chat.rs` ya no existe (`git ls-files` no devuelve nada para él; el árbol de trabajo no tiene tal archivo).
2. `crates/coven-cli/src/tui/mod.rs` existe con el contenido único `pub mod chat;` (más el comentario de documentación a nivel de módulo).
3. `crates/coven-cli/src/tui/chat/` contiene exactamente cuatro archivos: `mod.rs`, `app.rs`, `render.rs`, `events.rs`. Ningún otro.
4. `crates/coven-cli/src/main.rs` tiene `mod tui;` (posición alfabética ajustada) y `tui::chat::run_chat()` en la línea 150.
5. `cargo build -p coven-cli` tiene éxito con cero advertencias.
6. `cargo test -p coven-cli` pasa; el conteo de tests unitarios baja exactamente en uno (el test de salvaguarda eliminado). Los smoke tests pasan en 4.
7. `cargo clippy -p coven-cli --no-deps` produce cero advertencias.
8. Ningún elemento del módulo de chat se expone de forma nueva más allá de `tui::chat::run_chat`, `tui::chat::ChatMessage`, `tui::chat::AgentInfo`, `tui::chat::MessageRole` (los tres tipos reexportados de la superficie de hoy).
9. Smoke check manual: lanzar `coven chat` abre la TUI y renderiza sin regresiones visibles (mismos colores, mismo layout, mismas combinaciones de teclas).

## Escala de diff estimada

| Archivo | Acción | Líneas |
|---|---|---|
| `crates/coven-cli/src/chat.rs` | Eliminar | -1111 |
| `crates/coven-cli/src/tui/mod.rs` | Crear | ~10 |
| `crates/coven-cli/src/tui/chat/mod.rs` | Crear | ~40 |
| `crates/coven-cli/src/tui/chat/app.rs` | Crear | ~530 |
| `crates/coven-cli/src/tui/chat/render.rs` | Crear | ~380 |
| `crates/coven-cli/src/tui/chat/events.rs` | Crear | ~150 |
| `crates/coven-cli/src/main.rs` | Edición de 1 línea + 1 intercambio de mod | ±2 |

Neto: ~0 líneas (el archivo se reorganiza, no se reduce). El test de salvaguarda eliminado quita ~25 líneas del total final.
