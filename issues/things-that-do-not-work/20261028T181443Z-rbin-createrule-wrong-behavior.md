Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: afab077872ee6c32a9626f154d3b33fea96b8a18

# CreateRule mints a RuleArn from the raw credential-scope region, so the ARN it returns can be one this crate's own tag operations refuse as malformed

_Filed: 2026-10-28T18:14:43Z_
_Operation: CreateRule | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `afab077872ee6c32a9626f154d3b33fea96b8a18`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateRule` builds the rule's `RuleArn` — which is also the key of the store's `arn_index`, the ONLY way the three ARN-addressed operations reach a rule — by interpolating `crate::state::request_region` verbatim. `request_region` returns the SigV4 credential scope's region exactly as the caller wrote it in the `Authorization` header, with no normalisation and no check that it is a region string at all (the parser that produces it only rejects an EMPTY field).

The modelled `RuleArn` restricts the region segment to `[a-z\-0-9]{0,63}`, and this crate's own input-validation gate (`src/service.rs`) enforces that pattern on `TagResource$ResourceArn`, `UntagResource$ResourceArn` and `ListTagsForResource$ResourceArn` before any handler runs. So when a request is signed for a credential scope whose region carries any character outside that class — `US-EAST-1`, `us_east_1`, `local`, a 70-character string — `CreateRule` answers HTTP 201 with a `RuleArn` that violates the member's own modelled pattern, persists that ARN as the rule's `arn_index` key, and the three tag operations then refuse that very ARN with HTTP 400 `ValidationException`. The rule exists and is still reachable by `Identifier` (GetRule / UpdateRule / LockRule / DeleteRule), but it is PERMANENTLY untaggable: `TagResource`, `UntagResource` and `ListTagsForResource` can never be served for it, because the only string that resolves it through `arn_index` is a string the gate rejects.

The two sides contradict each other inside one crate: the create door writes an ARN the tag doors are built to reject, and `state.rs`'s own `arn_index` doc claims "the ARN-addressed tag operations resolve a rule without scanning the map" — for such a rule they cannot resolve it at all.

## Evidence

The region is taken verbatim from the caller's header — `services/service-com-amazonaws-rbin-amazonrecyclebin/src/state.rs:1193-1199`:
```
/// The region this request is for: the SigV4 credential scope's region, which is
/// what the SDK signs an endpoint's region into, else [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}
```
and that producer applies no character rule, only non-emptiness — `vendor/server/src/lib.rs:1800-1808`:
```
    if access_key_id.is_empty() || date.is_empty() || region.is_empty() || service.is_empty() {
        return None;
    }
    Some(SigV4Scope { ... region: region.to_string(), ... })
```

It is interpolated straight into the stored ARN — `src/ops/create_rule.rs:315`:
```
        let rule_arn = ::engine::emulator_arn("rbin", &region, &format!("rule/{identifier}"));
```
with `vendor/server/src/lib.rs:162-164`:
```
pub fn emulator_arn(service: &str, region: &str, resource: &str) -> String {
    format!("arn:aws:{service}:{region}:{EMULATOR_ACCOUNT_ID}:{resource}")
}
```
and that string becomes the rule's only ARN handle — `src/state.rs:1033-1037`:
```
    pub fn insert_rule(&mut self, rule: Rule) {
        self.arn_index
            .insert(rule.rule_arn.clone(), rule.identifier.clone());
```

What the model allows that string to be — `smithy_query(query="operation", operation="TagResource")`: `"ResourceArn": {"target": "RuleArn", "httpLabel": true, "required": true, "length": {"min": 0, "max": 1011}, "pattern": "^arn:\\S+:rbin:[a-z\\-0-9]{0,63}:[0-9]{12}:rule/[0-9a-zA-Z]{11}{0,1011}$"}` — the region segment is `[a-z\-0-9]{0,63}`, so `US-EAST-1` or `us_east_1` cannot appear in a `RuleArn`. The same pattern is declared on `CreateRuleResponse$RuleArn` (`services/service-com-amazonaws-rbin-amazonrecyclebin/OPERATIONS.md:30`).

This crate enforces that pattern on the tag operations before any handler runs — `src/service.rs:214-224`:
```
fn refuse_invalid_input<Op: Operation>(svc: &dyn Service, req: &RtRequest) -> Option<RtResponse> {
    let protocol = svc.definition().protocol();
    let violations = violations_of::<Op>(protocol, req);
    let error = crate::state::constraint_violations(&violations)?;
```
and the existing tests pin exactly that refusal for an ARN outside the pattern — `tests/all/error_response_bodies.rs:325-347`:
```
        ConstraintRow {
            case: "TagResource: a resource ARN that is not an ARN at all",
            ...
            member_path: "resourceArn",
            requirement: Requirement::Exact(format!(
                "Member must satisfy regular expression pattern: {RULE_ARN_PATTERN}"
            )),
```
(the `ListTagsForResource` row immediately after it is the same assertion).

The claim the pairing breaks — `src/state.rs:1006-1009`:
```
    /// `RuleArn` -> `Identifier`, so the ARN-addressed tag operations resolve a rule
    /// without scanning the map. An ARN carries its rule's Region, so an entry is
    /// still reached through a Region-scoped resolver.
```

Test coverage: every ARN test signs for a canonical lowercase region (`tests/all/retention_rule_lifecycle.rs::authorization`, `tests/all/tag_arn_label_encoding.rs`), and `tests/all/regional_rule_isolation.rs` only varies the region across `us-east-1`/`us-west-2`-style values, so no test exercises a credential-scope region outside `[a-z\-0-9]`.

## Steps to Reproduce

1. POST /rules with `Authorization: AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/US-EAST-1/rbin/aws4_request, SignedHeaders=host, Signature=x` and body {"ResourceType":"EBS_SNAPSHOT","RetentionPeriod":{"RetentionPeriodValue":7,"RetentionPeriodUnit":"DAYS"},"Tags":[{"Key":"env","Value":"dev"}]} — HTTP 201. Note the returned RuleArn: `arn:aws:rbin:US-EAST-1:000000000000:rule/<Identifier>`.
2. Compare it with the modelled RuleArn pattern `^arn:\S+:rbin:[a-z\-0-9]{0,63}:[0-9]{12}:rule/[0-9a-zA-Z]{11}{0,1011}$` — the uppercase region segment does not satisfy it, so CreateRule emitted a value for a member the model says cannot hold it.
3. GET /tags/{that RuleArn, percent-encoded as the tests encode it} signed for the same `US-EAST-1` scope. Expected: HTTP 200 with the rule's `env=dev` tag (the ARN is the one this service just handed out). Actual: HTTP 400 ValidationException — "Value at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:\S+:rbin:[a-z\-0-9]{0,63}:[0-9]{12}:rule/[0-9a-zA-Z]{11}{0,1011}" — the same refusal tests/all/error_response_bodies.rs:325-347 pins for "not-an-arn".
4. POST /tags/{same ARN} with {"Tags":[...]} and DELETE /tags/{same ARN}?tagKeys=env — both HTTP 400 for the same reason, so the rule can never be tagged, untagged or have its tags listed.
5. GET /rules/{Identifier} signed for the same scope — HTTP 200, proving the rule exists and is reachable by identifier: only every ARN-addressed operation is permanently unreachable, because `arn_index` is keyed on a string the gate rejects.
