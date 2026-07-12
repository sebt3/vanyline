export interface User {
  id: string;
  email: string;
}

export type LlmProviderType = 'ollama' | 'openai-compatible';

export interface LlmProvider {
  id: string;
  name: string;
  provider_type: LlmProviderType;
  endpoint: string;
  api_key?: string | null;
  available_models: string[];
  is_default: boolean;
  created_at: string;
  updated_at: string;
}

export type McpServerType = 'sse' | 'http-streamable';

export interface McpServer {
  id: string;
  name: string;
  server_type: McpServerType;
  url: string;
  headers: Record<string, string>;
  created_at: string;
  updated_at: string;
}

export type AgentMode = 'primary' | 'subagent' | 'all';

/** "auto" | "none" | liste de noms de skills — cf. lib/src/domain.rs::SkillSelection (Rust). */
export type SkillSelection = 'auto' | 'none' | string[];

export interface Agent {
  name: string;
  description?: string | null;
  mode: AgentMode;
  model: string;
  toolsets: string[];
  skills: SkillSelection;
  system_prompt: string;
}

export interface Conversation {
  id: string;
  user_id: string;
  agent_name?: string | null;
  title?: string | null;
  created_at: string;
  updated_at: string;
}

export type MessageRole = 'user' | 'assistant' | 'tool';

export interface Message {
  id: string;
  conversation_id: string;
  role: MessageRole;
  payload: MessagePayload;
  created_at: string;
}

export interface MessagePayload {
  content?: string | null;
  tool_calls?: ToolCall[] | null;
  tool_call_id?: string | null;
  name?: string | null;
}

export interface ToolCall {
  id: string;
  type: 'function';
  function: {
    name: string;
    arguments: string;
  };
}

export type WsClientMessage =
  | { type: 'message'; content: string };

export type WsServerMessage =
  | { type: 'token'; content: string }
  | { type: 'tool_call'; name: string; args: Record<string, unknown> }
  | { type: 'done'; message_id: string }
  | { type: 'error'; code: string; message: string };
