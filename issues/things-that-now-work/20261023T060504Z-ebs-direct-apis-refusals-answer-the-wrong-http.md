Issue-Source: trace-sweep-divergence
Issue-Trace: ebs-put-snapshot-block-rejections
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ebs-put-snapshot-block-rejections ebs-get-snapshot-block-token-rejections ebs-snapshot-lifecycle-and-block-listing ebs-start-snapshot-invalid-inputs
Issue-Shape-Id: com.amazonaws.ebs#Ebs
Issue-Key: ebs|PutSnapshotBlock|4xx->5xx|$.Message
Issue-Key: ebs|PutSnapshotBlock|4xx->5xx|$.Reason
Issue-Key: ebs|PutSnapshotBlock|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: ebs|PutSnapshotBlock|4xx->5xx|status
Issue-Key: ebs|GetSnapshotBlock|4xx->4xx|$
Issue-Key: ebs|GetSnapshotBlock|4xx->4xx|$.Message
Issue-Key: ebs|GetSnapshotBlock|4xx->4xx|$.Reason
Issue-Key: ebs|GetSnapshotBlock|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: ebs|GetSnapshotBlock|4xx->4xx|status
Issue-Key: ebs|ListSnapshotBlocks|4xx->4xx|$.Message
Issue-Key: ebs|ListSnapshotBlocks|4xx->4xx|$.Reason
Issue-Key: ebs|ListSnapshotBlocks|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: ebs|ListSnapshotBlocks|4xx->4xx|status
Issue-Key: ebs|PutSnapshotBlock|4xx->4xx|$.message
Issue-Key: ebs|PutSnapshotBlock|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: ebs|PutSnapshotBlock|4xx->4xx|status
Issue-Key: ebs|ListSnapshotBlocks|4xx->2xx|$.Message
Issue-Key: ebs|ListSnapshotBlocks|4xx->2xx|$.Reason
Issue-Key: ebs|ListSnapshotBlocks|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: ebs|ListSnapshotBlocks|4xx->2xx|status
Issue-Key: ebs|ListSnapshotBlocks|4xx->2xx|$
Issue-Key: ebs|StartSnapshot|4xx->2xx|$.Message
Issue-Key: ebs|StartSnapshot|4xx->2xx|$.Reason
Issue-Key: ebs|StartSnapshot|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: ebs|StartSnapshot|4xx->2xx|status
Issue-Commit: c0919b0ae1fd753648687abf4b08014f282092d3

# EBS direct APIs: refusals answer the wrong HTTP status and error class

> _Note: this issue was identified against the code base at commit `c0919b0ae1fd753648687abf4b08014f282092d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSnapshotBlock

## Description
Replaying the simparity-wire-aws-v1 EBS direct-API fixtures (6 fixtures, 37 steps, 6 passed) shows six distinct root causes in which an EBS refusal is answered with a DIFFERENT HTTP status and a different modeled error shape than AWS returns — including two cases where the emulator answers success or NotImplemented where AWS rejects. All six are on service `ebs`. The emulator was up (health 200, data listener accepting) and state_reset_failures was 0, so none of these are cross-fixture state bleed.

=== Cause 1 of 6: PutSnapshotBlock with no x-amz-Checksum header answers 501 NotImplemented instead of 400 ValidationException ===
AWS: Smithy com.amazonaws.ebs#PutSnapshotBlock input member Checksum is @required and bound as httpHeader x-amz-Checksum (ChecksumAlgorithm likewise @required); a request that omits it cannot be served and is a client input error. The recording shows HTTP 400 with the modeled com.amazonaws.ebs#ValidationException (@httpError 400), Reason INVALID_PARAMETER_VALUE (a member of com.amazonaws.ebs#ValidationExceptionReason), x-amzn-errortype 'ValidationException:http://internal.amazon.com/coral/com.amazon.zeppelindataservice/', message example 'Missing checksum for request payload'.
Emulator: HTTP 501 with x-amzn-errortype 'NotImplemented' and no Message/Reason members at all. PutSnapshotBlock itself IS implemented (other steps of the same fixture reach its validation), so this is the missing-required-header path falling through to the not-implemented responder rather than an unimplemented operation.
Why: 501 is a server-side 'operation does not exist' signal. An SDK/client treats it as non-retryable infrastructure breakage rather than as 'fix your request', so a caller that forgot the checksum header gets no indication which input was wrong, and anything branching on 4xx-vs-5xx takes the wrong path.
Fix: PutSnapshotBlock's request validation (the operation's input-validation layer in the ebs service crate) — reject a missing required Checksum/ChecksumAlgorithm header as the modeled ValidationException with Reason INVALID_PARAMETER_VALUE, never the not-implemented responder.
Traces: ebs-put-snapshot-block-rejections#task#2.

=== Cause 2 of 6: PutSnapshotBlock whose Checksum does not match the payload is 400 ValidationException instead of 403 InvalidSignatureException ===
AWS: PutSnapshotBlock carries an unsigned streaming @httpPayload (Smithy: BlockData @streaming/@httpPayload, operation marked unsignedPayload), so the x-amz-Checksum header is the request's integrity binding. The recording shows HTTP 403 with x-amzn-errortype 'InvalidSignatureException:http://internal.amazon.com/coral/com.amazon.coral.service/' and a request-derived message naming the computed and supplied checksums (example: 'The value passed in as x-amz-Checksum does not match the computed checksum. Computed checksum: <sha256-b64 of received bytes> expected checksum: <value sent>'); the rule is that both checksums are rendered from the request.
Emulator: HTTP 400 with x-amzn-errortype 'ValidationException' and no message member in the asserted position.
Why: 403/InvalidSignatureException is a signing-integrity class a client resigns or fails fast on; 400/ValidationException is a flat input error. Retry policies and credential-refresh logic keyed on the signing class never fire, and a client cannot tell a corrupted upload from a malformed request.
Fix: the PutSnapshotBlock handler's checksum verification — classify a payload/checksum mismatch as the 403 signing-integrity error and render the computed-vs-supplied checksum message from the request. Note: open issue 20261022T224826Z-ebs-direct-apis-ec2-describesnapshots-stub-blocks-23.md (behavioral corpus) describes this same misclassification as its cause 2, but it carries no wire replay key for this fixture step, so this cause is filed here with one.
Traces: ebs-put-snapshot-block-rejections#task#4.

=== Cause 3 of 6: GetSnapshotBlock with an invalid BlockToken is 404 ResourceNotFoundException/SNAPSHOT_NOT_FOUND instead of 400 ValidationException/INVALID_BLOCK_TOKEN ===
AWS: the BlockToken is the authorisation for a block read (Smithy: GetSnapshotBlock.BlockToken @required, httpQuery blockToken). Against an existing, completed snapshot, a well-formed but never-issued token is refused as HTTP 400 ValidationException with Reason INVALID_BLOCK_TOKEN (a member of ValidationExceptionReason) and a request-derived message naming the token (example: "The block token QUJD is invalid.").
Emulator: HTTP 404, x-amzn-errortype 'ResourceNotFoundException', Reason 'SNAPSHOT_NOT_FOUND', message 'snapshot <id> has no block at index 0' — it reports the snapshot as absent even though the fixture created and completed that snapshot in the same session, i.e. the lookup is keyed on (snapshot, index) and a miss is reported as a missing snapshot rather than as a rejected token. The fixture then polled for 60s/13 attempts waiting for the 400 that never arrives.
Why: the caller is told the snapshot does not exist, so it abandons a snapshot that is in fact present and readable, instead of re-listing to obtain a fresh token. Reason SNAPSHOT_NOT_FOUND vs INVALID_BLOCK_TOKEN is exactly the distinction the enums exist to carry.
Fix: the GetSnapshotBlock handler's token validation and its precedence — validate the token against the tokens the service minted and raise 400 ValidationException/INVALID_BLOCK_TOKEN; only a genuinely absent snapshot may produce 404/SNAPSHOT_NOT_FOUND.
Traces: ebs-get-snapshot-block-token-rejections#task#3.

=== Cause 4 of 6: ListSnapshotBlocks with a syntactically invalid snapshot id is 404 ResourceNotFoundException instead of 400 ValidationException/INVALID_SNAPSHOT_ID ===
AWS: the snapshot id is checked for form before existence. The recording shows HTTP 400 ValidationException with Reason INVALID_SNAPSHOT_ID (ValidationExceptionReason member) and a request-derived message naming the parameter and the rejected value (example: "Value (snap-00000000000000000) for parameter snapshotId is invalid. Expected: 'snap-...'."). Smithy: ListSnapshotBlocks.SnapshotId @required httpLabel, length 1..64, pattern ^snap-[0-9a-f]+$ — the service's id check is stricter than the modeled pattern, and the recorded 400 is what it produces.
Emulator: HTTP 404, x-amzn-errortype 'ResourceNotFoundException', Reason 'SNAPSHOT_NOT_FOUND', message 'snapshot snap-00000000000000000 does not exist' — it skips the id-format check and goes straight to a store lookup.
Why: a client cannot distinguish 'you sent a malformed id' (fix the request, never retry) from 'that snapshot is gone' (possibly a propagation delay worth retrying), and tooling that validates ids by probing gets the wrong class.
Fix: the ListSnapshotBlocks input validation (shared snapshot-id parsing in the ebs service crate) — run the id-format check before the existence lookup and emit 400 ValidationException/INVALID_SNAPSHOT_ID.
Traces: ebs-get-snapshot-block-token-rejections#task#5.

=== Cause 5 of 6: ListSnapshotBlocks answers 200 with an empty listing for a snapshot that is not readable, where AWS answers 404 ===
AWS: a snapshot that has not been completed (and one that no longer exists) is not visible to the read path. The recording shows HTTP 404 ResourceNotFoundException with Reason SNAPSHOT_NOT_FOUND and a request-derived message naming the snapshot (example: "The snapshot 'snap-ebae01ebd38b3f8dd' does not exist."). Smithy: ResourceNotFoundException @httpError 404, Reason target ResourceNotFoundExceptionReason includes SNAPSHOT_NOT_FOUND; ListSnapshotBlocks declares ResourceNotFoundException.
Emulator: HTTP 200 with a well-formed but empty body ({'Blocks': [], 'ExpiryTime': ..., 'VolumeSize': 1, 'BlockSize': 524288}) and no error members — both for a snapshot that is still pending (fixture task#2, before CompleteSnapshot) and at teardown after the snapshot should no longer be listable (teardown#7, where the fixture polled 13 attempts/60s for the 404). Note the teardown case also sits downstream of the already-filed ec2 DeleteSnapshot stub, but the read-path gate is the ebs-side defect in both.
Why: clients use readability as the completion/liveness signal. An empty 200 says 'this snapshot exists and has no blocks', so a caller concludes a half-written snapshot is complete-but-empty, or that a removed snapshot is still valid, instead of taking the not-found branch.
Fix: the read-path snapshot lookup shared by ListSnapshotBlocks (and GetSnapshotBlock) — gate it on the snapshot being completed and present, and raise 404 ResourceNotFoundException/SNAPSHOT_NOT_FOUND otherwise. Open issue 20261022T224826Z-ebs-direct-apis-ec2-describesnapshots-stub-blocks-23.md raises the readable-open-snapshot behaviour as its cause 3 from the behavioral suite, but carries no wire replay key for these steps.
Traces: ebs-snapshot-lifecycle-and-block-listing#task#2, ebs-snapshot-lifecycle-and-block-listing#teardown#7.

=== Cause 6 of 6: StartSnapshot with a KmsKeyArn that does not exist SUCCEEDS (201) where AWS rejects it 400 ===
AWS: StartSnapshot resolves the supplied KMS key through KMS before creating the snapshot. The recording shows HTTP 400 ValidationException with Reason INVALID_DEPENDENCY_REQUEST (ValidationExceptionReason member), x-amzn-errortype 'ValidationException:http://internal.amazon.com/coral/com.amazon.zeppelindataservice/', and a message that wraps the dependent service's failure (example: "Invalid parameters for a dependent service: com.amazonaws.services.kms.model.NotFoundException: Key '<arn>' does not exist (Service: AWSKMS; Status Code: 400; Error Code: NotFoundException; ...)" — the embedded request id is environment-specific). Smithy: StartSnapshotRequest.KmsKeyArn pattern ^arn:aws[a-z\\-]*:kms:.*:[0-9]{12}:key/, and StartSnapshot declares ValidationException.
Emulator: HTTP 201 with a normal StartSnapshotResponse — the key reference is accepted unvalidated and a snapshot is created.
Why: the caller believes it has a snapshot encrypted under a key that does not exist. The failure surfaces later (or never), encrypted-snapshot error handling is untestable against the emulator, and tests that assert the dependency refusal pass incorrectly.
Fix: the StartSnapshot handler — resolve Encrypted/KmsKeyArn against the KMS key store before creating snapshot state and refuse an unresolvable key with 400 ValidationException/INVALID_DEPENDENCY_REQUEST.
Traces: ebs-start-snapshot-invalid-inputs#task#4.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout: 6 fixtures replayed, 37 steps, 6 passed, 31 mismatched, score 16.2%, emulator_up=true, state_reset_failures=0, bootstrap_signal=false.

Cause 1 — ebs-put-snapshot-block-rejections#task#2 PutSnapshotBlock: expected status 400 with $.Reason='INVALID_PARAMETER_VALUE', $.Message='Missing checksum for request payload' and header x-amzn-errortype='ValidationException:http://internal.amazon.com/coral/com.amazon.zeppelindataservice/'. Actual: status 501, x-amzn-errortype='NotImplemented', Message and Reason absent.

Cause 2 — ebs-put-snapshot-block-rejections#task#4 PutSnapshotBlock: expected status 403, x-amzn-errortype='InvalidSignatureException:http://internal.amazon.com/coral/com.amazon.coral.service/', $.message naming computed vs expected checksum. Actual: status 400, x-amzn-errortype='ValidationException', no $.message.

Cause 3 — ebs-get-snapshot-block-token-rejections#task#3 GetSnapshotBlock: expected status 400 with {'Message': 'The block token QUJD is invalid.', 'Reason': 'INVALID_BLOCK_TOKEN'}. Actual: status 404 with {'__type': 'ResourceNotFoundException', 'Message': 'snapshot snap-6989d1073d0185eea has no block at index 0', 'Reason': 'SNAPSHOT_NOT_FOUND'}; poll gave up after 13 attempts/60s.

Cause 4 — ebs-get-snapshot-block-token-rejections#task#5 ListSnapshotBlocks: expected status 400, $.Reason='INVALID_SNAPSHOT_ID', $.Message="Value (snap-00000000000000000) for parameter snapshotId is invalid. Expected: 'snap-...'.". Actual: status 404, $.Reason='SNAPSHOT_NOT_FOUND', $.Message='snapshot snap-00000000000000000 does not exist', x-amzn-errortype='ResourceNotFoundException'.

Cause 5 — ebs-snapshot-lifecycle-and-block-listing#task#2 ListSnapshotBlocks (snapshot started, not completed): expected status 404, $.Reason='SNAPSHOT_NOT_FOUND'. Actual: status 200, no error members. Same at ebs-snapshot-lifecycle-and-block-listing#teardown#7: expected 404, actual 200 with body {'Blocks': [], 'ExpiryTime': ..., 'VolumeSize': 1, 'BlockSize': 524288} after 13 polling attempts.

Cause 6 — ebs-start-snapshot-invalid-inputs#task#4 StartSnapshot with a nonexistent KMS key arn: expected status 400, $.Reason='INVALID_DEPENDENCY_REQUEST', x-amzn-errortype='ValidationException:...zeppelindataservice/'. Actual: status 201, no error members.

## Steps to Reproduce
1. Build and boot the emulator from this checkout; point an `ebs` client (restJson1, sigv4) at the data listener.
2. Cause 1: replay ebs-put-snapshot-block-rejections step task#2 — PUT /snapshots/{snap}/blocks/0 with a 524288-byte body and x-amz-Data-Length but NO x-amz-Checksum header. Expect 400 ValidationException/INVALID_PARAMETER_VALUE; observe 501 NotImplemented.
3. Cause 2: same fixture step task#4 — PutSnapshotBlock with a valid 524288-byte payload and the base64 SHA256 of DIFFERENT bytes. Expect 403 InvalidSignatureException; observe 400 ValidationException.
4. Cause 3: replay ebs-get-snapshot-block-token-rejections setup (StartSnapshot, CompleteSnapshot) then step task#3 — GET /snapshots/{snap}/blocks/0?blockToken=QUJD. Expect 400 ValidationException/INVALID_BLOCK_TOKEN; observe 404 ResourceNotFoundException/SNAPSHOT_NOT_FOUND.
5. Cause 4: same fixture step task#5 — GET /snapshots/snap-00000000000000000/blocks. Expect 400 ValidationException/INVALID_SNAPSHOT_ID; observe 404 SNAPSHOT_NOT_FOUND.
6. Cause 5: replay ebs-snapshot-lifecycle-and-block-listing step task#2 — StartSnapshot, then ListSnapshotBlocks BEFORE CompleteSnapshot. Expect 404 SNAPSHOT_NOT_FOUND; observe 200 with an empty Blocks list. Then teardown#7 lists the snapshot again after cleanup and still gets 200.
7. Cause 6: replay ebs-start-snapshot-invalid-inputs step task#4 — StartSnapshot(VolumeSize=1, Encrypted=true, KmsKeyArn='arn:aws:kms:us-west-2:000000000000:key/00000000-0000-0000-0000-000000000000'). Expect 400 ValidationException/INVALID_DEPENDENCY_REQUEST; observe 201.
