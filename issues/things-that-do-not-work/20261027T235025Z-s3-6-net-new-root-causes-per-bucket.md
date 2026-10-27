Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.range-read-open-end s3.cli-s3-cp-large-file-multipart-checksum-defaults s3.conc-s3-conditional-writes-lock-stale-holder-release-rejected s3.deep-s3-storage-class-copy-in-place-to-deep-archive-closes-reads s3.multipart-complete-part-ordering-enforced s3.metadata-table-configuration-delete-removes-table s3.bucket-metadata-table-configuration-lifecycle
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.range-read-open-end|run#6|FlexibleChecksumError|got = resp["Body"].read()
Issue-Key: s3.cli-s3-cp-large-file-multipart-checksum-defaults|_run#25|AssertionError|assert r.returncode == 0, f"{' '.join(argv)} failed (rc={r.returncode}): {r.stderr[-600:]!r}"
Issue-Key: s3|DeleteObject|NotImplemented
Issue-Key: s3.deep-s3-storage-class-copy-in-place-to-deep-archive-closes-reads|run#20|AssertionError|assert back == _COPY_ARCHIVED, f"copy back to STANDARD returned {back!r}, expected {_COPY_ARCHIVED!r}"
Issue-Key: s3.multipart-complete-part-ordering-enforced|run#18|AssertionError|assert missing_message == "One or more of the specified parts could not be found. The part may not have been uploaded, or the specified entity tag may not match
Issue-Key: s3.metadata-table-configuration-delete-removes-table|run#12|AssertionError|assert code == "MetadataConfigurationNotFound", code
Issue-Key: s3.bucket-metadata-table-configuration-lifecycle|run#18|Timeout|time.sleep(5)
Issue-Commit: 10facd235b75ceff4fb05feb7e3df512bc30c659

# s3: 6 net-new root causes — per-bucket byte cap refuses UploadPart, conditional DeleteObject unimplemented, ranged GET returns a whole-object checksum, wrong codes/messages for archived-source CopyObject, InvalidPart and MetadataConfigurationNotFound

> _Note: this issue was identified against the code base at commit `10facd235b75ceff4fb05feb7e3df512bc30c659`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject (also CopyObject, UploadPart, DeleteObject, CompleteMultipartUpload, GetBucketMetadataConfiguration)

## Description
Six independent, net-new root causes in the s3 emulator, all grounded in failing behavioral checks that the replay report did NOT mark ALREADY FILED or OWNED. Each must be closed before this issue is submitted.

--- Cause 1 of 6: GetObject on a byte range answers with the WHOLE-object checksum header, so SDK response-checksum validation fails ---
AWS: Smithy: GetObject carries @httpChecksum{requestValidationModeMember:"ChecksumMode", responseAlgorithms:[CRC64NVME, CRC32, CRC32C, SHA256, SHA1, SHA512, MD5, XXHASH64, XXHASH3, XXHASH128]} and binds ContentRange -> Content-Range. A satisfiable `Range: bytes=<first>-` GET answers HTTP 206 with Content-Range `bytes <first>-<last>/<total>`, Content-Length equal to the returned span, and the body being exactly those bytes. Because the returned bytes are NOT the whole object, AWS does not advertise the whole-object x-amz-checksum-* value on that partial response: it omits the checksum header unless the requested range is exactly one part of a multipart object (in which case it is that part's digest). Any x-amz-checksum-* header S3 does return is the digest of the bytes actually in that response body.
Emulator: the response carries a checksum header whose value is the digest of the FULL object while the body is only the ranged span, so botocore's flexible-checksum reader rejects the response: `botocore.exceptions.FlexibleChecksumError: Expected checksum <header value> did not match calculated checksum: <digest of the ranged body>` (recorded as `Expected checksum mDw3tQ== did not match calculated checksum: u3b+aQ==`; both base64 digests are per-object values from the request's payload, not fixed text). The assertions on 206 / ContentLength / ContentRange are never reached because the error is raised while reading the body.
Why: every AWS SDK with response_checksum_validation at its default "when_supported" raises on EVERY ranged GetObject against the emulator. Range reads — resumable downloads, the CLI's and transfer managers' chunked GETs, byte-range readers — are unusable, and the failure surfaces as data corruption rather than as a service error.
Fix: the GetObject handler's response serializer — when a Range (or PartNumber) narrows the response to less than the whole object, do not emit the whole-object x-amz-checksum-* header; emit none (or the digest of exactly the bytes being returned, for a single-part-aligned range).
Traces: s3.range-read-open-end.

--- Cause 2 of 6: an emulator-only per-bucket 16 MiB byte cap refuses a legitimate UploadPart with ValidationException ---
AWS: recorded behavior only; the Smithy model puts NO byte quota on a bucket and UploadPart models no such error (UploadPart errors: none modeled; the only size constraints AWS enforces are the 5 MiB minimum for a non-final part and the 5 GiB per-part maximum). A bucket holds unbounded data; multipart uploads of many MiB succeed and CompleteMultipartUpload assembles them.
Emulator: UploadPart is refused with HTTP 400 `ValidationException` and the message `recording part <PartNumber> would leave the bucket "<BucketName>" holding <new total> bytes, past the <cap> bytes this emulator keeps per bucket` — fixed text is "recording part … would leave the bucket … holding … bytes, past the … bytes this emulator keeps per bucket"; the part number, bucket name and the two byte counts come from the request and the emulator's own state (recorded instance: part 2, 16777467 bytes, cap 16777216 bytes). The check uploaded two 12 MiB objects via `aws s3 cp` into one bucket; the first succeeded and the second was refused mid-upload. ValidationException is not an S3 error code at all — S3's REST error codes are `EntityTooLarge`/`EntityTooSmall`/`InvalidRequest`, never `ValidationException`.
Why: a client doing ordinary multi-object or multipart traffic gets a hard refusal with a code no S3 SDK or retry policy recognises, and the CLI reports `upload failed`. Any workload whose total bucket footprint crosses the emulator's internal cap diverges from AWS, and the cap is invisible to the client — nothing in the API announces it.
Fix: the object-plane state/quota layer behind UploadPart and PutObject (the per-bucket byte accounting that raises this ValidationException) — it must not reject writes AWS accepts; if a capacity ceiling must exist it belongs outside the modeled API surface, not as a synthetic error code on UploadPart.
Traces: s3.cli-s3-cp-large-file-multipart-checksum-defaults.

--- Cause 3 of 6: DeleteObject with a conditional If-Match / x-amz-if-match-* header is an unimplemented scaffold stub ---
AWS: Smithy: DeleteObject input binds IfMatch -> `If-Match`, IfMatchLastModifiedTime -> `x-amz-if-match-last-modified-time` (http-date) and IfMatchSize -> `x-amz-if-match-size`; success is HTTP 204. A DeleteObject whose If-Match equals the object's current ETag deletes it and answers 204; one carrying a stale ETag is refused with HTTP 412 `PreconditionFailed` and the message `At least one of the pre-conditions you specified did not hold`, and the object survives unchanged.
Emulator: the call is refused with `NotImplemented` — `operation DeleteObject with a conditional If-Match / x-amz-if-match-* header is scaffolded but not implemented in this emulator`. The unconditional DeleteObject path works; only the conditional variant is a stub, so the check fails on its first POSITIVE control (A releasing its own lock with its own ETag).
Why: S3-as-a-lock, the canonical compare-and-delete pattern (If-None-Match:* to acquire, If-Match to renew/release), cannot be exercised at all; a client gets NotImplemented where AWS gives 204 or 412, so a stale holder's release is neither honoured nor refused and the mutual-exclusion property the client relies on is untestable.
Fix: the DeleteObject handler's conditional-header evaluation (the scaffold stub that rejects requests carrying If-Match / x-amz-if-match-last-modified-time / x-amz-if-match-size). Related but DISTINCT open issue: 20261025T032458Z-s3-object-plane-10-root-causes-conditional-requests.md owns other conditional-request checks (including s3.s3-delete-object-with-a-stale-if-match-etag-is-refused-and-the-object-survives) but does not carry this check's replay key; this stub is the positive-control path.
Traces: s3.conc-s3-conditional-writes-lock-stale-holder-release-rejected.

--- Cause 4 of 6: CopyObject from an archived (DEEP_ARCHIVE) source is refused with the wrong error code and message ---
AWS: recorded behavior only for the code selection — Smithy models ObjectNotInActiveTierError (client, httpError 403) on CopyObject and InvalidObjectState (client, httpError 403) on GetObject, and the recording shows real AWS refusing a copy whose source object is in DEEP_ARCHIVE with code `InvalidObjectState` and the message `Operation is not valid for the source object's storage class` (fixed text; no variable parts). The paired GetObject on the same archived object is refused with `InvalidObjectState` and `The operation is not valid for the object's storage class`. Both are HTTP 403.
Emulator: the copy-back is refused with code `ObjectNotInActiveTierError` and the message `The source object of the COPY action is not in the active tier and is only stored in Amazon S3 Glacier.` — the legacy Glacier-tier refusal, not the storage-class refusal AWS now returns for an archived source. The emulator reaches this point correctly (the in-place re-class to DEEP_ARCHIVE keeps size and ETag and the archived GetObject is refused), so only the code/message pair diverges.
Why: a client branching on the error code — retry-after-restore logic, lifecycle tooling, error-class assertions — takes the wrong branch, because `ObjectNotInActiveTierError` and `InvalidObjectState` are different modeled errors with different handling.
Fix: the CopyObject handler's source-storage-class validation (the branch that selects the refusal for an archived source) and the error serializer's code/message pair for it.
Traces: s3.deep-s3-storage-class-copy-in-place-to-deep-archive-closes-reads.

--- Cause 5 of 6: CompleteMultipartUpload's InvalidPart message text does not match AWS's wording ---
AWS: recorded behavior only (the message is not in the model; CompleteMultipartUpload models no errors). A part list naming a PartNumber that was never uploaded, or an ETag that does not match the uploaded part, is refused with code `InvalidPart` and this exact fixed text, which contains TWO spaces after the first sentence's period: `One or more of the specified parts could not be found.  The part may not have been uploaded, or the specified entity tag may not match the part's entity tag.` It has no variable parts. (The sibling refusal in the same check, `InvalidPartOrder` / `The list of parts was not in ascending order. Parts must be ordered by part number.`, the emulator gets right.)
Emulator: the same refusal carries the right code `InvalidPart` but collapses the double space to a single one: `One or more of the specified parts could not be found. The part may not have been uploaded, or the specified entity tag may not match the part's entity tag.`
Why: a client or test asserting on S3's documented error text — and any consumer that fingerprints the message — sees a message that is not AWS's. The divergence is entirely in fixed text, so it is a requirement, not a per-run value.
Fix: the CompleteMultipartUpload handler's part-validation error construction / the error message catalogue entry for InvalidPart. Related but DISTINCT open issue: 20261025T160428Z-s3-multipart-analytics-3-net-new-root-causes.md covers other multipart causes and does not carry this check's replay key.
Traces: s3.multipart-complete-part-ordering-enforced.

--- Cause 6 of 6: after DeleteBucketMetadataConfiguration the metadata-configuration reads answer NoSuchConfiguration instead of MetadataConfigurationNotFound, and a client polling for the real code never converges ---
AWS: recorded behavior only (Smithy: GetBucketMetadataConfiguration — GET /{Bucket}?metadataConfiguration — declares no operation-specific errors). Once DeleteBucketMetadataConfiguration has removed a bucket's metadata configuration, GetBucketMetadataConfiguration is refused with code `MetadataConfigurationNotFound` and the message `The metadata configuration was not found` (fixed text, no variable parts), and a mutation on the dead configuration (UpdateBucketMetadataJournalTableConfiguration) is refused the same way rather than silently reviving or no-op'ing it. The bucket stays reusable: a fresh CreateBucketMetadataConfiguration can then be created and read back with its JournalTableConfigurationResult.
Emulator: the post-delete GetBucketMetadataConfiguration is refused with code `NoSuchConfiguration` — a code AWS does not use for this operation — so the delete-then-read contract reports the wrong error class. The second grounded check polls for `MetadataConfigurationNotFound` after the delete; because that code never arrives, the poll spins until the harness kills the check (`Check timed out after 60.0s`). Creation and the pre-delete read work, so only the not-found classification diverges.
Why: a client that distinguishes "this bucket has no metadata configuration" from other failures — teardown/idempotency logic, CFN-style delete-then-verify, retry-until-gone loops — never sees the terminal code and hangs or treats a successful delete as an unknown error.
Fix: the GetBucketMetadataConfiguration (and UpdateBucketMetadataJournalTableConfiguration) handler's not-found branch and the error serializer's code/message for it. Note: the separate, already-filed scaffold-stub issue for UpdateBucketMetadataJournalTableConfiguration (reported against services/service-com-amazonaws-s3-amazons3/src/ops/update_bucket_metadata_journal_table_configuration.rs via s3.bucket-metadata-journal-table-update) covers that operation being unimplemented; it does not cover this not-found code.
Traces: s3.metadata-table-configuration-delete-removes-table, s3.bucket-metadata-table-configuration-lifecycle.

## Evidence
Cause 1 — s3.range-read-open-end [error]
  Request: PutObject 16 bytes b"0123456789ABCDEF", then GetObject with Range="bytes=10-".
  Expected (AWS): HTTP 206, body b"ABCDEF", ContentLength 6, ContentRange "bytes 10-15/16", and NO whole-object x-amz-checksum-* header on the partial response.
  Actual (emulator): botocore.exceptions.FlexibleChecksumError: Expected checksum mDw3tQ== did not match calculated checksum: u3b+aQ==  (raised in httpchecksum.py _validate_checksum while reading the body; the header carried the full-object digest, the body was the 6 ranged bytes).

Cause 2 — s3.cli-s3-cp-large-file-multipart-checksum-defaults [mismatch]
  Request: two `aws s3 cp` uploads of a 12 MiB file into one bucket (CLI default 8 MiB part size => 2 parts each); leg 1 with --checksum-algorithm SHA256 succeeded.
  Expected (AWS): the second cp also succeeds, HeadObject reports a checksum correct over the real part split, and both objects download byte-exact.
  Actual (emulator): `upload failed: ... An error occurred (ValidationException) when calling the UploadPart operation: recording part 2 would leave the bucket "sim2real-behav-clichk-e11e918d3a2f" holding 16777467 bytes, past the 16777216 bytes this emulator keeps per bucket` (aws CLI rc=1).

Cause 3 — s3.conc-s3-conditional-writes-lock-stale-holder-release-rejected [error]
  Request: PutObject with IfNoneMatch="*" (acquire, succeeded), then DeleteObject(Bucket, Key, IfMatch=<current ETag>) as the positive-control release.
  Expected (AWS): HTTP 204 for the matching ETag; HTTP 412 PreconditionFailed / "At least one of the pre-conditions you specified did not hold" for a stale ETag, object untouched.
  Actual (emulator): ClientError (NotImplemented) when calling the DeleteObject operation: operation DeleteObject with a conditional If-Match / x-amz-if-match-* header is scaffolded but not implemented in this emulator.

Cause 4 — s3.deep-s3-storage-class-copy-in-place-to-deep-archive-closes-reads [mismatch]
  Request: in-place CopyObject to StorageClass=DEEP_ARCHIVE (accepted; HeadObject/ListObjectsV2 correctly report DEEP_ARCHIVE, size and ETag preserved; the archived GetObject is correctly refused InvalidObjectState), then CopyObject back to StorageClass=STANDARD from the same archived key.
  Expected (AWS): ('InvalidObjectState', "Operation is not valid for the source object's storage class"), HTTP 403.
  Actual (emulator): ('ObjectNotInActiveTierError', 'The source object of the COPY action is not in the active tier and is only stored in Amazon S3 Glacier.').

Cause 5 — s3.multipart-complete-part-ordering-enforced [mismatch]
  Request: CreateMultipartUpload, UploadPart 1 (5 MiB) and 2 (1 MiB), then CompleteMultipartUpload with Parts=[{1, etag1}, {3, "\"000...0\""}] (PartNumber 3 never uploaded).
  Expected (AWS): code InvalidPart, message `One or more of the specified parts could not be found.  The part may not have been uploaded, or the specified entity tag may not match the part's entity tag.` (two spaces after "found.").
  Actual (emulator): code InvalidPart, message `One or more of the specified parts could not be found. The part may not have been uploaded, or the specified entity tag may not match the part's entity tag.` (one space). The preceding out-of-order case (InvalidPartOrder + its message) matched.

Cause 6 — s3.metadata-table-configuration-delete-removes-table [mismatch] and s3.bucket-metadata-table-configuration-lifecycle [mismatch]
  Request: CreateBucketMetadataConfiguration with MetadataConfiguration={"JournalTableConfiguration":{"RecordExpiration":{"Expiration":"DISABLED"}}} (accepted; the pre-delete GetBucketMetadataConfiguration returned a JournalTableConfigurationResult with TableStatus CREATING/ACTIVE), then DeleteBucketMetadataConfiguration, then GetBucketMetadataConfiguration.
  Expected (AWS): code MetadataConfigurationNotFound, message "The metadata configuration was not found"; the same code/message for UpdateBucketMetadataJournalTableConfiguration on the deleted configuration; a fresh create afterwards readable again.
  Actual (emulator): AssertionError: NoSuchConfiguration (delete-removes-table, at the `assert code == "MetadataConfigurationNotFound"` line). In the lifecycle check the same wrong code makes the retry-until-MetadataConfigurationNotFound loop never converge: `Error: Check timed out after 60.0s`.

## Steps to Reproduce
All six are replayed by the behavioral suite against a booted emulator; re-run exactly the check ids in trace_ids.

Cause 1: create a bucket; PutObject a 16-byte body; GetObject with Range="bytes=10-" using any SDK with response checksum validation at its default. Expect 206 + body of bytes 10..15 and no whole-object checksum header; observe FlexibleChecksumError.
Cause 2: create one bucket; `aws s3 cp` a 12 MiB file to it twice (two different keys) so the bucket's total exceeds 16 MiB. The second cp's UploadPart is refused with ValidationException naming the per-bucket byte cap.
Cause 3: create a bucket; PutObject key with IfNoneMatch="*" and keep the ETag; call DeleteObject(Bucket, Key, IfMatch=<that ETag>). Expect 204; observe NotImplemented. Repeat with a stale ETag and expect 412 PreconditionFailed.
Cause 4: create a bucket; PutObject a small body; CopyObject in place with StorageClass=DEEP_ARCHIVE; then CopyObject in place with StorageClass=STANDARD. Compare the refusal code and message against InvalidObjectState / "Operation is not valid for the source object's storage class".
Cause 5: create a bucket; CreateMultipartUpload; UploadPart 1 (>=5 MiB) and 2; CompleteMultipartUpload with a part list naming a PartNumber that was never uploaded; compare the InvalidPart message byte-for-byte (note the double space after "found.").
Cause 6: create a bucket; CreateBucketMetadataConfiguration with a JournalTableConfiguration; GetBucketMetadataConfiguration (succeeds); DeleteBucketMetadataConfiguration; GetBucketMetadataConfiguration again and inspect the error code — expect MetadataConfigurationNotFound / "The metadata configuration was not found", observe NoSuchConfiguration. Then poll for MetadataConfigurationNotFound to see the lifecycle check's hang.
