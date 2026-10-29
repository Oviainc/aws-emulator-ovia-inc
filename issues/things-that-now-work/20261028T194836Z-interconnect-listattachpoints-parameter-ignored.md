Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: a67556f919df77bd586efc9ebe2e6b88c4f9f9e7

# ListAttachPoints never scopes the listing to the Environment it is given: environmentId is only existence-checked

_Filed: 2026-10-28T19:48:36Z_
_Operation: ListAttachPoints | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `a67556f919df77bd586efc9ebe2e6b88c4f9f9e7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model, and the handler's own generated contract header, say the listing is the attach points that are VALID FOR the specified Environment, and the required `environmentId` member is documented as "The identifier of the Environment for which to list valid Attach Points." The handler uses `environmentId` for exactly one thing — an existence check against the seeded catalog — and then computes the page from `crate::state::direct_connect::attach_points(ctx, &region)`, a call that takes no environment at all. Every ready Direct Connect gateway of the account is therefore reported for EVERY environment, and the answer for `env-csp-azure-us-east-1` is byte-identical to the answer for `env-lmp-megaport-sydney`, including for the seeded `unavailable` environment on which `CreateConnection` refuses outright. The result is that the member a caller must send has no effect on the result, so a caller cannot use the listing to tell which attach points one environment accepts from the ones another does.

The crate is not unaware of the concept: it threads `environment_id` through `judge_attach_point` and words every refusal with it (`attach_point_not_valid_message` -> "... is not an attach point valid for environment {environment_id}"), while the verdict that produces that wording (`resolve_attach_point`) is computed from a walk that ignores the environment. So the refusal claims a per-Environment judgement that neither the listing nor the resolution ever makes.

## Evidence

The rule, from the model and quoted verbatim in the handler's own header — services/service-com-amazonaws-interconnect-interconnect/src/ops/list_attach_points.rs:43-49:
```
//! Lists all Attach Points the caller has access to that are valid for the specified
//! Environment.
//!
//! ## Input
//!
//! * `environmentId` (EnvironmentId, len 1..64) **required** — The identifier of the
//!   Environment for which to list valid Attach Points.
```
(the same sentence from `smithy_query(query="doc",shape="ListAttachPoints")`: "Lists all Attach Points the caller has access to that are valid for the specified Environment.")

What the handler actually does with it — list_attach_points.rs:165-173 (existence check only):
```
        let environment_is_known = state.lock().environments.contains_key(&input.environment_id);
        if !environment_is_known {
            return Err(ListAttachPointsErrorModeled::InterconnectClientException(
```
and list_attach_points.rs:202, the one call that builds the page, which carries no environment:
```
        let descriptors = crate::state::direct_connect::attach_points(ctx, &region).await;
```
Its signature confirms there is no environment seam — src/state/direct_connect.rs:214-219:
```
pub async fn attach_points(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> Vec<AttachPointDescriptor> {
    walk_attach_points(ctx, region).await.0
}
```
Note also that the handler's inline comment re-quotes the model sentence with the environment clause cut off — list_attach_points.rs:196:
```
        // "Lists all Attach Points the caller has access to": `AttachPointType`'s only
```
while the refusal wording the crate shares between the two writers does claim a per-Environment verdict — src/state.rs:557-562:
```
pub fn attach_point_not_valid_message(identifier: &str, environment_id: &str) -> String {
    format!(
        "{REFUSAL_ENVELOPE}{identifier} is not an attach point valid for environment \
         {environment_id}"
    )
}
```
and the verdict behind it is computed from the same environment-free walk — src/state/direct_connect.rs:121-137 (`resolve_attach_point` takes only `ctx`, `region`, `identifier`).

Test coverage: tests/all/attach_points.rs and tests/all/caller_created_gateways_are_attach_points.rs assert that a created gateway appears in the listing, but no test asserts that two different `environmentId`s can ever produce different pages, so nothing pins the ignored member.

## Steps to Reproduce

1. Mount Interconnect beside Direct Connect (as tests/all/emulator.rs does) and create one ready Direct Connect gateway.
2. Call ListEnvironments and note two seeded environments with different providers, e.g. env-csp-azure-us-east-1 (cloudServiceProvider/available) and env-lmp-megaport-sydney (lastMileProvider/unavailable).
3. Call ListAttachPoints(environmentId='env-csp-azure-us-east-1') and record the attachPoints list.
4. Call ListAttachPoints(environmentId='env-lmp-megaport-sydney') and record the attachPoints list.
5. Observe the two lists are identical, including for the environment on which CreateConnection refuses every request — although src/ops/list_attach_points.rs:43-44 states the listing is of the attach points "that are valid for the specified Environment" and list_attach_points.rs:48-49 documents environmentId as "The identifier of the Environment for which to list valid Attach Points".
6. Confirm the member reaches nothing but the existence check: the only consumer of input.environment_id is list_attach_points.rs:165, and the page comes from list_attach_points.rs:202, whose callee signature (src/state/direct_connect.rs:214-219) accepts no environment.
