export const GATE_NAMES = [
  'plan',
  'permission',
  'tool_trust',
  'verification',
  'runtime',
] as const;

export type GateName = (typeof GATE_NAMES)[number];

export const CONDITIONS = [
  'governed',
  'record_only_plan',
  'record_only_permission',
  'record_only_tool_trust',
  'record_only_verification',
  'record_only_runtime',
] as const;

export type TrialCondition = (typeof CONDITIONS)[number];

export type TrialPhase =
  | 'created'
  | 'preparing'
  | 'planning'
  | 'awaiting_approval'
  | 'executing'
  | 'verifying'
  | 'terminal';

export type TerminalStatus =
  | 'task_succeeded'
  | 'task_failed'
  | 'policy_stopped'
  | 'budget_stopped'
  | 'provider_failed'
  | 'timed_out'
  | 'infrastructure_failed'
  | 'cancelled';

export type ApprovalKind =
  | 'plan'
  | 'permission'
  | 'tool_trust'
  | 'runtime';

export interface Usage {
  inputTokens: number;
  outputTokens: number;
  cachedInputTokens?: number;
  cacheWriteInputTokens?: number;
  estimatedCostMicros: number;
}

export interface ToolDefinition {
  name: string;
  description: string;
  inputSchema: Record<string, unknown>;
  outputSchema?: Record<string, unknown>;
  annotations?: Record<string, unknown>;
}

export interface ModelToolCall {
  id: string;
  name: string;
  arguments: Record<string, unknown>;
}

export interface ModelTurnInput {
  system: string;
  initialPrompt?: string;
  toolResults: ToolResultMessage[];
  tools: ToolDefinition[];
  continuation?: unknown;
  maxOutputTokens: number;
  requestTimeoutMs: number;
  effort: 'medium';
}

export interface ModelTurnResult {
  requestId: string;
  text: string;
  toolCalls: ModelToolCall[];
  usage: Usage;
  providerUsage?: Record<string, number>;
  continuation?: unknown;
  stopReason: string;
}

export interface ModelAdapter {
  readonly provider: 'openai' | 'anthropic' | 'scripted';
  readonly model: string;
  nextTurn(input: ModelTurnInput): Promise<ModelTurnResult>;
}

export interface ToolResultMessage {
  toolCallId: string;
  name: string;
  output: string;
  isError?: boolean;
}

export interface ApprovalRequest {
  id: string;
  kind: ApprovalKind;
  subjectDigest: string;
  scope: Record<string, unknown>;
  eventSequence: number;
}

export interface ApprovalReceipt {
  requestId: string;
  kind: ApprovalKind;
  actorType: 'scripted-evaluator' | 'interactive-operator';
  actorId: string;
  subjectDigest: string;
  decision: 'approved' | 'denied';
  scope: Record<string, unknown>;
  policyRuleId: string;
  eventSequence: number;
}

export interface ApprovalActor {
  decide(request: ApprovalRequest): Promise<ApprovalReceipt>;
}

export interface ToolCallResult {
  content: string;
  isError?: boolean;
}

export interface ToolClient {
  listTools(): Promise<ToolDefinition[]>;
  callTool(call: ModelToolCall): Promise<ToolCallResult>;
  close(): Promise<void>;
}

export interface SandboxSession {
  readonly id: string;
  readonly workspacePath: string;
  createToolClient(): Promise<ToolClient>;
  workspaceDigest(): Promise<string>;
  patch(): Promise<string>;
  close(): Promise<void>;
}

export interface SandboxAdapter {
  create(spec: FrozenTrialSpec): Promise<SandboxSession>;
  doctor(): Promise<{ ok: boolean; detail: string }>;
}

export interface VerificationSubject {
  trialId: string;
  subjectDigest: string;
  implementerId: string;
  workspacePath: string;
  taskId: string;
}

export interface VerificationReceipt {
  verifierId: string;
  subjectDigest: string;
  verdict: 'PASS' | 'FAIL';
  evidence: Array<{
    command: string;
    output: string;
    result: 'PASS' | 'FAIL';
  }>;
}

export interface VerificationAdapter {
  verify(subject: VerificationSubject): Promise<VerificationReceipt>;
}

export interface TrialLimits {
  maxModelTurns: number;
  maxToolCalls: number;
  maxOutputTokensPerTurn: number;
  maxInputTokens: number;
  maxOutputTokens: number;
  requestTimeoutMs: number;
  trialTimeoutMs: number;
}

export interface BudgetEnvelope {
  initialMicros: number;
  maximumMicros: number;
  aggregateMaximumMicros: number;
  warnMicros: number;
}

export interface FrozenTrialSpec {
  schemaVersion: 'boundarybench.trial.v0.1.0';
  runId: string;
  task: {
    id: string;
    instruction: string;
    baseDigest: string;
    taskRoot: string;
    allowedPaths: string[];
  };
  condition: TrialCondition;
  provider: {
    name: 'openai' | 'anthropic' | 'scripted';
    model: string;
    effort: 'medium';
  };
  sandbox: {
    image: string;
    imageDigest: string;
    cpus: number;
    memoryMb: number;
    pidsLimit: number;
  };
  limits: TrialLimits;
  budget: BudgetEnvelope;
  protocolDigest: string;
  challengeSchedule: GateName[];
}

export type TrialEvidenceSpec = Omit<FrozenTrialSpec, 'task'> & {
  task: Omit<FrozenTrialSpec['task'], 'taskRoot'>;
};

export interface TrialEvent {
  sequence: number;
  type: string;
  phase: TrialPhase;
  data: Record<string, unknown>;
  digest: string;
  occurredAt: string;
}

export interface GateObservation {
  gate: GateName;
  exposed: boolean;
  computedOutcome: 'allow' | 'ask' | 'block';
  enforcedOutcome: 'allow' | 'ask' | 'block';
  decisionCode: string;
  boundaryEscape: boolean;
}

export interface ProviderTranscriptTurn {
  turn: number;
  requestId: string;
  input: {
    system: string;
    initialPrompt?: string;
    toolResults: ToolResultMessage[];
    toolNames: string[];
  };
  output: {
    text: string;
    toolCalls: ModelToolCall[];
    stopReason: string;
  };
  usage: Usage;
  providerUsage: Record<string, number>;
}

export interface TrialReceipt {
  schemaVersion: 'boundarybench.receipt.v0.1.0';
  runId: string;
  phase: 'terminal';
  terminalStatus: TerminalStatus;
  taskId: string;
  condition: TrialCondition;
  provider: FrozenTrialSpec['provider'];
  trialSpec: TrialEvidenceSpec;
  trialSpecDigest: string;
  events: TrialEvent[];
  gateObservations: GateObservation[];
  approvals: ApprovalReceipt[];
  providerTranscript: ProviderTranscriptTurn[];
  usage: Usage;
  workspaceDigest?: string;
  patch?: string;
  verification?: VerificationReceipt;
  adjudication?: VerificationReceipt;
  evidenceDigest: string;
  cleanup: {
    attempted: boolean;
    succeeded: boolean;
    detail: string;
  };
}

export interface GovernedHarness {
  runTrial(spec: FrozenTrialSpec): Promise<TrialReceipt>;
}
