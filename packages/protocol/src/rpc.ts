import type { ChatEvent, ToolCallRecord } from './generated/chat-event';

/** Version de protocole supportée par le serveur CLI (`cli/src/rpc/protocol.rs`). */
export const PROTOCOL_VERSION = 1 as const;

/** Codes JSON-RPC standard (plage réservée -32768..-32000, spec JSON-RPC 2.0). */
export const jsonrpcCode = {
  PARSE_ERROR: -32700,
  METHOD_NOT_FOUND: -32601,
  SERVER_ERROR: -32000,
} as const;

/** Codes `VNL-RPC-*` du serveur (`cli/src/rpc/protocol.rs::vnl_code`). */
export const vnlCode = {
  MALFORMED_REQUEST: 'VNL-RPC-000',
  NOT_INITIALIZED: 'VNL-RPC-001',
  BUSY: 'VNL-RPC-002',
  UNKNOWN_PROTOCOL_VERSION: 'VNL-RPC-003',
  METHOD_NOT_FOUND: 'VNL-RPC-004',
  CONVERSATION_NOT_FOUND: 'VNL-RPC-005',
  CONFIG_ERROR: 'VNL-RPC-006',
  CONVERSATION_STORAGE_ERROR: 'VNL-RPC-007',
  NO_AGENT_RESOLVED: 'VNL-RPC-008',
  TURN_EXECUTION_ERROR: 'VNL-RPC-009',
  K8S_ERROR: 'VNL-RPC-010',
} as const;

export interface JsonRpcRequest {
  jsonrpc: '2.0';
  id: number | string | null;
  method: string;
  params?: unknown;
}

export interface JsonRpcErrorData {
  /** Identifiant unique `VNL-RPC-*` (règle du projet, cf. AGENTS.md). */
  code: string;
}

export interface JsonRpcErrorObj {
  code: number;
  message: string;
  data: JsonRpcErrorData;
}

export interface JsonRpcResponse {
  jsonrpc: '2.0';
  id: number | string | null;
  result?: unknown;
  error?: JsonRpcErrorObj;
}

export interface JsonRpcNotification<T> {
  jsonrpc: '2.0';
  method: string;
  params: T;
}

export interface InitializeParams {
  protocolVersion: number;
  workspace?: string;
}

export interface InitializeResult {
  protocolVersion: number;
  serverVersion: string;
  workspaceRoot?: string;
  defaultAgent?: string;
}

export interface ConversationSummary {
  id: string;
  agent?: string;
  title?: string;
  messageCount: number;
}

export interface ChatSendParams {
  conversationId: string;
  message: string;
  agent?: string;
}

export interface ChatSendResult {
  text: string;
  toolCalls: ToolCallRecord[];
}

export interface ChatEventNotificationParams {
  conversationId: string;
  seq: number;
  event: ChatEvent;
}