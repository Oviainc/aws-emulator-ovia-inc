Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 804429c0e5267423598a5b7dbc8d0525cc046b43

# The spent-ClientToken replay/conflict rule is written out twice, once in CreateWorkflow and once in StartWorkflowRun

_Filed: 2026-10-28T02:33:54Z_
_Operation: CreateWorkflow | Class: maintainability_

> _Note: this issue was identified against the code base at commit `804429c0e5267423598a5b7dbc8d0525cc046b43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate single-homes its shared rules deliberately — the page-size derivation is "the single derivation every paginated listing of this service shares" (state.rs:1315-1317), the cursor scope check "lives in `crate::state::parse_scoped_page_token`, which every child listing of a workflow reads its cursor through" (list_workflow_versions.rs:224-226), the active-version promotion is centralised so "an update and a version-scoped delete settle the active version the same single way" (update_workflow.rs:341-343), and the configuration digest walk is shared "so a member added to the configuration reaches this digest and every other one at once" (create_workflow.rs:552-555).

The `@idempotencyToken` rule is the one rule that escaped that treatment: the four steps — build the per-operation, per-region ledger key; look the token up; compare the stored request fingerprint and refuse a mismatch with `ConflictException` carrying the recorded resource's identity; otherwise answer from the recorded record — are open-coded twice, once per handler, together with a per-file `token_key` whose format string differs only in the operation name. The two copies can drift (a change to the ledger key shape, to the fingerprint comparison, or to the conflict's `ResourceId`/`ResourceType` convention has to be made in two places and nothing fails if only one is changed).

A single helper in `crate::state` returning "replay this record / refuse with this conflict", parameterised by the operation name and the resource type the conflict reports, keeps every byte of every response identical: the two messages and both `ResourceType` values are already derived from those two parameters.

## Evidence

Copy 1 — services/.../src/ops/create_workflow.rs:291-315 and 458-463:
```
        if let Some(token) = input.client_token.as_deref() {
            if let Some(recorded) = store.idempotency.get(&token_key(&region, token)).cloned() {
                if recorded.request_fingerprint() != fingerprint {
                    return Err(CreateWorkflowErrorModeled::ConflictException(
                        ConflictException {
                            message: format!(
                                "ClientToken {token} was already used for a CreateWorkflow \
                                 request with different parameters"
                            ),
                            resource_id: recorded.workflow_arn().to_string(),
                            resource_type: "workflow".to_string(),
                        },
                    )
                    .into());
                }
...
fn token_key(region: &str, client_token: &str) -> String {
    format!("CreateWorkflow:{region}:{client_token}")
}
```
Copy 2 — services/.../src/ops/start_workflow_run.rs:262-301 and 404-409:
```
        if let Some(token) = input.client_token.as_deref() {
            if let Some(recorded) = store.idempotency.get(&token_key(&region, token)).cloned() {
                if recorded.request_fingerprint() != fingerprint {
                    return Err(StartWorkflowRunErrorModeled::ConflictException(
                        ConflictException {
                            message: format!(
                                "ClientToken {token} was already used for a StartWorkflowRun \
                                 request with different parameters"
                            ),
                            resource_id: recorded.workflow_arn().to_string(),
                            resource_type: "workflow-run".to_string(),
                        },
                    )
                    .into());
                }
...
fn token_key(region: &str, client_token: &str) -> String {
    format!("StartWorkflowRun:{region}:{client_token}")
}
```
The recording half is duplicated too (create_workflow.rs:441-449 vs start_workflow_run.rs:384-393), both calling `store.idempotency.record(token_key(&region, token), ...)`.

Contrast with the rules this crate DOES single-home, e.g. state.rs:1315-1317: "The page size one walk of a paginated listing serves, from the `MaxResults` the request carries — the single derivation every list handler of this service reads its page size through, beside the cursor helpers they already share."

Existing tests that pin both copies' responses (and must stay green and unchanged): tests/all/workflow_lifecycle.rs:467-511 ("a repeated ClientToken must replay the first response", "expected one workflow per ClientToken") and tests/all/workflow_lifecycle.rs:912-965 (`starting_a_run_is_idempotent_and_an_unknown_workflow_is_refused`); tests/all/workflow_configuration_digest.rs:246,326 and tests/all/workflow_digest_member_boundaries.rs:83,239 ("expected the conflict to name the reused ClientToken"); tests/all/run_report_agreement.rs:56,215,288 ("a repeated ClientToken must replay the run it started"); tests/all/region_isolation.rs:498 (per-region token scoping).

## Steps to Reproduce

1. Open services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/ops/create_workflow.rs and read lines 291-315 plus the `token_key` helper at 458-463.
2. Open services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/ops/start_workflow_run.rs and read lines 262-301 plus the `token_key` helper at 404-409: the same four-step rule, the same ledger key shape and the same conflict construction, differing only in the operation name and the reported `ResourceType`.
3. Compare with state.rs:1315-1328 and list_workflow_versions.rs:222-234, where the crate states that the page-size derivation and the cursor scope check are each single-homed for exactly this reason.
4. Change the ledger key shape (or the conflict's `ResourceId` convention) in one copy only: nothing in the crate detects that the other copy still follows the old rule.
