Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__iam__s3.gjwr-bucket-name-reuse-from-the-expected-region-rejected
Issue-Key: glue__iam__s3.gjwr-bucket-name-reuse-from-the-expected-region-rejected|_error_code#5|AssertionError|raise AssertionError("the call under test SUCCEEDED where real AWS refuses it")
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# glue__iam__s3: S3 bucket names are a per-region namespace — CreateBucket of a name that exists in another region succeeds instead of BucketAlreadyOwnedByYou

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBucket

## Description
Cause 1 of 1: the bucket namespace is scoped per region instead of globally.
AWS: S3 bucket names are global. While a bucket exists in region A, CreateBucket for the same name in region B (recorded both with a CreateBucketConfiguration LocationConstraint and from us-east-1) is refused with BucketAlreadyOwnedByYou, and the bucket stays in region A — GetBucketLocation keeps reporting A.
Emulator: the second CreateBucket in the expected region SUCCEEDED (no ClientError at all), so two same-named buckets can exist in different regions.
Why: this is the exact trap the check guards — 'redeploy the stack to the correct region' only works if globally scoped names can be re-minted there, and a bucket name cannot. An emulator that accepts the second create reports a successful remediation while a real redeploy would fail, and any client logic that treats BucketAlreadyOwnedByYou as 'this name is mine elsewhere' never fires.
Fix: key the bucket registry by name alone (account-global) in the s3 service crate's state and have CreateBucket refuse an existing name owned by the caller with BucketAlreadyOwnedByYou (409), leaving the bucket's recorded location unchanged.
Traces: glue__iam__s3.gjwr-bucket-name-reuse-from-the-expected-region-rejected.

## Evidence
AssertionError: the call under test SUCCEEDED where real AWS refuses it — raised by the check's _error_code() helper around the second CreateBucket. Expected: error code 'BucketAlreadyOwnedByYou', and GetBucketLocation for that name still reporting the original (other) region.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. CreateBucket NAME in region A (e.g. with LocationConstraint A).
3. CreateBucket the same NAME in region B — observe 200 instead of BucketAlreadyOwnedByYou.
4. GetBucketLocation(NAME) from both endpoints.
5. Re-replay glue__iam__s3.gjwr-bucket-name-reuse-from-the-expected-region-rejected to confirm.
