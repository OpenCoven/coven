---
summary: "Поток релизов для @opencoven/cli и пакетов платформ."
read_when:
  - Cutting a release
title: "Выпуск релизов"
description: "Runbook для публикации Coven в npm: preflight, dry-run, нативные платформенные пакеты и postflight-проверки из workflow GitHub Actions."
---

Coven публикует npm-wrapper и нативные платформенные пакеты из workflow GitHub Actions **Release npm packages**. Версии исходных пакетов остаются `0.0.0`; версия dispatch'а workflow — это публикуемая npm-версия.

## Preflight

Перед публикацией:

1. Убедитесь, что нет открытых PR, которые должны попасть в релиз.
2. Убедитесь, что CI `main` зелёный для точного коммита, который вы будете релизить.
3. Проверьте в npm текущие `latest`-версии:

```sh
npm view @opencoven/cli version dist-tags
npm view @opencoven/cli-macos version dist-tags
npm view @opencoven/cli-linux-x64 version dist-tags
npm view @opencoven/cli-windows version dist-tags
```

4. Убедитесь, что changelog, статусный текст README пакета и брендовые ассеты соответствуют релизу.

## Dry Run

Сначала запустите workflow с `publish=false`. Это соберёт все платформенные бинарники и выполнит dry-run'ы npm publish без необходимости в npm-учётных данных:

```sh
gh workflow run release-npm.yml \
  --ref main \
  -f publish=false \
  -f version=0.0.12
```

Следите за запуском:

```sh
gh run list --workflow release-npm.yml --branch main --limit 1
gh run watch <run-id>
```

## Publish

Публикуйте только после того, как dry-run пройдёт успешно, а версии npm-пакетов всё ещё доступны:

```sh
gh workflow run release-npm.yml \
  --ref main \
  -f publish=true \
  -f version=0.0.12
```

Job публикации использует окружение `npm-publish` и `NPM_ACCESS_TOKEN`. Сначала публикуются нативные пакеты (`@opencoven/cli-linux-x64`, `@opencoven/cli-windows`, `@opencoven/cli-macos`), а затем wrapper-пакет (`@opencoven/cli`).

## Postflight

После завершения запуска публикации:

```sh
npm view @opencoven/cli version dist-tags
npm view @opencoven/cli-macos version dist-tags
npm view @opencoven/cli-linux-x64 version dist-tags
npm view @opencoven/cli-windows version dist-tags
```

Если какой-либо пакет не опубликовался, не запускайте повторно вслепую. Изучите упавший job, подтвердите, какие версии пакета существуют в npm, и перезапускайте только с новой версией, если npm уже принял часть релиза.
