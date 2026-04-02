# Quota Threshold Policy

```text
Threshold policy:

- If remaining included usage falls below warning threshold:
  alert operator

- If a workflow would require extra usage:
  require explicit approval before continue

- If extra usage is disabled:
  block remote execution

- If balance is low during execution:
  ask for confirmation or stop according to policy

- If maximum session spend is exceeded:
  terminate or hand back to operator
```
