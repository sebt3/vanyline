<script setup lang="ts">
import { computed, ref, watch, type Directive } from 'vue';
import { EditorView, basicSetup } from 'codemirror';
import { oneDark } from '@codemirror/theme-one-dark';
import { StreamLanguage } from '@codemirror/language';
import type { Extension } from '@codemirror/state';
import type {
  StepDefinition,
  Workflow,
  WorkflowCreate,
  WorkflowScope,
  WorkflowUpdate,
} from '@vanyline/protocol';
import { ApiError } from '../../api/client';
import { rhaiMode } from '../panels/langRhai';
import { handlebarsMode } from '../panels/langHandlebars';

// Édition d'un workflow (workflows.sdd, WorkflowEditor). Le composant ne fait aucun appel
// réseau : il émet `save` avec le corps prêt pour createWorkflow / updateWorkflow. La validation
// du DAG reste côté serveur : les erreurs 400/422 sont rendues sur le step qu'elles citent.

type StepKind = StepDefinition['kind'];

/** Kinds proposés par portée (miroir de `allowed_kinds`, run_context.sdd). Proposition d'UI
 *  seulement : le serveur reste l'autorité et revalide à chaque écriture. */
const KINDS_PAR_PORTEE: Record<WorkflowScope, readonly StepKind[]> = {
  project: ['agent', 'rhai'],
  sandbox: ['agent', 'rhai', 'shell', 'gate'],
};

const props = withDefaults(
  defineProps<{
    workflow?: Workflow | null;
    defaultScope?: WorkflowScope;
    saving?: boolean;
    /** Dernière erreur de l'enregistrement (ApiError attendue pour l'attribution aux steps). */
    error?: unknown;
  }>(),
  { defaultScope: 'project', saving: false },
);

const emit = defineEmits<{
  save: [body: WorkflowCreate | WorkflowUpdate];
  cancel: [];
}>();

/** Step en cours d'édition : `config` ne porte PAS `env` (géré en lignes séparées). */
interface StepDraft {
  key: number;
  id: string;
  kind: StepKind;
  dependsOn: string;
  config: Record<string, unknown>;
  /** `visible` : état d'affichage local d'une ligne d'env (jamais persisté, jamais envoyé). */
  envRows: { key: string; value: string; visible: boolean }[];
}

/** Fragments de clé évoquant un secret (insensible à la casse) : valeur masquée à l'édition. */
const FRAGMENTS_SECRET = ['token', 'secret', 'password', 'passwd', 'pass', 'key', 'credential', 'auth'];

function isSecretKey(key: string): boolean {
  const k = key.trim().toLowerCase();
  return FRAGMENTS_SECRET.some((f) => k.includes(f));
}

interface CodeBinding {
  value: string;
  lang: 'rhai' | 'handlebars';
  onChange: (value: string) => void;
}

let nextKey = 0;

function defaultConfig(kind: StepKind): Record<string, unknown> {
  switch (kind) {
    case 'agent':
      return { agent: '', prompt: '' };
    case 'rhai':
    case 'shell':
      return { script: '' };
    case 'gate':
      return {};
  }
}

function toDraft(steps: StepDefinition[]): StepDraft[] {
  return steps.map((st) => {
    const config = { ...(st.config as Record<string, unknown>) };
    let envRows: StepDraft['envRows'] = [];
    if (st.kind === 'shell') {
      const env = (config.env ?? {}) as Record<string, string>;
      delete config.env;
      envRows = Object.entries(env).map(([key, value]) => ({ key, value, visible: false }));
    }
    return {
      key: nextKey++,
      id: st.id,
      kind: st.kind,
      dependsOn: (st.depends_on ?? []).join(', '),
      config,
      envRows,
    };
  });
}

function buildStep(s: StepDraft): StepDefinition {
  const config: Record<string, unknown> = { ...s.config };
  if (config.cwd === '') delete config.cwd;
  if (config.base === '') delete config.base;
  if (s.kind === 'shell') {
    const env = Object.fromEntries(
      s.envRows.filter((r) => r.key.trim() !== '').map((r) => [r.key.trim(), r.value]),
    );
    if (Object.keys(env).length > 0) config.env = env;
  }
  return {
    id: s.id.trim(),
    kind: s.kind,
    depends_on: s.dependsOn
      .split(',')
      .map((d) => d.trim())
      .filter((d) => d !== ''),
    config,
  } as StepDefinition;
}

const isEdit = computed(() => props.workflow != null);
const name = ref(props.workflow?.name ?? '');
const description = ref(props.workflow?.description ?? '');
const scope = ref<WorkflowScope>(props.workflow?.scope ?? props.defaultScope);
const draft = ref<StepDraft[]>(props.workflow ? toDraft(props.workflow.steps) : []);
const mode = ref<'structure' | 'source'>('structure');
const sourceText = ref('');
const localError = ref<string | null>(null);

// Un nouveau workflow chargé par le parent remplace le brouillon.
watch(
  () => props.workflow,
  (w) => {
    name.value = w?.name ?? '';
    description.value = w?.description ?? '';
    scope.value = w?.scope ?? props.defaultScope;
    draft.value = w ? toDraft(w.steps) : [];
    mode.value = 'structure';
    localError.value = null;
  },
);

function kindOptions(s: StepDraft): StepKind[] {
  const allowed = KINDS_PAR_PORTEE[scope.value];
  // Le kind courant reste visible même s'il n'est plus proposé (donnée existante) : le serveur
  // tranche, l'utilisateur voit la valeur réelle plutôt qu'un select vide.
  return allowed.includes(s.kind) ? [...allowed] : [...allowed, s.kind];
}

function inputValue(e: Event): string {
  return (e.target as HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement).value;
}

function inputChecked(e: Event): boolean {
  return (e.target as HTMLInputElement).checked;
}

function text(s: StepDraft, key: string): string {
  const v = s.config[key];
  return typeof v === 'string' ? v : '';
}

function numText(s: StepDraft, key: string): string {
  const v = s.config[key];
  return typeof v === 'number' ? String(v) : '';
}

function listText(s: StepDraft, key: string): string {
  const v = s.config[key];
  return Array.isArray(v) ? v.join(', ') : '';
}

function setText(s: StepDraft, key: string, value: string): void {
  s.config[key] = value;
}

/** Champ numérique vide = clé ABSENTE du step envoyé (seuil désactivé, sans `null`). */
function setNum(s: StepDraft, key: string, e: Event): void {
  const raw = inputValue(e).trim();
  const n = Number(raw);
  if (raw === '' || !Number.isFinite(n)) delete s.config[key];
  else s.config[key] = n;
}

function setList(s: StepDraft, key: string, e: Event): void {
  const items = inputValue(e)
    .split(',')
    .map((x) => x.trim())
    .filter((x) => x !== '');
  if (items.length === 0) delete s.config[key];
  else s.config[key] = items;
}

function setBool(s: StepDraft, key: string, value: boolean): void {
  s.config[key] = value;
}

function setKind(s: StepDraft, kind: StepKind): void {
  if (kind === s.kind) return;
  s.kind = kind;
  s.config = defaultConfig(kind);
  s.envRows = [];
}

function addStep(): void {
  draft.value.push({
    key: nextKey++,
    id: `step${draft.value.length + 1}`,
    kind: 'agent',
    dependsOn: '',
    config: defaultConfig('agent'),
    envRows: [],
  });
}

function removeStep(index: number): void {
  draft.value.splice(index, 1);
}

function addEnvRow(s: StepDraft): void {
  s.envRows.push({ key: '', value: '', visible: false });
}

function toggleEnvVisible(row: StepDraft['envRows'][number]): void {
  row.visible = !row.visible;
}

function removeEnvRow(s: StepDraft, index: number): void {
  s.envRows.splice(index, 1);
}

// --- Éditeurs CodeMirror (coloration Rhai / Handlebars) ----------------------------------------

const rhaiLanguage = StreamLanguage.define(rhaiMode);
const handlebarsLanguage = StreamLanguage.define(handlebarsMode);
const codeViews = new WeakMap<HTMLElement, { view: EditorView; binding: CodeBinding }>();

/** Une instance CodeMirror par champ de script/prompt ; le texte reste la source de vérité
 *  côté `draft` (onChange), l'éditeur n'est re-synchronisé que si la valeur diverge. */
const vCodeEditor: Directive<HTMLElement, CodeBinding> = {
  mounted(el, binding) {
    const extensions: Extension[] = [
      basicSetup,
      oneDark,
      EditorView.lineWrapping,
      binding.value.lang === 'rhai' ? rhaiLanguage : handlebarsLanguage,
      EditorView.updateListener.of((update) => {
        if (!update.docChanged) return;
        const entry = codeViews.get(el);
        entry?.binding.onChange(update.state.doc.toString());
      }),
    ];
    const view = new EditorView({ doc: binding.value.value, parent: el, extensions });
    codeViews.set(el, { view, binding: binding.value });
  },
  updated(el, binding) {
    const entry = codeViews.get(el);
    if (!entry) return;
    entry.binding = binding.value;
    if (entry.view.state.doc.toString() !== binding.value.value) {
      entry.view.dispatch({
        changes: { from: 0, to: entry.view.state.doc.length, insert: binding.value.value },
      });
    }
  },
  unmounted(el) {
    codeViews.get(el)?.view.destroy();
    codeViews.delete(el);
  },
};

// --- Mode source (JSON du DAG) -----------------------------------------------------------------

function switchMode(target: 'structure' | 'source'): void {
  if (target === mode.value) return;
  if (target === 'source') {
    sourceText.value = JSON.stringify(draft.value.map(buildStep), null, 2);
    localError.value = null;
    mode.value = 'source';
    return;
  }
  const parsed = parseSteps(sourceText.value);
  if (typeof parsed === 'string') {
    localError.value = parsed;
    return;
  }
  draft.value = toDraft(parsed);
  localError.value = null;
  mode.value = 'structure';
}

/** Syntaxe seulement : un JSON valide qui est une liste ; le contenu du DAG est validé par l'API. */
function parseSteps(raw: string): StepDefinition[] | string {
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch (err) {
    return `JSON invalide : ${err instanceof Error ? err.message : String(err)}`;
  }
  if (!Array.isArray(value)) return 'Le DAG doit être une liste de steps (tableau JSON).';
  return value as StepDefinition[];
}

// --- Erreurs de l'API ------------------------------------------------------------------------

const apiMessage = computed<string | null>(() =>
  props.error instanceof Error ? props.error.message : props.error ? String(props.error) : null,
);

/** Le step est désigné par son id entre guillemets (backticks, apostrophes ou guillemets)
 *  dans le message 400/422 ; à défaut, le message reste en bandeau. */
function citesStep(message: string, id: string): boolean {
  return [...message.matchAll(/[`'"]([^`'"]+)[`'"]/g)].some((m) => m[1] === id);
}

const stepErrors = computed<Map<number, string>>(() => {
  const map = new Map<number, string>();
  const msg = apiMessage.value;
  const status = props.error instanceof ApiError ? props.error.status : null;
  if (!msg || (status !== 400 && status !== 422)) return map;
  for (const s of draft.value) {
    if (citesStep(msg, s.id)) map.set(s.key, msg);
  }
  return map;
});

const bannerMessage = computed<string | null>(() => {
  if (localError.value) return localError.value;
  if (!apiMessage.value || stepErrors.value.size > 0) return null;
  return apiMessage.value;
});

// --- Enregistrement --------------------------------------------------------------------------

const canSave = computed(() => !props.saving && (isEdit.value || name.value.trim() !== ''));

function onSave(): void {
  localError.value = null;
  let steps: StepDefinition[];
  if (mode.value === 'source') {
    const parsed = parseSteps(sourceText.value);
    if (typeof parsed === 'string') {
      localError.value = parsed;
      return;
    }
    steps = parsed;
  } else {
    steps = draft.value.map(buildStep);
  }

  if (props.workflow) {
    const body: WorkflowUpdate = { description: description.value.trim(), steps };
    emit('save', body);
    return;
  }
  const body: WorkflowCreate = {
    name: name.value.trim(),
    scope: scope.value,
    steps,
  };
  if (description.value.trim() !== '') body.description = description.value.trim();
  emit('save', body);
}
</script>

<template>
  <section class="wfe" data-testid="workflow-editor">
    <header class="wfe-head">
      <h2 class="wfe-title">{{ isEdit ? 'Modifier le workflow' : 'Nouveau workflow' }}</h2>
      <div class="wfe-modes" role="group" aria-label="Mode d'édition">
        <button
          type="button"
          data-testid="mode-structure"
          :aria-pressed="mode === 'structure'"
          :class="{ active: mode === 'structure' }"
          @click="switchMode('structure')"
        >
          Structuré
        </button>
        <button
          type="button"
          data-testid="mode-source"
          :aria-pressed="mode === 'source'"
          :class="{ active: mode === 'source' }"
          @click="switchMode('source')"
        >
          Source JSON
        </button>
      </div>
    </header>

    <p v-if="bannerMessage" class="wfe-banner" role="alert" data-testid="editor-error">
      {{ bannerMessage }}
    </p>

    <form class="wfe-form" @submit.prevent="onSave">
      <div class="wfe-meta">
        <label class="wfe-field">
          <span class="wfe-label">Nom</span>
          <input
            data-testid="name"
            type="text"
            :value="name"
            :disabled="isEdit"
            @input="name = inputValue($event)"
          />
        </label>
        <label class="wfe-field">
          <span class="wfe-label">Portée</span>
          <select
            data-testid="scope"
            :value="scope"
            :disabled="isEdit"
            @change="scope = inputValue($event) as WorkflowScope"
          >
            <option value="project">Projet</option>
            <option value="sandbox">Sandbox</option>
          </select>
        </label>
        <label class="wfe-field wfe-field--wide">
          <span class="wfe-label">Description</span>
          <input
            data-testid="description"
            type="text"
            :value="description"
            @input="description = inputValue($event)"
          />
        </label>
      </div>

      <div v-if="mode === 'structure'" class="wfe-steps">
        <article
          v-for="(step, i) in draft"
          :key="step.key"
          class="wfe-step"
          data-testid="step"
          :data-step-id="step.id"
        >
          <div class="wfe-step-head">
            <span class="wfe-step-num">Step {{ i + 1 }}</span>
            <button type="button" :data-testid="`remove-step-${i}`" @click="removeStep(i)">
              Supprimer
            </button>
          </div>

          <div class="wfe-meta">
            <label class="wfe-field">
              <span class="wfe-label">Id</span>
              <input
                :data-testid="`step-id-${i}`"
                type="text"
                :value="step.id"
                @input="step.id = inputValue($event)"
              />
            </label>
            <label class="wfe-field">
              <span class="wfe-label">Kind</span>
              <select
                :data-testid="`step-kind-${i}`"
                :value="step.kind"
                @change="setKind(step, inputValue($event) as StepKind)"
              >
                <option v-for="k in kindOptions(step)" :key="k" :value="k">{{ k }}</option>
              </select>
            </label>
            <label class="wfe-field wfe-field--wide">
              <span class="wfe-label">Dépend de</span>
              <input
                :data-testid="`step-deps-${i}`"
                type="text"
                placeholder="ids séparés par des virgules"
                :value="step.dependsOn"
                @input="step.dependsOn = inputValue($event)"
              />
            </label>
          </div>

          <!-- agent -->
          <div v-if="step.kind === 'agent'" class="wfe-meta">
            <label class="wfe-field">
              <span class="wfe-label">Agent</span>
              <input
                :data-testid="`agent-name-${i}`"
                type="text"
                :value="text(step, 'agent')"
                @input="setText(step, 'agent', inputValue($event))"
              />
            </label>
            <div class="wfe-field wfe-field--wide">
              <span class="wfe-label">Prompt (Handlebars)</span>
              <div
                :data-testid="`prompt-${i}`"
                class="wfe-code"
                v-code-editor="{
                  value: text(step, 'prompt'),
                  lang: 'handlebars',
                  onChange: (v: string) => setText(step, 'prompt', v),
                }"
              />
            </div>
            <label class="wfe-field">
              <span class="wfe-label">Délai (s)</span>
              <input
                :data-testid="`timeout-${i}`"
                type="number"
                min="1"
                :value="numText(step, 'timeout_secs')"
                @input="setNum(step, 'timeout_secs', $event)"
              />
            </label>
            <label class="wfe-field">
              <span class="wfe-label">Max étapes</span>
              <input
                :data-testid="`max-steps-${i}`"
                type="number"
                min="1"
                :value="numText(step, 'max_steps')"
                @input="setNum(step, 'max_steps', $event)"
              />
            </label>
            <label class="wfe-check">
              <input
                :data-testid="`json-${i}`"
                type="checkbox"
                :checked="step.config.json === true"
                @change="setBool(step, 'json', inputChecked($event))"
              />
              <span>Extraire du JSON de la sortie</span>
            </label>
          </div>

          <!-- rhai -->
          <div v-else-if="step.kind === 'rhai'" class="wfe-field wfe-field--wide">
            <span class="wfe-label">Script (Rhai)</span>
            <div
              :data-testid="`script-${i}`"
              class="wfe-code"
              v-code-editor="{
                value: text(step, 'script'),
                lang: 'rhai',
                onChange: (v: string) => setText(step, 'script', v),
              }"
            />
          </div>

          <!-- shell -->
          <div v-else-if="step.kind === 'shell'" class="wfe-meta">
            <label class="wfe-field wfe-field--wide">
              <span class="wfe-label">Script (shell)</span>
              <textarea
                :data-testid="`shell-script-${i}`"
                rows="6"
                spellcheck="false"
                :value="text(step, 'script')"
                @input="setText(step, 'script', inputValue($event))"
              />
            </label>
            <label class="wfe-field">
              <span class="wfe-label">Répertoire</span>
              <input
                :data-testid="`shell-cwd-${i}`"
                type="text"
                :value="text(step, 'cwd')"
                @input="setText(step, 'cwd', inputValue($event))"
              />
            </label>
            <label class="wfe-field">
              <span class="wfe-label">Délai (s)</span>
              <input
                :data-testid="`timeout-${i}`"
                type="number"
                min="1"
                :value="numText(step, 'timeout_secs')"
                @input="setNum(step, 'timeout_secs', $event)"
              />
            </label>
            <div class="wfe-env wfe-field--wide">
              <span class="wfe-label">Variables d'environnement</span>
              <div v-for="(row, r) in step.envRows" :key="r" class="wfe-env-row">
                <input
                  :data-testid="`env-key-${i}-${r}`"
                  type="text"
                  placeholder="NOM"
                  :value="row.key"
                  @input="row.key = inputValue($event)"
                />
                <input
                  :data-testid="`env-value-${i}-${r}`"
                  :type="isSecretKey(row.key) && !row.visible ? 'password' : 'text'"
                  autocomplete="off"
                  placeholder="valeur"
                  :value="row.value"
                  @input="row.value = inputValue($event)"
                />
                <button
                  v-if="isSecretKey(row.key)"
                  type="button"
                  :data-testid="`env-toggle-${i}-${r}`"
                  @click="toggleEnvVisible(row)"
                >
                  {{ row.visible ? 'masquer' : 'afficher' }}
                </button>
                <button type="button" @click="removeEnvRow(step, r)">Retirer</button>
              </div>
              <button type="button" :data-testid="`add-env-${i}`" @click="addEnvRow(step)">
                Ajouter une variable
              </button>
            </div>
            <label class="wfe-check">
              <input
                :data-testid="`allow-failure-${i}`"
                type="checkbox"
                :checked="step.config.allow_failure === true"
                @change="setBool(step, 'allow_failure', inputChecked($event))"
              />
              <span>Tolérer l'échec</span>
            </label>
          </div>

          <!-- gate -->
          <div v-else class="wfe-meta">
            <label class="wfe-field">
              <span class="wfe-label">Couverture minimale (%)</span>
              <input
                :data-testid="`gate-min-coverage-${i}`"
                type="number"
                min="0"
                max="100"
                placeholder="désactivée"
                :value="numText(step, 'min_coverage')"
                @input="setNum(step, 'min_coverage', $event)"
              />
            </label>
            <label class="wfe-field">
              <span class="wfe-label">Couverture du nouveau code (%)</span>
              <input
                :data-testid="`gate-min-new-coverage-${i}`"
                type="number"
                min="0"
                max="100"
                placeholder="désactivée"
                :value="numText(step, 'min_new_code_coverage')"
                @input="setNum(step, 'min_new_code_coverage', $event)"
              />
            </label>
            <label class="wfe-field">
              <span class="wfe-label">Base git</span>
              <input
                :data-testid="`gate-base-${i}`"
                type="text"
                :value="text(step, 'base')"
                @input="setText(step, 'base', inputValue($event))"
              />
            </label>
            <label class="wfe-field">
              <span class="wfe-label">Erreurs max</span>
              <input
                :data-testid="`gate-max-errors-${i}`"
                type="number"
                min="0"
                placeholder="désactivé"
                :value="numText(step, 'max_errors')"
                @input="setNum(step, 'max_errors', $event)"
              />
            </label>
            <label class="wfe-field wfe-field--wide">
              <span class="wfe-label">Vérifications ignorées</span>
              <input
                :data-testid="`gate-ignore-${i}`"
                type="text"
                placeholder="ids séparés par des virgules"
                :value="listText(step, 'ignore_checks')"
                @input="setList(step, 'ignore_checks', $event)"
              />
            </label>
            <label class="wfe-check">
              <input
                :data-testid="`gate-fail-unchecked-${i}`"
                type="checkbox"
                :checked="step.config.fail_on_unchecked === true"
                @change="setBool(step, 'fail_on_unchecked', inputChecked($event))"
              />
              <span>Échouer si un langage n'est pas vérifié</span>
            </label>
            <label class="wfe-check">
              <input
                :data-testid="`gate-enforce-${i}`"
                type="checkbox"
                :checked="step.config.enforce !== false"
                @change="setBool(step, 'enforce', inputChecked($event))"
              />
              <span>Appliquer (décochée = observation seulement)</span>
            </label>
            <label class="wfe-field">
              <span class="wfe-label">Attente max (s)</span>
              <input
                :data-testid="`gate-wait-${i}`"
                type="number"
                min="1"
                :value="numText(step, 'wait_timeout_secs')"
                @input="setNum(step, 'wait_timeout_secs', $event)"
              />
            </label>
          </div>

          <p v-if="stepErrors.get(step.key)" class="wfe-step-error" data-testid="step-error" role="alert">
            {{ stepErrors.get(step.key) }}
          </p>
        </article>

        <button type="button" class="wfe-add" data-testid="add-step" @click="addStep">
          Ajouter un step
        </button>
      </div>

      <div v-else class="wfe-source">
        <textarea
          data-testid="source-json"
          rows="20"
          spellcheck="false"
          :value="sourceText"
          @input="sourceText = inputValue($event)"
        />
        <p v-if="localError" class="wfe-banner" data-testid="source-error" role="alert">
          {{ localError }}
        </p>
      </div>

      <footer class="wfe-actions">
        <button type="button" data-testid="cancel" @click="emit('cancel')">Annuler</button>
        <button type="submit" data-testid="save" class="wfe-primary" :disabled="!canSave">
          {{ saving ? 'Enregistrement…' : 'Enregistrer' }}
        </button>
      </footer>
    </form>
  </section>
</template>

<style scoped>
/* Responsive : colonne unique ; sous md (768 px) l'éditeur devient un plein écran. */
.wfe {
  max-width: 760px;
  margin: 0 auto;
  padding: 16px;
  color: #e6e9f0;
  background: #101828;
  border: 1px solid #1c1c2a;
  border-radius: 10px;
}

.wfe-head {
  display: flex;
  flex-wrap: wrap;
  gap: 12px;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 12px;
}

.wfe-title {
  margin: 0;
  font-size: 15px;
  font-weight: 600;
}

.wfe-modes {
  display: flex;
  gap: 8px;
}

.wfe-modes button.active {
  font-weight: 600;
  border-color: #5b1ecf;
}

.wfe-banner {
  margin: 0 0 12px;
  padding: 10px 12px;
  border: 1px solid #7f1d1d;
  border-radius: 6px;
  background: #2a1215;
  color: #fca5a5;
  font-size: 13px;
  white-space: pre-wrap;
}

.wfe-form {
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.wfe-meta {
  display: grid;
  grid-template-columns: 1fr;
  gap: 12px;
}

.wfe-field {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
}

.wfe-label {
  font-size: 12px;
  font-weight: 600;
  color: #6a7185;
  text-transform: uppercase;
}

.wfe-check {
  display: flex;
  align-items: center;
  gap: 8px;
  min-height: var(--ctl-h);
}

input,
select,
textarea,
button {
  font: inherit;
  color: inherit;
}

input,
select,
textarea {
  width: 100%;
  min-height: var(--ctl-h);
  padding: 8px 10px;
  background: #0b1220;
  border: 1px solid #1c1c2a;
  border-radius: 6px;
}

textarea {
  font-family: ui-monospace, 'SFMono-Regular', Menlo, Consolas, monospace;
  font-size: 12.5px;
}

button {
  min-height: var(--ctl-h);
  padding: 8px 14px;
  background: #1c2433;
  border: 1px solid #2a3346;
  border-radius: 6px;
  cursor: pointer;
}

button:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.wfe-primary {
  background: #5b1ecf;
  border-color: #5b1ecf;
}

.wfe-steps {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.wfe-step {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 12px;
  border: 1px solid #1c1c2a;
  border-radius: 8px;
}

.wfe-step-head {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 8px;
}

.wfe-step-num {
  font-weight: 600;
}

.wfe-step-error {
  margin: 0;
  color: #fca5a5;
  font-size: 13px;
}

.wfe-code {
  min-height: 140px;
  border: 1px solid #1c1c2a;
  border-radius: 6px;
  overflow: hidden;
  text-align: left;
}

.wfe-env {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.wfe-env-row {
  display: grid;
  grid-template-columns: 1fr 1fr auto auto;
  gap: 8px;
}

.wfe-add {
  align-self: flex-start;
}

.wfe-source {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.wfe-actions {
  display: flex;
  justify-content: flex-end;
  gap: 12px;
}

/* Écrans larges : colonne centrée ; étroit : plein écran (dialogue plein écran sous md). */
@media (min-width: 768px) {
  .wfe-meta {
    grid-template-columns: 1fr 1fr;
  }
  .wfe-field--wide {
    grid-column: 1 / -1;
  }
  .wfe-env,
  .wfe-code,
  .wfe-field--wide {
    grid-column: 1 / -1;
  }
}

@media (max-width: 767px) {
  .wfe {
    position: fixed;
    inset: 0;
    z-index: 1001;
    max-width: none;
    overflow-y: auto;
    border-radius: 0;
    border: none;
  }
}
</style>
