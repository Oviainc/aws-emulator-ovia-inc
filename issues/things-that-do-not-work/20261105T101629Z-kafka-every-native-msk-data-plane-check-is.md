Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.msk-transactional-abort-invisible-to-read-committed-visible-to-read-uncommitted
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.msk-transactional-abort-invisible-to-read-committed-visible-to-read-uncommitted|_invoke#2|AssertionError|raise _no_executor(
Issue-Commit: acf010afbee59d2a607241e52e2b89d33ae0d393

# kafka: every native MSK data-plane check is blocked — the Lambda Python runtime's `os` module lacks register_at_fork, so the in-VPC probe handler dies with Runtime.ImportModuleError

> _Note: this issue was identified against the code base at commit `acf010afbee59d2a607241e52e2b89d33ae0d393`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1 (check: kafka.msk-transactional-abort-invisible-to-read-committed-visible-to-read-uncommitted).

Real MSK brokers resolve only to addresses inside the cluster's own VPC, so the MSK native-data-plane checks carry their measurement into the VPC in a Lambda function with VpcConfig (a python3.x function whose deployment package vendors kafka-python). The check first runs a `selftest` invocation of that executor to prove it is usable; that invocation never gets past module import, so the transactional read-committed/read-uncommitted behaviour of the MSK data plane is never exercised at all.

AWS: recorded behavior only for the runtime surface — the Smithy model for lambda Invoke says nothing about the contents of a runtime's standard library. On AWS, a managed python3.x runtime ships a complete, unmodified CPython standard library, so `import os` yields a module that has `os.register_at_fork` (present in CPython since 3.7 and used at import time by stdlib modules such as `random`). Importing the handler module therefore succeeds and Invoke answers HTTP 200 with the handler's JSON payload in the body and NO `FunctionError` / `X-Amz-Function-Error` header. A `Runtime.ImportModuleError` is what AWS returns only when the *customer's* package is genuinely unimportable — it is never produced by the stdlib being incomplete.

Emulator: Invoke returns HTTP 200 but with `FunctionError='Unhandled'` and the body `{"errorMessage":"Unable to import module 'index': module 'os' has no attribute 'register_at_fork'","errorType":"Runtime.ImportModuleError","stackTrace":[]}`. The fixed text of this payload is AWS's import-failure envelope (`errorType` `Runtime.ImportModuleError`, message template `Unable to import module '<handler module>': <python import error>`); the variable parts are the handler module name taken from the function's `Handler` setting and the underlying Python error text. Here the underlying text is emulator-authored — the emulator's in-process Python runtime exposes an `os` module that is missing `register_at_fork`, so the very first stdlib import in the handler raises `AttributeError` and the whole package is reported unimportable. No handler code runs.

Why: a client that deploys any function whose code (or whose vendored dependency) touches the standard library beyond a hand-picked subset gets a 200 with FunctionError instead of its result. For this suite it means no measurement of the MSK native data plane is possible from inside the VPC: every verdict about produce/fetch/transaction visibility is replaced by "the instrument is unusable". A developer debugging against the emulator is pointed at their own deployment package for a defect that lives entirely in the emulator's runtime.

Fix: the emulator's in-process Python Lambda runtime — the sandbox/stdlib shim that backs `Invoke` for the python runtimes. It must expose the real CPython `os` module (including `register_at_fork`) rather than a partial stand-in, so a handler package that imports `random`/`socket`/`os` imports cleanly and Invoke returns the handler's payload with no FunctionError.

Related open issues describe the same incomplete-`os` runtime from other services and do NOT carry this check's replay key: 20261105T083215Z-ec2-lambda-logs-route53resolver-the-lambda-python-runtime-has-a-crippled-stdlib.md and 20261105T090541Z-elasticache-every-in-vpc-data-plane-check-is.md. This issue is filed for the kafka check so the MSK data-plane replay key is owned.

Traces: kafka.msk-transactional-abort-invisible-to-read-committed-visible-to-read-uncommitted (failure raised in `_prove_executor_in_vpc` -> `_invoke`, check file lines 1166/1136).

## Evidence
Expected (AWS): `lambda.Invoke(FunctionName=<in-VPC executor>, Payload={"op":"selftest",...})` -> HTTP 200, no `FunctionError`, body is the handler's own JSON selftest result (route source IP, DNS rcode/ancount from the VPC resolver).

Actual (emulator): HTTP 200 with `FunctionError='Unhandled'` and body
`{"errorMessage":"Unable to import module 'index': module 'os' has no attribute 'register_at_fork'","errorType":"Runtime.ImportModuleError","requestId":"<per-run uuid>","stackTrace":[]}`

Suite assertion:
`AssertionError: the in-VPC executor is unusable, so this run measured nothing: Invoke of <fn> for op='selftest' returned StatusCode=200 FunctionError='Unhandled': b'{"errorMessage":"Unable to import module \'index\': module \'os\' has no attribute \'register_at_fork\'","errorType":"Runtime.ImportModuleError",...}'`

(The function name and requestId in the recording are per-run values and are not part of the requirement; the `errorType`, the message template and the presence of FunctionError are.)

## Steps to Reproduce
1. Create an IAM role with AWSLambdaVPCAccessExecutionRole attached.
2. CreateFunction with Runtime=python3.x, Handler=index.handler, VpcConfig pointing at a VPC subnet + security group, and a zip whose `index.py` imports a stdlib module that registers a fork handler at import time (e.g. `import random`, or vendored kafka-python).
3. Invoke the function with any payload, e.g. {"op":"selftest"}.
4. Observe HTTP 200 with FunctionError='Unhandled' and errorType Runtime.ImportModuleError, message "Unable to import module 'index': module 'os' has no attribute 'register_at_fork'", instead of a 200 carrying the handler's return value.

Or replay: kafka.msk-transactional-abort-invisible-to-read-committed-visible-to-read-uncommitted
