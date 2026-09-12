declare module "openclaw/plugin-sdk/acp-runtime" {
  export type AcpRuntimePromptMode = "prompt" | "steer";
  export type AcpRuntimeSessionMode = "persistent" | "oneshot";
  export type AcpSessionUpdateTag =
    | "agent_message_chunk"
    | "agent_thought_chunk"
    | "tool_call"
    | "tool_call_update"
    | "usage_update"
    | "available_commands_update"
    | "current_mode_update"
    | "config_option_update"
    | "session_info_update"
    | "plan"
    | (string & {});

  export type AcpRuntimeHandle = {
    sessionKey: string;
    agentId?: string;
    backend: string;
    runtimeSessionName: string;
    cwd?: string;
    acpxRecordId?: string;
    backendSessionId?: string;
    agentSessionId?: string;
    appliedModel?: { kind: "applied"; model: string } | { kind: "dropped" };
  };

  export type AcpRuntimeEnsureInput = {
    sessionKey: string;
    agentId?: string;
    persistedHandle?: AcpRuntimeHandle;
    agent: string;
    mode: AcpRuntimeSessionMode;
    resumeSessionId?: string;
    model?: string;
    modelExplicit?: boolean;
    thinking?: string;
    cwd?: string;
    env?: Record<string, string>;
  };

  export type AcpRuntimeTurnAttachment = {
    mediaType: string;
    data: string;
  };

  export type AcpRuntimeTurnInput = {
    handle: AcpRuntimeHandle;
    text: string;
    attachments?: AcpRuntimeTurnAttachment[];
    mode: AcpRuntimePromptMode;
    requestId: string;
    signal?: AbortSignal;
    onElicitation?: unknown;
  };

  export type AcpRuntimeCapabilities = {
    controls: string[];
    configOptionKeys?: string[];
  };

  export type AcpRuntimeStatus = {
    summary?: string;
    acpxRecordId?: string;
    backendSessionId?: string;
    agentSessionId?: string;
    details?: Record<string, unknown>;
  };

  export type AcpRuntimeDoctorReport = {
    ok: boolean;
    code?: string;
    message: string;
    installCommand?: string;
    details?: string[];
  };

  export type AcpRuntimeEvent =
    | {
        type: "text_delta";
        text: string;
        stream?: "output" | "thought";
        tag?: AcpSessionUpdateTag;
      }
    | {
        type: "status";
        text: string;
        tag?: AcpSessionUpdateTag;
        used?: number;
        size?: number;
      }
    | {
        type: "tool_call";
        text: string;
        tag?: AcpSessionUpdateTag;
        toolCallId?: string;
        status?: string;
        title?: string;
        kind?:
          | "read"
          | "edit"
          | "delete"
          | "move"
          | "search"
          | "execute"
          | "fetch"
          | "switch_mode"
          | "think"
          | "other";
      }
    | {
        type: "done";
        status?: "completed" | "cancelled";
        stopReason?: string;
      }
    | {
        type: "error";
        message: string;
        code?: string;
        detailCode?: string;
        retryable?: boolean;
      };

  export interface AcpRuntime {
    ownerAwareSessions?: 1;
    ensureSession(input: AcpRuntimeEnsureInput): Promise<AcpRuntimeHandle>;
    runTurn(input: AcpRuntimeTurnInput): AsyncIterable<AcpRuntimeEvent>;
    getCapabilities?(input?: {
      handle?: AcpRuntimeHandle;
    }): Promise<AcpRuntimeCapabilities> | AcpRuntimeCapabilities;
    getStatus?(input: {
      handle: AcpRuntimeHandle;
      signal?: AbortSignal;
    }): Promise<AcpRuntimeStatus>;
    doctor?(): Promise<AcpRuntimeDoctorReport>;
    cancel(input: { handle: AcpRuntimeHandle; reason?: string; signal?: AbortSignal }): Promise<void>;
    close(input: { handle: AcpRuntimeHandle; reason?: string; signal?: AbortSignal }): Promise<void>;
    prepareFreshSession?(input: { sessionKey: string; agentId?: string }): Promise<void>;
  }

  export class AcpRuntimeError extends Error {
    readonly code: string;
    readonly detailCode?: string;
    readonly cause?: unknown;
    constructor(code: string, message: string, options?: { cause?: unknown; detailCode?: string });
  }

  export type AcpRuntimeBackend = {
    id: string;
    runtime: AcpRuntime;
  };

  export function registerAcpRuntimeBackend(backend: AcpRuntimeBackend): void;
  export function unregisterAcpRuntimeBackend(id: string): void;
  export function getAcpRuntimeBackend(id?: string): AcpRuntimeBackend | undefined;
}
