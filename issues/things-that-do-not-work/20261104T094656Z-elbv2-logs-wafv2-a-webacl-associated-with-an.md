Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2__logs__wafv2.log-edge-producers-waf-log-records-rule-group-blocked-and-default-allowed-requests
Issue-Key: elbv2__logs__wafv2.log-edge-producers-waf-log-records-rule-group-blocked-and-default-allowed-requests|_poll_until#41|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: 7fd8acd3be042b6d00a1996e0a0d05df0b6f87ae

# elbv2__logs__wafv2: a WebACL associated with an ALB is not enforced on the data path and logs no blocked/allowed records

> _Note: this issue was identified against the code base at commit `7fd8acd3be042b6d00a1996e0a0d05df0b6f87ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateWebACL

## Description
Cause 1 of 1: after AssociateWebACL binds a WAFv2 WebACL to an ALB, the WebACL is never enforced by the load balancer and no WAF log records are produced for blocked or default-allowed requests.
AWS: recorded behavior only: once a regional WebACL is associated with an ALB, every ALB node evaluates it before forwarding — a request matching a rule-group rule with a Block action is answered by the ALB with HTTP 403 and the WAF's block body, a request matching nothing takes the WebACL's default Allow action and reaches the target, and each decision emits a WAF log record (with action, terminatingRuleId / ruleGroupList and the httpRequest) to the configured logging destination.
Emulator: the check never observes the WebACL taking effect on any ALB node — the probe that waits for consecutive agreeing enforcement pairs exhausts the run budget, so no blocked response and no log record are ever produced.
Why: a client cannot verify that its WAF protection is live: requests a Block rule should refuse with 403 are not refused, and the audit trail the WAF logging destination is supposed to carry is empty. Security posture validated against the emulator does not hold.
Fix: the ELBv2 data-plane listener must consult the associated WAFv2 WebACL before forwarding (WebACL association state + the ALB request pipeline), and the WAF log producer must emit a record per decision to the configured destination.
Traces: elbv2__logs__wafv2.log-edge-producers-waf-log-records-rule-group-blocked-and-default-allowed-requests.

## Evidence
Replay output for elbv2__logs__wafv2.log-edge-producers-waf-log-records-rule-group-blocked-and-default-allowed-requests:
`TimeoutError: the web ACL enforced on every ALB node (4 consecutive agreeing pairs): the run budget ended first; not graded`

Expected: the rule-group-matching request answered HTTP 403 by the ALB, the non-matching request answered by the target under the WebACL's default Allow, and one WAF log record per decision naming the terminating rule and the action. Actual: enforcement never observed on any node and no records produced. The "4 consecutive agreeing pairs" is the check's stability criterion, not an AWS requirement.

## Steps to Reproduce
1. Replay the behavioral suite and re-run elbv2__logs__wafv2.log-edge-producers-waf-log-records-rule-group-blocked-and-default-allowed-requests.
2. Create a WAFv2 regional WebACL with a rule group whose rule Blocks a distinguishing request, default action Allow, and a logging destination.
3. AssociateWebACL with the ALB's ARN, then send one request that matches the Block rule and one that does not.
4. Expect HTTP 403 for the first, a target response for the second, and a WAF log record for each; the emulator never enforces the WebACL and produces no records.
