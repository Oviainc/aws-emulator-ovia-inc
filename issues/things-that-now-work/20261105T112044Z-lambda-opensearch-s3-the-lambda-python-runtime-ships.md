Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__opensearch__s3.cli-create-lambda-with-s3-opensearch-role-and-invoke lambda__opensearch__s3.cli-lambda-role-policy-detach-blocks-opensearch-access-negative lambda__opensearch__s3.cli-update-lambda-env-opensearch-endpoint-verify lambda__opensearch__s3.deploy-iam-role-trusts-lambda-service-positive lambda__opensearch__s3.deploy-lambda-role-missing-opensearch-policy-denied lambda__opensearch__s3.deploy-opensearch-domain-policy-restricts-lambda-denied lambda__opensearch__s3.lambda-sdk-opensearch-bulk-index-positive lambda__opensearch__s3.lambda-sdk-opensearch-delete-index-positive lambda__opensearch__s3.lambda-sdk-opensearch-domain-access-policy-denies-negative lambda__opensearch__s3.lambda-sdk-opensearch-index-document-positive lambda__opensearch__s3.lambda-sdk-opensearch-missing-permission-negative lambda__opensearch__s3.lambda-sdk-opensearch-search-query-positive lambda__opensearch__s3.lambda-sdk-opensearch-update-document-positive lambda__opensearch__s3.lambda-sdk-s3-bucket-policy-deny-negative lambda__opensearch__s3.lambda-sdk-s3-copyobject-cross-bucket-positive lambda__opensearch__s3.lambda-sdk-s3-deleteobject-positive lambda__opensearch__s3.lambda-sdk-s3-headobject-positive lambda__opensearch__s3.lambda-sdk-s3-listobjects-positive lambda__opensearch__s3.lambda-sdk-s3-missing-listbucket-permission-negative lambda__opensearch__s3.lambda-sdk-s3-presigned-url-generation-positive
Issue-Key: lambda__opensearch__s3.cli-create-lambda-with-s3-opensearch-role-and-invoke|run#13|AssertionError|assert "FunctionError" not in meta, f"unexpected FunctionError: {payload}"
Issue-Key: lambda__opensearch__s3.cli-lambda-role-policy-detach-blocks-opensearch-access-negative|run#18|AssertionError|assert meta1.get("StatusCode") == 200 and "FunctionError" not in meta1, f"unexpected invoke error: meta={meta1}"
Issue-Key: lambda__opensearch__s3.cli-update-lambda-env-opensearch-endpoint-verify|run#17|AssertionError|assert meta1.get("StatusCode") == 200 and "FunctionError" not in meta1, (
Issue-Key: lambda__opensearch__s3.deploy-iam-role-trusts-lambda-service-positive|run#12|AssertionError|assert not resp.get("FunctionError"), resp.get("FunctionError")
Issue-Key: lambda__opensearch__s3.deploy-lambda-role-missing-opensearch-policy-denied|_invoke_lambda#16|AssertionError|raise AssertionError(f"lambda invocation errored ({resp['FunctionError']}): {body}")
Issue-Key: lambda__opensearch__s3.deploy-opensearch-domain-policy-restricts-lambda-denied|_invoke_lambda#22|AssertionError|raise AssertionError(
Issue-Key: lambda__opensearch__s3.lambda-sdk-opensearch-bulk-index-positive|_invoke#15|AssertionError|raise AssertionError(f"lambda invocation failed: {raw!r}")
Issue-Key: lambda__opensearch__s3.lambda-sdk-opensearch-delete-index-positive|_invoke#11|AssertionError|raise AssertionError(f"lambda invocation failed: {raw!r}")
Issue-Key: lambda__opensearch__s3.lambda-sdk-opensearch-domain-access-policy-denies-negative|_invoke#11|AssertionError|raise AssertionError(f"lambda invocation failed: {raw!r}")
Issue-Key: lambda__opensearch__s3.lambda-sdk-opensearch-index-document-positive|_invoke#12|AssertionError|raise AssertionError(f"lambda invocation failed: {raw!r}")
Issue-Key: lambda__opensearch__s3.lambda-sdk-opensearch-missing-permission-negative|_invoke#13|AssertionError|raise AssertionError(f"lambda invocation failed: {raw!r}")
Issue-Key: lambda__opensearch__s3.lambda-sdk-opensearch-search-query-positive|_invoke#12|AssertionError|raise AssertionError(f"lambda invocation failed: {raw!r}")
Issue-Key: lambda__opensearch__s3.lambda-sdk-opensearch-update-document-positive|_invoke#11|AssertionError|raise AssertionError(f"lambda invocation failed: {raw!r}")
Issue-Key: lambda__opensearch__s3.lambda-sdk-s3-bucket-policy-deny-negative|_invoke_lambda#15|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: lambda__opensearch__s3.lambda-sdk-s3-copyobject-cross-bucket-positive|_invoke_lambda#11|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: lambda__opensearch__s3.lambda-sdk-s3-deleteobject-positive|_invoke_lambda#10|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: lambda__opensearch__s3.lambda-sdk-s3-headobject-positive|_invoke_lambda#10|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: lambda__opensearch__s3.lambda-sdk-s3-listobjects-positive|_invoke_lambda#9|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: lambda__opensearch__s3.lambda-sdk-s3-missing-listbucket-permission-negative|_invoke_lambda#7|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: lambda__opensearch__s3.lambda-sdk-s3-presigned-url-generation-positive|_invoke_lambda#10|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Commit: c1082b91874a267e16e5e882b92de2a31503a88d

# lambda__opensearch__s3: the Lambda python runtime ships no AWS SDK and a crippled stdlib, so every SDK-calling handler faults Runtime.ImportModuleError and 20 checks never observe an OpenSearch/S3 call

> _Note: this issue was identified against the code base at commit `c1082b91874a267e16e5e882b92de2a31503a88d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Two net-new root causes in the emulator's python Lambda invoke runtime, both observed in the lambda__opensearch__s3 group. 20 failing checks.

Cause 1 of 2: the python3.x invoke runtime has no boto3/botocore on the handler's import path, so a function whose module does `import boto3` to reach S3 or to sign OpenSearch requests fails at module import and its body never runs. Failing checks: lambda__opensearch__s3.cli-create-lambda-with-s3-opensearch-role-and-invoke, lambda__opensearch__s3.lambda-sdk-s3-bucket-policy-deny-negative, lambda__opensearch__s3.lambda-sdk-s3-copyobject-cross-bucket-positive, lambda__opensearch__s3.lambda-sdk-s3-deleteobject-positive, lambda__opensearch__s3.lambda-sdk-s3-headobject-positive, lambda__opensearch__s3.lambda-sdk-s3-listobjects-positive, lambda__opensearch__s3.lambda-sdk-s3-missing-listbucket-permission-negative, lambda__opensearch__s3.lambda-sdk-s3-presigned-url-generation-positive, lambda__opensearch__s3.deploy-lambda-role-missing-opensearch-policy-denied, lambda__opensearch__s3.deploy-opensearch-domain-policy-restricts-lambda-denied, and (Hypothesis: same cause — these three deploy the same shape of SDK-importing handler and the assertion only carries the bare FunctionError, with no errorMessage in the diagnosis) lambda__opensearch__s3.cli-lambda-role-policy-detach-blocks-opensearch-access-negative, lambda__opensearch__s3.cli-update-lambda-env-opensearch-endpoint-verify, lambda__opensearch__s3.deploy-iam-role-trusts-lambda-service-positive.

AWS: AWS Lambda's managed python3.x runtimes ship the AWS SDK for Python (boto3/botocore) pre-installed on the function's sys.path, so a module-scope `import boto3` succeeds and the handler runs with its execution-role credentials. Smithy (lambda, Invoke): output StatusCode is bound to httpResponseCode, FunctionError to the response header `X-Amz-Function-Error`, Payload to the HTTP payload; a successful synchronous Invoke answers HTTP 200 with NO `X-Amz-Function-Error` header and Payload equal to the handler's return value. A handler whose OpenSearch or S3 call is refused by IAM is expected to run and report that refusal in its own payload, not to fail at import.

Emulator: Invoke answers HTTP 200 with `x-amz-function-error: Unhandled` and the payload is the runtime's own import failure — errorType `Runtime.ImportModuleError`, errorMessage `Unable to import module '<module>': No module named 'boto3'`. Fixed text: `Unable to import module '` + `': No module named 'boto3'`; the variable parts are `<module>` (taken from the function's Handler setting — observed as both `lambda_function` and `index`) and the per-invocation requestId. The handler's own keys are absent, so the checks assert on a payload that is the runtime error dict.

Why: a client whose function indexes documents into OpenSearch or reads/writes S3 gets a function that cannot run at all. The permission-boundary distinctions these checks exist to observe (granted principal succeeds / detached-policy or domain-policy-denied principal is refused) become unobservable, and the emulator attributes the fault to the customer's code rather than to the absent SDK.

Fix: the Lambda invoke runtime/sandbox backing the python3.x runtimes must expose boto3/botocore on the handler's import path (services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs and the python runtime bundle it loads).

Cause 2 of 2: the same runtime's python standard library is incomplete — `os` is missing the `stat_result` attribute — so a handler module whose imports touch `os.stat_result` (anything importing `pathlib`/`shutil`-style stdlib paths, which the opensearch-py style clients do) dies at import before the handler runs. Failing checks: lambda__opensearch__s3.lambda-sdk-opensearch-bulk-index-positive, lambda__opensearch__s3.lambda-sdk-opensearch-delete-index-positive, lambda__opensearch__s3.lambda-sdk-opensearch-domain-access-policy-denies-negative, lambda__opensearch__s3.lambda-sdk-opensearch-index-document-positive, lambda__opensearch__s3.lambda-sdk-opensearch-missing-permission-negative, lambda__opensearch__s3.lambda-sdk-opensearch-search-query-positive, lambda__opensearch__s3.lambda-sdk-opensearch-update-document-positive.

AWS: the managed python3.x runtimes provide the complete CPython standard library; `os.stat_result` is a core CPython type present in every supported python runtime, so importing it never fails. Recorded behavior only for the error class: AWS answers these invocations HTTP 200 with no `X-Amz-Function-Error` and the handler's JSON report as Payload.

Emulator: Invoke answers HTTP 200 with `x-amz-function-error: Unhandled` and payload errorType `Runtime.ImportModuleError`, errorMessage `Unable to import module '<module>': module 'os' has no attribute 'stat_result'` — fixed text `Unable to import module '` + `': module 'os' has no attribute 'stat_result'`, with `<module>` from the function's Handler setting and a per-invocation requestId.

Why: every OpenSearch data-plane operation driven from a Lambda handler (index, bulk, search, update, delete) is unreachable, and so is the negative half of each pair, so a client cannot tell an authorization refusal from a runtime that cannot start.

Fix: the python runtime bundle loaded by the Lambda invoke sandbox must expose a complete CPython stdlib, `os.stat_result` included (services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs and the runtime bundle it loads).

Related open issues describe the same runtime gaps under OTHER service groupings and do not carry these checks' replay keys: 20261105T094343Z-events-lambda-s3-the-lambda-python-runtime-provides.md, 20261105T081644Z-events-lambda-s3-stepfunctions-the-lambda-python-runtime.md, 20261105T083215Z-ec2-lambda-logs-route53resolver-the-lambda-python-runtime-has-a-crippled-stdlib.md. The open lambda__opensearch__s3 issues (20261103T020000Z-lambda-opensearch-s3-invoke-execution-and-cloudformation-stack-causes.md, 20261025T121424Z-lambda-opensearch-s3-an-s3-objectcreated-notification-with.md) own other checks of this group, not these 20.

Traces: see trace_ids.

## Evidence
Expected (AWS): synchronous Invoke -> HTTP 200, no `X-Amz-Function-Error` header, Payload = the handler's own JSON report (index/search/S3 result, or the AccessDenied it caught).

Actual (emulator), cause 1 — HTTP 200 + `x-amz-function-error: Unhandled`, payload:
{"errorMessage":"Unable to import module 'lambda_function': No module named 'boto3'","errorType":"Runtime.ImportModuleError","requestId":"<per-invocation id>","stackTrace":[]}
- lambda__opensearch__s3.cli-create-lambda-with-s3-opensearch-role-and-invoke: `AssertionError: unexpected FunctionError: {'errorMessage': "Unable to import module 'lambda_function': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', ...}`
- lambda__opensearch__s3.deploy-lambda-role-missing-opensearch-policy-denied / deploy-opensearch-domain-policy-restricts-lambda-denied: `AssertionError: lambda invocation errored (Unhandled): {... No module named 'boto3' ...}` (module name `index`)
- lambda__opensearch__s3.lambda-sdk-s3-*: `AssertionError: lambda invocation errored: {... No module named 'boto3' ...}`
- lambda__opensearch__s3.cli-update-lambda-env-opensearch-endpoint-verify: `AssertionError: expected StatusCode 200 with no FunctionError, got {'StatusCode': 200, 'FunctionError': 'Unhandled', 'ExecutedVersion': '$LATEST'}`
- lambda__opensearch__s3.cli-lambda-role-policy-detach-blocks-opensearch-access-negative: `AssertionError: unexpected invoke error: meta={'StatusCode': 200, 'FunctionError': 'Unhandled', 'ExecutedVersion': '$LATEST'}`
- lambda__opensearch__s3.deploy-iam-role-trusts-lambda-service-positive: `AssertionError: Unhandled`

Actual (emulator), cause 2 — HTTP 200 + `x-amz-function-error: Unhandled`, payload:
{"errorMessage":"Unable to import module 'lambda_function': module 'os' has no attribute 'stat_result'","errorType":"Runtime.ImportModuleError","requestId":"<per-invocation id>","stackTrace":[]}
observed identically for all seven lambda-sdk-opensearch-* checks, e.g. lambda__opensearch__s3.lambda-sdk-opensearch-search-query-positive: `AssertionError: lambda invocation failed: b'{"errorMessage":"Unable to import module \'lambda_function\': module \'os\' has no attribute \'stat_result\'",...}'`.

## Steps to Reproduce
1. CreateFunction (runtime python3.x) from a zip whose handler module does `import boto3` and calls s3.get_object / signs an OpenSearch request.
2. Invoke it synchronously -> observe HTTP 200 with `x-amz-function-error: Unhandled` and a Runtime.ImportModuleError payload naming boto3; no S3 or OpenSearch call is made.
3. CreateFunction from a zip whose handler module imports a stdlib path that touches `os.stat_result` (e.g. anything importing `pathlib`/`shutil`) and Invoke it -> observe the same shape with errorMessage `module 'os' has no attribute 'stat_result'`.
4. Re-replay the 20 checks in trace_ids to confirm.
