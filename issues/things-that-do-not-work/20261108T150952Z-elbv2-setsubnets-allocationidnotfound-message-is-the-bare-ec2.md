Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.set-subnets-allocation-id-not-found-fails elbv2.drop-invalid-header-fields-strips-the-header-before-the-target elbv2.desync-mitigation-strictest-blocks-ambiguous-framing-before-the-target
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2.set-subnets-allocation-id-not-found-fails|run#13|AssertionError|assert message.startswith(
Issue-Key: elbv2.drop-invalid-header-fields-strips-the-header-before-the-target|_poll_until#25|Timeout|time.sleep(interval_s)
Issue-Key: elbv2.desync-mitigation-strictest-blocks-ambiguous-framing-before-the-target|_poll_until#22|Timeout|time.sleep(interval_s)
Issue-Commit: 85304fbaaec1e137da409d02b027804fcd09632b

# elbv2: SetSubnets AllocationIdNotFound message is the bare EC2 error code, and the two load-balancer-attribute data-plane behaviours (drop_invalid_header_fields, desync_mitigation_mode=strictest) never converge (3 causes)

> _Note: this issue was identified against the code base at commit `85304fbaaec1e137da409d02b027804fcd09632b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetSubnets

## Description
Three net-new root causes in the elbv2 emulator. Each is independent and must be closed separately.

Cause 1 of 3: SetSubnets refuses an unknown EIP allocation id with the right code but the wrong message.
AWS: Smithy (com.amazonaws.elasticloadbalancingv2): SetSubnets declares AllocationIdNotFoundException, awsQueryError code `AllocationIdNotFound`, HTTP 400. Recorded behavior for the message: ELBv2 forwards the EC2 lookup failure verbatim, so the message is the fixed frame "The allocation ID '<allocation-id>' does not exist (Service: AmazonEC2; Status Code: 400; Error Code: InvalidAllocationID.NotFound; Request ID: <request-id>; Proxy: null)". Its fixed text is everything outside the angle brackets; the variable parts are the allocation id taken from the request's SubnetMappings[].AllocationId and the per-call request id of the internal EC2 lookup (a per-run value, never a requirement). The refused call must also leave the load balancer's existing EIP mapping untouched.
Emulator: answers HTTP 400 `AllocationIdNotFound` (code correct) with the message reduced to the bare code string "InvalidAllocationID.NotFound" — none of AWS's sentence, no allocation id, no service/status/request-id frame.
Why: a client that surfaces or parses the ELBv2 message cannot tell which allocation id was rejected, and message assertions / log scrapers that expect AWS's sentence break. Note the inverse direction was filed and fixed for CreateLoadBalancer/SubnetNotFound in the resolved issue 20261104T094656Z-elbv2-ten-wrong-error-code-wrong-error-message.md (its Cause 10 required the short text "InvalidSubnetID.NotFound" for SubnetNotFound); that fix appears to have been generalised to every EC2-sourced refusal, which is wrong here — SetSubnets/AllocationIdNotFound carries the long EC2-framed sentence. That issue is closed and does not cover SetSubnets.
Fix: the SetSubnets handler's EIP-allocation resolution branch (error message construction), which must emit the EC2-framed sentence for AllocationIdNotFound rather than the bare error code.
Traces: elbv2.set-subnets-allocation-id-not-found-fails.

Cause 2 of 3: with routing.http.drop_invalid_header_fields.enabled the ALB data plane never serves the attribute's effect, so the check exhausts its whole budget.
AWS: recorded behavior only (documented at https://docs.aws.amazon.com/elasticloadbalancing/latest/application/application-load-balancers.html#load-balancer-attributes): an internet-facing ALB with an HTTP:80 listener forwarding to a lambda-type target group invokes the Lambda target and answers 200 with the target's response; with routing.http.drop_invalid_header_fields.enabled=false both a header whose name contains an underscore (x_sim2real_bad) and a valid header (X-Sim2Real-Good) reach the target, and with it =true the underscore header is removed from the request the target sees while the valid one still arrives and the response is still 200. The recorded run of this sequence completes inside the suite's 60s budget for the check.
Emulator: the check never reaches a verdict — "Check timed out after 60.0s". The control-plane legs (ModifyLoadBalancerAttributes refusal of a non-boolean value, DescribeLoadBalancerAttributes read-back) are the early part of the same check and the previously-resolved issue 20261104T094656Z-elbv2-ten-wrong-error-code-wrong-error-message.md already listed this check for its Cause 1, so the sequence now stalls after them, in the part that needs the ALB to answer through the lambda target (`_poll_echo` on http://<lb-dns>/ every 5s) and then to reflect the attribute change. Hypothesis: the ALB data plane never answers for a lambda-type target group (or never applies the attribute), so the bounded poll runs until the budget is gone.
Why: a client cannot verify or rely on the attribute at all — the ALB endpoint never produces the request the target would have seen, so header-sanitising behaviour that real ALBs guarantee (and that callers depend on for request-smuggling hygiene) is simply absent.
Fix: the ALB data-plane request path for lambda-type target groups plus the application of the routing.http.drop_invalid_header_fields.enabled load-balancer attribute to the forwarded request (listener/forward handling and the attribute's data-plane effect).
Traces: elbv2.drop-invalid-header-fields-strips-the-header-before-the-target.

Cause 3 of 3: routing.http.desync_mitigation_mode is stored but has no data-plane effect, so the strictest-mode block of an ambiguously framed request never happens.
AWS: recorded behavior only: with routing.http.desync_mitigation_mode=monitor an ALB forwards both a compliant POST and a POST carrying both `Content-Length` and `Transfer-Encoding: chunked` to the target (the target is invoked and answers 200); after setting the mode to strictest the same ambiguous bytes are answered by the load balancer itself with the status line "HTTP/1.1 400 Bad Request" and the target is never invoked, while the compliant POST still reaches it. The recorded run completes inside the suite's 180s budget.
Emulator: the check never reaches a verdict — "Check timed out after 180.0s". The control plane accepts and reads back the mode (the check asserts that before touching the data plane), so the stall is in the raw-socket legs that poll (10s interval) for the load balancer to forward and then to refuse the ambiguous request. Hypothesis: the ALB data plane neither forwards the raw POST to the lambda target nor applies desync mitigation, so neither the monitor leg nor the strictest leg ever converges.
Why: request-smuggling mitigation is a security control; an emulator that stores the attribute but never blocks ambiguous framing lets a client validate a configuration that does not protect anything, and the 400-before-the-target contract is untestable.
Fix: the ALB data-plane HTTP request parser/forwarder — enforce routing.http.desync_mitigation_mode (monitor vs strictest) when framing headers conflict, answering 400 without invoking the target under strictest.
Traces: elbv2.desync-mitigation-strictest-blocks-ambiguous-framing-before-the-target.

## Evidence
Cause 1: `AssertionError: InvalidAllocationID.NotFound` raised by `assert message.startswith(f"The allocation ID '{fake_allocation_id}' does not exist (Service: AmazonEC2; Status Code: 400; Error Code: InvalidAllocationID.NotFound; Request ID: ") and message.endswith("; Proxy: null)")` in check_elbv2_set_subnets_allocation_id_not_found_fails.py line 44 — the assertion's failure text is the message the emulator actually returned, i.e. the whole message was the string "InvalidAllocationID.NotFound". The preceding assertion on the error code (`code == "AllocationIdNotFound"`) passed, so only the message diverges. (The allocation id eipalloc-0123456789abcdef0 and the request id are per-run values.)

Cause 2: `Error: Check timed out after 60.0s` for elbv2.drop-invalid-header-fields-strips-the-header-before-the-target — no assertion was reached; the check's own data-plane poll is bounded at 240s with a 5s interval, so the harness budget expired first.

Cause 3: `Error: Check timed out after 180.0s` for elbv2.desync-mitigation-strictest-blocks-ambiguous-framing-before-the-target — no assertion was reached; the check's data-plane polls are bounded at 420s/300s with a 10s interval, so the harness budget expired first. Both timeouts are from the same replay run in which the control-plane elbv2 checks did answer, so the emulator was serving.

## Steps to Reproduce
Replay the behavioral suite and re-run exactly the checks in trace_ids:
1. Cause 1 — create an internet-facing network load balancer with two SubnetMappings each carrying a real EIP AllocationId, wait for it active, then call SetSubnets with one mapping's AllocationId replaced by an unknown id (e.g. eipalloc-0123456789abcdef0). Expect HTTP 400 `AllocationIdNotFound` whose message is "The allocation ID '<that id>' does not exist (Service: AmazonEC2; Status Code: 400; Error Code: InvalidAllocationID.NotFound; Request ID: <id>; Proxy: null)", and expect DescribeLoadBalancers to still report the original allocation ids.
2. Cause 2 — create an internet-facing ALB with an open SG, a lambda-type target group registering an echo Lambda, and an HTTP:80 listener forwarding to it; GET http://<lb-dns>/ with headers {x_sim2real_bad, X-Sim2Real-Good}. Expect 200 with both headers echoed; then ModifyLoadBalancerAttributes routing.http.drop_invalid_header_fields.enabled=true and expect 200 with x_sim2real_bad absent from the echoed headers and X-Sim2Real-Good present.
3. Cause 3 — same ALB/lambda-target topology with an SQS side channel per invocation; with routing.http.desync_mitigation_mode=monitor send a raw POST carrying both Content-Length: 8 and Transfer-Encoding: chunked and expect 200 plus one side-channel message; set the mode to strictest and send the identical bytes, expecting the status line "HTTP/1.1 400 Bad Request" and no side-channel message, while a compliant POST is still forwarded.
