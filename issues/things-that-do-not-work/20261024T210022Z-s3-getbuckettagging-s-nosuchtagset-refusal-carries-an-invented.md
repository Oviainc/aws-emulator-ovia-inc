Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__s3control.xsvc-s3control-tag-resource-visible-through-s3-get-bucket-tagging
Issue-Key: s3__s3control.xsvc-s3control-tag-resource-visible-through-s3-get-bucket-tagging|run#10|AssertionError|assert err == want, f"GetBucketTagging on an untagged bucket returned {err!r}, expected {want!r}"
Issue-Commit: 1302d977b6b888f4927f8769f8d72a08ded5722c

# S3 GetBucketTagging's NoSuchTagSet refusal carries an invented message instead of the service's fixed "The TagSet does not exist"

> _Note: this issue was identified against the code base at commit `1302d977b6b888f4927f8769f8d72a08ded5722c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBucketTagging

## Description
One root cause, grounded in a failing behavioral check of suite simparity-behavioral-ga-prod-v1. Attribution note: the failing check id is s3__s3control.xsvc-s3control-tag-resource-visible-through-s3-get-bucket-tagging, whose leading token is the compound "s3__s3control"; the diverging response is S3's GetBucketTagging, so this issue is filed under service s3 (both s3 and s3control are bootstrapped).

Cause 1 of 1: GetBucketTagging on a bucket that has no tag set returns the right code with a different message.
AWS: Smithy GetBucketTagging (GET /{Bucket}?tagging, success 200, output TagSet required) binds no errors; recorded behavior: a bucket with no tag set is refused with error code NoSuchTagSet and the service's FIXED message "The TagSet does not exist" — a constant for this condition, with nothing interpolated from the request (AWS's S3 error-code list documents NoSuchTagSet with that same text).
Emulator: code NoSuchTagSet is correct, but the message is "There is no tag set associated with the bucket" — a different constant.
Why: for this condition the message is part of the service's contract, not an incidental string; clients and compatibility tests that match S3's documented not-found text do not match. The divergence also aborts the check at its first assertion, so the cross-service property it exists to grade — tags written through s3control TagResource(ResourceArn=arn:aws:s3:::<bucket>) being readable value-for-value through s3 GetBucketTagging, i.e. one shared bucket tag store rather than two per-API stores — is never exercised and remains ungraded.
Fix: the GetBucketTagging handler's no-tag-set error construction in the S3 error mapping (use the service's constant message).
Traces: s3__s3control.xsvc-s3control-tag-resource-visible-through-s3-get-bucket-tagging.

## Evidence
Suite simparity-behavioral-ga-prod-v1 — s3__s3control 0/1 passed.

AssertionError: GetBucketTagging on an untagged bucket returned ('NoSuchTagSet', 'There is no tag set associated with the bucket'), expected ('NoSuchTagSet', 'The TagSet does not exist') — check_s3__s3control_xsvc_s3control_tag_resource_visible_through_s3_get_bucket_tagging.py:42. The check never reached the s3control TagResource write or the subsequent GetBucketTagging read-back.

## Steps to Reproduce
Replay the behavioral suite against the emulator and re-run the check in trace_ids.

1. CreateBucket(Bucket=<new bucket>, CreateBucketConfiguration={LocationConstraint: <region>}).
2. GetBucketTagging(Bucket=<that bucket>) with no tags ever written — expect error code NoSuchTagSet with message exactly "The TagSet does not exist".
3. Then s3control TagResource(AccountId, ResourceArn="arn:aws:s3:::<bucket>", Tags=[{Team: platform}, {CostCenter: engineering}]) and s3 GetBucketTagging again — expect both tags, value for value, from the same store.
