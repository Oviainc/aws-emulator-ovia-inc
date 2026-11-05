Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__lambda__route53.route53-private-hosted-zone-name-resolves-only-from-inside-the-associated-vpc ec2__lambda__route53.route53-private-zone-name-rejected-with-nxdomain-after-the-vpc-is-disassociated
Issue-Key: ec2__lambda__route53.route53-private-hosted-zone-name-resolves-only-from-inside-the-associated-vpc|_invoke#16|AssertionError|assert not resp.get("FunctionError") and resp.get("StatusCode") == 200, (
Issue-Key: ec2__lambda__route53.route53-private-zone-name-rejected-with-nxdomain-after-the-vpc-is-disassociated|_invoke#18|AssertionError|assert not resp.get("FunctionError") and resp.get("StatusCode") == 200, (
Issue-Commit: 506f51337b02a9b5673b5526bcfdc40d4e5c4821

# The Lambda Python runtime has no _socket extension module, so the in-VPC DNS vantage-point handler dies with Runtime.ImportModuleError and no private-hosted-zone resolution can be observed

> _Note: this issue was identified against the code base at commit `506f51337b02a9b5673b5526bcfdc40d4e5c4821`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new root cause, reached by 2 checks.

Cause 1 of 1: lambda Invoke executes the handler on a Python runtime that cannot import _socket, so the handler module fails to import and the function never runs.

AWS: Invoke on a python3.x function returns StatusCode 200 with no FunctionError and the handler's payload; the managed runtime is a complete CPython whose socket module (backed by the built-in _socket extension) is importable, so a handler resolving a name from inside the VPC works. A genuine import failure is reported as FunctionError 'Unhandled' with errorType Runtime.ImportModuleError and message "Unable to import module '<module>': <import error>". Smithy (services/service-com-amazonaws-lambda-awsgirapiservice): Invoke returns StatusCode/FunctionError/Payload — nothing sanctions a runtime missing core extension modules.

Emulator: every invocation of the DNS vantage-point function returns StatusCode=200 with FunctionError='Unhandled' and the body {"errorMessage": "Unable to import module 'index': No module named '_socket'", "errorType": "Runtime.ImportModuleError", "stackTrace": []}. The emulator serves that reply, so the defect is the runtime it provides.

Why: the VPC-attached function is the only place a private hosted zone can be queried from, so both route53 claims it carries are unobservable: that a private zone's name resolves only from inside an associated VPC, and that it answers NXDOMAIN once the VPC is disassociated. Any customer handler that opens a socket — HTTP clients, DNS lookups, database drivers — fails identically.

Fix: the Lambda invoke runtime (the python runtime image/sandbox the lambda service's Invoke path executes handlers in) must ship the CPython built-in extension modules, _socket among them, so `import socket` and any handler depending on it imports cleanly and the handler's own result is returned.

Traces: the 2 checks in trace_ids, both failing with the identical "No module named '_socket'" import error. Open issue 20261105T074953Z-lambda-scheduler-the-python3-12-invoke-runtime-provides.md reports the same runtime missing boto3; this is a distinct missing component of that runtime and no open issue carries these checks' replay keys.

## Evidence
Expected: the VPC-attached vantage-point function executes and returns its resolution result (StatusCode 200, no FunctionError).
Actual (emulator), identical in both checks: AssertionError: the DNS vantage point sim2real-behav-r53dns-fn-<id> failed to execute: StatusCode=200 FunctionError='Unhandled': b'{"errorMessage":"Unable to import module \'index\': No module named \'_socket\'","errorType":"Runtime.ImportModuleError","requestId":"<per-run id>","stackTrace":[]}' — raised from _resolver_reachable before any private-hosted-zone assertion runs.

## Steps to Reproduce
1. Create a VPC with a subnet, a Route 53 private hosted zone associated with it holding an A record, and a python3.x Lambda function with VpcConfig in that subnet whose handler resolves a name (imports socket).
2. lambda Invoke the function with the name to resolve.
3. Expected: StatusCode 200, no FunctionError, and the resolved address in the payload. Observed: StatusCode=200 with FunctionError='Unhandled' and {"errorType": "Runtime.ImportModuleError", "errorMessage": "Unable to import module 'index': No module named '_socket'"}.
4. Minimal equivalent: a handler whose module does `import socket` — it fails to import.
