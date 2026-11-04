Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__s3.stack-lambda-code-s3key-update-serves-new-code-and-missing-key-rolls-back
Issue-Key: cloudformation__lambda__s3.stack-lambda-code-s3key-update-serves-new-code-and-missing-key-rolls-back|_expect_code#16|KeyError|sha = lam.get_function(FunctionName=function)["Configuration"]["CodeSha256"]
Issue-Commit: 6ff397d47b7f23b010f8590f84afedb6b986e178

# cloudformation__lambda__s3: GetFunction omits CodeSha256 for a stack-deployed function whose Code is an S3 object

> _Note: this issue was identified against the code base at commit `6ff397d47b7f23b010f8590f84afedb6b986e178`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFunction

## Description
Cause 1 of 1: the FunctionConfiguration of an S3-sourced, CloudFormation-deployed function carries no CodeSha256.
AWS: Smithy: lambda#FunctionConfiguration member CodeSha256 — "The SHA256 hash of the function's deployment package." It is populated on every function GetFunction can read, whatever the code source; for an AWS::Lambda::Function whose Code is {S3Bucket, S3Key} it is the base64 SHA-256 of the zip stored at that key, and it changes when the stack update points S3Key at different bytes.
Emulator: GetFunction(FunctionName=<stack function>)["Configuration"] has no CodeSha256 key at all — the check raises KeyError: 'CodeSha256' on the first read, before it can compare the digest across the S3Key update.
Why: CodeSha256 is the only way a client can tell which bytes a function is actually running. Without it, a CloudFormation S3Key update cannot be verified to have taken effect, deployment-drift detection and content-addressed deploys (CDK asset hashes) have nothing to compare, and any caller that reads the member gets a KeyError rather than a value.
Fix: the Lambda GetFunction/GetFunctionConfiguration serializer and the function state it reads — the S3-sourced create/update path must record the deployment package's digest (and size) the same way the ZipFile path does.
The open issue 20261029T211043Z-lambda-s3-s3-sourced-lambda-code-codesha256-codesize-are-absent.md reports the same missing members under the lambda__s3 service and does not carry this check's replay key; this one is the CloudFormation-deployed path of the same surface and should be closed with it.
Traces: cloudformation__lambda__s3.stack-lambda-code-s3key-update-serves-new-code-and-missing-key-rolls-back.

## Evidence
Expected: GetFunction(FunctionName=<fn>)["Configuration"]["CodeSha256"] is the base64 SHA-256 of the zip at the stack's Code.S3Key, and it changes when the stack update repoints S3Key.
Actual: KeyError: 'CodeSha256' — the member is absent from the Configuration map returned by GetFunction.
Raised at check_cloudformation__lambda__s3_stack_lambda_code_s3key_update_serves_new_code_and_missing_key_rolls_back.py line 118, `sha = lam.get_function(FunctionName=function)["Configuration"]["CodeSha256"]`.

## Steps to Reproduce
Replay cloudformation__lambda__s3.stack-lambda-code-s3key-update-serves-new-code-and-missing-key-rolls-back: upload two distinct handler zips to an S3 bucket, CreateStack with an AWS::Lambda::Function whose Code is {S3Bucket, S3Key: key-one}, wait for CREATE_COMPLETE, then call lambda GetFunction on the stack's function and read Configuration.CodeSha256.
