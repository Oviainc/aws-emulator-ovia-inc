Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3.layer-from-s3-is-mounted-under-opt-for-a-function lambda__s3.zipfile-and-s3-source-deploy-identical-code lambda__s3.layer-publish-rejected-for-an-unknown-s3-object-version
Issue-Key: lambda__s3.layer-from-s3-is-mounted-under-opt-for-a-function|_invoke#10|AssertionError|assert "FunctionError" not in resp, (
Issue-Key: lambda__s3.zipfile-and-s3-source-deploy-identical-code|_invoke#14|AssertionError|assert "FunctionError" not in resp, (
Issue-Key: lambda__s3.layer-publish-rejected-for-an-unknown-s3-object-version|run#12|AssertionError|assert got == ("InvalidParameterValueException", _NO_SUCH_VERSION_MSG), (
Issue-Commit: 4a9463f06c8da54961be1caf9572b353149b691e

# lambda__s3: an S3-sourced layer is not mounted under /opt, a function cannot read its own /var/task files, and PublishLayerVersion's NoSuchVersion refusal names GetObject instead of GetObjectVersion (3 causes)

> _Note: this issue was identified against the code base at commit `4a9463f06c8da54961be1caf9572b353149b691e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Three net-new root causes around S3-sourced Lambda code and layers. All three must be closed before this issue is submitted.

Cause 1 of 3 — a layer version published from an S3 object and attached to a function is never materialized into the execution environment, so the handler's import of the layer module fails.
AWS: Smithy: Lambda PublishLayerVersion (POST /2018-10-31/layers/{LayerName}/versions, 201; input Content: LayerVersionContentInput with S3Bucket/S3Key/S3ObjectVersion; output LayerArn, LayerVersionArn, Version, Content) and CreateFunction's Layers member. The sandbox layout itself is recorded behavior only: AWS extracts each attached layer's archive into /opt in the execution environment, and for python runtimes /opt/python and /opt/python/lib/python3.x/site-packages are on sys.path, so a handler's `import layer_marker` resolves and Invoke answers 200 with no FunctionError header.
Emulator: Invoke answers with FunctionError 'Unhandled' and the payload {"errorMessage":"Unable to import module 'app': No module named 'layer_marker'","errorType":"Runtime.ImportModuleError","requestId":"","stackTrace":[]} — the layer content never appears under /opt. In that message "Unable to import module '<handler module>': No module named '<module>'" is the template's fixed text with the handler module and the missing module taken from the request/package; 'Runtime.ImportModuleError' is the fixed errorType. The divergence is that the import fails at all.
Why: every function that keeps shared code or a dependency in a layer faults on its first invoke, so a layer is write-only configuration: it can be published and attached but never used.
Fix: the lambda execution-environment provisioner — layer archive extraction into /opt and the runtime's sys.path assembly — not PublishLayerVersion, which already minted the version. Open issue 20261107T091747Z-lambda-lambda-layers-are-never-materialized-into-the.md reports the same shape under service `lambda`; it does not carry this check's replay key, so this cause is filed here.
Traces: lambda__s3.layer-from-s3-is-mounted-under-opt-for-a-function.

Cause 2 of 3 — a function cannot open a file from its own deployment package at LAMBDA_TASK_ROOT.
AWS: recorded behavior only (the Lambda Smithy model does not describe the sandbox filesystem). The deployment package is extracted to /var/task, LAMBDA_TASK_ROOT names that directory, and its files are readable, so open(os.path.join(os.environ["LAMBDA_TASK_ROOT"], "index.py"), "rb") succeeds and Invoke answers 200 with no FunctionError — identically whether the code was uploaded as ZipFile or from an S3 object.
Emulator: Invoke answers FunctionError 'Unhandled' with {"errorMessage":"a Lambda function in this emulator cannot open a file of the host","errorType":"OSError","requestId":"<per-invoke uuid>","stackTrace":["  File \"/var/task/index.py\", line 6, in handler\n    with open(os.path.join(os.environ[\"LAMBDA_TASK_ROOT\"], \"index.py\"), \"rb\") as fh:\n"]} — an emulator-specific errorMessage that no AWS runtime emits, raised from the handler's own open() of its package file.
Why: any handler that reads a bundled data file, template, certificate bundle, schema or its own source — and any check that verifies a ZipFile-sourced and an S3-sourced deployment are byte-identical — fails with an error text that exists nowhere in AWS, so the failure cannot even be matched by a client's error handling.
Fix: the in-process lambda runtime's execution-environment filesystem (materialize the deployment package under /var/task and allow reads).
Traces: lambda__s3.zipfile-and-s3-source-deploy-identical-code.

Cause 3 of 3 — PublishLayerVersion pinned to a nonexistent S3 object version names the wrong S3 API in its refusal message.
AWS: Smithy: PublishLayerVersion errors include InvalidParameterValueException (client, HTTP 400). When Content.S3ObjectVersion is supplied and that version id does not exist, the refusal is InvalidParameterValueException with message 'Error occurred while GetObjectVersion. S3 Error Code: NoSuchVersion. S3 Error Message: The specified version does not exist.' — 'Error occurred while ', '. S3 Error Code: ', '. S3 Error Message: ' and the trailing '.' are fixed text; the S3 API name is GetObjectVersion precisely because the request pinned a version, and the S3 error code and message are passed through from S3.
Emulator: the status and error code are right (400 InvalidParameterValueException) but the message reads 'Error occurred while GetObject. S3 Error Code: NoSuchVersion. S3 Error Message: The specified version does not exist.' — it names GetObject, the unversioned read, for a versioned request.
Why: the message is the only place that distinguishes "the object is missing" from "that object version is missing"; a client or operator matching on it misdiagnoses a bad S3ObjectVersion as a bad S3Key.
Fix: the S3 content-resolution validation in PublishLayerVersion's handler — it must report the S3 operation it actually performed (GetObjectVersion when S3ObjectVersion is present) when formatting the InvalidParameterValueException. Open issue 20261103T031416Z-lambda-s3-6-net-new-causes-in-s3.md covers other S3 refusals of PublishLayerVersion and owns sibling checks in this group; it does not carry this check's replay key, so this cause is filed here.
Traces: lambda__s3.layer-publish-rejected-for-an-unknown-s3-object-version.

## Evidence
Cause 1 — check lambda__s3.layer-from-s3-is-mounted-under-opt-for-a-function [mismatch]:
  Expected: Invoke of a function whose Layers include the S3-published layer version returns 200 with no FunctionError (the handler imports layer_marker from /opt/python).
  Actual (replay): AssertionError: the handler raised ('Unhandled'): b'{"errorMessage":"Unable to import module \'app\': No module named \'layer_marker\'","errorType":"Runtime.ImportModuleError","requestId":"","stackTrace":[]}'

Cause 2 — check lambda__s3.zipfile-and-s3-source-deploy-identical-code [mismatch]:
  Expected: both the ZipFile-sourced and the S3-sourced function Invoke 200 with no FunctionError, the handler having read its own /var/task/index.py.
  Actual (replay): AssertionError: invoke reported FunctionError='Unhandled': b'{"errorMessage":"a Lambda function in this emulator cannot open a file of the host","errorType":"OSError","requestId":"...","stackTrace":["  File \\"/var/task/index.py\\", line 6, in handler\\n    with open(os.path.join(os.environ[\\"LAMBDA_TASK_ROOT\\"], \\"index.py\\"), \\"rb\\") as fh:\\n"]}'

Cause 3 — check lambda__s3.layer-publish-rejected-for-an-unknown-s3-object-version [mismatch]:
  Expected: ('InvalidParameterValueException', 'Error occurred while GetObjectVersion. S3 Error Code: NoSuchVersion. S3 Error Message: The specified version does not exist.')
  Actual (replay):  ('InvalidParameterValueException', 'Error occurred while GetObject. S3 Error Code: NoSuchVersion. S3 Error Message: The specified version does not exist.')

## Steps to Reproduce
Cause 1:
1. Create a bucket; PutObject a layer zip containing python/layer_marker.py.
2. PublishLayerVersion with Content={S3Bucket, S3Key} and CompatibleRuntimes=[python3.x].
3. CreateFunction with a handler module app.py that does `import layer_marker`, Layers=[<LayerVersionArn>].
4. Invoke the function. AWS: 200, no FunctionError. Emulator: FunctionError 'Unhandled', Runtime.ImportModuleError 'No module named layer_marker'.

Cause 2:
1. Build a zip whose index.py handler opens os.path.join(os.environ["LAMBDA_TASK_ROOT"], "index.py") in 'rb' and returns something derived from the bytes.
2. CreateFunction twice from the same bytes — once with Code.ZipFile, once with Code.S3Bucket/S3Key.
3. Invoke both. AWS: both 200 with identical results. Emulator: FunctionError 'Unhandled' with errorType OSError and errorMessage 'a Lambda function in this emulator cannot open a file of the host'.

Cause 3:
1. Create a versioning-enabled bucket and PutObject a valid layer zip.
2. PublishLayerVersion with Content={S3Bucket, S3Key, S3ObjectVersion: <a version id that does not exist>}.
3. Read the ClientError: AWS gives InvalidParameterValueException with 'Error occurred while GetObjectVersion. ...'; the emulator gives the same code with 'Error occurred while GetObject. ...'.
