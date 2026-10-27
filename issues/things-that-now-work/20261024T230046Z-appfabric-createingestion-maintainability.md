Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 8839c6cff18de024fc6b68ddfb2d1b408a6dd709

# The "authorization is Connected" rule is re-implemented in two handlers while state.rs's connected_authorization helper goes unused

_Filed: 2026-10-24T23:00:46Z_
_Operation: CreateIngestion | Class: maintainability_

> _Note: this issue was identified against the code base at commit `8839c6cff18de024fc6b68ddfb2d1b408a6dd709`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::AppBundleRecord::connected_authorization` exists and is documented as the lookup "an ingestion of that (app, tenant) pair requires", but nothing calls it: `grep` over `src/` finds the name only in its own definition and in the doc comment that points at it. The same rule (status == APP_AUTHORIZATION_STATUS_CONNECTED) is instead spelled out twice more in this crate: inline in CreateIngestion's dependency check and again as a private `is_connected` method in the StartUserAccessTasks handler. Three copies of one predicate, one of them dead, can drift — e.g. if the connected state ever needs to admit a second status, two live copies must both be found. Collapsing the live copies onto the existing helper changes no response and no test: both call sites keep the same decision (CreateIngestion also needs the held status for its message, which `authorization_for` already supplies).

## Evidence

The helper, unused — src/state.rs:274-283:
```
    /// The same authorization, but only once it has completed the connect
    /// handshake — the state an ingestion of that (app, tenant) pair requires.
    pub fn connected_authorization(
        &self,
        app: &str,
        tenant: &str,
    ) -> Option<&AppAuthorizationRecord> {
        self.authorization_for(app, tenant)
            .filter(|authorization| authorization.status == APP_AUTHORIZATION_STATUS_CONNECTED)
    }
```
(`grep -n 'connected_authorization' src/` matches only src/state.rs:184 (a doc link) and src/state.rs:276.)
Copy 1 — src/ops/create_ingestion.rs:238-243:
```
        let authorization_state = store
            .app_bundles
            .get(&bundle_key)
            .and_then(|bundle| bundle.authorization_for(&input.app, &input.tenant_id))
            .map(|authorization| authorization.status.clone());
        if authorization_state.as_deref() != Some(crate::state::APP_AUTHORIZATION_STATUS_CONNECTED) {
```
Copy 2 — src/ops/start_user_access_tasks.rs:145-149:
```
    /// Whether this authorization has completed the connect handshake, which is
    /// what AppFabric needs to reach the application at all.
    fn is_connected(&self) -> bool {
        self.status == crate::state::APP_AUTHORIZATION_STATUS_CONNECTED
    }
```
Existing tests that cover both call sites and must keep passing unchanged: tests/all/app_authorization_and_ingestion_dependency.rs (CreateIngestion refused for a non-Connected pair, lines 108-166 helpers) and tests/all/user_access_tasks.rs:70-96 (StartUserAccessTasks fan-out over connected vs. pending authorizations).

## Steps to Reproduce

1. grep -n 'connected_authorization' services/service-com-amazonaws-appfabric-fabricfrontendservice/src -> only the definition (state.rs:276) and a doc link (state.rs:184); no call site
2. grep -n 'APP_AUTHORIZATION_STATUS_CONNECTED' services/.../src/ops -> create_ingestion.rs:243 and start_user_access_tasks.rs:148 each re-implement the same predicate
3. Note the three copies of one rule, one of them dead code
