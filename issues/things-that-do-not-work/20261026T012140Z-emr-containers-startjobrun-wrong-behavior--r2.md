Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d25a8df08abf64a2b119d49e5e317e50f9894e06

# A spent clientToken carrying a DIFFERENT request is answered as a replay, silently dropping the new submission

_Filed: 2026-10-26T01:21:40Z_
_Operation: StartJobRun | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d25a8df08abf64a2b119d49e5e317e50f9894e06`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateJobTemplate` decides a replay by comparing a fingerprint of the whole request against the stored one, and refuses a token that is re-used with different parameters, because answering it with the first resource "would silently drop the parameters this call asked for". `StartJobRun` (and the same way `CreateVirtualCluster`, `CreateManagedEndpoint`, `CreateSecurityConfiguration`) applies no such comparison: any request carrying a token already spent in that scope is answered with the resource the FIRST call minted, whatever the new request asked for. So a caller that reuses a token by mistake with a different `jobDriver`/`executionRoleArn`/`releaseLabel` receives HTTP 200 with the earlier run's id and ARN, and the work it submitted is never stored and never runs — the exact harm the sibling handler's comment names. The crate therefore implements one `@idempotencyToken` contract two different ways; one of the two readings has to go.

## Evidence

The rule, as this crate states and enforces it — services/.../src/ops/create_job_template.rs:181-204:
```
// `clientToken` is `@idempotencyToken`: a token already spent on this operation
// names the template the first call minted ... The same token carrying a
// DIFFERENT request is not a replay — answering it with the first template would
// silently drop the parameters this call asked for — so it is refused.
...
if existing.request_fingerprint != fingerprint {
    return Err(CreateJobTemplateErrorModeled::ValidationException(...
        "clientToken {} was already used to create job template {}, with different parameters"
```
and its helper, state.rs:794-800: "`clientToken` is `@idempotencyToken`, which means \"the same token names the same request\": a second call carrying a token already spent is a replay only when it really is the same request. This digest is how that is decided".

The sibling that skips it — services/.../src/ops/start_job_run.rs:366-380:
```
// `clientToken` is `@idempotencyToken`: the same token replayed names the job
// run the first call minted rather than submitting the work twice.
let replayed = state.replayed_idempotency(&region, &scope, &input.client_token).cloned();
if let Some(existing) = replayed.and_then(|id| state.job_run(&region, &input.virtual_cluster_id, &id)) {
    return Ok(StartJobRunOutput { id: Some(existing.id.clone()), ... });
}
```
No comparison of the request against `existing`, and `JobRunRecord` (state.rs:570-598) stores no request fingerprint at all.

The same gap, with a handler doc that contradicts its own code — create_managed_endpoint.rs:12-14 "`clientToken` is `@idempotencyToken`, so a token already spent on this operation in this cluster does NOT mint a second endpoint: the same request replayed answers with the endpoint the first call minted", while create_managed_endpoint.rs:209-221 answers ANY request bearing that token with the stored endpoint. Identically create_virtual_cluster.rs:207-220 and create_security_configuration.rs:171-184.

Test coverage: tests/all/job_template_idempotency.rs:19-21 pins the refusal for CreateJobTemplate ("2. The same token carrying a DIFFERENT request is not a replay, and is refused with the modelled `ValidationException` (HTTP 400) rather than answered with the first template."); no test in tests/all/ asserts anything about a token re-used with different parameters on StartJobRun, CreateVirtualCluster, CreateManagedEndpoint or CreateSecurityConfiguration.

## Steps to Reproduce

1. Create a virtual cluster (POST /virtualclusters) and note its id.
2. POST /virtualclusters/{id}/jobruns with clientToken "t1", name "run-a", executionRoleArn/releaseLabel/jobDriver pointing at entryPoint "s3://bucket/a.py". Note the returned job-run id.
3. POST /virtualclusters/{id}/jobruns again with clientToken "t1" but a different jobDriver (entryPoint "s3://bucket/b.py") and name "run-b".
4. Observe HTTP 200 carrying the FIRST run's id/name/arn; GET /virtualclusters/{id}/jobruns shows only one run, and no run exists with entryPoint "s3://bucket/b.py" — the second submission was dropped.
5. Do the same against POST /jobtemplates with one clientToken and two different bodies: the second call is refused with ValidationException (create_job_template.rs:194-204), i.e. the two operations disagree about what a spent token means.
