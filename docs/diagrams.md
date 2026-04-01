# Governed Agent Autonomy Diagrams

## Control Plane Overview

This diagram shows the full control plane: planning, permissioning, trust review, verification, and visibility all sit between the operator and raw execution power.

```mermaid
flowchart TD
  U["Operator"] --> P["Planner"]
  P --> A["Plan Approval"]
  A --> E["Execution Path"]
  E --> G["Permission Gate"]
  G --> T["Tools and Commands"]
  G --> R["Tool Trust Review"]
  R --> T
  E --> V["Independent Verifier"]
  V --> D["Evidence + Verdict"]

  P --> S["Visibility Surfaces"]
  A --> S
  G --> S
  R --> S
  V --> S
```

## Plan Flow

This diagram shows that the planning path exists to delay mutation until the system has inspected context and a human has approved the proposed path.

```mermaid
flowchart LR
  R["Request"] --> X["Explore Context"]
  X --> N["Proposed Plan"]
  N --> H{"Explicit Approval?"}
  H -- "No" --> F["Refine or Reject"]
  F --> X
  H -- "Yes" --> E["Execute"]
```

## Permission Flow

This diagram shows why safe defaults matter: rule matching alone is not enough when dangerous operations need a harder stop.

```mermaid
flowchart LR
  Q["Tool Request"] --> M["Rule Evaluation"]
  M --> D["Dangerous Override Checks"]
  D --> O{"Outcome"}
  O -- "Allow" --> A["Run"]
  O -- "Ask" --> P["Prompt For Approval"]
  O -- "Deny" --> X["Block"]
```

## Verification Flow

This diagram shows the separation between implementation and verification. The verifier is a different role with a different job.

```mermaid
flowchart LR
  I["Implementer"] --> C["Changed Artifact"]
  C --> V["Independent Verifier"]
  V --> E["Evidence-Bearing Checks"]
  E --> R["Verdict"]
```

## Tool Trust Flow

This diagram shows that new servers, tools, and risky settings should not become trusted just because they exist.

```mermaid
flowchart LR
  N["New Tool, Server, or Setting"] --> K["Risk Review"]
  K --> D{"Decision"}
  D -- "Approve" --> A["Enable For Use"]
  D -- "Reject" --> B["Keep Disabled"]
  D -- "Needs Revision" --> R["Revise And Re-review"]
```

## Cross-Cutting Visibility Map

This diagram shows the surfaces that keep the whole control plane observable. Without these, even good controls become hard to trust.

```mermaid
flowchart TD
  P["Planning Events"] --> V["Visibility Layer"]
  G["Permission Decisions"] --> V
  T["Trust Reviews"] --> V
  R["Verification Verdicts"] --> V
  Q["Quota and Cost Signals"] --> V

  V --> L["Logs"]
  V --> N["Notifications"]
  V --> S["Approval States"]
  V --> A["Audit Trail"]
```
