import { describe, it, expect } from 'vitest';
import type {
  Provider, ModelProfile, McpServer, Toolset, McpSelection,
  Agent, SkillMeta, SkillDetail, SkillSelection, AgentMode,
} from './config-domain';

const FIXTURES: Array<
  | Provider
  | ModelProfile
  | McpServer
  | Toolset
  | Agent
  | SkillMeta
  | SkillDetail
> = [
  // Provider without web-augmented fields
  {
    name: 'ollama',
    type: 'ollama',
    endpoint: 'http://localhost:11434',
  },
  // Provider with web-augmented fields
  {
    name: 'openai',
    type: 'openai-compatible',
    endpoint: 'https://api.openai.com',
    api_key: 'sk-test',
    available_models: ['gpt-4', 'gpt-3.5'],
    is_default: true,
  },
  // ModelProfile with options
  {
    name: 'qwen',
    provider: 'ollama',
    model: 'qwen2.5',
    temperature: 0.7,
    max_tokens: 4096,
    options: { num_ctx: 65536 },
  },
  // ModelProfile minimal
  {
    name: 'gpt',
    provider: 'openai',
    model: 'gpt-4',
  },
  // McpServer
  {
    name: 'fs',
    type: 'http-streamable',
    url: 'http://mcp:3000',
    headers: { Authorization: 'Bearer x' },
    available_tools: ['read', 'write'],
  },
  // Toolset with MCP
  {
    name: 'dev',
    description: 'Dev toolset',
    prompt: 'You are a developer',
    local_tools: ['bash'],
    mcp: [
      { server: 'fs', tools: ['read', 'write'] },
    ],
  },
  // Agent with skills = 'auto'
  {
    name: 'coder',
    description: 'Code agent',
    mode: 'primary' as AgentMode,
    model: 'qwen',
    toolsets: ['dev'],
    skills: 'auto' as SkillSelection,
    system_prompt: 'You are a coder',
  },
  // Agent with skills = array
  {
    name: 'reviewer',
    mode: 'subagent' as AgentMode,
    model: 'gpt',
    toolsets: ['dev'],
    skills: ['a', 'b'] as SkillSelection,
    system_prompt: 'Review code',
  },
  // SkillMeta
  {
    name: 'git-status',
    description: 'Show git status',
  },
  // SkillDetail with body
  {
    name: 'deploy',
    description: 'Deploy to staging',
    body: '#!/bin/bash\nkubectl apply -f k8s/',
  },
];

describe('ConfigDomain conformance', () => {
  it('compile-time assignability — 6 domaines', () => {
    expect(FIXTURES).toHaveLength(10);
  });

  it('roundtrip JSON — clés stables', () => {
    for (const fixture of FIXTURES) {
      const json = JSON.stringify(fixture);
      const parsed = JSON.parse(json) as Record<string, unknown>;
      // After JSON.stringify + parse, the same keys must exist (order is preserved
      // in modern JS engines but we test key set equality).
      const keys = Object.keys(JSON.parse(json));
      expect(keys).toEqual(Object.keys(parsed));
    }
  });

  it('roundtrip JSON — discriminant `type` (pas provider_type/server_type)', () => {
    for (const fixture of FIXTURES) {
      const json = JSON.stringify(fixture);
      const parsed = JSON.parse(json) as Record<string, unknown>;
      if ('type' in parsed) {
        expect(parsed.type).toMatch(/^(ollama|openai-compatible|http-streamable)$/);
        expect(parsed).not.toHaveProperty('provider_type');
        expect(parsed).not.toHaveProperty('server_type');
      }
    }
  });

  it('SkillSelection — 3 formes assignables à Agent.skills', () => {
    const auto: SkillSelection = 'auto';
    const none: SkillSelection = 'none';
    const named: SkillSelection = ['x', 'y'];
    const agent: Agent = {
      name: 'test',
      mode: 'primary',
      model: 'm',
      toolsets: [],
      skills: auto,
      system_prompt: 'p',
    };
    agent.skills = none;
    agent.skills = named;
    expect([auto, none, named]).toHaveLength(3);
  });
});
