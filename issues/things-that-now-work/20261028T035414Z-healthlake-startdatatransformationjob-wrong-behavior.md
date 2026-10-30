Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1f84dda0f3deaf97a2c7ee51f672a37b240346b3

# A reused ClientToken is refused with 400, although the modelled member says the service returns no error

_Filed: 2026-10-28T03:54:14Z_
_Operation: StartDataTransformationJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1f84dda0f3deaf97a2c7ee51f672a37b240346b3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartDataTransformationJob` models `ClientToken` as a REQUIRED member whose documentation states what the service does when a token is seen twice: "If this token matches a previous request, the service ignores the request but does not return an error." The handler does the opposite for every repeat whose request body is not byte-for-byte equivalent to the first: it looks the token up on the stored job records, compares a fingerprint it computes over the request members, and when the fingerprint differs it answers the modelled `ValidationException` (HTTP 400) instead of ignoring the request.

The contradiction is also internal to the handler file: the module doc QUOTES the model sentence verbatim and then, in the same sentence, states the rule the code implements — "and a token reused with a DIFFERENT request is refused" — so the comment above the code says the service does not return an error while the code under it returns one.

Effect on a caller: a client that retries `StartDataTransformationJob` under the token it already used, with any member re-rendered differently (an added `JobName`, an explicit `ProvenanceEnabled`, a different `DriftDetectionEnabled`, a re-pointed `InputDataConfig`), gets a 400 instead of the idempotent answer the token exists to give — and the modelled contract says no error is returned for a token that matches a previous request.

The same contradiction is present in `CreateDataTransformationProfile`, whose `ClientToken` carries the identical modelled sentence (src/ops/create_data_transformation_profile.rs:66) and whose handler refuses a fingerprint mismatch the same way (src/ops/create_data_transformation_profile.rs:243-248); it is not filed separately, so a fix should settle both call sites.

## Evidence

MODEL — `smithy_query(query="doc", shape="StartDataTransformationJobRequest", member="ClientToken")`:
"A unique, case-sensitive identifier to ensure that the operation completes no more than one time. If this token matches a previous request, the service ignores the request but does not return an error."
(the member is `required: true`, `smithy_query(query="operation", operation="StartDataTransformationJob")`)

HANDLER DOC quoting that same sentence — services/service-com-amazonaws-healthlake-healthlake/src/ops/start_data_transformation_job.rs:21-27:
```
//! `ClientToken` is this operation's required idempotency token — "a unique,
//! case-sensitive identifier to ensure that the operation completes no more
//! than one time. If this token matches a previous request, the service ignores
//! the request but does not return an error" — so a repeat of the SAME call is
//! answered out of the job that token started instead of starting a second one,
//! and a token reused with a DIFFERENT request is refused, exactly as the other
//! operations in this crate that honour a token do.
```

CODE that returns the error the quoted sentence says is not returned — src/ops/start_data_transformation_job.rs:220-226:
```
        if let Some(started) = store.transformation_job_for_client_token(&input.client_token) {
            if started.request_fingerprint != fingerprint {
                return Err(validation(
                    "ClientToken was already used for a different \
                     StartDataTransformationJob request",
                ));
            }
```
(`validation(..)` is this operation's modelled `ValidationException`, src/ops/start_data_transformation_job.rs:316-321, served at HTTP 400.)

The refusal is pinned by tests/all/start_data_transformation_job.rs:540-592, whose stated reason is the handler's own reading rather than the model's sentence:
```
    // The same token with a DIFFERENT request is a different request, and is
    // refused rather than answered with the earlier job. The rows differ along
    // which member differs.
...
        assert_eq!(
            status, 400,
            "case {case:?}: a reused ClientToken must be refused, got {status}: {payload}"
        );
```

SIBLING with the identical modelled sentence — src/ops/create_data_transformation_profile.rs:66 ("If this token matches a previous request, the service ignores the request but") and src/ops/create_data_transformation_profile.rs:243-248, which refuses the same way.

## Steps to Reproduce

1. Create a data transformation profile and note its `ProfileId`.
2. Call `StartDataTransformationJob` with `ClientToken: "token-one"`, that `ProfileId`, an `InputDataConfig`, an `OutputDataConfig` and a `DataAccessRoleArn`; observe 200 with a `JobId`.
3. Call `StartDataTransformationJob` again with the SAME `ClientToken: "token-one"` but one member spelled differently (for example add `"JobName": "nightly"`).
4. Observe HTTP 400 `ValidationException` "ClientToken was already used for a different StartDataTransformationJob request" (src/ops/start_data_transformation_job.rs:220-226).
5. Compare with the modelled member documentation for `StartDataTransformationJobRequest$ClientToken`: "If this token matches a previous request, the service ignores the request but does not return an error."
