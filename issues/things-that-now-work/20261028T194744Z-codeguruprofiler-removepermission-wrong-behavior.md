Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3945994041ceeb2d9433bf9ac91cf7eef00f38b4

# RemovePermission's 200 can never carry policy or revisionId: the only reachable success path answers an empty body

_Filed: 2026-10-28T19:47:44Z_
_Operation: RemovePermission | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3945994041ceeb2d9433bf9ac91cf7eef00f38b4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

RemovePermission models BOTH response members as `@required` (`policy`, `revisionId`), and `policy`'s own doc says it reports "The JSON-formatted resource-based policy on the profiling group after the specified permissions were removed." The emulator's success path can never report either of them for ANY input the model admits, so every successful RemovePermission answers HTTP 200 with the body `{}`.

Why it is every call, not an edge case — this follows from the crate's own two constants/branches:

1. `MAX_POLICY_STATEMENTS` is defined as `crate::ops::types::ACTION_GROUP_VALUES.len()`, i.e. the number of values the model's `ActionGroup` enum declares, which is ONE (`agentPermissions`); `put_permission` refuses any write whose result `would_hold > MAX_POLICY_STATEMENTS`. A stored resource policy therefore never holds more than one statement.
2. `remove_permission` returns `PolicyWrite::Cleared` exactly when `policy.statements.len() == 1`, and the handler maps `Cleared` to `RemovePermissionOutput { policy: None, revision_id: None }`.

Since a policy can only ever hold one statement, every removal that gets past the group / no-policy / no-statement / revision checks takes the last statement, so `Cleared` is the ONLY reachable success — and the `PolicyWrite::Written` arm (both in `state.rs` and in the handler) is dead code for every request the public model allows. `RemovePermissionOutput` was even re-declared with hand-written optional members to make that empty body expressible, "for the same reason as `GetPolicyResponse`" — but the GetPolicy case is about a group that never had a policy, where the operation still reports both members once one exists, whereas RemovePermission reports them never.

Effect: a generated SDK client that reads the two members the model marks required gets nothing back on every successful RemovePermission, and a caller cannot learn the revision (or the state of the policy) the removal produced. The sibling writer of the same policy surface disagrees: `PutPermissionOutput` keeps `policy`/`revision_id` as non-`Option` fields its serializer always writes, so one half of the policy surface always reports the document and revision and the other half never does.

## Evidence

Model — `smithy_query(query="operation", operation="RemovePermission")`:
```
"output": {"policy": {"target": "String", "required": true},
           "revisionId": {"target": "RevisionId", "required": true,
                          "pattern": "[a-f0-9]{8}-..."}}
```
Model — `smithy_query(query="doc", shape="RemovePermissionResponse", member="policy")`:
"The JSON-formatted resource-based policy on the profiling group after the specified permissions were removed."

The only reachable success arm — services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/src/ops/remove_permission.rs:202-209:
```
            // The removal took the last statement, so the group now has no
            // resource-based policy: there is no document and no revision to report,
            crate::state::PolicyWrite::Cleared => Ok(RemovePermissionOutput {
                policy: None,
                revision_id: None,
            }),
```
Why it is the only one — src/state.rs:1562-1571:
```
        // Whether this removal takes the policy's LAST statement: then the group is
        // left with no policy, so nothing is rendered and no revision is minted.
        if policy.statements.len() == 1 {
            ... group.policy = None;
            return PolicyWrite::Cleared;
        }
```
and src/state.rs:404-409, which bounds a policy at ONE statement:
```
/// Every action group a resource-based policy can hold a statement for: the values the
/// model's `ActionGroup` enum declares, as the generated
/// [`crate::ops::types::ACTION_GROUP_VALUES`] lists them (one, `agentPermissions`) ...
pub const MAX_POLICY_STATEMENTS: usize = crate::ops::types::ACTION_GROUP_VALUES.len();
```
enforced in src/state.rs:1486-1491 (`if would_hold > MAX_POLICY_STATEMENTS { return PolicyWrite::LimitExceeded(...) }`), so the `PolicyWrite::Written` arm of src/state.rs:1574-1591 and of src/ops/remove_permission.rs:195-201 is unreachable.

The sibling writer, which always reports both — src/ops/types/put_permission_types.rs:25-27:
```
pub struct PutPermissionOutput {
    pub policy: String,
    pub revision_id: String,
```
versus src/ops/types/remove_permission_types.rs:22-32:
```
/// `RemovePermissionResponse`. Its schema and codec come from
/// [`profiler_schema_and_codec!`] ... for the same reason as `GetPolicyResponse`: both
/// members are `@required` in the model yet neither is in the body of the 200 that
/// answers a removal which took the policy's LAST statement
pub struct RemovePermissionOutput {
    pub policy: Option<String>,
    pub revision_id: Option<String>,
}
```

Test coverage that pins today's empty body — tests/all/permission_writes.rs:578-585 (in `remove_permission_drops_the_statement_and_deletes_the_policy_with_its_last_one`):
```
        // It took the policy's only statement, so the group now has no resource-based
        // policy: no document and no revision are reported, because none is stored.
        assert_eq!(
            removed.json(),
            serde_json::json!({}),
```
(tests/all/policy_surface.rs:217,249,289,390,487 also drive RemovePermission.)

## Steps to Reproduce

1. Boot the emulator and point an AWS SDK codeguruprofiler client at it.
2. CreateProfilingGroup(profilingGroupName="pg", clientToken="t1").
3. PutPermission(profilingGroupName="pg", actionGroup="agentPermissions", principals=["arn:aws:iam::000000000000:role/r"]) — note the returned `policy` and `revisionId` are both present.
4. RemovePermission(profilingGroupName="pg", actionGroup="agentPermissions", revisionId=<the revisionId just returned>).
5. Observe HTTP 200 with body `{}`: neither `policy` nor `revisionId` is present, although the model marks both `@required` on `RemovePermissionResponse`. Reading `response["revisionId"]` fails.
6. Confirm no input can reach the other branch: `agentPermissions` is the only value the model's `ActionGroup` enum declares (src/state.rs:404-409), so `policy.statements.len() == 1` always holds at src/state.rs:1564 and `PolicyWrite::Written` (src/state.rs:1574-1591) is never returned by `remove_permission`.
