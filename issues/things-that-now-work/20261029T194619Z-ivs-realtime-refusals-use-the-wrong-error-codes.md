Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ivs-realtime.tag-resource-nonexistent-resource-rejected ivs-realtime.tag-resource-round-trip-list-and-untag ivs-realtime.delete-encoder-configuration-removes-from-list ivs-realtime.tag-resource-exceeds-limit-rejected ivs-realtime.tag-resource-tag-limit-exceeded-rejected ivs-realtime.create-participant-token-default-duration-is-720-minutes ivs-realtime.create-participant-token-defaults ivs-realtime.create-participant-token-attributes-size-limit-exceeded-rejected ivs-realtime.create-ingest-configuration-rtmp-requires-insecure-flag-rejected ivs-realtime.encoder-configuration-pixel-product-exceeded-rejected ivs-realtime.get-stage-session-malformed-session-id-rejected
Issue-Shape-Id: com.amazonaws.ivsrealtime#AmazonInteractiveVideoServiceRealTime
Issue-Key: ivs-realtime.tag-resource-nonexistent-resource-rejected|run#5|AssertionError|assert code == "NotFoundException", f"expected NotFoundException, got {code}"
Issue-Key: ivs-realtime.tag-resource-round-trip-list-and-untag|run#18|AssertionError|assert tag_code == "NotFoundException", tag_code
Issue-Key: ivs-realtime.delete-encoder-configuration-removes-from-list|run#11|AssertionError|assert tagged.get("Code") == "NotFoundException", tagged
Issue-Key: ivs-realtime.tag-resource-exceeds-limit-rejected|run#6|AssertionError|assert code == "BadRequestException", (
Issue-Key: ivs-realtime.tag-resource-tag-limit-exceeded-rejected|run#6|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code}"
Issue-Key: ivs-realtime.create-participant-token-default-duration-is-720-minutes|run#7|AssertionError|assert "duration" not in token, f"duration unexpectedly present in response: {token.get('duration')!r}"
Issue-Key: ivs-realtime.create-participant-token-defaults|run#6|AssertionError|assert "duration" not in token, token.get("duration")
Issue-Key: ivs-realtime.create-participant-token-attributes-size-limit-exceeded-rejected|run#9|AssertionError|assert error.get("Message") == (
Issue-Key: ivs-realtime.create-ingest-configuration-rtmp-requires-insecure-flag-rejected|run#5|AssertionError|assert error.get("Message") == (
Issue-Key: ivs-realtime.encoder-configuration-pixel-product-exceeded-rejected|run#6|AssertionError|assert message == expected_message, f"expected {expected_message!r}, got {message!r}"
Issue-Key: ivs-realtime.get-stage-session-malformed-session-id-rejected|run#9|AssertionError|assert missing.get("Message") == 'Resource: stage session "st-0000000000000" not found', missing
Issue-Commit: baf896d7efe27c876c690dc5952f8e38980dff3b

# ivs-realtime refusals use the wrong error codes and message templates, and CreateParticipantToken echoes defaults AWS omits (7 causes)

> _Note: this issue was identified against the code base at commit `baf896d7efe27c876c690dc5952f8e38980dff3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Seven distinct root causes in the ivs-realtime emulator's refusal classification, refusal wording and response shaping, all grounded in failing behavioral checks of this suite. A fixer must close all seven before submitting.

Cause 1 of 7 — the tagging surface answers ResourceNotFoundException where AWS answers NotFoundException, with a different message.
AWS: recorded behavior only for the wire code (Smithy models TagResource/UntagResource/ListTagsForResource with ResourceNotFoundException (404) and ValidationException (400) on POST|DELETE|GET /tags/{resourceArn}). The recording shows AWS refusing TagResource against an ARN that references no existing resource — a never-created ARN, and the ARN of a resource that was deleted — with error Code "NotFoundException" and Message "Resource was not found" (entirely fixed text: no ARN, no resource kind interpolated). The same run shows the resource-specific reads keeping the other spelling: GetEncoderConfiguration/GetStage on a dead ARN answer Code "ResourceNotFoundException" with "Resource: <arn> not found".
Emulator: the tagging operations answer Code "ResourceNotFoundException" with "Resource: <arn> not found" — the resource-read spelling, with the request's ARN interpolated.
Why: botocore builds exception classes from the code, so `except client.exceptions.NotFoundException` (or any string match on "Resource was not found") never fires; a client distinguishing a tagging target from a resource read cannot, because both now return the same code.
Fix: the tagging handlers' not-found path (src/ops/tag_resource.rs, src/ops/untag_resource.rs, src/ops/list_tags_for_resource.rs) must emit the tagging-surface code and sentence, separately from crate::state::resource_not_found used by the resource reads.
Traces: ivs-realtime.tag-resource-nonexistent-resource-rejected, ivs-realtime.tag-resource-round-trip-list-and-untag, ivs-realtime.delete-encoder-configuration-removes-from-list.

Cause 2 of 7 — a TagResource batch past the 50-tag limit is refused with the wrong code and sentence.
AWS: Smithy: TagResourceRequest$tags is a Tags map with @length(0..50). Recorded: a request whose resultant tag set would exceed 50 user tags (51 tags on an untagged stage) is refused with Code "BadRequestException" and Message "The resultant tag set must not have more than 50 user tags." (entirely fixed text, including the trailing period), none of the batch is applied, and exactly 50 tags is accepted.
Emulator: Code "ValidationException" with the emulator's own constraint sentence.
Why: the exception class a client catches is wrong, and quota-handling code keyed on BadRequestException/that message treats the refusal as an unrelated validation failure.
Fix: the tag-count admission check in src/ops/tag_resource.rs (and the shared tag-limit helper in src/state.rs) must raise the tagging surface's BadRequestException with that exact sentence.
Traces: ivs-realtime.tag-resource-exceeds-limit-rejected, ivs-realtime.tag-resource-tag-limit-exceeded-rejected.

Cause 3 of 7 — CreateParticipantToken echoes the defaults it applied, which AWS omits from the response.
AWS: Smithy: CreateParticipantTokenRequest$duration (ParticipantTokenDurationMinutes, range 1..20160) and $capabilities are optional, and the member docs state the defaults (720 minutes; PUBLISH,SUBSCRIBE). Recorded: when the request omits them, the response's participantToken carries NEITHER `duration` NOR `capabilities` — the server does not echo a default it chose — while `expirationTime` lands ~720 minutes after issuance, `participantId` and `token` are present.
Emulator: participantToken.duration is present and set to 720 (src/state.rs:44 DEFAULT_TOKEN_DURATION_MINUTES is materialised into the reported token), so the response shape differs from AWS even though the 720-minute default itself is right.
Why: a client that uses member presence to tell "the caller requested a duration" from "the service defaulted it" (round-tripping a token description, or diffing a create response against a stored request) reads a value AWS never sends.
Fix: the token reporting path in src/state.rs / src/ops/create_participant_token.rs must keep the requested presence of duration and capabilities, applying the documented default only to the computed expirationTime and capability set, not to the reported members.
Traces: ivs-realtime.create-participant-token-default-duration-is-720-minutes, ivs-realtime.create-participant-token-defaults.

Cause 4 of 7 — the attributes-too-large refusal of CreateParticipantToken is worded differently and measures a different size.
AWS: Smithy: CreateParticipantTokenRequest$attributes is a ParticipantTokenAttributes map (the 1 KB budget is documented prose, not a modeled @length). Recorded: Code "ValidationException", Message "Request member: attributes failed to satisfy the constraints: attributes is required to have a maximum size of 1 KB, found <n> bytes" — fixed text with the member name and the measured size as the only variable parts, the limit spelled "1 KB". For attributes={"blob": "x"*2000} the recorded <n> is 2011, i.e. AWS measures the JSON encoding of the map ({, quoted key "blob", colon, quoted 2000-char value, } = 2011 bytes), not the sum of the raw key and value lengths.
Emulator: Code "ValidationException" (correct) with Message "CreateParticipantToken.attributes is required to have a maximum size of 1024 bytes, found 2004" — a different envelope ("<Operation>.<member> …" instead of "Request member: <member> failed to satisfy the constraints: …"), the limit spelled "1024 bytes", no "bytes" suffix on the measurement, and a size of 2004 because only key+value lengths are summed.
Why: the refusal text is part of the contract clients log and match, and the different measurement moves the accept/reject boundary: a map AWS refuses can be accepted here (and vice versa) near 1 KB.
Fix: the attributes size check and its message builder in src/ops/create_participant_token.rs / the validation helpers in src/state.rs.
Traces: ivs-realtime.create-participant-token-attributes-size-limit-exceeded-rejected.

Cause 5 of 7 — the RTMP/insecureIngest refusal of CreateIngestConfiguration is worded with the emulator's own template.
AWS: Smithy: CreateIngestConfigurationRequest carries ingestProtocol and insecureIngest; the RTMP rule is documented prose. Recorded: CreateIngestConfiguration(ingestProtocol="RTMP") without insecureIngest=true is refused with Code "ValidationException" and Message "Request member: ingestProtocol failed to satisfy the constraints: insecure ingest is required for Ingest RTMP protocol" — entirely fixed text naming the member ingestProtocol; nothing is created, and the same request with insecureIngest=true succeeds.
Emulator: Code "ValidationException" (correct) with Message "CreateIngestConfiguration.insecureIngest is required to be true when ingestProtocol is RTMP, found false" — different envelope, different member named (insecureIngest rather than ingestProtocol) and a value echo AWS does not send.
Why: clients matching the refusal, and operators reading it, see a sentence AWS never sends, and it attributes the failure to the wrong request member.
Fix: the RTMP admission check and message in src/ops/create_ingest_configuration.rs.
Traces: ivs-realtime.create-ingest-configuration-rtmp-requires-insecure-flag-rejected.

Cause 6 of 7 — the pixel-budget refusal of CreateEncoderConfiguration is worded with the emulator's own template.
AWS: Smithy: CreateEncoderConfigurationRequest$video (Video width/height); the total-pixel budget is documented prose. Recorded: video={width:1920,height:1920} (3,686,400 px, over the 2,073,600 budget, each member individually in range) is refused with Code "ValidationException" and Message "Request member: video failed to satisfy the constraints: Code: VideoResolutionLimitError Message: video resolution exceeded max limit" — entirely fixed text, including the embedded "Code: VideoResolutionLimitError Message: …" detail; nothing is created, and 1920x1080 succeeds.
Emulator: Code "ValidationException" (correct) with Message "CreateEncoderConfiguration.video is required to have a maximum of 2073600 total pixels, found 3686400" — different envelope and the limit/measurement spelled out where AWS sends the named detail code.
Why: the embedded VideoResolutionLimitError token is how clients classify this specific limit; it is absent, so the refusal is indistinguishable from any other video validation failure.
Fix: the pixel-budget check and message in src/ops/create_encoder_configuration.rs.
Traces: ivs-realtime.encoder-configuration-pixel-product-exceeded-rejected.

Cause 7 of 7 — GetStageSession's not-found message omits the resource kind AWS names.
AWS: Smithy: GetStageSessionRequest$sessionId is StageSessionId with @length(16,16) and @pattern ^st-[a-zA-Z0-9]+$; the operation models ResourceNotFoundException (404) and ValidationException (400). Recorded: a well-formed but nonexistent sessionId is refused with Code "ResourceNotFoundException" and Message 'Resource: stage session "<sessionId>" not found' — fixed text "Resource: stage session \"" + the sessionId from the request + "\" not found" (the resource kind named, the id quoted).
Emulator: Code "ResourceNotFoundException" (correct) with Message "Resource: <sessionId> not found" — the generic resource-read sentence, with neither the "stage session" kind nor the quotes.
Why: this is the only part of the answer that tells a caller WHICH referenced resource was missing (the session, not the stage); clients and operators matching on AWS's wording do not match.
Fix: the not-found message for this operation in src/ops/get_stage_session.rs (it must not reuse the generic crate::state::resource_not_found sentence).
Traces: ivs-realtime.get-stage-session-malformed-session-id-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1, ivs-realtime 40/69 passed; the 11 checks below failed.

Cause 1: [mismatch] ivs-realtime.tag-resource-nonexistent-resource-rejected — "AssertionError: expected NotFoundException, got ResourceNotFoundException" (expected Message "Resource was not found"). [mismatch] ivs-realtime.tag-resource-round-trip-list-and-untag — "AssertionError: ResourceNotFoundException" on `tag_code == "NotFoundException"` after DeleteStage. [mismatch] ivs-realtime.delete-encoder-configuration-removes-from-list — "AssertionError: {'Message': 'Resource: arn:aws:ivs:us-west-2:000000000000:encoder-configuration/000001d6febf not found', 'Code': 'ResourceNotFoundException'}" on `tagged.get("Code") == "NotFoundException"`; the same check's GetEncoderConfiguration assertion (ResourceNotFoundException + "Resource: <arn> not found") passed, so only the tagging surface diverges.

Cause 2: [mismatch] ivs-realtime.tag-resource-exceeds-limit-rejected — "AssertionError: TagResource with 51 tags returned 'ValidationException', not BadRequestException". [mismatch] ivs-realtime.tag-resource-tag-limit-exceeded-rejected — "AssertionError: expected BadRequestException, got ValidationException". Expected Message "The resultant tag set must not have more than 50 user tags."

Cause 3: [mismatch] ivs-realtime.create-participant-token-default-duration-is-720-minutes — "AssertionError: duration unexpectedly present in response: 720". [mismatch] ivs-realtime.create-participant-token-defaults — "AssertionError: 720" on `"duration" not in token`. Code: src/state.rs:39-51 (DEFAULT_TOKEN_DURATION_MINUTES = 720, DEFAULT_TOKEN_CAPABILITIES).

Cause 4: [mismatch] ivs-realtime.create-participant-token-attributes-size-limit-exceeded-rejected — actual {'Code': 'ValidationException', 'Message': 'CreateParticipantToken.attributes is required to have a maximum size of 1024 bytes, found 2004'}; expected "Request member: attributes failed to satisfy the constraints: attributes is required to have a maximum size of 1 KB, found 2011 bytes" for attributes={"blob": "x"*2000}.

Cause 5: [mismatch] ivs-realtime.create-ingest-configuration-rtmp-requires-insecure-flag-rejected — actual {'Code': 'ValidationException', 'Message': 'CreateIngestConfiguration.insecureIngest is required to be true when ingestProtocol is RTMP, found false'}; expected "Request member: ingestProtocol failed to satisfy the constraints: insecure ingest is required for Ingest RTMP protocol".

Cause 6: [mismatch] ivs-realtime.encoder-configuration-pixel-product-exceeded-rejected — "AssertionError: expected 'Request member: video failed to satisfy the constraints: Code: VideoResolutionLimitError Message: video resolution exceeded max limit', got 'CreateEncoderConfiguration.video is required to have a maximum of 2073600 total pixels, found 3686400'".

Cause 7: [mismatch] ivs-realtime.get-stage-session-malformed-session-id-rejected — actual {'Code': 'ResourceNotFoundException', 'Message': 'Resource: st-0000000000000 not found'}; expected 'Resource: stage session "st-0000000000000" not found'. The same check's malformed-id assertion (ValidationException on sessionId not matching ^st-[a-zA-Z0-9]+$) passed.

## Steps to Reproduce
Cause 1: TagResource(resourceArn="arn:aws:ivs:<region>:<account>:encoder-configuration/<random>", tags={"k":"v"}). Expected Code NotFoundException, Message "Resource was not found"; actual ResourceNotFoundException / "Resource: <arn> not found". Repeat after creating and then deleting a real encoder configuration (and a stage) — same divergence on the dead ARN, while GetEncoderConfiguration/GetStage correctly answer ResourceNotFoundException.
Cause 2: CreateStage, then TagResource(resourceArn=<stage arn>, tags={f"k{i}": f"v{i}" for i in range(51)}). Expected Code BadRequestException, Message "The resultant tag set must not have more than 50 user tags.", no tag applied; actual ValidationException with the emulator's constraint sentence. Exactly 50 tags must still be accepted.
Cause 3: CreateStage, then CreateParticipantToken(stageArn=<stage arn>) with no duration and no capabilities. Expected participantToken WITHOUT `duration` and WITHOUT `capabilities`, expirationTime ~720 minutes out; actual duration=720 present in the response.
Cause 4: CreateParticipantToken(stageArn=<stage arn>, attributes={"blob": "x"*2000}). Expected 400 ValidationException "Request member: attributes failed to satisfy the constraints: attributes is required to have a maximum size of 1 KB, found 2011 bytes"; actual the "CreateParticipantToken.attributes … 1024 bytes, found 2004" sentence.
Cause 5: CreateIngestConfiguration(name=<n>, ingestProtocol="RTMP") with no insecureIngest. Expected 400 ValidationException "Request member: ingestProtocol failed to satisfy the constraints: insecure ingest is required for Ingest RTMP protocol"; actual the "CreateIngestConfiguration.insecureIngest …" sentence. The same request with insecureIngest=True must succeed.
Cause 6: CreateEncoderConfiguration(name=<n>, video={"width":1920,"height":1920}). Expected 400 ValidationException "Request member: video failed to satisfy the constraints: Code: VideoResolutionLimitError Message: video resolution exceeded max limit"; actual the "… maximum of 2073600 total pixels, found 3686400" sentence. 1920x1080 must succeed.
Cause 7: CreateStage, then GetStageSession(stageArn=<stage arn>, sessionId="st-0000000000000"). Expected 404 ResourceNotFoundException with Message 'Resource: stage session "st-0000000000000" not found'; actual "Resource: st-0000000000000 not found".
