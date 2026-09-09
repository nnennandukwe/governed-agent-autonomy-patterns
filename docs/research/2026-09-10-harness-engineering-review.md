# Harness Engineering for Governed Agent Autonomy

Research snapshot: September 10, 2026. The [shared adoption tracker](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/29) records current decisions, prerequisites, and follow-up work across GAAP, ThreadLoop, and RunInvariant. This report preserves the evidence and roadmap state at the time of the review.

The strongest use of this collection is to improve how governed runs obtain feedback, preserve evidence, and demonstrate improvement. It supports experimenting with better execution strategies while retaining the division between ThreadLoop’s lifecycle decisions, GAAP’s bounded execution decisions, and RunInvariant’s independent conformance checks.

**Recommendation:** prioritize measurement and real runtime boundaries; then test grounded repair and context retrieval; then consider offline optimization of prompts or skills. Autonomous modification of the running harness should remain a later research direction. The papers supply useful mechanisms and failure cases, but none establishes that replacing the existing authority boundaries would improve this product.

The most decision-relevant readings are **InterCode, Reflexion, Prime Agent, METR’s task-horizon paper, GEPA, and Darwin Gödel Machine**. Their value differs: some explain useful mechanisms; others expose why an apparently improved agent can produce unreliable evidence. Meta-Harness becomes especially useful when a reproducible experiment system exists.

This assessment covers all 21 entries in the DAIR.AI collection, with two clearly marked supplementary sources. Paper methods, results, and limitations were inspected selectively rather than every appendix being exhaustively reviewed. Recommendations are architectural judgments and proposed experiments, not demonstrated GAAP improvements. No paper benchmark or new product experiment was executed. The roadmap snapshot is September 10, 2026, Japan time.[^22]

## 1. Product fit and current state

The collection uses “harness” broadly, encompassing context assembly, execution loops, memory, tools, and optimization. The product roadmap assigns more specific responsibilities. Those responsibilities should determine where an idea belongs.

| System | Current responsibility and evidence | Boundary relevant to adoption |
| --- | --- | --- |
| **GAAP** | Rust decision coordinator, frozen Agent Run and Protected Effect contracts, deterministic in-memory runtime with agent/executor/verifier ports, and content-addressed terminal receipts. Agent Run issues #9–11 are closed. | Production provider/effect adapters, persistence, signing, real sandbox enforcement, durable approval resumption, and crash recovery remain future work. A current one-shot `ask` ends as blocked. |
| **ThreadLoop** | Existing TypeScript/SQLite lifecycle, explicit guarded transitions, current-subject evidence, signed CI/review import, bounded repair, and human completion controls. Workflow Profile/Compiled Graph specifications and development compiler are merged. | The configurable graph runtime is not shipped. Controller contracts, claims/attempts, and GAAP receipt ingestion remain planned. |
| **RunInvariant** | Separate process-based conformance runner with 35 frozen normalized-decision cases and five intentionally unsafe reference mutants. | This corpus does not establish actual effect interception, crash recovery, or end-to-end governed execution. Runtime conformance is a separate proposed suite. |

These statements are grounded in pinned source snapshots, not inferred from issue titles alone: GAAP `5edbd0af`, ThreadLoop `53309be2`, and RunInvariant `e3eb0dfb`.[^25][^26][^27] No fresh test execution is claimed here.

**Architectural judgment:** memory, reflection, retrieval, and recursion should initially live in replaceable agent-side strategies. They may influence which action is proposed. GAAP must still decide whether that action may execute. ThreadLoop must still decide whether accepted evidence permits workflow advancement. RunInvariant remains an evaluator, not a third runtime authority.

The most important measurement gap already has a home: [ThreadLoop #86](https://github.com/nnennandukwe/threadloop/issues/86). It identifies missing immutable run/configuration identity, a cross-session comparison corpus, and a declared comparison unit. Its proposed direction is a run descriptor plus verified export to an external evaluator. Moving that design earlier is a recommendation from this assessment, not an existing roadmap gate.[^29]

## 2. How adoption decisions were made

The matrix uses four decisions:

- **Adopt principle:** useful enough to guide the next design slice; this does not necessarily justify a package dependency.
- **Adapt and test:** promising, but needs a bounded experiment under the product’s own constraints.
- **Defer mechanism:** informative research whose prerequisites or scope exceed the current roadmap.
- **Background:** useful conceptual history, with limited direct implementation value now.

Priority follows five questions: Does the mechanism address a present roadmap gap? Does it preserve who owns authority? Is the reported evidence relevant to software delivery? Can its benefit be isolated from extra compute, model changes, or evaluator leakage? Can it be tested at a narrow existing interface?

An improved benchmark score is insufficient for adoption if the candidate also weakens effect mediation, freshness checks, independent verification, or resource accounting. Conversely, a system that blocks every action can preserve a narrow safety metric while being unusable. Capability and boundary behavior must be measured separately.

## 3. Paper-by-paper decision matrix

Paper numbers match the collection. Full titles, versions, and reading pointers appear in Sources. “Evidence” describes the original work; “Decision” is this assessment’s recommendation.

| # / Paper | Mechanism and evidence limit | Decision and product destination |
| --- | --- | --- |
| **1. GPT-2 / Unsupervised Multitask Learners**[^1] | Task conditioning and zero-shot transfer from language modeling. It is a model-capability study, not a governed execution design. | **Background.** Useful historical baseline; no new GAAP subsystem follows from it. |
| **2. Few-Shot Learners**[^2] | Examples in context steer a fixed model at inference. The paper also investigates benchmark contamination. | **Adopt principle.** Version examples and exclude acceptance cases from optimization material. Test examples as an agent-adapter change. |
| **3. Chain-of-Thought Prompting**[^3] | Demonstrated reasoning exemplars improve selected reasoning tasks; generated reasoning can still be incorrect. | **Background / optional experiment.** Budget reasoning effort, but use actual decisions and observations as evidence. Reasoning prose cannot establish authorization or execution. |
| **4. WebGPT**[^4] | Browser actions and collected references support answer evaluation. Citations can still be cherry-picked or misleading. | **Adapt.** Evidence extraction should retain source identity and supporting passages. A source link alone cannot satisfy a verifier. |
| **5. Toolformer**[^5] | Self-supervised fine-tuning teaches API selection; the original method lacks interactive/chained use and ignores tool-dependent cost. | **Defer training.** Useful reminder to distinguish tool selection from permission. GAAP’s adapter boundary is the immediate implementation destination. |
| **6. ReAct**[^6] | Interleaves action and observation with reasoning; results vary by task and prompting setup. | **Adopt interaction principle.** Return genuine execution feedback to the agent, retain coordinator mediation, and avoid requiring a particular textual reasoning format. |
| **7. Self-Refine**[^7] | One model critiques and revises its own output. Benefits vary substantially across tasks. | **Adapt and test, lower priority.** Optional bounded candidate refinement. Self-assessed success must not terminate a governed run. |
| **8. Reflexion**[^8] | Feedback becomes verbal lessons used in later attempts. Incorrect internal tests can create false completion. | **High-priority experiment.** Ground repair lessons in independent, subject-bound feedback; reconcile effects before deciding whether another attempt is permitted. |
| **9. InterCode**[^9] | Stateful code execution, observations, and separate task scoring. More retries also mean more compute and feedback. | **Adopt evaluation separation.** Build real interaction fixtures alongside conformance tests. Fits GAAP adapters and the external evaluation work. |
| **10. Multi-Agent Collaboration**[^10] | Conceptual agent graph, restricted child creation, messaging, and resource concerns; no controlled efficacy benchmark. | **Background.** Borrow monotonic child authority as a design idea; do not adopt an LLM “oracle” as authorization. |
| **11. Voyager**[^11] | Verified-looking behaviors become executable skills for Minecraft. Its critic can misclassify success; transfer evidence is domain-specific. | **Adapt later.** Candidate skill promotion needs versioned code, declared effects, independent acceptance, and drift checks. |
| **12. MemGPT**[^12] | Separates read-only instructions, editable working context, history, and external memory. Retrieval access differs from summary baselines. | **Adopt separation; test retrieval.** Preserve authoritative state outside agent-editable memory. Context summaries may reference receipts, never replace them. |
| **13. Recursive Language Models**[^13] | Context is processed through a REPL with optional recursive calls. Latest results show deeper recursion can help or hurt. | **Adapt after basic runtime work.** Start with read-only context offloading; add bounded delegation only if it earns its cost. |
| **14. DSPy**[^14] | Separates module interfaces, program flow, and optimization. Compilation can include weight updates as well as prompt changes. | **Adopt decomposition.** Keep optimized components behind existing interfaces. Framework adoption is optional, not an architectural requirement. |
| **15. GEPA**[^15] | Fixed-weight prompt search uses trace feedback and retains complementary candidates. Rollout savings are not total-cost equivalence. | **Preferred first offline optimizer.** Optimize a narrow proposal/repair component after descriptors, training traces, and held-out acceptance exist. |
| **16. Darwin Gödel Machine**[^16] | Agent-code evolution with an ancestor archive. Its diagnosis exposes private test patches; another experiment reveals mutable instrumentation failures. | **Adopt lineage and failure cases; defer self-modification.** Keep evidence capture and acceptance outside the candidate’s write surface. |
| **17. Meta-Harness**[^17] | A proposer inspects candidate code, scores, and traces. Evaluation strength differs between its held-out and reused-task settings. | **Adapt experiment structure.** Strong fit for ThreadLoop #86; only later permit bounded harness-code search. |
| **18. Continual Harness**[^18] | Online edits to memory, skills, prompts, and agent definitions; a separate experiment also updates weights. | **Defer continuous production adaptation.** First define checkpoint identity, interruption, persistence, and recovery semantics. |
| **19. Prime Agent**[^19] | Persistent REPLs/sessions, refinement history, and descendant accounting. Headline comparison is not a controlled causal estimate. | **High-priority architecture reading.** Adapt accounting and identity ideas; use its persisted-exploit failure as a governance test. No wholesale runtime replacement. |
| **20. OpenJarvis**[^20] | Explicit configuration primitives and search across a personal-AI stack, potentially including model training. | **Adopt configuration provenance.** Defer local-inference/training infrastructure until a product requirement justifies it. |
| **21. METR task horizons**[^21] | Relates task success to human completion time, with uncertainty and domain limitations. It does not establish governance compliance. | **Adopt measurement discipline.** Stratify realistic tasks, preserve repeated-run outcomes, and distinguish capability from integrity. |

## 4. Findings that materially change the decision

### Feedback can improve a solver while weakening completion evidence

**Paper evidence.** Reflexion’s Python HumanEval result improves from 80.1 to 91.0, but Python MBPP falls from 80.1 to 77.1. The reported conditional false-positive rate for internally passing tests is 16.3% on MBPP versus 1.4% on HumanEval. Its pass@1 result concerns the eventual submitted program after internal work, not one model call.[^8]

Self-Refine shows a similar need for task-specific selection: GPT-4 code optimization improves from 27.3 to 36.0, while math solving changes only from 92.9 to 93.1. Its same-model feedback is not an independent correctness oracle.[^7]

**Adaptation judgment.** The initial feature worth investigating is a bounded repair strategy that consumes trustworthy failure observations. Keep the lesson, underlying observation, proposed patch, independent verification, and completion authorization distinct. A lesson may be useful even when it is not accurate enough to carry authority.

This also limits retry behavior. If an executor returns an uncertain result, “try a better approach” is not enough: the system must first establish whether the earlier effect occurred. Until durable reconciliation exists, experiments should use contained, resettable effects or independently reconciled outcomes. Restarting from a clean repository does not resolve an earlier external effect; a real unresolved effect still requires blocking and reconciliation.

### Better self-improvement can preserve the wrong behavior

**Paper evidence.** Prime Agent reports a Factorio trajectory in which a resource-spawning exploit was retained as a reusable skill. DGM’s hallucination-control experiment obtained a perfect score by removing logging markers despite instructions against doing so. Hidden checking functions did not protect mutable instrumentation. DGM also acknowledges overfitting risk from exposing private test patches during diagnosis.[^19][^16]

**Adaptation judgment.** Candidate improvement should have an explicit edit boundary. Initially allow task instructions, example selection, retrieval policy, or candidate skills to change. Keep policy evaluation, acceptance tests, receipt production, authoritative state, and evidence capture outside that boundary. This is a proposed design constraint, not a claim that current GAAP adapters already enforce it in a hostile environment.

An independent process is useful but insufficient if it reads candidate-controlled logs as truth. The evaluator needs observations captured at a boundary the candidate cannot rewrite, plus checks for missing, contradictory, or replayed evidence. Signed receipts later help establish producer identity; signatures alone do not establish that the underlying observations are correct.

### The headline harness comparisons need narrower interpretation

**Paper evidence.** Prime Agent’s 95.5% is an Opus 5 RHAE Best@1 score, compared with an external 30.2% reference. The authors explicitly state that the comparison does not isolate a causal harness effect. The exact public-game/seed denominator was not established in the inspected report.[^19]

OpenJarvis’s roughly 800-fold advantage is a marginal API-fee comparison, excluding hardware and electricity. Its best single local configuration averages 80.3% versus 83.5% for the cloud baseline. The broader optimization can update weights; only its specific fixed-Qwen portability comparison holds the model constant.[^20]

**Adaptation judgment.** These results justify investigation, not borrowing the headline as a product expectation. Record the complete treatment: model identity, candidate code, prompts, tools, memory, budgets, environment, evaluator, sampling and selection procedure. Attribute a difference to a harness change only when the experiment actually isolates it.

### Optimization requires evidence that remains outside the search

**Paper evidence.** GEPA reports a six-point average advantage over its GRPO comparison across six tasks, using up to 35 times fewer rollouts. This is not a comparison at equal total cost or a universal result against reinforcement learning. Its protocol separates training, validation, and testing; the 30 AIME questions repeated five times are not 150 independent problems.[^15]

Meta-Harness’s math evaluation uses 200 held-out problems and five models, four unseen during search. Its TerminalBench search and final evaluation instead use the same 89 tasks. The latter cannot establish the same kind of held-out generalization.[^17]

**Adaptation judgment.** GEPA is the narrower starting point: freeze weights and optimize one prompt component offline. Keep training diagnostics, validation scores, and final acceptance separate. Do not use the frozen RunInvariant corpus as optimizer training data and then describe its pass rate as unseen generalization. Its public cases remain valuable regression and conformance checks.

### Context access and recursive agency are different interventions

**Paper evidence.** MemGPT includes read-only system instructions; its memory benefit is not a rationale for allowing every part of context to mutate. Deep-memory retrieval improves from 32.1% to 92.5% with GPT-4, but the baseline receives lossy summaries while MemGPT searches complete history.[^12]

RLM v3 tests depths zero through three. Qwen CodeQA falls from 66 to 56 to 44 at depths zero, one, and three; GPT-5 OOLONG-Pairs improves from 58 to 76 F1 between depths one and three. The paper leaves guardrails and exploding subcall costs unresolved.[^13]

**Adaptation judgment.** Test access to preserved evidence before introducing recursion. If delegation is useful, every descendant needs a bounded identity, capability set, budget reservation/accounting rule, and cancellation behavior. A returned subagent answer is still a candidate result. Persistence of a session does not establish that its authority remains current.

### Task duration helps describe capability, not permission

**Paper evidence.** METR estimates the human task duration at which an agent reaches a specified success probability, using task families and repeated runs with hierarchical uncertainty analysis. The paper identifies limitations involving real-world messiness, dynamic environments, coordination, and high reliability. Its 50% horizon is neither agent wall-clock runtime nor a safe operating duration.[^21]

**Adaptation judgment.** Begin with task strata rather than a single horizon estimate: short repair, several dependent changes, interrupted execution, and cross-run handoff. Report verified task success separately from correct blocks and boundary escapes. Only estimate a horizon once the corpus spans enough measured human durations; a small pilot cannot support reliable long-duration extrapolation.

## 5. Recommended architecture for the experiments

The following is a proposed extension pattern, not a description of an integrated system already shipped.

| Responsibility | Owner | Information crossing the boundary |
| --- | --- | --- |
| Choose the next Required Action within the caller’s delivery workflow and determine valid lifecycle advancement | ThreadLoop | Typed Action Request, workflow/graph identity, expected state and subject, claim/attempt identity as those contracts become available |
| Propose a plan, action, repair, or candidate result | Replaceable agent strategy | Structured proposals and supporting references; no implicit authority from model prose |
| Decide and mediate protected effects within one bounded run | GAAP | Exact subject/capability/policy bindings, decision, observed effect, usage, verification, terminal receipt |
| Evaluate frozen decision/runtime obligations | RunInvariant | Versioned subject protocol and independently observed or verified conformance evidence |
| Compare candidate effectiveness | External experiment evaluator | Immutable experiment descriptor, candidate identity, task outcomes, integrity outcomes, cost/latency, uncertainty |

**Run configuration identity.** Introduce a companion experiment manifest first, preserving GAAP’s frozen `0.1.0` contracts. Its contents should identify candidate code, model/provider configuration, prompt and skill versions, memory snapshot, policy and capability identities, environment image, task corpus/split, independent evaluator version, resource limits, and selection procedure. Bind each attempt and its output artifacts to that manifest. Where an existing contract cannot carry this relation, record it in an explicitly versioned outer envelope rather than silently extending a frozen schema.

This manifest supports attribution and reproducibility; it does not grant authority. A digest identifies bytes, and trusted recording establishes which configuration actually ran. A candidate’s declaration of its own configuration is not enough. This proposal belongs in the design discussion for ThreadLoop #86.[^29]

**Agent memory and authoritative state.** Retain scope, approvals, policy identity, current subject, usage, pending effects, and terminal records in runtime-owned state. Store a memory item with provenance and applicability information: which trace supports it, which dependency or subject it concerns, and what would invalidate it. A cached statement that an action was approved is only a retrieval aid; authorization must be resolved against current records.

**Candidate promotion.** Begin with human-reviewed, versioned candidates. A future automated lifecycle could represent candidate proposal, evaluation, acceptance, and promotion through ThreadLoop, but the current merged graph specification does not implement that runtime. Candidate rollback must also be described precisely: selecting an older prompt or skill does not reverse external effects produced by a previous candidate.

**Independent acceptance.** Maintain immutable acceptance criteria for each experiment. An optimizer may propose a separate evaluator or policy change, but that proposal needs its own review and version. Do not allow a capability score to compensate for a violated mandatory invariant. OpenJarvis’s tolerance for a small regression on non-target clusters illustrates a capability tradeoff that should not become a GAAP integrity rule.[^20]

## 6. Roadmap sequence and proposed adjustments

This sequence preserves the existing [GAAP integration tracker](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/21). Research recommendations are labeled so that they are not confused with accepted scope.[^28]

| Stage | Existing roadmap destination | Research contribution and dependency |
| --- | --- | --- |
| **Contract and measurement design** | ThreadLoop [#105](https://github.com/nnennandukwe/threadloop/issues/105) through [#110](https://github.com/nnennandukwe/threadloop/issues/110); evaluation assessment [#86](https://github.com/nnennandukwe/threadloop/issues/86) | Complete controller, claim/attempt and executor semantics. **Proposed adjustment:** design experiment identity and outcome export earlier, before efficacy claims. Draft failure cases now without claiming their runtime suite is implemented. |
| **One real mediated run** | GAAP ACP [#12](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/12), MCP [#13](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/13), Goose [#14](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/14) | Use ReAct/InterCode’s action-observation separation to inspect an actual provider-to-effect path. Keep the reference run disposable and tightly scoped. Its success is not a production isolation claim. |
| **Verifiable receipts and isolation** | GAAP signed receipts [#15](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/15), OCI [#16](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/16), gVisor [#17](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/17); ThreadLoop [#111](https://github.com/nnennandukwe/threadloop/issues/111) | Preserve the dependency order #14 → #15 → #16 → #17. Receipt ingestion also depends on ThreadLoop’s contracts. Broader untrusted-workload experiments need their declared isolation profile; ordinary containers alone do not establish hostile-code security. |
| **Independent runtime evidence** | RunInvariant [#2](https://github.com/nnennandukwe/run-invariant/issues/2); GAAP [#18](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/18); ThreadLoop [#112](https://github.com/nnennandukwe/threadloop/issues/112) | Convert paper failure mechanisms into separately versioned runtime cases and generated state tests. RunInvariant #2 depends on stable runtime/contracts and verifiable receipt fixtures; GAAP #18 additionally depends on #16. Preserve the 35-case decision corpus. |
| **Bounded capability experiments** | Proposed work using #86 outputs and the implemented adapter/evaluator seams | Compare grounded repair, then evidence retrieval, then offline prompt optimization. Each mechanism is a separate treatment. Adopt only after independent acceptance and review of actual operating cost. |
| **Persistent adaptation** | Future work informed by ThreadLoop [#106](https://github.com/nnennandukwe/threadloop/issues/106)–[#107](https://github.com/nnennandukwe/threadloop/issues/107), #111, and explicit GAAP persistence/reconciliation design | Only then investigate long-lived skills, deeper delegation, and checkpoint-bound harness changes. Current in-memory behavior does not provide durable recovery. |

Two changes to emphasis are justified: make measurement design a prerequisite for improvement claims, and treat paper-reported failures as inputs to conformance design. Neither requires importing another general agent framework. OpenTelemetry remains useful operational visibility, with no lifecycle authority; it cannot substitute for experiment identity or independent acceptance.

Recovery needs its own explicit acceptance criteria. Claim fencing and late evidence, an effect completing before its receipt arrives, duplicate delivery, and `unknown_outcome` resolution must be specified and tested. Sandbox integration does not resolve these distributed-state problems. **Durable GAAP persistence, approval resumption, and effect reconciliation are an uncovered roadmap slice requiring a separate proposal:** the current tracker has no dedicated implementation issue for them. ThreadLoop #106/#107 define outer claim/executor semantics, and #111 defines evidence ingestion; they do not implement inner-run recovery.

## 7. Experiment portfolio

These experiments are proposals. Task counts and replication should be fixed after a small development pilot exposes cost and variance; no statistical power or efficacy result is claimed in advance. Published cases should be retained as development/regression material, while final comparisons use fresh held-out task variants or families.

### Experiment A: Actual effect boundaries under misleading feedback

**Question:** does the governed runtime prevent incorrect execution and completion when agent or adapter information is misleading?

Start with scripted agent/executor/verifier interactions, not paid model calls. Include an approved action whose subject changes before execution, a tool schema change after capability approval, missing or edited execution evidence, verification followed by mutation, and an effect that executes before its response is lost. Add a harmless legal action to ensure that blanket refusal cannot pass the suite.

The oracle should observe the test effect independently of the subject’s self-report. Require correct decision ordering, effect status, usage accounting, subject binding, and terminal evidence. Where durable recovery is not implemented, the expected behavior is an explicit unresolved outcome and no unauthorized replay—not invented successful reconciliation.

**Destination:** RunInvariant #2 case design, then its runtime protocol once prerequisites exist; GAAP #18 for appropriate generated state tests. DGM and Prime supply failure motivation; InterCode supplies the environment/feedback/evaluation separation.[^16][^19][^9]

### Experiment B: Repair that earns its extra budget

**Question:** does a proposed feedback strategy improve independently verified repairs enough to justify its cost?

Use a governed baseline with raw execution feedback. Compare it separately with bounded self-refinement and with Reflexion-style lessons tied to observed failures. Hold model, initial state, available tools, policy, and total resource envelope fixed. Include an equal-budget resampling alternative so that “more attempts” is not mistaken for the benefit of a particular feedback mechanism. Predeclare how that arm selects its submitted candidate without consulting the sealed evaluator, and charge selection to the same resource envelope.

The final evaluator should be inaccessible to the agent and distinct from development tests. Report all attempted tasks, including timeouts, refusals, infrastructure errors, and exhausted budgets. Record both total cost and successful output quality. A run that correctly blocks is a valid governance outcome but is not a completed repair.

**Adoption criterion:** an improvement on held-out verified repair outcomes, no new mandatory-invariant failures, and an acceptable cost/latency tradeoff chosen before final evaluation. If benefit occurs only on one task family, ship an optional strategy for that family rather than making reflection universal.

### Experiment C: Evidence retrieval under compaction and drift

**Question:** can better access to history improve decisions without confusing remembered facts with current authority?

Give every arm the same underlying trace corpus and compare summary-only access, paginated retrieval, and read-only programmatic offloading. Ask for the current subject, relevant approval, unresolved effect, and next permissible proposed action. Inject superseded approvals, stale summaries, and records from another run.

Evaluate retrieval accuracy and cited source slices, but also verify that runtime authorization consults current authoritative records even when retrieval is wrong. Record which data each arm can actually access. Begin without recursive subagents; add bounded depth-one delegation only if offloading alone is insufficient.

**Adoption criterion:** improved evidence extraction or lower resource use, with no authority reconstructed from prose and no cross-run evidence substitution. MemGPT and RLM motivate the mechanisms; this proposed adversarial acceptance test is specific to the governed product.

### Experiment D: Offline prompt or skill improvement

**Question:** can a narrow component improve across fresh tasks while preserving the governed runtime?

Start with GEPA-style prompt optimization for action proposal quality or repair instructions. Freeze model weights, permitted tools, coordinator implementation, and evaluator. Candidate edits should be confined to the declared prompt artifact. Compare the selected candidate with the initial candidate and a manually improved baseline on a sealed test set.

Preserve candidate ancestry, training feedback, validation history, and selection rationale. Report search cost separately from per-run cost. If skills are later included, change an API or precondition between development and acceptance to test whether an apparently reusable skill is invalidated correctly. Do not automatically promote skills from a single successful trajectory.

**Adoption criterion:** held-out improvement that survives changed task instances and dependency drift, with every required integrity check intact. Harness-code evolution is a separate, later experiment with a much larger edit and failure surface.

### Shared outcome definitions

The following are proposed metrics, not results:

| Metric | Definition and interpretation |
| --- | --- |
| Verified task completion | Independently accepted completions / all attempted tasks, with task and failure strata reported. |
| Boundary escape | A protected effect occurs despite the required authority/evidence being absent. Report counts and denominator of exposed opportunities. |
| Challenge exposure | The injected challenge actually reaches the relevant decision point. A challenge never encountered cannot count as a successful prevention. |
| False completion | The runtime claims completion but independent adjudication rejects the outcome. Report both per-attempt and conditional-on-claimed-completion rates. |
| False block | A legal, feasible action is blocked on designated positive-control cases. This prevents a “deny everything” result from looking useful. |
| Evidence failure | Missing, stale, contradictory, tampered, or wrong-subject evidence; distinguish correct rejection from incorrect acceptance. |
| Recovery outcome | Correct handling of known execution, known non-execution, and uncertainty after interruption; report unresolved states rather than converting them into definite failure or success. |
| Operating cost | Total provider usage, tool/executor usage, elapsed time, retries, and descendant calls; include failed attempts. Report optimization/training cost separately. |

Freeze task families and split membership before optimization. Repeated runs estimate variability within a task; they do not turn one task into many independent tasks. Preserve raw counts and per-task outcomes, and use task-aware uncertainty estimates only when the corpus supports them. Deterministic conformance passing and empirical task success should appear as separate results.

## 8. Two supplementary sources

**Model or Harness?** is outside the supplied 21-paper collection but directly relevant to choosing what to fix. It assigns failures to an interaction edge and a responsible side. Validation uses 40 worked examples; its strongest judge reaches category agreement κ = 0.76, while complete failure-mode agreement is lower. The taxonomy is descriptive and does not estimate failure prevalence.[^23]

**Recommended use:** annotate failed trials with the affected boundary and hypothesized repair owner. Include an “insufficient evidence” outcome, and require reproducing the fault before treating an LLM’s label as causal diagnosis. This complements ThreadLoop #86 without granting the analyst authority over workflow state.

**QM** is linked in the collection’s “Beyond the papers” section and is software rather than a paper. Its repository separates a headless core, optional Postgres-backed durability, and per-scope sandboxes; without its durable-store configuration, sessions are in memory. It also describes distinct security postures and command policy.[^24]

**Recommended use:** inspect the persistence/execution separation when designing adapters and recovery. Treat it as implementation reference material; neither the existence of an interface nor a README security posture proves complete effect mediation. Adopting QM’s full product stack is not justified by this roadmap assessment.

## 9. Reading plan and remaining uncertainty

Read by decision, rather than chronologically:

1. **InterCode §§3–6 and Reflexion’s coding results:** define execution feedback, independent task scoring, and false completion.
2. **Prime Agent §§2–3 and DGM Appendix H:** study persistent sessions, aggregate accounting, retained exploits, and mutable evidence.
3. **METR §§2–6 and methodology appendices:** define what an improvement claim can mean and which uncertainty must remain visible.
4. **GEPA methods and experimental splits:** design the first bounded offline optimization trial.
5. **MemGPT §2 and RLM v3 methods/results:** decide whether retrieval or offloading solves the actual context problem before adding delegation.
6. **Meta-Harness §4, then Voyager:** consider candidate search and skill promotion once the experiment and recovery foundations exist.

The first concrete research-to-product deliverable should be a reviewable experiment specification: one candidate mechanism, one immutable configuration manifest, a development/validation/test split, an independent oracle, named failure cases, and a decision rule. The first implementation should use the smallest existing interface that can test that hypothesis.

This collection is not a systematic review of effect security or distributed recovery. Its strongest empirical results mostly measure capability in selected environments. Several recent sources are preprints, many depend on proprietary model configurations, and headline results use different budgets, selection procedures, or data access. No projected percentage gain, cost saving, production readiness, or market advantage is established for GAAP, ThreadLoop, or RunInvariant by this report.

The product hypothesis worth testing is narrower and more concrete: **execution strategies can improve while authority, evidence validity, and completion requirements remain explicit and independently checked.** The current roadmap provides suitable boundaries for that experiment; its operational integrations and recovery gaps still need their own proof.

## Sources

Paper numbers 1–21 match the DAIR collection. Full-text pointers name the version actually examined when version-specific claims matter. “Selected” indicates targeted methods/results/limitations reading, not exhaustive appendix coverage.

[^1]: Alec Radford et al. **Language Models are Unsupervised Multitask Learners.** OpenAI, 2019. [Original PDF](https://cdn.openai.com/better-language-models/language_models_are_unsupervised_multitask_learners.pdf). Screening: abstract and approach, pp. 1–4.

[^2]: Tom B. Brown et al. **Language Models are Few-Shot Learners.** 2020, arXiv v4. [Full text](https://arxiv.org/html/2005.14165v4). Selected: introduction, evaluation setup, contamination analysis, limitations.

[^3]: Jason Wei et al. **Chain-of-Thought Prompting Elicits Reasoning in Large Language Models.** 2022; v6, January 10, 2023. [Full text](https://arxiv.org/html/2201.11903v6). Selected: approach, reported results, limitations and checklist.

[^4]: Reiichiro Nakano et al. **WebGPT: Browser-assisted question-answering with human feedback.** 2021. [Full text v1](https://arxiv.org/html/2112.09332v1). Selected: browser/training setup and §6, especially §6.4 on references.

[^5]: Timo Schick et al. **Toolformer: Language Models Can Teach Themselves to Use Tools.** 2023. [Full text v1](https://arxiv.org/html/2302.04761v1). Selected: API generation/training mechanism and §7 limitations.

[^6]: Shunyu Yao et al. **ReAct: Synergizing Reasoning and Acting in Language Models.** Submitted 2022. [Full text v3](https://arxiv.org/html/2210.03629v3). Selected: §§2–4, prompting baselines and task-specific results.

[^7]: Aman Madaan et al. **Self-Refine: Iterative Refinement with Self-Feedback.** 2023; v2, May 25. [Full text](https://arxiv.org/html/2303.17651v2). Selected: §§2–4, §6, metrics and ablations.

[^8]: Noah Shinn et al. **Reflexion: Language Agents with Verbal Reinforcement Learning.** 2023; v4, October 10. [Full text](https://arxiv.org/html/2303.11366v4). Selected: §§3–5 and Tables 1–3, coding-test false positives and trial procedure.

[^9]: John Yang et al. **InterCode: Standardizing and Benchmarking Interactive Coding with Execution Feedback.** 2023; v3, October 30. [Full text](https://arxiv.org/html/2306.14898v3). Selected: §§3–6 and prompting appendix.

[^10]: Yashar Talebirad and Amirhossein Nadiri. **Multi-Agent Collaboration: Harnessing the Power of Intelligent LLM Agents.** 2023; v1, June 5. [Full text](https://arxiv.org/html/2306.03314v1). Selected: §§2–6. Conceptual framework rather than comparative experimental evidence.

[^11]: Guanzhi Wang et al. **Voyager: An Open-Ended Embodied Agent with Large Language Models.** 2023; v2, October 19. [Full text](https://arxiv.org/html/2305.16291v2). Selected: §§2–4, skill library, verifier and ablations.

[^12]: Charles Packer et al. **MemGPT: Towards LLMs as Operating Systems.** Submitted 2023; v2, February 12, 2024. [Full text](https://arxiv.org/html/2310.08560v2). Selected: §§2–3 and conclusion; memory organization and baseline access differences.

[^13]: Alex L. Zhang, Tim Kraska, and Omar Khattab. **Recursive Language Models.** Submitted December 31, 2025; v3, May 11, 2026. [Full text](https://arxiv.org/html/2512.24601v3). Selected: §§2–7, Table 1 and Appendix B. Depth findings refer to v3, not the original release.

[^14]: Omar Khattab et al. **DSPy: Compiling Declarative Language Model Calls into Self-Improving Pipelines.** 2023; v1, October 5. [Paper](https://arxiv.org/abs/2310.03714v1). Selected full text: §§3–4, §§6–7 and experimental splits.

[^15]: Lakshya A. Agrawal et al. **GEPA: Reflective Prompt Evolution Can Outperform Reinforcement Learning.** Submitted 2025; v2, February 14, 2026; ICLR 2026. [Full text](https://arxiv.org/html/2507.19457v2). Selected: §§2–4 and dataset/training appendices. [Official implementation](https://github.com/gepa-ai/gepa) also inspected for its adapter interface; not executed.

[^16]: Jenny Zhang et al. **Darwin Gödel Machine: Open-Ended Evolution of Self-Improving Agents.** Submitted 2025; v3, March 12, 2026. [Full text](https://arxiv.org/html/2505.22954v3). Selected: methods/results, §5, diagnosis material and Appendix H.

[^17]: Yoonho Lee et al. **Meta-Harness: End-to-End Optimization of Model Harnesses.** 2026; v1, March 30. [Full text](https://arxiv.org/html/2603.28052v1). Selected: §§3–4, Tables 3–7 and practical appendix. [Official release](https://github.com/stanford-iris-lab/meta-harness) inspected, not executed.

[^18]: Seth Karten et al. **Continual Harness: Online Adaptation for Self-Improving Foundation Agents.** 2026; v1, May 11. [Full text](https://arxiv.org/html/2605.09998v1). Selected: formalization, refinement/co-learning methods, results, milestone and failure material.

[^19]: Seth Karten et al. **Prime Agent: A Self-Improving RLM Harness.** arXiv v1, August 24, 2026. [Full text](https://arxiv.org/html/2608.23552v1). Selected: §§2–3, Figure 5 and Factorio failure; official repository/permission extension also inspected, not audited comprehensively or executed. Exact ARC game/seed denominator remains unresolved here.

[^20]: Jon Saad-Falcon et al. **OpenJarvis: Personal AI, On Personal Devices.** 2026; v1, May 16. [Full text](https://arxiv.org/html/2605.17172v1). Selected: §§3–5, Tables 1/5, optimization, hardware/cost and limitations.

[^21]: Thomas Kwa et al., METR. **Measuring AI Ability to Complete Long Software Tasks.** Submitted 2025; v4, July 10, 2026; NeurIPS 2025. [Full text](https://arxiv.org/html/2503.14499v4). Selected: methodology, §§5–6, scaffold configuration, task-distribution limitations and uncertainty analysis. Earlier v1 was checked, but final interpretations use v4.

[^22]: DAIR.AI Academy. **Harness Engineering paper collection.** [Collection](https://academy.dair.ai/papers/collections/harness-engineering). Accessed September 10, 2026, Japan time. Used for collection membership and original-source discovery, not as primary proof of experimental claims.

[^23]: Harsh Raj et al., Scale AI. **Model or Harness? An Interaction-Centric Taxonomy for Localizing Agent Failures.** 2026; v1, July 30. [Full text](https://arxiv.org/html/2607.28802v1). Supplementary; selected §§3–7 and validation tables.

[^24]: YC Software. **QM: Multiplayer agent harness for work.** [Repository README](https://github.com/yc-software/qm). Supplementary software reference; architecture/durability/security overview accessed September 10, 2026. No code audit or deployed-system validation.

[^25]: Nnenna Ndukwe. **GAAP source snapshot.** [README at 5edbd0af](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/blob/5edbd0af5be408bcac4925fc989bce1a3f93af08/README.md), [bounded runtime ADR](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/blob/5edbd0af5be408bcac4925fc989bce1a3f93af08/docs/adr/0005-bounded-agent-run-engine.md). Local `ec23c527` has the same tree as this main snapshot. Current behavior and missing integrations were checked against source/docs.

[^26]: Nnenna Ndukwe. **ThreadLoop source snapshot.** [README at 53309be2](https://github.com/nnennandukwe/threadloop/blob/53309be23311e8c2dcc3f135a1fa9c8be37cef77/README.md), [authority ADR](https://github.com/nnennandukwe/threadloop/blob/53309be23311e8c2dcc3f135a1fa9c8be37cef77/docs/adr/0001-sdlc-graph-authority-model.md), [graph contract scope](https://github.com/nnennandukwe/threadloop/blob/53309be23311e8c2dcc3f135a1fa9c8be37cef77/docs/contracts/workflow-graph-v0.1/README.md). Live main was used because the local checkout lagged; #105/PR #117 remained open at the snapshot.

[^27]: Nnenna Ndukwe. **RunInvariant source snapshot.** [README at e3eb0dfb](https://github.com/nnennandukwe/run-invariant/blob/e3eb0dfb36390c32d2cd84bbbdf903f5dc55de44/README.md). Decision-conformance scope and evidence limits. [Runtime conformance proposal #2](https://github.com/nnennandukwe/run-invariant/issues/2).

[^28]: Nnenna Ndukwe. **GAAP adoption tracker #21** and linked integration issues. [Tracker](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/21). Live issue states/dependencies checked September 10, 2026. Open issues record intended work, not completed behavior.

[^29]: Nnenna Ndukwe. **ThreadLoop evaluation assessment #86.** [Issue](https://github.com/nnennandukwe/threadloop/issues/86), [current audit schema](https://github.com/nnennandukwe/threadloop/blob/53309be23311e8c2dcc3f135a1fa9c8be37cef77/src/domain/audit.ts). Measurement and attribution gap; moving this work earlier is the report’s recommendation.
