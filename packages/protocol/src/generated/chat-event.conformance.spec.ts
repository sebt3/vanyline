import { describe, it, expect } from 'vitest';
import type { ChatEvent } from './chat-event';

const FIXTURES: ChatEvent[] = [
  { type: 'token', content: 'hello' },
  { type: 'reasoning_delta', content: 'je réfléchis' },
  { type: 'tool_call', id: 'c1', name: 'search', args: { q: 'x' } },
  { type: 'tool_result', id: 'c1', name: 'search', result: '42', is_error: false },
  { type: 'skill_loaded', name: 'my-skill' },
  { type: 'subagent_start', id: 'sub-1', agent: 'dev', task: 'test' },
  { type: 'subagent_event', id: 's1', event: { type: 'token', content: 'x' } },
  { type: 'subagent_end', id: 's1', result: 'done' },
  { type: 'usage', input_tokens: 10, output_tokens: 5 },
  { type: 'done' },
  { type: 'error', code: 'ERR-001', message: 'broke' },
  { type: 'tool_unavailable', server: 'sandbox', reason: 'sandbox introuvable' },
];

describe('ChatEvent conformance', () => {
  it('compile-time assignability — 12 wire-format tags', () => {
    // L'assignabilité de FIXTURES (typé ChatEvent[]) garantit que chaque
    // fixture a le bon type et les bons champs. Tsc/npm run check échouerait
    // à la compilation si le type était infidèle (tag non snake_case, champ
    // absent, u64→bigint, variante manquante).
    expect(FIXTURES).toHaveLength(12);
  });

  it('roundtrip JSON — tous les tags sont conservés', () => {
    const expectedTags = [
      'token',
      'reasoning_delta',
      'tool_call',
      'tool_result',
      'skill_loaded',
      'subagent_start',
      'subagent_event',
      'subagent_end',
      'usage',
      'done',
      'error',
      'tool_unavailable',
    ];

    for (const fixture of FIXTURES) {
      const json = JSON.stringify(fixture);
      const parsed = JSON.parse(json) as Record<string, unknown>;
      const tag = parsed['type'];
      expect(expectedTags).toContain(tag);
    }
  });

  it('roundtrip JSON — assignabilité post-parse', () => {
    for (const fixture of FIXTURES) {
      const json = JSON.stringify(fixture);
      const parsed = JSON.parse(json);
      // En TS strict, le cast est sûr car on sait que tous les champs
      // correspondent au type wire (roundtrip JSON d'un objet typé
      // garde les mêmes clés et types primitives).
      const reTyped = parsed as ChatEvent;
      expect(reTyped).toBeDefined();
      expect((reTyped as { type: string }).type).toBe((fixture as { type: string }).type);
    }
  });
});