Issue-Source: trace-sweep-divergence
Issue-Trace: kms-tag-error-paths
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kms-tag-error-paths kms-tagging-errors-unknown-and-malformed-key kms-grant-lifecycle-create-list-retire-revoke kms-multiple-tags-pagination-and-invalid-marker
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms|TagResource|4xx->4xx|$.message
Issue-Key: kms|RetireGrant|4xx->4xx|$.Message
Issue-Key: kms|ListResourceTags|4xx->4xx|$.message
Issue-Key: kms|ListResourceTags|4xx->4xx|$.__type
Issue-Key: kms|UntagResource|4xx->4xx|$.message
Issue-Key: kms|TagResource|4xx->4xx|$.__type
Issue-Commit: c1b8e21acf710dd592fea1840f63adefca5c3a99

# KMS tagging and grant refusals answer the wrong error class and emulator-authored messages

> _Note: this issue was identified against the code base at commit `c1b8e21acf710dd592fea1840f63adefca5c3a99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Six client-error paths answer HTTP 400 as AWS does but the BODY diverges: two of them name a different error shape than KMS does, three carry an emulator-authored sentence instead of KMS's wording, and one omits the message member entirely. All six are refusal-payload defects in the tagging and grant handlers' validation, not status-code defects. 6 causes.

Smithy facts used (services/service-com-amazonaws-kms-trentservice/smithy-model): TagResource/UntagResource bind TagException (client, httpError 400, member `message`), InvalidArnException (client, 400, `message`) and NotFoundException (client, 404 modeled, `message`); ListResourceTags binds InvalidMarkerException (client, 400, `message`), InvalidArnException and NotFoundException; RetireGrant binds InvalidGrantIdException, InvalidArnException, NotFoundException and KMSInvalidStateException. The model fixes the error SHAPES and their message member but not the message TEXT, so the recorded response is the requirement for wording.

Cause 1 of 6: TagResource duplicate tag keys — emulator-authored message.
AWS: refuses a Tags list containing the same TagKey twice with HTTP 400 and the fixed message 'Duplicate tag keys' — fixed text, no variable part, the offending key is NOT named.
Emulator: 400 with $.message 'One or more tags are not valid: the tag key Dup appears more than once.' — a sentence KMS does not use, which additionally interpolates the duplicated key name.
Why: a client matching on the refusal text to distinguish a duplicate-key mistake from other TagException causes never matches, and the emulator's variant leaks a request value into a message AWS keeps constant.
Fix: the TagResource validation path's TagException message (duplicate-key branch).
Traces: kms-tag-error-paths#task#2 (2 affected steps, also kms-tagging-errors-unknown-and-malformed-key#task#1).

Cause 2 of 6: RetireGrant on a grant that does not exist — message member omitted.
AWS: refuses with HTTP 400 and a Message member (capital M) reading 'User: <caller-arn> is not authorized to perform: kms:RetireGrant on this resource because the resource does not exist in this Region, no resource-based policies allow access, or a resource-based policy explicitly denies access'. Fixed text: 'User: ', ' is not authorized to perform: kms:RetireGrant on this resource because the resource does not exist in this Region, no resource-based policies allow access, or a resource-based policy explicitly denies access'. Variable part: the caller principal ARN, taken from the signing identity of the request (arn:aws:iam::000000000000:user/emulator in the recording — an environment value, not part of the template).
Emulator: answers 400 with the right error class but NO message member at all ($.Message: missing, got None).
Why: KMS deliberately reports a missing grant as an authorization failure rather than a not-found, and the sentence is how a caller learns that; with an empty body the client gets an untyped 400 it cannot explain or log.
Fix: the RetireGrant handler's refusal path — render the Message member on the not-authorized/does-not-exist branch.
Traces: kms-grant-lifecycle-create-list-retire-revoke#task#13.

Cause 3 of 6: ListResourceTags with an undecodable Marker — message member omitted.
AWS: refuses with HTTP 400 InvalidMarkerException whose message is 'Invalid NextMarker: <marker>' — fixed text 'Invalid NextMarker: ', variable part the Marker value echoed verbatim from the request.
Emulator: 400 with the message member absent ($.message: missing, got None).
Why: a paginating client that mishandles a marker gets no indication of which token was rejected, and cannot distinguish an invalid marker from any other 400.
Fix: the ListResourceTags marker validation — populate InvalidMarkerException.message with the request's Marker.
Traces: kms-multiple-tags-pagination-and-invalid-marker#task#5 (2 affected steps, also kms-tagging-errors-unknown-and-malformed-key#task#3).

Cause 4 of 6: ListResourceTags with a malformed key ARN — wrong error class AND wrong message.
AWS: when KeyId is a truncated/malformed KMS ARN, KMS answers 400 with __type NotFoundException and message 'Invalid arn <keyid>' — fixed text 'Invalid arn ', variable part the KeyId as supplied in the request. KMS reports this as a not-found, not as an ARN-syntax error.
Emulator: 400 with __type InvalidArnException and message 'The specified ARN is not valid: <keyid>.' (emulator-authored wording, plus a trailing period AWS does not emit).
Why: the error class is what SDKs branch on. A client catching NotFoundException to treat the key as absent falls through to its unhandled-exception path on InvalidArnException, so a benign missing-key case becomes a hard failure.
Fix: the ListResourceTags KeyId resolution path — classify a malformed ARN as NotFoundException with KMS's 'Invalid arn <keyid>' wording, instead of raising the generic ARN-syntax refusal.
Traces: kms-tag-error-paths#task#6.

Cause 5 of 6: UntagResource on a nonexistent key id — emulator-authored message.
AWS: refuses with HTTP 400 and message "Invalid keyId '<keyid>'" — fixed text "Invalid keyId '" and the closing quote, variable part the KeyId exactly as the request supplied it (a bare id, NOT expanded to an ARN).
Emulator: 400 with message "Key '<arn>' does not exist" — different wording, and it has rewritten the caller's bare key id into a fully qualified arn:aws:kms:<region>:<account>:key/<keyid> that the request never contained.
Why: a client that echoes or matches the refusal sees text AWS never produces, and the emulator's expansion discloses a resource ARN synthesized from an identifier AWS treats as invalid.
Fix: the UntagResource key-resolution refusal — use KMS's wording and echo the request's KeyId unmodified.
Traces: kms-tagging-errors-unknown-and-malformed-key#task#2.

Cause 6 of 6: TagResource with a malformed key ARN — wrong error class AND wrong message.
AWS: answers 400 with __type NotFoundException and message 'Invalid arn <token>', where <token> is the fragment of the supplied KeyId that KMS reports as unparseable (the recording shows 'us-east-1' for KeyId 'arn:aws:kms:us-east-1:not-an-account:key/'). Same classification rule as cause 4: a malformed ARN on the tagging surface is a NotFoundException.
Emulator: 400 with __type InvalidArnException and message 'The specified ARN is not valid: arn:aws:kms:us-east-1:not-an-account:key/.' — wrong class, emulator wording, and it echoes the whole ARN plus a trailing period rather than the offending token.
Why: same as cause 4 — the SDK-visible error type changes the client's control flow.
Fix: the TagResource KeyId resolution path, alongside cause 4's ListResourceTags fix (shared ARN-parsing refusal).
Traces: kms-tagging-errors-unknown-and-malformed-key#task#4.

Related: 20261029T175759Z-kms-7-net-new-refusal-fidelity-root-causes.md is marked FIXED and covers CreateAlias/Encrypt refusals, not any of the six paths above.

## Evidence
Replay of simparity-wire-aws-v1, 6 mismatch clusters, all expected_status 400 / actual_status 400 with a divergent body:
- TagResource, kms-tag-error-paths#task#2: $.message value_mismatch — expected 'Duplicate tag keys', got 'One or more tags are not valid: the tag key Dup appears more than once.'; 2 affected steps.
- RetireGrant, kms-grant-lifecycle-create-list-retire-revoke#task#13: $.Message missing — expected 'User: arn:aws:iam::000000000000:user/emulator is not authorized to perform: kms:RetireGrant on this resource because the resource does not exist in this Region, no resource-based policies allow access, or a resource-based policy explicitly denies access', got None.
- ListResourceTags, kms-multiple-tags-pagination-and-invalid-marker#task#5: $.message missing — expected 'Invalid NextMarker: not-a-real-marker-token', got None; 2 affected steps.
- ListResourceTags, kms-tag-error-paths#task#6: $.__type value_mismatch — expected 'NotFoundException', got 'InvalidArnException'; $.message value_mismatch — expected 'Invalid arn arn:aws:kms:us-east-1:not-an-account', got 'The specified ARN is not valid: arn:aws:kms:us-east-1:not-an-account.'.
- UntagResource, kms-tagging-errors-unknown-and-malformed-key#task#2: $.message value_mismatch — expected "Invalid keyId '000000000000-does-not-exist-0000-0000'", got "Key 'arn:aws:kms:us-west-2:000000000000:key/000000000000-does-not-exist-0000-0000' does not exist".
- TagResource, kms-tagging-errors-unknown-and-malformed-key#task#4: $.__type value_mismatch — expected 'NotFoundException', got 'InvalidArnException'; $.message value_mismatch — expected 'Invalid arn us-east-1', got 'The specified ARN is not valid: arn:aws:kms:us-east-1:not-an-account:key/.'.
Smithy cross-check: TagException/InvalidArnException/InvalidMarkerException/NotFoundException are all bound to these operations and all carry a `message` member, so neither the omission nor the class substitution is a modeling gap.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire corpus simparity-wire-aws-v1.
3. kms-tag-error-paths#task#2: TagResource with two Tags entries sharing one TagKey — expect 400 'Duplicate tag keys', observe the emulator's 'One or more tags are not valid: the tag key <key> appears more than once.'.
4. kms-grant-lifecycle-create-list-retire-revoke#task#13: RetireGrant naming a grant that no longer exists — expect a 400 carrying the kms:RetireGrant not-authorized Message, observe a 400 with no Message member.
5. kms-multiple-tags-pagination-and-invalid-marker#task#5: ListResourceTags with Marker='not-a-real-marker-token' — expect 400 'Invalid NextMarker: <marker>', observe a 400 with no message member.
6. kms-tag-error-paths#task#6 and kms-tagging-errors-unknown-and-malformed-key#task#4: ListResourceTags / TagResource with a malformed KMS ARN as KeyId — expect __type NotFoundException with 'Invalid arn <token>', observe __type InvalidArnException with 'The specified ARN is not valid: ...'.
7. kms-tagging-errors-unknown-and-malformed-key#task#2: UntagResource with a bare key id that does not exist — expect "Invalid keyId '<keyid>'", observe "Key '<arn>' does not exist" with the id expanded to an ARN.
