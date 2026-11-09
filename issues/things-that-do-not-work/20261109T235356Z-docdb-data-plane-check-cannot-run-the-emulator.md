Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: docdb.cluster-mongo-insert-find-round-trip
Issue-Shape-Id: com.amazonaws.docdb#AmazonRDSv19
Issue-Key: docdb.cluster-mongo-insert-find-round-trip|_probe_call#21|AssertionError|assert "FunctionError" not in resp, f"the in-VPC probe raised: {out!r}"
Issue-Commit: 798119d59e6f7ad23ed03b3274703a15fb5570c4

# docdb data-plane check cannot run: the emulator's python3.12 Lambda runtime has no _ssl module, so any handler importing ssl fails with Runtime.ImportModuleError

> _Note: this issue was identified against the code base at commit `798119d59e6f7ad23ed03b3274703a15fb5570c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: the Python managed runtime the emulator serves to Lambda has no CPython `_ssl` extension module, so a handler whose module body does `import ssl` cannot be imported at all. The docdb check reaches this because it verifies the cluster's MongoDB data plane from inside the VPC: it deploys a one-file python3.12 probe function (Code.ZipFile with index.py, Handler index.handler, VpcConfig in the run's subnet/security group), waits for State Active, then calls Lambda Invoke with the cluster host/port and a TLS MongoDB session (ssl.create_default_context + wrap_socket, SCRAM-SHA-1, insert, find).

AWS: recorded behavior only for the runtime's contents — the Smithy model for Lambda Invoke says nothing about which standard-library modules a runtime ships; it only models the response's FunctionError member and the payload. On real AWS the managed python3.12 runtime is a complete CPython build, so `import ssl` succeeds and the probe's invoke returns a 200 with NO FunctionError header/member and the handler's own result document (ok/authenticated/insert_ok/insert_n/find_ok/first_batch, or an `error` string the handler itself composed). DocDB itself is TLS-only, so any client of a DocDB cluster necessarily imports ssl.

Emulator: the Invoke answers with FunctionError set and the payload `{"errorMessage": "Unable to import module 'index': No module named '_ssl'", "errorType": "Runtime.ImportModuleError", "requestId": "", "stackTrace": []}`. Fixed text of that message is `Unable to import module '<module>': ` followed by the interpreter's own ImportError text; the variable parts are the handler's module name (here `index`, taken from the Handler of the request) and the missing module name (`_ssl`). `requestId` being empty is itself wrong — AWS fills it with the invocation's request id — but that is a per-run value, so the requirement is only that it is populated. errorType `Runtime.ImportModuleError` is the right class for a genuinely missing user module; here nothing is missing from the user's deployment package, only from the emulator's runtime.

Why: a client cannot exercise any TLS data plane (DocDB/MongoDB, HTTPS, any boto3 call) from a Python Lambda on this emulator — the function never starts. The failure is attributed to the customer's code (ImportModuleError on their module) when the defect is in the runtime image, so a user debugging it is sent to the wrong place. For this check it also masks whatever the DocDB endpoint would have answered, so the cluster's mongo insert/find round trip reaches no verdict at all.

Fix: the in-process Python Lambda runtime of the Lambda crate — the interpreter/stdlib it bundles for the python3.x runtimes must include the `_ssl` extension (and so a working `ssl`, `hashlib` HMAC/PBKDF2 path and `socket` TLS), the same place the sibling open issue 20261109T143515Z-dynamodb-iam-lambda-logs-sts-the-in-process-lambda-runtime-has-no-aws reports a missing bundled library for. That issue is about a missing durable-execution SDK package and does NOT cover this: `_ssl` is part of the standard library, not an AWS SDK add-on. Note also that even with the probe fixed, this check still depends on the published DocDB endpoint being reachable (open issue 20261109T120625Z-docdb-cluster-instance-endpoints-are-published-as-real), so re-replay must get past both.

Traces: docdb.cluster-mongo-insert-find-round-trip (the probe invoke at _probe_call, assert "FunctionError" not in resp).

## Evidence
Expected (AWS): Lambda Invoke of the in-VPC python3.12 probe returns no FunctionError and the handler's own result document, letting the check assert the MongoDB insert/find round trip against the DocDB cluster endpoint.
Actual (emulator): Invoke returns FunctionError with payload {'errorMessage': "Unable to import module 'index': No module named '_ssl'", 'errorType': 'Runtime.ImportModuleError', 'requestId': '', 'stackTrace': []}; the check fails at `assert "FunctionError" not in resp, f"the in-VPC probe raised: {out!r}"`. The probe's only unusual import is the standard library's `ssl` (used for ssl.create_default_context()/wrap_socket() because DocDB requires TLS).

## Steps to Reproduce
1. Replay docdb.cluster-mongo-insert-find-round-trip. 2. Or directly: lambda.create_function(Runtime='python3.12', Handler='index.handler', Code={'ZipFile': <zip with index.py containing `import ssl` at module level and a handler>}), wait for State Active, then lambda.invoke(FunctionName=...). Observe FunctionError and the Runtime.ImportModuleError payload naming `_ssl`; on AWS the invoke succeeds.
