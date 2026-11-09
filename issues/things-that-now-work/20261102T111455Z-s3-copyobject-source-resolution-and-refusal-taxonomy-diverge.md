Issue-Source: trace-sweep-divergence
Issue-Trace: s3-mvb-copy-source-header-mistakes
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-mvb-copy-source-header-mistakes s3-mvs3-copy-source-header-mistakes s3-sync-copy-source-mistakes s3-eescfn-overwrote-baseline-restore-from-version s3-mvb-delete-before-copy-recovery s3-mvs3-delete-before-copy-version-recovery s3-sync-canned-acl-choices-on-enforced-bucket s3-sync-metadata-header-mistakes
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|CopyObject|4xx->4xx|$.Error.BucketName
Issue-Key: s3|CopyObject|4xx->4xx|$.Error.Code
Issue-Key: s3|CopyObject|4xx->4xx|status
Issue-Key: s3|CopyObject|2xx->4xx|$.CopyObjectResult
Issue-Key: s3|CopyObject|2xx->4xx|$headers.x-amz-server-side-encryption
Issue-Key: s3|CopyObject|2xx->4xx|status
Issue-Key: s3|CopyObject|4xx->2xx|$.Error
Issue-Key: s3|CopyObject|4xx->2xx|status
Issue-Key: s3|CopyObject|4xx->4xx|$headers.x-amz-delete-marker
Issue-Key: s3|CopyObject|4xx->4xx|$headers.x-amz-version-id
Issue-Key: s3|CopyObject|4xx->4xx|$.Error.MaxSizeAllowed
Issue-Key: s3|CopyObject|4xx->4xx|$.Error.Size
Issue-Related: 20261030T053639Z-s3-multipart-range-and-precondition-error-envelopes-drop.md
Issue-Related: 20261030T053639Z-s3-object-error-envelopes-omit-key-and-versionid.md
Issue-Related: 20261030T053639Z-s3-refusals-use-the-wrong-error-code-and.md
Issue-Commit: 717da2edf9c8dae639a46753310fbbbfd49465e2

# s3 CopyObject source resolution and refusal taxonomy diverge

> _Note: this issue was identified against the code base at commit `717da2edf9c8dae639a46753310fbbbfd49465e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CopyObject

## Description
Cause 1 of 7: a percent-encoded x-amz-copy-source is rejected by a regex instead of being resolved. AWS: the header is URL-decoded into bucket+key and the request fails on the resolved reference - 404 NoSuchBucket 'The specified bucket does not exist' with <BucketName>, or 404 NoSuchKey 'The specified key does not exist.' with <Key>. Recorded behavior only. Emulator: 400 InvalidArgument "Value '<encoded source>' at 'copySource' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\/?.+\/.+$" - it never decodes the %2F separators, so every encoded copy source is malformed to it. Why: SDK/CLI copies of keys containing '/' or spaces fail with a constraint error instead of the real reason, and valid copies fail outright. Fix: decode x-amz-copy-source before resolving it in the CopyObject handler. Traces: s3-mvb-copy-source-header-mistakes#task#3, s3-mvb-copy-source-header-mistakes#task#4, s3-mvb-copy-source-header-mistakes#task#6 (a valid copy, expected 200, refused 400).

Cause 2 of 7: a copy from a nonexistent key succeeds. AWS: 404 NoSuchKey 'The specified key does not exist.' with <Key> = the source key as resolved. Recorded behavior only. Emulator: 200 - a copy result is returned. Why: the caller believes an object was copied when nothing was; downstream reads then 404. Fix: resolve and verify the source in state before writing the destination. Traces: s3-mvs3-copy-source-header-mistakes#task#3.

Cause 3 of 7: an unparseable copy-source URI blames the bucket. AWS: 400 InvalidArgument 'Invalid copy source URI.' with ArgumentName/ArgumentValue 'x-amz-copy-source'. Recorded behavior only. Emulator: 404 NoSuchBucket 'The specified bucket does not exist'. Why: the caller retries bucket creation for a header-syntax fault. Fix: classify the parse failure in the CopyObject source parser. Traces: s3-sync-copy-source-mistakes#task#4.

Cause 4 of 7: version-qualified copy refusals use the wrong code and sentence. AWS: a copy source with a bad or stale versionId answers 400 Code 'InvalidRequest', Message 'Invalid Request' (fixed text). Recorded behavior only. Emulator: 400 InvalidArgument 'Invalid version id specified', or 404 NoSuchVersion 'The specified version does not exist.' Why: retry logic keyed on InvalidRequest vs NoSuchVersion branches wrongly, and a 4xx class flips between 400 and 404. Fix: the versionId resolution branch of CopyObject. Traces: s3-sync-copy-source-mistakes#task#6, s3-eescfn-overwrote-baseline-restore-from-version#task#7, s3-eescfn-overwrote-baseline-restore-from-version#task#8.

Cause 5 of 7: copying from a delete-marked key omits the delete-marker signalling. AWS: 404 with <Key> set, plus headers x-amz-delete-marker: true and x-amz-version-id = the marker's version id. Recorded behavior only. Emulator: the 404 carries neither the Key member nor those headers. Why: a client cannot tell 'deleted' from 'never existed' and cannot address the marker to undo it. Fix: the delete-marker branch of the copy-source resolver. Traces: s3-mvb-delete-before-copy-recovery#task#6.

Cause 6 of 7: naming a delete marker by version id as the copy source answers MethodNotAllowed. AWS: 400 Code 'InvalidRequest', Message 'The source of a copy request may not specifically refer to a delete marker by version id.' (fixed text). Recorded behavior only. Emulator: 405 MethodNotAllowed 'The specified method is not allowed against this resource.' Why: both the status class and the explanation are wrong, so the caller cannot learn what to change. Fix: the same resolver branch. Traces: s3-mvb-delete-before-copy-recovery#task#7.

Cause 7 of 7: a canned ACL on an ACL-disabled destination is accepted, and oversized metadata is misclassified. AWS: a copy carrying x-amz-acl into a BucketOwnerEnforced bucket answers 400 AccessControlListNotSupported 'The bucket does not allow ACLs'; metadata over the limit answers 400 MetadataTooLarge 'Your metadata headers exceed the maximum allowed metadata size' with Size and MaxSizeAllowed. Recorded behavior only. Emulator: 200 for the canned ACL, and 400 InvalidRequest "A parameter or header in your request isn't valid. For details, see the description of this API operation." for the metadata case. Why: an ACL the bucket forbids is silently honoured, and the metadata refusal names no limit. Fix: the CopyObject header validation path. Traces: s3-sync-canned-acl-choices-on-enforced-bucket#task#5, s3-sync-metadata-header-mistakes#task#5.

## Evidence
Replay of simparity-wire-aws-v1. Representative s3-mvb-copy-source-header-mistakes#task#3: status expected 404 got 400; Code expected 'NoSuchBucket' got 'InvalidArgument'; Message got the copySource regex constraint text; BucketName missing. #task#4: expected 404 NoSuchKey with Key, got the same regex refusal. #task#6: expected 200 with CopyObjectResult (ETag, ChecksumCRC32, ChecksumType) and x-amz-server-side-encryption, got 400. s3-mvs3-copy-source-header-mistakes#task#3: expected 404 NoSuchKey with Key 'sourc/reports/q1 2024 final.txt', got 200. s3-sync-copy-source-mistakes#task#4: expected 400 InvalidArgument 'Invalid copy source URI.', got 404 NoSuchBucket. #task#6 and s3-eescfn-overwrote-baseline-restore-from-version#task#7/#task#8: expected InvalidRequest 'Invalid Request', got InvalidArgument 'Invalid version id specified' / NoSuchVersion. s3-mvb-delete-before-copy-recovery#task#6: Error.Key, x-amz-delete-marker and x-amz-version-id missing; #task#7: expected 400 InvalidRequest 'The source of a copy request may not specifically refer to a delete marker by version id.', got 405 MethodNotAllowed. s3-sync-canned-acl-choices-on-enforced-bucket#task#5: expected 400 AccessControlListNotSupported, got 200. s3-sync-metadata-header-mistakes#task#5: expected MetadataTooLarge with Size '2108' and MaxSizeAllowed '2048'.

## Steps to Reproduce
1. Replay s3-mvb-copy-source-header-mistakes and compare steps task#3, task#4 and task#6. 2. Replay s3-mvs3-copy-source-header-mistakes step task#3 and s3-sync-copy-source-mistakes steps task#4/task#6. 3. Replay s3-mvb-delete-before-copy-recovery steps task#6 and task#7. 4. Replay s3-sync-canned-acl-choices-on-enforced-bucket step task#5 and s3-sync-metadata-header-mistakes step task#5.
