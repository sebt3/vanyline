import { describe, it, expect, expectTypeOf } from 'vitest';
import type { HealthReport } from './generated/health-report';
import {
  STEP_KINDS,
} from './workflow-domain';
import type {
  GateStepConfig,
  HealthState,
  RunDetail,
  RunLaunch,
  RunSummary,
  StepDefinition,
  Workflow,
  WorkflowSummary,
} from './workflow-domain';

/** Types JSON acceptés par clé requise (`null` = valeur absente explicite). */
type Shape = Record<string, string[]>;

function kindOf(value: unknown): string {
  if (value === null) return 'null';
  if (Array.isArray(value)) return 'array';
  return typeof value;
}

function expectShape(label: string, value: object, shape: Shape): void {
  for (const [key, accepted] of Object.entries(shape)) {
    expect(value, `${label}: clé requise ${key}`).toHaveProperty(key);
    const actual = kindOf((value as Record<string, unknown>)[key]);
    expect(accepted, `${label}.${key} : type ${actual}`).toContain(actual);
  }
}

const HEALTH_REPORT: HealthReport = {
  revision: 'abc1234',
  checks: [
    {
      id: 'clippy',
      kind: 'lint',
      status: 'passed',
      exit_code: 0,
      duration_ms: 12,
      output_tail: '',
    },
  ],
  coverage: null,
  issues: [],
  new_code: null,
  unchecked_languages: ['go'],
};

const WORKFLOW_LIST: WorkflowSummary[] = [
  {
    id: 3,
    name: 'alice/demo/ci',
    scope: 'sandbox',
    description: 'Intégration continue',
    kinds: ['shell', 'gate'],
    steps_count: 2,
  },
  {
    id: 4,
    name: 'alice/demo/notify',
    scope: 'project',
    kinds: ['agent'],
    steps_count: 1,
  },
];

const WORKFLOW_DETAIL: Workflow = {
  ...WORKFLOW_LIST[0],
  steps: [
    { id: 'build', kind: 'shell', depends_on: [], config: { script: 'make' } },
    {
      id: 'gate',
      kind: 'gate',
      depends_on: ['build'],
      config: { min_coverage: null, enforce: true },
    },
  ],
};

const RUN_STATUS_HEADER = {
  invocation_id: 'inv-1',
  state: 'Running' as const,
  started_at: '2026-10-17T10:00:00Z',
  ended_at: null,
  error: null,
};

const RUN: RunSummary = {
  run_id: 'r1',
  workflow: 'alice/demo/ci',
  scope: 'sandbox',
  sandbox: 's1',
  triggered_by: 'ui',
  started_at: '2026-10-17T10:00:00Z',
  status: RUN_STATUS_HEADER,
};

const RUN_DETAIL: RunDetail = {
  ...RUN,
  status: {
    ...RUN_STATUS_HEADER,
    steps: [
      {
        step_id: 'build',
        state: 'Succeeded',
        started_at: '2026-10-17T10:00:01Z',
        ended_at: '2026-10-17T10:00:05Z',
        error: null,
      },
    ],
  },
};

const LAUNCH: RunLaunch = { run_id: 'r1', invocation_id: 'inv-1' };

const HEALTH_FRESH: HealthState = {
  phase: 'fresh',
  report: HEALTH_REPORT,
  summary: {
    status: 'passed',
    coverage_percent: null,
    files: [],
  },
  computed_at: 1760000000000,
  stale_since: null,
  base: null,
};

const HEALTH_COMPUTING: HealthState = {
  phase: 'computing',
  report: null,
  summary: null,
};

describe('WorkflowDomain conformance', () => {
  it('conformite_aux_fixtures — liste de workflows : clés requises et types', () => {
    for (const wf of WORKFLOW_LIST) {
      expectShape('WorkflowSummary', wf, {
        id: ['number'],
        name: ['string'],
        scope: ['string'],
        kinds: ['array'],
        steps_count: ['number'],
      });
    }
  });

  it('conformite_aux_fixtures — détail de workflow avec steps', () => {
    expectShape('Workflow', WORKFLOW_DETAIL, {
      id: ['number'],
      name: ['string'],
      scope: ['string'],
      kinds: ['array'],
      steps_count: ['number'],
      steps: ['array'],
    });
    for (const step of WORKFLOW_DETAIL.steps) {
      expectShape('StepDefinition', step, {
        id: ['string'],
        kind: ['string'],
        depends_on: ['array'],
        config: ['object'],
      });
    }
  });

  it('conformite_aux_fixtures — run (liste) et run (détail) avec statut', () => {
    expectShape('RunSummary', RUN, {
      run_id: ['string'],
      workflow: ['string'],
      scope: ['string'],
      triggered_by: ['string'],
      started_at: ['string'],
      status: ['object'],
    });
    expectShape('RunStatus', RUN_DETAIL.status, {
      invocation_id: ['string'],
      state: ['string'],
      started_at: ['string', 'null'],
      ended_at: ['string', 'null'],
      error: ['string', 'null'],
      steps: ['array'],
    });
    for (const step of RUN_DETAIL.status.steps) {
      expectShape('StepStatus', step, {
        step_id: ['string'],
        state: ['string'],
        started_at: ['string', 'null'],
        ended_at: ['string', 'null'],
        error: ['string', 'null'],
      });
    }
  });

  it('conformite_aux_fixtures — réponse de lancement', () => {
    expectShape('RunLaunch', LAUNCH, {
      run_id: ['string'],
      invocation_id: ['string'],
    });
  });

  it('conformite_aux_fixtures — état de santé : clés requises et types', () => {
    expectShape('HealthState', HEALTH_FRESH, {
      phase: ['string'],
      report: ['object'],
      summary: ['object'],
      computed_at: ['number', 'null'],
      stale_since: ['number', 'null'],
      base: ['string', 'null'],
    });
    expectShape('HealthState (computing)', HEALTH_COMPUTING, {
      phase: ['string'],
      report: ['null'],
      summary: ['null'],
    });
    expectShape('HealthSummary', HEALTH_FRESH.summary!, {
      status: ['string'],
      coverage_percent: ['number', 'null'],
      files: ['array'],
    });
  });

  it('sante_importee_du_genere — HealthState.report est le HealthReport généré', () => {
    expectTypeOf<NonNullable<HealthState['report']>>().toEqualTypeOf<HealthReport>();
    expect(HEALTH_FRESH.report).toEqual(HEALTH_REPORT);
  });

  it('kinds_fermes — exactement agent, rhai, shell, gate', () => {
    const exhaustive: Record<StepDefinition['kind'], true> = {
      agent: true,
      rhai: true,
      shell: true,
      gate: true,
    };
    expect(Object.keys(exhaustive).sort()).toEqual(['agent', 'gate', 'rhai', 'shell']);
    expect([...STEP_KINDS].sort()).toEqual(['agent', 'gate', 'rhai', 'shell']);
    expectTypeOf<StepDefinition['kind']>().toEqualTypeOf<
      'agent' | 'rhai' | 'shell' | 'gate'
    >();
  });

  it('kinds_fermes — config typée par kind (null désactive un seuil de gate)', () => {
    const gate: GateStepConfig = {
      min_coverage: null,
      min_new_code_coverage: 80,
      base: 'main',
      max_errors: null,
      ignore_checks: ['ts-coverage'],
      fail_on_unchecked: false,
      enforce: true,
      wait_timeout_secs: 900,
    };
    expect(gate.min_coverage).toBeNull();

    const rhai: StepDefinition = {
      id: 'probe',
      kind: 'rhai',
      depends_on: [],
      config: { script: '1 + 1' },
    };
    expect(rhai.kind).toBe('rhai');

    // @ts-expect-error — config d'un kind `agent` inappropriée pour `rhai`
    const mismatch: StepDefinition = { id: 'x', kind: 'rhai', depends_on: [], config: { agent: 'a' } };
    expect(mismatch.kind).toBe('rhai');
  });
});
