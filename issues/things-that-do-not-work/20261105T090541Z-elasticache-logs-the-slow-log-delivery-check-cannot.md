Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache__logs.slow-log-delivery-lands-the-engine-command-in-cloudwatch-logs
Issue-Key: elasticache__logs.slow-log-delivery-lands-the-engine-command-in-cloudwatch-logs|_invoke#1|AssertionError|raise _no_executor(
Issue-Commit: 6da489cbc4c2969d809761cd42ffd786edefa992

# elasticache__logs: the slow-log delivery check cannot run — the Lambda Python runtime's os module lacks register_at_fork, so the in-VPC RESP probe dies with Runtime.ImportModuleError

> _Note: this issue was identified against the code base at commit `6da489cbc4c2969d809761cd42ffd786edefa992`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new root cause, reached by 1 check.

Cause 1 of 1: the Lambda invoke runtime the emulator executes handlers on is an incomplete CPython — its `os` module has no `register_at_fork` — so the in-VPC RESP probe handler module fails to import, no slow command is ever issued to the cache, and the CloudWatch Logs slow-log delivery claim is never exercised.

AWS: Invoke of a python3.x function returns HTTP 200 with the handler's payload and NO FunctionError header; the managed runtime is a complete CPython, where `os.register_at_fork` has existed since CPython 3.7, so stdlib modules touching it (`random`, `subprocess`, `multiprocessing`) import cleanly. A genuine import failure is reported as FunctionError 'Unhandled' with fixed text `Unable to import module '<module>': ` plus the underlying import error and errorType `Runtime.ImportModuleError`. Smithy (lambda, Invoke): the response carries StatusCode / FunctionError / Payload; nothing in the model sanctions a partial standard library. The ElastiCache/Logs side is recorded behavior only: a cache with slow-log delivery configured to a log group emits the slow command into that group, and the command can only be issued from inside the VPC.

Emulator: the Invoke returns StatusCode=200 with FunctionError='Unhandled' and body {"errorMessage": "Unable to import module 'index': module 'os' has no attribute 'register_at_fork'", "errorType": "Runtime.ImportModuleError", "requestId": "<per-run id>", "stackTrace": []} — the requestId is a per-run value, the rest is the fixed text. The emulator serves that reply, so the defect is its handler execution environment.

Why: a client cannot reach an ElastiCache endpoint except from inside the VPC, so the whole slow-log pipeline (engine records a slow command -> the configured LogDeliveryConfiguration ships it -> it appears in the CloudWatch Logs group) is unobservable: nothing can generate the command whose delivery is being claimed.

Fix: the Lambda invoke runtime (the python sandbox the lambda service's Invoke path executes handler modules in) must expose a complete CPython standard library, including `os.register_at_fork`, so module import succeeds and the handler's own return value is served. Open issue 20261105T083215Z-ec2-lambda-logs-route53resolver-the-lambda-python-runtime-has-a-crippled-stdlib.md reports the identical missing attribute for a Route 53 Resolver in-VPC DNS probe but does not carry this check's replay key.

Traces: elasticache__logs.slow-log-delivery-lands-the-engine-command-in-cloudwatch-logs, failing in `_prove_executor_in_vpc` -> `_invoke` on the executor self-test, before any slow-log or Logs assertion runs.

## Evidence
Expected: lambda Invoke of the VPC-attached stdlib RESP executor returns StatusCode 200, no FunctionError, and the probe's raw reply bytes; the check then issues a slow command and asserts it lands in the configured CloudWatch Logs group.
Actual (emulator): AssertionError: the in-VPC executor is unusable, so this run measured nothing: Invoke of sim2real-behav-invpc-fn-<id> returned StatusCode=200 FunctionError='Unhandled': b'{"errorMessage":"Unable to import module \'index\': module \'os\' has no attribute \'register_at_fork\'","errorType":"Runtime.ImportModuleError","requestId":"<per-run id>","stackTrace":[]}'

## Steps to Reproduce
1. Create a VPC with subnets and a security group, an ElastiCache cluster with a slow-log LogDeliveryConfiguration targeting a CloudWatch Logs group, and a python3.x Lambda with VpcConfig on the same subnets whose pure-stdlib handler opens a RESP socket.
2. lambda Invoke the function with the executor self-test payload.
3. Expected: StatusCode 200, no FunctionError, handler result. Observed: StatusCode 200 with FunctionError 'Unhandled' and {"errorType": "Runtime.ImportModuleError", "errorMessage": "Unable to import module 'index': module 'os' has no attribute 'register_at_fork'"}.
4. Minimal equivalent: a handler module containing `import os; os.register_at_fork(after_in_child=lambda: None)`.
