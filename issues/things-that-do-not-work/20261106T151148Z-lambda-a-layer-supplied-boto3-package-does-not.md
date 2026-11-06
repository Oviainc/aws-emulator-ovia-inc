Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.layer-module-overrides-runtime-bundled-boto3
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.layer-module-overrides-runtime-bundled-boto3|run#9|AssertionError|assert overridden == {
Issue-Commit: 5678b2d659e23eeff4b2f39316abc675f4291395

# lambda: a layer-supplied boto3 package does not override the runtime-bundled one

> _Note: this issue was identified against the code base at commit `5678b2d659e23eeff4b2f39316abc675f4291395`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PublishLayerVersion

## Description
One root cause behind 1 failing check.

Cause 1 of 1: a Lambda layer that ships its own `boto3` package is not resolved by `import boto3`; the runtime's bundled copy wins.
AWS: layer content is extracted to `/opt` and, for python runtimes, `/opt/python` and `/opt/python/lib/python<X.Y>/site-packages` are placed on `sys.path` AHEAD of the runtime's own bundled SDK directory. So a layer that provides `boto3` shadows the runtime's bundled boto3: `boto3.__file__` resolves inside the layer (under `/opt/python/...`), and any marker attribute the layer's package defines is visible to the handler. Only the function's own package at `/var/task` takes precedence over a layer. Model: PublishLayerVersion / UpdateFunctionConfiguration.Layers are modelled in services/service-com-amazonaws-lambda-awsgirapiservice; the `/opt` path-precedence rule is recorded behavior only.
Emulator: with the layer attached, the handler reports `{'file': '/var/task/boto3/__init__.py', 'marker': None, 'has_client': True}` — `import boto3` resolved a copy under `/var/task` (the runtime's own injected SDK), not the layer's, and the layer's marker attribute is absent.
Why: pinning or patching the AWS SDK via a layer is a standard practice (newer botocore, extra service models, a vendored fork). Here the layer is silently ignored, so a function runs against a version its owner did not choose and cannot tell that the override failed — and, given the bundled SDK's limited service catalog, a layer is exactly how a user would try to work around it.
Fix: the Lambda execution environment's module path construction — layer content under `/opt/python` (and `/opt/python/lib/python<X.Y>/site-packages`) must precede the runtime's bundled SDK on `sys.path`, and the runtime must not inject its own SDK into `/var/task`.
Traces: lambda.layer-module-overrides-runtime-bundled-boto3.

## Evidence
Expected (AWS): with a layer that ships boto3, the handler sees `boto3.__file__` under the layer's `/opt/python/...` path and the layer package's marker attribute set.

Actual (emulator): AssertionError: import boto3 must resolve the layer's package; the handler returned {'file': '/var/task/boto3/__init__.py', 'marker': None, 'has_client': True}.

## Steps to Reproduce
1. PublishLayerVersion with a zip containing `python/boto3/__init__.py` that defines a marker attribute.
2. Create a python function with that layer attached, whose handler returns boto3.__file__ and the marker.
3. Invoke it. Expected: a path under /opt and the marker present. Observed: /var/task/boto3/__init__.py and marker None.
