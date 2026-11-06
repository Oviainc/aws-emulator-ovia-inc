Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__s3.cdk-bootstrap-asset-deploy-is-content-addressed
Issue-Key: cloudformation__iam__lambda__s3.cdk-bootstrap-asset-deploy-is-content-addressed|_cdk#18|AssertionError|assert rc == expect_rc, f"`cdk {args[0]}` rc={rc}, expected {expect_rc}: {out[-1200:]!r}"
Issue-Commit: 5b7d66e77a060af6cd000fd4d7e09192c88c4502

# CDK asset publishing fails: the bootstrap file-publishing role is denied s3:ListBucket on the CDK assets bucket, so `cdk deploy` exits 1

> _Note: this issue was identified against the code base at commit `5b7d66e77a060af6cd000fd4d7e09192c88c4502`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListObjectsV2

## Description
Cause 1 of 1: an assumed CDK file-publishing role is refused s3:ListBucket on the bootstrap assets bucket it owns, so asset publication (and therefore every content-addressed `cdk deploy`) fails.
AWS: the CDK bootstrap stack creates `cdk-<qualifier>-file-publishing-role-<account>-<region>` with a policy granting s3:ListBucket/GetObject*/PutObject* on the bootstrap assets bucket and its objects; after assuming that role the CLI's HeadBucket/ListObjectsV2 against `cdk-<qualifier>-assets-<account>-<region>` succeeds and the template and Code assets publish, so `cdk deploy` exits 0 and the asset keys are content-addressed. Recorded behavior only for the role/bucket naming.
Emulator: publication fails with the authorization refusal `User: arn:aws:sts::<account>:assumed-role/cdk-<qualifier>-file-publishing-role-<account>-<region>/aws-cdk-mockforge is not authorized to perform: s3:ListBucket on resource: "arn:aws:s3:::cdk-<qualifier>-assets-<account>-<region>" because no identity-based policy allows the s3:ListBucket action`, twice, then `Failed to publish asset ... Template` and an AggregateError; `cdk deploy` returns rc=1 where 0 is expected. The fixed text is AWS's authorization-refusal wording; the role/session/bucket names come from the bootstrap state.
Why: no CDK application can deploy against the emulator - asset publishing is the first step of every `cdk deploy` - and the refusal blames a policy that the bootstrap stack did grant.
Fix: either the IAM authorization evaluation that resolves the bootstrap role's identity-based policy for s3:ListBucket (bucket-level ARN with no object suffix), or the bootstrap role's policy as the emulator materialises it.
Traces: cloudformation__iam__lambda__s3.cdk-bootstrap-asset-deploy-is-content-addressed.

## Evidence
Expected (AWS): `cdk deploy --require-approval never` exits 0 after publishing the Template and Code assets under content-addressed keys in the bootstrap assets bucket.
Actual (emulator): rc=1 with `fail: User: arn:aws:sts::<account>:assumed-role/cdk-<qualifier>-file-publishing-role-<account>-<region>/aws-cdk-mockforge is not authorized to perform: s3:ListBucket on resource: "arn:aws:s3:::cdk-<qualifier>-assets-<account>-<region>" because no identity-based policy allows the s3:ListBucket action` (emitted for both the Code and the Template asset), then `Failed to publish asset <stack> Template` / `AggregateError`.

## Steps to Reproduce
1. Replay the behavioral suite and re-run cloudformation__iam__lambda__s3.cdk-bootstrap-asset-deploy-is-content-addressed.
2. Equivalently: run `cdk bootstrap`, then `cdk deploy` on a stack with a Lambda whose code is a local asset; or sts:AssumeRole into the generated file-publishing role and call s3:ListObjectsV2 on the assets bucket.
