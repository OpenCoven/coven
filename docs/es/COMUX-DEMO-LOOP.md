---
title: "Bucle de demo de comux y Coven"
description: "El contrato del lado de Coven para hacer que las sesiones de Codex y Claude Code sean descubribles como paneles visibles de comux a través de la API por socket local."
---

# Bucle de demo comux + Coven

Este es el contrato del lado de Coven para hacer visibles en comux las sesiones de Codex y Claude Code gestionadas por Coven.

```mermaid
flowchart LR
  subgraph Dev["Developer"]
    Open[Open repo in comux]
  end

  subgraph Local["Local machine"]
    Comux[comux cockpit]
    CLI[coven CLI]
    Daemon[Coven daemon]
    PTY1[Codex PTY]
    PTY2[Claude PTY]
    Store[(SQLite store + events)]
  end

  Open --> Comux
  Comux -->|discover| CLI
  CLI -->|coven sessions --json| Daemon
  Daemon --> Store
  Daemon --> PTY1
  Daemon --> PTY2
  Comux -->|open / attach| Daemon
  Daemon -->|/events| Comux
  Comux --> Review[Inspect · Diff · Merge · PR]
  Review --> Ritual[Archive · Summon · Sacrifice]
  Ritual --> Daemon
```

El bucle de demo es extremo a extremo: comux nunca se salta el daemon, y el daemon nunca confía en comux para la aplicación de raíz de proyecto, harness o borrado destructivo.

## Bucle

1. Abre el repositorio objetivo en comux.
2. Arranca Coven si es necesario:

   ```sh
   coven daemon start
   ```

3. Lanza una sesión respaldada por Coven desde el mismo repositorio:

   ```sh
   coven run codex "fix the failing tests"
   coven run claude "review the diff"
   ```

4. Deja que comux descubra sesiones a través de cualquiera de las rutas de cliente soportadas:
   - `coven sessions --json` para descubrimiento local simple por CLI.
   - `GET /api/v1/sessions` después de `GET /api/v1/health` para clientes del daemon.
5. Abre la sesión como un panel visible de comux, o adjúntate manualmente:

   ```sh
   coven attach <session-id>
   ```

6. Inspecciona archivos, diffs y la salida de la sesión desde comux.
7. Haz merge, crea un PR, archiva, invoca, sacrifica o limpia explícitamente tras la verificación.

## Descubrimiento por CLI

`coven sessions --json` imprime un objeto estable con un array `sessions`. Los registros usan los mismos nombres snake_case que la API del daemon:

```json
{
  "sessions": [
    {
      "id": "session-1",
      "project_root": "/repo",
      "harness": "codex",
      "title": "Fix the tests",
      "status": "running",
      "exit_code": null,
      "archived_at": null,
      "created_at": "2026-05-14T07:00:00Z",
      "updated_at": "2026-05-14T07:00:01Z"
    }
  ]
}
```

Usa `--all --json` cuando las sesiones archivadas también deban ser visibles.

## Descubrimiento por daemon

Los clientes del daemon deben usar la API por socket versionada:

1. `GET /api/v1/health`
2. Verifica `apiVersion === "coven.daemon.v1"` y `capabilities.sessions === true`.
3. `GET /api/v1/sessions`
4. Filtra las sesiones por raíz de proyecto verificada antes de mostrarlas en una UI limitada al proyecto.

El socket del daemon usa por defecto `~/.coven/coven.sock`. El daemon sigue siendo la autoridad para raíces de proyecto, cwd, ids de harness, comprobaciones de sesión viva, input, peticiones de kill, estado de archivo y reglas de borrado destructivo.

## Estados no disponibles

Los clientes deben mantener su UI principal usable cuando Coven falta o está detenido:

- CLI faltante: muestra la guía de instalación para `@opencoven/cli`.
- Daemon detenido o socket faltante: sugiere `coven daemon start`.
- Harness faltante: sugiere `coven doctor`.
- Versión de API no compatible: pide al usuario que actualice Coven o el cliente.

## Roadmap

El roadmap más amplio de OpenCoven sigue siendo el punto de seguimiento público para la demo extremo a extremo: [ROADMAP.md](/ROADMAP).

> **Image asset prompt (to be generated and dropped into `docs/images/comux-demo-loop.png`):** Render a 1920×1080 split-pane mockup that mimics a comux cockpit on the OpenCoven dark palette (`#1A1825` background, `#9A8ECD` chrome). Three tmux-like panes: (1) file explorer with a fake monorepo tree, (2) live attach view streaming a fake Codex session ("Fix the failing tests"), (3) diff/review pane with a small green/red unified diff. Bottom status bar reads `coven daemon · running · 1.2 MB events · 2 active sessions`. Use `Fragment Mono`.
