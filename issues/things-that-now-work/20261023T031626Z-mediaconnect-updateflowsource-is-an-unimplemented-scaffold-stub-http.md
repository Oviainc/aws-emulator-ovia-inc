Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mediaconnect.add-remove-flow-source-round-trip
Issue-Shape-Id: com.amazonaws.mediaconnect#MediaConnect
Issue-Key: mediaconnect.add-remove-flow-source-round-trip|run#20|AssertionError|assert code == "NotFoundException", f"expected NotFoundException updating removed source, got {code}"
Issue-Commit: 7f205ee96657d4151f96c4aa7296372f34590a63

# MediaConnect: UpdateFlowSource is an unimplemented scaffold stub (HTTP 501 NotImplemented) instead of updating a source or rejecting a stale source ARN with 404 NotFoundException

> _Note: this issue was identified against the code base at commit `7f205ee96657d4151f96c4aa7296372f34590a63`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateFlowSource

## Description
One net-new root cause found replaying the behavioral suite against the MediaConnect emulator (9/28 checks pass). It belongs to service `mediaconnect`. Causes the replay report marked ALREADY FILED (CreateBridge, RemoveFlowMediaStream, RevokeFlowEntitlement stubs) or OWNED by an open issue (the router-interface union / EventBridge / UDP-ingest / error-message-envelope issue 20261022T132818Z-mediaconnect-router-interface-union-payload-rejected-no-flow.md and the DeleteFlow issue 20261022T213407Z-mediaconnect-deleteflow-accepts-an-active-flow-instead-of.md) are NOT included here.

=== Cause 1 of 1: UpdateFlowSource answers HTTP 501 NotImplemented for every request ===
AWS: Smithy model services/service-com-amazonaws-mediaconnect-mediaconnect/smithy-model/com-amazonaws-mediaconnect-mediaconnect.json — UpdateFlowSource is PUT /v1/flows/{FlowArn}/source/{SourceArn}, success status 202, input requires FlowArn (@httpLabel, pattern ^arn:.+:mediaconnect.+:flow:.+$) and SourceArn (@httpLabel, pattern ^arn:.+:mediaconnect.+:source:.+$) plus optional source settings (Description, Protocol, IngestPort, WhitelistCidr, MaxBitrate, MaxLatency, Decryption, …); output is {flowArn, source}. Modeled errors: BadRequestException (400), ForbiddenException (403), NotFoundException (404), InternalServerErrorException (500), ServiceUnavailableException (503), TooManyRequestsException (429) — there is no 501/NotImplemented in the modeled error set. Recorded behavior for the specific condition this check exercises: after RemoveFlowSource detaches a source, that source ARN is dead, and UpdateFlowSource against it is refused with HTTP 404 error code NotFoundException; the message follows this service's NotFoundException envelope, `Not found: <the requested SourceArn>` (request-derived envelope, as described in cause 6 of the already-open issue 20261022T132818Z-mediaconnect-router-interface-union-payload-rejected-no-flow.md).
Emulator: services/service-com-amazonaws-mediaconnect-mediaconnect/src/ops/update_flow_source.rs still carries the `SCAFFOLDED, NOT IMPLEMENTED` marker and its handle body is `Err(crate::scaffolded::Scaffolded::not_implemented("UpdateFlowSource"))`, so every call — valid update or stale-ARN mutation — answers HTTP 501 with error code `NotImplemented`. The replay saw CreateFlow, UpdateFlow (failover config), AddFlowSources, DescribeFlow and RemoveFlowSource all behave correctly, and only the final UpdateFlowSource call diverged: expected NotFoundException, got NotImplemented.
Why: a client cannot modify a flow source at all, and — worse for correctness — it cannot distinguish a dead/removed source ARN from a server fault: it receives an unmodeled 501 (which SDKs treat as a non-retryable server-side failure) instead of the modeled 404 NotFoundException it branches on. A mutation against a removed source therefore neither succeeds nor reports the resource as gone, so detach is not observable as final.
Fix: implement the handler in services/service-com-amazonaws-mediaconnect-mediaconnect/src/ops/update_flow_source.rs (the only file for this operation) against the flow's source list in src/state.rs: resolve FlowArn and SourceArn in the stored state; when either is unknown or the source is no longer attached to the flow, return the modeled NotFoundException (404) rendered through this service's `Not found: <arn>` message envelope; otherwise apply the supplied source settings and return 202 with {FlowArn, Source}. Delete the SCAFFOLDED marker line once implemented.
Traces: mediaconnect.add-remove-flow-source-round-trip.

## Evidence
Replay report: mediaconnect 9/28 passed (32%), 19 failed. Failing check mediaconnect.add-remove-flow-source-round-trip [mismatch]:

  setup (all succeeded): create_flow(Source={Name:'source1',Protocol:'rtp',WhitelistCidr:'0.0.0.0/0',IngestPort:<derived>}) -> 1 source; update_flow(SourceFailoverConfig={State:'ENABLED',FailoverMode:'MERGE',RecoveryWindow:200}); add_flow_sources([source2]) -> 2 sources; remove_flow_source(SourceArn=<source2>) -> 1 source, source2 absent.
  request:  mc.update_flow_source(FlowArn=<flowArn>, SourceArn=<the just-removed source2 ARN>, Description='dead')
  expected: HTTP 404, Error.Code 'NotFoundException', Error.Message 'Not found: <that SourceArn>'
  actual:   HTTP 501, Error.Code 'NotImplemented'
  assertion: AssertionError: expected NotFoundException updating removed source, got NotImplemented

Source of the 501 (checkout): services/service-com-amazonaws-mediaconnect-mediaconnect/src/ops/update_flow_source.rs line 4 (`//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented`) and line 224 (`Err(crate::scaffolded::Scaffolded::not_implemented("UpdateFlowSource"))`).
Model: UpdateFlowSource http {method: PUT, uri: /v1/flows/{FlowArn}/source/{SourceArn}, code: 202}; errors BadRequestException 400, ForbiddenException 403, NotFoundException 404, InternalServerErrorException 500, ServiceUnavailableException 503, TooManyRequestsException 429.

## Steps to Reproduce
Boot the emulator and run the behavioral suite (run_behavioral_suite), or drive it directly with a botocore mediaconnect client pointed at the emulator endpoint:

1. flow = mc.create_flow(Name='mc-flow-1', Source={'Name':'source1','Protocol':'rtp','WhitelistCidr':'0.0.0.0/0','IngestPort':5001}); flow_arn = flow['Flow']['FlowArn'].
2. mc.update_flow(FlowArn=flow_arn, SourceFailoverConfig={'State':'ENABLED','FailoverMode':'MERGE','RecoveryWindow':200}).
3. src2 = mc.add_flow_sources(FlowArn=flow_arn, Sources=[{'Name':'source2','Protocol':'rtp','WhitelistCidr':'0.0.0.0/0','IngestPort':5002}])['Sources'][0]['SourceArn'].
4. mc.update_flow_source(FlowArn=flow_arn, SourceArn=src2, Description='x') -> observe HTTP 501 / Error.Code 'NotImplemented' instead of HTTP 202 with the updated Source.
5. mc.remove_flow_source(FlowArn=flow_arn, SourceArn=src2); then mc.update_flow_source(FlowArn=flow_arn, SourceArn=src2, Description='dead') -> observe HTTP 501 / 'NotImplemented' instead of HTTP 404 / 'NotFoundException' with message 'Not found: <src2>'.
