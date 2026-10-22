Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: medialive.delete-input-attached-to-channel-rejected medialive.signal-map-create-completes
Issue-Shape-Id: com.amazonaws.medialive#MediaLive
Issue-Key: medialive.delete-input-attached-to-channel-rejected|run#11|AssertionError|assert code == "ConflictException", code
Issue-Key: medialive.signal-map-create-completes|run#18|AssertionError|assert input_arn in discovered_neighbor_arns, (
Issue-Commit: b4ac6d961cdb10c212ccdfac1172ee263c3e4aa3

# medialive: DeleteInput unimplemented (501) and signal-map discovery omits a channel's attached inputs

> _Note: this issue was identified against the code base at commit `b4ac6d961cdb10c212ccdfac1172ee263c3e4aa3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteInput, CreateSignalMap

## Description
Two net-new root causes in the medialive emulator, found by replaying the behavioral suite. Both must be closed before submitting.

Cause 1 of 2: DeleteInput is still a scaffold stub, so the input lifecycle cannot be ended and its busy-input rejection cannot be observed.
Failing check ids: medialive.delete-input-attached-to-channel-rejected
  AWS: Smithy model (services/service-com-amazonaws-medialive-medialive/smithy-model/com-amazonaws-medialive-medialive.json) binds DeleteInput as DELETE /prod/inputs/{InputId}, success HTTP 200, and models ConflictException (HTTP 409) and NotFoundException (HTTP 404) among its errors; the operation doc is "Deletes the input end point". Recorded behavior: DeleteInput on an input whose State is ATTACHED (it is referenced by an existing channel) is refused with error code ConflictException at HTTP 409 and the input stays ATTACHED; once the referencing channel is deleted, DeleteInput succeeds and the input settles into State DELETED. The recorded message "Input <id> is busy, it cannot be deleted" is an example of that refusal, not the requirement — the requirement is the 409/ConflictException classification plus the absent effect, and the DELETED terminal state on the positive control.
  Emulator: every DeleteInput call answers HTTP 501 with error code NotImplemented and message "operation DeleteInput is scaffolded but not implemented in this emulator". The handler's `handle` body ends in `Err(crate::scaffolded::Scaffolded::not_implemented("DeleteInput"))` and the file still carries the SCAFFOLDED marker, so neither the attached-input refusal nor the successful delete exists. The check saw code NotImplemented where it expected ConflictException.
  Why: a client cannot delete an input at all, so no teardown path works and inputs accumulate; an unmodeled 501/NotImplemented is not in DeleteInput's error set, so SDK error handling that branches on ConflictException (retry after detaching) or NotFoundException (already gone) never fires, and a test asserting that AWS refuses deleting a busy input passes vacuously against this emulator.
  Fix: the DeleteInput handler and the input slice of the service state — refuse with the modeled ConflictException while any channel's InputAttachments reference the input, otherwise mark the input deleted so DescribeInput reports State DELETED; then drop the SCAFFOLDED marker line.
  Traces: medialive.delete-input-attached-to-channel-rejected (file/line citations: services/service-com-amazonaws-medialive-medialive/src/ops/delete_input.rs lines 4 and 162).

Cause 2 of 2: signal-map discovery from a channel ARN never traces to the channel's attached input, so MediaResourceMap contains no input neighbors.
Failing check ids: medialive.signal-map-create-completes
  AWS: Smithy model documents CreateSignalMap as "Initiates the creation of a new signal map. Will discover a new mediaResourceMap based on the provided discoveryEntryPointArn"; MediaResourceMap is "A map representing an AWS media workflow as a graph", each MediaResource carrying Sources and Destinations lists of MediaResourceNeighbor, each neighbor "A direct source or destination neighbor to an AWS media resource" with a required Arn. Recorded behavior: discovery started at a channel ARN returns a MediaResourceMap whose nodes list the channel's attached input ARN among their Sources/Destinations neighbors, and the signal map settles to Status CREATE_COMPLETE; GetSignalMap returns the same discovered graph.
  Emulator: CreateSignalMap/GetSignalMap return a MediaResourceMap whose nodes have empty Sources and Destinations, so the set of discovered neighbor ARNs is empty (replay: "expected discovery starting at channel <channel arn> to trace to its attached input <input arn>; discovered neighbors were set()"). The state helper `discover_media_resource_map` walks CHANNEL-to-CHANNEL links only (a follower channel's primary and the channels following it) and deliberately excludes inputs the store does hold.
  Why: the whole point of a signal map is the discovered media-resource graph. A client that reads MediaResourceMap to learn which inputs feed a channel (monitoring deployment, graph rendering, dependency checks) sees an isolated node with no sources, and concludes the channel has no input — so parity tests on discovery pass against a graph that contains no edges.
  Fix: the discovery walk in the service state (the media-resource-graph builder used by CreateSignalMap and read back by GetSignalMap) — include each discovered channel's attached inputs as nodes and as Sources neighbors of the channel, derived from the channel's stored InputAttachments and the input records the store already holds.
  Traces: medialive.signal-map-create-completes (file/line citations: services/service-com-amazonaws-medialive-medialive/src/state.rs lines 1211-1261, services/service-com-amazonaws-medialive-medialive/src/ops/create_signal_map.rs line 224).

## Evidence
Cause 1 — medialive.delete-input-attached-to-channel-rejected [mismatch]:
  Expected (AWS): DeleteInput on an ATTACHED input -> HTTP 409, Error.Code "ConflictException", input still readable with State ATTACHED; after the channel is deleted, DeleteInput succeeds and DescribeInput reports State "DELETED".
  Actual (emulator): HTTP 501, Error.Code "NotImplemented", message "operation DeleteInput is scaffolded but not implemented in this emulator".
  Assertion: `assert code == "ConflictException", code` -> AssertionError: NotImplemented.

Cause 2 — medialive.signal-map-create-completes [mismatch]:
  Expected (AWS): GetSignalMap(Identifier=<id>) returns a non-empty MediaResourceMap whose nodes' Sources/Destinations include the ARN of the input attached to the discovery entry-point channel.
  Actual (emulator): MediaResourceMap nodes carry no Sources/Destinations; the union of discovered neighbor ARNs is the empty set.
  Assertion: `assert input_arn in discovered_neighbor_arns` -> AssertionError: "expected discovery starting at channel <channel arn> to trace to its attached input <input arn>; discovered neighbors were set()".

## Steps to Reproduce
Boot the emulator and point a botocore medialive client at it, then:

Cause 1 (DeleteInput):
1. CreateInput (URL_PULL) -> note InputId.
2. CreateChannel referencing that input -> the input's DescribeInput State becomes ATTACHED.
3. Call DeleteInput(InputId=<id>). AWS: HTTP 409 ConflictException, input still ATTACHED. Emulator: HTTP 501 NotImplemented.
4. DeleteChannel the channel, wait for it to be gone, then DeleteInput(InputId=<id>). AWS: succeeds and the input reaches State DELETED. Emulator: HTTP 501 NotImplemented.
Replay check: medialive.delete-input-attached-to-channel-rejected

Cause 2 (signal-map discovery):
1. CreateInput, then CreateChannel attaching it -> note the channel Arn and the input Arn.
2. CreateSignalMap(Name=..., DiscoveryEntryPointArn=<channel arn>) and wait for Status CREATE_COMPLETE.
3. GetSignalMap(Identifier=<signal map id>) and collect every neighbor ARN from MediaResourceMap values' Sources + Destinations. AWS: the attached input's ARN is present. Emulator: the neighbor set is empty.
Replay check: medialive.signal-map-create-completes
