---
summary: "Запускай CLI OpenAI Codex под надзором Coven. Id harness'а `codex`."
read_when:
  - Настройка Codex для Coven
  - Диагностика специфичных для Codex сбоев harness'а
title: "Harness Codex"
description: "Запускай CLI OpenAI Codex под надзором Coven с id harness'а codex, PTY, ограниченным проектом, и обычными потоками сессии, attach и ритуалов."
---


Codex — это CLI кодирующего агента OpenAI. Coven оборачивает её в PTY, ограниченный проектом, чтобы запуски, attach и ритуалы работали так же, как для любого другого harness'а.

| Поле | Значение |
|---|---|
| Id harness'а | `codex` |
| Установка | `npm install -g @openai/codex` |
| Auth | `codex login` (одноразово, со стороны OpenAI) |
| Проверка doctor | `coven doctor` сообщает разрешённый путь и версию Codex. |

## Настройка

<Steps>
  <Step title="Установи Codex">
    ```bash
    npm install -g @openai/codex
    ```
    Другие методы установки (Homebrew cask, менеджеры пакетов) перечислены в [репо Codex](https://github.com/openai/codex).
  </Step>
  <Step title="Войди в OpenAI">
    ```bash
    codex login
    ```
    Учётные данные провайдера остаются с Codex. Coven никогда их не читает.
  </Step>
  <Step title="Подтверди с Coven">
    ```bash
    coven doctor
    ```
    Вывод должен включать строку вроде `codex: ok (/usr/local/bin/codex)`.
  </Step>
  <Step title="Запуск">
    ```bash
    coven run codex "fix the failing tests"
    ```
  </Step>
</Steps>

## Флаги для каждой сессии

```bash
coven run codex "audit this repo" --cwd packages/cli --title "CLI audit"
```

- `--cwd` — канонизирован внутри корня проекта.
- `--title` — задаёт читаемый заголовок в браузере сессий.
- `--json` — печатает структурированные метаданные запуска для клиентов.

## Граница auth провайдера

Codex владеет собственным потоком OAuth и кэшем токенов. Если ты видишь `Invalidated OAuth token`, снова запусти `codex login`. Coven сохранит существующую запись сессии, чтобы ты мог перезапустить с тем же заголовком.

Для локального пути спасения:

```bash
coven patch openclaw "fix Codex auth profile order after invalidated OAuth token"
```

## Решение проблем

| Симптом | Вероятная причина | Решение |
|---|---|---|
| `coven doctor` сообщает, что `codex` отсутствует | Codex не в `PATH` | `npm install -g @openai/codex`, затем повторно запусти doctor. |
| Codex просит логин при каждом запуске | Устаревший токен | `codex login`. |
| Сессия зависает при старте | Codex ждёт prompt TTY | Отсоединись с `Ctrl-]`, перезапусти с `coven run` напрямую. |

## Как Coven контролирует Codex

```mermaid
sequenceDiagram
  participant U as User
  participant C as coven CLI
  participant D as Coven daemon
  participant Cx as Codex PTY
  participant Op as OpenAI API

  U->>C: coven run codex "audit this repo"
  C->>D: POST /api/v1/sessions
  D->>D: canonicalize root + cwd
  D->>D: lookup adapter for "codex"
  D->>Cx: spawn codex (prefix: exec --skip-git-repo-check --color never)
  Cx->>Op: provider auth (uses ~/.codex credentials — Coven does not see)
  Op-->>Cx: model response stream
  Cx-->>D: stdout / exit events
  D-->>C: SessionRecord (id, status=running)
  C-->>U: print session id, switch to attach view
```

Пунктирная линия, на которую стоит обратить внимание: Coven никогда не подключается к API OpenAI сам. Путь учётных данных — **CLI Codex ↔ OpenAI**, при этом Coven только наблюдает вывод PTY.

> **Image asset prompt (to be generated and dropped into `docs/images/harnesses-codex-launch.png`):** Render a 1600×900 split-screen mockup. Left half: a terminal showing `coven run codex "audit this repo"` followed by the live attach stream (a few wrapped lines of fake Codex output). Right half: the Coven TUI session browser showing one new entry highlighted in the OpenCoven accent (`#D4B5FF`): `codex · running · "audit this repo" · session-1`. Background `#1A1825`, primary `#9A8ECD`.

## Связанное

- [Установка CLI harness'ов](/harnesses/installing)
- [Граница auth провайдера](/harnesses/provider-auth)
- [Решение проблем harness'а](/harnesses/troubleshooting)
