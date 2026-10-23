Issue-Source: trace-sweep-divergence
Issue-Trace: gameliftstreams-application-error-paths
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: gameliftstreams-application-error-paths gameliftstreams-session-and-url-error-paths gameliftstreams-stream-group-error-paths
Issue-Shape-Id: com.amazonaws.gameliftstreams#GameLiftStreams
Issue-Key: gameliftstreams|GetApplication|4xx->4xx|$.Message
Issue-Key: gameliftstreams|DeleteApplication|4xx->4xx|$.Message
Issue-Key: gameliftstreams|GetStreamSession|4xx->4xx|$.Message
Issue-Key: gameliftstreams|TerminateStreamSession|4xx->4xx|$.Message
Issue-Key: gameliftstreams|StartStreamSession|4xx->4xx|$.Message
Issue-Key: gameliftstreams|GetStreamGroup|4xx->4xx|$.Message
Issue-Key: gameliftstreams|UpdateStreamGroup|4xx->4xx|$.Message
Issue-Key: gameliftstreams|AddStreamGroupLocations|4xx->4xx|$.Message
Issue-Key: gameliftstreams|DeleteStreamGroup|4xx->4xx|$.Message
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# GameLift Streams wire replay: ResourceNotFoundException messages omit resolved ARN and Internal ID suffix across 9 operations

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetApplication, DeleteApplication, GetStreamGroup, UpdateStreamGroup, AddStreamGroupLocations, DeleteStreamGroup, StartStreamSession, GetStreamSession, TerminateStreamSession

## Description
Wire corpus simparity-wire-aws-v1 replayed 19 steps across 3 gameliftstreams fixtures: 0 passed, 19 mismatched, clustered into 9 divergences. Every one of the 9 is a ResourceNotFoundException whose HTTP status (404) and error code are CORRECT but whose $.Message does not follow the service's message-construction rule. There are NINE causes below, one per operation's not-found path, and all nine must be closed before this issue is submitted.

SHARED RULE (cited once, applies to every cause). AWS's GameLift Streams builds a not-found message from the REQUEST, in two parts:
  (a) it resolves the request's bare identifier to the full resource ARN and names that ARN, in the form "Resource '<arn>' does not exist" for resource-scoped operations, or "Stream session '<arn>' does not exist" for the session-scoped ones;
  (b) it appends a per-request suffix " (Internal ID: <uuid>)".
The uuid VALUE is per-request and is NOT a requirement; the two structural rules are. Smithy types ResourceNotFoundException with a single required Message member and @httpError 404 and says nothing about its text, so the text rule itself is recorded behavior only — but it is a rule built from the request (the resolved ARN of the identifier the caller passed), not an opaque server string, which is why it is a requirement here and not merely an example. The same rule is independently documented as Cause 3 of the open behavioral issue 20261022T092721Z-gameliftstreams-gamelift-streams-7-control-plane-fidelity-defects-missing.md; that issue was filed from the behavioral suite, carries no wire fixture/step linkage, and names only CreateStreamGroup and GetStreamGroup, so it does not cover and cannot gate the nine wire paths below.

Across all nine causes the emulator instead emits an ad-hoc message of the form "no <resource kind> matches identifier <bare identifier>": it never resolves the identifier to an ARN and never appends the internal-id suffix.

=== Cause 1 of 9: GetApplication ===
AWS: Smithy GetApplication, GET /applications/{Identifier}, success 200, errors include ResourceNotFoundException @httpError 404; Identifier is @httpLabel @required, length 1..128, pattern ^(^[a-zA-Z0-9-]+$)|(^arn:aws:gameliftstreams:([^:\n]*):([0-9]{12}):([^:\n]*)$)$ — so a bare id like a-9ZY8X7Wv9 is legal input and must be resolved to an ARN for the message. Recorded 404 ResourceNotFoundException, message e.g. "Resource 'arn:aws:gameliftstreams:us-west-2:000000000000:application/a-9ZY8X7Wv9' does not exist (Internal ID: c4defd5c-5a87-44d5-8d02-a07b9eabd59e)" (example text; the requirement is the resolved application ARN plus the internal-id suffix).
Emulator: 404 ResourceNotFoundException with message 'no application matches identifier a-9ZY8X7Wv9'.
Why: a client that parses the message to recover which ARN was missing gets nothing parseable, and a client that strips the internal-id suffix before comparing or logging finds no suffix to strip.
Fix: the shared not-found error construction used by the GetApplication handler in the gameliftstreams service crate — one helper that resolves identifier -> ARN and appends the internal-id suffix.
Traces: gameliftstreams-application-error-paths#task#1.

=== Cause 2 of 9: DeleteApplication ===
AWS: Smithy DeleteApplication, DELETE /applications/{Identifier}, success 204, @idempotent, errors include ResourceNotFoundException 404. Recorded 404 ResourceNotFoundException naming the resolved application ARN, e.g. "Resource 'arn:aws:gameliftstreams:us-west-2:000000000000:application/a-9ZY8X7Wv9' does not exist (Internal ID: 68624b84-b373-47ac-904f-5cd18faec682)".
Emulator: 404 ResourceNotFoundException with message 'no application matches identifier a-9ZY8X7Wv9'.
Why: same as Cause 1 — delete-then-confirm flows that key off the ARN in the message cannot tell which resource the service meant.
Fix: same shared not-found helper, called from the DeleteApplication handler.
Traces: gameliftstreams-application-error-paths#task#4.

=== Cause 3 of 9: GetStreamGroup ===
AWS: Smithy GetStreamGroup, GET /streamgroups/{Identifier}, success 200, errors include ResourceNotFoundException 404. Recorded 404 naming the resolved stream-group ARN, e.g. "Resource 'arn:aws:gameliftstreams:us-west-2:000000000000:streamgroup/sg-1AB2C3De9' does not exist (Internal ID: f9bf2d79-e915-48b8-afbe-bcd36f2e3984)".
Emulator: 404 ResourceNotFoundException with message 'no stream group matches identifier sg-1AB2C3De9'.
Why: as above; this is the single most common not-found path in the API, so the wrong format propagates into most client error handling.
Fix: same shared not-found helper, called from the GetStreamGroup handler.
Traces: gameliftstreams-stream-group-error-paths#task#1.

=== Cause 4 of 9: UpdateStreamGroup ===
AWS: Smithy UpdateStreamGroup, PATCH /streamgroups/{Identifier}, success 200, errors include ResourceNotFoundException 404 (also ConflictException 409, ServiceQuotaExceededException 402). Recorded 404 naming the resolved stream-group ARN, e.g. "Resource 'arn:aws:gameliftstreams:us-west-2:000000000000:streamgroup/sg-1AB2C3De9' does not exist (Internal ID: a374c67b-cac8-4de7-aa8c-7b313d1c55b3)".
Emulator: 404 ResourceNotFoundException with message 'no stream group matches identifier sg-1AB2C3De9'.
Why: as above. Note this is a DIFFERENT defect from the resolved stub issue 20261022T092721Z-gameliftstreams-updatestreamgroup-stub.md: the operation now answers, but its not-found message is wrong.
Fix: same shared not-found helper, called from the UpdateStreamGroup handler.
Traces: gameliftstreams-stream-group-error-paths#task#2.

=== Cause 5 of 9: AddStreamGroupLocations ===
AWS: Smithy AddStreamGroupLocations, POST /streamgroups/{Identifier}/locations, success 200, errors include ResourceNotFoundException 404. Recorded 404 naming the resolved stream-group ARN, e.g. "Resource 'arn:aws:gameliftstreams:us-west-2:000000000000:streamgroup/sg-1AB2C3De9' does not exist (Internal ID: 767f219d-2a1a-4511-8243-853d03a8dd91)".
Emulator: 404 ResourceNotFoundException with message 'no stream group matches identifier sg-1AB2C3De9'.
Why: as above. Distinct from the resolved stub issue 20261022T092721Z-gameliftstreams-addstreamgrouplocations-stub.md — the stub is implemented, the message format is not.
Fix: same shared not-found helper, called from the AddStreamGroupLocations handler.
Traces: gameliftstreams-stream-group-error-paths#task#3.

=== Cause 6 of 9: DeleteStreamGroup ===
AWS: Smithy DeleteStreamGroup, DELETE /streamgroups/{Identifier}, success 204, @idempotent, errors include ResourceNotFoundException 404. Recorded 404 naming the resolved stream-group ARN, e.g. "Resource 'arn:aws:gameliftstreams:us-west-2:000000000000:streamgroup/sg-1AB2C3De9' does not exist (Internal ID: 00bdec4a-a2a0-401b-be0f-02956b6c95dc)".
Emulator: 404 ResourceNotFoundException with message 'no stream group matches identifier sg-1AB2C3De9'.
Why: as above.
Fix: same shared not-found helper, called from the DeleteStreamGroup handler.
Traces: gameliftstreams-stream-group-error-paths#task#5.

=== Cause 7 of 9: StartStreamSession ===
AWS: Smithy StartStreamSession, POST /streamgroups/{Identifier}/streamsessions, success 201, errors include ResourceNotFoundException 404. When the stream group in the path does not exist, the recorded 404 names the resolved STREAM GROUP ARN, e.g. "Resource 'arn:aws:gameliftstreams:us-west-2:000000000000:streamgroup/sg-1AB2C3De9' does not exist (Internal ID: 06bde091-0248-4d13-afba-e0096ea7039a)" — i.e. the group is the resource reported for the create path.
Emulator: 404 ResourceNotFoundException with message 'no stream group matches identifier sg-1AB2C3De9'.
Why: as above; the resource kind is right here but the ARN resolution and the suffix are still missing.
Fix: same shared not-found helper, called from the StartStreamSession handler's stream-group resolution.
Traces: gameliftstreams-session-and-url-error-paths#task#7.

=== Cause 8 of 9: GetStreamSession reports the wrong resource KIND, not just the wrong format ===
AWS: Smithy GetStreamSession, GET /streamgroups/{Identifier}/streamsessions/{StreamSessionIdentifier}, success 200, errors include ResourceNotFoundException 404; both Identifier and StreamSessionIdentifier are @httpLabel @required. For a session-scoped operation the recorded 404 names the composite STREAM SESSION ARN built from BOTH path labels — e.g. "Stream session 'arn:aws:gameliftstreams:us-west-2:000000000000:streamsession/sg-1AB2C3De9/ABC123def4568' does not exist (Internal ID: 4b4975e2-577d-47d0-9f99-6365981a0646)" — even when it is the enclosing stream group that is absent. So the requirement is twofold: the "Stream session '<arn>' does not exist" wording and a streamsession/<group>/<session> ARN composed from both labels.
Emulator: 404 ResourceNotFoundException with message 'no stream group matches identifier sg-1AB2C3De9' — it short-circuits on the enclosing group and reports a stream GROUP miss, naming a resource of the wrong kind and dropping the session identifier entirely.
Why: a client cannot distinguish "this session does not exist" from "this group does not exist" the way AWS presents it, and the session identifier it passed never appears in the error at all, so reconnect/polling logic that echoes the failing session loses it.
Fix: the GetStreamSession handler's identifier resolution — compose the streamsession ARN from both path labels and raise the session-scoped not-found through the shared helper, rather than failing out of the group lookup first.
Traces: gameliftstreams-session-and-url-error-paths#task#2.

=== Cause 9 of 9: TerminateStreamSession reports the wrong resource KIND, not just the wrong format ===
AWS: Smithy TerminateStreamSession, DELETE /streamgroups/{Identifier}/streamsessions/{StreamSessionIdentifier}, success 204, @idempotent, errors include ResourceNotFoundException 404. Same session-scoped rule as Cause 8: recorded 404 names the composite stream session ARN, e.g. "Stream session 'arn:aws:gameliftstreams:us-west-2:000000000000:streamsession/sg-1AB2C3De9/ABC123def4568' does not exist (Internal ID: 50302c54-27e1-46b0-b33a-32c17858528e)".
Emulator: 404 ResourceNotFoundException with message 'no stream group matches identifier sg-1AB2C3De9'.
Why: as Cause 8 — teardown code that logs or retries on the failing session ARN has no session ARN to work with.
Fix: the TerminateStreamSession handler's identifier resolution, through the same shared session-scoped not-found helper.
Traces: gameliftstreams-session-and-url-error-paths#task#3.

## Evidence
Replay summary (trace_set simparity-wire-aws-v1): emulator_up=true, bootstrap_signal=false, state_reset_failures=0, fixtures_replayed=3, steps_total=19, steps_passed=0, steps_mismatched=19, steps_errored=0, steps_blocked=0, divergence_count=9, score_pct=0.0. (not_implemented=10 is reported separately and is covered by the scaffold-stub issues, not by this one.)

Every divergence below has expected_status=404 and actual_status=404 with the correct ResourceNotFoundException code; the only diverging key is $.Message.

Cause 1 — gameliftstreams-application-error-paths#task#1, GetApplication
  divergence_key: service=gameliftstreams|GetApplication|status=404->404|keys=$.Message
  expected $.Message: "Resource 'arn:aws:gameliftstreams:us-west-2:000000000000:application/a-9ZY8X7Wv9' does not exist (Internal ID: c4defd5c-5a87-44d5-8d02-a07b9eabd59e)"
  actual   $.Message: 'no application matches identifier a-9ZY8X7Wv9'

Cause 2 — gameliftstreams-application-error-paths#task#4, DeleteApplication
  divergence_key: service=gameliftstreams|DeleteApplication|status=404->404|keys=$.Message
  expected $.Message: "Resource 'arn:aws:gameliftstreams:us-west-2:000000000000:application/a-9ZY8X7Wv9' does not exist (Internal ID: 68624b84-b373-47ac-904f-5cd18faec682)"
  actual   $.Message: 'no application matches identifier a-9ZY8X7Wv9'

Cause 3 — gameliftstreams-stream-group-error-paths#task#1, GetStreamGroup
  divergence_key: service=gameliftstreams|GetStreamGroup|status=404->404|keys=$.Message
  expected $.Message: "Resource 'arn:aws:gameliftstreams:us-west-2:000000000000:streamgroup/sg-1AB2C3De9' does not exist (Internal ID: f9bf2d79-e915-48b8-afbe-bcd36f2e3984)"
  actual   $.Message: 'no stream group matches identifier sg-1AB2C3De9'

Cause 4 — gameliftstreams-stream-group-error-paths#task#2, UpdateStreamGroup
  divergence_key: service=gameliftstreams|UpdateStreamGroup|status=404->404|keys=$.Message
  expected $.Message: "Resource 'arn:aws:gameliftstreams:us-west-2:000000000000:streamgroup/sg-1AB2C3De9' does not exist (Internal ID: a374c67b-cac8-4de7-aa8c-7b313d1c55b3)"
  actual   $.Message: 'no stream group matches identifier sg-1AB2C3De9'

Cause 5 — gameliftstreams-stream-group-error-paths#task#3, AddStreamGroupLocations
  divergence_key: service=gameliftstreams|AddStreamGroupLocations|status=404->404|keys=$.Message
  expected $.Message: "Resource 'arn:aws:gameliftstreams:us-west-2:000000000000:streamgroup/sg-1AB2C3De9' does not exist (Internal ID: 767f219d-2a1a-4511-8243-853d03a8dd91)"
  actual   $.Message: 'no stream group matches identifier sg-1AB2C3De9'

Cause 6 — gameliftstreams-stream-group-error-paths#task#5, DeleteStreamGroup
  divergence_key: service=gameliftstreams|DeleteStreamGroup|status=404->404|keys=$.Message
  expected $.Message: "Resource 'arn:aws:gameliftstreams:us-west-2:000000000000:streamgroup/sg-1AB2C3De9' does not exist (Internal ID: 00bdec4a-a2a0-401b-be0f-02956b6c95dc)"
  actual   $.Message: 'no stream group matches identifier sg-1AB2C3De9'

Cause 7 — gameliftstreams-session-and-url-error-paths#task#7, StartStreamSession
  divergence_key: service=gameliftstreams|StartStreamSession|status=404->404|keys=$.Message
  expected $.Message: "Resource 'arn:aws:gameliftstreams:us-west-2:000000000000:streamgroup/sg-1AB2C3De9' does not exist (Internal ID: 06bde091-0248-4d13-afba-e0096ea7039a)"
  actual   $.Message: 'no stream group matches identifier sg-1AB2C3De9'

Cause 8 — gameliftstreams-session-and-url-error-paths#task#2, GetStreamSession
  divergence_key: service=gameliftstreams|GetStreamSession|status=404->404|keys=$.Message
  expected $.Message: "Stream session 'arn:aws:gameliftstreams:us-west-2:000000000000:streamsession/sg-1AB2C3De9/ABC123def4568' does not exist (Internal ID: 4b4975e2-577d-47d0-9f99-6365981a0646)"
  actual   $.Message: 'no stream group matches identifier sg-1AB2C3De9'  <- wrong resource kind, session identifier absent

Cause 9 — gameliftstreams-session-and-url-error-paths#task#3, TerminateStreamSession
  divergence_key: service=gameliftstreams|TerminateStreamSession|status=404->404|keys=$.Message
  expected $.Message: "Stream session 'arn:aws:gameliftstreams:us-west-2:000000000000:streamsession/sg-1AB2C3De9/ABC123def4568' does not exist (Internal ID: 50302c54-27e1-46b0-b33a-32c17858528e)"
  actual   $.Message: 'no stream group matches identifier sg-1AB2C3De9'  <- wrong resource kind, session identifier absent

Model references used (services/service-com-amazonaws-gameliftstreams-gameliftstreams/smithy-model/com-amazonaws-gameliftstreams-gameliftstreams.json): ResourceNotFoundException @httpError 404 with one required Message, bound to all nine operations; GetApplication GET /applications/{Identifier} 200; DeleteApplication DELETE /applications/{Identifier} 204 @idempotent; GetStreamGroup GET /streamgroups/{Identifier} 200; UpdateStreamGroup PATCH /streamgroups/{Identifier} 200; AddStreamGroupLocations POST /streamgroups/{Identifier}/locations 200; DeleteStreamGroup DELETE /streamgroups/{Identifier} 204 @idempotent; StartStreamSession POST /streamgroups/{Identifier}/streamsessions 201; GetStreamSession GET /streamgroups/{Identifier}/streamsessions/{StreamSessionIdentifier} 200; TerminateStreamSession DELETE /streamgroups/{Identifier}/streamsessions/{StreamSessionIdentifier} 204 @idempotent; Identifier @httpLabel @required length 1..128 pattern ^(^[a-zA-Z0-9-]+$)|(^arn:aws:gameliftstreams:([^:\n]*):([0-9]{12}):([^:\n]*)$)$ (bare ids are legal and must be resolved to an ARN in the message).

## Steps to Reproduce
Replay the wire corpus against a freshly booted emulator: replay_wire_corpus() over trace set simparity-wire-aws-v1, fixtures gameliftstreams-application-error-paths, gameliftstreams-stream-group-error-paths and gameliftstreams-session-and-url-error-paths. The representative step is gameliftstreams-application-error-paths#task#1; the other eight causes are the fixture#step pairs in trace_ids. All nine must be re-replayed before this issue is closed.

Equivalently, by hand with a botocore gameliftstreams client in region us-west-2 pointed at the emulator, using identifiers that do not exist:

Cause 1: get_application(Identifier='a-9ZY8X7Wv9')
Cause 2: delete_application(Identifier='a-9ZY8X7Wv9')
Cause 3: get_stream_group(Identifier='sg-1AB2C3De9')
Cause 4: update_stream_group(Identifier='sg-1AB2C3De9', Description='d')
Cause 5: add_stream_group_locations(Identifier='sg-1AB2C3De9', LocationConfigurations=[{'LocationName':'us-west-2','AlwaysOnCapacity':1}])
Cause 6: delete_stream_group(Identifier='sg-1AB2C3De9')
Cause 7: start_stream_session(Identifier='sg-1AB2C3De9', Protocol='WebRTC', SignalRequest='{"type":"offer","sdp":"v=0"}', ApplicationIdentifier='a-9ZY8X7Wv9')
Cause 8: get_stream_session(Identifier='sg-1AB2C3De9', StreamSessionIdentifier='ABC123def4568')
Cause 9: terminate_stream_session(Identifier='sg-1AB2C3De9', StreamSessionIdentifier='ABC123def4568')

In each case the HTTP status is 404 and the code is ResourceNotFoundException (correct); inspect Error.Message. Expected shape: "Resource '<resolved arn>' does not exist (Internal ID: <uuid>)" for Causes 1-7, and "Stream session '<arn:aws:gameliftstreams:<region>:<account>:streamsession/<group>/<session>>' does not exist (Internal ID: <uuid>)" for Causes 8-9. Observed: 'no application matches identifier <id>' / 'no stream group matches identifier <id>' — no resolved ARN, no internal-id suffix, and for Causes 8-9 the wrong resource kind with the session identifier omitted.
