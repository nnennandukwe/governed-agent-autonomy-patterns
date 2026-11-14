// Exact network example. Review the scope and ordered input before pinning.
permit(principal == Gaap::AgentRun::"example-run", action == Gaap::Action::"sha256:80416680bb807dc8a3db9a76c50a27d13808a5afc3e05005a18c19baa1ee2687", resource == Gaap::Network::"sha256:b412251eb9d41cf877e17c54a62798dda1d4ab4bd5e188801af3953b78dc980a");
forbid(principal, action, resource) when { context.subject_locator == "/workspace/blocked" };
