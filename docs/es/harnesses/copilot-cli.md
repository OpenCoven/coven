---
summary: "Ejecuta GitHub Copilot CLI bajo la supervisión de Coven. Id de harness `copilot`."
read_when:
  - Configurando GitHub Copilot CLI para Coven
  - Diagnosticando fallos de harness específicos de Copilot
title: "Harness de Copilot CLI"
description: "Ejecuta la GitHub Copilot CLI bajo la supervisión de Coven con el id de harness copilot, sesiones ancladas al proyecto y los flujos habituales de attach y rituales."
---


GitHub Copilot CLI es la CLI de agente de código de GitHub. Coven usa un PTY
anclado al proyecto tanto para lanzamientos interactivos como one-shot, así
que las sesiones, attaches y rituales funcionan igual que con cualquier otro
harness.

| Campo | Valor |
|---|---|
| Id de harness | `copilot` |
| Instalación | `npm install -g @github/copilot` o `brew install --cask copilot-cli` |
| Auth | `copilot login` (una vez, del lado de GitHub) |
| Chequeo de doctor | `coven doctor` informa la disponibilidad de Copilot CLI y la pista de instalación cuando falta. |

## Configuración

<Steps>
  <Step title="Instala Copilot CLI">
    ```bash
    npm install -g @github/copilot
    # o
    brew install --cask copilot-cli
    ```
  </Step>
  <Step title="Inicia sesión en GitHub">
    ```bash
    copilot login
    ```
    Las credenciales de GitHub se quedan con Copilot. Coven nunca las lee.
  </Step>
  <Step title="Confirma con Coven">
    ```bash
    coven doctor
    ```
    La sección Harnesses debe incluir `[OK] Copilot CLI` con el ejecutable `copilot` resuelto.
  </Step>
  <Step title="Lanza">
    ```bash
    coven run copilot "arregla los tests que fallan"
    ```
  </Step>
</Steps>

## Mapeo de permisos

La superficie de permisos de Copilot son flags booleanos/multi-token en lugar
de un único flag de modo, así que `--permission` de Coven se mapea a listas
de argv:

| Política de Coven | Argv de Copilot | Efecto |
|---|---|---|
| `full` | `--allow-all` | Todas las herramientas, rutas y URLs se ejecutan sin confirmación. |
| `read-only` | `--deny-tool write --deny-tool shell` | Las escrituras de archivos y los comandos de shell se deniegan directamente (las reglas de denegación ganan a cualquier regla de permiso). Las lecturas dentro del directorio de trabajo siguen permitidas. |
| *(ninguna)* | *(sin flags)* | Aplican los valores por defecto de Copilot. En modo no interactivo, Copilot auto-deniega cualquier herramienta que habría pedido confirmación. |

## Continuidad de sesión

Copilot soporta ids de sesión preasignados: `coven chat` envía
`--session-id <uuid>` en el primer turno y el mismo flag en los turnos
siguientes. `--session-id` crea una sesión nueva bajo un UUID elegido y
también reanuda una existente, así que los ids obsoletos se auto-reparan en
una conversación nueva en lugar de fallar.

## Solución de problemas

| Síntoma | Causa probable | Arreglo |
|---|---|---|
| `coven doctor` reporta `copilot` como faltante | Copilot CLI no está en `PATH` | `npm install -g @github/copilot` (o `brew install --cask copilot-cli`) y re-ejecuta doctor. |
| Las ejecuciones fallan de inmediato con un error de auth | Sin sesión iniciada | `copilot login`. |
| `Error: Model "auto" does not support reasoning effort configuration` | `--model auto` combinado con `--think`/`--speed` | Quita el flag de esfuerzo o elige un modelo concreto. |
| La sesión no puede leer un archivo fuera del repo | Verificación de rutas de Copilot | Re-lanza con `--add-dir <ese-directorio>`. |

## Relacionado

- [Instalar las CLIs de harness](/harnesses/installing)
- [Frontera de auth del proveedor](/harnesses/provider-auth)
- [Guía de adaptadores de harness](/HARNESS-ADAPTERS)
