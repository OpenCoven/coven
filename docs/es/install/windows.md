---
summary: "Instala Coven en Windows nativo."
read_when:
  - Instalando en Windows
title: "Instalación en Windows"
description: "Instala Coven en Windows: cómo configurar el wrapper, el binario nativo del daemon, COVEN_HOME y las CLIs de harness en un host de Windows o un entorno WSL2."
---

# Instalación en Windows

Usa el wrapper de npm publicado desde PowerShell, Windows Terminal u otro terminal que pueda ejecutar paquetes de Node.js:

```powershell
npx @opencoven/cli doctor
```

Para uso recurrente, instala el wrapper de forma global:

```powershell
npm install -g @opencoven/cli
coven doctor
```

El wrapper expone el comando `coven` y lanza el binario nativo de Windows cuando el paquete de la release incluye uno para tu plataforma. `coven doctor` es el primer paso de verificación: comprueba el estado local e informa de si las CLIs de harness compatibles, como Codex o Claude Code, están disponibles en `PATH`.

## Primera ejecución

Desde el directorio de un proyecto:

```powershell
coven
```

El comando por defecto abre la TUI prompt-first. También puedes usar el flujo explícito de CLI:

```powershell
coven doctor
coven daemon start
coven run codex "fix the failing tests"
coven sessions
```

Instala y autentica al menos una CLI de harness antes de esperar que `coven run` lance trabajo. Si `coven doctor` informa de un harness ausente, instala esa herramienta, abre un nuevo terminal para que `PATH` se refresque y ejecuta `coven doctor` de nuevo.

## Notas sobre Windows

- Mantén `COVEN_HOME` en una ruta local propiedad de tu usuario de Windows cuando lo sobrescribas.
- Ejecuta Coven y tu CLI de harness desde el mismo entorno. Un harness instalado solo dentro de WSL2 no está disponible para PowerShell nativo de Windows a menos que lo expongas por separado.
- Si la entrada del terminal se comporta de forma extraña, actualiza al wrapper más reciente y ejecuta `coven tui` de nuevo. La TUI de Windows filtra los eventos de pulsación de teclas para que los caracteres tecleados, las flechas y Enter se manejen una sola vez.

## Relacionado

- [Empieza con Coven](/GETTING-STARTED)
- [TUI de Coven](/start/coven-tui)
- [Solución de problemas](/TROUBLESHOOTING)
- [Referencia de la CLI](/reference/cli)
