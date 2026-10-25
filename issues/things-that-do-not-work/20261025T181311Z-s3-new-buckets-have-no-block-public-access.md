Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.public-access-block-state-machine s3.s3-pab-three-member-put-reads-back-the-omitted-member-false s3.s3-pab-cli-text-query-prints-the-four-flags-in-member-order s3.deploy-s3-bucket-public-access-block-enforced-rejected s3.e2e-b2b-commerce-summary-and-reports-lifecycle s3.e2e-create-s3-bucket-with-config-lifecycle s3.e2e-sync-buckets-replace-metadata-lifecycle s3.block-public-policy-public-grant-rejected-oac-scoped-grant-accepted s3.bucket-acl-public-grant-blocked-by-public-access-block-rejected s3.s3-pab-block-public-policy-rejects-a-re-put-of-the-policy-it-let-stand s3.deep-s3-immutability-copy-unknown-tagging-directive-rejected s3.get-object-attributes-invalid-attribute-name-rejected s3.s3-oc-create-bucket-unknown-object-ownership-rejected-invalid-argument s3.s3-old-buckets-list-buckets-max-buckets-above-ten-thousand-rejected s3.object-tagging-exceeds-limit-rejected s3.op-contract-put-bucket-tagging-invalid-tag-rejected s3.s3unicode-object-tag-value-with-an-astral-emoji-rejected s3.get-object-nonexistent-version-errors s3.s3-delete-objects-reports-every-requested-key-version-pair-exactly-once s3.s3-objver-versioned-hardened-bucket-config-round-trips s3.update-object-encryption-error-preconditions
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|GetPublicAccessBlock|NoSuchPublicAccessBlockConfiguration
Issue-Key: s3.s3-pab-cli-text-query-prints-the-four-flags-in-member-order|_cli_flags#7|AssertionError|assert proc.returncode == 0, (
Issue-Key: s3.block-public-policy-public-grant-rejected-oac-scoped-grant-accepted|run#8|AssertionError|assert message == (
Issue-Key: s3.bucket-acl-public-grant-blocked-by-public-access-block-rejected|run#18|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: s3.s3-pab-block-public-policy-rejects-a-re-put-of-the-policy-it-let-stand|run#13|AssertionError|assert message == (
Issue-Key: s3.deep-s3-immutability-copy-unknown-tagging-directive-rejected|run#6|AssertionError|assert (code, status, message) == ("InvalidArgument", 400, "Unknown tagging directive."), (
Issue-Key: s3.get-object-attributes-invalid-attribute-name-rejected|run#9|AssertionError|assert err.get("Message") == "Invalid attribute name specified.", err
Issue-Key: s3.s3-oc-create-bucket-unknown-object-ownership-rejected-invalid-argument|run#6|AssertionError|assert message == _OWNERSHIP_MSG, (
Issue-Key: s3.s3-old-buckets-list-buckets-max-buckets-above-ten-thousand-rejected|run#6|AssertionError|assert message == "Argument max-buckets must be an integer between 1 and 10000", (
Issue-Key: s3.object-tagging-exceeds-limit-rejected|run#12|AssertionError|assert message == "Object tags cannot be greater than 10", f"unexpected message {message!r}"
Issue-Key: s3.op-contract-put-bucket-tagging-invalid-tag-rejected|run#11|AssertionError|assert code == _DUPLICATE_TAG_KEY, f"expected InvalidTag for duplicate tag key, got {code}"
Issue-Key: s3.s3unicode-object-tag-value-with-an-astral-emoji-rejected|run#22|AssertionError|assert message == "The TagValue you have provided is invalid", f"unexpected message {message!r}"
Issue-Key: s3.get-object-nonexistent-version-errors|run#8|AssertionError|assert err.get("Code") == "InvalidArgument", f"expected InvalidArgument for a malformed VersionId, got {err!r}"
Issue-Key: s3.s3-delete-objects-reports-every-requested-key-version-pair-exactly-once|run#31|AssertionError|assert bogus_code == "NoSuchVersion", (
Issue-Key: s3.s3-objver-versioned-hardened-bucket-config-round-trips|run#12|AssertionError|assert rules[0].get("BlockedEncryptionTypes") == {"EncryptionType": ["SSE-C"]}, (
Issue-Key: s3.update-object-encryption-error-preconditions|run#16|AssertionError|assert code == "NoSuchKey", f"expected NoSuchKey, got {code!r}"
Issue-Commit: ec4677bae9b70a2d31b2abdbe7af61c1994727a3

# s3: new buckets have no Block Public Access configuration, BPA and model-constraint refusals carry the wrong messages, version ids are misclassified, and 3 more (7 causes)

> _Note: this issue was identified against the code base at commit `ec4677bae9b70a2d31b2abdbe7af61c1994727a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPublicAccessBlock

## Description
Seven distinct net-new root causes in the S3 surface, all grounded in failing behavioral checks whose check id leads with `s3`. None of these checks is marked ALREADY FILED or OWNED in the replay report.

----------------------------------------------------------------
Cause 1 of 7: a newly created bucket carries NO Block Public Access configuration — GetPublicAccessBlock answers NoSuchPublicAccessBlockConfiguration instead of the all-true creation default.
Failing checks: s3.public-access-block-state-machine, s3.s3-pab-three-member-put-reads-back-the-omitted-member-false, s3.s3-pab-cli-text-query-prints-the-four-flags-in-member-order, s3.deploy-s3-bucket-public-access-block-enforced-rejected, s3.e2e-b2b-commerce-summary-and-reports-lifecycle, s3.e2e-create-s3-bucket-with-config-lifecycle, s3.e2e-sync-buckets-replace-metadata-lifecycle

AWS: Smithy models GetPublicAccessBlock -> GetPublicAccessBlockOutput{PublicAccessBlockConfiguration} with an EMPTY modeled error list, and PublicAccessBlockConfiguration carries exactly four boolean members BlockPublicAcls, IgnorePublicAcls, BlockPublicPolicy, RestrictPublicBuckets. Since April 2023 every newly created bucket is created with all four settings enabled, so GetPublicAccessBlock on a fresh bucket answers 200 with all four members true; NoSuchPublicAccessBlockConfiguration is reserved for a bucket whose configuration was explicitly removed with DeletePublicAccessBlock.
Emulator: GetPublicAccessBlock on a bucket that was just created answers `(NoSuchPublicAccessBlockConfiguration) The public access block configuration was not found`, both through the SDK and through `aws s3api get-public-access-block` (which exits 254). Four end-to-end/deploy lifecycle checks abort in their first phase on the same call.
Why: every caller that audits public-access posture reads this subresource first. A 404-class refusal where AWS reports four booleans makes a bucket that IS fully blocked look unconfigured — an auditor classifies a compliant bucket as deviant (or crashes), and a client that "repairs" it by putting a configuration silently changes nothing while believing it hardened the bucket. It also makes the put-REPLACES-the-document semantics untestable: a PutPublicAccessBlock naming only three members must read back the omitted member as an explicit false against the all-true default, which cannot be observed while the default is absent.
Fix: bucket creation in the s3 crate must install the all-true PublicAccessBlockConfiguration as part of CreateBucket's state, and the GetPublicAccessBlock handler must serialise all four members, reserving NoSuchPublicAccessBlockConfiguration for the post-DeletePublicAccessBlock state. The same default must be visible to a CloudFormation-created AWS::S3::Bucket (the deploy-s3-bucket-public-access-block check reads it on a stack-created bucket).
Traces: the seven check ids above.

----------------------------------------------------------------
Cause 2 of 7: a Block-Public-Access refusal of PutBucketPolicy/PutBucketAcl answers the bare message "Access Denied" instead of AWS's message naming the caller, the action and the setting.
Failing checks: s3.block-public-policy-public-grant-rejected-oac-scoped-grant-accepted, s3.bucket-acl-public-grant-blocked-by-public-access-block-rejected, s3.s3-pab-block-public-policy-rejects-a-re-put-of-the-policy-it-let-stand

AWS: Smithy documents BlockPublicPolicy as "causes Amazon S3 to reject calls to PUT Bucket policy if the specified bucket policy allows public access" and BlockPublicAcls as "PUT Bucket ACL and PUT Object ACL calls fail if the specified ACL is public". The refusal is AccessDenied, and its message follows a fixed template whose variable parts come from the request and the caller's identity:
  `User: <caller-arn> is not authorized to perform: s3:PutBucketPolicy on resource: "<bucket-arn>" because public policies are prevented by the BlockPublicPolicy setting in S3 Block Public Access.`
  `User: <caller-arn> is not authorized to perform: s3:PutBucketAcl on resource: "<bucket-arn>" because public ACLs are prevented by the BlockPublicAcls setting in S3 Block Public Access.`
The fixed text is `User: ` / ` is not authorized to perform: ` / ` on resource: ` / ` because public policies are prevented by the BlockPublicPolicy setting in S3 Block Public Access.` (resp. `... public ACLs ... BlockPublicAcls ...`); the caller ARN, the action name and the quoted bucket ARN are values taken from the request and the calling identity, never literals.
Emulator: the error code is correct (AccessDenied) but the message is the bare string `Access Denied` in all three cases, so nothing distinguishes a BPA refusal from an ordinary IAM denial.
Why: the message is the only thing that tells an operator WHICH of the four BPA settings refused the call and that the cause is BPA rather than a missing IAM grant. With "Access Denied" the standard remediation path (loosen the identity policy) is wrong and the real one (adjust BlockPublicPolicy/BlockPublicAcls) is invisible, so callers retry or escalate permissions pointlessly.
Fix: the BPA evaluation in the PutBucketPolicy and PutBucketAcl handlers of the s3 crate must raise AccessDenied with the templated message above, interpolating the caller ARN, the action and the bucket ARN.
Traces: the three check ids above.

----------------------------------------------------------------
Cause 3 of 7: enum/range violations are refused with the generic model-constraint message instead of S3's own message.
Failing checks: s3.deep-s3-immutability-copy-unknown-tagging-directive-rejected, s3.get-object-attributes-invalid-attribute-name-rejected, s3.s3-oc-create-bucket-unknown-object-ownership-rejected-invalid-argument, s3.s3-old-buckets-list-buckets-max-buckets-above-ten-thousand-rejected

AWS: the model states the constraints — TaggingDirective is an enum over [COPY, REPLACE]; ObjectAttributes is an enum over [ETag, Checksum, ObjectParts, StorageClass, ObjectSize]; ObjectOwnership is an enum over [BucketOwnerPreferred, ObjectWriter, BucketOwnerEnforced]; MaxBuckets is an integer with range min 1 / max 10000. S3 refuses each violation with 400 InvalidArgument (the code the emulator already gets right) and its OWN per-field message, not a generic constraint report:
  CopyObject x-amz-tagging-directive outside the enum -> `Unknown tagging directive.` (entirely fixed text)
  GetObjectAttributes x-amz-object-attributes outside the enum -> `Invalid attribute name specified.` (entirely fixed text)
  CreateBucket x-amz-object-ownership outside the enum -> `Invalid x-amz-object-ownership header: <the rejected value>` (fixed prefix + the value from the request header)
  ListBuckets max-buckets outside the range -> `Argument max-buckets must be an integer between 1 and 10000` (entirely fixed text; the bounds are the modeled ones, not the request's value)
Emulator: all four answer InvalidArgument/400 with the framework's model-constraint prose, e.g. `Value 'MERGE' at 'taggingDirective' failed to satisfy constraint: Member must satisfy enum value set: [COPY, REPLACE]`, `Value 'NotARealAttribute' at 'objectAttributes.2.member' failed to satisfy constraint: ...`, `Value 'BucketOwnerOnly' at 'objectOwnership' failed to satisfy constraint: ...`, `Value '10001' at 'maxBuckets' failed to satisfy constraint: Member must have value less than or equal to 10000`.
Why: these messages name Smithy member paths (`objectAttributes.2.member`, `taggingDirective`) that no S3 client or document ever uses, so message-matching tooling and operator runbooks keyed on S3's wording break, and the emulator leaks its own model plumbing into the public error contract.
Fix: S3's request-validation layer must not let the generic model-constraint validator author these messages — each operation's header/query validation (CopyObject, GetObjectAttributes, CreateBucket, ListBuckets) must raise InvalidArgument with S3's own message for that field.
Traces: the four check ids above.

----------------------------------------------------------------
Cause 4 of 7: tagging-validation refusals use emulator-authored prose instead of S3's tagging error messages.
Failing checks: s3.object-tagging-exceeds-limit-rejected, s3.op-contract-put-bucket-tagging-invalid-tag-rejected, s3.s3unicode-object-tag-value-with-an-astral-emoji-rejected

AWS: the error codes are the ones the emulator already returns (InvalidTag / BadRequest-class 400), but the messages are fixed S3 text:
  more than 10 tags on an object -> `Object tags cannot be greater than 10`
  a bucket tag set naming the same key twice -> `Cannot provide multiple Tags with the same key`
  a tag value containing a character outside S3's permitted tag character set -> `The TagValue you have provided is invalid`
All three are entirely fixed text: S3 does NOT interpolate the offending key, value, count or the permitted character set.
Emulator: `Tagging.TagSet carries 11 tags, more than the 10 tags one object may hold`; `Tagging.TagSet names the key "dup" twice; a bucket tag set holds one value per key`; `Tagging.TagSet has a Tag.Value carrying '<the emoji>', which is not one of the letters, numbers, whitespace and ['+', '-', '=', '.', '_', ':', '/', '@'] a tag may hold`.
Why: the messages are reworded and, worse, they interpolate request content (the tag count, the duplicated key, the rejected value and an internal character list) that AWS's messages never carry. Clients that match on AWS's wording fail to classify the error, and tests written against the emulator encode values that will never appear against real S3.
Fix: the tag-set validation shared by PutObjectTagging/PutObject's x-amz-tagging and PutBucketTagging in the s3 crate must emit S3's fixed messages above rather than describing the violation.
Traces: the three check ids above.

----------------------------------------------------------------
Cause 5 of 7: version-id classification is inverted — a malformed VersionId is answered NoSuchVersion, and a well-formed but nonexistent one is answered InvalidArgument.
Failing checks: s3.get-object-nonexistent-version-errors, s3.s3-delete-objects-reports-every-requested-key-version-pair-exactly-once

AWS: S3 keeps the two conditions apart. A VersionId that is not a structurally valid version id is a bad argument: 400 InvalidArgument with message `Invalid version id specified`. A structurally valid VersionId that names no version of the key is 404 NoSuchVersion with message `The specified version does not exist.` In DeleteObjects the same classification appears per-entry in the Errors[] list (DeleteObjects itself has no modeled top-level errors; each result entry carries Key, VersionId, Code, Message).
Emulator: exactly swapped. GetObject with a structurally invalid VersionId answers `{'Code': 'NoSuchVersion', 'Message': 'The specified version does not exist.'}` where InvalidArgument/`Invalid version id specified` is required; DeleteObjects given a well-formed-but-never-issued VersionId reports `{'Key': 'batch/versioned.bin', 'VersionId': 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'Code': 'InvalidArgument', 'Message': 'Invalid version id specified'}` where the entry must read NoSuchVersion.
Why: the two codes drive opposite client behavior — InvalidArgument means "fix the request", NoSuchVersion means "that version is gone". Inverting them makes a cleanup tool treat an already-deleted version as a programming error and a genuinely malformed id as a benign 404, and in the DeleteObjects batch path a per-key result is misreported while the surrounding batch still succeeds.
Fix: one shared version-id parse/lookup in the s3 crate must classify well-formedness BEFORE existence and be used by both the GetObject (and HeadObject/DeleteObject) single-object path and the DeleteObjects per-entry path, so a parse failure yields InvalidArgument/`Invalid version id specified` and a successful parse with no matching version yields NoSuchVersion/`The specified version does not exist.`
Traces: the two check ids above.

----------------------------------------------------------------
Cause 6 of 7: GetBucketEncryption omits BlockedEncryptionTypes, so a new bucket does not report SSE-C as blocked.
Failing check: s3.s3-objver-versioned-hardened-bucket-config-round-trips

AWS: Smithy models ServerSideEncryptionRule with three members: ApplyServerSideEncryptionByDefault, BucketKeyEnabled and BlockedEncryptionTypes, the last documented as "A bucket-level setting for Amazon S3 general purpose buckets used to prevent the upload of new objects encrypted with the specified server-side encryption type... Currently, this parameter only supports blocking or unblocking server-side encryption with customer-provided keys (SSE-C)." A new general-purpose bucket reports its default rule WITH BlockedEncryptionTypes {EncryptionType: ["SSE-C"]} alongside SSEAlgorithm AES256 and BucketKeyEnabled false.
Emulator: GetBucketEncryption on a new bucket returns only `{'ApplyServerSideEncryptionByDefault': {'SSEAlgorithm': 'AES256'}, 'BucketKeyEnabled': False}` — the BlockedEncryptionTypes member is absent.
Why: a client reading the bucket's hardened configuration cannot see that SSE-C uploads are blocked, so it cannot distinguish a bucket that blocks SSE-C from one that permits it, and any posture round-trip (read the rule, re-put it) silently drops the setting.
Fix: the GetBucketEncryption serialiser and the bucket-encryption default installed at CreateBucket in the s3 crate must carry BlockedEncryptionTypes with EncryptionType ["SSE-C"], and PutBucketEncryption must round-trip the member.
Related open issue that does NOT carry this check's replay key: 20261025T032458Z-s3-object-plane-10-root-causes-conditional-requests.md (it owns the sibling sseinv-new-bucket-default check).
Traces: s3.s3-objver-versioned-hardened-bucket-config-round-trips.

----------------------------------------------------------------
Cause 7 of 7: UpdateObjectEncryption answers ValidationException for a key that does not exist, instead of its modeled NoSuchKey.
Failing check: s3.update-object-encryption-error-preconditions

AWS: Smithy models UpdateObjectEncryption (PUT /{Bucket}/{Key+}?encryption) with exactly three errors: AccessDenied (403), InvalidRequest (400) and NoSuchKey (404). ValidationException is not in that set. An UpdateObjectEncryption naming a Key that does not exist in the bucket is refused 404 NoSuchKey with S3's fixed message `The specified key does not exist.`, even when the supplied ObjectEncryption.SSEKMS.KMSKeyArn is itself well formed.
Emulator: the call is refused with error code `ValidationException` — `AssertionError: expected NoSuchKey, got 'ValidationException'`. The handler evidently validates the request body before resolving the object, and reports an error class the operation does not model.
Why: ValidationException is not in UpdateObjectEncryption's modeled error set, so an SDK consumer has no branch for it and cannot distinguish "that object is gone" (NoSuchKey — stop) from "my request is malformed" (InvalidRequest — fix and retry). Precondition ordering matters too: the later legs of the contract (InvalidRequest for a malformed KMS key ARN, AccessDenied without kms:GenerateDataKey) are unreachable while the not-found path misfires.
Fix: the UpdateObjectEncryption handler in the s3 crate must resolve Bucket/Key (and VersionId) first and raise NoSuchKey/`The specified key does not exist.` for a missing object, and must restrict its refusals to the modeled AccessDenied / InvalidRequest / NoSuchKey set.
Related open issue that does NOT carry this check's replay key: 20261025T032458Z-s3-remaining-divergences-8-root-causes-selectobjectcontent-routing.md (it owns the sibling s3.update-object-encryption-error-paths check).
Traces: s3.update-object-encryption-error-preconditions.

## Evidence
Replay report (suite simparity-behavioral-ga-prod-v1, 197/749 passed), service s3 146/347 (201 failed). The 21 failing checks below are the ones NOT marked ALREADY FILED or OWNED.

Cause 1 — no Block Public Access default on a new bucket (7 checks):
  [error] s3.public-access-block-state-machine —
    botocore.exceptions.ClientError: An error occurred (NoSuchPublicAccessBlockConfiguration) when calling the GetPublicAccessBlock operation: The public access block configuration was not found
    (fails at the FIRST call, line 94408: default_cfg = s3.get_public_access_block(Bucket=name), asserting all four members True)
  [error] s3.s3-pab-three-member-put-reads-back-the-omitted-member-false — same ClientError, at the precondition read before the three-member put
  [mismatch] s3.s3-pab-cli-text-query-prints-the-four-flags-in-member-order —
    AssertionError: aws s3api get-public-access-block exited 254 for sim2real-scn-pab-cli-...: 'aws: [ERROR]: An error occurred (NoSuchPublicAccessBlockConfiguration) when calling the GetPublicAccessBlock operation: The public access block configuration was not found'
    (expected stdout for the un-put bucket: "True\tTrue\tTrue\tTrue")
  [error] s3.deploy-s3-bucket-public-access-block-enforced-rejected — same ClientError, on a bucket created by a CloudFormation stack declaring PublicAccessBlockConfiguration
  [error] s3.e2e-b2b-commerce-summary-and-reports-lifecycle — [phase 1/7 deploy] ClientError: (NoSuchPublicAccessBlockConfiguration) ...
  [error] s3.e2e-create-s3-bucket-with-config-lifecycle — [phase 1/5 deploy] ClientError: (NoSuchPublicAccessBlockConfiguration) ...
  [error] s3.e2e-sync-buckets-replace-metadata-lifecycle — [phase 1/7 deploy] ClientError: (NoSuchPublicAccessBlockConfiguration) ...
  Expected: HTTP 200 and PublicAccessBlockConfiguration{BlockPublicAcls: true, IgnorePublicAcls: true, BlockPublicPolicy: true, RestrictPublicBuckets: true} on a bucket that was just created (GetPublicAccessBlock has an EMPTY modeled error list).
  Actual:   NoSuchPublicAccessBlockConfiguration / "The public access block configuration was not found".
  Still ungraded as a consequence: after PutPublicAccessBlock(IgnorePublicAcls, BlockPublicPolicy, RestrictPublicBuckets = true) the read-back must carry all four members with BlockPublicAcls explicitly false (replace, not merge).

Cause 2 — BPA refusal message is bare "Access Denied" (3 checks, verdict [mismatch]):
  s3.block-public-policy-public-grant-rejected-oac-scoped-grant-accepted —
    AssertionError: the BlockPublicPolicy refusal must name the caller, the action and the setting; got 'Access Denied'
    (code assertion directly above it passed: AccessDenied)
    Expected: User: <caller-arn> is not authorized to perform: s3:PutBucketPolicy on resource: "arn:aws:s3:::<bucket>" because public policies are prevented by the BlockPublicPolicy setting in S3 Block Public Access.
  s3.bucket-acl-public-grant-blocked-by-public-access-block-rejected —
    AssertionError: expected message 'User: arn:aws:iam::000000000000:user/emulator is not authorized to perform: s3:PutBucketAcl on resource: "arn:aws:s3:::sim2real-behav-aclblock-..." because public ACLs are prevented by the BlockPublicAcls setting in S3 Block Public Access.', got 'Access Denied'
  s3.s3-pab-block-public-policy-rejects-a-re-put-of-the-policy-it-let-stand —
    AssertionError: the BlockPublicPolicy refusal must name the caller, the action and the setting; got 'Access Denied'
  (The caller ARN and bucket name above are the values of that one run; the requirement is the template, not those values.)

Cause 3 — generic model-constraint messages (4 checks, verdict [mismatch]):
  s3.deep-s3-immutability-copy-unknown-tagging-directive-rejected —
    AssertionError: expected InvalidArgument/400 with AWS's exact message for an unknown TaggingDirective, got 'InvalidArgument'/400/"Value 'MERGE' at 'taggingDirective' failed to satisfy constraint: Member must satisfy enum value set: [COPY, REPLACE]"  (expected message: "Unknown tagging directive.")
  s3.get-object-attributes-invalid-attribute-name-rejected —
    AssertionError: {'Code': 'InvalidArgument', 'Message': "Value 'NotARealAttribute' at 'objectAttributes.2.member' failed to satisfy constraint: Member must satisfy enum value set: [ETag, Checksum, ObjectParts, StorageClass, ObjectSize]"}  (expected message: "Invalid attribute name specified.")
  s3.s3-oc-create-bucket-unknown-object-ownership-rejected-invalid-argument —
    AssertionError: the InvalidArgument refusal must carry the exact message 'Invalid x-amz-object-ownership header: BucketOwnerOnly'; got "Value 'BucketOwnerOnly' at 'objectOwnership' failed to satisfy constraint: Member must satisfy enum value set: [BucketOwnerPreferred, ObjectWriter, BucketOwnerEnforced]"
  s3.s3-old-buckets-list-buckets-max-buckets-above-ten-thousand-rejected —
    AssertionError: InvalidArgument for MaxBuckets=10001 must carry S3's own message; got "Value '10001' at 'maxBuckets' failed to satisfy constraint: Member must have value less than or equal to 10000"  (expected message: "Argument max-buckets must be an integer between 1 and 10000")

Cause 4 — tagging messages reworded and value-interpolated (3 checks, verdict [mismatch]):
  s3.object-tagging-exceeds-limit-rejected —
    AssertionError: unexpected message 'Tagging.TagSet carries 11 tags, more than the 10 tags one object may hold'  (expected: "Object tags cannot be greater than 10")
  s3.op-contract-put-bucket-tagging-invalid-tag-rejected —
    AssertionError: expected InvalidTag for duplicate tag key, got ('InvalidTag', 'Tagging.TagSet names the key "dup" twice; a bucket tag set holds one value per key')  (expected: ('InvalidTag', 'Cannot provide multiple Tags with the same key'))
  s3.s3unicode-object-tag-value-with-an-astral-emoji-rejected —
    AssertionError: unexpected message "Tagging.TagSet has a Tag.Value carrying '<emoji>', which is not one of the letters, numbers, whitespace and ['+', '-', '=', '.', '_', ':', '/', '@'] a tag may hold"  (expected: "The TagValue you have provided is invalid")

Cause 5 — version-id classification inverted (2 checks, verdict [mismatch]):
  s3.get-object-nonexistent-version-errors —
    AssertionError: expected InvalidArgument for a malformed VersionId, got {'Code': 'NoSuchVersion', 'Message': 'The specified version does not exist.'}
    Request: GetObject(Bucket=<b>, Key="k", VersionId=<a real version id with its last character altered>). Expected 400 InvalidArgument / "Invalid version id specified".
  s3.s3-delete-objects-reports-every-requested-key-version-pair-exactly-once —
    AssertionError: expected Errors[] Code 'NoSuchVersion' for the non-existent version ('batch/versioned.bin', 'aaaa...aaaa'), got 'InvalidArgument' in {'Key': 'batch/versioned.bin', 'VersionId': 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'Code': 'InvalidArgument', 'Message': 'Invalid version id specified'}
    Expected per-entry: Code NoSuchVersion / Message "The specified version does not exist."

Cause 6 — BlockedEncryptionTypes missing (verdict [mismatch]):
  s3.s3-objver-versioned-hardened-bucket-config-round-trips —
    check line 45: assert rules[0].get("BlockedEncryptionTypes") == {"EncryptionType": ["SSE-C"]}
    AssertionError: a new bucket's encryption rule reports SSE-C as blocked by default; got {'ApplyServerSideEncryptionByDefault': {'SSEAlgorithm': 'AES256'}, 'BucketKeyEnabled': False}

Cause 7 — UpdateObjectEncryption not-found misclassified (verdict [mismatch]):
  s3.update-object-encryption-error-preconditions —
    check line 55: assert code == "NoSuchKey"
    AssertionError: expected NoSuchKey, got 'ValidationException'
    Request: UpdateObjectEncryption(Bucket=<b>, Key="does-not-exist.bin", ObjectEncryption={SSEKMS:{KMSKeyArn: arn:aws:kms:<region>:<account>:key/00000000-0000-0000-0000-000000000000}}).
    Expected: 404 NoSuchKey / "The specified key does not exist." (modeled errors are only AccessDenied, InvalidRequest, NoSuchKey).
    Actual:   error code ValidationException, which the operation does not model.

## Steps to Reproduce
Boot the emulator from this checkout and replay the 21 `s3` checks listed in trace_ids.

Cause 1 (no BPA default):
  1. s3.CreateBucket(Bucket=<b>) (with CreateBucketConfiguration.LocationConstraint outside us-east-1).
  2. s3.GetPublicAccessBlock(Bucket=<b>)
     -> expect 200 with all four of BlockPublicAcls/IgnorePublicAcls/BlockPublicPolicy/RestrictPublicBuckets true.
     -> observe NoSuchPublicAccessBlockConfiguration "The public access block configuration was not found".
  3. `aws s3api get-public-access-block --bucket <b> --query 'PublicAccessBlockConfiguration.[BlockPublicAcls,IgnorePublicAcls,BlockPublicPolicy,RestrictPublicBuckets]' --output text` -> expect "True\tTrue\tTrue\tTrue"; observe exit 254.
  4. s3.PutPublicAccessBlock(Bucket=<b>, PublicAccessBlockConfiguration={IgnorePublicAcls:true, BlockPublicPolicy:true, RestrictPublicBuckets:true}) then GetPublicAccessBlock -> expect all four members present with BlockPublicAcls explicitly false.
  5. s3.DeletePublicAccessBlock(Bucket=<b>) then GetPublicAccessBlock -> expect NoSuchPublicAccessBlockConfiguration ONLY here.
  6. Deploy a CloudFormation stack declaring an AWS::S3::Bucket with PublicAccessBlockConfiguration and repeat step 2 against the stack's bucket.

Cause 2 (BPA refusal message):
  1. s3.CreateBucket(<b>); PutPublicAccessBlock with BlockPublicPolicy true and BlockPublicAcls true; put a non-public base policy.
  2. s3.PutBucketPolicy(Bucket=<b>, Policy=<a policy granting Principal "*">)
     -> expect AccessDenied with: User: <caller-arn> is not authorized to perform: s3:PutBucketPolicy on resource: "arn:aws:s3:::<b>" because public policies are prevented by the BlockPublicPolicy setting in S3 Block Public Access.
     -> observe AccessDenied / "Access Denied".
  3. s3.PutBucketAcl(Bucket=<b>, ACL="public-read")
     -> expect the BlockPublicAcls variant of the same template (s3:PutBucketAcl / "public ACLs" / BlockPublicAcls); observe "Access Denied".
  4. Confirm GetBucketPolicy still returns the unchanged base policy.

Cause 3 (model-constraint messages):
  1. s3.CopyObject(..., TaggingDirective="MERGE") -> expect 400 InvalidArgument / "Unknown tagging directive."
  2. s3.GetObjectAttributes(..., ObjectAttributes=["ETag","ObjectSize","NotARealAttribute"]) -> expect 400 InvalidArgument / "Invalid attribute name specified."
  3. s3.CreateBucket(..., ObjectOwnership="BucketOwnerOnly") -> expect 400 InvalidArgument / "Invalid x-amz-object-ownership header: BucketOwnerOnly"
  4. s3.ListBuckets(MaxBuckets=10001) -> expect 400 InvalidArgument / "Argument max-buckets must be an integer between 1 and 10000"
  In each case observe the generic "Value '<v>' at '<member>' failed to satisfy constraint: ..." message instead.

Cause 4 (tagging messages):
  1. s3.PutObjectTagging(Bucket=<b>, Key=<k>, Tagging={TagSet: 11 distinct tags}) -> expect "Object tags cannot be greater than 10".
  2. s3.PutBucketTagging(Bucket=<b>, Tagging={TagSet: [{Key:"dup",Value:"a"},{Key:"dup",Value:"b"}]}) -> expect InvalidTag / "Cannot provide multiple Tags with the same key".
  3. s3.PutObjectTagging(Bucket=<b>, Key=<k>, Tagging={TagSet:[{Key:"k",Value:"<astral emoji>"}]}) -> expect "The TagValue you have provided is invalid".

Cause 5 (version ids):
  1. s3.CreateBucket(<b>); PutBucketVersioning Enabled; PutObject("k") twice -> v1, v2.
  2. s3.GetObject(Bucket=<b>, Key="k", VersionId=<v1 with its last character changed>)
     -> expect 400 InvalidArgument / "Invalid version id specified"; observe NoSuchVersion / "The specified version does not exist."
  3. s3.DeleteObject(Bucket=<b>, Key="k", VersionId=v1), then GetObject with v1 -> expect NoSuchVersion.
  4. s3.DeleteObjects(Bucket=<b>, Delete={Objects:[{Key:"batch/versioned.bin", VersionId:"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}, ...]})
     -> expect that entry in Errors[] with Code NoSuchVersion; observe Code InvalidArgument / "Invalid version id specified".

Cause 6 (BlockedEncryptionTypes):
  1. s3.CreateBucket(<b>); s3.GetBucketEncryption(Bucket=<b>)
     -> expect Rules[0] = {ApplyServerSideEncryptionByDefault:{SSEAlgorithm:"AES256"}, BucketKeyEnabled:false, BlockedEncryptionTypes:{EncryptionType:["SSE-C"]}}.
     -> observe the rule without BlockedEncryptionTypes.

Cause 7 (UpdateObjectEncryption):
  1. s3.CreateBucket(<b>); s3.PutObject(Bucket=<b>, Key="encrypted-object.bin").
  2. s3.UpdateObjectEncryption(Bucket=<b>, Key="does-not-exist.bin", ObjectEncryption={SSEKMS:{KMSKeyArn:"arn:aws:kms:<region>:<account>:key/00000000-0000-0000-0000-000000000000"}})
     -> expect 404 NoSuchKey / "The specified key does not exist."; observe ValidationException.
  3. Then, on the existing key, UpdateObjectEncryption with KMSKeyArn="not-a-valid-kms-key-arn-string" -> expect InvalidRequest; and without kms:GenerateDataKey on the target key -> expect AccessDenied. Both legs are unreachable while step 2 misclassifies.
