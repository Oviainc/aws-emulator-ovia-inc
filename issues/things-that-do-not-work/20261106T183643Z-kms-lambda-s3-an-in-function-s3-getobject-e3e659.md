Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__lambda__s3.sse-kms-object-unreadable-when-key-policy-denies-role
Issue-Key: kms__lambda__s3.sse-kms-object-unreadable-when-key-policy-denies-role|_invoke_lambda#13|Timeout|resp = ctx.client("lambda").invoke(
Issue-Commit: 1ed505cf711c8d93e4d58056b26ffee0d457f81a

# kms__lambda__s3: an in-function S3 GetObject of an SSE-KMS object never returns the handler's report, so the key-policy-denial lifecycle hangs to the 60s cap (1 cause)

> _Note: this issue was identified against the code base at commit `1ed505cf711c8d93e4d58056b26ffee0d457f81a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
ONE cause (Cause 1 of 1). This is the only failing check in this run that the suite report marked neither "ALREADY FILED" nor "OWNED by"; every other failure in the report is already covered.

Cause 1 of 1: a python Lambda handler that builds an S3 client and reads an SSE-KMS object never produces the payload AWS would return, so the check's first poll — which waits for BOTH the denied-key object and the control object to come back with a body before anything is broken — can never be satisfied and the run is killed at the harness's 60s cap. None of the scenario the check exists for (an SSE-KMS read refused once the CMK's key policy Denies the reading role, and readable again when that Deny is removed) is ever exercised. Failing check id for this cause (1): kms__lambda__s3.sse-kms-object-unreadable-when-key-policy-denies-role.

AWS: Smithy (lambda Invoke -> InvocationResponse; http {method: POST, uri: /2015-03-31/functions/{FunctionName}/invocations, code: 200}): output StatusCode is @httpResponseCode, FunctionError is the header X-Amz-Function-Error, Payload is the @httpPayload blob. A successful RequestResponse invocation therefore answers HTTP 200 with X-Amz-Function-Error ABSENT and Payload equal to exactly the JSON the handler returned. The contents of the managed runtime are recorded behavior only (the model describes the Invoke wire shape, not the execution environment): AWS's managed python3.x runtimes ship a current boto3/botocore whose data directory carries every public service model, so `boto3.client("s3")` at handler module scope always constructs and GetObject is dispatched with the execution role's credentials. The handler consequently reports S3's own outcome: the object's bytes while the role may decrypt kms, and — once the key policy carries an explicit Deny of kms:Decrypt for that role — an access-denied class refusal with NO body, Error.Code AccessDenied at HTTP 403 (the S3-relayed KMS form KMS.AccessDeniedException is the other accepted code). In that message class the fixed text is S3's own ('Access Denied', or the relayed KMS denial naming the principal, the action kms:Decrypt and the key), and its variable parts — the assumed-role session ARN arn:aws:sts::<account>:assumed-role/<execution-role-name>/<function-name>, the action kms:Decrypt and the key ARN — come from the request and from the key policy in state; no literal id, name or timestamp from any single run is part of the requirement. The object itself stays in place (HeadObject still reports SSEKMSKeyId = the denied key's ARN), and restoring the original key policy makes the identical GetObject return the exact original bytes again.

Emulator: the check is reported as 'Check timed out after 60.0s'. Its first `_poll_until(lambda: _invoke_payload(ctx, fn["name"], event), lambda r: "body" in r.get("denied.txt", {}) and "body" in r.get("control.txt", {}), timeout_s=120.0, interval_s=5.0)` never sees a body for either object, so no assertion in the check body is ever reached and neither the denial nor the restore is graded. `_invoke_payload` asserts StatusCode 200 and no FunctionError, so the invocation is being served — it just never carries the handler's report of the S3 read. Hypothesis: the sandbox python runtime's botocore data catalog is incomplete, so `s3 = boto3.client("s3")` cannot construct (botocore UnknownServiceError, "Unknown service: '<service-name>'. Valid service names are: <the models the runtime image contains>" — fixed text as quoted, variable parts being the requested service id and the installed list) and no body can ever be produced. Corroboration from this same suite run, not invented here: kms__lambda.kms-grant-authorizes-lambda-decrypt-then-revoke-denies returns {'denied': True, 'code': 'UnknownServiceError'} from the identical handler pattern with boto3.client("kms"). Whichever refusal occurs inside the sandbox, the emulator serves this Invoke and does not serve the handler's report.

Why: a client cannot verify from application code the single most important S3+KMS control — that an SSE-KMS object becomes unreadable to a principal the CMK's key policy denies, and readable again when it does not. Worse, the scenario cannot even fail informatively: it hangs, so an operator sees a stuck deployment rather than either a KMS denial or a runtime defect, and anyone shipping a function that reads SSE-KMS objects against this emulator gets no signal at all.

Fix: the Lambda service's execution-environment component — the part of the Invoke path that assembles the python runtime filesystem for the sandbox and puts boto3/botocore on the handler's import path (services/service-com-amazonaws-lambda-awsgirapiservice, Invoke handler and the python runtime bundle it loads) — must provide a botocore whose data directory carries the full set of service models, so an in-function S3 client constructs and the GetObject actually reaches S3. Behind it, the SSE-KMS authorization on S3's GetObject path must relay the KMS decrypt decision for the caller's role (an access-denied class refusal with no body when the key policy denies kms:Decrypt). The Invoke response wire shape itself is correct.

Related open issues that the suite report did NOT mark as owning this check (named for context only, they do not cover it): 20261106T143106Z-kms-lambda-s3-an-in-function-s3-getobject.md (same check id under the same timeout symptom, but its recorded replay key names a _poll_until frame the current run no longer reports); 20261105T112044Z-kms-lambda-s3-the-lambda-python-runtime-provides.md (the earlier symptom of the same runtime gap: Runtime.ImportModuleError, "No module named 'boto3'" — the failure has since moved past module import to a hang with no handler report); 20261105T111951Z-kms-lambda-s3-an-sse-kms-object-stays.md (the enforcement half on GetObject); 20261029T175759Z-kms-lambda-s3-headobject-reports-ssekmskeyid-as-a.md (the other check of this group).

Traces: kms__lambda__s3.sse-kms-object-unreadable-when-key-policy-denies-role (see trace_ids).

## Evidence
Expected (AWS): lambda Invoke(FunctionName=<fn>, InvocationType=RequestResponse, Payload={"keys": ["denied.txt", "control.txt"]}) of a python3.12 function whose handler does `import boto3`, `s3 = boto3.client("s3")` and s3.get_object(Bucket=os.environ["BUCKET"], Key=<key>) -> HTTP 200, X-Amz-Function-Error absent, Payload = exactly the handler's JSON: {"denied.txt": {"body": "<the object's exact bytes>"}, "control.txt": {"body": "<...>"}} before anything is broken. After PutKeyPolicy adds an explicit Deny of kms:Decrypt for the function's role on the denied key: {"denied.txt": {"code": "AccessDenied"}} (or "KMS.AccessDeniedException"), HTTP 403 from S3, with NO 'body' member, while control.txt still returns its body; HeadObject on denied.txt still reports SSEKMSKeyId = the denied key's ARN; after restoring the original key policy the identical read returns the exact original bytes again.

Actual (emulator): the check result is 'Check timed out after 60.0s'. The first poll (timeout_s=120.0, interval_s=5.0, description="both SSE-KMS objects readable by the Lambda role before the key-policy change") never observes 'body' for either key, so the run is killed before any assertion executes — the key-policy Deny is never applied and the restore is never attempted. Invoke itself answers 200 with no FunctionError (otherwise _invoke_payload would have asserted immediately), so the emulator serves the invocation without serving the handler's report. Corroborating from the same run: the identical handler pattern against KMS in kms__lambda.kms-grant-authorizes-lambda-decrypt-then-revoke-denies returned {'denied': True, 'code': 'UnknownServiceError'}, the botocore failure for a service missing from the sandbox's data catalog. Generated bucket, object, key, role and function names, request ids and timestamps vary per run and are not requirements.

## Steps to Reproduce
1. kms CreateKey twice (a 'denied' key and a 'control' key).
2. s3 CreateBucket (region-correct) and PutObject 'denied.txt' with ServerSideEncryption=aws:kms, SSEKMSKeyId=<denied-key-arn>, plus 'control.txt' under the control key.
3. iam CreateRole trusting lambda.amazonaws.com with an inline policy allowing logs:CreateLogGroup/CreateLogStream/PutLogEvents, s3:GetObject on arn:aws:s3:::<bucket>/* and kms:Decrypt on both key ARNs.
4. lambda CreateFunction Runtime=python3.12, Handler=lambda_function.lambda_handler, Environment.Variables.BUCKET=<bucket>, Code.ZipFile = a zip whose lambda_function.py is:
       import os, boto3
       s3 = boto3.client("s3")
       def lambda_handler(event, context):
           out = {}
           for k in event["keys"]:
               try:
                   out[k] = {"body": s3.get_object(Bucket=os.environ["BUCKET"], Key=k)["Body"].read().decode()}
               except Exception as exc:
                   err = getattr(exc, "response", None) or {}
                   out[k] = {"code": err.get("Error", {}).get("Code", type(exc).__name__)}
           return out
   then wait for State == Active.
5. lambda Invoke(InvocationType=RequestResponse, Payload={"keys": ["denied.txt", "control.txt"]}), polled every 5s: no invocation ever returns a 'body' for either object (on AWS the first invocation already returns both bodies), so the check hangs and is killed at the 60s cap.
6. (The intended remainder, unreachable today: kms PutKeyPolicy with an explicit Deny of kms:Decrypt for the function's role on the denied key, Invoke -> code AccessDenied / KMS.AccessDeniedException and no body for denied.txt while control.txt still reads; HeadObject denied.txt still reporting the denied key's ARN; restore the original policy -> the identical read returns the original bytes.)
7. Re-replay kms__lambda__s3.sse-kms-object-unreadable-when-key-policy-denies-role to confirm.
