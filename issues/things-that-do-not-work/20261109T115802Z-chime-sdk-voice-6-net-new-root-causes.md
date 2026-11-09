Issue-Source: trace-sweep-divergence
Issue-Trace: chime-sdk-voice-voice-connector-group-create-validation-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: chime-sdk-voice-voice-connector-group-create-validation-errors chime-sdk-voice-voice-connector-group-validation-and-not-found chime-sdk-voice-voice-connector-not-found-and-list chime-sdk-voice-voice-connector-not-found-errors chime-sdk-voice-voice-profile-domain-create-validation-errors
Issue-Shape-Id: com.amazonaws.chimesdkvoice#ChimeSDKTelephonyService
Issue-Key: chime-sdk-voice|CreateVoiceConnectorGroup|4xx->4xx|$.message
Issue-Key: chime-sdk-voice|CreateVoiceConnectorGroup|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: chime-sdk-voice|CreateVoiceConnectorGroup|4xx->4xx|$.Code
Issue-Key: chime-sdk-voice|CreateVoiceConnectorGroup|4xx->4xx|$.Message
Issue-Key: chime-sdk-voice|CreateVoiceConnectorGroup|4xx->4xx|$.Resource
Issue-Key: chime-sdk-voice|CreateVoiceConnectorGroup|4xx->4xx|status
Issue-Key: chime-sdk-voice|GetVoiceConnector|4xx->4xx|$.Message
Issue-Key: chime-sdk-voice|GetVoiceConnector|4xx->4xx|$.RequestId
Issue-Key: chime-sdk-voice|UpdateVoiceConnector|4xx->4xx|$.Message
Issue-Key: chime-sdk-voice|UpdateVoiceConnector|4xx->4xx|$.RequestId
Issue-Key: chime-sdk-voice|DeleteVoiceConnector|4xx->4xx|$.Message
Issue-Key: chime-sdk-voice|DeleteVoiceConnector|4xx->4xx|$.RequestId
Issue-Key: chime-sdk-voice|CreateVoiceProfileDomain|4xx->4xx|$.message
Issue-Key: chime-sdk-voice|CreateVoiceProfileDomain|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# chime-sdk-voice: 6 net-new root causes — error responses carry bespoke messages, the wrong error class and the wrong status

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVoiceConnectorGroup

## Description
Six net-new root causes in how chime-sdk-voice renders client-error responses. They share one fault family: the refusal the emulator serves is its OWN wording / error class / status rather than the one the Chime SDK Voice wire produces, so a client that branches on the error (status, x-amzn-errortype, Code, Message) branches wrongly. Model facts below come from smithy_query against services/service-com-amazonaws-chimesdkvoice-chimesdktelephonyservice/smithy-model/com-amazonaws-chimesdkvoice-chimesdktelephonyservice.json. No open issue covers chime-sdk-voice (the open chime-family issues are for chime-sdk-media-pipelines, chime-sdk-meetings, chime-sdk-identity and chime-sdk-messaging), so all six are net-new.

=== Cause 1 of 6: CreateVoiceConnectorGroup — missing-required-member refusal drops AWS's "N validation error detected: " prefix and is labelled BadRequestException ===
AWS: CreateVoiceConnectorGroup input member Name is @required (Smithy: CreateVoiceConnectorGroup.Name -> VoiceConnectorGroupName, @required, @length min 1 max 256, @pattern ^[a-zA-Z0-9 _.-]+$). A request omitting it is refused BEFORE the operation runs by the framework's input-validation layer, with HTTP 400 and a body whose member is the lowercase `message` (not the modeled `Message`/`Code` pair of BadRequestException, which the model defines with members Code: ErrorCode and Message: String). The recorded message is the standard AWS validation template: fixed text "<n> validation error detected: Value <value> at '<memberPath>' failed to satisfy constraint: Member must not be null" — the fixed parts being the leading "<n> validation error detected: " segment, " at '", "' failed to satisfy constraint: Member must not be null"; the variable parts being the error count, the offending value as rendered (`null`), and the wire member path taken from the request (here the lowerCamel member name of the null member). The recorded x-amzn-errortype for this response is asserted by SHAPE and is not the modeled BadRequestException: this is the framework validation error class, not a modeled service exception.
Emulator: answers 400 with `message` = "Value null at 'name' failed to satisfy constraint: Member must not be null" — the per-member clause only, with AWS's "1 validation error detected: " count prefix omitted — and sets x-amzn-errortype: BadRequestException, which mismatches the recorded header's shape. The response is also internally inconsistent: the header claims BadRequestException while the body carries a lowercase `message` and neither of BadRequestException's modeled `Code`/`Message` members.
Why: an SDK/client that string-matches or parses the validation envelope (the "N validation error detected" prefix is how callers count and split multi-member validation failures) sees a message in a shape AWS never emits, and a client that dispatches on x-amzn-errortype classifies a framework validation failure as the service's modeled BadRequestException — so retry/report logic and error-type assertions that work against AWS fail here.
Fix: the service's shared pre-handler input-validation/serialization layer that renders constraint violations — it must emit the counted validation envelope and the framework validation error type rather than relabelling the refusal as the operation's modeled BadRequestException.
Traces: chime-sdk-voice-voice-connector-group-create-validation-errors#task#1 (representative).

=== Cause 2 of 6: CreateVoiceConnectorGroup — a reference to a VoiceConnectorId that does not exist is refused 400 instead of 404 NotFoundException ===
AWS: CreateVoiceConnectorGroup models NotFoundException (HTTP 404, "The requested resource couldn't be found.") alongside BadRequestException (HTTP 400). A well-formed request whose VoiceConnectorItems reference a VoiceConnectorId no account resource holds is a lookup failure, not a syntax failure: the recorded response is HTTP 404 with body Code = "NotFound" (a value of the model's ErrorCode enum) and Message = "Unable to find the requested identifier" — that message is FIXED text for this error class, carrying no request value — plus a `Resource` member (asserted by shape; not in the modeled NotFoundException, so: recorded behavior only — the live service adds it to not-found error bodies).
Emulator: answers HTTP 400 with x-amzn-errortype: BadRequestException and none of Code, Message or Resource present. Hypothesis: the request is being refused by the generic pre-handler validation layer (the CreateVoiceConnectorGroup handler is still an unimplemented scaffold, so no existence check against the Voice Connector store happens at all), so an unknown-reference request can never reach the 404 path.
Why: a client cannot distinguish "you sent a bad request, do not retry" from "that connector id does not exist, create it first" — the status class alone (400 vs 404) drives most SDK error handling, and the absent Code/Message means even a body-level fallback finds nothing to classify.
Fix: CreateVoiceConnectorGroup's handler plus its validation — validate the group's VoiceConnectorItems references against the Voice Connector store and raise the modeled NotFoundException (Code "NotFound", AWS's fixed not-found message, and the Resource member the wire carries) for a well-formed id that names no stored connector, reserving 400 for genuinely malformed input.
Traces: chime-sdk-voice-voice-connector-group-create-validation-errors#task#2.

=== Cause 3 of 6: GetVoiceConnector — NotFoundException message is bespoke prose and the error body omits RequestId ===
AWS: GetVoiceConnector models NotFoundException (HTTP 404) with members Code: ErrorCode and Message: String (Smithy: GetVoiceConnector errors -> NotFoundException @httpError 404). The recorded 404 body carries Code "NotFound" and a Message whose FIXED text is "Could not find VoiceConnector with Id '<id>'" — fixed: "Could not find VoiceConnector with Id '" and the closing "'"; variable: the VoiceConnectorId taken from the request path. The recorded body also carries a `RequestId` member (asserted by shape; not in the modeled NotFoundException, so: recorded behavior only — the live service includes it in error bodies).
Emulator: answers 404 with Message "no Voice Connector with id <id>" — the emulator's own wording, not AWS's — and no RequestId member at all. Source of the wording: the shared constructor crate::state::voice_connector_not_found (services/service-com-amazonaws-chimesdkvoice-chimesdktelephonyservice/src/state.rs:86-91), which all the Voice Connector handlers raise.
Why: tests and tooling that assert on the not-found message (a common way to confirm "this id really does not exist" versus another 404 cause) fail, and clients that log or correlate by the error body's RequestId have no request id to correlate with.
Fix: the shared NotFoundException payload constructor in the service's state module (read by GetVoiceConnector's handler at src/ops/get_voice_connector.rs:164-169) plus the error serializer that must include the RequestId member the wire carries.
Traces: chime-sdk-voice-voice-connector-not-found-and-list#task#2 (cluster also covers chime-sdk-voice-voice-connector-not-found-errors#task#1).

=== Cause 4 of 6: UpdateVoiceConnector — same not-found divergence on the update path ===
AWS: UpdateVoiceConnector models NotFoundException (HTTP 404) for a VoiceConnectorId that names no connector (Smithy: UpdateVoiceConnector errors -> NotFoundException @httpError 404; input VoiceConnectorId is an @httpLabel with @pattern ^([a-z0-9]{21,22}|<uuid>)$). The recorded 404 body carries Code "NotFound", Message with fixed text "Could not find VoiceConnector with Id '<id>'" (variable part: the id from the request path) and a RequestId member (recorded behavior only).
Emulator: answers 404 with Message "no Voice Connector with id <id>" and no RequestId member.
Why: same as cause 3 — a client's not-found assertions and body-level correlation break, on the write path where a caller is most likely to branch on "does not exist" versus "exists but refused".
Fix: the shared not-found payload constructor the UpdateVoiceConnector handler raises (src/state.rs:86-91) and the error serializer's RequestId member.
Traces: chime-sdk-voice-voice-connector-not-found-and-list#task#3 (cluster also covers chime-sdk-voice-voice-connector-not-found-errors#task#2).

=== Cause 5 of 6: DeleteVoiceConnector — same not-found divergence on the delete path ===
AWS: DeleteVoiceConnector models NotFoundException (HTTP 404) (Smithy: DeleteVoiceConnector errors -> NotFoundException @httpError 404; success is 204). The recorded 404 body carries Code "NotFound", Message with fixed text "Could not find VoiceConnector with Id '<id>'" (variable part: the id from the request path) and a RequestId member (recorded behavior only).
Emulator: answers 404 with Message "no Voice Connector with id <id>" and no RequestId member.
Why: same as causes 3 and 4 — idempotent-delete logic that keys off the not-found message, and correlation off the body's RequestId, both fail.
Fix: the shared not-found payload constructor the DeleteVoiceConnector handler raises (src/state.rs:86-91) and the error serializer's RequestId member.
Traces: chime-sdk-voice-voice-connector-not-found-and-list#task#4 (cluster also covers chime-sdk-voice-voice-connector-not-found-errors#task#3).

=== Cause 6 of 6: CreateVoiceProfileDomain — same missing "N validation error detected: " envelope and BadRequestException relabelling ===
AWS: CreateVoiceProfileDomain input members Name and ServerSideEncryptionConfiguration are @required (Smithy: CreateVoiceProfileDomain.Name -> VoiceProfileDomainName, @required, @length min 1 max 256, @pattern ^[a-zA-Z0-9 _.-]+$; CreateVoiceProfileDomain.ServerSideEncryptionConfiguration @required). A request omitting a required member is refused 400 by the framework validation layer with the counted envelope in the lowercase `message` member: fixed text "<n> validation error detected: Value <value> at '<memberPath>' failed to satisfy constraint: Member must not be null", with the count, the rendered value and the wire member path varying with the request. The recorded x-amzn-errortype is asserted by shape and is not the modeled BadRequestException.
Emulator: answers 400 with `message` = "Value null at 'name' failed to satisfy constraint: Member must not be null" (the "1 validation error detected: " prefix omitted) and x-amzn-errortype: BadRequestException, mismatching the recorded header's shape. The same divergence repeats across all four validation steps of the fixture, i.e. it is the shared validation layer and not one operation's handler.
Why: identical to cause 1 — the validation envelope a client parses is absent and the error type it dispatches on is the wrong class, so any client-side handling of required-member failures on the voice-profile-domain API misclassifies them.
Fix: the shared pre-handler input-validation/serialization layer (same component as cause 1); fixing it there fixes both create paths rather than per operation.
Traces: chime-sdk-voice-voice-profile-domain-create-validation-errors#task#1 (cluster also covers that fixture's task#2, task#3 and task#4).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true, bootstrap_signal false, 37 steps, 1 passed, 36 mismatched, 6 divergence clusters — all service=chime-sdk-voice, all with empty stub_path and empty owned_by).

Cause 1 — chime-sdk-voice-voice-connector-group-create-validation-errors#task#1, CreateVoiceConnectorGroup, verdict mismatch, status 400 -> 400:
  $.message: expected "1 validation error detected: Value null at 'name' failed to satisfy constraint: Member must not be null", got "Value null at 'name' failed to satisfy constraint: Member must not be null"
  $headers.x-amzn-errortype: shape_mismatch (expected <shaped-not-asserted>, got 'BadRequestException')
  cluster also covers: chime-sdk-voice-voice-connector-group-validation-and-not-found#task#1, chime-sdk-voice-voice-connector-group-validation-and-not-found#task#3

Cause 2 — chime-sdk-voice-voice-connector-group-create-validation-errors#task#2, CreateVoiceConnectorGroup, verdict mismatch:
  status: expected 404, got 400
  $.Code: missing (expected 'NotFound', got None)
  $.Message: missing (expected 'Unable to find the requested identifier', got None)
  $.Resource: missing (expected <shaped-not-asserted>, got None)
  $headers.x-amzn-errortype: shape_mismatch (expected <shaped-not-asserted>, got 'BadRequestException')

Cause 3 — chime-sdk-voice-voice-connector-not-found-and-list#task#2, GetVoiceConnector, verdict mismatch, status 404 -> 404:
  $.Message: expected "Could not find VoiceConnector with Id '03b96ace5f7a66f92e8650'", got 'no Voice Connector with id 03b96ace5f7a66f92e8650'
  $.RequestId: missing (expected <shaped-not-asserted>, got None)
  cluster also covers: chime-sdk-voice-voice-connector-not-found-errors#task#1

Cause 4 — chime-sdk-voice-voice-connector-not-found-and-list#task#3, UpdateVoiceConnector, verdict mismatch, status 404 -> 404:
  $.Message: expected "Could not find VoiceConnector with Id '<id>'", got 'no Voice Connector with id <id>'
  $.RequestId: missing
  cluster also covers: chime-sdk-voice-voice-connector-not-found-errors#task#2

Cause 5 — chime-sdk-voice-voice-connector-not-found-and-list#task#4, DeleteVoiceConnector, verdict mismatch, status 404 -> 404:
  $.Message: expected "Could not find VoiceConnector with Id '<id>'", got 'no Voice Connector with id <id>'
  $.RequestId: missing
  cluster also covers: chime-sdk-voice-voice-connector-not-found-errors#task#3

Cause 6 — chime-sdk-voice-voice-profile-domain-create-validation-errors#task#1, CreateVoiceProfileDomain, verdict mismatch, status 400 -> 400:
  $.message: expected "1 validation error detected: Value null at 'name' failed to satisfy constraint: Member must not be null", got "Value null at 'name' failed to satisfy constraint: Member must not be null"
  $headers.x-amzn-errortype: shape_mismatch (expected <shaped-not-asserted>, got 'BadRequestException')
  cluster also covers that fixture's task#2, task#3 and task#4

Emulator-side source of the not-found wording: services/service-com-amazonaws-chimesdkvoice-chimesdktelephonyservice/src/state.rs:86-91 (voice_connector_not_found builds Message via format!("no Voice Connector with id {voice_connector_id}") and sets no RequestId); raised from src/ops/get_voice_connector.rs:164-169 and the sibling update/delete handlers. CreateVoiceConnectorGroup's handler is still a scaffold stub (src/ops/create_voice_connector_group.rs:179), so it performs no reference lookup.

Identifiers quoted above (the connector id 03b96ace5f7a66f92e8650, request ids, timestamps) are values of one recording and are NOT requirements; the requirements are the statuses, error codes, error types and the fixed message text with its request-derived variable parts.

## Steps to Reproduce
1. git_clone the repo and boot the emulator built from it.
2. replay_wire_corpus() against trace set simparity-wire-aws-v1; all six clusters below reproduce deterministically (service chime-sdk-voice).
3. Cause 1: replay chime-sdk-voice-voice-connector-group-create-validation-errors step task#1 — POST /voice-connector-groups with the required Name member absent. Observe 400 with `message` lacking the "1 validation error detected: " prefix and x-amzn-errortype: BadRequestException.
4. Cause 2: replay the same fixture's step task#2 — a well-formed CreateVoiceConnectorGroup whose VoiceConnectorItems reference a VoiceConnectorId that does not exist. Observe 400/BadRequestException with no Code/Message/Resource instead of 404 NotFoundException (Code "NotFound", Message "Unable to find the requested identifier").
5. Causes 3-5: replay chime-sdk-voice-voice-connector-not-found-and-list steps task#2 (GET /voice-connectors/{id}), task#3 (PUT /voice-connectors/{id}) and task#4 (DELETE /voice-connectors/{id}) with a well-formed VoiceConnectorId that was never created. Each answers 404 with Message "no Voice Connector with id <id>" and no RequestId member instead of "Could not find VoiceConnector with Id '<id>'" plus RequestId. The same clusters reproduce via chime-sdk-voice-voice-connector-not-found-errors steps task#1..task#3.
6. Cause 6: replay chime-sdk-voice-voice-profile-domain-create-validation-errors step task#1 (and task#2..task#4) — POST /voice-profile-domains missing a required member. Observe the same missing validation-count envelope and the BadRequestException error type.
7. Re-run replay_wire_corpus() after the fix; all six clusters (and the other members of each cluster listed in the evidence) must pass.
