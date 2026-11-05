Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__opensearch__s3.cli-create-lambda-with-s3-opensearch-role-and-invoke lambda__opensearch__s3.cli-lambda-role-policy-detach-blocks-opensearch-access-negative lambda__opensearch__s3.cli-update-lambda-env-opensearch-endpoint-verify lambda__opensearch__s3.deploy-iam-role-trusts-lambda-service-positive lambda__opensearch__s3.deploy-lambda-role-missing-opensearch-policy-denied lambda__opensearch__s3.deploy-opensearch-domain-policy-restricts-lambda-denied lambda__opensearch__s3.lambda-sdk-s3-bucket-policy-deny-negative lambda__opensearch__s3.lambda-sdk-s3-copyobject-cross-bucket-positive lambda__opensearch__s3.lambda-sdk-s3-deleteobject-positive lambda__opensearch__s3.lambda-sdk-s3-headobject-positive lambda__opensearch__s3.lambda-sdk-s3-listobjects-positive lambda__opensearch__s3.lambda-sdk-s3-missing-listbucket-permission-negative lambda__opensearch__s3.lambda-sdk-s3-presigned-url-generation-positive lambda__opensearch__s3.lambda-sdk-opensearch-bulk-index-positive lambda__opensearch__s3.lambda-sdk-opensearch-delete-index-positive lambda__opensearch__s3.lambda-sdk-opensearch-domain-access-policy-denies-negative lambda__opensearch__s3.lambda-sdk-opensearch-index-document-positive lambda__opensearch__s3.lambda-sdk-opensearch-missing-permission-negative lambda__opensearch__s3.lambda-sdk-opensearch-search-query-positive lambda__opensearch__s3.lambda-sdk-opensearch-update-document-positive
Issue-Key: lambda__opensearch__s3.cli-create-lambda-with-s3-opensearch-role-and-invoke|run#13|AssertionError|assert "FunctionError" not in meta, f"unexpected FunctionError: {payload}"
Issue-Key: lambda__opensearch__s3.cli-lambda-role-policy-detach-blocks-opensearch-access-negative|run#18|AssertionError|assert meta1.get("StatusCode") == 200 and "FunctionError" not in meta1, f"unexpected invoke error: meta={meta1}"
Issue-Key: lambda__opensearch__s3.cli-update-lambda-env-opensearch-endpoint-verify|run#17|AssertionError|assert meta1.get("StatusCode") == 200 and "FunctionError" not in meta1, (
Issue-Key: lambda__opensearch__s3.deploy-iam-role-trusts-lambda-service-positive|run#12|AssertionError|assert not resp.get("FunctionError"), resp.get("FunctionError")
Issue-Key: lambda__opensearch__s3.deploy-lambda-role-missing-opensearch-policy-denied|_invoke_lambda#16|AssertionError|raise AssertionError(f"lambda invocation errored ({resp['FunctionError']}): {body}")
Issue-Key: lambda__opensearch__s3.deploy-opensearch-domain-policy-restricts-lambda-denied|_invoke_lambda#22|AssertionError|raise AssertionError(
Issue-Key: lambda__opensearch__s3.lambda-sdk-s3-bucket-policy-deny-negative|_invoke_lambda#15|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: lambda__opensearch__s3.lambda-sdk-s3-copyobject-cross-bucket-positive|_invoke_lambda#11|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: lambda__opensearch__s3.lambda-sdk-s3-deleteobject-positive|_invoke_lambda#10|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: lambda__opensearch__s3.lambda-sdk-s3-headobject-positive|_invoke_lambda#10|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: lambda__opensearch__s3.lambda-sdk-s3-listobjects-positive|_invoke_lambda#9|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: lambda__opensearch__s3.lambda-sdk-s3-missing-listbucket-permission-negative|_invoke_lambda#7|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: lambda__opensearch__s3.lambda-sdk-s3-presigned-url-generation-positive|_invoke_lambda#10|AssertionError|raise AssertionError(f"lambda invocation errored: {body}")
Issue-Key: lambda__opensearch__s3.lambda-sdk-opensearch-bulk-index-positive|_invoke#15|AssertionError|raise AssertionError(f"lambda invocation failed: {raw!r}")
Issue-Key: lambda__opensearch__s3.lambda-sdk-opensearch-delete-index-positive|_invoke#11|AssertionError|raise AssertionError(f"lambda invocation failed: {raw!r}")
Issue-Key: lambda__opensearch__s3.lambda-sdk-opensearch-domain-access-policy-denies-negative|_invoke#11|AssertionError|raise AssertionError(f"lambda invocation failed: {raw!r}")
Issue-Key: lambda__opensearch__s3.lambda-sdk-opensearch-index-document-positive|_invoke#12|AssertionError|raise AssertionError(f"lambda invocation failed: {raw!r}")
Issue-Key: lambda__opensearch__s3.lambda-sdk-opensearch-missing-permission-negative|_invoke#13|AssertionError|raise AssertionError(f"lambda invocation failed: {raw!r}")
Issue-Key: lambda__opensearch__s3.lambda-sdk-opensearch-search-query-positive|_invoke#12|AssertionError|raise AssertionError(f"lambda invocation failed: {raw!r}")
Issue-Key: lambda__opensearch__s3.lambda-sdk-opensearch-update-document-positive|_invoke#11|AssertionError|raise AssertionError(f"lambda invocation failed: {raw!r}")
Issue-Commit: a0ca0e6d405fa8f821a988e0d013d6063af395f8

# lambda__opensearch__s3: the Lambda python runtime has no AWS SDK and its `os` module is incomplete (no os.stat_result)

> _Note: this issue was identified against the code base at commit `a0ca0e6d405fa8f821a988e0d013d6063af395f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 2 — the managed python runtime ships no boto3/botocore.
AWS: the python3.x managed runtimes bundle boto3/botocore; a handler may `import boto3` and call s3:* and the OpenSearch domain's endpoint under its execution role, returning its own result — or the service's AccessDenied/403 when an identity policy, a bucket policy or the domain access policy denies it.
Emulator: Invoke answers HTTP 200 with FunctionError='Unhandled' and payload {"errorMessage":"Unable to import module '<module>': No module named 'boto3'","errorType":"Runtime.ImportModuleError"} — fixed text "Unable to import module '<module>': No module named 'boto3'" with the configured handler module substituted.
Why: no handler can index, search or copy anything, so both the positive flows and the bucket-policy/domain-policy denial controls are unobservable.
Fix: the python runtime's bundled site-packages / module search path in the lambda runtime component.
Traces: lambda__opensearch__s3.cli-create-lambda-with-s3-opensearch-role-and-invoke, .cli-lambda-role-policy-detach-blocks-opensearch-access-negative, .cli-update-lambda-env-opensearch-endpoint-verify, .deploy-iam-role-trusts-lambda-service-positive, .deploy-lambda-role-missing-opensearch-policy-denied, .deploy-opensearch-domain-policy-restricts-lambda-denied, .lambda-sdk-s3-bucket-policy-deny-negative, .lambda-sdk-s3-copyobject-cross-bucket-positive, .lambda-sdk-s3-deleteobject-positive, .lambda-sdk-s3-headobject-positive, .lambda-sdk-s3-listobjects-positive, .lambda-sdk-s3-missing-listbucket-permission-negative, .lambda-sdk-s3-presigned-url-generation-positive.

Cause 2 of 2 — the runtime's `os` module has no `stat_result`.
AWS: the python managed runtime is a complete CPython; `os.stat_result` exists, so modules that reference it (and anything importing them, including common HTTP/auth libraries) import cleanly.
Emulator: Runtime.ImportModuleError with errorMessage "Unable to import module '<module>': module 'os' has no attribute 'stat_result'"; the fixed text is "module 'os' has no attribute 'stat_result'" and the only variable part is the handler module name.
Why: a handler that imports any ordinary library touching os.stat_result dies at init, so the OpenSearch data-plane calls never run. The open issue 20261105T085348Z-iam-lambda-s3-2-net-new-root-causes.md records this for the iam__lambda__s3 group and does not carry these replay keys.
Fix: the embedded CPython's `os` module surface in the lambda runtime component.
Traces: lambda__opensearch__s3.lambda-sdk-opensearch-bulk-index-positive, .lambda-sdk-opensearch-delete-index-positive, .lambda-sdk-opensearch-domain-access-policy-denies-negative, .lambda-sdk-opensearch-index-document-positive, .lambda-sdk-opensearch-missing-permission-negative, .lambda-sdk-opensearch-search-query-positive, .lambda-sdk-opensearch-update-document-positive.

## Evidence
Cause 1: AssertionError: unexpected FunctionError / lambda invocation errored: {'errorMessage': "Unable to import module 'lambda_function': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError'}; also 'expected StatusCode 200 with no FunctionError, got {StatusCode: 200, FunctionError: Unhandled}'.
Cause 2: AssertionError: lambda invocation failed: {"errorMessage":"Unable to import module 'lambda_function': module 'os' has no attribute 'stat_result'","errorType":"Runtime.ImportModuleError","stackTrace":[]}. Expected: the module imports and the handler returns its OpenSearch result.

## Steps to Reproduce
1. CreateFunction (python3.12) whose handler does `import boto3` and calls s3.list_objects_v2, then Invoke (cause 1).
2. CreateFunction whose handler imports a module that references os.stat_result (e.g. an HTTP/auth helper used to sign OpenSearch requests), then Invoke -> Runtime.ImportModuleError naming os.stat_result (cause 2).
