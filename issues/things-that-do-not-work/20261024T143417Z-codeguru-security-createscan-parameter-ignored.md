Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# A clientToken replay silently drops the new request's resourceId, scanType, analysisType and tags, and the token never expires

_Filed: 2026-10-24T14:34:17Z_
_Operation: CreateScan | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `clientToken` is `@idempotencyToken` and documented as preventing "the accidental creation of duplicate scans if there are failures and retries". A retry means the SAME request sent again, so the only inputs a replay may answer from the original record are those that were identical; a token replayed with DIFFERENT parameters is not a retry of anything, and a service either starts the new revision or refuses the mismatch with the modeled ConflictException (409, "The requested operation would cause a conflict with the current state of a service resource associated with the request"). AWS idempotency tokens are also bounded in time (commonly 8-24 hours), after which the same token is treated as a fresh request.

Actual: the handler consults the ledger on `(clientToken, scanName)` ONLY and, on a hit, returns the stored run unconditionally — before `resourceId` is even looked at. Consequences observable over HTTP:
* `resourceId.codeArtifactId` is dropped: a second CreateScan with the same token but a brand-new code artifact answers 200 with the ORIGINAL `runId` and the ORIGINAL artifact id echoed back in `resourceId`, no new run is recorded, and the newly uploaded resource is never scanned. The caller is told its scan started when nothing was started for the artifact it named.
* `scanType`, `analysisType` and `tags` of the replayed request are likewise never compared and never stored, so a replay that asks for `Express`/`All` is answered with the original run's `Standard`/`Security` values and the scan keeps its old tag map.
* the replay path runs before the artifact-existence check (`ResourceNotFoundException`) and before the scan-name/artifact agreement check (`ValidationException`), so a replay naming a nonexistent or expired `codeArtifactId` — an input that would otherwise be refused — is answered 200.
* nothing ever removes an entry from `store.idempotency`, so a token is honoured for the lifetime of the process: a client that reuses a fixed token (scripts, IaC) can never start a second revision of a scan and keeps receiving the first run's `runId` and `scanState` forever.

This is distinct from the already-filed CreateScan issues (tag-map replacement on a re-run; the 15-minute artifact TTL): both of those concern a request that actually creates a run, whereas this one concerns the short-circuit path where no run is created at all.

## Evidence

Smithy model: `CreateScanRequest$clientToken` carries `@idempotencyToken` (length 1..64, pattern ^[\S]+$) and is documented "Amazon CodeGuru Security uses this value to prevent the accidental creation of duplicate scans if there are failures and retries"; `resourceId` is `@required` (union ResourceId#codeArtifactId); `scanType`/`analysisType`/`tags` are modeled inputs; CreateScan declares ConflictException (409) and ResourceNotFoundException (404). Handler: services/service-com-amazonaws-codegurusecurity-awscodegurusecurity/src/ops/create_scan.rs lines 202-232 — the ledger lookup on `(token, scan_name)` returns `CreateScanOutput` built from the recorded run (`run.code_artifact_id`, `run.scan_state`) with no comparison of `input.resource_id`, `input.scan_type`, `input.analysis_type` or `input.tags`, and it precedes the artifact lookup at lines 240-253 and the scanName/artifact agreement check at lines 259-278. State: `store.idempotency` is written at lines 412-416 and, per the module docs in src/state.rs lines 25-26, is only ever grown — there is no expiry sweep for it (unlike `artifact_expiry` / `pending_runs`). Test coverage: no test in tests/all/ replays a clientToken with a changed resourceId or changed scanType/analysisType/tags (tests/all/scan_lifecycle.rs exercises the identical-retry case only), so the mismatch behavior is unasserted in either direction.

## Steps to Reproduce

1. POST /uploadUrl {"scanName":"idem-scan"} -> note codeArtifactId A and s3Url A; PUT the archive to s3Url A
2. POST /scans {"scanName":"idem-scan","clientToken":"fixed-token","resourceId":{"codeArtifactId":"A"},"scanType":"Standard"} -> 200 with runId R1
3. POST /uploadUrl {"scanName":"idem-scan"} again -> note codeArtifactId B; PUT a DIFFERENT archive to s3Url B
4. POST /scans {"scanName":"idem-scan","clientToken":"fixed-token","resourceId":{"codeArtifactId":"B"},"scanType":"Express","analysisType":"All","tags":{"Env":"prod"}} -> observe 200 echoing runId R1 and resourceId.codeArtifactId = A; artifact B is never scanned and no second revision exists
5. GET /scans/idem-scan -> observe numberOfRevisions still 0 and analysisType still Security; GET /tags/<scanNameArn> -> observe the Env tag from step 4 was never stored
6. POST /scans with clientToken="fixed-token", scanName="idem-scan" and resourceId.codeArtifactId set to a UUID this emulator never minted -> observe 200 (the replay answers before the ResourceNotFoundException check that would otherwise refuse it)
