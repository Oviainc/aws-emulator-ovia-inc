Issue-Source: trace-sweep-divergence
Issue-Trace: wickr-bot-lifecycle-and-validation
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wickr-bot-lifecycle-and-validation wickr-bot-lifecycle-and-validation-errors wickr-create-bot-in-security-group wickr-network-lifecycle-and-not-found-errors wickr-network-create-and-validation
Issue-Shape-Id: com.amazonaws.wickr#WickrAdminApi
Issue-Key: wickr|CreateNetwork|2xx->2xx|status
Issue-Key: wickr|CreateNetwork|2xx->2xx|$.message
Issue-Key: wickr|CreateNetwork|2xx->2xx|$headers.content-type
Issue-Key: wickr|CreateNetwork|2xx->2xx|$headers.etag
Issue-Key: wickr|DeleteNetwork|2xx->2xx|$.message
Issue-Key: wickr|DeleteNetwork|2xx->2xx|$.networkId
Issue-Key: wickr|DeleteNetwork|2xx->2xx|$headers.content-type
Issue-Key: wickr|DeleteNetwork|2xx->2xx|$headers.etag
Issue-Commit: 22941ee6d43df66d29bcadd7a62b7906e5868e84

# wickr: network write-path success envelopes diverge — CreateNetwork status/message, DeleteNetwork body, response headers

> _Note: this issue was identified against the code base at commit `22941ee6d43df66d29bcadd7a62b7906e5868e84`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateNetwork

## Description
Three net-new causes in the Wickr network write path. All three are response-shaping defects: the request is accepted and the state change happens, but the HTTP envelope the emulator returns is not the one AWS returns. A fixer must close all THREE before submitting.

Cause 1 of 3: CreateNetwork answers HTTP 200 with no `message`, where AWS answers HTTP 201 with one.
  AWS: recorded behavior only for the status and the member. The Smithy model (services/service-com-amazonaws-wickr-wickradminapi/smithy-model/com-amazonaws-wickr-wickradminapi.json) binds CreateNetwork POST /networks and models CreateNetworkResponse with only networkId/networkName/encryptionKeyArn; the recorded AWS response is status 201 and its JSON body additionally carries the member `message` with the fixed text "Network created successfully" (no variable parts — it names neither the network nor its id).
  Emulator: status 200, and the body carries no `message` at all ($.message missing).
  Why: a client that treats 201 as "created" (or that keys resource-creation handling off the created-status, as SDK/HTTP middleware and CDK-style wrappers do) sees a plain 200 and cannot distinguish creation from a non-creating success; a client reading the confirmation `message` gets null.
  Fix: the CreateNetwork handler's response construction in the wickr service crate — the operation's success status code and its output serializer (the handler lives under services/service-com-amazonaws-wickr-wickradminapi/src/ops/, included from src/ops/mod.rs).
  Traces: wickr-bot-lifecycle-and-validation#setup#1 (status-only cluster, also witnessed by wickr-bot-lifecycle-and-validation-errors#setup#1, wickr-create-bot-in-security-group#setup#1, wickr-network-lifecycle-and-not-found-errors#setup#1) and wickr-network-create-and-validation#task#1 (status + missing message).

Cause 2 of 3: DeleteNetwork's success body uses the emulator's own wording and omits `networkId`.
  AWS: DeleteNetworkResponse$message is documented "A message indicating that the network deletion has been initiated successfully"; the recorded 200 response body is {"message": "Network deletion initiated successfully", "networkId": "<the request's networkId>"}. The message is entirely fixed text — it embeds no id, no count and no timestamp. `networkId` is recorded behavior only (DeleteNetworkResponse models just `message`) and echoes the networkId path label of the request.
  Emulator: 200 with $.message = "network <id> and its <n> users were deleted" — wording the service does not use, interpolating the network id and a deleted-user count — and no $.networkId member.
  Why: deletion is asynchronous on AWS ("initiated"), so a client that reads the message as a completion statement, or that reads back the echoed networkId to correlate the delete, is misled; the absent networkId member breaks that correlation outright.
  Fix: the DeleteNetwork handler's response construction/serializer in the same crate.
  Traces: wickr-network-lifecycle-and-not-found-errors#task#6.

Cause 3 of 3: every wickr response omits the content-type charset parameter and the ETag header.
  AWS: recorded behavior only. Every recorded Wickr restJson1 response — success and error alike — carries `content-type: application/json; charset=utf-8` and an `etag` header (the fixture asserts only that the etag is present and shaped, not its value, since an entity tag is a per-response value).
  Emulator: `content-type: application/json` (charset parameter dropped) and no `etag` header on any response.
  Why: a client or proxy that parses the media type's charset parameter has to guess the encoding, and conditional-request/caching logic keyed off ETag has nothing to key off.
  Fix: the wickr service's restJson1 response serializer / response-header construction — this is service-wide, not per-operation, so it must be fixed where the response headers for every operation are emitted.
  Traces: witnessed on every asserted wickr step, e.g. wickr-network-create-and-validation#task#1, and likewise on wickr-network-create-and-validation#task#2/#task#3/#task#4 and wickr-network-lifecycle-and-not-found-errors#task#3..#task#7.

No open wickr issue covers any of these (the only existing wickr issue is the resolved bootstrap issue).

## Evidence
Cause 1 — wickr-bot-lifecycle-and-validation#setup#1: expected status 201, got 200 (cluster of 4 steps). wickr-network-create-and-validation#task#1: expected 201, got 200; $.message missing (expected 'Network created successfully', got None).
Cause 2 — wickr-network-lifecycle-and-not-found-errors#task#6 (DeleteNetwork, 200): $.message value_mismatch (expected 'Network deletion initiated successfully', got 'network 10000000 and its 0 users were deleted'); $.networkId missing (expected the request's networkId, got None).
Cause 3 — on the same steps: $headers.content-type value_mismatch (expected 'application/json; charset=utf-8', got 'application/json'); $headers.etag missing (expected a shaped-but-not-asserted value, got None).

## Steps to Reproduce
1. POST /networks with a valid networkName and accessLevel (e.g. STANDARD) and observe the response status and body: AWS answers 201 with {"networkId":..., "networkName":..., "message":"Network created successfully"}; the emulator answers 200 with no message.
2. DELETE /networks/{networkId} for that network and observe the body: AWS answers {"message":"Network deletion initiated successfully","networkId":"<id>"}; the emulator answers its own "network <id> and its <n> users were deleted" with no networkId.
3. Inspect the response headers of any of the above: AWS sends content-type 'application/json; charset=utf-8' plus an etag; the emulator sends 'application/json' and no etag.
4. Replay fixtures wickr-bot-lifecycle-and-validation (setup#1), wickr-network-create-and-validation (task#1) and wickr-network-lifecycle-and-not-found-errors (task#6).
