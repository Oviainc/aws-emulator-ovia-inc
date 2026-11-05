Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__s3.sam-layerversion-serves-function-and-outlives-stack
Issue-Key: cloudformation__lambda__s3.sam-layerversion-serves-function-and-outlives-stack|_invoke#28|AssertionError|assert "FunctionError" not in out, f"{fn} raised: {body[:300]!r}"
Issue-Commit: c1082b91874a267e16e5e882b92de2a31503a88d

# cloudformation__lambda__s3: a SAM/CloudFormation-deployed LayerVersion is not mounted on the function's import path, so the function faults Runtime.ImportModuleError on the layer's module

> _Note: this issue was identified against the code base at commit `c1082b91874a267e16e5e882b92de2a31503a88d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: a layer version created by a CloudFormation/SAM stack (AWS::Serverless::LayerVersion / AWS::Lambda::LayerVersion) and attached to a function via its Layers list is never mounted into the invoke sandbox, so a handler that imports a module shipped by that layer fails at module import. Failing check: cloudformation__lambda__s3.sam-layerversion-serves-function-and-outlives-stack.

AWS: Lambda extracts every layer in a function's Layers list into `/opt` before the handler loads, and for the python runtimes `/opt/python` (and `/opt/python/lib/python3.x/site-packages`) is on `sys.path`, so a module packaged in the layer archive is importable from the handler. Smithy (lambda, Invoke): output StatusCode is bound to httpResponseCode, FunctionError to the response header `X-Amz-Function-Error`, Payload to the HTTP payload; a successful synchronous Invoke answers HTTP 200 with NO `X-Amz-Function-Error` header and Payload equal to the handler's return value. Smithy (lambda, PublishLayerVersion/GetLayerVersion) returns `Content: LayerVersionContentOutput` for the published archive — the layer content exists independently of the stack that created it, which is the second half of what this check asserts.

Emulator: the Invoke answers HTTP 200 with `x-amz-function-error: Unhandled` and the Payload is an import failure for the layer's own module — errorType `Runtime.ImportModuleError`, errorMessage `Unable to import module '<handler module>': No module named '<module shipped by the layer>'`. Fixed text: `Unable to import module '` + `': No module named '` + `'`; the variable parts are the handler module name (from the function's Handler setting), the layer-provided module name taken from the deployed layer archive, and the per-invocation requestId. The layer's bytes are therefore not on the handler's import path even though the stack reached CREATE_COMPLETE and the function's Layers list names the version.

Why: a client that factors shared code into a layer — the normal SAM pattern — gets a function that cannot load at all, and the failure is reported as if the customer's own package were broken. The further assertion that the layer outlives the stack that published it can never be reached.

Fix: the Lambda invoke runtime must extract each attached layer version's archive into the sandbox and put `/opt/python` on the handler's import path, and the layer/function state must keep the published archive addressable after the owning stack is deleted (services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs plus the layer state behind publish_layer_version.rs / the CloudFormation provisioner for AWS::Lambda::LayerVersion).

The open cloudformation__lambda__s3 issue 20261104T232822Z-cloudformation-lambda-s3-getfunction-omits-codesha256-for-a.md owns the other failing check of this group, not this one; 20261103T031416Z-lambda-s3-6-net-new-causes-in-s3.md describes layer mounting under the lambda__s3 grouping and does not carry this check's replay key.

Traces: see trace_ids.

## Evidence
Expected (AWS): Invoke of the stack's function -> HTTP 200, no `X-Amz-Function-Error` header, Payload = the handler's result computed with the layer-provided module.

Actual (emulator): `AssertionError: <function name> raised: '{"errorMessage":"Unable to import module \'index\': No module named \'<layer module>\'","errorType":"Runtime.ImportModuleError","requestId":"<per-invocation id>","stackTrace":[]}'` at check_cloudformation__lambda__s3_sam_layerversion_serves_function_and_outlives_stack.py line 126 (`assert "FunctionError" not in out`). The response is HTTP 200 with `x-amz-function-error: Unhandled`, so the stack and the function both look healthy while the layer content is absent from the import path.

## Steps to Reproduce
1. Deploy a SAM/CloudFormation stack declaring an AWS::Serverless::LayerVersion whose archive ships a python module under `python/`, and a function whose Layers list references it and whose handler imports that module.
2. Wait for CREATE_COMPLETE, then Invoke the function synchronously.
3. Observe HTTP 200 with `x-amz-function-error: Unhandled` and a Runtime.ImportModuleError payload naming the layer's module as missing.
4. Re-replay cloudformation__lambda__s3.sam-layerversion-serves-function-and-outlives-stack to confirm.
