Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2__logs__wafv2.log-edge-producers-waf-log-records-rule-group-blocked-and-default-allowed-requests
Issue-Key: elbv2__logs__wafv2.log-edge-producers-waf-log-records-rule-group-blocked-and-default-allowed-requests|_raw_get#41|PublishedUrlRejected|with socket.create_connection(ctx.endpoint_host(host, 80), timeout=20) as sock:
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# The WAF-protected ALB's endpoint host is published as <name>.<region>.elb.amazonaws.com:80, so no request can be sent through WAF

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLoadBalancer

## Description
Cause 1 of 1: the load balancer announces an AWS production hostname rather than the address the emulator serves.
AWS: requests sent to the ALB's DNSName pass through the associated WAF web ACL, and the recorded run compares the rule-group-blocked record with the default-allowed record in the WAF log destination.
Emulator: the endpoint host is published as 'sim2real-lel-<id>-<id>.us-west-2.elb.amazonaws.com:80' — 'its host is not one of the emulator's own hosts; a customer of the emulator needs an own host[:port]' — so no request can be made and no WAF log record is produced.
Why: a client cannot exercise a WAF-protected ALB at all, so blocked/allowed logging is unobservable.
Fix: the DNSName/endpoint-host serializer in the elbv2 crate.
Traces: elbv2__logs__wafv2.log-edge-producers-waf-log-records-rule-group-blocked-and-default-allowed-requests.

## Evidence
'simparity.published.PublishedUrlRejected: endpoint_host was given sim2real-lel-<id>-<id>.us-west-2.elb.amazonaws.com:80 (published by the emulator or built by the check): its host is not one of the emulator's own hosts'.

## Steps to Reproduce
Replay elbv2__logs__wafv2.log-edge-producers-waf-log-records-rule-group-blocked-and-default-allowed-requests: create the ALB and web ACL, read the ALB's endpoint host and send the blocked/allowed requests.
