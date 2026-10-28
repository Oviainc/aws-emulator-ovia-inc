Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: e172822ed36ddbdf21d6296d81819842e39b78b2

# UntagResource answers ValidationException for an absent tagKeys, a code the crate states it must not answer

_Filed: 2026-10-28T20:32:12Z_
_Operation: UntagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `e172822ed36ddbdf21d6296d81819842e39b78b2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The tag surface of this crate deliberately answers its OWN error taxonomy (`BadRequestException` / `NotFoundException`) instead of the generic `ValidationException`, and the crate justifies that choice three times with the fact that `UntagResource` does not model `ValidationException` at all (its modelled error set is `InternalServerException` + `ResourceNotFoundException`).

But the code that implements the rule only recognises TWO tag-surface refusals: a violation on the `resourceArn` label, and a `MaxLength` violation on a tag-selection member (`tags` / `tagKeys`). Every OTHER violation falls through to the generic constraint envelope, which replaces the code with `VALIDATION_EXCEPTION` ("ValidationException").

`tagKeys` is `@required` and `@httpQuery`, so a `DELETE /tags/{resourceArn}` carrying no `tagKeys` query parameter produces a `ViolationKind::MissingRequiredMember` violation on the member path `tagKeys`. That is not `MaxLength`, so `tag_api_refusal` declines it and `refused()` answers the request with the error code `ValidationException` — exactly the code the crate's own rule, its handler doc and its test suite all state this operation must never answer because the model does not declare it. A client branching on `UntagResource`'s modelled errors sees an undeclared code for the most ordinary malformed request this operation has.

The path is not covered by any test: `tests/all/tag_selection_limit.rs` deliberately skips the zero-key row and notes it is "a different refusal" without pinning what that refusal is.

## Evidence

Rule (this crate, three statements of it):

* `src/service/validation.rs:14-18` — "**The tagging surface answers its own taxonomy.** The tag APIs (`/tags/{resourceArn}`) address their resource with the `resourceArn` label, and a value outside that label's modelled grammar is refused with `BadRequestException` — not the generic `ValidationException`, which `UntagResource` does not even model."
* `src/ops/untag_resource.rs:25-27` — "Neither code is in this operation's modelled error set (`ResourceNotFoundException`, `InternalServerException`), so both travel through `crate::scaffolded::Scaffolded`'s unmodeled channel."
* `tests/all/tag_selection_limit.rs:9-13` — "never with the generic `ValidationException`, which `UntagResource` does not model at all (`smithy_query(query=\"operation\",operation=\"UntagResource\")` lists only `InternalServerException` and `ResourceNotFoundException`)."

Code that breaks it:

* `src/service/validation.rs:385-387` — `if TAG_SELECTION_MEMBERS.contains(&path) && matches!(first.kind, ViolationKind::MaxLength(_)) { return Some(&TAG_LIMIT); }` ... `None` — only a `MaxLength` violation on `tags`/`tagKeys` takes the surface's own refusal.
* `src/service/validation.rs:396-406` — `pub(crate) fn refused(refusal: &Refusal) -> Option<Refused> { match refusal { Refusal::Invalid(violations) => match tag_api_refusal(violations) { Some(tag_refusal) => ..., None => Some(Refused { code: VALIDATION_EXCEPTION.to_string(), ...`
* `src/service/validation.rs:76` — `pub(crate) const VALIDATION_EXCEPTION: &str = "ValidationException";`
* `src/service/validation.rs:533` — the crate's own unit test drives `ViolationKind::MissingRequiredMember` through this same formatter, so that class does reach it.

Model (confirms the code is undeclared): `smithy_query(query="operation",operation="UntagResource")` reports `"tagKeys": {"target": "TagKeyList", "httpQuery": "tagKeys", "required": true, ...}` and `"errors": [InternalServerException (500), ResourceNotFoundException (400)]` — no `ValidationException`.

Test coverage gap: `tests/all/tag_selection_limit.rs:292-294` — "(`tagKeys` is `@required`, so a request naming none of them is a different refusal and not a row of this rule.)" — no suite asserts what that refusal is.

## Steps to Reproduce

1. Start the emulator and register a provider so a valid resource ARN exists (`POST /subscription/RegisterSubscriptionProvider` with `SubscriptionProviderSource=RedHat` and a well-formed `SecretArn`).
2. Send `DELETE /tags/arn:aws:license-manager-linux-subscriptions:us-east-1:<account>:subscription-provider/<id>` with NO `tagKeys` query parameter (the harness helper `untag(h, arn, &[])` builds exactly this request: `tests/all/harness.rs:325-329` joins one `tagKeys=` component per key, so an empty key list emits none).
3. Observe the response error code: `ValidationException` (HTTP 400) from the generic envelope in `src/service/validation.rs:403-406`.
4. Compare with the rule the crate states for this surface (`src/service/validation.rs:14-18`, `src/ops/untag_resource.rs:25-27`, `tests/all/tag_selection_limit.rs:9-13`): `UntagResource` must not answer `ValidationException`, since its model declares only `InternalServerException` and `ResourceNotFoundException`, and a refusal on this surface is stated with the surface's own `BadRequestException`.
