Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 8ff612b8d27f8140aa4881a7fd23caef0de9fa96

# Minted invocationArn can exceed the modelled @length(1..128), making the job's status permanently unreadable

_Filed: 2026-10-28T19:19:15Z_
_Operation: InvokeDataAutomationAsync | Class: validation_

> _Note: this issue was identified against the code base at commit `8ff612b8d27f8140aa4881a7fd23caef0de9fa96`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`InvokeDataAutomationAsync` splices the SigV4 credential-scope region verbatim into the ARN it mints, and `crate::state::request_region` checks that region only for emptiness and charset — never for length. The ARN is built as `arn:aws:bedrock:{region}:000000000000:data-automation-invocation/{33-char id}`, whose fixed part is 90 characters, so any signed region longer than 38 characters (all of which the modelled region charset `[a-zA-Z0-9-]*` admits) produces an `invocationArn` of 129 characters or more.

Two consequences:

(1) The 200 response carries an `invocationArn` that violates the modelled `InvocationArn` `@length(min: 1, max: 128)` — the operation's own `@required` output member. The crate's own code states the opposite invariant: `mint_invocation_id`'s doc claims "the whole ARN stays well inside its `@length(1..128)`", and a test asserts `arn.len() <= 128` (but only for the 9-character region `us-west-2`).

(2) The record is persisted but its status can never be read again. `GetDataAutomationStatus`'s `invocationArn` input carries the SAME `@length(1..128)`, so the framework's boundary walk refuses the very ARN this handler just returned with `ValidationException` (400) before the handler runs. The job stays in the bounded store (`invocation_count()` shows it, and it holds one of the `MAX_INVOCATIONS` slots until eviction) while no caller can ever observe its status, completion time or duration. The three tagging operations happen to still resolve it, because `TaggableResourceArn` is bounded at 1011 rather than 128 — so the same record is addressable for tagging but not for the read it exists for, which is an inconsistency the crate's "the whole ARN stays well inside its `@length(1..128)`" comment was supposed to rule out.

The handler neither bounds nor rejects the region it splices in, so a request it cannot represent inside the modelled ARN is accepted and answered 200.

## Evidence

The modelled bound (kind A), quoted from `smithy_query` operation `InvokeDataAutomationAsync` / `GetDataAutomationStatus` and restated in the crate's generated contract docs:
- `OPERATIONS.md:55` — "**Output:** `invocationArn` (string, len 1..128, pattern ^arn:aws(|-cn|-iso|-iso-[a-z]|-us-gov):bedrock:[a-zA-Z0-9-]*:[0-9]{12}:data-automation-invocation/[a-zA-Z0-9-_]+$, required)"
- `OPERATIONS.md:61` — "**Input:** `invocationArn` (string, len 1..128, pattern ^arn:aws...:data-automation-invocation/[a-zA-Z0-9-_]+$, @httpLabel, required)"
- `smithy_query` operation GetDataAutomationStatus: `"invocationArn": {"target": "InvocationArn", ..., "length": {"min": 1, "max": 128}, "pattern": "...:bedrock:[a-zA-Z0-9-]*:[0-9]{12}:data-automation-invocation/[a-zA-Z0-9-_]+$"}` — note the region segment `[a-zA-Z0-9-]*` carries no length bound of its own.

The crate's own claim that the code breaks (kind B) — `src/state.rs:667-669`:
```
/// reused even after the record it named was retired. Lower-case hex and one hyphen,
/// so the id is inside the modelled `InvocationArn` pattern's `[a-zA-Z0-9-_]+`
/// resource segment and the whole ARN stays well inside its `@length(1..128)`.
```

The code that can break it — `src/state.rs:811-821` (only emptiness and charset are checked; no length):
```
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}
```
and `src/state.rs:794-800`:
```
pub fn invocation_arn(region: &str, invocation_id: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{INVOCATION_RESOURCE_PREFIX}{invocation_id}"),
    )
}
```
with `vendor/server/src/lib.rs:162-163`:
```
pub fn emulator_arn(service: &str, region: &str, resource: &str) -> String {
    format!("arn:aws:{service}:{region}:{EMULATOR_ACCOUNT_ID}:{resource}")
}
```

The region is taken verbatim out of the `Authorization` header with no length check — `vendor/server/src/lib.rs:1800`: `if access_key_id.is_empty() || date.is_empty() || region.is_empty() || service.is_empty() { return None; }`.

The handler path — `src/ops/invoke_data_automation_async.rs:263` `let region = crate::state::request_region(ctx.parts());` and `:281-282`:
```
let invocation_id = store.mint_invocation_id(now);
let invocation_arn = crate::state::invocation_arn(&region, &invocation_id);
```

Arithmetic: `arn:aws:bedrock:` (16) + region + `:` (1) + `000000000000` (12) + `:` (1) + `data-automation-invocation/` (27) + the 33-character id from `mint_invocation_id` (`format!("{since_epoch:016x}-{:016x}", ...)`, src/state.rs:676) = 90 + len(region). A region of 39+ admissible characters yields 129+ characters.

Test coverage: the invariant IS asserted, but only for a short region — `tests/all/data_automation_invocations.rs:212-216`:
```
assert!(
    !arn.is_empty() && arn.len() <= 128,
    "case {case:?}: InvocationArn is @length(1..128), got {} chars in {arn:?}",
    arn.len()
);
```
with `tests/all/data_automation_invocations.rs:51` `const REGION: &str = "us-west-2";` (9 characters), so no row exercises a long signed region. `tests/all/asset_processing_configuration.rs:34` and `tests/all/tag_key_refusal.rs:28` likewise sign only for `us-west-2`. Nothing in the crate justifies or documents a divergence from the 128 bound — `src/state.rs:667-669` claims compliance.

## Steps to Reproduce

1. Build a router over one shared `engine::InMemoryEngine` with `bda::EmulatedService` registered, and seed a data-automation project in the build-time store (as `tests/all/data_automation_invocations.rs` `service()` does).
2. POST / with `x-amz-target: AmazonBedrockKeystoneRuntimeService.InvokeDataAutomationAsync` and an `authorization` header whose credential scope region is 39 admissible characters, e.g. `AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/aaaaaaaaa-bbbbbbbbb-ccccccccc-ddddddddd/bedrock/aws4_request, SignedHeaders=host, Signature=0000`, body {"inputConfiguration":{"s3Uri":"s3://in-bucket/assets/doc.pdf"},"outputConfiguration":{"s3Uri":"s3://out-bucket/results/"},"dataAutomationProfileArn":"arn:aws:bedrock:us-west-2:aws:data-automation-profile/us.data-automation-v1","dataAutomationConfiguration":{"dataAutomationProjectArn":"<the seeded project ARN>"}}.
3. Observe HTTP 200 and read `invocationArn` from the body: it is 129 characters, past the modelled `InvocationArn` `@length(min: 1, max: 128)` that `src/state.rs:669` claims it stays "well inside".
4. Call `GetDataAutomationStatus` with that exact ARN: the framework's boundary walk refuses it with HTTP 400 `ValidationException` ("Member must have length less than or equal to 128") because the input member carries the same bound — the job just minted can never have its status, `jobCompletionTime` or `jobDurationInSeconds` read.
5. Read `bda::state::service_state(&engine).unwrap().lock().invocation_count()`: the unreachable record is still retained and still occupies one of the `MAX_INVOCATIONS` slots.
6. For contrast, call `ListTagsForResource` with the same ARN: it succeeds, because `TaggableResourceArn` is bounded at 1011 rather than 128 — the same record is addressable for tagging but not for the status read it exists for.
