Issue-Source: trace-sweep-divergence
Issue-Trace: ebs-get-snapshot-block-token-rejections
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ebs-get-snapshot-block-token-rejections ebs-put-snapshot-block-rejections ebs-snapshot-lifecycle-and-block-listing ebs-start-snapshot-idempotency-and-validation ebs-start-snapshot-invalid-inputs
Issue-Shape-Id: com.amazonaws.ebs#Ebs
Issue-Key: ebs|StartSnapshot|2xx->2xx|$.SseType
Issue-Key: ebs|StartSnapshot|2xx->2xx|$.Tags
Issue-Key: ebs|CompleteSnapshot|2xx->2xx|$.Status
Issue-Key: ebs|GetSnapshotBlock|4xx->4xx|$.message
Issue-Key: ebs|PutSnapshotBlock|4xx->4xx|$.Message
Issue-Key: ebs|PutSnapshotBlock|4xx->4xx|$.Reason
Issue-Key: ebs|StartSnapshot|4xx->4xx|$.Message
Issue-Commit: c0919b0ae1fd753648687abf4b08014f282092d3

# EBS direct APIs: success and error response bodies carry wrong members and messages

> _Note: this issue was identified against the code base at commit `c0919b0ae1fd753648687abf4b08014f282092d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartSnapshot

## Description
Replaying the simparity-wire-aws-v1 EBS direct-API fixtures shows seven further root causes in which the status and error class are right but the RESPONSE BODY diverges: members AWS omits are serialized, a lifecycle status value is wrong, a modeled error envelope is emitted without its members, a constraint pattern is narrower than the service's, and three refusal messages whose rule is known are locally authored. All seven are on service `ebs`; state_reset_failures was 0.

=== Cause 1 of 7: StartSnapshot serializes SseType='none' and Tags=[] where AWS omits both members ===
AWS: Smithy com.amazonaws.ebs#StartSnapshotResponse members SseType (target SSEType, enum values sse-ebs|sse-kms|none) and Tags (target Tags, list) are optional with no @default; restJson1 omits an absent member rather than emitting a placeholder. The recording of an unencrypted, untagged StartSnapshot (HTTP 201) contains NEITHER key.
Emulator: emits "SseType": "none" on every StartSnapshot response and, where no tags were supplied, also "Tags": [].
Why: clients (and generated SDK model code) distinguish 'the service did not report an SSE type' from 'the service reported none', and `if 'Tags' in response` / presence-based conditionals flip. A caller round-tripping the response back into StartSnapshot also now sends an explicit SseType AWS never returned.
Fix: the StartSnapshot response serializer — only populate SseType and Tags when the snapshot actually has them, and suppress empty/defaulted optional members instead of materialising them.
Traces: ebs-get-snapshot-block-token-rejections#setup#1 (SseType), ebs-start-snapshot-invalid-inputs#setup#1 (SseType and Tags).

=== Cause 2 of 7: CompleteSnapshot returns Status 'completed' immediately instead of 'pending' ===
AWS: Smithy CompleteSnapshot is POST /snapshots/completion/{SnapshotId} with http code 202 (Accepted) and returns Status (enum com.amazonaws.ebs#Status: completed|pending|error); sealing is asynchronous, and the recording of the accepted request carries Status 'pending' — the terminal 'completed' is observed later through the snapshot read path, not in the 202 body.
Emulator: answers 202 with Status 'completed' — it seals synchronously and reports the terminal state in the acknowledgement.
Why: a client that waits for pending->completed has nothing to wait for, so any code exercising the asynchronous completion path (polling, timeouts, error-on-seal handling) is never exercised and a test written against AWS's 'pending' acknowledgement fails against the emulator.
Fix: the CompleteSnapshot handler and the snapshot state machine — the 202 acknowledgement must report the snapshot as pending, with the transition to completed observable through the read path afterwards.
Traces: ebs-get-snapshot-block-token-rejections#setup#2.

=== Cause 3 of 7: the blockToken constraint the emulator enforces is narrower than the service's, so valid tokens are rejected ===
AWS: the validation refusal for an empty blockToken is a framework-generated constraint message built from the request — it names the value, the member and the regular expression the service actually enforces: "1 validation error detected: Value '' at 'blockToken' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9+/=_-]+$" (HTTP 400). That pattern is the requirement expressed by the message; note the on-disk Smithy model's GetSnapshotBlockRequest.BlockToken pattern is the narrower ^[A-Za-z0-9+/=]+$, so the live service accepts '_' and '-' (URL-safe base64) that the model's pattern does not.
Emulator: same status and same message template, but it reports (and therefore enforces) ^[A-Za-z0-9+/=]+$ — so a service-issued URL-safe-base64 BlockToken containing '_' or '-' is refused as a constraint violation instead of being accepted.
Why: this is not only a wording difference: a client passing back a token AWS would mint and accept gets a 400 from the emulator, which makes block reads intermittently impossible depending on the token's alphabet.
Fix: the GetSnapshotBlock blockToken validation (the operation's input-validation layer) — widen the enforced pattern to the service's ^[A-Za-z0-9+/=_-]+$ and render the constraint message from the pattern actually enforced.
Traces: ebs-get-snapshot-block-token-rejections#task#4.

=== Cause 4 of 7: PutSnapshotBlock validation refusals carry neither Message nor Reason ===
AWS: Smithy com.amazonaws.ebs#ValidationException (@httpError 400) has members Message and Reason (enum ValidationExceptionReason). For an unsupported checksum algorithm the recording is HTTP 400 with Reason 'INVALID_PARAMETER_VALUE' and a request-derived message naming the rejected algorithm and the supported set (example: "Unsupported Checksum Algorithm 'MD5', supported algorithms are SHA256"; Smithy: ChecksumAlgorithm is @required and modeled as an enum).
Emulator: the status is 400 but both modeled members are ABSENT from the body — the asserted $.Message and $.Reason are missing, i.e. the refusal is rendered through an envelope that does not carry the modeled ValidationException members at all. Two steps of the same fixture show it.
Why: every SDK surfaces error['Message'] and the Reason enum is the only machine-readable discriminator among the fifteen ValidationExceptionReason values. With both absent a client gets an empty error, cannot attribute the rejected input, and cannot branch on the reason.
Fix: the ebs error serializer for ValidationException (and the PutSnapshotBlock validation paths that raise it) — always populate Message from the request-derived template and set Reason to the matching ValidationExceptionReason member.
Traces: ebs-put-snapshot-block-rejections#task#3 (also task#5, same cluster).

=== Cause 5 of 7: StartSnapshot ClientToken conflict returns a locally authored ConflictException message ===
AWS: Smithy com.amazonaws.ebs#ConflictException (@httpError 409, doc 'The request uses the same client token as a previous, but non-identical request') is declared on StartSnapshot. The recorded message is FIXED and request-independent — 'Current request does not match request stored with this token.' — so the constant string is the rule here, not a sample of a varying text, and in particular AWS does not echo the caller's token.
Emulator: correct 409/ConflictException but the message embeds the token and operation: 'ClientToken <token> was already used for a different StartSnapshot request'.
Why: idempotency handling is one of the few places clients match on refusal text, and echoing the caller-supplied token leaks a value AWS deliberately does not return.
Fix: the StartSnapshot handler's client-token conflict path — emit the fixed ConflictException message. (Open issue 20261022T224826Z-ebs-direct-apis-ec2-describesnapshots-stub-blocks-23.md raises this as its cause 6 from the behavioral suite but carries no wire replay key for this fixture step.)
Traces: ebs-start-snapshot-idempotency-and-validation#task#4.

=== Cause 6 of 7: StartSnapshot volume-size refusal message is not the AWS request-derived template ===
AWS: Smithy StartSnapshotRequest.VolumeSize is @required with @range min 1; an out-of-range size is refused HTTP 400 ValidationException with a message built from the request that names only the rejected value — 'Invalid volume size: <VolumeSize>' (recording: 'Invalid volume size: 99999'). The template is the requirement; the value varies with the request.
Emulator: right status and class, but the message is locally worded and asserts a bound AWS does not state here: 'VolumeSize 99999 exceeds the maximum snapshot volume size of 65536 GiB'.
Why: callers and test suites that attribute which input was rejected by matching the AWS template diverge, and the emulator's invented 65536 GiB bound reads as a service-stated limit it is not.
Fix: the StartSnapshot validation layer's message rendering for the VolumeSize refusal (and set Reason to ValidationExceptionReason INVALID_VOLUME_SIZE).
Traces: ebs-start-snapshot-idempotency-and-validation#task#5 (also ebs-start-snapshot-invalid-inputs#task#2, same cluster).

=== Cause 7 of 7: StartSnapshot with a nonexistent ParentSnapshotId uses a locally worded not-found message ===
AWS: Smithy StartSnapshotRequest.ParentSnapshotId (pattern ^snap-[0-9a-f]+$) with StartSnapshot declaring ResourceNotFoundException (@httpError 404). The refusal is the service's standard request-derived snapshot not-found template, naming the snapshot id: "The snapshot '<ParentSnapshotId>' does not exist." with Reason SNAPSHOT_NOT_FOUND — the same template the ListSnapshotBlocks recordings use, so the rule is the template, not the one recorded string.
Emulator: right status (404) and Reason, but the message is 'no completed snapshot <id> does exist to use as ParentSnapshotId' — a different template that also conflates 'absent' with 'not yet completed'.
Why: a caller cannot match the service's not-found wording, and the emulator's phrasing implies a distinct 'exists but incomplete' condition that this refusal does not represent.
Fix: the StartSnapshot parent-snapshot resolution path — render the shared request-derived snapshot not-found message.
Traces: ebs-start-snapshot-invalid-inputs#task#3.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout: 6 fixtures, 37 steps, 6 passed, 31 mismatched, score 16.2%, emulator_up=true, state_reset_failures=0.

Cause 1 — ebs-get-snapshot-block-token-rejections#setup#1 StartSnapshot (201): '$.SseType: type_mismatch (expected None, got \'none\')'. ebs-start-snapshot-invalid-inputs#setup#1 StartSnapshot (201): '$.SseType: type_mismatch (expected None, got \'none\')' and '$.Tags: type_mismatch (expected None, got [])'. Cluster also covers ebs-put-snapshot-block-rejections#setup#1, ebs-snapshot-lifecycle-and-block-listing#task#1, ebs-start-snapshot-idempotency-and-validation#task#3.

Cause 2 — ebs-get-snapshot-block-token-rejections#setup#2 CompleteSnapshot (202): '$.Status: value_mismatch (expected \'pending\', got \'completed\')'. Same cluster: ebs-snapshot-lifecycle-and-block-listing#task#3.

Cause 3 — ebs-get-snapshot-block-token-rejections#task#4 GetSnapshotBlock (400 both sides): expected $.message "1 validation error detected: Value '' at 'blockToken' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9+/=_-]+$"; actual the same sentence with pattern '^[A-Za-z0-9+/=]+$'.

Cause 4 — ebs-put-snapshot-block-rejections#task#3 PutSnapshotBlock (400 both sides): '$.Message: missing (expected "Unsupported Checksum Algorithm \'MD5\', supported algorithms are SHA256", got None)'; '$.Reason: missing (expected \'INVALID_PARAMETER_VALUE\', got None)'. Same cluster: ebs-put-snapshot-block-rejections#task#5.

Cause 5 — ebs-start-snapshot-idempotency-and-validation#task#4 StartSnapshot (409 both sides): expected $.Message 'Current request does not match request stored with this token.'; actual 'ClientToken snap-055f871ca34010e74 was already used for a different StartSnapshot request'.

Cause 6 — ebs-start-snapshot-idempotency-and-validation#task#5 StartSnapshot (400 both sides): expected $.Message 'Invalid volume size: 99999'; actual 'VolumeSize 99999 exceeds the maximum snapshot volume size of 65536 GiB'. Same cluster: ebs-start-snapshot-invalid-inputs#task#2.

Cause 7 — ebs-start-snapshot-invalid-inputs#task#3 StartSnapshot (404 both sides): expected $.Message "The snapshot 'snap-0000000000000dead' does not exist."; actual 'no completed snapshot snap-0000000000000dead exists to use as ParentSnapshotId'.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and point an `ebs` client at the data listener.
2. Cause 1: StartSnapshot(VolumeSize=1) with no Tags and no encryption (ebs-get-snapshot-block-token-rejections#setup#1 / ebs-start-snapshot-invalid-inputs#setup#1) and inspect the 201 body — 'SseType' and 'Tags' keys must be absent; observe SseType='none' and Tags=[].
3. Cause 2: StartSnapshot, one PutSnapshotBlock, then CompleteSnapshot(ChangedBlocksCount=1) (ebs-get-snapshot-block-token-rejections#setup#2) and read Status from the 202 body — expect 'pending', observe 'completed'.
4. Cause 3: GET /snapshots/{snap}/blocks/0?blockToken= (empty) (ebs-get-snapshot-block-token-rejections#task#4) and read the constraint pattern quoted in the 400 message — expect ^[A-Za-z0-9+/=_-]+$; observe ^[A-Za-z0-9+/=]+$. Then pass a token containing '_' or '-' and observe it refused.
5. Cause 4: PutSnapshotBlock with x-amz-Checksum-Algorithm: MD5 (ebs-put-snapshot-block-rejections#task#3) and inspect the 400 body for Message and Reason — both absent.
6. Cause 5: StartSnapshot(VolumeSize=1, ClientToken=T) then StartSnapshot with the same ClientToken and a different payload (ebs-start-snapshot-idempotency-and-validation#task#4); inspect the 409 Message.
7. Cause 6: StartSnapshot(VolumeSize=99999) (ebs-start-snapshot-idempotency-and-validation#task#5); inspect the 400 Message.
8. Cause 7: StartSnapshot(VolumeSize=1, ParentSnapshotId='snap-0000000000000dead') (ebs-start-snapshot-invalid-inputs#task#3); inspect the 404 Message.
