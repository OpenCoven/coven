---
summary: "Установка Coven на нативный Windows."
read_when:
  - Установка на Windows
title: "Установка на Windows"
description: "Установка Coven на Windows: как настроить wrapper, нативный бинарник демона, COVEN_HOME и CLI харнессов на хосте Windows или в среде WSL2."
---

# Установка на Windows

Используй опубликованный npm-wrapper из PowerShell, Windows Terminal или другого терминала, который может запускать пакеты Node.js:

```powershell
npx @opencoven/cli doctor
```

Для регулярного использования установи wrapper глобально:

```powershell
npm install -g @opencoven/cli
coven doctor
```

Wrapper предоставляет команду `coven` и запускает нативный Windows-бинарник, если пакет релиза содержит его для твоей платформы. `coven doctor` — это первый шаг проверки: он анализирует локальное состояние и сообщает, доступны ли в `PATH` поддерживаемые CLI харнессов, такие как Codex или Claude Code.

## Первый запуск

Из каталога проекта:

```powershell
coven
```

Команда по умолчанию открывает prompt-first TUI. Также можно использовать явный поток CLI:

```powershell
coven doctor
coven daemon start
coven run codex "fix the failing tests"
coven sessions
```

Установи и авторизуй хотя бы одну CLI харнесса, прежде чем ожидать, что `coven run` запустит работу. Если `coven doctor` сообщает об отсутствующем харнессе, установи этот инструмент, открой новый терминал, чтобы `PATH` обновился, и снова запусти `coven doctor`.

## Заметки по Windows

- При переопределении `COVEN_HOME` храни его по локальному пути, принадлежащему твоему пользователю Windows.
- Запускай Coven и CLI харнесса из одного и того же окружения. Харнесс, установленный только внутри WSL2, недоступен для нативного PowerShell Windows, если ты не предоставишь его отдельно.
- Если ввод в терминале ведёт себя странно, обнови wrapper до последней версии и снова запусти `coven tui`. TUI на Windows фильтрует события нажатия клавиш, так что набранные символы, стрелки и Enter должны обрабатываться один раз.

## Связанное

- [Начало работы с Coven](/GETTING-STARTED)
- [TUI Coven](/start/coven-tui)
- [Устранение неполадок](/TROUBLESHOOTING)
- [Справочник CLI](/reference/cli)
