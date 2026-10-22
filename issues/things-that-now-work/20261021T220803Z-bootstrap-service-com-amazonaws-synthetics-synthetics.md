Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 95c91931c960f7f391e4448ce7f3ebc67d41fa17

# Bootstrap the synthetics emulator

_Filed: 2026-10-21T22:08:03Z_
_Operation: service-com-amazonaws-synthetics-synthetics | Class: missing_

> _Note: this issue was identified against the code base at commit `95c91931c960f7f391e4448ce7f3ebc67d41fa17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 22 operations in `service-com-amazonaws-synthetics-synthetics/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state. Implement a FIRST stateful slice covering the core CloudWatch Synthetics canary lifecycle, so a realistic client script works end to end. Protocol is `restJson1` (SDK id `synthetics`).

### Happy path to implement (in order)

1. **CreateCanary** — `POST /canary`. Required input: `Name` (1..255, `^[0-9a-z_\-]+$`), `Code` (CanaryCodeInput: `Handler`, `S3Bucket`/`S3Key`/`ZipFile`), `ArtifactS3Location`, `ExecutionRoleArn`, `Schedule` (CanaryScheduleInput: `Expression`, `DurationInSeconds`), `RuntimeVersion`. Optional: `RunConfig`, `SuccessRetentionPeriodInDays`, `FailureRetentionPeriodInDays`, `VpcConfig`, `ArtifactConfig`, `Tags`, `ProvisionedResourceCleanup`. Must persist a new canary record and return `Canary` populated from the stored record: generated `Id` (UUID matching `^[a-f0-9]{8}-...`), `Name`, `Code` (CanaryCodeOutput with `Handler` + synthesized `SourceLocationArn`), `ExecutionRoleArn`, `Schedule` (CanaryScheduleOutput echoing stored `Expression`/`DurationInSeconds`), `RunConfig`, retention days, `ArtifactS3Location`, `RuntimeVersion`, `EngineArn`, `Tags`, and `Status.State = CREATING` (or `READY`) plus `Timeline.Created`/`LastModified` set to now. Duplicate `Name` must fail (real service returns `ValidationException`, HTTP 400); malformed `Name`/`ExecutionRoleArn` against the modeled patterns must return `ValidationException` too.
2. **GetCanary** — `GET /canary/{Name}`. Returns `Canary` read back from state, byte-for-byte consistent with what CreateCanary stored (same `Id`, `Schedule.Expression`, `RuntimeVersion`, `Tags`, timeline). Unknown name must NOT return a hollow 200 — return the modeled `ResourceNotFoundException` (404) for a name that was never created.
3. **StartCanary** — `POST /canary/{Name}/start`. Empty output body, but must mutate state: set `Status.State` to `RUNNING`, set `Timeline.LastStarted = now`, and append a `CanaryRun` record (`Id` UUID, `Name`, `Status.State = RUNNING`, `Timeline.Started`, `ArtifactS3Location`) to that canary's run history. `ResourceNotFoundException` (404) for an unknown canary; `ConflictException` (409) when the canary is already running.
4. **StopCanary** — `POST /canary/{Name}/stop`. Set `Status.State = STOPPED`, `Timeline.LastStopped = now`, and finish the open run (`Status.State = PASSED`, `Timeline.Completed`). `ConflictException` (409) when the canary is not running; `ResourceNotFoundException` (404) for unknown names.
5. **DescribeCanaries** — `POST /canaries`. Returns `Canaries` built by listing the stored canaries, honouring `Names` (filter, 1..5 entries), `MaxResults` (1..20) and `NextToken` (4..252) — emit `NextToken` only when more stored canaries remain, and resume exactly from it on the next call. Must reflect live state (a created canary appears; a deleted one does not).
6. **DeleteCanary** — `DELETE /canary/{Name}?deleteLambda=`. Removes the canary (and its run history) from state, returns an empty 200. A later `GetCanary`/`DescribeCanaries` must no longer see it, and a second DeleteCanary must return `ResourceNotFoundException` (404). Per the service, deleting a canary in `RUNNING` state is a `ConflictException` (409).

Every response in this slice must be computed from the persisted record — no canned structs, no echoing the request back without storing it, no random `Id` that is not written to the store. A `Canary` returned by GetCanary/DescribeCanaries must be the same record CreateCanary wrote, including its generated `Id` and timeline timestamps.

## State Model

In `src/state.rs` (`ComAmazonawsSyntheticsSyntheticsState`, reached via `crate::state::service_state(&*self.engine)`):

- `canaries: BTreeMap<String /* canary name */, StoredCanary>` — BTreeMap so DescribeCanaries has a stable, deterministic order for tokenized pagination.
- `StoredCanary { id: String /* UUID */, name: String, code: StoredCode { handler: Option<String>, source_location_arn: String, s3_bucket: Option<String>, s3_key: Option<String>, s3_version: Option<String> }, execution_role_arn: String, schedule: StoredSchedule { expression: String, duration_in_seconds: Option<i64> }, run_config: Option<StoredRunConfig { timeout_in_seconds, memory_in_mb, active_tracing, environment_variables }>, success_retention_period_in_days: i32 (default 31), failure_retention_period_in_days: i32 (default 31), artifact_s3_location: String, artifact_config: Option<StoredArtifactConfig>, runtime_version: String, vpc_config: Option<StoredVpcConfig>, engine_arn: String, provisioned_resource_cleanup: Option<String>, kms_key_arn: Option<String>, state: CanaryState /* CREATING | READY | STARTING | RUNNING | STOPPING | STOPPED | UPDATING | ERROR | DELETING */, state_reason: Option<String>, state_reason_code: Option<String>, created: SystemTime, last_modified: SystemTime, last_started: Option<SystemTime>, last_stopped: Option<SystemTime>, tags: BTreeMap<String,String>, runs: Vec<StoredRun> }`
- `StoredRun { id: String /* UUID */, name: String, state: CanaryRunState /* RUNNING | PASSED | FAILED */, state_reason: Option<String>, state_reason_code: Option<String>, started: SystemTime, completed: Option<SystemTime>, artifact_s3_location: String }` — newest-first when later surfaced by GetCanaryRuns.
- `arn_for(name)` helper producing `arn:aws:synthetics:<region>:<account>:canary:<name>` so the tag/group operations can key off the same records when they are implemented later.

Out of scope for this first slice (stay 501, filed separately): groups (CreateGroup/GetGroup/DeleteGroup/ListGroups/ListGroupResources/ListAssociatedGroups/AssociateResource/DisassociateResource), tagging (TagResource/UntagResource/ListTagsForResource), UpdateCanary, GetCanaryRuns, DescribeCanariesLastRun, DescribeRuntimeVersions, StartCanaryDryRun.

## Evidence

All 22 handler files under `services/service-com-amazonaws-synthetics-synthetics/src/ops/` carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker on line 4 and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501, so no canary can be created or read. Contract grounded in `services/service-com-amazonaws-synthetics-synthetics/OPERATIONS.md`: CreateCanary `POST /canary` (required `Name`/`Code`/`ArtifactS3Location`/`ExecutionRoleArn`/`Schedule`/`RuntimeVersion`, output `Canary`, errors ValidationException 400), GetCanary `GET /canary/{Name}` (output `Canary`), StartCanary `POST /canary/{Name}/start` and StopCanary `POST /canary/{Name}/stop` (empty output, errors ConflictException 409 / ResourceNotFoundException 404), DescribeCanaries `POST /canaries` (@paginated NextToken/MaxResults 1..20, `Names` 1..5, output `Canaries` + `NextToken`), DeleteCanary `DELETE /canary/{Name}` with `deleteLambda` query param (errors ConflictException 409, ResourceNotFoundException 404). Field lists for the `Canary`, `CanaryStatus`, `CanaryTimeline`, `CanaryCodeOutput`, `CanaryScheduleOutput`, `CanaryRun` structures and the `CanaryState`/`CanaryRunState` enums come from the Smithy model (`smithy_query shape`).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `synthetics` client at it.
2. CreateCanary with Name=`my-canary`, Code={Handler:`index.handler`, S3Bucket/S3Key or ZipFile}, ArtifactS3Location=`s3://my-bucket/canary`, ExecutionRoleArn=`arn:aws:iam::123456789012:role/CanaryRole`, Schedule={Expression:`rate(5 minutes)`}, RuntimeVersion=`syn-nodejs-puppeteer-9.0`, Tags={env:test} — expect 200 with `Canary.Id` a UUID and `Canary.Status.State` in CREATING/READY (currently 501).
3. GetCanary Name=`my-canary` — expect the SAME `Id`, `Schedule.Expression=rate(5 minutes)`, `RuntimeVersion`, `Tags` and a populated `Timeline.Created`.
4. StartCanary Name=`my-canary`, then GetCanary — expect `Status.State=RUNNING` and `Timeline.LastStarted` set.
5. StopCanary Name=`my-canary`, then GetCanary — expect `Status.State=STOPPED` and `Timeline.LastStopped` set.
6. DescribeCanaries with no filter — expect `my-canary` in `Canaries`; repeat with MaxResults=1 over two canaries and verify `NextToken` round-trips without repeating or skipping entries.
7. DeleteCanary Name=`my-canary`, then GetCanary — expect ResourceNotFoundException (404), and DescribeCanaries no longer lists it.
8. CreateCanary twice with the same Name — expect the second call to fail (ValidationException 400), not to silently overwrite.
