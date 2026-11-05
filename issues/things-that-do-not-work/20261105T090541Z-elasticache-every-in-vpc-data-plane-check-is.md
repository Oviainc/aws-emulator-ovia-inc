Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.elasticache-auth-token-required-cluster-rejects-unauthenticated-command elasticache.elasticache-lua-eval-atomicity-and-script-cache elasticache.elasticache-redis-roundtrip-in-vpc elasticache.elasticache-revoked-security-group-ingress-blocked-from-cache-data-plane elasticache.elasticache-streams-consumer-group-pending-and-autoclaim elasticache.slowlog-threshold-parameter-governs-what-the-engine-records elasticache.transit-encryption-endpoint-refuses-plaintext-resp-in-vpc
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache.elasticache-auth-token-required-cluster-rejects-unauthenticated-command|_invoke#2|AssertionError|raise _no_executor(
Issue-Key: elasticache.elasticache-lua-eval-atomicity-and-script-cache|_invoke#2|AssertionError|raise _no_executor(
Issue-Key: elasticache.elasticache-redis-roundtrip-in-vpc|_invoke#10|AssertionError|raise _no_executor(
Issue-Key: elasticache.elasticache-revoked-security-group-ingress-blocked-from-cache-data-plane|_invoke#3|AssertionError|raise _no_executor(
Issue-Key: elasticache.elasticache-streams-consumer-group-pending-and-autoclaim|_invoke#2|AssertionError|raise _no_executor(
Issue-Key: elasticache.slowlog-threshold-parameter-governs-what-the-engine-records|_invoke#3|AssertionError|raise _no_executor(
Issue-Key: elasticache.transit-encryption-endpoint-refuses-plaintext-resp-in-vpc|_invoke#2|AssertionError|raise _no_executor(
Issue-Commit: 6da489cbc4c2969d809761cd42ffd786edefa992

# elasticache: every in-VPC data-plane check is blocked — the Lambda Python runtime's os module lacks register_at_fork, so the RESP probe handler dies with Runtime.ImportModuleError

> _Note: this issue was identified against the code base at commit `6da489cbc4c2969d809761cd42ffd786edefa992`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new root cause, reached by 7 elasticache checks.

Cause 1 of 1: the Lambda invoke runtime the emulator executes handlers on is an incomplete CPython — its `os` module has no `register_at_fork` — so the in-VPC RESP probe handler module fails to import and no ElastiCache data-plane command is ever issued.

AWS: Invoke of a python3.x function returns HTTP 200 with the handler's payload and NO FunctionError header; the managed runtime is a complete CPython, where `os.register_at_fork` has existed since CPython 3.7, so stdlib modules that touch it (`random`, `subprocess`, `multiprocessing`, `concurrent.futures`, and anything importing them) import cleanly. A real import failure is reported as FunctionError 'Unhandled' with a payload whose fixed text is `Unable to import module '<module>': ` followed by the underlying import error, and errorType `Runtime.ImportModuleError` — correct shape, but only legitimate when the import genuinely fails. Smithy (lambda, Invoke): the response carries StatusCode / FunctionError / Payload; nothing in the model sanctions a runtime with a partial standard library. The ElastiCache side is recorded behavior only: the cache endpoint is VPC-only, so the recording reaches it from a VPC-attached Lambda.

Emulator: every Invoke of the pure-stdlib in-VPC executor returns StatusCode=200 with FunctionError='Unhandled' and body {"errorMessage": "Unable to import module 'index': module 'os' has no attribute 'register_at_fork'", "errorType": "Runtime.ImportModuleError", "requestId": "<per-run id>", "stackTrace": []}. Here the fixed text is `Unable to import module 'index': module 'os' has no attribute 'register_at_fork'`; the requestId is a per-run value and not part of the requirement. The emulator serves that reply, so the defect is the Python environment it executes handlers in, not the check's code.

Why: the VPC-attached Lambda is the only vantage point from which an ElastiCache endpoint can be reached, so a client gets no observable data plane at all. Every ElastiCache claim that depends on speaking RESP to the cache is unobservable: AUTH-required clusters rejecting unauthenticated commands, EVAL/SCRIPT atomicity and the script cache, basic SET/GET round-trip in the VPC, security-group ingress revocation actually blocking the data plane, stream consumer-group PENDING/XAUTOCLAIM semantics, the slowlog threshold parameter governing what the engine records, and a transit-encryption endpoint refusing plaintext RESP. More generally any customer handler importing `random`/`subprocess`/`multiprocessing` is dead on arrival.

Fix: the Lambda invoke runtime (the python sandbox the lambda service's Invoke path executes handler modules in) must expose a complete CPython standard library — `os.register_at_fork` and the fork machinery depending on it — so module import succeeds and the handler's own return value is served. Open issue 20261105T083215Z-ec2-lambda-logs-route53resolver-the-lambda-python-runtime-has-a-crippled-stdlib.md reports the identical missing attribute for Route 53 Resolver's in-VPC DNS probe, and 20261105T074953Z-lambda-scheduler-the-python3-12-invoke-runtime-provides.md reports the same runtime missing boto3; neither carries these elasticache checks' replay keys, so they are filed here for re-replay.

Traces: the 7 checks in trace_ids, all failing identically in `_prove_executor_in_vpc` -> `_invoke` on the executor self-test, before any ElastiCache assertion runs.

## Evidence
Expected: lambda Invoke of the VPC-attached stdlib RESP executor returns StatusCode 200, no FunctionError, and the probe's raw reply bytes in the payload; the check then asserts on those bytes.
Actual (emulator), identical across all 7 checks: AssertionError: the in-VPC executor is unusable, so this run measured nothing: Invoke of sim2real-behav-invpc-fn-<id> returned StatusCode=200 FunctionError='Unhandled': b'{"errorMessage":"Unable to import module \'index\': module \'os\' has no attribute \'register_at_fork\'","errorType":"Runtime.ImportModuleError","requestId":"<per-run id>","stackTrace":[]}'
Raised from each check's _prove_executor_in_vpc/_invoke helper on the executor self-test invoke, i.e. no RESP command ever reached the cache.

## Steps to Reproduce
1. Create a VPC with subnets and a security group, an ElastiCache cluster/replication group on them, and a python3.x Lambda function with VpcConfig on the same subnets whose handler module is pure stdlib (it imports `random`, or directly calls `os.register_at_fork`).
2. lambda Invoke the function with the executor self-test payload.
3. Expected: StatusCode 200, no FunctionError, and the handler's result. Observed: StatusCode 200 with FunctionError 'Unhandled' and {"errorType": "Runtime.ImportModuleError", "errorMessage": "Unable to import module 'index': module 'os' has no attribute 'register_at_fork'"}.
4. Minimal equivalent: a handler module containing `import os; os.register_at_fork(after_in_child=lambda: None)` — it fails to import.
