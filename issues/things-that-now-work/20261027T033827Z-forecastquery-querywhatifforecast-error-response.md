Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: ce187dd52ad3609a091254aaff4a955715a54b39

# `Invalid ARN for what-if-forecast:` echoes a 257..300-character WhatIfForecastArn unelided, unlike the boundary path that words the same rule

_Filed: 2026-10-27T03:38:27Z_
_Operation: QueryWhatIfForecast | Class: error-response_

> _Note: this issue was identified against the code base at commit `ce187dd52ad3609a091254aaff4a955715a54b39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ResourceKind::invalid_arn_message` is the ONE sentence this crate words an unreadable resource ARN with, and it is emitted from TWO places: the boundary refusal path (`EmulatedService::override_refusal`, which interpolates `Violation::value`) and the handler path (`QueryWhatIfForecast::handle`, which interpolates `input.what_if_forecast_arn` directly). The framework has ALREADY elided `Violation::value` at 256 bytes, while the handler path interpolates the raw request value and never calls this module's own `echoed()` ceiling.

`WhatIfForecastArn` targets `LongArn` (modelled `@length` max 300), so values of 257..300 characters pass the boundary validator and reach the handler. The consequence is that one and the same message rule produces two different texts for inputs that differ by one character:

* a 300-character ARN that satisfies the modelled pattern but names a `forecast/...` resource instead of a `what-if-forecast/...` one -> handler path -> `Invalid ARN for what-if-forecast: <all 300 characters>`;
* a 301-character value of the same form (`MaxLength` violation) or a 300-character value outside the pattern -> boundary path -> `Invalid ARN for what-if-forecast: <first 256 chars>... [301 bytes]`.

This contradicts (a) `src/service.rs`, which states the two paths never differ in what the caller is told, (b) the module's own echo-ceiling rule, which `pattern_clause`, `map_keys_clause`, `QueryRejection::UnknownDimension` and `QueryRejection::MalformedDate` all go through and which the comment calls "the module's one echo ceiling, like every other request-derived value a message interpolates", and (c) the expectation the existing test helper already encodes for this very message (`echoed == sent` only when `sent.len() <= MAX_ECHOED_VALUE`, otherwise the elision marker) — a rule no test row currently exercises on the handler path, because every `wrong_kind` row is far shorter than 256 characters.

QueryForecast cannot show the defect: its `ForecastArn` targets `Arn` (max 256), so a handler-path value can never exceed the echo ceiling. The helper is shared, but only `QueryWhatIfForecast` can reach the divergence.

## Evidence

The handler path interpolates the raw request value, with no `echoed()` — services/.../src/state.rs:198-200:
```rust
    pub fn invalid_arn_message(&self, arn: &str) -> String {
        format!("Invalid ARN for {}: {}", self.resource_type, arn)
    }
```
called with the submitted value at services/.../src/ops/query_what_if_forecast.rs:208-218:
```rust
        if !crate::state::WHAT_IF_FORECAST.names(&input.what_if_forecast_arn) {
            return Err(QueryWhatIfForecastErrorModeled::InvalidInputException(
                InvalidInputException {
                    message: Some(
                        crate::state::WHAT_IF_FORECAST
                            .invalid_arn_message(&input.what_if_forecast_arn),
```

The boundary path words the SAME sentence from an already-elided value — services/.../src/service.rs:225-230:
```rust
                resource_arn_violation(violations).map(|(violation, kind)| Refused {
                    code: INVALID_INPUT_EXCEPTION.to_string(),
                    message: kind.invalid_arn_message(violation.value.as_deref().unwrap_or_default()),
```
and vendor/smithy-schema-server/src/constraints.rs:64-66 says that value is pre-elided: "ALREADY elided to `MAX_ECHOED_VALUE`: a caller controls its length, and [`Violation::message`] interpolates it as-is, so a value stored unelided would amplify a request into an arbitrarily long response."

The claim the code breaks — services/.../src/service.rs:216-218: "The two paths differ only in WHERE the value is read, never in what the caller is told, so a client cannot tell a boundary refusal from a handler rejection."

The helper this path skips, and the crate's own statement that EVERY other request-derived echo goes through it — services/.../src/state.rs:795-799: "Ceiling on a value as rendered INTO a message, mirroring the one `aws_smithy_schema_server::constraints` applies to every value it echoes: a caller controls the length of what it sends, and a clause interpolates it, so an unelided echo amplifies a request into an arbitrarily long response." and services/.../src/state.rs:1034-1036: "// Through the module's one echo ceiling, like every other request-derived value a message interpolates." followed by `echoed(key),`. Sibling users: state.rs:845 (`value: Some(echoed(value).into_owned())`), state.rs:867 (`echoed(&rendered_map(map))`), state.rs:1059 (`echoed(value)`).

The member is modelled long enough to reach it — smithy_query(query="operation"): `"WhatIfForecastArn": {"target": "LongArn", "required": true, "length": {"min": 0, "max": 300}, "pattern": "^arn:([a-z\\d-]+):forecast:.*:.*:.+$"}`.

Test coverage: services/.../tests/all/invalid_arn_message.rs:148-161 already encodes the rule for this exact message —
```rust
    if sent.len() <= MAX_ECHOED_VALUE {
        assert_eq!(echoed, sent, ...);
    } else {
        // Over the echo bound, so the value is quoted back as its leading bytes plus the
        // validation layer's own elision marker.
        let expected = format!("{}... [{} bytes]", &sent[..MAX_ECHOED_VALUE], sent.len());
```
but the only row over the bound is "one character past the member's maximum length" (invalid_arn_message.rs:326-330), which takes the BOUNDARY path; every `wrong_kind` row (lines 212-230) is under 100 characters, so the handler path above 256 characters is unpinned. The 404 wording is a different function (`not_found_message`) and is pinned separately by resource_not_found_message.rs:296-301, so applying the ceiling here changes no existing assertion.

## Steps to Reproduce

1. Build the harness from tests/all/invalid_arn_message.rs (`harness()`), with nothing seeded.
2. Construct a 300-character value whose prefix is `arn:aws:forecast:us-east-1:000000000000:forecast/` (49 characters) padded with 251 `x` characters. It satisfies `LongArn`'s @pattern and its @length max of 300, so the boundary validator passes it, and `WHAT_IF_FORECAST.names()` rejects it because the resource type is `forecast`, not `what-if-forecast`.
3. POST it as `{"WhatIfForecastArn": <that 300-char value>, "Filters": {"item_id": "client_21"}}` with `x-amz-target: AmazonForecastRuntime.QueryWhatIfForecast`.
4. Observe HTTP 400 `InvalidInputException` with `Message = "Invalid ARN for what-if-forecast: " + all 300 characters` — the whole caller-controlled value, unelided.
5. Repeat with one more `x` (301 characters, one past the modelled maximum) and observe the same sentence with the value elided: `Invalid ARN for what-if-forecast: <first 256 chars>... [301 bytes]`.
6. Compare the two: the same documented refusal, same operation, reports the submitted value under two different rules — and the 300-character row would fail `assert_invalid_arn` (invalid_arn_message.rs:148-161) if it were added to the `wrong_kind` table.
