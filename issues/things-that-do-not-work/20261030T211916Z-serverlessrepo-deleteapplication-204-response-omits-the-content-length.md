Issue-Source: trace-sweep-divergence
Issue-Trace: serverlessapplicationrepository-application-lifecycle
Issue-Step: task#6
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: serverlessapplicationrepository-application-lifecycle serverlessapplicationrepository-application-policy serverlessapplicationrepository-application-version-lifecycle serverlessapplicationrepository-cloudformation-template-preparation
Issue-Shape-Id: com.amazonaws.serverlessapplicationrepository#ServerlessApplicationRepository
Issue-Key: serverlessrepo|DeleteApplication|2xx->2xx|$headers.content-length
Issue-Commit: c0ce0118fd7c38fba0f2e9d7692bcc75f2ecf7c6

# serverlessrepo DeleteApplication 204 response omits the Content-Length: 0 header

> _Note: this issue was identified against the code base at commit `c0ce0118fd7c38fba0f2e9d7692bcc75f2ecf7c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteApplication

## Description
This bundle carries 1 CAUSE and it must be closed before submitting.

Cause 1 of 1: the empty-body success response of DeleteApplication is sent with no Content-Length header, where AWS frames it explicitly as Content-Length: 0 (fixture serverlessapplicationrepository-application-lifecycle, step task#6).

AWS: Smithy (services/service-com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository/smithy-model/...json): DeleteApplication, http {method: DELETE, uri: /applications/{ApplicationId}, code: 204}, input DeleteApplicationRequest with only the @required httpLabel member ApplicationId, output shape smithy.api#Unit — i.e. a modeled bodyless success. The model states nothing about response headers, so for the header itself this is recorded behavior only: the recording shows SAR answering the delete with status 204 and an explicit `Content-Length: 0` header (no body, length declared as zero). There is no error code or error message involved — the step is a success on both sides.

Emulator: answers the correct status 204 with the correct empty body, but emits no `Content-Length` header at all ($headers.content-length: missing — expected '0', got None). The divergence is purely in response framing and is identical on every DeleteApplication step in the corpus, so it is the general behavior of the empty-body success path, not a property of one recording. Nothing in the recorded header value is per-run: the required value is the literal `0`.

Why: a client that reads the declared entity length to decide how to finish the exchange gets no declaration. On a 204 most clients fall back to "no body by status", but any consumer that requires explicit framing — a strict HTTP/1.1 proxy or gateway in front of the emulator, a load-balancer that logs/validates response length, an SDK or test harness asserting on the response headers AWS actually returns, or a connection-reuse path that keys off the declared length — sees a response AWS never sends and can treat the message as unframed or the connection as unreusable. It also means header-level fidelity assertions against this operation fail even though the status and body are right.

Fix: the response serializer / HTTP framing layer for bodyless (204) success responses, reached through DeleteApplication's OUTPUT_SCHEMA (DeleteApplicationOutput::SCHEMA, declared with_http(..., Some(204)) and SUCCESS_STATUS = 204 in src/ops/delete_application.rs, whose handler itself is correct and returns the empty DeleteApplicationOutput). The serializer must set `Content-Length: 0` when it serializes a success whose output shape is smithy.api#Unit / has no bound body members, rather than omitting the header; the same path serves every 204-modeled operation of this service (UnshareApplication is modeled 204 as well), so the fix belongs in the shared empty-body response path and not in this one handler.

Traces: serverlessapplicationrepository-application-lifecycle#task#6 is the representative; the identical divergence was recorded on serverlessapplicationrepository-application-policy#task#12, serverlessapplicationrepository-application-version-lifecycle#task#9 and serverlessapplicationrepository-cloudformation-template-preparation#task#6 — all members of the same cluster (service=serverlessrepo|DeleteApplication|status=204->204|keys=$headers.content-length), so replaying the representative exercises them all.

Related but NOT covering: the open issue 20261030T010555Z-serverlessrepo-version-and-template-responses-report-an-unfollowable.md (templateUrl / parameterDefinitions / template status body divergences) and 20261022T011815Z-serverlessrepo-version-publish-and-template-generation-fixtures-404.md (semantic-version error wording). Neither touches response framing on the delete path and neither carries this cluster's replay key.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (emulator_up true, /__admin/health answered 200, data listener accepting): 7 divergence clusters, of which this is the only one neither blocked by an unimplemented operation nor owned by an open issue.

Cluster: verdict=mismatch, service=serverlessrepo, operation=DeleteApplication, divergence_key=service=serverlessrepo|DeleteApplication|status=204->204|keys=$headers.content-length, affected_count=4.
Representative step: fixture serverlessapplicationrepository-application-lifecycle, step task#6.
Expected status 204, actual status 204 — the status and the empty body match.
Detail, verbatim: `$headers.content-length: missing (expected '0', got None)`.
Other cluster members showing the identical detail: serverlessapplicationrepository-application-policy#task#12, serverlessapplicationrepository-application-version-lifecycle#task#9, serverlessapplicationrepository-cloudformation-template-preparation#task#6.
No stub_path (operation is implemented) and no owned_by for this cluster.

Model evidence: smithy_query(query="operation", operation="DeleteApplication") reports output_shape "Unit", http {"method":"DELETE","uri":"/applications/{ApplicationId}","code":204}.
Code: services/service-com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository/src/ops/delete_application.rs lines 62-74 (DeleteApplicationOutput::SCHEMA, Unit struct with_http("DELETE", "/applications/{ApplicationId}", Some(204))) and line 140 (SUCCESS_STATUS = 204u16); the handler at lines 142-184 returns the empty DeleteApplicationOutput, so no Content-Length is ever attached to the serialized 204.

## Steps to Reproduce
1. Boot the emulator from this checkout and confirm /__admin/health answers 200.
2. Replay fixture serverlessapplicationrepository-application-lifecycle up to step task#6 (CreateApplication, then DELETE /applications/{ApplicationId} for the application created earlier in the fixture).
3. Inspect the raw response headers of that DELETE: the status is 204 and the body is empty, but there is no `Content-Length` header. AWS's recording for the same step carries `Content-Length: 0`.
4. Repeat with any other DeleteApplication step in the corpus — serverlessapplicationrepository-application-policy#task#12, serverlessapplicationrepository-application-version-lifecycle#task#9 or serverlessapplicationrepository-cloudformation-template-preparation#task#6 — and observe the same missing header, confirming it is the empty-body success path and not one fixture.
5. After the fix, every 204 success of this service must declare `Content-Length: 0`.
