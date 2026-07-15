---
summary: "Запуск GitHub Copilot CLI под наблюдением Coven. Id harness'а — `copilot`."
read_when:
  - Настройка GitHub Copilot CLI для Coven
  - Диагностика сбоев harness, специфичных для Copilot
title: "Harness Copilot CLI"
description: "Запуск GitHub Copilot CLI под наблюдением Coven с id harness'а copilot, сессиями в границах проекта и обычными потоками attach и ритуалов."
---


GitHub Copilot CLI — это CLI-агент для кода от GitHub. Coven использует PTY в
границах проекта и для интерактивных, и для one-shot запусков, поэтому
сессии, attach и ритуалы работают так же, как с любым другим harness.

| Поле | Значение |
|---|---|
| Id harness'а | `copilot` |
| Установка | `npm install -g @github/copilot` или `brew install --cask copilot-cli` |
| Auth | `copilot login` (один раз, на стороне GitHub) |
| Проверка doctor | `coven doctor` сообщает о доступности Copilot CLI и печатает подсказку по установке, если он отсутствует. |

## Настройка

<Steps>
  <Step title="Установите Copilot CLI">
    ```bash
    npm install -g @github/copilot
    # или
    brew install --cask copilot-cli
    ```
  </Step>
  <Step title="Войдите в GitHub">
    ```bash
    copilot login
    ```
    Учётные данные GitHub остаются у Copilot. Coven никогда их не читает.
  </Step>
  <Step title="Проверьте через Coven">
    ```bash
    coven doctor
    ```
    Раздел Harnesses должен содержать `[OK] Copilot CLI` с найденным исполняемым файлом `copilot`.
  </Step>
  <Step title="Запустите">
    ```bash
    coven run copilot "почини падающие тесты"
    ```
  </Step>
</Steps>

## Отображение прав доступа

Поверхность прав Copilot — это булевы/многотокенные флаги, а не один флаг
режима, поэтому `--permission` в Coven отображается в списки argv:

| Политика Coven | Argv Copilot | Эффект |
|---|---|---|
| `full` | `--allow-all` | Все инструменты, пути и URL выполняются без подтверждения. |
| `read-only` | `--deny-tool write --deny-tool shell` | Запись файлов и shell-команды запрещаются сразу (правила запрета сильнее любых правил разрешения). Чтение внутри рабочего каталога остаётся доступным. |
| *(нет)* | *(без флагов)* | Действуют настройки Copilot по умолчанию. В неинтерактивном режиме Copilot автоматически отклоняет любой инструмент, который потребовал бы подтверждения. |

## Непрерывность сессий

Copilot поддерживает предварительно назначенные id сессий: `coven chat`
отправляет `--session-id <uuid>` в первом ходе и тот же флаг в последующих.
`--session-id` создаёт новую сессию под выбранным UUID и возобновляет
существующую, поэтому устаревшие id самовосстанавливаются в новую беседу
вместо ошибки.

## Устранение неполадок

| Симптом | Вероятная причина | Решение |
|---|---|---|
| `coven doctor` сообщает, что `copilot` отсутствует | Copilot CLI нет в `PATH` | `npm install -g @github/copilot` (или `brew install --cask copilot-cli`), затем повторите doctor. |
| Запуски сразу падают с ошибкой auth | Нет входа | `copilot login`. |
| `Error: Model "auto" does not support reasoning effort configuration` | `--model auto` вместе с `--think`/`--speed` | Уберите флаг усилия или выберите конкретную модель. |
| Сессия не может прочитать файл вне репозитория | Проверка путей Copilot | Перезапустите с `--add-dir <тот-каталог>`. |

## См. также

- [Установка CLI harness'ов](/harnesses/installing)
- [Граница auth провайдера](/harnesses/provider-auth)
- [Руководство по адаптерам harness](/HARNESS-ADAPTERS)
