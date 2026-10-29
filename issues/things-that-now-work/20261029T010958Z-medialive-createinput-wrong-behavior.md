Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 41c43c88bb67cbcbcf61b56c50a2a3e4a7862d36

# CreateInput replays a RequestId spent by CreatePartnerInput and answers 201 with that partner input, creating nothing

_Filed: 2026-10-29T01:09:58Z_
_Operation: CreateInput | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `41c43c88bb67cbcbcf61b56c50a2a3e4a7862d36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateInput` and `CreatePartnerInput` share ONE idempotency-token map (`state.input_tokens`, written by `record_input_idempotency_token` from both handlers). `CreatePartnerInput` therefore filters a token hit down to an input that is a partner of the input ITS path named, and its own comment states why. `CreateInput` applies no such filter: it accepts whatever input the token created and answers with it.

So a `CreateInput` whose `RequestId` was already spent on a `CreatePartnerInput` is answered 201 with THAT partner input — a different resource, carrying the primary input's `Type`, `Sources`, `Destinations`, `SecurityGroups` and `Name`, plus a `PartnerOf` relationship the caller never asked for — and the input the request actually described is never created and never stored. By the crate's own stated reasoning this "report[s] a resource the request did not ask to exist". The caller gets a success it can neither detect nor undo, and every later read of the id it was handed describes somebody else's partner input.

## Evidence

The guard CreatePartnerInput applies to the SHARED map — services/service-com-amazonaws-medialive-medialive/src/ops/create_partner_input.rs:166-189:
```
// A retried create (same `@idempotencyToken` `RequestId`) must return the
// partner input the first one created, not a second one. The token map is
// the input store's, shared with `CreateInput`, so a replay counts only
// when the input that token created is a partner OF THE INPUT THIS
// REQUEST NAMES: a token spent on some other create names an input this
// call was not asked for, and answering with it would report a resource
// the request did not ask to exist.
if let Some(existing) = input
    .request_id
    .as_deref()
    .and_then(|token| state.input_for_token(token))
    .filter(|existing| {
        state
            .inputs
            .get(*existing)
            .is_some_and(|stored| stored.partner_of.as_deref() == Some(&input.input_id))
    })
```

The same lookup with NO guard — services/service-com-amazonaws-medialive-medialive/src/ops/create_input.rs:203-215:
```
// A retried create (same `@idempotencyToken` `RequestId`) must return the
// input the first one created, not a second input.
if let Some(existing) = input
    .request_id
    .as_deref()
    .and_then(|token| state.input_for_token(token))
    .map(str::to_string)
{
    let created = state.input_description(&existing).ok_or_else(unavailable)?;
    return Ok(CreateInputOutput { input: Some(created) });
}
```

The map really is one map — services/service-com-amazonaws-medialive-medialive/src/state.rs:3432-3443:
```
/// Record `token` as having created `input_id`, under the same bounded,
/// evicting discipline as the channel tokens.
pub fn record_input_idempotency_token(&mut self, token: String, input_id: &str) {
    self.input_tokens
        .record(token, input_id, MAX_INPUT_IDEMPOTENCY_TOKENS);
}

/// The input a stored `RequestId` created, if that input still exists.
pub fn input_for_token(&self, request_id: &str) -> Option<&str> {
    let id = self.input_tokens.created(request_id)?;
    self.inputs.get(id).map(|input| input.id.as_str())
}
```
and both handlers write it: create_partner_input.rs:240 `state.record_input_idempotency_token(token, &id);` and create_input.rs:273 `state.record_input_idempotency_token(token, &id);`.

The partner record is NOT an input the request described — services/service-com-amazonaws-medialive-medialive/src/state.rs:5656-5672 `new_partner_input` builds it from `..primary.clone()` with `partner_of: Some(primary.id.clone())`.

Test coverage: tests/all/partner_input.rs and tests/all/input_lifecycle.rs exercise the two creates, but no suite presents one create's `RequestId` to the other.

## Steps to Reproduce

1. CreateInput { Type: "UDP_PUSH", Name: "primary", Destinations: [{ StreamName: "a" }], InputSecurityGroups: [<a created group id>] } -> 201, input id P
2. CreatePartnerInput { InputId: P, RequestId: "tok-1" } -> 201, partner input id Q (records tok-1 -> Q in state.input_tokens)
3. CreateInput { Type: "URL_PULL", Name: "brand-new", Sources: [{ Url: "http://example.com/a.m3u8" }], RequestId: "tok-1" }
4. Observe 201 whose body is input Q — the partner of P, with P's Type/Sources/Name — instead of a new URL_PULL input
5. ListInputs: only P and Q exist; the URL_PULL input the third call described was never created
