Issue-Source: trace-sweep-divergence
Issue-Trace: gameliftstreams-stream-group-application-association-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: gameliftstreams-stream-group-application-association-lifecycle
Issue-Shape-Id: com.amazonaws.gameliftstreams#GameLiftStreams
Issue-Key: gameliftstreams|GetApplication|2xx->2xx|$.ReplicationStatuses[*]
Issue-Key: gameliftstreams|GetApplication|2xx->2xx|$.StorageBytes
Issue-Key: gameliftstreams|GetApplication|2xx->2xx|$.ReplicationStatuses[*].Status
Issue-Key: gameliftstreams|UntagResource|2xx->2xx|$headers.content-length
Issue-Commit: ae54f48f2b6fca43108843f25838024299d6c3fb

# gameliftstreams application read path omits StorageBytes and replication state, and empty UntagResource response omits Content-Length

> _Note: this issue was identified against the code base at commit `ae54f48f2b6fca43108843f25838024299d6c3fb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetApplication

## Description
THREE net-new root causes in the gameliftstreams application read path and in its empty-body response framing, all found by replaying wire fixture gameliftstreams-stream-group-application-association-lifecycle against the emulator built from this checkout. ALL THREE must be closed before this issue is submitted; each cause names its own fixture step below. The replay report marked none of the three as stub_path or owned_by. Related but NOT covering open issues: 20261103T145438Z-gameliftstreams-stream-group-read-path-drops-capacity-and.md (its cause 3 is the SAME empty-body Content-Length framing defect but is scoped to TagResource, step task#11, and its Issue-Keys list only TagResource; cause 3 here is the UntagResource occurrence, step task#13, which replay reported as unowned) and 20261029T190407Z-gameliftstreams-7-control-plane-fidelity-defects-createapplication-skips.md (its cause 4 is the behavioral-suite finding that a READY application reports an EMPTY ReplicationStatuses; causes 1 and 2 here are the WIRE occurrences of the application read path's replication/StorageBytes projection and add the never-COMPLETED and missing-StorageBytes halves).

=== Cause 1 of 3: GetApplication reports no replication row at all and no StorageBytes for an application whose content is in place (step task#5) ===
AWS: Smithy (services/service-com-amazonaws-gameliftstreams-gameliftstreams/smithy-model/com-amazonaws-gameliftstreams-gameliftstreams.json): GetApplication is GET /applications/{Identifier}, success 200, output GetApplicationOutput, whose member ReplicationStatuses (list of ReplicationStatus) is documented "A set of replication statuses for each location"; ReplicationStatus carries Location (LocationName) and Status (enum ReplicationStatusType, values REPLICATING | COMPLETED), and ReplicationStatus's own doc states "An application cannot be streamed from a location until it has finished replicating there." So an application whose content has been ingested reports at least one location row, and a location whose replication has finished reports Status COMPLETED. The recorded AWS response additionally carries a top-level StorageBytes member reporting the size in bytes of the application's stored content (recorded behavior only: StorageBytes is NOT declared on GetApplicationOutput, CreateApplicationOutput or ApplicationSummary in this on-disk model, so the recording is the only authority for it; the value is the byte size of the content at ApplicationSourceUri, not a fixed number).
Emulator: GetApplication answers 200 with ReplicationStatuses EMPTY — replay reports $.ReplicationStatuses[0] missing (expected a {Location: <region>, Status: COMPLETED} row, got None) — and with no StorageBytes member at all ($.StorageBytes missing, got None; the fixture's content was 17 bytes). By construction: state.rs Application::replication_statuses is documented "Empty for an application no stream group references" and derives rows only from self.replications, which are appended when a stream group adds a location (src/ops/add_stream_group_locations.rs); the Application record holds no content-size field at all, and GetApplicationOutput as the emulator builds it (src/ops/get_application.rs lines 243-258) sets no StorageBytes.
Why: replication state is how a client learns whether its content can be streamed anywhere, and StorageBytes is how it learns the content was ingested and how big it is. With ReplicationStatuses empty, a caller that waits for a COMPLETED location before deploying has nothing to wait on and either loops forever or treats "no locations" as "nothing to check"; with StorageBytes absent, SDK consumers reading response["StorageBytes"] get a KeyError/nil instead of a number, and size-based gating (quota accounting, "is the upload complete" checks) cannot run.
Fix: the application state and the shared GetApplication/CreateApplication/ListApplications read projection — an application whose content is in place must carry its own (home-region) replication row independent of any stream-group association, and the content's byte size must be recorded on the application record and serialized as StorageBytes. Because StorageBytes is absent from this on-disk model, serializing it needs a local output-schema member for the operation (the same technique src/ops/untag_resource.rs already uses to override its output schema) in addition to the state change.
Traces: gameliftstreams-stream-group-application-association-lifecycle#task#5.

=== Cause 2 of 3: once a replication row does exist, it stays REPLICATING after the location is usable, and StorageBytes is still absent (step task#9) ===
AWS: Smithy: ReplicationStatus.Status is the enum ReplicationStatusType {REPLICATING, COMPLETED}; replication to a location is finished — and therefore COMPLETED — by the time that location can stream, per ReplicationStatus's documentation ("An application cannot be streamed from a location until it has finished replicating there"). The recording, taken at the point in the lifecycle where the stream group and its location have been brought up, shows ReplicationStatuses[0].Status == COMPLETED (and, again, a top-level StorageBytes — recorded behavior only, as in cause 1).
Emulator: GetApplication answers 200 with $.ReplicationStatuses[0].Status == 'REPLICATING' where the recording has 'COMPLETED', and still omits $.StorageBytes entirely. state.rs Application::replication_statuses reports a row COMPLETED only once now >= replication.completes_at, i.e. replication completion is gated on an independent emulator timer rather than on the location actually becoming usable, so the group/location can read as brought up while the application still reports REPLICATING. (Cause 1 is the row being absent; this is the row never advancing — the first is the projection's existence, the second its state machine, and both must be closed.)
Why: a client that polls GetApplication until every ReplicationStatuses entry is COMPLETED before starting a stream session never converges, even though the stream group reports itself usable; the two halves of the emulator's state disagree, so correct AWS-shaped orchestration code deadlocks against it.
Fix: the application's replication state machine in state.rs (the replication rows and Application::replication_statuses) — a location's replication must report COMPLETED once that location is activated/usable for the group that asked for it, rather than on a timer unrelated to the location's own activation, and the same read projection must carry StorageBytes (cause 1).
Traces: gameliftstreams-stream-group-application-association-lifecycle#task#9.

=== Cause 3 of 3: UntagResource's empty success response omits the Content-Length header (step task#13) ===
AWS: Smithy: UntagResource is DELETE /tags/{ResourceArn} with output shape UntagResourceResponse, a structure with NO members (the model's @http code is defaulted). AWS frames such an empty REST-JSON success with a Content-Length header describing the body it sent, and the recording asserts only that the header is PRESENT and shaped ('<shaped-not-asserted>'), not any particular numeric value — so the requirement is the presence of Content-Length on the empty-body success, not a specific length.
Emulator: the status (204) matched the recording, but the response carries no Content-Length header at all (replay: $headers.content-length missing, got None). src/ops/untag_resource.rs frames the success as 204 with an empty body via its UNTAG_RESOURCE_NO_CONTENT output schema and emits no length header.
Why: an HTTP client that frames a response by Content-Length and receives neither it nor chunked framing must read until close or treat the message as unterminated; on keep-alive connections this stalls or desynchronizes the connection, and strict clients and proxies reject the response, so a tag removal the emulator considers successful surfaces to the caller as a transport error.
Fix: the shared response serializer for operations whose output shape has no members — emit Content-Length for the empty body it writes. This is the same framing defect already filed for TagResource as cause 3 of 20261103T145438Z-gameliftstreams-stream-group-read-path-drops-capacity-and.md; that issue's replay keys cover only TagResource, so the UntagResource occurrence is recorded here and a serializer-level fix closes both.
Traces: gameliftstreams-stream-group-application-association-lifecycle#task#13.

## Evidence
Replay of wire trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up=true (/__admin/health 200), 48 steps total, 31 passed, 17 mismatched, score 64.6%. All three causes come from fixture gameliftstreams-stream-group-application-association-lifecycle; none carried stub_path or owned_by.

Cause 1 of 3 — fixture gameliftstreams-stream-group-application-association-lifecycle, step task#5, GetApplication, verdict mismatch, expected status 200 / actual 200:
  $.ReplicationStatuses[0]: missing (expected {'Location': 'us-west-2', 'Status': 'COMPLETED'}, got None)
  $.StorageBytes: missing (expected 17, got None)
  The location name and the byte count are this run's values (the fixture's region and its 17-byte content object); the requirement is that a replication row for the application's location is reported with Status COMPLETED and that StorageBytes reports the stored content's size at all.

Cause 2 of 3 — same fixture, step task#9, GetApplication, verdict mismatch, expected status 200 / actual 200:
  $.ReplicationStatuses[0].Status: value_mismatch (expected 'COMPLETED', got 'REPLICATING')
  $.StorageBytes: missing (expected 17, got None)

Cause 3 of 3 — same fixture, step task#13, UntagResource, verdict mismatch, expected status 204 / actual 204:
  $headers.content-length: missing (expected '<shaped-not-asserted>', got None)
  Status and the empty body matched; only the Content-Length framing header is absent. The sibling TagResource occurrence (step task#11) is reported by replay as owned by 20261103T145438Z-gameliftstreams-stream-group-read-path-drops-capacity-and.md; this UntagResource occurrence is not.

Model evidence (services/service-com-amazonaws-gameliftstreams-gameliftstreams/smithy-model/com-amazonaws-gameliftstreams-gameliftstreams.json): GetApplication GET /applications/{Identifier} code 200, output GetApplicationOutput with members Arn (required), Description, RuntimeEnvironment, ExecutablePath, ApplicationLogPaths, ApplicationLogOutputUri, ApplicationSourceUri, Id, Status, StatusReason, ReplicationStatuses, CreatedAt, LastUpdatedAt, AssociatedStreamGroups — NO StorageBytes member is declared here, nor on CreateApplicationOutput or ApplicationSummary, so StorageBytes is recorded behavior only. ReplicationStatus = {Location: LocationName, Status: ReplicationStatusType}, ReplicationStatusType values REPLICATING | COMPLETED, documented "An application cannot be streamed from a location until it has finished replicating there." UntagResource DELETE /tags/{ResourceArn}, output shape UntagResourceResponse with no members, @http code defaulted.

Emulator source: src/ops/get_application.rs lines 243-258 build GetApplicationOutput with replication_statuses: Some(application.replication_statuses(now)) and no StorageBytes; src/state.rs Application::replication_statuses (around line 687) is documented "Empty for an application no stream group references" and reports COMPLETED only when now >= replication.completes_at; src/ops/untag_resource.rs lines 99-113 and 173 frame the success as 204 with an empty body (UNTAG_RESOURCE_NO_CONTENT) with no length header.

## Steps to Reproduce
Boot the emulator built from this checkout and replay wire fixture gameliftstreams-stream-group-application-association-lifecycle (steps task#5, task#9, task#13), or by hand:
1. Create an S3 bucket and put a small content object under a prefix, then CreateApplication(Description=<d>, RuntimeEnvironment={UBUNTU,22_04_LTS}, ExecutablePath=<exe>, ApplicationSourceUri="s3://<bucket>/<prefix>/") and poll GetApplication until Status == READY.
2. Cause 1: GetApplication(Identifier=<application ARN>) — observe ReplicationStatuses == [] (no location row) and no StorageBytes member in the body, where AWS reports a row {Location: <region>, Status: COMPLETED} and StorageBytes == the byte size of the content at ApplicationSourceUri.
3. CreateStreamGroup(LocationConfigurations=[{LocationName=<region>, AlwaysOnCapacity=2}]), AssociateApplications(Identifier=<group ARN>, ApplicationIdentifiers=[<application ARN>]), and poll GetStreamGroup until the group/location reads brought up (ACTIVE).
4. Cause 2: GetApplication(Identifier=<application ARN>) again — observe ReplicationStatuses[0].Status == 'REPLICATING' while the group's location already reads usable, where AWS reports 'COMPLETED'; StorageBytes is still absent.
5. Cause 3: TagResource(ResourceArn=<group ARN>, Tags={<k>: <v>}) then UntagResource(ResourceArn=<group ARN>, TagKeys=[<k>]) and inspect the raw response headers — observe status 204 with NO Content-Length header on the empty-body success, where AWS frames the empty response with Content-Length.
