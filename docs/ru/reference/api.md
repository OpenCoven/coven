---
summary: "Текущие endpoint'ы локального socket API Coven."
read_when:
  - Поиск endpoint'а
  - Построение клиента против `/api/v1`
title: "Справочник API Coven"
description: "Справочник endpoint'ов для локального socket API Coven под /api/v1: health, capabilities, actions, sessions, events и пересылка input."
---


Демон Coven предоставляет свой публичный API как HTTP через Unix socket под `<covenHome>/coven.sock`. Активный контракт — **`coven.daemon.v1`**, обслуживаемый под `/api/v1`.

```mermaid
flowchart LR
  Root["/api/v1"] --> Version["GET /api-version"]
  Root --> Health["GET /health"]
  Root --> Capabilities["GET /capabilities"]
  Root --> Actions["POST /actions"]
  Root --> Sessions["/sessions"]
  Root --> Events["GET /events"]

  Sessions --> SList["GET /"]
  Sessions --> SCreate["POST /"]
  Sessions --> SById["/:id"]
  SById --> SGet["GET /"]
  SById --> SInput["POST /input"]
  SById --> SKill["POST /kill"]
```

## Endpoint'ы

| Метод | Путь | Назначение | Тело | Успех | Ошибки |
|---|---|---|---|---|---|
| GET | `/api/v1/api-version` | Активная версия API + поддерживаемые версии. | — | `{ apiVersion, supportedApiVersions }` | — |
| GET | `/api/v1/health` | Доступность демона, версия, capabilities, pid. | — | `{ ok, apiVersion, covenVersion, capabilities, daemon }` | `503 runtime_unavailable` |
| GET | `/api/v1/capabilities` | Каталог capabilities с подсказками политики. | — | `{ capabilities: [...] }` | — |
| POST | `/api/v1/actions` | Маршрутизировать известный id действия плоскости управления. | `{ action, origin, intentId, args }` | `{ ok, accepted, status, event }` | `400 invalid_request` (неизвестное действие) |
| GET | `/api/v1/sessions` | Перечислить активные сессии. | — | `SessionRecord[]` | — |
| POST | `/api/v1/sessions` | Запустить сессию harness'а, ограниченную проектом. | `{ projectRoot, cwd?, harness, prompt, title?, launchMode?, conversation?, conversationId? }` | `SessionRecord` | `400 invalid_request` (включая cwd вне проекта, неизвестный id harness, некорректный body), `500 launch_failed` (runtime spawn / начальная запись / старт CLI дали сбой; строка помечена как `failed`) |
| GET | `/api/v1/sessions/:id` | Получить одну сессию. | — | `SessionRecord` | `404 session_not_found` |
| POST | `/api/v1/sessions/:id/input` | Переслать input в живую сессию. | `{ data }` | `{ ok, accepted }` | `400 invalid_request` (некорректный body / `data` отсутствует или не-string), `404 session_not_found`, `409 session_not_live`, `500 send_input_failed` |
| POST | `/api/v1/sessions/:id/kill` | Убить живую сессию. | — | `{ ok, accepted }` | `404 session_not_found`, `409 session_not_live`, `500 kill_failed` |
| GET | `/api/v1/events` | Прочитать пагинированные события сессии. | — (`?sessionId`, `?afterSeq`, `?afterEventId`, `?limit`) | `{ events, nextCursor, hasMore }` | `400 invalid_request` |

Все ответы об ошибках используют структурированный конверт, задокументированный в [Контракт API](/API-CONTRACT#structured-error-envelope).

## Всегда начинай с health

```http
GET /api/v1/health
```

Ответ говорит тебе активную `apiVersion`, `capabilities` демона и работающий pid/uptime. Рассматривай остальную часть API как неопределённую, пока не прочитаешь эти поля.

См. [Локальный API Coven](/API) для примеров ответов и [Контракт API](/API-CONTRACT) для стабильных форм и конвертов сбоев.

## Связанное

- [Локальный API Coven](/API)
- [Контракт API](/API-CONTRACT)
- [Аутентификация и локальный доступ](/AUTH)
- [Интеграция клиентов](/CLIENT-INTEGRATION)
