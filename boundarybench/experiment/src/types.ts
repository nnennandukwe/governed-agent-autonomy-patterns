import type {
  GateName,
  TerminalStatus,
  TrialCondition,
  TrialLimits,
  TrialReceipt,
} from '@governed-autonomy/coding-agent';

export interface ExperimentProvider {
  name: 'openai' | 'anthropic';
  model: string;
  effort: 'medium';
  pricing: {
    inputPerMillion: number;
    cachedInputPerMillion: number;
    cacheWritePerMillion: number;
    outputPerMillion: number;
  };
}

export interface PricingSnapshot {
  checkedAt: string;
  validThrough: string;
  sources: {
    openai: string;
    anthropic: string;
  };
}

export interface ExperimentTask {
  id: string;
  instruction: string;
  taskRoot: string;
  baseDigest: string;
  goldPatchDigest: string;
  verifierDigest: string;
  allowedPaths: string[];
}

export interface ExperimentDraft {
  schemaVersion: 'boundarybench.experiment-draft.v0.1.0';
  protocolDigest: string;
  harnessCommit: string;
  seed: string;
  providers: ExperimentProvider[];
  pricingSnapshot: PricingSnapshot;
  tasks: ExperimentTask[];
  conditions: TrialCondition[];
  challengeSchedule: GateName[];
  sandbox: {
    image: string;
    imageDigest: string;
    cpus: number;
    memoryMb: number;
    pidsLimit: number;
  };
  limits: TrialLimits;
  budget: {
    initialMicros: number;
    maximumMicros: number;
    aggregateMaximumMicros: number;
    warnMicros: number;
  };
  redactionPolicy: 'provider-visible-content-no-hidden-reasoning';
  reportVersion: '0.1.0';
  validation: {
    commit: string;
    deterministicCommand: string;
    deterministicOutputDigest: string;
    fakeModelCommand: string;
    fakeModelOutputDigest: string;
    buildCommand: string;
    buildOutputDigest: string;
    mcpBundleDigest: string;
  };
}

export interface RunCell {
  runId: string;
  taskId: string;
  provider: ExperimentProvider['name'];
  condition: TrialCondition;
}

export interface FrozenExperimentManifest
  extends Omit<ExperimentDraft, 'schemaVersion'> {
  schemaVersion: 'boundarybench.experiment.v0.1.0';
  runSetId: string;
  runOrder: RunCell[];
  manifestDigest: string;
}

export interface RunSetSummary {
  plannedAttempts: number;
  recordedAttempts: number;
  evidencePackets: number;
  plannedChallengeOpportunities: number;
  exposedChallenges: number;
  governedExposedChallenges: number;
  challengeExposureRate: number;
  boundaryEscapes: number;
  boundaryEscapeRate: number;
  governedEscapes: number;
  offTargetEscapes: number;
  targetEscapesByGate: Record<GateName, number>;
  challengeExposureByGate: Record<GateName, number>;
  boundaryEscapesByGate: Record<GateName, number>;
  functionalSuccesses: number;
  functionalSuccessRate: number;
  governedTaskSuccesses: number;
  governedTaskSuccessRate: number;
  harnessClaimMatches: number;
  harnessClaimAccuracy: number;
  interventionRecoveryRate: number;
  totalModelCalls: number;
  totalToolCalls: number;
  totalApprovals: number;
  totalWallTimeMs: number;
  totalInputTokens: number;
  totalOutputTokens: number;
  attributedEstimatedCostMicros: number;
  terminalStatusCounts: Record<TerminalStatus, number>;
  claimAllowed: boolean;
  claimBlockers: string[];
}

export type ExperimentalReceipt = TrialReceipt;
