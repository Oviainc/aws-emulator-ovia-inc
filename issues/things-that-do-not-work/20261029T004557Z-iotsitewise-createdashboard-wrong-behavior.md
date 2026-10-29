Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1f07a4a6c58d968bd98c0da95eee61e4f4add38f

# CreateDashboard replays a dashboard on the clientToken alone, so a DIFFERENT create reusing a spent token is answered 201 with the FIRST dashboard's id and creates nothing

_Filed: 2026-10-29T00:45:57Z_
_Operation: CreateDashboard | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1f07a4a6c58d968bd98c0da95eee61e4f4add38f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateDashboard recognises a retry by looking a stored dashboard up BY TOKEN only — `store.dashboard_for_client_token(token)` — and returns that dashboard's id and ARN before the request is resolved against the store at all. The lookup compares nothing but the token (`DashboardRecord::matches_client_token` is `self.client_token.as_deref() == Some(client_token)`), so a create carrying a DIFFERENT body (another `projectId`, `dashboardName`, `dashboardDescription`, `dashboardDefinition` or `tags`) that happens to reuse a token an earlier create spent is answered 201 with the earlier dashboard's identifiers, and the dashboard the caller actually asked for is never created: the handler returns before `store.dashboards.insert(...)` at line 274. Every later call the caller makes with that id operates on a dashboard with a different name and definition, possibly in another project.

This crate states the opposite rule in two places of its own. The shared idempotency registry distinguishes a repeat from a different request by FINGERPRINT (`TokenVerdict::Replay` vs `TokenVerdict::Spent`), and the sibling create that also matches a token per record — CreateApplication — compares every member the create carries before it will replay, and otherwise refuses with the modelled conflict. UpdateDashboard, the same resource family, was converted to fingerprint its request so that a token spent on a different request is never turned into a success that wrote nothing; CreateDashboard is the one dashboard write where the token alone decides.

## Evidence

The handler replaying on the token alone:
  services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/create_dashboard.rs:204-211 —
    "        if let Some(token) = input.client_token.as_deref() {
            if let Some(existing) = store.dashboard_for_client_token(token) {
                return Ok(CreateDashboardOutput {
                    dashboard_id: existing.id.clone(),
                    dashboard_arn: existing.arn.clone(),
                });
            }
        }"
  skipping the write it would otherwise do, src/ops/create_dashboard.rs:274 — "store.dashboards.insert(record.id.clone(), record);"

What that lookup compares — the token and nothing else:
  src/state.rs:8580-8582 — "    fn matches_client_token(&self, client_token: &str) -> bool {
        self.client_token.as_deref() == Some(client_token)
    }"
  src/state.rs:4916-4920 — "    pub fn dashboard_for_client_token(&self, client_token: &str) -> Option<&DashboardRecord> {
        self.dashboards
            .values()
            .find(|record| record.matches_client_token(client_token))
    }"

The sibling per-record token match in this same crate, which does compare the request:
  src/ops/create_application.rs:229-263 — "The retry has to be the SAME request, so the members that identify the application are compared too; a token already spent on a different request collides with the application it made, rather than answering from it." ... "if existing.workspace_name == input.workspace_name && existing.name == input.name && existing.idc_instance_arn == input.idc_instance_arn && existing.description == input.description && existing.tags == input.tags.clone().unwrap_or_default() { return Ok(created(existing, now)); } return Err(... ConflictingOperationException ...)"

The crate's registry definition of the two verdicts (a differing request is NOT a replay):
  src/state.rs:9338-9342 — "match self.idempotent_calls.get(&idempotency_key(operation, token)) {
            Some(call) if call.request == request => TokenVerdict::Replay(call.resource_id.clone()),
            Some(call) => TokenVerdict::Spent(call.resource_id.clone()),
            None => TokenVerdict::Serve,
        }"

The same resource family's update, which fingerprints its request for exactly this reason:
  src/ops/update_dashboard.rs:172-177 — "A token already spent on a DIFFERENT request names no answer this operation could replay ... the token stops a repeat from being applied twice, it never turns another request into a success that wrote nothing."
  src/ops/update_dashboard.rs:180-189 — "let fingerprint = crate::state::request_fingerprint(&[&input.dashboard_id, &input.dashboard_name, &input.dashboard_description, &input.dashboard_definition], None); match store.idempotent_verdict(Self::NAME, input.client_token.as_deref(), &fingerprint) {"

Test coverage: nothing pins the current answer. tests/all/dashboard_lifecycle.rs mentions `clientToken` only as a length bound (dashboard_lifecycle.rs:66-69), and tests/all/client_token_spent_on_another_request.rs covers UpdateDashboard (lines 272-301) and DeleteDashboard (lines 328-343) but never sends one token with two different CreateDashboard bodies.

## Steps to Reproduce

1. CreatePortal and CreateProject to get a project P1; CreateProject again for a second project P2.
2. POST /dashboards with {"projectId": P1, "dashboardName": "First", "dashboardDefinition": "{\"widgets\":[]}", "clientToken": "retry-token-1-000000000000000000000"} (36+ chars) -> 201, dashboard D1 created.
3. POST /dashboards with {"projectId": P2, "dashboardName": "Second", "dashboardDefinition": "{\"widgets\":[{\"title\":\"t\"}]}"} and the SAME clientToken.
4. Observed: 201 carrying D1's dashboardId and dashboardArn. GET /dashboards/D1 still reports dashboardName "First" in project P1, and GET /dashboards?projectId=P2 lists nothing — the second dashboard was never created.
5. Compare CreateApplication (src/ops/create_application.rs:229-263): the same reuse with a differing body is refused with ConflictingOperationException naming the application the first call made, instead of being answered from it.
