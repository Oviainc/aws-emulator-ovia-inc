Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__lambda__s3.sse-kms-object-unreadable-when-key-policy-denies-role
Issue-Key: kms__lambda__s3.sse-kms-object-unreadable-when-key-policy-denies-role|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Commit: 5678b2d659e23eeff4b2f39316abc675f4291395

# kms__lambda__s3: an in-function S3 GetObject of an SSE-KMS object never returns the handler's own report, so the key-policy-denial lifecycle is killed at the 60s cap

> _Note: this issue was identified against the code base at commit `5678b2d659e23eeff4b2f39316abc675f4291395`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: a python Lambda handler that constructs an S3 client and reads an SSE-KMS object never produces the payload AWS would return, so the check's first poll — which waits for both the denied-key object and the control object to come back with a body — can never be satisfied and the run is killed at the harness's 60s cap. Nothing of the scenario (an SSE-KMS read refused once the CMK's key policy Denies the reading role, and readable again when the Deny is removed) is ever exercised. Failing check id for this cause (1): kms__lambda__s3.sse-kms-object-unreadable-when-key-policy-denies-role.

AWS: recorded behavior only for the contents of the managed runtime (the Smithy model describes the Invoke wire shape, not the execution environment). AWS's managed python3.x runtimes ship a current boto3/botocore whose data directory holds every public AWS service model, so `boto3.client("s3")` inside a handler always constructs and GetObject is dispatched with the execution role's credentials. Smithy (lambda Invoke -> InvocationResponse, http {method: POST, uri: /2015-03-31/functions/{FunctionName}/invocations, code: 200}): StatusCode is httpResponseCode, FunctionError is the header X-Amz-Function-Error, Payload is the httpPayload blob; a successful RequestResponse invocation answers HTTP 200 with X-Amz-Function-Error ABSENT and Payload equal to exactly the JSON the handler returned. On AWS the handler therefore reports S3's own outcome: the object's bytes while the role may decrypt, and — once the key policy carries an explicit Deny of kms:Decrypt for that role — an access-denied class refusal with NO body, Error.Code `AccessDenied` (HTTP 403; the S3-relayed KMS form `KMS.AccessDeniedException` is the other accepted code). Fixed text of that class of message is S3's own ('Access Denied', or the relayed KMS denial naming the principal, the action kms:Decrypt and the key ARN); its variable parts — the assumed-role session ARN arn:aws:sts::<account>:assumed-role/<execution-role-name>/<function-name>, the action, and the key ARN — come from the request and from the key policy in state. The object itself stays in place: HeadObject still reports SSEKMSKeyId = the denied key's ARN, and restoring the original key policy makes the identical GetObject return the exact original bytes again.

Emulator: the check is reported as 'Check timed out after 60.0s'. Its first `_poll_until(..., timeout_s=120.0, interval_s=5.0)` requires 'body' in the handler's result for BOTH object keys before anything is broken, and that never happens within the harness's 60s cap, so no assertion in the check body is ever reached. Hypothesis: the same cause already observed in the sibling group under this exact runtime — the SDK installed in the sandbox carries a truncated botocore service-model data directory, so `s3 = boto3.client("s3")` at module scope raises botocore.exceptions.UnknownServiceError (`Unknown service: '<service-name>'. Valid service names are: <the models the runtime image contains>`; fixed text as quoted, variable parts being the requested service id and the installed list) and the invocation can never yield a body. Evidence for that reading, not invented here: in the same suite run kms__lambda.kms-grant-authorizes-lambda-decrypt-then-revoke-denies returned {'denied': True, 'code': 'UnknownServiceError'} from the identical handler pattern with boto3.client("kms"), and the already-filed 20261106T131020Z-bedrock-agent-runtime-lambda-s3vectors-the-lambda-python.md recorded the installed list as only sqs, events, eventbridge, dynamodb. Whatever the precise refusal inside the sandbox, the emulator serves this invocation, and it does not serve the handler's report.

Why: a client cannot verify the single most important S3+KMS control from application code — that an SSE-KMS object becomes unreadable to a principal the key policy denies and readable again when it does not. The scenario cannot even fail informatively: it hangs, so an operator sees a stuck deployment rather than either the KMS denial or a runtime defect, and an emulator user who ships a function that reads SSE-KMS objects gets no signal at all.

Fix: the Lambda service's execution-environment component — the part that assembles the python runtime filesystem for the sandbox and puts boto3/botocore on the handler's import path (the Invoke path of services/service-com-amazonaws-lambda-awsgirapiservice and the python runtime bundle it loads) — must install a botocore whose data directory carries the full set of service models, so an in-function S3 client constructs and the GetObject actually reaches S3. Behind it, S3's GetObject must relay the KMS decrypt decision for the caller's role (an access-denied class refusal with no body when the key policy denies kms:Decrypt), which is the enforcement side described by the related open issue below. The Invoke response wire shape itself is correct.

Related open work that does NOT cover this check (the suite marked it neither owned nor already filed): 20261105T112044Z-kms-lambda-s3-the-lambda-python-runtime-provides.md carries this check id under the EARLIER symptom of the same runtime (Runtime.ImportModuleError, "No module named 'boto3'"), so adding boto3 alone did not close it — the failure has moved past module import to a hang with no handler report; 20261105T111951Z-kms-lambda-s3-an-sse-kms-object-stays.md describes the enforcement half; 20261029T175759Z-kms-lambda-s3-headobject-reports-ssekmskeyid-as-a.md owns the other check of this group.

Traces: the 1 failing check id above; see trace_ids.

## Evidence
Expected (AWS): lambda Invoke(FunctionName=<fn>, InvocationType=RequestResponse, Payload={"keys": ["denied.txt", "control.txt"]}) of a python3.12 function whose handler does `import boto3`, `s3 = boto3.client("s3")` and s3.get_object(Bucket=os.environ["BUCKET"], Key=<key>) -> HTTP 200, no X-Amz-Function-Error, and Payload = the handler's own JSON: {"denied.txt": {"body": "<the exact object bytes>"}, "control.txt": {"body": "..."}} before anything is broken; after PutKeyPolicy adds an explicit Deny of kms:Decrypt for the function's role on the denied key, {"denied.txt": {"code": "AccessDenied"}} (or "KMS.AccessDeniedException") with NO 'body' member while control.txt still returns its body; HeadObject on denied.txt still reporting SSEKMSKeyId = the denied key ARN; and, once the original key policy is restored, the identical read returning the exact original bytes again.

Actual (emulator): 'Check timed out after 60.0s'. The check's first poll (`_poll_until(lambda: _invoke_payload(ctx, fn["name"], event), lambda r: "body" in r.get("denied.txt", {}) and "body" in r.get("control.txt", {}), timeout_s=120.0, interval_s=5.0)`) never sees a body for either object, so the run is killed before any assertion executes and neither the denial nor the restore is graded. Corroborating evidence for the cause from the same suite run: the identical handler pattern against KMS in kms__lambda.kms-grant-authorizes-lambda-decrypt-then-revoke-denies returned {'denied': True, 'code': 'UnknownServiceError'} — botocore cannot construct a client for a service missing from the sandbox's data catalog, whose message is "Unknown service: '<service-name>'. Valid service names are: <installed model list>". Generated bucket, object, key, role and function names, request ids and timestamps vary per run and are not requirements.

## Steps to Reproduce
1. kms CreateKey twice (a 'denied' key and a 'control' key).
2. s3 CreateBucket (region-correct) and PutObject 'denied.txt' with ServerSideEncryption=aws:kms, SSEKMSKeyId=<denied-key-arn>, plus 'control.txt' under the control key.
3. iam CreateRole trusting lambda.amazonaws.com with an inline policy allowing logs:*, s3:GetObject on arn:aws:s3:::<bucket>/* and kms:Decrypt on both key ARNs.
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
   wait for State == Active.
5. lambda Invoke(InvocationType=RequestResponse, Payload={"keys": ["denied.txt", "control.txt"]}) and poll it every 5s: no invocation ever returns a 'body' for either object, so the scenario never reaches the step that adds the key-policy Deny; on AWS the first invocation already returns both bodies.
6. (The intended remainder, unreachable today: PutKeyPolicy an explicit Deny of kms:Decrypt for the function's role on the denied key, Invoke -> code AccessDenied / KMS.AccessDeniedException and no body for denied.txt while control.txt still reads; restore the policy -> the identical read returns the original bytes.)
7. Re-replay kms__lambda__s3.sse-kms-object-unreadable-when-key-policy-denies-role to confirm.
