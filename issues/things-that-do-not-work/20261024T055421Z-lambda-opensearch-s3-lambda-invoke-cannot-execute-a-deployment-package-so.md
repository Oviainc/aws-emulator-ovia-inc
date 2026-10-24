Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__opensearch__s3.cli-create-lambda-with-s3-opensearch-role-and-invoke
Issue-Key: lambda__opensearch__s3.cli-create-lambda-with-s3-opensearch-role-and-invoke|_run_shell#11|AssertionError|raise AssertionError(
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# Lambda Invoke cannot execute a deployment package, so function-driven cross-service workflows return NotImplemented

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One root cause.

Cause 1 of 1: Invoke on a function with a python3.12 zip deployment package returns NotImplemented instead of running the handler and returning its payload.
AWS: Invoke runs the function's deployment package under the configured runtime and returns HTTP 200 with StatusCode=200, the handler's JSON payload in the response body, and (for a handler error) FunctionError plus the error payload. Smithy: lambda#Invoke -> InvocationResponse{StatusCode, FunctionError, LogResult, Payload, ExecutedVersion}; its modeled errors are request/state errors (ResourceNotFoundException, InvalidRequestContentException, TooManyRequestsException, ...) — NotImplemented is not among them and no 501 is part of the contract. Side effects the handler performs with its execution role (here S3 PutObject and opensearch ListDomainNames) are observable after the call.
Emulator: `aws lambda invoke --function-name <fn>` exits rc=254 with error code NotImplemented and the message 'Invoke resolved arn:aws:lambda:us-west-2:...:function:<fn> at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator: there is no in-process runtime for it, and no response payload is returned that the function did not produce.'
Why: Lambda is the glue in most event-driven estates. A client that invokes a function and reads its payload, or that relies on the function's own AWS calls to have happened (an object written to S3, a domain listed), gets an error code that is not in the operation's contract and no side effects at all. Every workflow with a function in the middle is untestable.
Fix: the Lambda Invoke handler and its execution path — provide a runtime that executes the stored deployment package (at minimum the declared python runtime), return the handler's Payload/StatusCode/FunctionError, and let the function's own SDK calls reach the emulator under the function's execution role.
Traces: lambda__opensearch__s3.cli-create-lambda-with-s3-opensearch-role-and-invoke.

## Evidence
Expected: Invoke -> HTTP 200, StatusCode=200, Payload = the handler's JSON result; and the handler's own side effects (S3 PutObject of 'hello-from-lambda', opensearch ListDomainNames) visible afterwards.
Actual: AssertionError: Command failed (rc=254):
cmd: ['aws', '--output', 'json', 'lambda', 'invoke', '--function-name', 'sim2real-fn-...', '--cli-binary-format', 'raw-in-base64-out', '--payload', 'file:///tmp/.../payload.json', '/tmp/.../out.json']
stderr: aws: [ERROR]: An error occurred (NotImplemented) when calling the Invoke operation: Invoke resolved arn:aws:lambda:us-west-2:000000000000:function:sim2real-fn-... at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator

## Steps to Reproduce
1. Create an IAM execution role with S3 + OpenSearch permissions.
2. lambda.create_function(Runtime='python3.12', Handler='index.handler', Code={'ZipFile': <zip of the handler>}, Role=<role arn>, Environment with TARGET_BUCKET).
3. lambda.invoke(FunctionName=<fn>, Payload=b'{}').
4. Observe error code NotImplemented; no object appears in the target bucket.
5. Replay lambda__opensearch__s3.cli-create-lambda-with-s3-opensearch-role-and-invoke.
