Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3__s3control.xsvc-s3-3-access-point-policy-denied-lambda-role-still-reads-bucket-directly
Issue-Key: lambda|Invoke|NotImplemented
Issue-Commit: bd3308b0c8d992d57414e8fe0564c68280e702c4

# Lambda Invoke refuses every real invocation with NotImplemented instead of running the function and returning its payload

> _Note: this issue was identified against the code base at commit `bd3308b0c8d992d57414e8fe0564c68280e702c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Attribution note: the grounding check is cross-service and its id's leading token is the composite "lambda__s3__s3control", which matches no single service token. The diverging operation is Lambda's Invoke, so this issue is filed under service 'lambda'; lambda, s3 and s3control are all bootstrapped, so the per-service gate is FILE under any of the three.

Cause 1 of 1: a synchronous Invoke of a deployed function is refused rather than executed.
AWS: Smithy Invoke — POST /2015-03-31/functions/{FunctionName}/invocations, success code 200; output InvocationResponse carries StatusCode (httpResponseCode), Payload (httpPayload blob = whatever the function returned), ExecutedVersion (X-Amz-Executed-Version, e.g. $LATEST), and FunctionError (X-Amz-Function-Error) only when the function itself failed. The modeled error set is large (InvalidRequestContentException 400, ResourceNotFoundException 404, RequestTooLargeException 413, TooManyRequestsException 429, ServiceException 500, the 502 runtime/ENI/KMS family, …) and contains no NotImplemented: a function that exists and is Active is invoked, and the caller gets its payload back.
Emulator: the Invoke returns error code NotImplemented, stating that the function arn:aws:lambda:<region>:<account>:function:<name> at version $LATEST resolved, but that running its python3.12 deployment package is not implemented — there is no in-process runtime and no payload is returned. So the control-plane path (CreateFunction, role, resolution to $LATEST) works and only the execution is absent.
Why: every client whose flow depends on the function's RESULT breaks: the Payload a caller parses never exists, StatusCode 200 never arrives, and FunctionError can never be distinguished from a service refusal. Downstream, nothing a function would have done is observable — any cross-service behavior that runs inside a Lambda's execution-role credentials (here, S3 reads performed by the function through an access point ARN versus the bucket name) is unreachable, so authorization behavior exercised only from inside a function body cannot be validated at all.
Fix: the Invoke handler's execution path — an in-process (or sandboxed) runtime for the function's deployment package, or at minimum a faithful 200 + Payload path for invoked functions — plus the error mapping, which should never answer NotImplemented for a function that resolved. Note what already exists and should be preserved: InvocationType=DryRun validates an invocation without running the function.
Traces: lambda__s3__s3control.xsvc-s3-3-access-point-policy-denied-lambda-role-still-reads-bucket-directly.

## Evidence
Suite: simparity-behavioral-aws-v1 — lambda__s3__s3control 0/1 passed.

botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the Invoke operation: Invoke resolved arn:aws:lambda:us-west-2:000000000000:function:sim2real-xsvc-fn-4a25d7b6f44c at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator: there is no in-process runtime for it, and no response payload is returned that the function did not produce. InvocationType=DryRun validates an invocation without running the function.

Expected (Smithy Invoke / InvocationResponse): HTTP 200 with StatusCode 200 and a Payload blob holding the handler's return value — in this check, a JSON object mapping each label to either the object body read through the given S3 path or the error code/message that path produced. No NotImplemented error is modeled for Invoke.

## Steps to Reproduce
1. CreateFunction with a python3.12 zip deployment package (handler returns a JSON object), wait for it to be Active.
2. Call Invoke(FunctionName=<name>, Payload=<json>) with the default InvocationType=RequestResponse.
3. Expect HTTP 200, StatusCode 200, ExecutedVersion $LATEST and the handler's JSON in Payload; the emulator instead returns error code NotImplemented.
4. Full grounding: replay lambda__s3__s3control.xsvc-s3-3-access-point-policy-denied-lambda-role-still-reads-bucket-directly, where the function reads one key twice (once via an access point ARN, once via the bucket name) and the two outcomes are compared.
