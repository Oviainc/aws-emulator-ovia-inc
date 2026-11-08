Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.deploy-s3-1-bucket-update-versioning-policy-rename
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.deploy-s3-1-bucket-update-versioning-policy-rename|run#34|AssertionError|assert current.get("VersionId") == "null", (
Issue-Commit: b173a8e674d7ba975def7d68c06d3de96ae43616

# s3: an object written while bucket versioning is Suspended is read back with no VersionId instead of the "null" version id

> _Note: this issue was identified against the code base at commit `b173a8e674d7ba975def7d68c06d3de96ae43616`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Cause 1 of 1: after versioning is moved from Enabled to Suspended, a PutObject stores the object as the "null" version, but GetObject of the current object reports no version id at all.
AWS: in a bucket whose versioning status is Suspended, a write creates or replaces the single version whose version id is the literal string "null". PutObject itself returns no x-amz-version-id for that write (which the emulator already gets right), but a subsequent read of the current object reports it: GetObject answers 200 with x-amz-version-id: null, and ListObjectVersions lists that object alongside the pre-suspension version ids with VersionId "null". Smithy (com-amazonaws-s3-amazons3): GetObject output member VersionId is bound to HTTP header x-amz-version-id (target ObjectVersionId); the "null" version id for a suspended-versioning write is documented S3 behavior and is what the recording shows.
Emulator: GetObject of that key returns the correct body (the post-suspension bytes) but omits the x-amz-version-id header entirely, so the SDK surfaces VersionId = None instead of "null".
Why: a client that addresses, copies, deletes or audits the current object by the version id it just read has nothing to address — it cannot distinguish "unversioned/null version" from "no version information", and code that round-trips GetObject's VersionId into a versioned GetObject/DeleteObject/CopySource call fails or silently targets the wrong thing. Inventory and compliance tooling that reconciles ListObjectVersions against per-object reads also diverges.
Fix: the GetObject (and HeadObject) response serializer/state in the s3 service crate — the object-version model must represent the suspended-versioning write as version id "null" and emit it in x-amz-version-id, rather than treating it as versionless.
Traces: s3.deploy-s3-1-bucket-update-versioning-policy-rename.

## Evidence
Setup: bucket versioning Enabled, two writes producing version ids v1 and v2; a CloudFormation update then sets versioning to Suspended (GetBucketVersioning correctly reports Status=Suspended) and a third write is made to the same key.
Expected (AWS): PutObject returns no VersionId (passes); GetObject of the key returns the third write's bytes with x-amz-version-id: null, i.e. response VersionId == "null"; ListObjectVersions for the key lists exactly {v1, v2, "null"}.
Actual (emulator): GetObject returns the correct body but no x-amz-version-id header, so VersionId is absent (None). Assertion: "current object after a Suspended-bucket write has VersionId None, expected 'null'". The concrete bucket name, key and the pre-suspension version ids in the recording are per-run values.

## Steps to Reproduce
1. Create a bucket and PutBucketVersioning Status=Enabled.
2. PutObject the same key twice; keep the two returned version ids.
3. PutBucketVersioning Status=Suspended (confirm GetBucketVersioning reports Suspended).
4. PutObject the same key once more — the response correctly carries no VersionId.
5. GetObject the key with no VersionId parameter. Observed: 200 with the new bytes but no x-amz-version-id header (VersionId absent); expected x-amz-version-id: null.
6. ListObjectVersions with that key's prefix and compare: AWS lists the two pre-suspension ids plus "null".
