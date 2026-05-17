---
summary: "Flujo de release para @opencoven/cli y los paquetes de plataforma."
read_when:
  - Cutting a release
title: "Publicando releases"
description: "Runbook para operadores: cómo publicar Coven en npm con preflight, dry-run, publicación del wrapper de CLI y los paquetes nativos, y verificación postflight."
---

Coven publica el wrapper de npm y los paquetes nativos de plataforma desde el workflow de GitHub Actions **Release npm packages**. Las versiones del paquete fuente permanecen en `0.0.0`; la versión del dispatch del workflow es la versión publicada en npm.

## Preflight

Antes de publicar:

1. Confirma que no hay PRs abiertos que deban entrar en la release.
2. Confirma que el CI de `main` está en verde para el commit exacto que vas a publicar.
3. Comprueba en npm las versiones actuales de `latest`:

```sh
npm view @opencoven/cli version dist-tags
npm view @opencoven/cli-macos version dist-tags
npm view @opencoven/cli-linux-x64 version dist-tags
npm view @opencoven/cli-windows version dist-tags
```

4. Confirma que el changelog, la copia de estado del README del paquete y los recursos de marca coinciden con la release.

## Dry Run

Ejecuta primero el workflow con `publish=false`. Esto construye todos los binarios de plataforma y realiza dry-runs de npm publish sin necesidad de credenciales de npm:

```sh
gh workflow run release-npm.yml \
  --ref main \
  -f publish=false \
  -f version=0.0.12
```

Observa la ejecución:

```sh
gh run list --workflow release-npm.yml --branch main --limit 1
gh run watch <run-id>
```

## Publish

Publica solo después de que el dry-run tenga éxito y las versiones de los paquetes npm sigan disponibles:

```sh
gh workflow run release-npm.yml \
  --ref main \
  -f publish=true \
  -f version=0.0.12
```

El job de publish usa el entorno `npm-publish` y `NPM_ACCESS_TOKEN`. Publica primero los paquetes nativos (`@opencoven/cli-linux-x64`, `@opencoven/cli-windows`, `@opencoven/cli-macos`) y luego el paquete wrapper (`@opencoven/cli`).

## Postflight

Después de que la ejecución de publish se complete:

```sh
npm view @opencoven/cli version dist-tags
npm view @opencoven/cli-macos version dist-tags
npm view @opencoven/cli-linux-x64 version dist-tags
npm view @opencoven/cli-windows version dist-tags
```

Si algún paquete no se publicó, no vuelvas a ejecutar a ciegas. Inspecciona el job fallido, confirma qué versiones de paquete existen en npm y vuelve a ejecutar solo con una nueva versión si npm ya ha aceptado parte de la release.
