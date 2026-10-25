Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__glue__iam__s3.deploy-gjwr-redeploy-whole-stack-to-expected-region-then-retire-the-misdeployed-one cloudformation__glue__iam__s3.deploy-gjwr-redeploy-with-region-free-role-name-rejected
Issue-Key: cloudformation__glue__iam__s3|CreateStack|AlreadyExistsException
Issue-Key: cloudformation__glue__iam__s3.deploy-gjwr-redeploy-with-region-free-role-name-rejected|run#10|AssertionError|assert clash["status"] == "ROLLBACK_COMPLETE", (
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# cloudformation__glue__iam__s3: 2 net-new root causes — stack names are a single global namespace (same name in a second region is AlreadyExists), and a region-free IAM role name collision does not fail the stack

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two distinct net-new root causes, neither marked ALREADY FILED or OWNED in the replay report. Both are namespace-scoping defects exposed by the standard 'stack was deployed to the wrong region, redeploy it to the right one' remediation.

Cause 1 of 2: CloudFormation stack names are scoped globally instead of per region.
AWS: a stack name is unique within an account AND region. The same template under the same stack name can be created in a second region while the first copy still exists, and both stacks coexist; AlreadyExistsException is only raised for a name already used in the SAME region.
Emulator: CreateStack in the expected region, with the stack of that name existing only in the other region, is refused `AlreadyExistsException: Stack [sim2real-gjwr-fix-b5cea3c4934d] already exists` — so the emulator keeps one global stack namespace.
Why: the entire redeploy-to-the-correct-region remediation is impossible, and any multi-region deployment that reuses one stack name per region (the normal pattern) fails on the second region with a misleading AlreadyExists.
Fix: key the stack registry by (account, region, stack name) in the cloudformation service crate's state, and scope CreateStack's existence check and DescribeStacks lookups to the request's region.
Traces: cloudformation__glue__iam__s3.deploy-gjwr-redeploy-whole-stack-to-expected-region-then-retire-the-misdeployed-one.

Cause 2 of 2: a stack whose AWS::IAM::Role uses a region-free (globally unique) role name already taken by the other region's stack reaches CREATE_COMPLETE.
AWS: IAM role names are global to the account, so the second stack's CreateRole is refused EntityAlreadyExists (409, modeled on CreateRole) and the stack ends ROLLBACK_COMPLETE.
Emulator: the redeploy with the region-free role name 'sim2real-gjwr-glob-7bbbd58729fa-role' in us-west-2 ended 'CREATE_COMPLETE' — the global IAM name collision is not detected (or not propagated into the stack's status).
Why: this is exactly the failure a client is told to look for when copying a stack between regions — a template whose IAM names are not region-qualified must fail loudly. Reporting success means two stacks silently believe they own one role, and the second deploy's grants are attributed to the first stack's role.
Fix: enforce the account-global uniqueness of role names in the iam crate's CreateRole handler (EntityAlreadyExists 409) and make the AWS::IAM::Role provider fail and roll back the stack on that error.
Traces: cloudformation__glue__iam__s3.deploy-gjwr-redeploy-with-region-free-role-name-rejected.

## Evidence
Cause 1: botocore.errorfactory.AlreadyExistsException: An error occurred (AlreadyExistsException) when calling the CreateStack operation: Stack [sim2real-gjwr-fix-b5cea3c4934d] already exists — raised for a CreateStack in the expected region while that stack name existed only in the other region. Expected: 200 and two coexisting per-region stacks.
Cause 2: AssertionError: redeploying with the region-free role name 'sim2real-gjwr-glob-7bbbd58729fa-role' in us-west-2 ended 'CREATE_COMPLETE', expected ROLLBACK_COMPLETE (the second CreateRole should be EntityAlreadyExists / 409).

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. Cause 1: CreateStack with name N in region A, wait for CREATE_COMPLETE, then CreateStack with the same name N and template in region B — observe AlreadyExistsException instead of a second stack.
3. Cause 2: deploy a template whose AWS::IAM::Role has a fixed (not region-derived) RoleName in region A, then deploy the same template in region B — observe CREATE_COMPLETE instead of a rollback caused by EntityAlreadyExists.
4. Re-replay the two check ids in trace_ids to confirm.
