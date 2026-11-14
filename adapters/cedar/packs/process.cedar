// Exact process example. Review the scope and ordered input before pinning.
permit(principal == Gaap::AgentRun::"example-run", action == Gaap::Action::"sha256:1274ffc348712127e7a788eaa954f2f639852645050c899018dfdd954480896a", resource == Gaap::Process::"sha256:3fe0f934af2cdc34844fba8ac6cf4258b7bbd4cd493ab271cefbee29ad2e36b0");
forbid(principal, action, resource) when { context.subject_locator == "/workspace/blocked" };
