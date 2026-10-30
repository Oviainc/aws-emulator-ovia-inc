Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 75063c57edf72fb8c72d4cb78542f16ad5c4fd3a

# The platform-not-found refusal echoes an unbounded platformId, the ceiling the sibling refusal in the same handler applies

_Filed: 2026-10-29T12:35:10Z_
_Operation: GetRevocationStatus | Class: error-response_

> _Note: this issue was identified against the code base at commit `75063c57edf72fb8c72d4cb78542f16ad5c4fd3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetRevocationStatus` judges the `platformId` query member twice, in this order: first against the published character class (`crate::state::platform_id_pattern_refusal`), then against the published platform catalog (`crate::state::platform_not_found_message`). The FIRST refusal deliberately elides the echoed request value at a 16 KiB ceiling through `crate::state::stated_value`, because — in the crate's own words — "A value with no ceiling at all would let one oversized query parameter become an equally large error body". The SECOND refusal, on the very next check of the very same member, interpolates the caller's bytes whole: `format!("Platform with id {platform_id} does not exist")`.

Nothing bounds that value. In this service's model `PlatformId` is a bare string — `{"shape_id": "com.amazonaws.signer#PlatformId", "type": "string"}`, no `@length` and no `@pattern` — so the boundary validator imposes no size, and a value made only of characters the class admits (ASCII letters, digits, `_`, `-`) passes the first check at any length and reaches the second. A `GET /revocations?platformId=<1 MiB of 'a'>&…` is therefore answered with a 400 whose `message` carries that whole megabyte back: one oversized query parameter becomes an equally large error body, which is exactly the outcome the crate states its ceiling exists to prevent. The same unelided-echo path is reached from `GetSigningPlatform` and `PutSigningProfile`, which resolve the same unconstrained member through the same message builder, and the tagging slice has the same hole for the unconstrained `resourceArn` label (`TaggingArnProblem::message`).

The rule is already written and already used once in this crate; one path that needs it skips it, so the two refusals of one member disagree about whether the request's bytes are bounded before they are quoted.

## Evidence

The rule the crate states, services/service-com-amazonaws-signer-wallabyservice/src/state.rs:1480-1490:
```
/// The longest rejected value a clause states in full.
///
/// The value between the quotes is the caller's OWN bytes — AWS builds the
/// clause from the request, so a caller comparing the echo against what it sent
/// must see it whole. A value with no ceiling at all would let one oversized
/// query parameter become an equally large error body, so the ceiling is set
/// where the shared refusal-wording module sets the largest echo it restores
/// into a clause (16 KiB) …
const MAX_STATED_VALUE: usize = 16 * 1024;
```
Where it IS applied — state.rs:1533-1539 (inside `platform_id_pattern_refusal`):
```
        kind: ::aws_smithy_schema_server::ViolationKind::Pattern(
            REVOCATION_PLATFORM_ID_PATTERN.to_string(),
        ),
        value: Some(stated_value(value)),
```
Where it is SKIPPED — state.rs:1448-1450:
```
pub fn platform_not_found_message(platform_id: &str) -> String {
    format!("Platform with id {platform_id} does not exist")
}
```
Both refusals, adjacent, on the same member — services/service-com-amazonaws-signer-wallabyservice/src/ops/get_revocation_status.rs:194-198 then 217-221:
```
        if let Some(message) =
            crate::state::platform_id_pattern_refusal(PLATFORM_ID_QUERY, &input.platform_id)
        {
            return Err(validation(message));
        }
…
        if crate::state::signing_platform(&input.platform_id).is_none() {
            return Err(validation(crate::state::platform_not_found_message(
                &input.platform_id,
            )));
        }
```
The member carries no size bound to lean on — `smithy_query(query="shape", shape="PlatformId")` returns `{"shape_id": "com.amazonaws.signer#PlatformId", "type": "string"}` (no length, no pattern), and `smithy_query(query="operation", operation="GetRevocationStatus")` binds it as `"platformId": {"target": "PlatformId", "httpQuery": "platformId", "required": true}`.

The same hole for the equally unconstrained tagging label (`"resourceArn": {"target": "String", "httpLabel": true, "required": true}`) — state.rs:1030-1038:
```
    pub fn message(self, resource_arn: &str) -> String {
        match self {
            Self::Malformed => {
                format!("{resource_arn} is not a valid signing profile resource ARN")
```

Test coverage, and why a fix changes no existing expectation: tests/all/platform_id_character_class.rs:273-294 pins the elision for the pattern refusal (`const BOUND: usize = 16 * 1024;` … `pattern_refusal(&format!("{}... [{} bytes]", &value[..BOUND], value.len()))`), while the not-found expectation is asserted only for values far inside that ceiling — tests/all/platform_id_character_class.rs:402 (`("a long in-class value", "a".repeat(200))`) compared against `not_found_refusal(value)` (full echo) at line 417-423. Routing the not-found echo through `stated_value` leaves every one of those rows byte-identical.

## Steps to Reproduce

1. Boot the emulator with this service mounted.
2. Issue `GET /revocations?signatureTimestamp=1700000000&platformId=<1048576 'a' characters>&profileVersionArn=arn:aws:signer:us-east-1:000000000000:/signing-profiles/P/0000000001&jobArn=arn:aws:signer:us-east-1:000000000000:/signing-jobs/x&certificateHashes=ab` (every 'a' is inside the published character class, so the pattern check passes and the catalog check refuses).
3. Observe HTTP 400 `ValidationException` whose `message` is `Platform with id aaaa…(1 MiB)… does not exist` — the error body is as large as the request parameter.
4. Repeat with `platformId=<1048576 'a' characters>!` (one character outside the class, same length). Observe the pattern refusal instead, whose echoed value stops at 16 KiB and ends with `... [1048577 bytes]` — the ceiling `crate::state::stated_value` applies.
5. Compare the two answers: the same member, refused one step apart, is bounded on one path and unbounded on the other.
