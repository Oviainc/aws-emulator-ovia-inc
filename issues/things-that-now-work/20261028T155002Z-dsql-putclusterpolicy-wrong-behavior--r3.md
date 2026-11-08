Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: e113b6e45c2c86d79f2618efd25a6c62e8ae6901

# The policy-lockout safety check treats NotAction/NotResource as complements but ignores NotPrincipal entirely

_Filed: 2026-10-28T15:50:02Z_
_Operation: PutClusterPolicy | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `e113b6e45c2c86d79f2618efd25a6c62e8ae6901`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::validate_cluster_policy` — the one check both `PutClusterPolicy` and `CreateCluster` run over a resource-based policy document — decides "does this document leave nobody able to replace it" with three set matchers: `covers_every_principal`, `covers_management_action` and `covers_this_resource`.

Two of the three implement IAM's negated form as the COMPLEMENT of the set, and say so in their own doc comments: `covers_management_action` honours `NotAction`, and `covers_this_resource` honours `NotResource`. The third, `covers_every_principal`, has no `NotPrincipal` branch at all: its `match` arms cover only `Principal` as a string and `Principal` as a block, and everything else falls into `_ => false`.

The consequence is that a `Deny` statement written with `NotPrincipal` instead of `Principal` is treated as covering NO principal, so `locks_out_every_principal` short-circuits to `false` and the safety check never fires. A document such as

    {"Version":"2012-10-17",
     "Statement":{"Effect":"Deny",
                  "NotPrincipal":{"AWS":"arn:aws:iam::111122223333:role/nobody"},
                  "Action":"dsql:*",
                  "Resource":"*"}}

is attached with 200 and no `bypassPolicyLockoutSafetyCheck`, although it denies `dsql:PutClusterPolicy` to every principal except one the emulator does not own — exactly the class of document the check exists to refuse, and exactly what the same statement spelled `"Principal":"*"` plus `"NotAction":"dsql:DbConnect"` IS refused for today (that `NotAction` case is in the pinned table).

The same hole is reachable through `CreateCluster`'s inline `policy`, which calls the identical helper.

## Evidence

The two matchers that DO honour the negated form, with their stated rule:

services/service-com-amazonaws-dsql-dsql/src/state.rs:1943-1955
```
/// Whether the statement's `Action` set covers [`POLICY_MANAGEMENT_ACTION`].
/// `NotAction` is the complement, so it covers the action unless it names it.
fn covers_management_action(statement: &::serde_json::Map<String, ::serde_json::Value>) -> bool {
    if let Some(actions) = statement.get("Action") { ... }
    if let Some(not_actions) = statement.get("NotAction") {
        return !string_values(not_actions)
            .iter()
            .any(|action| matches_pattern(action, POLICY_MANAGEMENT_ACTION));
    }
```

services/service-com-amazonaws-dsql-dsql/src/state.rs:1966-1982
```
/// An absent `Resource` is the whole resource in a resource-based policy, which is
/// this cluster; `NotResource` is the complement, so it covers this cluster unless
/// it names it.
...
    if let Some(not_resources) = statement.get("NotResource") {
        return !string_values(not_resources)
            .iter()
            .any(|resource| matches_pattern(resource, resource_arn));
    }
```

The third matcher, which does not:

services/service-com-amazonaws-dsql-dsql/src/state.rs:1930-1941
```
/// Whether the statement applies to every principal: `"Principal": "*"`, or a
/// `Principal` block whose `AWS` entry is `*`.
fn covers_every_principal(statement: &::serde_json::Map<String, ::serde_json::Value>) -> bool {
    match statement.get("Principal") {
        Some(::serde_json::Value::String(principal)) => principal == "*",
        Some(::serde_json::Value::Object(block)) => block
            .values()
            .flat_map(string_values)
            .any(|principal| principal == "*"),
        _ => false,
    }
}
```

The rule the three matchers together are supposed to decide, state.rs:1889-1903:
```
/// Read the way an IAM resource policy is read: an explicit `Deny` beats every
/// `Allow`, so a `Deny` statement that covers all principals, covers
/// [`POLICY_MANAGEMENT_ACTION`], covers the cluster being protected and carries no
/// `Condition` narrowing it leaves nobody able to call that action again.
fn locks_out_every_principal(...) -> bool {
    statements(document).iter().any(|statement| {
        effect_is(statement, "Deny")
            && !statement.contains_key("Condition")
            && covers_every_principal(statement)
```

Both writers reach the same helper, so neither entry point refuses the document:
- services/service-com-amazonaws-dsql-dsql/src/ops/put_cluster_policy.rs:246-252 `crate::state::validate_cluster_policy(&input.policy, &cluster_arn, input.bypass_policy_lockout_safety_check)`
- services/service-com-amazonaws-dsql-dsql/src/ops/create_cluster.rs:280-288 the same call on the inline `policy`

Test coverage confirms the negated forms of the OTHER two sets are pinned while `NotPrincipal` is absent from every table. services/service-com-amazonaws-dsql-dsql/tests/all/inline_policy_validation.rs:19-21 states the span:
```
//! action set (`Action`, a wildcard family, `NotAction`), of its principal set (a
//! string wildcard, an `AWS` block, one named role) and of its resource set
//! (absent, `*`, a wildcard over the account's clusters, another cluster's ARN)
```
and services/service-com-amazonaws-dsql-dsql/tests/all/error_contract.rs:830-834 pins the `NotAction` lockout as refused:
```
        (
            "deny every principal everything but one other action",
            json!({"Effect": "Deny", "Principal": "*", "NotAction": "dsql:DbConnect"}),
            true,
        ),
```
No case anywhere under services/service-com-amazonaws-dsql-dsql/tests/ names `NotPrincipal`, so nothing pins the current answer with a stated reason.

## Steps to Reproduce

1. Create a cluster and wait for it to settle to ACTIVE (`POST /cluster` with `{"deletionProtectionEnabled": false}`, then `GET /cluster/{identifier}` until `status` is `ACTIVE`).
2. Call `POST /cluster/{identifier}/policy` with `bypassPolicyLockoutSafetyCheck` unset and body `{"policy": "{\"Version\":\"2012-10-17\",\"Statement\":{\"Effect\":\"Deny\",\"NotPrincipal\":{\"AWS\":\"arn:aws:iam::111122223333:role/nobody\"},\"Action\":\"dsql:*\",\"Resource\":\"*\"}}"}`.
3. Observe 200 with a fresh `policyVersion`, and `GET /cluster/{identifier}/policy` reading the document back: the lockout safety check did not fire.
4. Repeat with the equivalent statement spelled `"Principal": "*"` plus `"NotAction": "dsql:DbConnect"` and observe 400 `ValidationException` with `reason` `other` — the same class of document, refused, because `covers_management_action` honours its negated form while `covers_every_principal` has no `NotPrincipal` branch.
5. The same pair of requests through `CreateCluster`'s inline `policy` member shows the identical asymmetry, since both call `crate::state::validate_cluster_policy`.
