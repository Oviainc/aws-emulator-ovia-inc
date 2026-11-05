Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53profiles.dns-firewall-resolution-native-protocol-vpc-only
Issue-Shape-Id: com.amazonaws.route53profiles#Route53Profiles
Issue-Key: route53profiles.dns-firewall-resolution-native-protocol-vpc-only|dig#8|AssertionError|assert "FunctionError" not in resp, f"the in-VPC querier failed: {body!r}"
Issue-Commit: 9de1430d35a30df63dfe38bccf7c2cf4feacacf3

# route53profiles: the Lambda Python runtime's os module lacks register_at_fork, so the in-VPC DNS querier dies with Runtime.ImportModuleError and the Profile DNS-firewall data-plane check can never be observed

> _Note: this issue was identified against the code base at commit `9de1430d35a30df63dfe38bccf7c2cf4feacacf3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new root cause, reached by 1 check.

Cause 1 of 1: the emulator executes python3.12 Lambda handlers on a Python environment whose `os` module has no `register_at_fork`, so the handler module fails to import before any user code runs and Invoke answers a Runtime.ImportModuleError payload instead of the handler's result.

AWS: Smithy (services/service-com-amazonaws-lambda-awsgirapiservice, operation Invoke, POST /2015-03-31/functions/{FunctionName}/invocations, success code 200) returns StatusCode (httpResponseCode), FunctionError (header X-Amz-Function-Error) and Payload (httpPayload). For a valid python3.x ZipFile handler AWS returns StatusCode 200, NO FunctionError header, and the handler's own JSON payload. The managed python3.12 runtime is a complete CPython: `os.register_at_fork` has existed since CPython 3.7 and is used at import time by stdlib modules including `random` (`os.register_at_fork(after_in_child=...)`), so a handler whose module does nothing but `import random, socket, struct` imports cleanly. AWS only emits the FunctionError shape {"errorMessage": "Unable to import module '<module>': <import error>", "errorType": "Runtime.ImportModuleError", "stackTrace": []} with FunctionError 'Unhandled' when the import genuinely fails — the fixed text is "Unable to import module '<module>': " and the variable parts are the handler module name taken from the function's Handler and the underlying import error; nothing in the model sanctions a runtime with a partial standard library. The model says nothing further about the execution environment's stdlib, so that part is recorded behavior only.

Emulator: every Invoke of the VPC-attached querier function (Runtime python3.12, Handler index.handler, a stdlib-only DNS resolver) returns HTTP 200 with FunctionError set and the payload {"errorMessage": "Unable to import module 'index': module 'os' has no attribute 'register_at_fork'", "errorType": "Runtime.ImportModuleError", "stackTrace": []} (the requestId in the payload is a per-run value and is not part of the requirement). The emulator serves that reply, so the defect is the Python environment its Invoke path executes handlers in, not the handler code.

Why: a Lambda function with a VPC ENI is the only vantage point from which a client can issue a DNS query that originates inside the VPC, and Route 53's Resolver answers in-VPC sources only. With every such handler dead at import, a client can never observe whether a DNS Firewall BLOCK rule wired through a Route 53 Profile actually takes effect in the VPC: the whole control-plane chain (firewall domain list -> rule -> rule group -> AssociateResourceToProfile -> AssociateProfile onto the VPC) reports COMPLETE while the data-plane claim (NXDOMAIN for the blocked name, the control name still resolving) is unverifiable. More broadly any handler importing `random`, `subprocess`, `multiprocessing` or `concurrent.futures` is dead on arrival.

Fix: the Lambda Invoke execution environment (the python runtime sandbox the lambda service's Invoke handler runs user code in) must expose a complete CPython standard library — `os.register_at_fork` and the fork machinery that depends on it — so module import succeeds and Invoke returns StatusCode 200 with no FunctionError and the handler's own payload.

Traces: route53profiles.dns-firewall-resolution-native-protocol-vpc-only, which fails on its first dig() call, before any Profile/firewall assertion runs. Related open issues report the same runtime gap reached from other services and do NOT carry this check's replay key: 20261105T083215Z-ec2-lambda-logs-route53resolver-the-lambda-python-runtime-has-a-crippled-stdlib.md (same os.register_at_fork gap via Route 53 Resolver), 20261105T090541Z-elasticache-every-in-vpc-data-plane-check-is.md and 20261105T101629Z-kafka-every-native-msk-data-plane-check-is.md (same gap via ElastiCache/MSK in-VPC probes), and 20261105T074953Z-lambda-scheduler-the-python3-12-invoke-runtime-provides.md (the same runtime missing boto3, a different missing piece).

## Evidence
Expected (AWS): lambda Invoke of the python3.12 VPC-attached querier returns HTTP/StatusCode 200, no X-Amz-Function-Error header, and the handler's payload {"results": [{"query": "example.com.", "qtype": "A", "rcode": "NOERROR", "answers": [...]}, ...]}.

Actual (emulator): StatusCode 200 with FunctionError set and payload {'errorMessage': "Unable to import module 'index': module 'os' has no attribute 'register_at_fork'", 'errorType': 'Runtime.ImportModuleError', 'requestId': '<per-run id>', 'stackTrace': []}. Fixed text of the message: "Unable to import module '<module>': "; variable parts: the module name from the function's Handler ('index') and the underlying import error. The check surfaces it as:
  AssertionError: the in-VPC querier failed: {'errorMessage': "Unable to import module 'index': module 'os' has no attribute 'register_at_fork'", 'errorType': 'Runtime.ImportModuleError', ...}
raised at check_route53profiles_dns_firewall_resolution_native_protocol_vpc_only.py line 330 (`assert "FunctionError" not in resp`) from the pre-rule baseline dig() at line 43 — so the Profile-side assertions (resource association COMPLETE, VPC association COMPLETE, NXDOMAIN on the blocked domain, control domain still resolving) are never reached.

## Steps to Reproduce
1. Create a VPC and a subnet, and create an IAM role Lambda can assume with the VPC access execution policy.
2. lambda CreateFunction with Runtime=python3.12, Handler=index.handler, VpcConfig pointing at that subnet and the VPC's default security group, and a ZipFile whose index.py is a stdlib-only DNS resolver (its module does `import random`, `import socket`, `import struct`).
3. lambda Invoke the function with {"queries": [{"name": "example.com.", "type": "A"}]}.
4. Expected: StatusCode 200, no FunctionError, payload containing the DNS answers. Observed: StatusCode 200 with FunctionError and {"errorType": "Runtime.ImportModuleError", "errorMessage": "Unable to import module 'index': module 'os' has no attribute 'register_at_fork'"}.
5. Minimal equivalent handler module: `import os; os.register_at_fork(after_in_child=lambda: None)` — it fails to import on the emulator and imports fine on AWS python3.12.
