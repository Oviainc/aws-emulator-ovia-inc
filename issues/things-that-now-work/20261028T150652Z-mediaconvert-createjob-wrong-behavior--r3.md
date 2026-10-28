Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a19f71fb9b9d334f8efc64d442ea8394bd20139d

# CreateJob trims ClientRequestToken, so two distinct tokens collide and the echoed token is not the one sent

_Filed: 2026-10-28T15:06:52Z_
_Operation: CreateJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a19f71fb9b9d334f8efc64d442ea8394bd20139d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateJob` normalises the caller's `ClientRequestToken` with `str::trim` before using it as the idempotency key AND before storing it on the job record. Two consequences, both wrong:

1. Two DIFFERENT tokens become one key. A caller that sends `clientRequestToken: "tok"` and then `clientRequestToken: "tok "` (or `"\ttok"`) gets the FIRST job handed back for the second request instead of a second job being created — the second create is silently turned into a no-op success, and the job the caller asked for is never stored. The model says a token "can be any string that includes up to 64 ASCII characters"; a space is an ASCII character, so `"tok"` and `"tok "` are two strings and therefore two tokens.

2. `Job$ClientRequestToken` echoes a value the caller never sent: the record stores the trimmed string (`client_request_token: token`) and `JobRecord::view` returns it verbatim, so a caller that sent `"tok "` reads back `"tok"`.

This contradicts the crate's own single, stated normalisation rule for caller-sent key members, which every sibling create follows byte for byte: only a value with NO characters at all is refused, and a value that does carry characters is stored and echoed unchanged, "surrounding whitespace included". The token is exactly such a key — it is the key of the `idempotency` map.

No test pins the trim: `tests/all/job_lifecycle.rs::a_repeated_client_request_token_returns_the_same_job` (line 988) only drives tokens with no surrounding whitespace (`"t"`, a UUID, `"x".repeat(64)`), so the fix leaves it green.

## Evidence

The trim, in the handler — `services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/create_job.rs:288-293`:
```rust
        let token = input
            .client_request_token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(str::to_string);
```
and the trimmed value is what is persisted and echoed — `create_job.rs:589`: `client_request_token: token,`; `src/state.rs:3211`: `client_request_token: self.client_request_token.clone(),`.

The crate's own rule for a caller-sent key member — `src/state.rs:607-611`, the doc on `carries_no_characters`:
```
/// This is the only normalisation such a member gets: a value that DOES carry characters
/// is stored and echoed byte for byte, surrounding whitespace included, because that value
/// is the resource's key.
pub fn carries_no_characters(value: &str) -> bool {
    value.trim().is_empty()
}
```
The sibling that follows it — `src/ops/create_queue.rs:181-188`:
```
        // The `Name` is the resource's KEY, so it is stored and echoed back exactly as the
        // caller sent it — never normalised: nothing in the model or in AWS's
        // documentation normalises it, ...
        let name = input.name;
        if crate::state::carries_no_characters(&name) {
```

The model text the handler is answering — `src/ops/create_job.rs:56-61` (generated from the model; `smithy_query(query="doc",shape="CreateJobRequest",member="ClientRequestToken")`):
```
//! * `ClientRequestToken` (__string, @idempotencyToken) — Prevent duplicate jobs from being
//!   created and ensure idempotency for your requests. A client request token can be any
//!   string that includes up to 64 ASCII characters. If you reuse a client request token
//!   within one minute of a successful request, the API returns the job details of the
//!   original request instead.
```

Test coverage: `tests/all/job_lifecycle.rs:985-1031` covers token reuse and distinctness, but only for tokens with no surrounding whitespace.

## Steps to Reproduce

1. POST /2017-08-29/jobs with {"role": <role>, "clientRequestToken": "tok ", "settings": {<one input, one complete output group>}} and note the 201 and the returned job id.
2. Read the response's /job/clientRequestToken: it is "tok", not the "tok " that was sent.
3. POST /2017-08-29/jobs again with the same body but "clientRequestToken": "tok" (no trailing space).
4. Observe the response is the SAME job (same /job/id) instead of a new job: the two distinct tokens were collapsed into one idempotency key.
5. GET /2017-08-29/jobs and confirm only one job exists, although two different tokens were used.
