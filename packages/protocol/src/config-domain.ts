// Miroir TS manuel de lib/src/domain.rs (serde). Forme = wire RPC de F2.
// DO NOT drift — cf. tests de conformité config-domain.conformance.spec.ts
// et lib/src/domain.rs (tests *_wire_shape).

export type ProviderType = 'ollama' | 'openai-compatible';

/** Couche dont une entrée de config est résolue (fusion workspace/global).
 *  `source` en lecture RPC uniquement (`config/<domain>` liste et
 *  `config/skills/get`) — jamais sur le wire d'écriture, jamais côté web
 *  (`app` n'a pas de notion de couche). Sert au badge de couche de l'extension. */
export type ConfigEntrySource = 'workspace' | 'global';

export interface Provider {
  name: string;
  type: ProviderType;
  endpoint: string;
  api_key?: string;
  /** Web-augmenté, lecture seule. Peuplé par `app` après un test. RPC → []. */
  available_models?: string[];
  /** Web-augmenté, lecture seule. Concept web-only. RPC → false. */
  is_default?: boolean;
  /** RPC-augmenté, lecture seule. Couche source de l'entrée (tâche F4).
   *  Absent côté web et sur le wire d'écriture. */
  source?: ConfigEntrySource;
}

export interface ModelProfile {
  name: string;
  /** Nom d'un `Provider` (jamais un id). */
  provider: string;
  model: string;
  temperature?: number;
  max_tokens?: number;
  options?: Record<string, unknown>;
  /** RPC-augmenté, lecture seule. Couche source de l'entrée (tâche F4).
   *  Absent côté web et sur le wire d'écriture. */
  source?: ConfigEntrySource;
}

/** `http-streamable` (le seul modélisé par `lib/src/domain.rs` à ce jour) ou
 *  `sse`. Les MCP se connectent au moteur d'inférence, qui supporte les deux.
 *  `domain.rs::McpTransport` doit gagner `Sse` en F2 (réconciliation RPC) —
 *  divergence intentionnelle et documentée en attendant. */
export type McpTransport = 'sse' | 'http-streamable';

export interface McpSelection {
  server: string;
  /** Patterns glob sur les noms d'outils. Vide = tous. */
  tools: string[];
}

export interface McpServer {
  name: string;
  type: McpTransport;
  url: string;
  headers?: Record<string, string>;
  /** Web-augmenté, lecture seule. Peuplé par `app` après une découverte. RPC → []. */
  available_tools?: string[];
  /** RPC-augmenté, lecture seule. Couche source de l'entrée (tâche F4).
   *  Absent côté web et sur le wire d'écriture. */
  source?: ConfigEntrySource;
}

export interface Toolset {
  name: string;
  description?: string;
  prompt?: string;
  local_tools: string[];
  mcp: McpSelection[];
  /** RPC-augmenté, lecture seule. Couche source de l'entrée (tâche F4).
   *  Absent côté web et sur le wire d'écriture. */
  source?: ConfigEntrySource;
}

export type AgentMode = 'primary' | 'subagent' | 'all';

/** `'auto'` (tous les skills), `'none'`, ou une liste explicite de noms. */
export type SkillSelection = 'auto' | 'none' | string[];

export interface Agent {
  name: string;
  description?: string;
  mode: AgentMode;
  /** Nom d'un `ModelProfile` (jamais un id). */
  model: string;
  toolsets: string[];
  skills: SkillSelection;
  system_prompt: string;
  /** RPC-augmenté, lecture seule. Couche source de l'entrée (tâche F4).
   *  Absent côté web et sur le wire d'écriture. */
  source?: ConfigEntrySource;
}

export interface SkillMeta {
  name: string;
  description: string;
  /** RPC-augmenté, lecture seule. Couche source de l'entrée (tâche F4).
   *  Absent côté web et sur le wire d'écriture. */
  source?: ConfigEntrySource;
}

export interface SkillDetail extends SkillMeta {
  body: string;
}
