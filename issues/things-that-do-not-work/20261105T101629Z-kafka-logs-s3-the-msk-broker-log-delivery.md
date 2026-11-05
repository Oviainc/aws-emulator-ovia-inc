Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka__logs__s3.msk-broker-logs-reach-cloudwatch-logs-and-s3-while-an-unwired-log-group-stays-empty
Issue-Key: kafka__logs__s3.msk-broker-logs-reach-cloudwatch-logs-and-s3-while-an-unwired-log-group-stays-empty|_invoke#1|AssertionError|raise _no_executor(
Issue-Commit: acf010afbee59d2a607241e52e2b89d33ae0d393

# kafka__logs__s3: the MSK broker-log delivery check cannot run — the Lambda Python runtime's `os` module lacks register_at_fork, so the in-VPC probe handler dies with Runtime.ImportModuleError

> _Note: this issue was identified against the code base at commit `acf010afbee59d2a607241e52e2b89d33ae0d393`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1 (check: kafka__logs__s3.msk-broker-logs-reach-cloudwatch-logs-and-s3-while-an-unwired-log-group-stays-empty).

This check measures whether MSK broker logs reach a wired CloudWatch Logs group and an S3 prefix while an unwired log group stays empty. The broker traffic that produces those log events has to be generated from inside the cluster's VPC, so the check deploys a python3.x Lambda with VpcConfig (vendoring kafka-python) and proves it is usable with a `selftest` invocation first. That invocation never gets past module import, so neither the CloudWatch Logs delivery leg, the S3 delivery leg, nor the unwired-group absence leg is ever exercised.

AWS: recorded behavior only for the runtime surface — the Smithy model for lambda Invoke says nothing about the contents of a runtime's standard library. On AWS, a managed python3.x runtime ships a complete, unmodified CPython standard library, so `import os` yields a module that has `os.register_at_fork` (present in CPython since 3.7 and used at import time by stdlib modules such as `random`). The handler package therefore imports and Invoke answers HTTP 200 with the handler's JSON payload and NO `FunctionError` / `X-Amz-Function-Error` header. AWS emits `Runtime.ImportModuleError` only when the customer's own package is unimportable, never because the stdlib is incomplete.

Emulator: Invoke answers HTTP 200 with `FunctionError='Unhandled'` and body `{"errorMessage":"Unable to import module 'index': module 'os' has no attribute 'register_at_fork'","errorType":"Runtime.ImportModuleError","stackTrace":[]}`. The fixed text is AWS's import-failure envelope (`errorType` `Runtime.ImportModuleError`, message template `Unable to import module '<handler module>': <python import error>`); the variable parts are the handler module name from the function's `Handler` setting and the underlying Python error text. Here the underlying text is emulator-authored: the emulator's in-process Python runtime exposes an incomplete `os` module with no `register_at_fork`, so the handler's first stdlib import raises AttributeError and no handler code runs.

Why: any client function whose code or vendored dependency touches more of the standard library than the emulator's partial shim provides gets a 200-with-FunctionError instead of its result, and the error text blames the client's deployment package. Concretely here, MSK broker-log delivery to CloudWatch Logs and to S3 is unobservable — a user wiring LoggingInfo gets no signal at all about whether the destinations work.

Fix: the emulator's in-process Python Lambda runtime — the sandbox/stdlib shim behind `Invoke` for the python runtimes. It must expose the real CPython `os` module (including `register_at_fork`) instead of a partial stand-in, so a handler package importing `random`/`socket`/`os` imports cleanly and Invoke returns the handler's payload with no FunctionError.

Related open issues report the same incomplete-`os` runtime from other services and do NOT carry this check's replay key: 20261105T083215Z-ec2-lambda-logs-route53resolver-the-lambda-python-runtime-has-a-crippled-stdlib.md, 20261105T090541Z-elasticache-logs-the-slow-log-delivery-check-cannot.md.

Traces: kafka__logs__s3.msk-broker-logs-reach-cloudwatch-logs-and-s3-while-an-unwired-log-group-stays-empty (failure raised in `_prove_executor_in_vpc` -> `_invoke`, check file lines 985/959).

## Evidence
Expected (AWS): `lambda.Invoke(FunctionName=<in-VPC executor>, Payload={"op":"selftest",...})` -> HTTP 200, no `FunctionError`, body is the handler's own JSON selftest result.

Actual (emulator): HTTP 200 with `FunctionError='Unhandled'` and body
`{"errorMessage":"Unable to import module 'index': module 'os' has no attribute 'register_at_fork'","errorType":"Runtime.ImportModuleError","requestId":"<per-run uuid>","stackTrace":[]}`

Suite assertion:
`AssertionError: the in-VPC executor is unusable, so this run measured nothing: Invoke('selftest') returned FunctionError='Unhandled': {"errorMessage":"Unable to import module 'index': module 'os' has no attribute 'register_at_fork","errorType":"Runtime.ImportModuleError",...}`

(The requestId is a per-run value and not part of the requirement; the errorType, the message template and the presence of FunctionError are.)

## Steps to Reproduce
1. Create an IAM role with AWSLambdaVPCAccessExecutionRole attached.
2. CreateFunction with Runtime=python3.x, Handler=index.handler, VpcConfig pointing at a VPC subnet + security group, and a zip whose `index.py` imports a stdlib module that registers a fork handler at import time (e.g. `import random`, or vendored kafka-python).
3. Invoke the function with {"op":"selftest"}.
4. Observe HTTP 200 with FunctionError='Unhandled' and errorType Runtime.ImportModuleError ("module 'os' has no attribute 'register_at_fork'") instead of a 200 carrying the handler's return value.

Or replay: kafka__logs__s3.msk-broker-logs-reach-cloudwatch-logs-and-s3-while-an-unwired-log-group-stays-empty
