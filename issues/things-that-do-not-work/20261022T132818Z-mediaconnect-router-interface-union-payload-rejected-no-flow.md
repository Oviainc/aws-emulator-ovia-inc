Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mediaconnect.router-input-lifecycle mediaconnect.router-output-lifecycle mediaconnect.take-router-input-routes-and-unroutes mediaconnect.take-router-input-routing-scope-mismatch-rejected mediaconnect.flow-state-change-eventbridge-pattern-match-delivers mediaconnect.flow-state-change-eventbridge-pattern-mismatch-no-delivery mediaconnect.eventbridge-missing-queue-policy-blocks-delivery mediaconnect.describe-flow-source-metadata-not-yet-available mediaconnect.remove-only-flow-source-rejected mediaconnect.native-media-ingest-udp-round-trip mediaconnect.describe-flow-not-found mediaconnect.batch-get-router-input-arn-limit-exceeded-rejected mediaconnect.start-flow-missing-source-rejected
Issue-Shape-Id: com.amazonaws.mediaconnect#MediaConnect
Issue-Key: mediaconnect|CreateRouterNetworkInterface|ValidationException
Issue-Key: mediaconnect.flow-state-change-eventbridge-pattern-match-delivers|_poll_for_messages#16|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: mediaconnect.flow-state-change-eventbridge-pattern-mismatch-no-delivery|_poll_for_messages#25|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: mediaconnect.eventbridge-missing-queue-policy-blocks-delivery|_poll_for_messages#21|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: mediaconnect.describe-flow-source-metadata-not-yet-available|run#5|AssertionError|assert code == "BadRequestException", (
Issue-Key: mediaconnect.remove-only-flow-source-rejected|run#9|AssertionError|assert error.get("Code") == "BadRequestException", (
Issue-Key: mediaconnect.native-media-ingest-udp-round-trip|_poll_for_peer_ip#11|Timeout|time.sleep(3)
Issue-Key: mediaconnect.describe-flow-not-found|run#8|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: mediaconnect.batch-get-router-input-arn-limit-exceeded-rejected|run#8|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: mediaconnect.start-flow-missing-source-rejected|run#7|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: b4ac6d961cdb10c212ccdfac1172ee263c3e4aa3

# MediaConnect: router-interface union payload rejected, no flow-status EventBridge events, two 501 stubs, no native UDP ingest, and non-AWS error-message envelope

> _Note: this issue was identified against the code base at commit `b4ac6d961cdb10c212ccdfac1172ee263c3e4aa3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRouterNetworkInterface

## Description
Six distinct root causes found replaying the behavioral suite against the MediaConnect emulator (2/28 checks pass). All six belong to service `mediaconnect`. Causes marked ALREADY FILED by the replay report (AddFlowOutputs, UpdateFlow, ListGateways, TagResource, GrantFlowEntitlements, AddFlowMediaStreams, and cloudwatch DescribeAlarmHistory) are NOT included here.

=== Cause 1 of 6: CreateRouterNetworkInterface rejects a validly-populated `Configuration` union before the handler runs ===
AWS: Smithy model `services/service-com-amazonaws-mediaconnect-mediaconnect/smithy-model/com-amazonaws-mediaconnect-mediaconnect.json` — CreateRouterNetworkInterface (POST /v1/routerNetworkInterface, success 201) takes required member `Configuration` targeting the union `RouterNetworkInterfaceConfiguration`, whose members are `Public` (jsonName `public`, target PublicRouterNetworkInterfaceConfiguration) and `Vpc` (jsonName `vpc`). A restJson1 request body `{"name":...,"configuration":{"public":{"allowRules":[{"cidr":"0.0.0.0/0"}]}}}` therefore has exactly one union member set and is valid input; AWS accepts it and returns the created RouterNetworkInterface. The operation models no ValidationException at all (its client errors are BadRequestException 400, ConflictException 409, ForbiddenException 403, RouterNetworkInterfaceServiceQuotaExceededException 420, TooManyRequestsException 429).
Emulator: the request is refused during the pre-handler deserialize/validate walk with `ValidationException: union 'RouterNetworkInterfaceConfiguration' had no member set` — i.e. the union deserializer does not recognise the wire member name (`public`/`vpc` per @jsonName) and concludes no member was set. The operation's own handler (`src/ops/create_router_network_interface.rs`, still a SCAFFOLDED stub) is never reached, so this is a wire/deserialization defect distinct from the stub.
Why: every router-plane client is blocked at the first call. A caller sending the exact payload the SDK builds for `Configuration={'Public': {...}}` gets a 400 ValidationException the model does not declare for this operation, instead of a 201 (or, failing that, the stub's 501). Union-typed input members are unusable across this service.
Fix: the union member deserialization in the shared restJson1 input path / the generated `RouterNetworkInterfaceConfiguration` schema in `services/service-com-amazonaws-mediaconnect-mediaconnect/src/ops/types.rs` must match union members by their @jsonName wire keys; then implement `src/ops/create_router_network_interface.rs`.
Traces: mediaconnect.router-input-lifecycle, mediaconnect.router-output-lifecycle, mediaconnect.take-router-input-routes-and-unroutes, mediaconnect.take-router-input-routing-scope-mismatch-rejected (all four fail on the shared `_create_public_network_interface` helper's first call).

=== Cause 2 of 6: no 'MediaConnect Flow Status Change' event is emitted to EventBridge on a flow status transition ===
AWS: recorded behavior only (the Smithy model does not describe eventing). MediaConnect publishes a `MediaConnect Flow Status Change` event with `source: aws.mediaconnect` to the account's default EventBridge bus whenever a flow's Status transitions, with the flow ARN in `resources` and the previous/current status in `detail` (`detail.currentStatus` in STARTING/ACTIVE after StartFlow). An ENABLED rule matching that source/detail-type with an SQS target (and a queue policy granting events.amazonaws.com sqs:SendMessage) receives the event.
Emulator: StartFlow (`src/ops/start_flow.rs`) only mutates stored flow status (`begin_transition(STARTING -> ACTIVE)`) and emits nothing; no event ever reaches the rule's SQS target, so every check polling for the event spins until the 60s harness timeout (the reported failure is `Check timed out after 60.0s`, hit inside the 120s positive-control poll).
Why: event-driven clients (rules, Lambda/SQS fan-out, state-change alarms) never fire, and the negative-delivery checks cannot even be evaluated because their positive control never delivers. A correct emulator must publish the status-change event on every flow transition.
Fix: flow status transition handling in `src/ops/start_flow.rs` / `src/ops/stop_flow.rs` and `src/state.rs` (`settle_due_transitions`) must publish the flow-status-change event onto the emulator's EventBridge default bus.
Traces: mediaconnect.flow-state-change-eventbridge-pattern-match-delivers, mediaconnect.flow-state-change-eventbridge-pattern-mismatch-no-delivery, mediaconnect.eventbridge-missing-queue-policy-blocks-delivery.

=== Cause 3 of 6: DescribeFlowSourceMetadata is an unimplemented scaffold stub ===
AWS: Smithy model — DescribeFlowSourceMetadata, GET /v1/flows/{FlowArn}/source-metadata, success 200, output {flowArn, messages, timestamp, transportMediaInfo, ndiInfo}; modeled errors include BadRequestException (400). Recorded behavior: the call is gated on the flow's Status — on a freshly created STANDBY flow it is refused with HTTP 400 `BadRequestException`, and the identical call succeeds once the flow is ACTIVE, returning TransportMediaInfo (empty Programs list) with no media actually ingested.
Emulator: answers HTTP 501 with error code `NotImplemented` in both states (`src/ops/describe_flow_source_metadata.rs` still carries the `SCAFFOLDED, NOT IMPLEMENTED` marker).
Why: a client gets a 501/NotImplemented that is not in the operation's modeled error set, so it can neither read source metadata nor distinguish the legitimate 400 'flow must be ACTIVE' gate from a server fault; SDK retry/branching on BadRequestException never triggers.
Fix: implement `src/ops/describe_flow_source_metadata.rs` — reject with BadRequestException while the stored flow is not ACTIVE, otherwise project TransportMediaInfo from `src/state.rs`.
Traces: mediaconnect.describe-flow-source-metadata-not-yet-available.

=== Cause 4 of 6: RemoveFlowSource is an unimplemented scaffold stub ===
AWS: Smithy model — RemoveFlowSource, DELETE /v1/flows/{FlowArn}/source/{SourceArn}, success 202, output {flowArn, sourceArn}; modeled errors include BadRequestException (400) and NotFoundException (404). Recorded behavior: removing a flow's ONLY source is refused with HTTP 400 `BadRequestException` and the flow keeps that source.
Emulator: answers HTTP 501 with error code `NotImplemented` (`src/ops/remove_flow_source.rs` still carries the `SCAFFOLDED, NOT IMPLEMENTED` marker), so the last-source guard is unobservable.
Why: source detach is unavailable, and a client that relies on BadRequestException to detect the 'cannot remove the only source' rule sees an unmodeled 501 instead.
Fix: implement `src/ops/remove_flow_source.rs`, including the reject-when-last-source rule, against the flow's source list in `src/state.rs`.
Traces: mediaconnect.remove-only-flow-source-rejected.

=== Cause 5 of 6: an ACTIVE flow advertises an ingest endpoint but no native UDP listener exists and Source.PeerIpAddress never populates ===
AWS: recorded behavior only (data plane is raw UDP, not an API call). Once a flow is ACTIVE, MediaConnect binds the endpoint it advertises on DescribeFlow as Source.IngestIp:IngestPort and, upon observing real inbound UDP packets from a sender, records that sender's address on the flow's `Source.PeerIpAddress` — no conformant RTP/MPEG-TS payload is required.
Emulator: after CreateFlow/StartFlow and the flow reaching the advertised endpoint, UDP packets sent to IngestIp:IngestPort are never observed — DescribeFlow's Source.PeerIpAddress never populates, so the check spins until the 60s harness timeout (`Check timed out after 60.0s`). Nothing is listening on the endpoint the emulator itself announced.
Why: the emulator accepts the control-plane calls and advertises an ingest endpoint that does not exist, so any client doing a real ingest round trip (or asserting the peer address it sees) hangs forever rather than failing fast. Note this check also asserts, after the round trip, that CreateFlow with Source.Protocol='udp' is refused with BadRequestException naming the protocol invalid — unverified here because the run never got past the ingest poll.
Fix: the flow data plane — bind the advertised ingest endpoint when a flow becomes ACTIVE and record the observed sender on Source.PeerIpAddress in `src/state.rs`, driven from the flow start path.
Traces: mediaconnect.native-media-ingest-udp-round-trip.

=== Cause 6 of 6: client-error messages do not use MediaConnect's error-message envelope ===
AWS: recorded behavior only for the exact prose, but the ENVELOPE is a consistent, request-derived rule visible across independent recordings of this service: NotFoundException messages are `Not found: <the requested ARN>`; BadRequestException messages for a semantic refusal are `Bad request: <detail>`; BadRequestException messages for a request-validation refusal are `Validation failed:\n<detail>`. The model itself says the precise meaning of BadRequestException/NotFoundException 'is documented in the error message', so the envelope is part of the contract clients read. HTTP statuses and error codes DO match today (404/NotFoundException, 400/BadRequestException) — only the message envelope diverges.
Emulator: emits its own prose with no envelope. DescribeFlow on an unknown ARN returns `Flow with ARN <arn> was not found.` where AWS returns `Not found: <arn>` (see `src/ops/start_flow.rs`, which formats the same non-envelope text). BatchGetRouterInput with 21 ARNs (modeled `Arns` @length min 1 max 20, @httpQuery `arns`) returns the generic constraint text `Value '21 items' at 'arns' failed to satisfy constraint: Member must have length less than or equal to 20` where AWS returns `Validation failed:\nToo many ARNs provided. The maximum number of Router Inputs that can be requested in a single batch is 20.`. CreateFlow with neither Source nor Sources returns `A create flow request must include one source: set either Source or Sources.` where AWS returns a `Validation failed:\n...` envelope.
Why: callers and operators that parse or log MediaConnect's documented message envelope (and tooling that distinguishes a validation refusal from a semantic one by the `Validation failed:`/`Bad request:`/`Not found:` prefix) misread every refusal. The recorded prose after the prefix is an example, not a requirement; the envelope and the request-derived ARN are the requirement.
Fix: the service's error construction — render modeled BadRequestException/NotFoundException messages through a shared envelope helper used by every MediaConnect op handler (e.g. the NotFoundException sites in `src/ops/*.rs`) and route modeled-constraint rejections for this service through it too.
Traces: mediaconnect.describe-flow-not-found, mediaconnect.batch-get-router-input-arn-limit-exceeded-rejected, mediaconnect.start-flow-missing-source-rejected.

## Evidence
Replay report: mediaconnect 2/28 passed (7%), 26 failed (9 mismatch, 17 error).

Cause 1 — mediaconnect.router-input-lifecycle / router-output-lifecycle / take-router-input-routes-and-unroutes / take-router-input-routing-scope-mismatch-rejected:
  request: create_router_network_interface(Name=..., Configuration={"Public": {"AllowRules": [{"Cidr": "0.0.0.0/0"}]}}, Tags={...})
  expected: HTTP 201 with RouterNetworkInterface (model: union member Public, jsonName "public")
  actual:   botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the CreateRouterNetworkInterface operation: union 'RouterNetworkInterfaceConfiguration' had no member set

Cause 2 — mediaconnect.flow-state-change-eventbridge-pattern-match-delivers / pattern-mismatch-no-delivery / eventbridge-missing-queue-policy-blocks-delivery:
  expected: after StartFlow, an ENABLED rule matching {source: [aws.mediaconnect], detail-type: [MediaConnect Flow Status Change], resources: [flowArn]} delivers the event to its SQS target; detail.currentStatus in (STARTING, ACTIVE)
  actual:   no message ever arrives on the authorized/matching queue; 'Check timed out after 60.0s' inside the 120s positive-control poll

Cause 3 — mediaconnect.describe-flow-source-metadata-not-yet-available:
  expected: HTTP 400 BadRequestException on a STANDBY flow; HTTP 200 with TransportMediaInfo once ACTIVE
  actual:   AssertionError: expected BadRequestException on a STANDBY flow, got 'NotImplemented' (HTTP 501)

Cause 4 — mediaconnect.remove-only-flow-source-rejected:
  expected: HTTP 400 BadRequestException removing the flow's only source
  actual:   AssertionError: removing the flow's only source was rejected with 'NotImplemented', expected 'BadRequestException' (HTTP 501)

Cause 5 — mediaconnect.native-media-ingest-udp-round-trip:
  expected: 188-byte UDP probe packets sent to the ACTIVE flow's advertised Source.IngestIp:IngestPort cause DescribeFlow to report Source.PeerIpAddress
  actual:   PeerIpAddress never populates; 'Check timed out after 60.0s' (the harness verified it can open a UDP socket, so this is not a runner limitation)

Cause 6 — error-message envelope (status and error code already match):
  mediaconnect.describe-flow-not-found: expected 404/NotFoundException message 'Not found: arn:aws:mediaconnect:<region>:<account>:flow:<id>:<name>', got 'Flow with ARN arn:aws:mediaconnect:<region>:<account>:flow:<id>:<name> was not found.'
  mediaconnect.batch-get-router-input-arn-limit-exceeded-rejected: expected 400/BadRequestException message 'Validation failed:\nToo many ARNs provided. The maximum number of Router Inputs that can be requested in a single batch is 20.', got "Value '21 items' at 'arns' failed to satisfy constraint: Member must have length less than or equal to 20"
  mediaconnect.start-flow-missing-source-rejected: expected 400/BadRequestException message beginning 'Validation failed:\n', got 'A create flow request must include one source: set either Source or Sources.'

## Steps to Reproduce
Boot the emulator and run the behavioral suite (run_behavioral_suite), or exercise each cause directly with a botocore mediaconnect client pointed at the emulator endpoint:

Cause 1: mc.create_router_network_interface(Name='n1', Configuration={'Public': {'AllowRules': [{'Cidr': '0.0.0.0/0'}]}}) -> observe 400 ValidationException "union 'RouterNetworkInterfaceConfiguration' had no member set" instead of a 201 (or the op's 501 stub).

Cause 2: create a flow with Source={'Name':'source1','Protocol':'rtp','WhitelistCidr':'0.0.0.0/0'}; events.put_rule(EventPattern={'source':['aws.mediaconnect'],'detail-type':['MediaConnect Flow Status Change'],'resources':[flowArn]}, State='ENABLED'); grant events.amazonaws.com sqs:SendMessage on an SQS queue and events.put_targets to it; mc.start_flow(FlowArn=flowArn); poll sqs.receive_message -> no event ever arrives.

Cause 3: create a flow (it is STANDBY) and call mc.describe_flow_source_metadata(FlowArn=flowArn) -> 501 NotImplemented instead of 400 BadRequestException; start the flow, wait for ACTIVE, call again -> still 501 instead of 200 with TransportMediaInfo.

Cause 4: create a flow with exactly one source and call mc.remove_flow_source(FlowArn=flowArn, SourceArn=<that source>) -> 501 NotImplemented instead of 400 BadRequestException.

Cause 5: create a flow, mc.start_flow, poll DescribeFlow until ACTIVE, read Source.IngestIp/IngestPort, send UDP datagrams there from a plain socket, then poll DescribeFlow for Source.PeerIpAddress -> never populates.

Cause 6: (a) mc.describe_flow(FlowArn=<well-formed but unknown flow ARN>) -> compare Error.Message to 'Not found: <arn>'. (b) mc.batch_get_router_input(Arns=[21 ARNs]) -> compare Error.Message to the 'Validation failed:\n...' envelope. (c) mc.create_flow(Name='x') with neither Source nor Sources -> compare Error.Message to the 'Validation failed:\n...' envelope.
