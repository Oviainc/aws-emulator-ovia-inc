Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__wafv2.wafv2-body-inspection-stops-at-the-oversize-limit-and-oversize-handling-decides-past-it
Issue-Key: apigateway__wafv2.wafv2-body-inspection-stops-at-the-oversize-limit-and-oversize-handling-decides-past-it|_poll#32|Timeout|time.sleep(interval_s)
Issue-Commit: c8d80ea15f1454d07f388eb2b932b3a5430349f4

# WAFv2 at the edge: the oversize-body inspection check never completes — the protected stage invocation never returns

> _Note: this issue was identified against the code base at commit `c8d80ea15f1454d07f388eb2b932b3a5430349f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSampledRequests

## Description
Cause 1 of 1: a request to a stage protected by a WebACL whose body-inspection rule has an OversizeHandling setting never produces an HTTP response.
AWS: WAFv2 inspects a request body only up to the body-inspection limit and then applies the rule's OversizeHandling (CONTINUE/MATCH/NO_MATCH) to decide the request past that limit; the protected API Gateway stage answers the request either way — 200 when allowed and 403 with the WebACL's block action when refused — and GetSampledRequests records the sampled request.
Emulator: the check runs to the harness deadline and is killed with 'Error: Check timed out after 60.0s'. No status, no headers and no body are produced, so neither the at-limit decision nor the past-limit OversizeHandling decision can be observed.
Why: a client calling a WAF-protected stage blocks until its own timeout, and the oversize-handling semantics the WebACL configures are unreachable.
Fix: the execute-api invoke pipeline where the WAFv2 web-ACL evaluation runs (the request must still be answered when the body exceeds the inspection limit).
Traces: apigateway__wafv2.wafv2-body-inspection-stops-at-the-oversize-limit-and-oversize-handling-decides-past-it

Note: the open issues 20261029T041807Z-apigateway-wafv2-three-more-wafv2-at-the-edge.md and 20261025T092822Z-apigateway-wafv2-six-wafv2-at-the-edge-checks.md cover other WAFv2-at-the-edge checks; neither carries this replay key.

## Evidence
Replay output, verbatim: 'Error: Check timed out after 60.0s' for apigateway__wafv2.wafv2-body-inspection-stops-at-the-oversize-limit-and-oversize-handling-decides-past-it. The emulator returned no HTTP response for the protected stage invocation, so the expected allow/block statuses could not be compared.

## Steps to Reproduce
1. Create a REST API with a method and an integration, deploy it to a stage.
2. Create a WAFv2 WebACL (scope REGIONAL) with a ByteMatchStatement inspecting the request Body and an OversizeHandling setting, and associate it with the stage.
3. Invoke the stage with a body at the inspection limit and then with a body past it. Each call never returns; the suite kills the check at 60s.
4. Replay the check id in trace_ids.
