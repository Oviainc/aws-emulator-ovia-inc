Issue-Source: trace-sweep-divergence
Issue-Trace: neptunedata-query-languages-live-cluster-round-trip
Issue-Step: task#18
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: neptunedata-query-languages-live-cluster-round-trip
Issue-Shape-Id: com.amazonaws.neptunedata#AmazonNeptuneDataplane
Issue-Key: neptunedata|GetEngineStatus|2xx->2xx|$.dbEngineVersion
Issue-Key: neptunedata|GetEngineStatus|2xx->2xx|$.features.AuditLog
Issue-Key: neptunedata|GetEngineStatus|2xx->2xx|$.features.IAMAuthentication
Issue-Key: neptunedata|GetEngineStatus|2xx->2xx|$.features.InlineServerGeneratedEdgeId
Issue-Key: neptunedata|GetEngineStatus|2xx->2xx|$.features.SlowQueryLogs
Issue-Key: neptunedata|GetEngineStatus|2xx->2xx|$.features.Streams
Issue-Key: neptunedata|GetEngineStatus|2xx->2xx|$.gremlin.version
Issue-Key: neptunedata|GetEngineStatus|2xx->2xx|$.labMode.TableSpaceAutoExtendSizeInMB
Issue-Key: neptunedata|GetEngineStatus|2xx->2xx|$.settings.SlowQueryLogsThreshold
Issue-Key: neptunedata|GetEngineStatus|2xx->2xx|$.settings.StrictTimeoutValidation
Issue-Key: neptunedata|GetEngineStatus|2xx->2xx|$.startTime
Issue-Key: neptunedata|GetEngineStatus|2xx->2xx|$headers.content-type
Issue-Key: neptunedata|ExecuteFastReset|2xx->2xx|$headers.content-type
Issue-Key: neptunedata|ExecuteFastReset|4xx->4xx|$.code
Issue-Key: neptunedata|ExecuteFastReset|4xx->4xx|$.detailedMessage
Issue-Key: neptunedata|ExecuteFastReset|4xx->4xx|$.message
Issue-Key: neptunedata|ExecuteFastReset|4xx->4xx|$.requestId
Issue-Key: neptunedata|ExecuteFastReset|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: ab6fe5351849b3ed2ec61e1c169bea3a72511392

# neptunedata data-plane responses diverge from Neptune: engine-status payload, JSON content-type and fast-reset missing-token refusal

> _Note: this issue was identified against the code base at commit `ab6fe5351849b3ed2ec61e1c169bea3a72511392`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetEngineStatus, ExecuteFastReset

## Description
Three net-new root causes on the neptunedata data plane, all reached against a running, answering emulator (the steps were served; this is not the open unreachability issues 20261104T102956Z-neptunedata-is-entirely-unreachable-the-neptune-db-cluster.md / 20261109T113928Z-neptunedata-is-still-entirely-unreachable-the-neptune-db.md, and none of them is a refusal path covered by 20261104T225928Z-neptunedata-5-net-new-refusal-path-causes-ml.md, which is about ML/query-status/query-execution 501s and the empty message field). A fixer must close all THREE before submitting.

Cause 1 of 3: GetEngineStatus serves an engine-status document whose shape and content are not Neptune's — the `features` map is wrapped, documented keys are absent, the reported engine/query-language versions are stale, and `startTime` is rendered as an ISO-8601 instant.
AWS: Smithy (com.amazonaws.neptunedata) models GetEngineStatus GET /status -> 200, @readonly, output GetEngineStatusOutput with `features` targeting DocumentValuedMap (key String, value Document), `labMode` and `settings` targeting StringValuedMap, `gremlin`/`sparql`/`opencypher` targeting QueryLanguageVersion{version}, and `startTime`/`dbEngineVersion` as String. Recorded behavior: each `features` entry's value is the feature's status as a BARE string ('enabled'/'disabled'), not an object; the feature set includes `InlineServerGeneratedEdgeId` and `IAMAuthentication` (the latter reporting 'enabled' because the cluster under test was created with IAM database authentication, i.e. it reflects cluster configuration rather than a constant); `labMode` includes `TableSpaceAutoExtendSizeInMB`; `settings` includes `StrictTimeoutValidation` and `SlowQueryLogsThreshold` alongside clusterQueryTimeoutInMs; `dbEngineVersion` is a released Neptune version in the four-part + release-suffix form (the recording shows the 1.4.x/.Rn family) and `gremlin.version` is the TinkerPop version that engine ships (recorded as the tinkerpop-3.7.x family) — i.e. the data plane must report the engine version the control plane provisioned for the cluster and the query-language versions that engine actually carries, not a hard-coded older pair. Hypothesis: `startTime` in Neptune's /status payload is a Java `Date.toString()`-style UTC stamp ("EEE MMM dd HH:mm:ss 'UTC' yyyy"); what the recording asserts is only that the shape of the emulator's value does not match the recorded one, so the requirement is the recorded format, not an ISO-8601 instant.
Emulator: `features` values are serialized as `{"status": "<status>"}` objects (ops/get_engine_status.rs wraps each status in Document::Object under a "status" key), the record carries only AuditLog/ResultCache/SlowQueryLogs/Streams, `labMode` only ObjectIndex/ReadWriteConflictDetection, `settings` only clusterQueryTimeoutInMs, dbEngineVersion '1.3.2.0', gremlin.version 'tinkerpop-3.6.2', and startTime '2026-11-09T17:45:14.851648Z' (crate::state::engine_timestamp renders RFC 3339).
Why: a client that reads engine capability from /status mis-reads every feature (`status['features']['Streams']` is a dict where AWS gives a string, so an `== 'enabled'` test is always false and a type-sensitive client raises), cannot see whether IAM authentication is on, cannot read the slow-query threshold or timeout-validation settings, and version-gating logic (e.g. "openCypher/feature X requires engine >= 1.4") branches the wrong way off a stale dbEngineVersion/gremlin version. A startTime a date parser rejects breaks uptime computations.
Fix: the GetEngineStatus handler's response rendering and the engine-status record it reads — the feature map must carry the status string itself as the Document value (no wrapper object), the record's feature/labMode/settings key sets must cover the documented keys and derive the auth/feature entries from the cluster configuration, the engine and query-language versions must come from the provisioned cluster's engine version, and the timestamp renderer used for engine-status instants must emit the format Neptune's /status emits.
Traces: neptunedata-query-languages-live-cluster-round-trip#task#18 (ops/get_engine_status.rs:206-247, state.rs:53-65, 112-148, 409-423).

Cause 2 of 3: every neptunedata JSON response omits the charset parameter on Content-Type.
AWS: recorded behavior only (the model states the protocol, not the header parameter): the Neptune data plane answers with `content-type: application/json; charset=UTF-8` on both success and error responses; the recording shows it on GetEngineStatus (200) and on both ExecuteFastReset responses (200 and 400).
Emulator: the same responses carry `content-type: application/json` with no charset parameter.
Why: a client or proxy that selects a decoder from the Content-Type parameters has to guess the encoding, and any test or middleware asserting the documented media type (exact string match on the response content type) fails against the emulator even though the body is right.
Emulator fix: the service's response serializer / the content-type the restJson1 response writer sets for this service, not any single handler — it diverges for every operation.
Traces: neptunedata-query-languages-live-cluster-round-trip#task#27 (ExecuteFastReset 200) and the same header mismatch inside task#18 and task#28.

Cause 3 of 3: a performDatabaseReset with no token is refused with the wrong error class, the wrong message, and an error body whose `code` carries the HTTP status instead of the exception name and which has no `message` member at all.
AWS: Smithy models ExecuteFastReset POST /system -> 200 with input {action (Action enum, required), token (String)} and errors including BadRequestException (httpError 400, "Raised when a request is submitted that cannot be processed") and MissingParameterException (httpError 400); both payloads are {detailedMessage, requestId, code}, all required. Recorded behavior: a performDatabaseReset that carries no token is refused HTTP 400 with the body's `code` = "BadRequestException" (the exception NAME, not the status number), `detailedMessage` = "token is null" AND a `message` member carrying the same text, with `requestId` a per-response value. The message's fixed form is "<missing parameter name> is null" — the only variable part is the parameter name, here `token`, taken from the request's own modelled member; the recording's requestId is a per-run value and is not part of the requirement.
Emulator: answers HTTP 400 but classifies the refusal as MissingParameterException (x-amzn-errortype: MissingParameterException), writes `code` = "400", `detailedMessage` = "Request is missing mandatory parameter 'token'", and emits NO `message` member (so a botocore client surfaces an empty Error.Message).
Why: a client branching on Error.Code gets "400"/"MissingParameterException" where AWS gives "BadRequestException", so fast-reset error handling and retry classification never match; and because the error body has no `message`, the refusal is unactionable in logs and in message-matching tests — the same empty-message defect the open refusal-path issue reports for the Gremlin status family, here on a wire-recorded step that issue does not cover.
Fix: the ExecuteFastReset handler's missing-token branch (choose the class Neptune uses for a performDatabaseReset with no token) together with the shared modelled-error payload builders in this crate's state — they must set `code` to the exception name and populate the member a client reads as the error message, not detailedMessage alone. The `code = "400"`/`"500"` convention is shared by every builder there, so fixing it in one place is what keeps the sibling refusals consistent.
Traces: neptunedata-query-languages-live-cluster-round-trip#task#28 (ops/execute_fast_reset.rs:230-237, state.rs:486-492).

## Evidence
Cause 1 — fixture neptunedata-query-languages-live-cluster-round-trip, step task#18, GetEngineStatus, expected 200 / actual 200, body diverges:
  $.features.SlowQueryLogs: expected 'disabled' (string), got {'status': 'disabled'} (type_mismatch); same for $.features.Streams and $.features.AuditLog
  $.features.InlineServerGeneratedEdgeId: expected 'disabled', got None (missing)
  $.features.IAMAuthentication: expected 'enabled', got None (missing)
  $.labMode.TableSpaceAutoExtendSizeInMB: expected present, got None (missing)
  $.settings.StrictTimeoutValidation: expected 'true', got None; $.settings.SlowQueryLogsThreshold: expected '5000', got None
  $.dbEngineVersion: expected the 1.4.8.0.R1-family released version, got '1.3.2.0'
  $.gremlin.version: expected the tinkerpop-3.7.1-family version, got 'tinkerpop-3.6.2'
  $.startTime: shaped assertion failed, got '2026-11-09T17:45:14.851648Z'
(the recorded 1.4.8.0.R1 / tinkerpop-3.7.1 / '5000' / 'true' values are that recording's engine configuration; the requirement is that the members exist, are the documented types, and report the cluster's actual engine/feature configuration.)

Cause 2 — step task#27, ExecuteFastReset initiateDatabaseReset, expected 200 / actual 200, sole divergence: $headers.content-type expected 'application/json; charset=UTF-8', got 'application/json'. The same header mismatch is reported inside task#18 (GetEngineStatus 200) and task#28 (ExecuteFastReset 400).

Cause 3 — step task#28, ExecuteFastReset performDatabaseReset without a token, expected 400 / actual 400:
  $.code: expected 'BadRequestException', got '400'
  $.message: expected 'token is null', got None (member absent)
  $.detailedMessage: expected 'token is null', got "Request is missing mandatory parameter 'token'"
  $headers.x-amzn-errortype: got 'MissingParameterException', which does not match the recorded error shape
  $.requestId: per-response value, not a requirement (recorded mismatch '18e671845f7cf1af-0000000000000000' is ignorable).

## Steps to Reproduce
1. Replay fixture neptunedata-query-languages-live-cluster-round-trip against the emulator (replay_wire_corpus); it stands up the Neptune cluster and addresses the data plane at the cluster endpoint.
2. Cause 1: step task#18 — GET /status (neptunedata GetEngineStatus) and compare the whole 200 body: the type of every $.features value, the presence of features.IAMAuthentication / features.InlineServerGeneratedEdgeId / labMode.TableSpaceAutoExtendSizeInMB / settings.StrictTimeoutValidation / settings.SlowQueryLogsThreshold, and the format of $.startTime, $.dbEngineVersion and $.gremlin.version.
3. Cause 2: step task#27 — POST /system with action=initiateDatabaseReset and inspect the response's content-type header (also visible on the task#18 and task#28 responses).
4. Cause 3: step task#28 — POST /system with action=performDatabaseReset and no token; inspect the 400 body's code / message / detailedMessage and the x-amzn-errortype header.
5. Re-replay the fixture and confirm task#18, task#27 and task#28 all pass.
