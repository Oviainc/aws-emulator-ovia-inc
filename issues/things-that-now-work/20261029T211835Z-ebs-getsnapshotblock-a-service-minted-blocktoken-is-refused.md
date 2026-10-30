Issue-Source: trace-sweep-divergence
Issue-Trace: ebs-start-snapshot-parent-readiness-and-block-read-roundtrip
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ebs-start-snapshot-parent-readiness-and-block-read-roundtrip
Issue-Shape-Id: com.amazonaws.ebs#Ebs
Issue-Key: ebs|GetSnapshotBlock|2xx->4xx|$
Issue-Key: ebs|GetSnapshotBlock|2xx->4xx|$headers.content-type
Issue-Key: ebs|GetSnapshotBlock|2xx->4xx|$headers.x-amz-checksum
Issue-Key: ebs|GetSnapshotBlock|2xx->4xx|$headers.x-amz-checksum-algorithm
Issue-Key: ebs|GetSnapshotBlock|2xx->4xx|$headers.x-amz-data-length
Issue-Key: ebs|GetSnapshotBlock|2xx->4xx|status
Issue-Commit: 14bf84ae7f57c35d094a91bae03d265103651cef

# ebs GetSnapshotBlock: a service-minted BlockToken is refused 403 SignatureDoesNotMatch instead of returning the block

> _Note: this issue was identified against the code base at commit `14bf84ae7f57c35d094a91bae03d265103651cef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSnapshotBlock

## Description
Replaying the simparity-wire-aws-v1 EBS direct-API fixtures against the emulator built from this checkout surfaces one net-new root cause on service `ebs` (every other EBS/EC2 divergence in this replay is already owned by an open issue or by a filed operation stub). The emulator was up (health 200, data listener accepting) and bootstrap_signal was false, so this is a fidelity defect and not a non-serving emulator.

=== Cause 1 of 1: GetSnapshotBlock with a valid, service-minted BlockToken is refused 403 SignatureDoesNotMatch before the operation runs, so block data can never be read back ===

AWS: GetSnapshotBlock is the only way to read the bytes of a block out of an EBS snapshot. Smithy com.amazonaws.ebs#GetSnapshotBlock binds GET /snapshots/{SnapshotId}/blocks/{BlockIndex} with success code 200; BlockToken is @required and bound as httpQuery `blockToken` (length 0..256, pattern ^[A-Za-z0-9+/=]+$), i.e. the modeled token alphabet explicitly includes the standard-base64 characters `+`, `/` and `=`. A token obtained from ListSnapshotBlocks/ListChangedBlocks for an existing, completed snapshot is answered 200 whose body is the block bytes (GetSnapshotBlockResponse.BlockData, @httpPayload @streaming blob) with the three response headers derived from those bytes: x-amz-Data-Length (DataLength, the byte count of the block, 524288 for a default 512 KiB block), x-amz-Checksum (Checksum, the base64 SHA-256 of the returned bytes) and x-amz-Checksum-Algorithm (ChecksumAlgorithm, whose only modeled value is SHA256), served as application/octet-stream. GetSnapshotBlock's modeled errors are AccessDeniedException (403), InternalServerException (500), RequestThrottledException (400), ResourceNotFoundException (404), ServiceQuotaExceededException (402) and ValidationException (400) — SignatureDoesNotMatch is not among them; it is a transport/signing refusal, not an operation outcome. The recording shows the 200-with-block-bytes response for exactly this request.

Emulator: HTTP 403 with x-amzn-errortype `SignatureDoesNotMatch` and message `The request signature does not match`, carrying none of BlockData, x-amz-Data-Length, x-amz-Checksum, x-amz-Checksum-Algorithm or the application/octet-stream content type. The request is refused before dispatch: the GetSnapshotBlock handler is fully implemented (services/service-com-amazonaws-ebs-ebs/src/ops/get_snapshot_block.rs resolves the token against the stored block and returns the persisted bytes with the length and checksum derived from them) and its error channel can only produce the operation's modeled 400/402/403-AccessDenied/404/500 shapes, so it cannot emit SignatureDoesNotMatch at all. The refusal is raised by the shared SigV4 verifier, which re-signs the arriving request and compares signatures. The same fixture's next step is refused the same way (403/SignatureDoesNotMatch where AWS answers 400 ValidationException/INVALID_BLOCK_TOKEN), while a GetSnapshotBlock carrying the purely alphanumeric token `QUJD` in another fixture DOES reach the handler and is answered by it — so the operation is routable and signature verification succeeds for a token with no reserved characters and fails for a minted, standard-base64 one.

Hypothesis: the verifier rebuilds the signable URI from the raw wire query string and hands it to the signer, so a `blockToken` value containing `+`, `/` or `=` is canonicalised differently than the client canonicalised it (a percent-encoded `%2B` re-encoded, or a literal `+` decoded as a space), yielding a different canonical query string and therefore a different signature. Neither the model nor the recording states this; what the recording states is the 200 and what the replay states is the 403.

Why: a client that writes a snapshot with PutSnapshotBlock, completes it, lists its blocks and then reads a block back — the entire EBS direct-API read path, and the whole point of the API — cannot read any block whose token happens to contain a reserved base64 character, which is most tokens. The failure is also mis-classified: 403/SignatureDoesNotMatch tells the SDK its credentials or clock are wrong, so a caller refreshes credentials and retries forever instead of being told anything about the snapshot or the token, and cannot distinguish an un-minted token (400 ValidationException/INVALID_BLOCK_TOKEN) or a missing snapshot (404 ResourceNotFoundException/SNAPSHOT_NOT_FOUND) from a signing problem. Because the refusal happens before dispatch it also masks every GetSnapshotBlock behaviour behind it, so no block-read fidelity can be exercised at all.

Fix: the shared SigV4 request verification in the emulator core — the signable-request reconstruction that feeds the re-signing step must canonicalise the query string exactly as SigV4 specifies for a non-S3 signing name, so a request whose query value contains reserved characters (`+`, `/`, `=`) verifies. It must not consume GetSnapshotBlock's modeled token alphabet as a signing failure; once the request verifies, the already-implemented GetSnapshotBlock handler supplies the 200/400/404 outcomes.

Traces: ebs-start-snapshot-parent-readiness-and-block-read-roundtrip#task#7 (the 200 block read). Corroborating, and NOT filed here because the replay reports it as owned by open issue 20261023T060504Z-ebs-direct-apis-refusals-answer-the-wrong-http.md: ebs-start-snapshot-parent-readiness-and-block-read-roundtrip#task#8, the same signing refusal standing in front of the invalid-block-token refusal. Code: services/service-com-amazonaws-ebs-ebs/src/ops/get_snapshot_block.rs lines 177-240 (the implemented handler, which cannot return SignatureDoesNotMatch); vendor/server/src/request_authentication.rs lines 86-89 (the SIGNATURE_MISMATCH refusal: code `SignatureDoesNotMatch`, message `The request signature does not match`), 189-224 (SigV4Authenticator::verify) and 329-358 (signable_uri_with_signed_host, which reassembles the query from the raw wire items).

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout: 9 fixtures replayed, 61 steps, 35 passed, 26 mismatched, 0 errored, 0 blocked, score 57.4%, emulator_up=true (health 200, data listener accepting), bootstrap_signal=false, 11 divergence clusters. Ten clusters are already owned (the ec2 DeleteSnapshot operation stub, 20261023T060504Z-ebs-direct-apis-refusals-answer-the-wrong-http.md and 20261029T180806Z-ebs-direct-apis-response-values-diverge-from-aws.md); the cluster below is the only one with no owner and no stub.

Cause 1 — ebs-start-snapshot-parent-readiness-and-block-read-roundtrip#task#7, GetSnapshotBlock (divergence_key service=ebs|GetSnapshotBlock|status=200->403|keys=$,$headers.content-type,$headers.x-amz-checksum,$headers.x-amz-checksum-algorithm,$headers.x-amz-data-length,status; affected_count=1; stub_path empty; owned_by empty).
  Expected: HTTP 200, body = the block's bytes (the recording's 512 KiB block payload), with headers x-amz-Data-Length = the block's byte count, x-amz-Checksum = base64 SHA-256 of those bytes, x-amz-Checksum-Algorithm = SHA256, content-type application/octet-stream.
  Actual: HTTP 200 -> 403. Reported as `status: expected 200, got 403` plus `$: type_mismatch` (the asserted block payload replaced by an error envelope) and a mismatch on each of content-type, x-amz-checksum, x-amz-checksum-algorithm and x-amz-data-length. The 403 envelope is the core's signing refusal: x-amzn-errortype `SignatureDoesNotMatch`, message `The request signature does not match` (fixed text, no variable parts) — the same envelope observed verbatim on the adjacent step task#8, where the replay records `$headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'SignatureDoesNotMatch')`.
  Discriminator: in fixture ebs-get-snapshot-block-token-rejections#task#3, GetSnapshotBlock with the purely alphanumeric blockToken `QUJD` is NOT refused 403 — it reaches the handler and is answered by it (404 ResourceNotFoundException/SNAPSHOT_NOT_FOUND, a separate already-owned divergence). So signature verification succeeds for a token with no reserved characters and fails for a minted standard-base64 token containing `+`/`/`/`=`.
  Non-emulator causes excluded: emulator_up=true, 0 errored and 0 blocked steps, and the refusal body is the emulator's own SIGNATURE_MISMATCH constant — not a runner TLS/DNS failure, not a missing tool, and not a per-run value (no request id, clock time, generated id or host is asserted in this cause).

## Steps to Reproduce
1. Build and boot the emulator from this checkout; point an `ebs` client (restJson1, sigv4, signing name `ebs`) at the data listener and sign with the emulator's published credentials.
2. Replay fixture ebs-start-snapshot-parent-readiness-and-block-read-roundtrip through its setup: StartSnapshot(VolumeSize=1), PutSnapshotBlock at BlockIndex 0 with a 524288-byte payload plus its x-amz-Checksum/x-amz-Checksum-Algorithm, then CompleteSnapshot, then ListSnapshotBlocks to obtain the BlockToken the service minted for block 0. Note that the minted token is standard base64 and contains at least one of `+`, `/`, `=`.
3. Step task#7: GET /snapshots/{snapshotId}/blocks/0?blockToken=<that token>, SigV4-signed.
   Expect HTTP 200 whose body is the 524288 bytes written in step 2, with x-amz-Data-Length = 524288, x-amz-Checksum = the base64 SHA-256 of those bytes, x-amz-Checksum-Algorithm = SHA256, content-type application/octet-stream.
   Observe HTTP 403 with x-amzn-errortype `SignatureDoesNotMatch` and message `The request signature does not match`, and none of those headers or the block payload.
4. Contrast: issue the same GET against any snapshot with blockToken=QUJD (alphanumeric only, no reserved characters). The signing layer lets it through and the GetSnapshotBlock handler answers it, confirming the refusal in step 3 is raised before dispatch and is tied to the reserved characters in the query value rather than to the credentials, the clock or the operation.
