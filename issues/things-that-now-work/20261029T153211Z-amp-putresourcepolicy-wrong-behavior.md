Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 95be350e2e64bcff4ad89a5a8e5d1ae15d5b4a08

# A put replaying a clientToken is answered 202 off a policy whose removal the clock already owes, so the call persists nothing

_Filed: 2026-10-29T15:32:11Z_
_Operation: PutResourcePolicy | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `95be350e2e64bcff4ad89a5a8e5d1ae15d5b4a08`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutResourcePolicy`'s idempotency replay matches on the stored token ALONE: if the attached policy's `client_token` equals the request's `clientToken`, the handler returns the stored policy and writes nothing — before it consults `crate::state::policy_mutation_allowed`, which is the gate that refuses a put while the policy or the workspace is not ACTIVE.

Two consequences, both of them the outcome this crate says nothing produces:

1. After `DeleteResourcePolicy` has been accepted, the policy is still held and reports `DELETING` while its removal is owed on the clock. `DeleteResourcePolicy` records its own `delete_token` but leaves `client_token` (the put's token) standing, so a put re-sending that token takes the replay branch: it answers 202 with `policyStatus: DELETING` and the previous `revisionId`, stores nothing, and moments later the clock drops the policy entirely — an accepted put that left the store holding nothing for it, and the caller's `policyDocument` gone. The identical put WITHOUT a `clientToken` in that same window is refused 409 `ConflictException` (pinned by the suite), so the presence of an idempotency token turns a refusal into a hollow success.

2. The replay branch also ignores the WORKSPACE's status, so a put replaying the token on a workspace whose own deletion is owed (`DELETING`) is answered 202 instead of the 409 the non-token path gives.

Every sibling replay path in this crate guards exactly this: the two alert-manager handlers require the record not to be `DELETING` (and the put also requires the workspace to be `ACTIVE`), the four keyed/minted creates filter the token lookup on `status_code != STATUS_DELETING`, and the two single-valued configuration creates refuse outright while a removal is owed. The two sibling deletes additionally CLEAR the token of the write that made the record, with a comment stating why; `DeleteResourcePolicy` is the one delete that does not.

## Evidence

THE REPLAY IS DECIDED ON THE TOKEN ALONE, BEFORE THE STATUS GATE — src/ops/put_resource_policy.rs:222-238:
```
        let existing = workspace.policy.clone();

        // `clientToken` carries `@idempotencyToken`: replaying the very call that
        // produced the current revision answers with THAT revision — read off the
        // stored policy, so the status it reports is the one the policy is in now —
        // instead of minting a second one.
        if let (Some(token), Some(policy)) = (input.client_token.as_deref(), existing.as_ref()) {
            if policy.client_token.as_deref() == Some(token) {
                return Ok(response_for(policy));
            }
        }

        // "Update/Deletion is disallowed until Resource-based Policy is ACTIVE and
        // Workspace is ACTIVE" (`WorkspacePolicyStatusCode`) ...
        if !crate::state::policy_mutation_allowed(workspace) {
```

THE SIBLING PUT OF THE OTHER SINGLE-VALUED WORKSPACE CHILD GUARDS BOTH — src/ops/put_alert_manager_definition.rs:235-247:
```
        // `clientToken` carries `@idempotencyToken`: re-sending the very call that
        // applied an update ... It is a replay
        // only while the record is still what that call left.
        if input.client_token.is_some()
            && definition.change_token == input.client_token
            && definition.data == input.data
            && definition.status_code != crate::state::DEFINITION_STATUS_DELETING
            && workspace_status == crate::state::STATUS_ACTIVE
        {
            return Ok(response_for(definition));
        }
```
and src/ops/create_alert_manager_definition.rs:244-250 (`&& existing.status_code != crate::state::DEFINITION_STATUS_DELETING`), src/ops/create_workspace.rs:297 / src/ops/create_scraper.rs:279 / src/ops/create_anomaly_detector.rs:299 / src/ops/create_rule_groups_namespace.rs:265 (all `status_code != …STATUS_DELETING` on the token lookup), src/ops/create_logging_configuration.rs:198-213 and src/ops/create_query_logging_configuration.rs:195-211, whose comment states the rule:
```
            // A record whose removal the clock already owes is NOT the record the create
            // that made it left: `crate::state::materialize_due` drops it once its due
            // instant has passed, so answering a create from it would accept the call
            // (202) and leave the workspace holding no configuration at all.
```

THE DELETE THAT LEAVES THE PUT'S TOKEN STANDING — src/ops/delete_resource_policy.rs:299-301 writes only:
```
            policy.status_code = crate::state::POLICY_STATUS_DELETING.to_string();
            policy.transition_due = Some(now + crate::state::TRANSITION_DELAY);
            policy.delete_token = input.client_token;
```
while both sibling deletes clear the token of the write that made the record — src/ops/delete_logging_configuration.rs:277-280:
```
        // The stored update token stood for the update that made this record what it is;
        // it does not speak for a record on its way out, so a replay of that update is a
        // new request against a configuration that is being deleted.
        logging.update_token = None;
```
(and src/ops/delete_query_logging_configuration.rs:231-235, same comment and `configuration.update_token = None;`).

THE RULE THE CRATE'S OWN SUITE PINS — tests/all/create_while_a_removal_is_owed.rs:6-12:
```
//! The rule these suites pin: an idempotent create answers as a replay only from a
//! stored record that is still the one its original call left, so a record whose
//! removal the clock already owes (the transient `DELETING` every deletion in this
//! service writes) is never what a create is answered from — a create in that window
//! either refuses, where the resource is single-valued and the one it would create is
//! being deleted, or creates a new resource, where the resource is keyed or minted.
//! Nothing accepts the call and then leaves the store holding nothing for it.
```
That the record really goes away — src/state.rs:1086-1096: `if policy.status_code == POLICY_STATUS_DELETING { true } … if policy_removed { workspace.policy = None; }`.

TEST COVERAGE: tests/all/resource_policy_lifecycle.rs:579-634 (`replaying_a_client_token_mints_no_second_revision`) exercises a replay only while the policy is ACTIVE; tests/all/resource_policy_lifecycle.rs:901-949 drives the policy into `DELETING` and then asserts a put is refused 409 — but that put carries NO `clientToken` (`&serde_json::json!({ "policyDocument": policy_document("Racing", 1) })`), so the token-carrying path in that window is pinned by nothing.

## Steps to Reproduce

1. POST /workspaces with {"alias":"p"} and wait past the owed transition so the workspace reports ACTIVE
2. PUT /workspaces/{workspaceId}/policy with {"policyDocument": D1, "clientToken": "t1"} — 202; wait past the owed transition so DescribeResourcePolicy reports ACTIVE with revision R1
3. DELETE /workspaces/{workspaceId}/policy — 202; the policy now reports DELETING with its removal owed on the clock
4. PUT /workspaces/{workspaceId}/policy with {"policyDocument": D2, "clientToken": "t1"} — answered 202 with policyStatus DELETING and revisionId R1, and nothing is written (the same request without clientToken is refused 409 ConflictException)
5. Wait past the owed transition, then GET /workspaces/{workspaceId}/policy — 404: the put that was accepted persisted nothing and D2 is lost
6. Variant for the workspace gate: after step 2, DELETE /workspaces/{workspaceId} (202, workspace DELETING), then repeat the step-4 put — answered 202 instead of the 409 the non-token path gives
