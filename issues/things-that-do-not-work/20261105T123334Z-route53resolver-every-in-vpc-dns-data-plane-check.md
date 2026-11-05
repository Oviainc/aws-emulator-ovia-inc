Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53resolver.firewall-block-response-arms-serve-three-distinct-answers route53resolver.firewall-rule-group-association-priority-decides-the-live-answer route53resolver.firewall-rule-qtype-scopes-the-block-to-that-query-type route53resolver.forward-rule-diverts-only-the-matched-domain-and-disassociation-restores-it route53resolver.system-rule-overrides-the-forward-rule-for-the-subdomain
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver.firewall-block-response-arms-serve-three-distinct-answers|dig#8|AssertionError|assert "FunctionError" not in r, f"the in-VPC querier failed: {body!r}"
Issue-Key: route53resolver.firewall-rule-group-association-priority-decides-the-live-answer|dig#7|AssertionError|assert "FunctionError" not in r, f"the in-VPC querier failed: {body!r}"
Issue-Key: route53resolver.firewall-rule-qtype-scopes-the-block-to-that-query-type|dig#6|AssertionError|assert "FunctionError" not in r, f"the in-VPC querier failed: {body!r}"
Issue-Key: route53resolver.forward-rule-diverts-only-the-matched-domain-and-disassociation-restores-it|dig#8|AssertionError|assert "FunctionError" not in r, f"the in-VPC querier failed: {body!r}"
Issue-Key: route53resolver.system-rule-overrides-the-forward-rule-for-the-subdomain|dig#6|AssertionError|assert "FunctionError" not in r, f"the in-VPC querier failed: {body!r}"
Issue-Commit: 9de1430d35a30df63dfe38bccf7c2cf4feacacf3

# route53resolver: every in-VPC DNS data-plane check is blocked — the Lambda Python runtime's os module lacks register_at_fork, so the DNS probe handler dies with Runtime.ImportModuleError

> _Note: this issue was identified against the code base at commit `9de1430d35a30df63dfe38bccf7c2cf4feacacf3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new root cause, reached by 5 checks listed under `### route53resolver`.

Cause 1 of 1: the Lambda Invoke path executes handlers on a Python runtime whose `os` module has no `register_at_fork`, so the in-VPC DNS querier module fails to import before any user code runs, and no Route 53 Resolver data-plane claim can be observed.

AWS: Invoke on a python3.x function returns HTTP 200 with StatusCode 200, no FunctionError, and the handler's own payload. The managed Lambda Python runtime is a complete CPython: `os.register_at_fork` has existed since CPython 3.7, so any module that touches fork machinery (subprocess, multiprocessing, concurrent.futures, and anything importing them — e.g. a socket/struct DNS probe helper) imports cleanly. AWS only answers the import-failure shape when the import genuinely fails: FunctionError 'Unhandled' with a payload whose errorType is `Runtime.ImportModuleError` and whose errorMessage is the fixed text `Unable to import module '<module>': ` followed by the underlying import error — the module name and the trailing cause are values taken from the request's handler configuration and the real failure, not fixed text. Smithy (services/service-com-amazonaws-lambda-awsgirapiservice): Invoke returns StatusCode/FunctionError/Payload; nothing in the model sanctions a runtime with a partial standard library. For Route 53 Resolver itself, recorded behavior only: the VPC resolver at 169.254.169.253 answers in-VPC sources, so the firewall/forward-rule effects are only observable from a function inside the VPC.

Emulator: every invocation of the in-VPC querier returns the payload {"errorMessage": "Unable to import module 'index': module 'os' has no attribute 'register_at_fork'", "errorType": "Runtime.ImportModuleError", "stackTrace": []} with a FunctionError set. The emulator serves that reply, so the defect is the Python environment its Invoke path runs handlers in — the import error is manufactured by the emulator's own incomplete stdlib, not by the handler.

Why: a client that stands up a VPC-attached function as its only vantage point inside the VPC can never run it, so every Route 53 Resolver behavior that is only visible from inside a VPC is unobservable: which of three BLOCK response arms (NODATA/NXDOMAIN/OVERRIDE) is served, which association priority decides the live answer, whether a rule's Qtype scopes the block to that query type only, whether a forward rule diverts only the matched domain and whether disassociation restores resolution, and whether a system rule overrides a forward rule for a subdomain. More generally, any customer handler importing subprocess/multiprocessing — or any module that does — is dead on arrival.

Fix: the Lambda invoke runtime (the Python sandbox/runtime image the lambda service's Invoke handler executes functions in) must supply a complete CPython standard library, including os.register_at_fork and the fork/subprocess machinery that depends on it, so module import succeeds and the handler's own result is returned in Payload with no FunctionError.

Traces: the 5 checks in trace_ids, all failing with the identical Runtime.ImportModuleError on os.register_at_fork raised from each check's dig()/_poll_dns helper before any resolver assertion runs. Related open issues report the same runtime missing other pieces but do NOT carry these checks' replay keys: 20261105T083215Z-ec2-lambda-logs-route53resolver-the-lambda-python-runtime-has-a-crippled-stdlib.md (same missing attribute, but keyed only to the 9 ec2__lambda__logs__route53resolver checks), 20261105T083215Z-ec2-lambda-route53-the-lambda-python-runtime-has-no-socket-extension.md, and 20261105T074953Z-lambda-scheduler-the-python3-12-invoke-runtime-provides.md (missing boto3).

## Evidence
Expected: lambda Invoke of the VPC-attached DNS querier returns HTTP 200, StatusCode 200, no FunctionError, and a Payload containing the handler's result ({"src_ip": ..., "results": [{"query": ..., "rcode": "NOERROR"|"NXDOMAIN"|..., "answers": [...]}]}), so the check can then assert the resolver's answer.

Actual (emulator), byte-identical across all 5 checks: the Invoke response carries a FunctionError and the payload
{'errorMessage': "Unable to import module 'index': module 'os' has no attribute 'register_at_fork'", 'errorType': 'Runtime.ImportModuleError', 'requestId': '<per-run id>', 'stackTrace': []}
raised as `AssertionError: the in-VPC querier failed: {...}` from each check's dig() helper (e.g. check_route53resolver_firewall_rule_qtype_scopes_the_block_to_that_query_type.py line 281, check_route53resolver_system_rule_overrides_the_forward_rule_for_the_subdomain.py line 302), before any firewall/forward-rule assertion executes. The fixed text here is `Unable to import module '<module>': ` + the import error; 'index' is this check's handler module and the requestId is per-run, so neither is part of the requirement. The requirement is that no import error is produced at all.

## Steps to Reproduce
1. Create a VPC with a private subnet and DNS hostnames enabled, plus a Route 53 Resolver DNS-firewall rule group (or forward rule) associated with that VPC.
2. Create a python3.x Lambda function with VpcConfig in that subnet whose handler module performs DNS queries against 169.254.169.253 (a module that pulls in fork/subprocess machinery, as a dig-style helper does).
3. Call lambda Invoke with the query payload.
4. Expected: StatusCode 200, no FunctionError, and the handler's DNS results in Payload. Observed: FunctionError 'Unhandled' with {"errorType": "Runtime.ImportModuleError", "errorMessage": "Unable to import module 'index': module 'os' has no attribute 'register_at_fork'"}.
5. Minimal equivalent: a handler whose module body does `import os; os.register_at_fork(after_in_child=lambda: None)` — it fails to import on the emulator and imports cleanly on AWS.
