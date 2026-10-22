Issue-Source: trace-sweep-divergence
Issue-Trace: emr-serverless-error-paths
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: emr-serverless-error-paths emr-serverless-tag-resource-list-tags-roundtrip emr-serverless-untag-resource-removes-tag
Issue-Shape-Id: com.amazonaws.emrserverless#AwsToledoWebService
Issue-Key: emr-serverless|CreateApplication|4xx->2xx|$.message
Issue-Key: emr-serverless|CreateApplication|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: emr-serverless|CreateApplication|4xx->2xx|status
Issue-Key: emr-serverless|DeleteApplication|2xx->4xx|status
Issue-Key: emr-serverless|GetApplication|2xx->2xx|$.application.architecture
Issue-Key: emr-serverless|GetApplication|2xx->2xx|$.application.autoStartConfig
Issue-Key: emr-serverless|GetApplication|2xx->2xx|$.application.autoStopConfig
Issue-Key: emr-serverless|GetApplication|2xx->2xx|$.application.autoStopConfiguration.idleTimeout
Issue-Key: emr-serverless|GetApplication|2xx->2xx|$.application.extendedSupportConfiguration
Issue-Key: emr-serverless|GetApplication|2xx->2xx|$.application.jobLevelCostAllocationConfiguration
Issue-Key: emr-serverless|GetApplication|2xx->2xx|$.application.monitoringConfiguration
Issue-Key: emr-serverless|GetApplication|2xx->2xx|$.application.state
Issue-Key: emr-serverless|GetApplication|2xx->2xx|$.application.stateDetails
Issue-Key: emr-serverless|GetApplication|2xx->2xx|$.application.tags
Issue-Key: emr-serverless|GetApplication|2xx->2xx|$.application.type
Issue-Commit: bffba5cba440022693dbdf16c8135b8d2c175737

# emr-serverless application lifecycle and create-time engine validation diverge from AWS

> _Note: this issue was identified against the code base at commit `bffba5cba440022693dbdf16c8135b8d2c175737`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplication

## Description
Three causes in the emr-serverless application control plane: a create that should be refused is accepted, a delete that should succeed is refused, and the application projection a read returns is incomplete and renormalised. The second and third share one root — the emulator invents a transient CREATING window and gates DeleteApplication on it — but each is a separate replay failure and all three must be closed.

Cause 1 of 3: CreateApplication accepts an engine type that is not supported for the requested release label.
AWS: CreateApplication models `type` (EngineType, @required, length 1..64) and `releaseLabel` (@required) and declares ValidationException (HTTP 400). Smithy: CreateApplication errors = ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ValidationException 400; EngineType is an open string, so the supported engines for a release label are a service-side rule, not a model constraint. Recorded behavior: a create naming an engine type the release label does not support is refused with HTTP 400 and the REST-JSON error header x-amzn-errortype set; the message is built from the request and names the rejected type (recorded example: "Type NOT_A_REAL_ENGINE is not supported for release label").
Emulator: answers HTTP 200 and mints an application, with no body message and no x-amzn-errortype header.
Why: a client that validates its engine/release pairing by calling CreateApplication is told a nonexistent engine is fine. It then holds an application id whose jobs can never run, and its error-handling path for the 400 is never exercised — a bad configuration escapes into the caller's workflow instead of being rejected at the API boundary.
Fix: CreateApplication's handler/validation — the engine-type-versus-release-label check is simply absent from `handle` (the handler stores `input.r#type` and `input.release_label` unexamined), so add the refusal as the operation's modelled ValidationException.
Traces: emr-serverless-error-paths#task#3; services/service-com-amazonaws-emrserverless-awstoledowebservice/src/ops/create_application.rs:242-294.

Cause 2 of 3: DeleteApplication refuses a freshly created application with HTTP 400 instead of deleting it with HTTP 200.
AWS: Smithy: DeleteApplication DELETE /applications/{applicationId}, success code 200 with an empty body; errors ValidationException 400, ResourceNotFoundException 404, InternalServerException 500. The operation documents the precondition "An application has to be in a stopped or created state in order to be deleted." Recorded behavior: deleting an application immediately after CreateApplication returns 200 — AWS does not hold the application in a non-deletable state across the create/delete pair.
Emulator: answers HTTP 400 ValidationException because it keeps the record in CREATING for a self-chosen 500ms window (state.rs LIFECYCLE_TRANSITION) and DELETABLE_APPLICATION_STATES admits only CREATED and STOPPED, so any delete issued inside that window is rejected.
Why: every create-then-delete flow — including test teardown — fails against the emulator, so applications accumulate and callers see a modelled validation error for a request AWS accepts. The window is wall-clock dependent, making the failure timing-sensitive and non-reproducible for the client. This cluster also covers the teardown of emr-serverless-tag-resource-list-tags-roundtrip and emr-serverless-untag-resource-removes-tag.
Fix: DeleteApplication's handler and the application state machine in the service state — either settle the create transition before the deletable-state gate or do not fabricate a transient CREATING window that AWS's recordings never show.
Traces: emr-serverless-error-paths#teardown#8 (cluster also covers emr-serverless-tag-resource-list-tags-roundtrip#teardown#5 and emr-serverless-untag-resource-removes-tag#teardown#5); services/service-com-amazonaws-emrserverless-awstoledowebservice/src/ops/delete_application.rs:142-175; services/service-com-amazonaws-emrserverless-awstoledowebservice/src/state.rs:92 and :120.

Cause 3 of 3: GetApplication returns an incomplete Application with a renormalised engine type and no post-delete terminal state.
AWS: Smithy: GetApplication GET /applications/{applicationId} -> Application, where `state` (ApplicationState: CREATING|CREATED|STARTING|STARTED|STOPPING|STOPPED|TERMINATED) and `type` (EngineType) are @required, and `stateDetails`, `tags`, `architecture`, `autoStartConfiguration`, `autoStopConfiguration` and `monitoringConfiguration` are modelled members. Recorded behavior: after the application is deleted, GetApplication still answers 200 with state TERMINATED; the engine type is reported in AWS's canonical casing (recorded example `Spark`, not the request's casing); `stateDetails` is populated (recorded example "Application created by user."); `tags` is present as an empty map rather than omitted; and the auto-start/auto-stop/monitoring/architecture configuration is echoed (the recording also carries the unmodelled aliases autoStartConfig/autoStopConfig/extendedSupportConfiguration/jobLevelCostAllocationConfiguration — recorded behavior only).
Emulator: answers 200 but with state CREATING, type uppercased to the request's `SPARK` instead of the canonical form, and architecture, stateDetails, tags, autoStartConfiguration/autoStopConfiguration and monitoringConfiguration all absent from the body. Because Cause 2's delete was refused the record was never removed, and the emulator's DeleteApplication drops the record outright rather than retaining it as TERMINATED, so no code path can produce the recorded post-delete read.
Why: a client polling GetApplication for a lifecycle transition never observes a terminal state, so it waits forever or treats a deleted application as still creating; a client reading back `type`, `tags` or the auto-stop configuration it submitted gets a different value or no value at all, breaking round-trip comparisons and any config-drift check.
Fix: GetApplication's serializer/projection plus the application record in the service state — persist and report stateDetails, tags and the defaulted configuration blocks, canonicalise the engine type instead of echoing request casing, and retain a deleted application as TERMINATED instead of removing it.
Traces: emr-serverless-error-paths#teardown#9; services/service-com-amazonaws-emrserverless-awstoledowebservice/src/ops/create_application.rs:242-294; services/service-com-amazonaws-emrserverless-awstoledowebservice/src/ops/delete_application.rs:168-175.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 21.7%, 18 mismatched of 23 steps):

Cause 1 — emr-serverless-error-paths#task#3 CreateApplication: expected 400, got 200; $.message missing (expected 'Type NOT_A_REAL_ENGINE is not supported for release label', got None); $headers.x-amzn-errortype missing.

Cause 2 — emr-serverless-error-paths#teardown#8 DeleteApplication: status expected 200, got 400. Same cluster: emr-serverless-tag-resource-list-tags-roundtrip#teardown#5, emr-serverless-untag-resource-removes-tag#teardown#5.

Cause 3 — emr-serverless-error-paths#teardown#9 GetApplication: $.application.state value_mismatch (expected 'TERMINATED', got 'CREATING'); $.application.type value_mismatch (expected 'Spark', got 'SPARK'); $.application.stateDetails missing (expected 'Application created by user.'); $.application.tags missing (expected {}); $.application.architecture missing (expected 'X86_64'); $.application.autoStartConfig missing (expected {'enabled': True}); $.application.autoStopConfig missing (expected {'enabled': True, 'idleTimeout': 15, 'idleTimeoutMinutes': 15}); $.application.autoStopConfiguration.idleTimeout missing (expected 15); $.application.monitoringConfiguration missing; $.application.extendedSupportConfiguration missing; $.application.jobLevelCostAllocationConfiguration missing.

No state_reset_failures were reported for this run, so these are not cross-fixture state leaks.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it; replay the wire corpus (replay_wire_corpus).
2. Cause 1: replay fixture emr-serverless-error-paths step task#3 — CreateApplication with a release label that does not support the named engine type. Emulator answers 200; AWS answers 400 ValidationException with x-amzn-errortype set.
3. Cause 2: replay fixture emr-serverless-error-paths step teardown#8 — DeleteApplication on the application created earlier in the fixture, issued immediately after the create. Emulator answers 400 ValidationException ('... is in CREATING state ...'); AWS answers 200. Reproduces identically in emr-serverless-tag-resource-list-tags-roundtrip#teardown#5 and emr-serverless-untag-resource-removes-tag#teardown#5.
4. Cause 3: replay fixture emr-serverless-error-paths step teardown#9 — GetApplication on that same application after the delete. Emulator answers 200 with state CREATING, type 'SPARK' and no stateDetails/tags/architecture/auto-start/auto-stop/monitoring members; AWS answers 200 with state TERMINATED, type 'Spark' and those members populated.
