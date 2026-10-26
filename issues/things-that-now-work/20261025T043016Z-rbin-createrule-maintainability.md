Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: dbe9ca7a8a80909c811e91f1b58fc3cbb14634ed

# Compose the ServiceQuotaExceededException bodies in crate::state like every other modelled refusal

_Filed: 2026-10-25T04:30:16Z_
_Operation: CreateRule | Class: maintainability_

> _Note: this issue was identified against the code base at commit `dbe9ca7a8a80909c811e91f1b58fc3cbb14634ed`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state` declares itself the single home of this service's modelled error bodies, and it already hosts the reason constant (`SERVICE_QUOTA_REASON_EXCEEDED`) and one quota body (`resource_tag_quota_exceeded`). Two handlers nevertheless compose `ServiceQuotaExceededException` inline — `CreateRule` twice (tags-per-rule and rules-per-Region) and `TagResource` once (tags-per-rule) — each re-typing the reason enum as a bare `"SERVICE_QUOTA_EXCEEDED"` literal instead of the constant that exists for it. The tags-per-rule refusal is therefore built in two places, so the two copies can drift in reason or envelope while the handlers keep claiming the same refusal class. Moving the three bodies behind constructors in `crate::state` (keeping the existing wording of each message verbatim) restores the rule the module comment states; no response and no existing assertion changes.

## Evidence

The rule the crate states, `services/service-com-amazonaws-rbin-amazonrecyclebin/src/state.rs:337-343`:
```
// ---------------------------------------------------------------------------
// The modelled error BODIES this service answers with.
//
// Every one of them is composed here, from the request and from the rule's own state,
// so one refusal class cannot be worded two ways by two handlers — and so the gate in
// `crate::service`, which answers a constraint refusal before any handler runs, states
// it exactly as a handler-raised refusal of the same class does.
```
and the facilities it already provides: `src/state.rs:78` `pub const SERVICE_QUOTA_REASON_EXCEEDED: &str = "SERVICE_QUOTA_EXCEEDED";`, `src/state.rs:96` `pub fn resource_tag_quota_exceeded() -> ServiceQuotaExceededException {` (which does use the constant, `src/state.rs:102`: `reason: Some(SERVICE_QUOTA_REASON_EXCEEDED.to_string()),`).

Copy 1 — `src/ops/create_rule.rs:236-247`:
```
            return Err(CreateRuleErrorModeled::ServiceQuotaExceededException(
                ServiceQuotaExceededException {
                    message: Some(format!(
                        "The request would create a retention rule with {} tags, \
                         exceeding the quota of {} tags per resource",
                        tags.len(),
                        crate::state::MAX_TAGS_PER_RULE
                    )),
                    reason: Some("SERVICE_QUOTA_EXCEEDED".to_string()),
```
Copy 2 — `src/ops/create_rule.rs:255-262` (same inline body for the rules-per-Region quota, `reason: Some("SERVICE_QUOTA_EXCEEDED".to_string()),` at line 261).
Copy 3 — `src/ops/tag_resource.rs:170-178`:
```
            return Err(TagResourceErrorModeled::ServiceQuotaExceededException(
                ServiceQuotaExceededException {
                    message: Some(format!(
                        "The request would leave the retention rule with {projected} tags, \
                         exceeding the quota of {} tags per resource",
                        crate::state::MAX_TAGS_PER_RULE
                    )),
                    reason: Some("SERVICE_QUOTA_EXCEEDED".to_string()),
```

Tests that pin these bodies and must keep passing unchanged: `tests/all/resource_tags.rs:663` and `:750` (`Some("SERVICE_QUOTA_EXCEEDED")` for the TagResource and CreateRule tag-quota refusals), `tests/all/retention_rule_lifecycle.rs:849` (rules-per-Region quota), `tests/all/retention_rule_fidelity.rs:673`, `tests/all/resource_tag_pair_quota.rs:170`.

## Steps to Reproduce

1. Open services/service-com-amazonaws-rbin-amazonrecyclebin/src/state.rs:337-343 and read the stated rule that every modelled error body is composed in that module.
2. grep -n 'SERVICE_QUOTA_EXCEEDED' src/ — three bare literals appear in src/ops/create_rule.rs:244,261 and src/ops/tag_resource.rs:177 while src/state.rs:78 defines SERVICE_QUOTA_REASON_EXCEEDED for exactly that value.
3. Compare src/ops/create_rule.rs:236-247 with src/ops/tag_resource.rs:170-180: the tags-per-rule refusal body (message envelope plus reason) is built twice outside crate::state.
4. Confirm the existing assertions on those bodies (tests/all/resource_tags.rs:663,750; tests/all/retention_rule_lifecycle.rs:849) only read Reason and the message text, so hoisting the composition leaves every response and test unchanged.
