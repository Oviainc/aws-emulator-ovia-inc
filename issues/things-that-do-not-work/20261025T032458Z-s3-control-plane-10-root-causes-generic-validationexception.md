Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.deep-s3-immutability-copy-unknown-tagging-directive-rejected s3.get-object-attributes-invalid-attribute-name-rejected s3.object-tagging-exceeds-limit-rejected s3.op-contract-put-bucket-tagging-invalid-tag-rejected s3.s3-objver-disabling-versioning-rejected-malformed-xml s3.s3-oc-create-bucket-unknown-object-ownership-rejected-invalid-argument s3.s3-old-buckets-list-buckets-max-buckets-above-ten-thousand-rejected s3.update-object-encryption-error-preconditions s3.s3-inventory-put-rejected-for-an-unsupported-schedule-frequency s3.get-object-nonexistent-version-errors s3.s3unicode-delete-objects-xml-rejects-a-control-char-a-put-key-accepts s3.s3unicode-object-tag-value-with-an-astral-emoji-rejected s3.deploy-s3-bucket-public-access-block-enforced-rejected s3.s3-pab-cli-text-query-prints-the-four-flags-in-member-order s3.s3-pab-three-member-put-reads-back-the-omitted-member-false s3.e2e-b2b-commerce-summary-and-reports-lifecycle s3.e2e-create-s3-bucket-with-config-lifecycle s3.e2e-sync-buckets-replace-metadata-lifecycle s3.block-public-policy-public-grant-rejected-oac-scoped-grant-accepted s3.bucket-acl-public-grant-blocked-by-public-access-block-rejected s3.s3-pab-block-public-policy-rejects-a-re-put-of-the-policy-it-let-stand s3.s3-oc-deleted-controls-re-enable-bucket-acls s3.s3-oc-enforced-put-bucket-acl-rejected-even-for-the-private-canned-acl s3.s3-oc-switch-to-enforced-rejected-while-the-bucket-acl-grants-log-delivery s3.s3-oc-ownership-controls-on-a-missing-bucket-rejected-no-such-bucket s3.s3-old-buckets-create-without-location-constraint-on-a-regional-endpoint-rejected s3.s3-txtsim-create-bucket-without-regional-constraint-rejected s3.s3-objver-versioned-hardened-bucket-config-round-trips s3.s3-csvcols-object-put-without-content-type-defaults-to-binary-octet-stream
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.deep-s3-immutability-copy-unknown-tagging-directive-rejected|run#6|AssertionError|assert (code, status, message) == ("InvalidArgument", 400, "Unknown tagging directive."), (
Issue-Key: s3.get-object-attributes-invalid-attribute-name-rejected|run#8|AssertionError|assert err.get("Code") == "InvalidArgument", f"expected InvalidArgument, got {err!r}"
Issue-Key: s3.object-tagging-exceeds-limit-rejected|run#11|AssertionError|assert code == "BadRequest", f"expected BadRequest for >10 tags, got {code!r}"
Issue-Key: s3.op-contract-put-bucket-tagging-invalid-tag-rejected|run#11|AssertionError|assert code == _DUPLICATE_TAG_KEY, f"expected InvalidTag for duplicate tag key, got {code}"
Issue-Key: s3.s3-objver-disabling-versioning-rejected-malformed-xml|run#4|AssertionError|assert (code, status) == ("MalformedXML", 400), (
Issue-Key: s3.s3-oc-create-bucket-unknown-object-ownership-rejected-invalid-argument|run#5|AssertionError|assert code == "InvalidArgument", (
Issue-Key: s3.s3-old-buckets-list-buckets-max-buckets-above-ten-thousand-rejected|run#5|AssertionError|assert (code, status) == ("InvalidArgument", 400), (
Issue-Key: s3.update-object-encryption-error-preconditions|run#16|AssertionError|assert code == "NoSuchKey", f"expected NoSuchKey, got {code!r}"
Issue-Key: s3.s3-inventory-put-rejected-for-an-unsupported-schedule-frequency|run#6|AssertionError|assert code == "MalformedXML", (
Issue-Key: s3.get-object-nonexistent-version-errors|run#8|AssertionError|assert err.get("Code") == "InvalidArgument", f"expected InvalidArgument for a malformed VersionId, got {err!r}"
Issue-Key: s3.s3unicode-delete-objects-xml-rejects-a-control-char-a-put-key-accepts|run#23|AssertionError|assert code == "MalformedXML", (
Issue-Key: s3.s3unicode-object-tag-value-with-an-astral-emoji-rejected|run#18|AssertionError|assert exc is not None, (
Issue-Key: s3|GetPublicAccessBlock|NoSuchPublicAccessBlockConfiguration
Issue-Key: s3.s3-pab-cli-text-query-prints-the-four-flags-in-member-order|_cli_flags#7|AssertionError|assert proc.returncode == 0, (
Issue-Key: s3.block-public-policy-public-grant-rejected-oac-scoped-grant-accepted|_rejection#6|AssertionError|raise AssertionError(f"the call succeeded where real AWS rejects it; it returned {result!r}")
Issue-Key: s3.bucket-acl-public-grant-blocked-by-public-access-block-rejected|_client_error#14|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: s3.s3-pab-block-public-policy-rejects-a-re-put-of-the-policy-it-let-stand|_poll#9|Timeout|time.sleep(interval_s)
Issue-Key: s3.s3-oc-deleted-controls-re-enable-bucket-acls|run#4|AssertionError|assert code == "AccessControlListNotSupported", (
Issue-Key: s3.s3-oc-enforced-put-bucket-acl-rejected-even-for-the-private-canned-acl|_rejection#3|AssertionError|raise AssertionError(f"the call succeeded where real AWS rejects it; it returned {result!r}")
Issue-Key: s3|CreateBucket|ValidationException
Issue-Key: s3.s3-oc-ownership-controls-on-a-missing-bucket-rejected-no-such-bucket|run#6|AssertionError|assert codes == {"get": "NoSuchBucket", "put": "NoSuchBucket", "delete": "NoSuchBucket"}, (
Issue-Key: s3.s3-old-buckets-create-without-location-constraint-on-a-regional-endpoint-rejected|_error#5|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: s3.s3-txtsim-create-bucket-without-regional-constraint-rejected|_error_response#5|_UnexpectedSuccess|raise _UnexpectedSuccess(
Issue-Key: s3.s3-objver-versioned-hardened-bucket-config-round-trips|run#11|AssertionError|assert rules[0].get("BucketKeyEnabled") is False, (
Issue-Key: s3.s3-csvcols-object-put-without-content-type-defaults-to-binary-octet-stream|run#7|AssertionError|assert untyped == "binary/octet-stream", (
Issue-Commit: 271fde8cc1a42675cd9f2105c7c7bd008dd933ab

# S3 control plane: 10 root causes — generic ValidationException replaces S3 error codes, public-access-block is neither stored nor enforced, ownership controls unenforced, CreateBucket location handling

> _Note: this issue was identified against the code base at commit `271fde8cc1a42675cd9f2105c7c7bd008dd933ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutPublicAccessBlock

## Description
Ten distinct net-new root causes on the S3 bucket control plane. Each is numbered with its own failing check ids; all ten must be closed before this issue is submitted.

Cause 1 of 10: input validation is refused with the generic Smithy ValidationException instead of S3's modeled error codes.
AWS: S3 is a restXml service whose refusals carry S3 error codes in an <Error><Code> body — InvalidArgument, MalformedXML, InvalidTag, BadRequest, NoSuchKey. The Smithy model constrains the members (e.g. s3#GetObjectAttributes objectAttributes is an enum of ETag/Checksum/ObjectParts/StorageClass/ObjectSize), but the wire code AWS answers with is the S3 code, never 'ValidationException'.
Emulator: every constraint violation surfaces as `ValidationException` with a Smithy-shaped message, e.g. `expected InvalidArgument, got {'Code': 'ValidationException', 'Message': "1 validation error detected: Value 'NotARealAttribute' at 'objectAttributes.2.member' failed to satisfy constraint: Member must satisfy enum value set: [...]"}`.
Why: clients branch on the S3 error code; 'ValidationException' matches no S3 handler, so a recoverable 400 becomes an unhandled failure.
Fix: the shared request-validation layer for the S3 service — it must map constraint failures onto S3's modeled error codes per operation.
Traces: s3.deep-s3-immutability-copy-unknown-tagging-directive-rejected, s3.get-object-attributes-invalid-attribute-name-rejected, s3.object-tagging-exceeds-limit-rejected, s3.op-contract-put-bucket-tagging-invalid-tag-rejected, s3.s3-objver-disabling-versioning-rejected-malformed-xml, s3.s3-oc-create-bucket-unknown-object-ownership-rejected-invalid-argument, s3.s3-old-buckets-list-buckets-max-buckets-above-ten-thousand-rejected, s3.update-object-encryption-error-preconditions, s3.s3-inventory-put-rejected-for-an-unsupported-schedule-frequency

Cause 2 of 10: a malformed VersionId is answered NoSuchVersion instead of InvalidArgument.
AWS: a syntactically invalid versionId is a request error (InvalidArgument/400); NoSuchVersion/404 is reserved for a well-formed id that does not exist.
Emulator: `expected InvalidArgument for a malformed VersionId, got {'Code': 'NoSuchVersion', 'Message': 'The specified version does not exist.'}`.
Why: a client cannot tell a client-side bug from a genuinely absent version, and retries a request that can never succeed.
Fix: VersionId parsing/validation on the GetObject handler.
Traces: s3.get-object-nonexistent-version-errors

Cause 3 of 10: a DeleteObjects body containing a control character faults with 500.
AWS: an XML body carrying a character the XML charset forbids is refused MalformedXML/400.
Emulator: `expected MalformedXML for a DeleteObjects body containing U+000B, got '500'`.
Why: a 5xx is retried by every SDK's retry policy, so a permanently-bad request becomes a retry storm instead of a 400.
Fix: the DeleteObjects XML deserializer.
Traces: s3.s3unicode-delete-objects-xml-rejects-a-control-char-a-put-key-accepts

Cause 4 of 10: the tag-value character class is not enforced.
AWS: S3 tag values are restricted to a documented character set; an astral-plane code point is refused.
Emulator: `PutObjectTagging accepted the astral-plane tag value '😀'; real S3 refuses it, so the tag-value character class is not being enforced`.
Why: a tag that AWS would never store round-trips here, so cost-allocation/tag-based policy tests pass locally and fail in AWS.
Fix: PutObjectTagging/PutBucketTagging validation.
Traces: s3.s3unicode-object-tag-value-with-an-astral-emoji-rejected

Cause 5 of 10: GetPublicAccessBlock answers NoSuchPublicAccessBlockConfiguration after a successful PutPublicAccessBlock.
AWS: Smithy s3#GetPublicAccessBlock is GET /{Bucket}?publicAccessBlock, declares no modeled errors, and returns the stored PublicAccessBlockConfiguration with every omitted member materialised as false. NoSuchPublicAccessBlockConfiguration is only correct when no configuration was ever put.
Emulator: after a successful PutPublicAccessBlock, `An error occurred (NoSuchPublicAccessBlockConfiguration) when calling the GetPublicAccessBlock operation: The public access block configuration was not found`.
Why: the write is accepted and then lost, so every bucket-hardening workflow reads back 'not configured' and deploy pipelines abort on their first verification step (three e2e lifecycle checks die in phase 1).
Fix: PublicAccessBlock bucket state and the GetPublicAccessBlock handler.
Traces: s3.deploy-s3-bucket-public-access-block-enforced-rejected, s3.s3-pab-cli-text-query-prints-the-four-flags-in-member-order, s3.s3-pab-three-member-put-reads-back-the-omitted-member-false, s3.e2e-b2b-commerce-summary-and-reports-lifecycle, s3.e2e-create-s3-bucket-with-config-lifecycle, s3.e2e-sync-buckets-replace-metadata-lifecycle

Cause 6 of 10: Block Public Access is not enforced on PutBucketPolicy or PutBucketAcl.
AWS: with BlockPublicPolicy on, a policy statement granting a principal of "*" is refused AccessDenied/403 and the stored policy is left untouched; with BlockPublicAcls on, a public canned/explicit grant on PutBucketAcl is refused AccessDenied. Smithy s3#PutBucketPolicy declares no modeled errors, so this refusal is a service-common AccessDenied.
Emulator: the public PutBucketPolicy is accepted — `the call succeeded where real AWS rejects it; it returned {'ResponseMetadata': {'HTTPStatusCode': 204 ...}}` — and the public bucket ACL is accepted with 200.
Why: the single guardrail customers rely on to prove a bucket cannot be made public does not exist, so a test suite asserting 'public grants are impossible' passes against an emulator that allows them.
Fix: public-policy/public-ACL classification in the PutBucketPolicy and PutBucketAcl handlers, gated on the stored PublicAccessBlockConfiguration.
Traces: s3.block-public-policy-public-grant-rejected-oac-scoped-grant-accepted, s3.bucket-acl-public-grant-blocked-by-public-access-block-rejected, s3.s3-pab-block-public-policy-rejects-a-re-put-of-the-policy-it-let-stand

Cause 7 of 10: object ownership (BucketOwnerEnforced) is not enforced on ACL writes, and CreateBucket refuses the log-delivery-write canned ACL.
AWS: under BucketOwnerEnforced, any ACL write — including the private canned ACL — is refused AccessControlListNotSupported/400; CreateBucket accepts x-amz-acl: log-delivery-write; every ownership-controls operation on a missing bucket answers NoSuchBucket/404.
Emulator: the ACL write succeeds with 200 (`the call succeeded where real AWS rejects it`); the required refusal arrives as 'ValidationException' rather than AccessControlListNotSupported; CreateBucket rejects the documented canned ACL with `ValidationException ... Value 'log-delivery-write' at 'aCL' failed to satisfy constraint: Member must satisfy enum value set: [private, public-read, public-read-write, authenticated-read]`; and DeleteBucketOwnershipControls on a missing bucket answers NotImplemented where get/put correctly answer NoSuchBucket.
Why: the ACL-disabled posture is unobservable and a legitimate canned ACL (the one access-logging requires) cannot be set at all.
Fix: the ObjectOwnership bucket state, the PutBucketAcl/CreateBucket ACL validation, and the ownership-controls handlers' not-found path.
Traces: s3.s3-oc-deleted-controls-re-enable-bucket-acls, s3.s3-oc-enforced-put-bucket-acl-rejected-even-for-the-private-canned-acl, s3.s3-oc-switch-to-enforced-rejected-while-the-bucket-acl-grants-log-delivery, s3.s3-oc-ownership-controls-on-a-missing-bucket-rejected-no-such-bucket

Cause 8 of 10: CreateBucket on a regional endpoint without a LocationConstraint is accepted.
AWS: a CreateBucket sent to a non-us-east-1 regional endpoint with no CreateBucketConfiguration.LocationConstraint is refused (IllegalLocationConstraintException/InvalidLocationConstraint, 400).
Emulator: the call succeeds — `the call succeeded; real AWS rejects it, so there is no error code` and `_UnexpectedSuccess: the call under test succeeded; real AWS rejects it, so accepting it is the finding`.
Why: bucket-creation code that is broken in every region but us-east-1 passes here, so the defect ships.
Fix: the CreateBucket handler's location-constraint validation against the request's region.
Traces: s3.s3-old-buckets-create-without-location-constraint-on-a-regional-endpoint-rejected, s3.s3-txtsim-create-bucket-without-regional-constraint-rejected

Cause 9 of 10: GetBucketEncryption omits BucketKeyEnabled instead of reporting it false.
AWS: after a PutBucketEncryption that does not set BucketKeyEnabled, AWS reports the rule with BucketKeyEnabled: false; the member is materialised, not dropped.
Emulator: `BucketKeyEnabled was not set and must read back false, not absent; got {'ApplyServerSideEncryptionByDefault': {'SSEAlgorithm': 'AES256'}}`.
Why: a config-drift or compliance reader that asserts on the flag sees an absent key and cannot distinguish 'off' from 'unknown'.
Fix: the GetBucketEncryption serializer / default materialisation.
Traces: s3.s3-objver-versioned-hardened-bucket-config-round-trips

Cause 10 of 10: PutObject with no Content-Type guesses a type from the key suffix.
AWS: an object put with no Content-Type is stored as binary/octet-stream; S3 never infers from the key.
Emulator: `an object put with no Content-Type must default to binary/octet-stream, not a type guessed from its .csv suffix; HEAD reports 'application/xml'`.
Why: HEAD/GET report a content type the caller never sent, so browsers and content-negotiating clients behave differently than against AWS.
Fix: the PutObject handler's Content-Type defaulting.
Traces: s3.s3-csvcols-object-put-without-content-type-defaults-to-binary-octet-stream

## Evidence
Cause 1: `AssertionError: expected InvalidArgument, got {'Code': 'ValidationException', ...}` (GetObjectAttributes); `expected BadRequest for >10 tags, got 'ValidationException'`; `expected InvalidTag for duplicate tag key, got ('ValidationException', 'Tagging.TagSet names the key "dup" twice; ...')`; `Status=Disabled must be refused MalformedXML/400; got ('ValidationException', 400)`; `an unknown x-amz-object-ownership header on CreateBucket must be InvalidArgument; got 'ValidationException'`; `MaxBuckets=10001 must be rejected InvalidArgument/400; got 'ValidationException'/400`; `expected NoSuchKey, got 'ValidationException'`; `an unsupported Schedule.Frequency must be rejected MalformedXML; got 'ValidationException'`; `expected InvalidArgument/400 with AWS's exact message for an unknown TaggingDirective, got 'ValidationException'/400`.
Cause 2: `expected InvalidArgument for a malformed VersionId, got {'Code': 'NoSuchVersion', 'Message': 'The specified version does not exist.'}`.
Cause 3: `expected MalformedXML for a DeleteObjects body containing U+000B, got '500'`.
Cause 4: `PutObjectTagging accepted the astral-plane tag value '😀'`.
Cause 5: `botocore.exceptions.ClientError: An error occurred (NoSuchPublicAccessBlockConfiguration) when calling the GetPublicAccessBlock operation: The public access block configuration was not found` — after PutPublicAccessBlock succeeded; three e2e checks die as `_PhaseError: [phase 1/N deploy] ClientError: ... NoSuchPublicAccessBlockConfiguration`. Expected 200 with the four flags.
Cause 6: `AssertionError: the call succeeded where real AWS rejects it; it returned {'ResponseMetadata': {'HTTPStatusCode': 204, ...}}` (PutBucketPolicy with Principal "*" under BlockPublicPolicy; expected AccessDenied/403); `expected the call to be rejected, but it SUCCEEDED and returned {... 'HTTPStatusCode': 200 ...}` (PutBucketAcl public grant under BlockPublicAcls).
Cause 7: `the call succeeded where real AWS rejects it; it returned {... 200 ...}` (PutBucketAcl private under BucketOwnerEnforced; expected AccessControlListNotSupported/400); `under the default BucketOwnerEnforced a log-delivery-write bucket ACL must be refused with AccessControlListNotSupported; got 'ValidationException'`; `An error occurred (ValidationException) when calling the CreateBucket operation: 1 validation error detected: Value 'log-delivery-write' at 'aCL' ...`; `every ownership-controls op on a missing bucket must be NoSuchBucket; got {'get': 'NoSuchBucket', 'put': 'NoSuchBucket', 'delete': 'NotImplemented'}`.
Cause 8: `AssertionError: the call succeeded; real AWS rejects it, so there is no error code`; `_UnexpectedSuccess: the call under test succeeded; real AWS rejects it`.
Cause 9: `BucketKeyEnabled was not set and must read back false, not absent; got {'ApplyServerSideEncryptionByDefault': {'SSEAlgorithm': 'AES256'}}`.
Cause 10: `an object put with no Content-Type must default to binary/octet-stream ...; HEAD reports 'application/xml'`.

## Steps to Reproduce
Boot the emulator and replay the behavioral suite, re-running the check ids listed under each cause's Traces line (all ids are in trace_ids). Manual repros: (1) GetObjectAttributes with ObjectAttributes=['NotARealAttribute'] and read Error.Code; (2) GetObject with VersionId='not-a-version'; (3) DeleteObjects with a key containing U+000B; (4) PutObjectTagging with value '😀'; (5) PutPublicAccessBlock then GetPublicAccessBlock on the same bucket; (6) with BlockPublicPolicy=true, PutBucketPolicy granting s3:GetObject to Principal "*"; (7) with ObjectOwnership=BucketOwnerEnforced, PutBucketAcl ACL=private, and CreateBucket with ACL=log-delivery-write; (8) CreateBucket in us-west-2 with no CreateBucketConfiguration; (9) PutBucketEncryption AES256 then GetBucketEncryption; (10) PutObject key 'a.csv' with no ContentType then HeadObject.
