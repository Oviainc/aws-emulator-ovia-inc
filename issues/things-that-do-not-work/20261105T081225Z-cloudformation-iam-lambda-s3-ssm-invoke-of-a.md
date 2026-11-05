Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__s3__ssm.deploy-lambda-3-rollback-blocked-when-prior-code-object-deleted cloudformation__iam__lambda__s3__ssm.deploy-lambda-3-update-disable-rollback-keeps-applied-param-until-rollback-stack cloudformation__iam__lambda__s3__ssm.deploy-lambda-3-update-missing-code-object-rolls-back
Issue-Key: lambda|Invoke|InvalidZipFileException
Issue-Commit: bd1d039abc40ef8a9373a2b76b9b7f1b159abc82

# cloudformation__iam__lambda__s3__ssm: Invoke of a CFN-created function whose Code is an S3 object is refused 502 InvalidZipFileException — the package is never held, so the function can never run

> _Note: this issue was identified against the code base at commit `bd1d039abc40ef8a9373a2b76b9b7f1b159abc82`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
REOPENS 20261029T233239Z-lambda-synchronous-invoke-data-plane-answers-501-notimplemented.md: its fix left the call refused.

Cause 1 of 1: a RequestResponse Invoke of a Lambda function that CloudFormation created from `Code.S3Bucket`/`Code.S3Key` is refused instead of running the handler.

AWS: Smithy lambda `Invoke` (POST /2015-03-31/functions/{FunctionName}/invocations, success code 200) runs the resolved version's deployment package and answers 200 with the handler's payload plus `X-Amz-Executed-Version`. CloudFormation reads the `Code.S3Bucket`/`Code.S3Key` object once, at create (and again on update/rollback), and Lambda then OWNS those bytes: deleting the S3 object afterwards does not change what the function serves, which is exactly the invariant these checks assert. `InvalidZipFileException` is modelled on `Invoke` as errorKind=server, httpError=502, and AWS raises it only for an archive Lambda cannot unzip ("Lambda could not unzip the deployment package"), never for a package it is simply not holding.

Emulator: every `Invoke` of such a function answers 502 `InvalidZipFileException` with the emulator-authored message "The deployment package of this function is not held by this emulator, so there is nothing to run." The whole sentence is emulator wording — there is no request- or state-derived variable in it, and it does not match AWS's wording for this error class. Because the code is a modelled 5xx, botocore retries it and the call finally raises after 3 retries, so each affected check burns the retry budget before failing. The function itself exists (the stack reaches CREATE_COMPLETE and `GetFunctionConfiguration` answers), so only the stored package is missing.

Why: a client that deploys a function through CloudFormation with S3-hosted code — the normal path for anything larger than an inline ZipFile — gets a 5xx from every invocation of a function the emulator reports as created and healthy. Nothing downstream of that function can be exercised: code-object rollback semantics, CodeSha256 continuity across a failed rollback, and the `disable-rollback` scenarios all depend on the function answering.

Fix: lambda's deployment-package store for code supplied BY REFERENCE — `CreateFunction`/`UpdateFunctionCode` with `Code.S3Bucket`/`Code.S3Key` must read the referenced object and persist the bytes for the version, the way an inline `ZipFile` already is — together with the `AWS::Lambda::Function` arm of the CloudFormation provisioner that passes that reference through on create, update and rollback. Hypothesis (neither the model nor the recording states which of the two drops the bytes): the reference is accepted and discarded at create time, since the same inline-ZipFile path in the cloudformation__lambda__ssm checks does run. Secondary: `Invoke` must not report an absent package as `InvalidZipFileException`, a corrupt-archive error.

Traces: the three failing checks in trace_ids; all three fail at the first `_invoke` after the stack reaches CREATE_COMPLETE.

## Evidence
All three checks: `botocore.errorfactory.InvalidZipFileException: An error occurred (InvalidZipFileException) when calling the Invoke operation (reached max retries: 3): The deployment package of this function is not held by this emulator, so there is nothing to run.`
Expected (deploy-lambda-3-rollback-blocked-when-prior-code-object-deleted, line 14609): `out = _invoke(lam, fn_name)` / `assert out == "one"` — 200 with the handler's payload for a function created from `code/v1.zip`, whose `CodeSha256` the next assertion compares against the uploaded zip's SHA256.
Smithy (lambda): Invoke http {method POST, uri /2015-03-31/functions/{FunctionName}/invocations, code 200}; errors include InvalidZipFileException {errorKind: server, httpError: 502}.

## Steps to Reproduce
Replay the behavioral suite, group cloudformation__iam__lambda__s3__ssm. (1) Create an S3 bucket and put a valid python3.12 handler zip at `code/v1.zip`. (2) `cloudformation:CreateStack` with an `AWS::IAM::Role` + `AWS::Lambda::Function` whose `Code` is `{"S3Bucket": <bucket>, "S3Key": "code/v1.zip"}`; wait for CREATE_COMPLETE. (3) Call `lambda:Invoke` (InvocationType=RequestResponse, Payload={}) on the stack's function: the emulator answers 502 InvalidZipFileException after 3 retries instead of 200 with the handler's payload.
