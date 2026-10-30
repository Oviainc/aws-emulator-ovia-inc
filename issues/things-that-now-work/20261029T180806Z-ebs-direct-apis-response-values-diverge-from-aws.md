Issue-Source: trace-sweep-divergence
Issue-Trace: ebs-snapshot-lifecycle-and-block-listing
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ebs-snapshot-lifecycle-and-block-listing ebs-put-snapshot-block-rejections ebs-start-snapshot-invalid-inputs
Issue-Shape-Id: com.amazonaws.ebs#Ebs
Issue-Key: ebs|CompleteSnapshot|2xx->2xx|$.Status
Issue-Key: ebs|PutSnapshotBlock|4xx->4xx|$.Message
Issue-Key: ebs|PutSnapshotBlock|4xx->4xx|$.Reason
Issue-Key: ebs|StartSnapshot|4xx->4xx|$.Message
Issue-Commit: ab6490681e1ab26d5567ff83a1d290924eb5dab0

# EBS direct APIs: response values diverge from AWS (snapshot status, validation reason, message text)

> _Note: this issue was identified against the code base at commit `ab6490681e1ab26d5567ff83a1d290924eb5dab0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CompleteSnapshot

## Description
Replaying the simparity-wire-aws-v1 EBS direct-API fixtures shows three root causes in which the emulator returns the CORRECT HTTP status but the WRONG value inside the response body: a snapshot-lifecycle status, a ValidationException Reason enum, and a ValidationException message. Because the status matches, nothing in a client's error/status plumbing flags the difference — the client simply reads a wrong value. All three are on service `ebs` (bootstrapped). The emulator was up (health 200, data listener accepting) and bootstrap_signal was false.

These three are distinct from open issue 20261023T060504Z-ebs-direct-apis-refusals-answer-the-wrong-http.md, which covers EBS refusals answered with the wrong HTTP STATUS and error CLASS (501/403/404/201 confusions). That issue carries no replay key for any of the three steps below, and none of its causes is about a right-status/wrong-value response.

=== Cause 1 of 3: CompleteSnapshot reports Status 'pending' where AWS reports 'completed' ===
AWS: Smithy com.amazonaws.ebs#CompleteSnapshot responds HTTP 202 (operation http code 202) with a single output member Status targeting com.amazonaws.ebs#Status, an enum whose values are exactly completed | pending | error. CompleteSnapshot is the call that seals a snapshot: once the declared ChangedBlocksCount matches the blocks that were written, the snapshot leaves the open/writable state and the recording shows Status 'completed' in that 202 body. The value is state-derived, not a literal: it is whatever state the snapshot record is in after the completion is applied.
Emulator: answers 202 but with Status 'pending' — the snapshot record is never transitioned by the completion, so the handler renders the pre-completion state. (Consistently with that, the same fixture's later read path still treats the snapshot as an open, empty, writable snapshot.)
Why: Status is the only completion signal CompleteSnapshot returns. A caller that waits for 'completed' before reading blocks, registering the snapshot with EC2, or declaring the backup durable either polls forever or concludes the snapshot is still being written; a caller that branches on 'pending' vs 'error' cannot distinguish a stalled snapshot from a finished one. Every workflow built on the EBS direct APIs ends at this call, so a wrong value here breaks the whole sequence even though each individual request succeeded.
Fix: the CompleteSnapshot handler and the snapshot state it mutates in the ebs service crate — apply the completion to the snapshot record (mark it sealed/completed when ChangedBlocksCount agrees with the blocks written) and render the Status member from that record, rather than returning the snapshot's prior state.
Traces: ebs-snapshot-lifecycle-and-block-listing#task#5.

=== Cause 2 of 3: PutSnapshotBlock with an unreadable/empty payload reports Reason INVALID_BLOCK and a byte-count message instead of INVALID_PARAMETER_VALUE / 'Failed to read block data' ===
AWS: Smithy PutSnapshotBlock input has BlockData @required @httpPayload @streaming (@sensitive blob) and DataLength @required bound to header x-amz-Data-Length; the operation is unsignedPayload. When the service cannot read the declared block payload off the request, it refuses with HTTP 400 com.amazonaws.ebs#ValidationException (@httpError 400) whose Reason is INVALID_PARAMETER_VALUE (a member of com.amazonaws.ebs#ValidationExceptionReason) and whose Message is the fixed text 'Failed to read block data' — the whole message is fixed text, it interpolates nothing from the request. The modeled meaning of the enum members is the distinction being lost: INVALID_PARAMETER_VALUE classifies a bad request parameter/payload read, while INVALID_BLOCK classifies a block that was read but is not acceptable.
Emulator: returns the right status (400 ValidationException) but Reason 'INVALID_BLOCK' and a constructed message of the form 'DataLength <declared x-amz-Data-Length> does not match the <received byte count> bytes of BlockData sent' — i.e. it reaches a length-comparison check and reports a content-level block rejection for what AWS classifies as a failure to read the payload at all.
Why: Reason is the enum clients and SDK error-mapping branch on, and the two members mean different remediations: INVALID_PARAMETER_VALUE says the request as framed is wrong (fix x-amz-Data-Length / resend the body), INVALID_BLOCK says this particular block's data was rejected (re-put a different block). Since the HTTP status and error shape are both correct, nothing surfaces the divergence — a client written against AWS silently takes the wrong recovery branch, and a test asserting the documented Reason fails against the emulator while the operation looks healthy.
Fix: PutSnapshotBlock's payload-read and input-validation path in the ebs service crate — when the declared block payload cannot be read (including a zero-length body against a non-zero x-amz-Data-Length), raise ValidationException with Reason INVALID_PARAMETER_VALUE and the fixed message 'Failed to read block data'; reserve INVALID_BLOCK for a block that was read and then rejected.
Traces: ebs-put-snapshot-block-rejections#task#5.

=== Cause 3 of 3: StartSnapshot's mutually-exclusive ParentSnapshotId+Encrypted refusal uses the emulator's own wording, not AWS's ===
AWS: Smithy StartSnapshot declares ValidationException (@httpError 400) and its input carries both ParentSnapshotId (SnapshotId, pattern ^snap-[0-9a-f]+$) and Encrypted (boolean); a snapshot taken from a parent inherits the parent's encryption, so the two cannot be requested together. The recording shows HTTP 400 ValidationException whose Message is 'ParentSnapshotId and Encrypt cannot be specified together'. That message is entirely fixed text — it names the two input members (and AWS words the second one 'Encrypt', not 'Encrypted') and interpolates no value from the request or from state.
Emulator: same status (400) and same Reason, but Message 'Encrypted and ParentSnapshotId cannot both be specified in the same request' — the members are named in the opposite order, the second is spelled 'Encrypted', and the trailing clause is re-worded.
Why: the error message is part of the API contract for this error class. Clients, CLI users and conformance tests that match on the refusal text do not recognise the emulator's wording, and because the status and Reason are both correct the divergence is invisible to any status/class-level check — it only shows up as a failed text assertion or a human reading an unfamiliar message.
Fix: StartSnapshot's input validation in the ebs service crate — emit AWS's fixed message text for the ParentSnapshotId/Encrypted mutual-exclusion refusal verbatim, rather than a locally phrased equivalent.
Traces: ebs-start-snapshot-invalid-inputs#task#2.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout: 9 fixtures replayed, 61 steps, 37 passed, 24 mismatched, score 60.7%, emulator_up=true, bootstrap_signal=false. The three causes below are the divergence clusters with no stub_path and no owning open issue.

Cause 1 — ebs-snapshot-lifecycle-and-block-listing#task#5 CompleteSnapshot: expected status 202 with $.Status='completed'; actual status 202 with $.Status='pending'. Divergence key service=ebs|CompleteSnapshot|status=202->202|keys=$.Status (affected_count 1).

Cause 2 — ebs-put-snapshot-block-rejections#task#5 PutSnapshotBlock: expected status 400 with $.Reason='INVALID_PARAMETER_VALUE' and $.Message='Failed to read block data'; actual status 400 with $.Reason='INVALID_BLOCK' and $.Message='DataLength 524288 does not match the 0 bytes of BlockData sent' (the 524288 and 0 are this run's declared length and received byte count — the fixed part of AWS's message is the whole string 'Failed to read block data'). Divergence key service=ebs|PutSnapshotBlock|status=400->400|keys=$.Message,$.Reason (affected_count 1).

Cause 3 — ebs-start-snapshot-invalid-inputs#task#2 StartSnapshot(ParentSnapshotId + Encrypted together): expected status 400 with $.Message='ParentSnapshotId and Encrypt cannot be specified together'; actual status 400 with $.Message='Encrypted and ParentSnapshotId cannot both be specified in the same request'. Both messages are pure fixed text; no request value is interpolated by either. Divergence key service=ebs|StartSnapshot|status=400->400|keys=$.Message (affected_count 1).

## Steps to Reproduce
1. Build and boot the emulator from this checkout and point an `ebs` client (restJson1, sigv4) at the data listener.
2. Cause 1: replay ebs-snapshot-lifecycle-and-block-listing through step task#5 — StartSnapshot(VolumeSize=1), PutSnapshotBlock for the block(s), then POST /snapshots/completion/{SnapshotId} with x-amz-ChangedBlocksCount matching the blocks written. Expect 202 with Status='completed'; observe 202 with Status='pending'.
3. Cause 2: replay ebs-put-snapshot-block-rejections step task#5 — PUT /snapshots/{SnapshotId}/blocks/0 declaring x-amz-Data-Length=524288 with a valid checksum header but an empty request body. Expect 400 ValidationException with Reason='INVALID_PARAMETER_VALUE' and Message='Failed to read block data'; observe 400 with Reason='INVALID_BLOCK' and a 'DataLength ... does not match the ... bytes of BlockData sent' message.
4. Cause 3: replay ebs-start-snapshot-invalid-inputs step task#2 — POST /snapshots with both ParentSnapshotId and Encrypted set. Expect 400 ValidationException with Message='ParentSnapshotId and Encrypt cannot be specified together'; observe the emulator's re-worded 'Encrypted and ParentSnapshotId cannot both be specified in the same request'.
5. All three re-replay via the fixture steps named in Issue-Trace/Issue-Step and Issue-Trace-Ids; each must be closed before submitting.
