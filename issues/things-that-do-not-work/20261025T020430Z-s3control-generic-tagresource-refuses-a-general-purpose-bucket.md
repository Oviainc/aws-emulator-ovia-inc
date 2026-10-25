Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__s3control.xsvc-s3control-tag-resource-visible-through-s3-get-bucket-tagging s3control.storage-lens-group-update-and-arn-tagging-roundtrip
Issue-Key: s3__s3control|TagResource|ValidationException
Issue-Key: s3control.storage-lens-group-update-and-arn-tagging-roundtrip|run#14|AssertionError|assert updated["Filter"]["And"]["MatchObjectSize"] == {"BytesGreaterThan": 1024}, (
Issue-Commit: 8e40a1008c33b06d6b244a457efc673d78e6bf69

# s3control: generic TagResource refuses a general-purpose bucket ARN (cross-service tag store unreachable), and StorageLensGroup filters come back with default-0 members AWS omits (2 causes)

> _Note: this issue was identified against the code base at commit `8e40a1008c33b06d6b244a457efc673d78e6bf69`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource, GetStorageLensGroup/UpdateStorageLensGroup

## Description
Two root causes in the s3control emulator, both grounded in failing behavioral checks of this suite. Each must be closed before this issue is submitted.

Note on attribution: Cause 1's check id is multi-service (`s3__s3control.xsvc-...`), so it carries no single leading service token; the failing call is s3control's TagResource, so the issue is filed under service `s3control` (both `s3` and `s3control` are bootstrapped, so the gate verdict is FILE either way).

Cause 1 of 2: s3control TagResource rejects ResourceArn="arn:aws:s3:::<bucket>" with a JSON-protocol ValidationException, so a bucket tag set can never be written through s3control's generic tagging surface and the cross-service read path is unreachable.
AWS: recorded behavior only (the Smithy model binds NO errors to TagResource). Smithy: TagResource — POST /v20180820/tags/{ResourceArn+}, success 204; input ResourceArn targets S3ResourceArn (@length 0..1011, @pattern ^arn:[^:]+:s3(express)?:[^:]) and is required, AccountId is the x-amz-account-id header/host label. The recording shows the general-purpose bucket ARN form `arn:aws:s3:::<bucket>` being ACCEPTED and the tags landing in the SAME tag store that S3's own GetBucketTagging reads back: before the write, s3 GetBucketTagging answers NoSuchTagSet ("The TagSet does not exist") — the emulator already matches that — and after s3control TagResource writes Team=platform and CostCenter=engineering, s3 GetBucketTagging must return exactly those two key/value pairs. So there is one tag store per bucket, not one per API surface. (Model tension worth resolving in the fix: the modeled S3ResourceArn pattern requires a non-colon character after `s3:` and its doc names only directory buckets / Storage Lens groups / Access Grants resources, yet the service accepts the general-purpose bucket ARN; independently of that, s3control is a REST-XML surface whose refusals are XML <Error><Code>...</Code> documents — a "N validation errors detected ... failed to satisfy constraint" ValidationException body is a JSON-protocol shape foreign to this service.)
Emulator: the call is refused with error code `ValidationException` (HTTP 400), example message "1 validation error detected: Value 'arn:aws:s3:::sim2real-xsvc-s3ctagwrite-790d760c8adc' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:[^:]+:s3(express)?:[^:]". No tag is recorded, and the cross-service assertion behind the write is never reached.
Why: a client that tags a bucket through s3control and reads it back through S3 (or vice versa) gets a hard 400 on a call real AWS accepts, and it never matches the error code it is handed because s3control's documented refusal codes are the XML ones (InvalidRequest/InvalidArgument), not ValidationException. A target holding two independent tag stores would also pass each envelope alone, so this path is the only thing that proves one store.
Related open issue, which does NOT cover this check: 20261022T213144Z-s3control-access-points-directory-bucket-listing-job-object.md describes the same ARN rejection as its Cause 7, but its replay list carries only `s3control.untag-resource-removes-specific-bucket-tag-keys` (the s3control-only direction, write-then-untag). It does not carry this cross-service check and does not state the requirement that an s3control TagResource write is visible through s3 GetBucketTagging; whoever fixes either must re-replay this check too.
Fix: ResourceArn parsing/validation for the s3control tagging handlers (accept the general-purpose bucket ARN form and dispatch it to the SHARED bucket tag store that S3's GetBucketTagging/PutBucketTagging read and write), plus the s3control error serializer, which must emit this service's XML error envelope instead of a JSON ValidationException.
Traces: s3__s3control.xsvc-s3control-tag-resource-visible-through-s3-get-bucket-tagging.

Cause 2 of 2: a StorageLensGroup filter is read back with the absent MatchObjectSize member materialized as its 0 default, so the round-trip is not value-for-value.
AWS: Smithy: UpdateStorageLensGroup — PUT /v20180820/storagelensgroup/{Name}, 204; GetStorageLensGroup — GET /v20180820/storagelensgroup/{Name}, 200, httpPayload StorageLensGroup. StorageLensGroupAndOperator.MatchObjectSize targets MatchObjectSize, whose members BytesGreaterThan and BytesLessThan are ObjectSizeValue longs carrying @default 0; their docs require "a positive number, greater than 0" (BytesLessThan must be "greater than the minimum object size"), so 0 is not a meaningful bound. Recorded behavior: a group updated with Filter.And.MatchObjectSize={BytesGreaterThan: 1024} reads back as exactly {"BytesGreaterThan": 1024} — the member that was not sent is OMITTED, not defaulted; the update also REPLACES the filter (the standalone MatchAnyPrefix clause is gone) and does not re-mint StorageLensGroupArn.
Emulator: GetStorageLensGroup after that update returns Filter.And = {'MatchAnyPrefix': ['logs/'], 'MatchObjectSize': {'BytesGreaterThan': 1024, 'BytesLessThan': 0}} — the unsent BytesLessThan appears as 0. (The replacement of the top-level filter did take effect; the assertion that fails is the value-for-value one on the And composite.)
Why: a client that round-trips a group document — reads it, edits one clause, writes it back — now submits BytesLessThan=0, a bound AWS documents as invalid (and which, taken literally, is an empty size range), and any client diffing stored configuration against its desired state sees a permanent spurious difference.
Fix: the StorageLens-group filter model/serializer path for this service — absent optional members of MatchObjectSize/MatchObjectAge must stay absent through deserialize → store → serialize rather than being filled with their Smithy @default, in the StorageLensGroup state record and the GetStorageLensGroup output serializer (handler: services/service-com-amazonaws-s3control-awss3controlservicev20180820/src/ops/get_storage_lens_group.rs, which echoes the stored record verbatim, so the defaulting happens on the write/deserialize side).
Traces: s3control.storage-lens-group-update-and-arn-tagging-roundtrip.

## Evidence
Suite: simparity-behavioral-ga-prod-v1 — total 3/35 passed; s3control 3/33 (9%), s3__s3control 0/1, lambda__s3__s3control 0/1.

Cause 1 — [error] s3__s3control.xsvc-s3control-tag-resource-visible-through-s3-get-bucket-tagging:
botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the TagResource operation: 1 validation error detected: Value 'arn:aws:s3:::sim2real-xsvc-s3ctagwrite-790d760c8adc' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:[^:]+:s3(express)?:[^:]
Raised at the s3c.tag_resource(AccountId=..., ResourceArn='arn:aws:s3:::<bucket>', Tags=[{Team,platform},{CostCenter,engineering}]) call. The preceding assertion — s3 GetBucketTagging on the untagged bucket returns ('NoSuchTagSet', 'The TagSet does not exist') — PASSED, so the only divergence observed is the refusal of the bucket ARN. Expected: TagResource succeeds (204) and the following s3.get_bucket_tagging returns TagSet with exactly Team=platform and CostCenter=engineering.

Cause 2 — [mismatch] s3control.storage-lens-group-update-and-arn-tagging-roundtrip:
AssertionError: the And composite read back as {'MatchAnyPrefix': ['logs/'], 'MatchObjectSize': {'BytesGreaterThan': 1024, 'BytesLessThan': 0}}
at check_s3control_storage_lens_group_update_and_arn_tagging_roundtrip.py:67 — assert updated["Filter"]["And"]["MatchObjectSize"] == {"BytesGreaterThan": 1024}, where updated = s3c.get_storage_lens_group(AccountId, Name)["StorageLensGroup"] after update_storage_lens_group(StorageLensGroup={"Name": group, "Filter": {"And": {"MatchAnyPrefix": ["logs/"], "MatchObjectSize": {"BytesGreaterThan": 1024}}}}). Expected MatchObjectSize == {"BytesGreaterThan": 1024} with no BytesLessThan member. The checks after that line (ARN stability across the update, TagResource/ListTagsForResource/UntagResource on the group ARN, NoSuchConfiguration/NoSuchResource after delete) were never reached and are not claimed here.

## Steps to Reproduce
Replay the behavioral suite against the emulator and re-run exactly the checks in trace_ids.

1. Cause 1 — s3 CreateBucket(<bucket>) in the test region; s3 GetBucketTagging(<bucket>) and expect NoSuchTagSet; s3control TagResource(AccountId=<account>, ResourceArn="arn:aws:s3:::<bucket>", Tags=[{Key:Team,Value:platform},{Key:CostCenter,Value:engineering}]) and expect HTTP 204 (not ValidationException); then s3 GetBucketTagging(<bucket>) and expect a TagSet of exactly Team=platform and CostCenter=engineering.

2. Cause 2 — s3control CreateStorageLensGroup(AccountId, StorageLensGroup={Name:<g>, Filter:{MatchAnyPrefix:["logs/"]}}); UpdateStorageLensGroup(AccountId, Name=<g>, StorageLensGroup={Name:<g>, Filter:{And:{MatchAnyPrefix:["logs/"], MatchObjectSize:{BytesGreaterThan:1024}}}}); GetStorageLensGroup(AccountId, Name=<g>) and expect Filter.And.MatchObjectSize == {BytesGreaterThan: 1024} with BytesLessThan ABSENT, Filter with no top-level MatchAnyPrefix, and StorageLensGroupArn unchanged.
