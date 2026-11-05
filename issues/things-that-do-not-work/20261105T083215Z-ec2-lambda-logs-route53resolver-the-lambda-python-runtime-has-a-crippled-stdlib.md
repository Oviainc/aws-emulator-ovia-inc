Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__lambda__logs__route53resolver.concurrent-firewall-rules-at-one-priority-admit-exactly-one-and-it-takes-effect ec2__lambda__logs__route53resolver.deleting-a-firewall-domain-list-in-use-rejected-and-it-keeps-blocking ec2__lambda__logs__route53resolver.disabling-autodefined-reverse-lookup-stops-the-reverse-answer ec2__lambda__logs__route53resolver.dns-firewall-block-override-returns-the-configured-override-record ec2__lambda__logs__route53resolver.dns-firewall-block-rule-actually-blocks-resolution-inside-the-vpc ec2__lambda__logs__route53resolver.firewall-alert-action-logs-the-match-without-blocking-the-answer ec2__lambda__logs__route53resolver.firewall-block-takes-precedence-over-the-private-hosted-zone ec2__lambda__logs__route53resolver.firewall-rule-priority-decides-which-action-the-resolver-applies ec2__lambda__logs__route53resolver.query-log-config-records-the-query-the-instance-actually-made
Issue-Key: ec2__lambda__logs__route53resolver.concurrent-firewall-rules-at-one-priority-admit-exactly-one-and-it-takes-effect|dig#2|AssertionError|assert "FunctionError" not in r, f"the in-VPC querier failed: {body!r}"
Issue-Key: ec2__lambda__logs__route53resolver.deleting-a-firewall-domain-list-in-use-rejected-and-it-keeps-blocking|dig#2|AssertionError|assert "FunctionError" not in r, f"the in-VPC querier failed: {body!r}"
Issue-Key: ec2__lambda__logs__route53resolver.disabling-autodefined-reverse-lookup-stops-the-reverse-answer|dig#2|AssertionError|assert "FunctionError" not in r, f"the in-VPC querier failed: {body!r}"
Issue-Key: ec2__lambda__logs__route53resolver.dns-firewall-block-override-returns-the-configured-override-record|dig#2|AssertionError|assert "FunctionError" not in r, f"the in-VPC querier failed: {body!r}"
Issue-Key: ec2__lambda__logs__route53resolver.dns-firewall-block-rule-actually-blocks-resolution-inside-the-vpc|dig#2|AssertionError|assert "FunctionError" not in r, f"the in-VPC querier failed: {body!r}"
Issue-Key: ec2__lambda__logs__route53resolver.firewall-alert-action-logs-the-match-without-blocking-the-answer|dig#2|AssertionError|assert "FunctionError" not in r, f"the in-VPC querier failed: {body!r}"
Issue-Key: ec2__lambda__logs__route53resolver.firewall-block-takes-precedence-over-the-private-hosted-zone|dig#2|AssertionError|assert "FunctionError" not in r, f"the in-VPC querier failed: {body!r}"
Issue-Key: ec2__lambda__logs__route53resolver.firewall-rule-priority-decides-which-action-the-resolver-applies|dig#2|AssertionError|assert "FunctionError" not in r, f"the in-VPC querier failed: {body!r}"
Issue-Key: ec2__lambda__logs__route53resolver.query-log-config-records-the-query-the-instance-actually-made|dig#2|AssertionError|assert "FunctionError" not in r, f"the in-VPC querier failed: {body!r}"
Issue-Commit: 506f51337b02a9b5673b5526bcfdc40d4e5c4821

# The Lambda Python runtime has a crippled stdlib (os.register_at_fork missing), so every in-VPC DNS probe handler dies with Runtime.ImportModuleError and all 9 Route 53 Resolver DNS-firewall checks are blocked

> _Note: this issue was identified against the code base at commit `506f51337b02a9b5673b5526bcfdc40d4e5c4821`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new root cause, reached by 9 checks.

Cause 1 of 1: lambda Invoke runs the handler on a Python runtime whose `os` module lacks register_at_fork, so importing the handler module fails before any user code runs.

AWS: Invoke on a python3.x function returns StatusCode 200 with the handler's payload and no FunctionError; the managed runtime is a complete CPython — os.register_at_fork has existed since CPython 3.7, so modules that touch it (subprocess/multiprocessing/concurrent.futures, and anything importing them, e.g. a dig-style DNS probe) import cleanly. A genuine import failure is reported as FunctionError 'Unhandled' with errorType Runtime.ImportModuleError and message "Unable to import module '<module>': <import error>" — that shape is only correct when the import really fails. Smithy (services/service-com-amazonaws-lambda-awsgirapiservice): Invoke returns StatusCode/FunctionError/Payload; nothing in the model sanctions a runtime with a partial standard library.

Emulator: every invocation of the in-VPC querier function returns the error payload {"errorMessage": "Unable to import module 'index': module 'os' has no attribute 'register_at_fork'", "errorType": "Runtime.ImportModuleError", "stackTrace": []}. The emulator serves that reply, so the defect is the Python environment it executes handlers in.

Why: the Lambda function is the only vantage point inside the VPC, so DNS resolution from inside a VPC cannot be observed at all. Every Route 53 Resolver claim that depends on an in-VPC query is unobservable: DNS-firewall BLOCK actually blocking, BLOCK override records, ALERT logging without blocking, rule-priority precedence, firewall-vs-private-hosted-zone precedence, autodefined reverse lookup, in-use domain-list delete, concurrent same-priority rule creation, and query logging. More generally, any customer handler importing subprocess/multiprocessing or socket-using libraries is dead on arrival.

Fix: the Lambda invoke runtime (the python runtime image/sandbox the lambda service's Invoke path executes handlers in) must provide a complete CPython standard library — os.register_at_fork and the fork/subprocess machinery depending on it — so module import succeeds and the handler's own result is returned.

Traces: the 9 checks in trace_ids, all failing with the identical Runtime.ImportModuleError on os.register_at_fork. Open issue 20261105T074953Z-lambda-scheduler-the-python3-12-invoke-runtime-provides.md reports the same runtime missing the AWS SDK (boto3); this is a different missing piece of the same runtime and no open issue carries these checks' replay keys.

## Evidence
Expected: lambda Invoke of the VPC-attached querier returns StatusCode 200, no FunctionError, and the handler's DNS answers in the payload.
Actual (emulator), identical across all 9 checks: AssertionError: the in-VPC querier failed: {'errorMessage': "Unable to import module 'index': module 'os' has no attribute 'register_at_fork'", 'errorType': 'Runtime.ImportModuleError', 'requestId': '<per-run id>', 'stackTrace': []} — raised from each check's _poll_dns/_claim helper on the first dig() call, before any firewall/query-log assertion runs.

## Steps to Reproduce
1. Create a VPC with a private subnet, a Route 53 Resolver DNS-firewall rule group associated to it, and a python3.x Lambda function with VpcConfig in that subnet whose handler module performs DNS queries (imports a dig-style helper that pulls in subprocess).
2. lambda Invoke the function with the query payload.
3. Expected: StatusCode 200, no FunctionError, and the handler's DNS answers. Observed: FunctionError 'Unhandled' with {"errorType": "Runtime.ImportModuleError", "errorMessage": "Unable to import module 'index': module 'os' has no attribute 'register_at_fork'"}.
4. Minimal equivalent: a handler whose module does `import os; os.register_at_fork(after_in_child=lambda: None)` — it fails to import.
