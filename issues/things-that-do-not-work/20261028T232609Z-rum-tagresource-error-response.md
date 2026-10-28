Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: badb62e3f908ba62c49b0461176e1662666c01f4

# A tag-key refusal on the MAP form states only the first offending key, where the sibling list form states every one and the crate's own wording rule demands one clause per failure

_Filed: 2026-10-28T23:26:09Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `badb62e3f908ba62c49b0461176e1662666c01f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both halves of this service's tagging plane apply the same constraints — the ones the model declares on `TagMap`'s key shape `TagKey` — and both do it in the handler because the boundary walk never validates a map key and cannot compile that shape's leading negative lookahead. But the two halves report a multi-key violation differently.

`TagResource` refuses through `crate::state::tag_keys_refusal`, which is a `find_map` over the sorted keys: it stops at the FIRST key outside `TagKey` and envelopes that ONE violation, so the body reads `1 validation error detected: …` however many keys of the request are bad. The sibling removal path `crate::state::tag_key_list_refusal` collects a violation for EVERY offending key and envelopes them all, and its own doc states the reason for doing so.

That contradicts the single rule this crate states for every constraint refusal it words: the envelope carries "one clause per failing (member, requirement)". Two distinct bad keys in one `Tags` map are two distinct failing (member, requirement) pairs — `tag_keys_refusal` builds a DIFFERENT path per key (`tags.<key>.member`) — so a request with two bad keys has two clauses to state and states one. The caller then has to fix one key, retry, and be refused again for the next, one round trip per bad key, while the very same set of keys spelled as an `UntagResource` removal is told about all of them in one answer.

The same helper is what `CreateAppMonitor` seeds a monitor's tag map through, so a create carrying two bad keys is reported the same incomplete way.

## Evidence

The crate's ONE wording rule for a constraint refusal — `services/service-com-amazonaws-rum-rum/src/state/validation.rs:48-50`:
```
//! 1. the envelope is AWS's `N validation error(s) detected: ` over one clause per failing
//!    (member, requirement), joined with `; `, in the order the walk found them;
```

The MAP form, which stops at the first offender — `services/service-com-amazonaws-rum-rum/src/state.rs:2060-2065`:
```
pub fn tag_keys_refusal<'a>(keys: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let mut keys: Vec<&str> = keys.into_iter().collect();
    keys.sort_unstable();
    keys.into_iter()
        .find_map(|key| tag_key_violation(&format!("tags.{key}.member"), key))
}
```
and the single-violation envelope it builds, `services/service-com-amazonaws-rum-rum/src/state.rs:2040-2047`:
```
fn tag_key_violation(path: &str, key: &str) -> Option<String> {
    let violation = validation::string_violation(validation::ConstrainedString::TagKey, path, key)?;
    // `envelope` answers `None` only for an EMPTY violation list, which the one-element
    // slice below can never be. ...
    let clause = ::aws_smithy_schema_server::envelope(::std::slice::from_ref(&violation));
```

The LIST form in the same module, which states them all — `services/service-com-amazonaws-rum-rum/src/state.rs:2084-2098`:
```
/// EVERY offending key is stated,
/// because the walk accumulates one violation per element rather than stopping at the first
/// — so a caller is told about each key it has to fix.
pub fn tag_key_list_refusal<'a>(keys: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let violations: Vec<::aws_smithy_schema_server::constraints::Violation> = keys
        .into_iter()
        .enumerate()
        .filter_map(|(position, key)| { ... })
        .collect();
```
and the note that the two apply the SAME constraints, `services/service-com-amazonaws-rum-rum/src/state.rs:2075-2077`:
```
/// The constraints are the same ones [`tag_keys_refusal`] applies — they are declared once,
/// on the shape every tag key of this service targets, so they hold for a key a request
/// names as well as for one it writes
```

The two handlers that pick the two helpers — `services/service-com-amazonaws-rum-rum/src/ops/tag_resource.rs:315-317`:
```
        if let Some(message) = tag_keys_refusal(requested.iter().map(|(key, _)| key.as_str())) {
            return Err(TagResourceError::BadRequest(ValidationException { message }));
        }
```
and `services/service-com-amazonaws-rum-rum/src/ops/untag_resource.rs:375-379`:
```
        if let Some(message) =
            crate::state::tag_key_list_refusal(input.tag_keys.iter().map(String::as_str))
        {
            return Err(UntagResourceError::BadRequest(ValidationException { message }));
        }
```
(`CreateAppMonitor` takes the map form too — `services/service-com-amazonaws-rum-rum/src/ops/create_app_monitor.rs:283-288`.)

Test coverage: the removal side PINS the all-offenders behaviour — `services/service-com-amazonaws-rum-rum/tests/all/resource_tagging.rs:597-607` is the row `"two keys outside the pattern are both stated"` with `offending: vec![0, 1]`, asserted clause by clause at `:552-563`. The write side never sends more than one bad key: `tests/all/resource_tagging.rs:491-494` loops `for key in REJECTED_TAG_KEYS` and builds `map_of(&[(key, "value"), ("Valid", "value")])` — one rejected key beside an admitted one — and `tests/all/create_app_monitor_tags.rs` drives one `KeyCase` per request (`:134-144`). No test in the crate asserts a `tags.<key>.member` clause or a clause COUNT for a tag map, so nothing pins the first-offender-only answer with a stated reason.

## Steps to Reproduce

1. CreateAppMonitor(Name="m", Domain="example.com") — 200
2. POST /tags/<percent-encoded m ARN> with {"Tags":{"aws:owner":"x","key with space":"y"}} — both keys are outside the modelled `TagKey` (the reserved `aws:` prefix the pattern's lookahead excludes, and a character outside its class). Observed: HTTP 400 whose body states ONE clause — `1 validation error detected: Value 'aws:owner' at 'tags.aws:owner.member' failed to satisfy constraint: …` — because `tag_keys_refusal` is a `find_map` over the sorted keys. Expected: a clause for each failing (member, requirement), as the crate's rule 1 states, i.e. both `tags.aws:owner.member` and `tags.key with space.member`
3. DELETE /tags/<same ARN>?tagKeys=aws:owner&tagKeys=key%20with%20space — observed: HTTP 400 stating BOTH clauses (pinned by tests/all/resource_tagging.rs:597-607), so the same two keys get a complete answer on the removal path and an incomplete one on the write path
4. Fix the first key and repeat step 2 with {"Tags":{"Valid":"x","key with space":"y"}} — refused again, one round trip per bad key
