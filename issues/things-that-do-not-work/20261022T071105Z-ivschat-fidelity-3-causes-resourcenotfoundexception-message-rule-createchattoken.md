Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ivschat.create-chat-token-nonexistent-room-not-found ivschat.delete-room-removes-resource ivschat.create-chat-token-session-duration-out-of-range-rejected ivschat.tag-and-untag-resource-room
Issue-Shape-Id: com.amazonaws.ivschat#AmazonInteractiveVideoServiceChat
Issue-Key: ivschat.create-chat-token-nonexistent-room-not-found|run#6|AssertionError|assert message == f"Resource: {bogus_arn} not found", f"unexpected message: {message!r}"
Issue-Key: ivschat.delete-room-removes-resource|run#9|AssertionError|assert message == f"Resource: {arn} not found", f"GetRoom message: {message!r}"
Issue-Key: ivschat.create-chat-token-session-duration-out-of-range-rejected|run#7|AssertionError|assert message == (
Issue-Key: ivschat|TagResource|ValidationException
Issue-Commit: d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea

# ivschat fidelity: 3 causes — ResourceNotFoundException message rule, CreateChatToken range-violation ValidationException message rule, and TagResource refusing a valid room ARN

> _Note: this issue was identified against the code base at commit `d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChatToken

## Description
Three distinct root causes in the ivschat emulator, found by replaying the behavioral suite (5/15 passed). All three are in the ivschat service. Causes 1 and 2 are error-construction rules; cause 3 refuses a request real AWS accepts. No existing open issue covers any of them (list_existing_issues finds no ivschat issue other than the auto-filed CreateLoggingConfiguration stub issue, which is a different operation and is skipped here).

=== Cause 1 of 3: ResourceNotFoundException Message does not follow the service's 'Resource: <identifier> not found' rule ===
AWS: Amazon IVS Chat answers a request naming a well-formed but nonexistent room with ResourceNotFoundException, HTTP 404 (Smithy: ivschat#ResourceNotFoundException, errorKind client, httpError 404; modeled on CreateChatToken, GetRoom, UpdateRoom, TagResource). The recorded responses build the Message from the request by a single rule across operations: "Resource: <the identifier the caller sent> not found" — e.g. for CreateChatToken(roomIdentifier=<arn>) and for GetRoom/UpdateRoom(identifier=<arn>) the message is the same shape with the ARN echoed verbatim. Because the message is derived from the request member and is identical across three different operations in the recordings, the rule — not the one recorded string — is the requirement.
Emulator: builds a different message for the same condition: 'Room not found for identifier arn:aws:ivschat:us-west-2:000000000000:room/<id>.' (status 404 and code ResourceNotFoundException are correct). The text comes from one central constructor, services/service-com-amazonaws-ivschat-amazoninteractivevideoservicechat/src/state.rs:234 `room_not_found`, so every not-found path in the service diverges the same way (observed on CreateChatToken and on GetRoom after DeleteRoom; the same check also asserts it for UpdateRoom).
Why: clients and SDK-level test suites that match on the service's not-found message rule (and anything that parses the identifier back out of the message) see an unrecognized string, so not-found handling that works against AWS does not work against the emulator.
Fix: the ResourceNotFoundException constructor shared by the room operations — `room_not_found` in the service's state module — must emit the service's message rule 'Resource: {identifier} not found' using the identifier as the caller sent it (the full ARN, not the extracted room id), leaving resourceId/resourceType as they are.
Traces: ivschat.create-chat-token-nonexistent-room-not-found, ivschat.delete-room-removes-resource (citations: src/state.rs:234; src/ops/get_room.rs:193).

=== Cause 2 of 3: CreateChatToken range-constraint refusal uses the generic Smithy constraint message instead of the service's ValidationException rule ===
AWS: Smithy: CreateChatToken.sessionDurationInMinutes targets SessionDurationInMinutes with range min 1, max 180; the operation models ValidationException (client, httpError 400). A value above the maximum is refused with ValidationException/400 and a message built from the request by the service's own rule: '<Operation>.<member> is required to have a maximum value of <max>.0, found <value>.0' — the recording shows 'CreateChatToken.sessionDurationInMinutes is required to have a maximum value of 180.0, found 181.0'. Every element of that message comes from the operation name, the violated member, its modeled bound and the submitted value, so the rule is known and is the requirement.
Emulator: refuses with the correct status and code but with the framework's generic constraint prose: "1 validation error detected: Value '181' at 'sessionDurationInMinutes' failed to satisfy constraint: Member must have value less than or equal to 180" — it names neither the operation nor the service's phrasing, and renders the bound and the value as integers rather than the decimal form the service uses.
Why: a client that classifies which input it must correct by parsing the service's validation message (a common pattern for range/length refusals, since fieldList alone does not carry the bound) cannot read the emulator's message, and anything asserting the documented message rule fails even though the request was correctly rejected.
Fix: the pre-handler constraint-violation reporting for this service must render range (and the equivalent length/pattern) violations through the ivschat ValidationException rule rather than the generic Smithy validator text — i.e. the service's validation layer feeding ValidationException, alongside the existing field-level constructor at src/state.rs:254 `field_validation_failed`. The boundary value 180 must continue to be accepted and honored, not clamped.
Traces: ivschat.create-chat-token-session-duration-out-of-range-rejected.

=== Cause 3 of 3: TagResource refuses a valid room ARN with 400 ValidationException, so room tagging is unreachable ===
AWS: TagResource (Smithy: POST /tags/{resourceArn}, resourceArn is an httpLabel of ResourceArn, required, length 1..128, pattern ^arn:aws:ivschat:[a-z0-9-]+:[0-9]+:[a-z-]/[a-zA-Z0-9-]+$; errors ValidationException 400, ResourceNotFoundException 404, InternalServerException 500). The modeled pattern is a known defect in the published model — its resource-type segment is the single-character class [a-z-], which no real resource type ('room/...', 'logging-configuration/...') can match — and real AWS does NOT enforce it: TagResource against a room ARN returned by CreateRoom succeeds with 200, the tags are then visible via ListTagsForResource, and UntagResource removes exactly the requested keys (recorded happy path).
Emulator: enforces the modeled pattern verbatim at the request boundary and rejects the room ARN it just minted itself: ValidationException, 400, message "1 validation error detected: Value 'arn:aws:ivschat:us-west-2:000000000000:room/<id>' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws:ivschat:[a-z0-9-]+:[0-9]+:[a-z-]/[a-zA-Z0-9-]+$". Separately, TagResource itself is still a scaffold stub (src/ops/tag_resource.rs:156 returns Scaffolded::not_implemented), so even past the pattern the tagging trio cannot answer; the pattern rejection is what the replay actually hit, before the stub.
Why: no client can tag an ivschat room against this emulator — every ARN the emulator's own CreateRoom/CreateLoggingConfiguration produces is refused as malformed, turning a modeled happy path into a hard 400. Tag-driven setup and teardown in client test suites breaks on the first call.
Fix: relax the boundary enforcement of the ResourceArn pattern for the tagging operations so a real ivschat resource ARN ('room/<id>', and the logging-configuration form) is accepted as AWS accepts it — i.e. this service's input-validation layer for ResourceArn — and then implement TagResource/UntagResource/ListTagsForResource so tags attach, list and untag per-key.
Traces: ivschat.tag-and-untag-resource-room (citations: src/ops/tag_resource.rs:56-57 and :156).

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator — ivschat 5/15 passed (33%), 10 failed.

Cause 1:
- ivschat.create-chat-token-nonexistent-room-not-found [mismatch]: code ResourceNotFoundException (correct) but AssertionError: unexpected message: 'Room not found for identifier arn:aws:ivschat:us-west-2:000000000000:room/nonexist71b79ede10ff.'; expected the service rule 'Resource: <roomIdentifier> not found'.
- ivschat.delete-room-removes-resource [mismatch]: after DeleteRoom, GetRoom answers ResourceNotFoundException (correct) with message 'Room not found for identifier arn:aws:ivschat:us-west-2:000000000000:room/0AHajh7wQKfu.'; expected 'Resource: <arn> not found'. The same assertion is made for UpdateRoom on the dead identifier.

Cause 2:
- ivschat.create-chat-token-session-duration-out-of-range-rejected [mismatch]: sessionDurationInMinutes=181 is correctly refused with ValidationException/400, but AssertionError: unexpected message: "1 validation error detected: Value '181' at 'sessionDurationInMinutes' failed to satisfy constraint: Member must have value less than or equal to 180"; expected 'CreateChatToken.sessionDurationInMinutes is required to have a maximum value of 180.0, found 181.0'.

Cause 3:
- ivschat.tag-and-untag-resource-room [error]: botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the TagResource operation: 1 validation error detected: Value 'arn:aws:ivschat:us-west-2:000000000000:room/0AHajh7wQKfu' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws:ivschat:[a-z0-9-]+:[0-9]+:[a-z-]/[a-zA-Z0-9-]+$ — expected HTTP 200, tags readable via ListTagsForResource, and UntagResource removing only 'env'. The rejected ARN was produced by the emulator's own CreateRoom in the same check.

Model evidence: smithy_query(query="operation", operation="TagResource,CreateChatToken,GetRoom,UpdateRoom") on services/service-com-amazonaws-ivschat-amazoninteractivevideoservicechat/smithy-model/com-amazonaws-ivschat-amazoninteractivevideoservicechat.json — CreateChatToken.sessionDurationInMinutes range 1..180, ValidationException httpError 400, ResourceNotFoundException httpError 404; TagResource.resourceArn pattern ^arn:aws:ivschat:[a-z0-9-]+:[0-9]+:[a-z-]/[a-zA-Z0-9-]+$ (single-char resource-type class), while RoomIdentifier/RoomArn use the correct ':room/' form.
Emulator source evidence: src/state.rs:234 (room_not_found message), src/state.rs:254 (field_validation_failed), src/ops/get_room.rs:193, src/ops/tag_resource.rs:56-57 and :156.

## Steps to Reproduce
1. git clone the repo and boot the emulator; point a botocore ivschat client at it (region us-west-2).
2. Cause 1a: account=$(sts get-caller-identity Account); call CreateChatToken(roomIdentifier="arn:aws:ivschat:us-west-2:$account:room/neverCreated1", userId="u"). Observe ResourceNotFoundException/404 with Message 'Room not found for identifier <arn>.' instead of 'Resource: <arn> not found'.
3. Cause 1b: CreateRoom(name="r") -> arn; DeleteRoom(identifier=arn); then GetRoom(identifier=arn) and UpdateRoom(identifier=arn, name="r2"). Both answer ResourceNotFoundException/404 with the 'Room not found for identifier ...' message.
4. Cause 2: CreateRoom -> arn; CreateChatToken(roomIdentifier=arn, userId="u", sessionDurationInMinutes=181). Observe ValidationException/400 whose Message is the generic '1 validation error detected: Value '181' at 'sessionDurationInMinutes' ...' rather than 'CreateChatToken.sessionDurationInMinutes is required to have a maximum value of 180.0, found 181.0'. Then call with 180 and confirm it is accepted and sessionExpirationTime is ~180 minutes out (not clamped).
5. Cause 3: CreateRoom -> arn; TagResource(resourceArn=arn, tags={"env":"sim2real","keep":"me"}). Observe ValidationException/400 on the resourceArn pattern instead of 200; ListTagsForResource/UntagResource are therefore never reached.
6. Re-replay the four checks in trace_ids to confirm a fix.
