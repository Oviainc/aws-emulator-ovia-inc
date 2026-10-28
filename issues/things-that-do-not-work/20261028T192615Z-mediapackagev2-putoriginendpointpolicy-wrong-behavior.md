Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: e8dc968dccb3edde6e1cac399927b1a6964cab15

# A Deny statement written with NotPrincipal/NotAction/NotResource never denies, although policy_allows_anonymous's own doc says it does

_Filed: 2026-10-28T19:26:15Z_
_Operation: PutOriginEndpointPolicy | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `e8dc968dccb3edde6e1cac399927b1a6964cab15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::policy_allows_anonymous` is the one authorization decision the egress data plane makes for a policy stored by `PutOriginEndpointPolicy`. Its doc states the fail-closed rule in both directions: an `Allow` carrying a member the evaluator cannot judge never grants, and "A `Deny` carrying one still denies, which is the closed side there too."

The code only honours the first half. The `matching` closure that both the Deny pass and the Allow pass go through requires the statement to carry an `Action` member and a `Principal` member naming `*`: `statement_names_action` returns `false` when there is no `Action` key, and `statement_names_anonymous_principal` returns `false` when there is no `Principal` key. A statement written in the inverted IAM form — `NotAction`, `NotPrincipal`, or a `NotResource`-only statement — therefore carries neither, so `matching(statement, "Deny")` is `false` and the Deny pass skips it entirely.

The consequence is that an explicit `Deny` the caller stored is dropped, and a sibling `Allow` to `*` is honoured unchanged: the anonymous egress fetch the policy denies is authorized (`serve_egress` falls through to the "no content yet" 404) instead of being refused 403. That is the opposite of the closed side the helper documents, and the opposite of how the same inverted members are treated on the `Allow` pass, where `carries_unevaluated_member` deliberately withholds the grant.

## Evidence

The rule the helper states for itself — services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/state.rs:1592-1597:
```
/// This evaluator understands `Effect`, `Action`, `Principal` and `Resource` only, so it
/// fails CLOSED on anything else: a statement carrying a member it cannot evaluate
/// ([`UNEVALUATED_STATEMENT_MEMBERS`] — `Condition`, `NotAction`, `NotPrincipal`,
/// `NotResource`) never GRANTS, because honouring a conditional `Allow` as an
/// unconditional one would authorize a fetch on a condition nothing checked. A `Deny`
/// carrying one still denies, which is the closed side there too.
```

The code under it — state.rs:1611-1622:
```
    let matching = |statement: &::serde_json::Value, effect: &str| {
        statement.get("Effect").and_then(::serde_json::Value::as_str) == Some(effect)
            && statement_names_action(statement, action)
            && statement_names_anonymous_principal(statement)
            && statement_names_resource(statement, resource)
    };
    if statements
        .iter()
        .any(|statement| matching(statement, "Deny"))
    {
        return false;
    }
```

Why an inverted statement cannot match — state.rs:1702-1705:
```
fn statement_names_action(statement: &::serde_json::Value, action: &str) -> bool {
    let Some(declared) = statement.get("Action") else {
        return false;
    };
```
and state.rs:1730-1734:
```
    match statement.get("Principal") {
        Some(::serde_json::Value::Object(map)) => map.values().any(is_everyone),
        Some(value) => is_everyone(value),
        None => false,
    }
```

The decision this feeds — services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/service.rs:586-598:
```
    let granted = store.endpoint_at(&region, &target).is_some_and(|endpoint| {
        endpoint.policy.as_ref().is_some_and(|attached| {
            crate::state::policy_allows_anonymous(
                &attached.policy,
                egress_action(req.method()),
...
    if !granted {
        return error_response(403, "AccessDenied", ACCESS_DENIED_MESSAGE);
    }
```

Test coverage: the ALLOW side is pinned — tests/all/rejection_rule_and_negative_paths.rs:1836-1849 pins `"a grant to everyone written with NotAction"` as 403, and :1820-1834 pins the `Condition`-qualified grant as 403. The DENY side is pinned only in the plain form (:1796-1814, `"a grant to everyone with a Deny of the same action"` -> 403); no test in the crate exercises a `Deny` written with `NotPrincipal`/`NotAction`/`NotResource`, so nothing pins the behaviour this issue reports.

## Steps to Reproduce

1. CreateChannelGroup `g`, CreateChannel `c` in it, CreateOriginEndpoint `e` under `c` (any container type with one manifest); note the group's `EgressDomain` and the endpoint's manifest `Url`.
2. Call PutOriginEndpointPolicy on `g/c/e` with `Policy` = `{"Statement":[{"Effect":"Allow","Principal":"*","Action":"mediapackagev2:GetObject"},{"Effect":"Deny","NotPrincipal":{"AWS":"arn:aws:iam::123456789012:role/cdn"},"Action":"mediapackagev2:GetObject"}]}` — i.e. allow everyone, then deny everyone who is not that one role (which includes an anonymous player).
3. GET the published manifest URL with no credential.
4. Observed: the Deny is skipped by `matching(statement, "Deny")` (no `Principal` member), the `Allow` grants, and `serve_egress` answers 404 `NotFound` / "No content is available at this origin endpoint yet" — i.e. the fetch was authorized.
5. Expected per the helper's own documented rule ("A `Deny` carrying one still denies, which is the closed side there too."): the fetch is refused 403 `AccessDenied`, the same way the `Allow` side of the same inverted-member list fails closed.
