Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3__s3control.xsvc-s3-3-access-point-policy-denied-lambda-role-still-reads-bucket-directly
Issue-Key: lambda__s3__s3control.xsvc-s3-3-access-point-policy-denied-lambda-role-still-reads-bucket-directly|_invoke_payload#16|AssertionError|assert not resp.get("FunctionError"), f"handler faulted: {body[:500]!r}"
Issue-Commit: c1082b91874a267e16e5e882b92de2a31503a88d

# lambda__s3__s3control: the python3.12 Invoke runtime provides no AWS SDK (boto3), so the access-point permission-chain handler dies with Runtime.ImportModuleError

> _Note: this issue was identified against the code base at commit `c1082b91874a267e16e5e882b92de2a31503a88d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new root cause, grounded in the single failing lambda__s3__s3control check.

Cause 1 of 1: the emulator's managed python3.12 Lambda runtime does not provide the AWS SDK for Python on the handler's import path, so a deployment package that does not vendor boto3 — exactly what AWS expects — never reaches its entrypoint, and the whole S3 access-point permission-chain scenario is unobservable.

AWS: AWS Lambda's managed Python runtimes (python3.12 among them) ship the AWS SDK for Python — boto3 and botocore — preinstalled and importable, so a package containing only the handler module can `import boto3` and build a client. Smithy: Invoke, POST /2015-03-31/functions/{FunctionName}/invocations, output InvocationResponse{StatusCode (httpResponseCode), FunctionError (X-Amz-Function-Error header), LogResult, Payload (httpPayload), ExecutedVersion}; an unhandled init failure is NOT a modeled error — it is reported in-band as HTTP 200 plus the FunctionError header. Recorded behavior only for the handler itself: the check zips a single `lambda_function.py` whose first statement is `import boto3`, builds `boto3.client("s3")`, and GetObjects the same key twice — once through the S3 access point ARN and once through the bucket name — returning {"<label>": {"body": ...}} per arm that served bytes and {"<label>": {"code": ..., "message": ...}} per arm that was refused. On AWS, Invoke answers 200 with no X-Amz-Function-Error and that JSON document as the Payload.

Emulator: Invoke answers 200 but sets FunctionError and returns the Python runtime's init-failure document as the Payload: {"errorMessage": "Unable to import module 'lambda_function': No module named 'boto3'", "errorType": "Runtime.ImportModuleError", "requestId": "<per-invocation uuid>", "stackTrace": []}. Message anatomy: the fixed text is `Unable to import module '<module>': No module named '<module-name>'`; `lambda_function` is the module half of the function's configured Handler (`lambda_function.lambda_handler`) and `boto3` is the module the handler asked for — both are taken from the request and the function's state, and `requestId` is a per-run value that is not part of the requirement. The errorType `Runtime.ImportModuleError` is the Python runtime's init-error class. So the archive IS executed (the in-process runtime stands up and the call is not refused), but its environment is bare: no boto3, no botocore.

Why: calling another AWS service from inside a function is the central Lambda pattern, and on the managed Python runtimes the SDK is a runtime-provided dependency nobody bundles. Against this emulator every such function fails at module import before one line of handler code runs, so a client sees FunctionError / Runtime.ImportModuleError where AWS returns the handler's result. Here it also destroys the scenario's subject: the check exists to prove that an S3 access-point policy's explicit Deny on aws:PrincipalArn beats the Lambda role's identity Allow through the access point while the bucket path still serves the object, and that deleting the AP policy restores the AP arm. Because the only observer of both arms is the in-function boto3 client, the emulator's access-point authorization behavior — and the AccessDenied code and the `User: arn:aws:sts::<account>:assumed-role/<role-name>/<function-name> is not authorized to perform: s3:GetObject on resource: "<access-point-arn>/object/<key>" with an explicit deny in a resource-based policy` message AWS words for that refusal — is never exercised at all.

This is the same runtime gap already filed for other service pairings (e.g. 20261105T074953Z-lambda-scheduler-the-python3-12-invoke-runtime-provides.md and 20261105T081727Z-lambda-sns-sqs-the-emulator-s-python-lambda.md). Neither of those carries this check's replay key, so this check is not covered by them; closing it needs the runtime-provided SDK to be importable for THIS pairing too, and the access-point authorization behavior behind it to then be checked.

Fix: the Lambda Invoke execution path's runtime environment — the lambda crate's Invoke handler/execution component that builds the sandbox for a `python3.*` function. The interpreter it starts must have boto3 and botocore importable as runtime-provided modules, in addition to whatever the deployment package contains, the way the managed runtime does. Hypothesis (neither the model nor the recording states it, but the check asserts on the effect): that runtime-provided SDK must also resolve to the emulator's own endpoint and receive the function's execution-role credentials through the standard runtime environment variables, or the import will succeed and every in-function S3/s3control call will then be directed off-box.

Traces: lambda__s3__s3control.xsvc-s3-3-access-point-policy-denied-lambda-role-still-reads-bucket-directly.

## Evidence
Expected (AWS): lambda:Invoke -> HTTP 200, no X-Amz-Function-Error header, Payload = the handler's return document, i.e. both arms resolved, e.g. {"bucket": {"body": "<object bytes>"}, "access_point": {"code": "AccessDenied", "message": "User: arn:aws:sts::<account>:assumed-role/<role-name>/<function-name> is not authorized to perform: s3:GetObject on resource: \"<access-point-arn>/object/<key>\" with an explicit deny in a resource-based policy"}}.

Actual (emulator): lambda:Invoke -> HTTP 200 with X-Amz-Function-Error set and Payload:
  {"errorMessage": "Unable to import module 'lambda_function': No module named 'boto3'",
   "errorType": "Runtime.ImportModuleError",
   "requestId": "<uuid, per invocation>",
   "stackTrace": []}

The suite surfaces this from the check's _invoke_payload helper at `assert not resp.get("FunctionError"), f"handler faulted: {body[:500]!r}"` ->
  AssertionError: handler faulted: b'{"errorMessage":"Unable to import module \'lambda_function\': No module named \'boto3\'","errorType":"Runtime.ImportModuleError","requestId":"...","stackTrace":[]}'

Message anatomy: fixed text `Unable to import module '<handler module>': No module named '<module>'`; `lambda_function` comes from the function's Handler (`lambda_function.lambda_handler`), `boto3` is the module the handler imported; requestId is a per-run value and not a requirement. Because the handler faults at init, the access-point arm never produces a verdict, so the AccessDenied code/message above was never emitted or compared.

## Steps to Reproduce
1. s3:CreateBucket a bucket and s3:PutObject a key into it.
2. s3control:CreateAccessPoint on that bucket and note the returned AccessPointArn.
3. s3:PutBucketPolicy delegating access control to the account's access points (Allow Principal "*" on the bucket and its objects, conditioned on s3:DataAccessPointAccount == the account).
4. iam:CreateRole trusting lambda.amazonaws.com with an inline policy allowing s3:GetObject on both arn:aws:s3:::<bucket>/* and <access-point-arn>/object/*.
5. lambda:CreateFunction with Runtime="python3.12", Handler="lambda_function.lambda_handler", Timeout=30, MemorySize=128 and Code.ZipFile containing only lambda_function.py, whose body starts with `import boto3`, builds `boto3.client("s3")`, and in the handler GetObjects event["key"] once per entry of event["paths"], returning the body on success and the ClientError's Code/Message on failure. The archive deliberately bundles NO boto3, as on AWS.
6. s3control:PutAccessPointPolicy on the access point with an explicit Deny on s3:GetObject for <access-point-arn>/object/*, conditioned ArnEquals aws:PrincipalArn == the function's role ARN; confirm it reads back Effect=Deny via GetAccessPointPolicy.
7. Wait for State=Active, then lambda:Invoke (InvocationType RequestResponse) with Payload {"key": "<key>", "paths": {"access_point": "<access-point-arn>", "bucket": "<bucket>"}}.
8. Observe the response: AWS returns 200 with no X-Amz-Function-Error and both arms resolved (bucket arm serves the bytes, access_point arm AccessDenied); the emulator returns 200 with X-Amz-Function-Error set and the Runtime.ImportModuleError document above, so neither arm is ever evaluated.
9. Replay the behavioral suite and run lambda__s3__s3control.xsvc-s3-3-access-point-policy-denied-lambda-role-still-reads-bucket-directly.
