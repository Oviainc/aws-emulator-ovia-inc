Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3.async-invoke-returns-accepted-and-converges-to-the-same-object lambda__s3.bucket-policy-alone-grants-lambda-role-object-read lambda__s3.bucket-policy-deny-overrides-lambda-identity-allow lambda__s3.csv-to-xlsx-handler-escapes-xml-metacharacters lambda__s3.e2e-check-lambda-layers-s-code-lifecycle lambda__s3.function-sdk-put-outside-granted-prefix-denied-as-function-error lambda__s3.function-sdk-reads-what-the-caller-wrote lambda__s3.handler-guard-rejects-key-outside-the-raw-prefix lambda__s3.invoke-reports-handler-exception-through-function-error lambda__s3.output-key-is-the-basename-of-a-nested-source-key
Issue-Key: lambda__s3.async-invoke-returns-accepted-and-converges-to-the-same-object|run#11|KeyError|assert json.loads(sync_body)["written"] == [_out_key(_OUTPUT_PREFIX, sync_key)], (
Issue-Key: lambda__s3.bucket-policy-alone-grants-lambda-role-object-read|_invoke_payload#12|AssertionError|assert "error" not in result, f"handler faulted instead of returning a result: {result!r}"
Issue-Key: lambda__s3.bucket-policy-deny-overrides-lambda-identity-allow|_invoke_payload#9|AssertionError|assert "error" not in result, f"handler faulted instead of returning a result: {result!r}"
Issue-Key: lambda__s3.csv-to-xlsx-handler-escapes-xml-metacharacters|run#8|AssertionError|assert resp.get("FunctionError") is None, (
Issue-Key: lambda__s3.e2e-check-lambda-layers-s-code-lifecycle|phase#6|_PhaseError|raise _PhaseError(
Issue-Key: lambda__s3.function-sdk-put-outside-granted-prefix-denied-as-function-error|run#8|AssertionError|assert err is None, f"the permitted PutObject failed the invocation: {err!r} {out!r}"
Issue-Key: lambda__s3.function-sdk-reads-what-the-caller-wrote|run#12|AssertionError|assert err is None, f"the function's reads failed: {err!r} {out!r}"
Issue-Key: lambda__s3.handler-guard-rejects-key-outside-the-raw-prefix|run#9|AssertionError|assert resp.get("FunctionError") is None, (
Issue-Key: lambda__s3.invoke-reports-handler-exception-through-function-error|run#9|AssertionError|assert detail["errorType"] == "KeyError", (
Issue-Key: lambda__s3.output-key-is-the-basename-of-a-nested-source-key|run#10|AssertionError|assert resp.get("FunctionError") is None, (
Issue-Commit: c1082b91874a267e16e5e882b92de2a31503a88d

# lambda__s3: 3 net-new root causes — the python runtime has no boto3, its stdlib is missing os.stat_result, and a layer version's Content omits CodeSha256

> _Note: this issue was identified against the code base at commit `c1082b91874a267e16e5e882b92de2a31503a88d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Three net-new root causes in the lambda__s3 group. 10 failing checks.

Cause 1 of 3: the python3.x Lambda invoke runtime has no boto3/botocore on the handler's import path, so a function that imports the AWS SDK to call S3 fails at module import and its body never runs. Failing checks: lambda__s3.bucket-policy-alone-grants-lambda-role-object-read, lambda__s3.bucket-policy-deny-overrides-lambda-identity-allow, lambda__s3.function-sdk-put-outside-granted-prefix-denied-as-function-error, lambda__s3.function-sdk-reads-what-the-caller-wrote, lambda__s3.invoke-reports-handler-exception-through-function-error, lambda__s3.async-invoke-returns-accepted-and-converges-to-the-same-object.
AWS: the managed python3.x runtimes ship boto3/botocore pre-installed on sys.path. Smithy (lambda, Invoke): StatusCode is httpResponseCode, FunctionError is the response header `X-Amz-Function-Error`, Payload is the HTTP payload; a successful synchronous Invoke answers HTTP 200 with no `X-Amz-Function-Error` and Payload equal to the handler's return value. When the handler itself raises, AWS sets `X-Amz-Function-Error: Unhandled` and the payload's errorType is the raised python exception class (e.g. `KeyError`).
Emulator: every such Invoke answers HTTP 200 with `x-amz-function-error: Unhandled` and payload errorType `Runtime.ImportModuleError`, errorMessage `Unable to import module '<module>': No module named 'boto3'` (fixed text `Unable to import module '` + `': No module named 'boto3'`; `<module>` comes from the function's Handler setting — observed as `lambda_function` and `index` — and requestId is per-invocation). Because the module never loads, lambda__s3.invoke-reports-handler-exception-through-function-error sees errorType `Runtime.ImportModuleError` where the handler's own `KeyError` is required, and lambda__s3.async-invoke-returns-accepted-and-converges-to-the-same-object gets a payload with no `written` key (`KeyError: 'written'`).
Why: a client whose function reads or writes S3 gets a function that cannot run, the granted/denied prefix distinction is unobservable, and a genuine handler exception is misreported as an SDK-absence import error, so the caller blames its own code.
Fix: the Lambda invoke runtime/sandbox backing the python3.x runtimes must expose boto3/botocore on the handler's import path (services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs and the python runtime bundle it loads).

Cause 2 of 3: the same runtime's python standard library is incomplete — `os` has no `stat_result` attribute — so a handler module whose imports touch it dies at import. Failing checks: lambda__s3.csv-to-xlsx-handler-escapes-xml-metacharacters, lambda__s3.handler-guard-rejects-key-outside-the-raw-prefix, lambda__s3.output-key-is-the-basename-of-a-nested-source-key.
AWS: the managed python3.x runtimes provide the complete CPython stdlib; `os.stat_result` is a core CPython type, so importing it never fails. Recorded behavior only for the class: AWS answers HTTP 200, no `X-Amz-Function-Error`, with the handler's JSON report.
Emulator: HTTP 200 with `x-amz-function-error: Unhandled` and errorType `Runtime.ImportModuleError`, errorMessage `Unable to import module '<module>': module 'os' has no attribute 'stat_result'` — fixed text `Unable to import module '` + `': module 'os' has no attribute 'stat_result'`, `<module>` from the Handler setting, requestId per invocation.
Why: no handler that uses ordinary file/zip/xlsx stdlib paths can run, so conversion and key-guard behavior is entirely unreachable.
Fix: the python runtime bundle loaded by the Lambda invoke sandbox must expose a complete CPython stdlib including `os.stat_result` (same component as cause 1).

Cause 3 of 3: a layer version published from an S3 object reports a Content without `CodeSha256`, so the deploy phase of the layers lifecycle cannot read it. Failing check: lambda__s3.e2e-check-lambda-layers-s-code-lifecycle.
AWS: Smithy (lambda) — PublishLayerVersion (HTTP 201) and GetLayerVersion (HTTP 200) both return `Content: LayerVersionContentOutput`, whose members include `CodeSha256` ("The SHA-256 hash of the layer archive"), `CodeSize` and `Location`; AWS populates CodeSha256 for every published layer version, whether the content came from an inline ZipFile or from S3.
Emulator: the Content it returns omits CodeSha256 entirely, so a caller indexing `Content['CodeSha256']` gets `KeyError: 'CodeSha256'` — the check fails in `[phase 1/4 deploy]`.
Why: a client cannot verify the integrity of a published layer archive or detect that a layer's bytes changed, because the only hash AWS exposes for it is missing.
Fix: the PublishLayerVersion / GetLayerVersion / GetLayerVersionByArn serializer and the layer state that backs them must compute and carry CodeSha256 (services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/publish_layer_version.rs and get_layer_version.rs). The open issues 20261103T031416Z-lambda-s3-6-net-new-causes-in-s3.md and 20261029T211043Z-lambda-s3-s3-sourced-lambda-code-codesha256-codesize-are-absent.md describe the same omission for other checks of this group but do not carry this check's replay key.

Related open issues for causes 1-2 under other service groupings (they do not carry these checks' replay keys): 20261105T094343Z-events-lambda-s3-the-lambda-python-runtime-provides.md, 20261105T081644Z-lambda-s3-stepfunctions-the-lambda-python-runtime-ships.md.

Traces: see trace_ids.

## Evidence
Expected (AWS): Invoke -> HTTP 200, no `X-Amz-Function-Error`, Payload = the handler's JSON report; when the handler raises, `X-Amz-Function-Error: Unhandled` with errorType = the python exception class. PublishLayerVersion/GetLayerVersion -> Content carrying CodeSha256, CodeSize and Location.

Actual (emulator):
- lambda__s3.bucket-policy-alone-grants-lambda-role-object-read: `AssertionError: handler faulted instead of returning a result: {'status_code': 200, 'error': 'Unhandled', 'payload': {'errorMessage': "Unable to import module 'lambda_function': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', ...}}`
- lambda__s3.bucket-policy-deny-overrides-lambda-identity-allow: same payload shape.
- lambda__s3.function-sdk-put-outside-granted-prefix-denied-as-function-error: `AssertionError: the permitted PutObject failed the invocation: 'Unhandled' {... "Unable to import module 'index': No module named 'boto3'" ...}`
- lambda__s3.function-sdk-reads-what-the-caller-wrote: `AssertionError: the function's reads failed: 'Unhandled' {... No module named 'boto3' ...}`
- lambda__s3.invoke-reports-handler-exception-through-function-error: `AssertionError: the error payload reports errorType 'Runtime.ImportModuleError', expected the Python exception class KeyError`
- lambda__s3.async-invoke-returns-accepted-and-converges-to-the-same-object: `KeyError: 'written'` — the payload is the import error, not the handler report.
- lambda__s3.csv-to-xlsx-handler-escapes-xml-metacharacters / handler-guard-rejects-key-outside-the-raw-prefix / output-key-is-the-basename-of-a-nested-source-key: `'Unhandled' '{"errorMessage":"Unable to import module \'lambda_function\': module \'os\' has no attribute \'stat_result\'","errorType":"Runtime.ImportModuleError",...}'`
- lambda__s3.e2e-check-lambda-layers-s-code-lifecycle: `_PhaseError: [phase 1/4 deploy] KeyError: 'CodeSha256'` — the layer version's Content omits CodeSha256.

## Steps to Reproduce
1. CreateFunction (python3.x) from a zip whose handler does `import boto3` and calls s3.get_object/put_object; Invoke -> HTTP 200, `x-amz-function-error: Unhandled`, Runtime.ImportModuleError naming boto3.
2. CreateFunction (python3.x) from a zip whose handler imports a stdlib path touching `os.stat_result`; Invoke -> same shape with `module 'os' has no attribute 'stat_result'`.
3. CreateFunction whose handler raises KeyError and Invoke -> errorType must be `KeyError`, not `Runtime.ImportModuleError`.
4. PutObject a layer zip to S3, PublishLayerVersion with Content.S3Bucket/S3Key, then GetLayerVersion -> Content must carry CodeSha256 alongside CodeSize and Location.
5. Re-replay the 10 checks in trace_ids.
