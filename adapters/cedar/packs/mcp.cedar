// Exact mcp example. Review the scope and ordered input before pinning.
permit(principal == Gaap::AgentRun::"example-run", action == Gaap::Action::"sha256:b8772a1f64fd31a4fff1c41bb7b91b00eb99946e53b57f4e03f51593d48da8ff", resource == Gaap::ExternalService::"sha256:f8795c5181315e0775bb01b5001fdfd6a9d9e9d64eb36de592ccc17d7ac6ce8e");
forbid(principal, action, resource) when { context.subject_locator == "/workspace/blocked" };
