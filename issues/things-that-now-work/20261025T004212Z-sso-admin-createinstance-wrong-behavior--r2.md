Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5b028d5a37462f071f2cc1a7c0f151a69190cfd2

# CreateInstance's ClientToken replay check accepts a retry whose Tags differ, turning the modelled ConflictException into a 200

_Filed: 2026-10-25T00:42:12Z_
_Operation: CreateInstance | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5b028d5a37462f071f2cc1a7c0f151a69190cfd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateInstance` decides whether a repeated `ClientToken` is a genuine retry or "the same token carrying a different request" with the local helper `tags_match`, whose own doc says it answers "Whether two tag sets are the same set of (key, value) pairs, order disregarded". The code does not implement that: it compares the two LIST LENGTHS and then checks only that every REQUESTED pair occurs somewhere in the stored list. One-directional containment plus equal length is not multiset (or set) equality, so a request whose `Tags` list repeats one entry is judged identical to a stored list that contains a tag the request never sent.

Concretely, with the instance created under `Tags: [{Owner,platform},{Tier,test}]`, a second call carrying the SAME `ClientToken` and `Tags: [{Owner,platform},{Owner,platform}]` has `stored.len() == requested.len() == 2` and every requested pair is present in the stored list, so `same_request` is `true`: the handler answers HTTP 200 with the first call's `InstanceArn` instead of the modelled `ConflictException` the crate keeps `CLIENT_TOKEN_MISMATCH_MESSAGE` for. A refusal becomes a success, and the caller is told its differing request was accepted.

The two sibling creates with the same `@idempotencyToken` rule do not have this hole: both compute a stored fingerprint over the tag pairs SORTED and length-prefixed (`state::application_fingerprint`, `state::trusted_token_issuer_fingerprint`), which cannot confuse two different tag lists.

## Evidence

The helper and its contradicted doc — `services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/create_instance.rs:295-307`:

```rust
/// Whether two tag sets are the same set of (key, value) pairs, order disregarded — the
/// comparison the `ClientToken` idempotency check needs over `Tags`, which the model
/// carries as a list but AWS treats as a set keyed by `Key`.
fn tags_match(stored: &[Tag], requested: &[Tag]) -> bool {
    if stored.len() != requested.len() {
        return false;
    }
    requested.iter().all(|wanted| {
        stored
            .iter()
            .any(|tag| tag.key == wanted.key && tag.value == wanted.value)
    })
}
```

Its one caller, which turns that answer into a success — `create_instance.rs:224-239`:

```rust
        if let Some(client_token) = input.client_token.as_deref() {
            if let Some(existing) = store.instance_by_client_token(client_token) {
                let same_request = existing.name == input.name
                    && tags_match(&existing.tags, input.tags.as_deref().unwrap_or(&[]));
                if !same_request {
                    return Err(CreateInstanceErrorModeled::ConflictException(... CLIENT_TOKEN_MISMATCH_MESSAGE ...)
                    .into());
                }
                return Ok(CreateInstanceOutput { instance_arn: Some(existing.instance_arn.clone()) });
```

The rule the refusal exists for — `src/state.rs:205-214`: "The refusal a repeated `ClientToken` earns when the rest of the `CreateInstance` request differs from the call that token belongs to … The rule is the `ClientToken` member's own documentation: 'Passing the same value to a later call to an operation requires that you also pass the same value for all other parameters'."

The siblings that compare correctly — `src/state.rs:2716-2724` (`application_fingerprint`) and `src/state.rs:2755-2763` (`trusted_token_issuer_fingerprint`): `let mut sorted: Vec<(&str, &str)> = tags.iter().map(...).collect(); sorted.sort_unstable(); for (key, value) in sorted { parts.push(key.to_string()); parts.push(value.to_string()); }` over `request_fingerprint`, which length-prefixes every part (`state.rs:2767-2778`).

Test coverage stops short of this input: `tests/all/instance_create.rs:394-425` drives only "the request verbatim", "the same tags in another order" (both 200) and "the same token, fewer tags" (a LENGTH difference, so ConflictException). No row sends an equal-length tag list with a repeated entry, so the defect is uncovered.

## Steps to Reproduce

1. Start the emulator and address the ssoadmin (`SWBExternalService`) surface in an account that holds no instance.
2. `CreateInstance` with `{"ClientToken":"tok-1","Name":"Idempotent","Tags":[{"Key":"Owner","Value":"platform"},{"Key":"Tier","Value":"test"}]}` -> 200, note the `InstanceArn`.
3. `CreateInstance` again with `{"ClientToken":"tok-1","Name":"Idempotent","Tags":[{"Key":"Owner","Value":"platform"},{"Key":"Owner","Value":"platform"}]}` — the same token carrying a DIFFERENT parameter set (the stored instance carries `Tier=test`, this request does not).
4. Observe HTTP 200 echoing the first call's `InstanceArn`. Expected, by the handler's own rule, the modelled `ConflictException` carrying `state::CLIENT_TOKEN_MISMATCH_MESSAGE` (the answer the "fewer tags" row already gets).
5. For contrast, run the same replay-with-different-tags against `CreateApplication` or `CreateTrustedTokenIssuer`, whose fingerprint comparison refuses it with `ConflictException`.
