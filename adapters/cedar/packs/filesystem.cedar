// Exact filesystem example. Review the scope and ordered input before pinning.
permit(principal == Gaap::AgentRun::"example-run", action == Gaap::Action::"sha256:45a2a2c59bc9aa5f6cc7918560b85b5be41f5215f1e0fe4895ec2adbe6b6338d", resource == Gaap::Filesystem::"sha256:abb72db79fda711f4400d7eb912665352c14d1ff1e2b3eed04611486e8a26c32");
forbid(principal, action, resource) when { context.subject_locator == "/workspace/blocked" };
