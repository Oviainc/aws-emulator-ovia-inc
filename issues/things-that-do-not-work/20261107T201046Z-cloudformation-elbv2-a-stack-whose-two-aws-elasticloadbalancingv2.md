Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__elbv2.deploy-elbv2-2-duplicate-rule-priority-rejected-rolls-back-stack
Issue-Key: cloudformation__elbv2.deploy-elbv2-2-duplicate-rule-priority-rejected-rolls-back-stack|run#13|AssertionError|assert broken_status == "ROLLBACK_COMPLETE", (
Issue-Commit: 47509ee38907103620e4b84df19d39bf76901d44

# cloudformation__elbv2: a stack whose two AWS::ElasticLoadBalancingV2::ListenerRule resources share a priority reaches CREATE_COMPLETE — CreateRule never raises PriorityInUse

> _Note: this issue was identified against the code base at commit `47509ee38907103620e4b84df19d39bf76901d44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRule

## Description
Cause 1 of 1: ELBv2 does not enforce per-listener priority uniqueness on rule creation, so a CloudFormation stack whose listener rules collide on priority is provisioned instead of failing and rolling back.
AWS: Smithy (elasticloadbalancingv2) CreateRule models PriorityInUseException with awsQueryError code PriorityInUse and HTTP 400; Priority is required with range 1..50000 and must be unique among a listener's rules. Creating a second rule at an occupied priority is refused with code PriorityInUse (HTTP 400) and leaves the listener's rule set unchanged. Through CloudFormation, the AWS::ElasticLoadBalancingV2::ListenerRule handler surfaces that refusal as a CREATE_FAILED event on the colliding logical resource whose ResourceStatusReason is the resource-handler wrapper AWS emits — 'Resource handler returned message: "<the ELBv2 exception message> (Service: ElasticLoadBalancingV2, Status Code: 400, ...)"' — the fixed parts being the wrapper and "(Service: ..., Status Code: ..." and the variable parts the ELBv2 message text, service name and status from the underlying call; the whole stack then rolls back to ROLLBACK_COMPLETE. (Error code/status: Smithy; the reason wrapper and message: recorded behavior.)
Emulator: the colliding rule is created, no CREATE_FAILED event is emitted, and the stack reaches CREATE_COMPLETE; the same collision driven straight at the API (CreateRule at an occupied priority) likewise succeeds rather than raising PriorityInUse.
Why: a listener's rule ordering is defined by unique priorities. A client is told a deployment succeeded while the listener holds two rules claiming the same priority — a state AWS cannot produce — so request routing is undefined, the rollback that should have left nothing behind never runs, and code that catches PriorityInUse to pick a free priority never sees it.
Fix: the ELBv2 CreateRule handler's validation (reject a priority already taken on that listener with PriorityInUse / HTTP 400 and leave the rule set untouched), and the CloudFormation AWS::ElasticLoadBalancingV2::ListenerRule provider that must surface that refusal as CREATE_FAILED and roll the stack back.
Traces: cloudformation__elbv2.deploy-elbv2-2-duplicate-rule-priority-rejected-rolls-back-stack. Open issue 20261023T025808Z-cloudformation-elbv2-a-cloudformation-stack-whose-elbv2-listener-rules-collide.md describes the same shape of defect but does not carry this check (the replay report lists this failure as neither ALREADY FILED nor OWNED); 20261028T123303Z-elbv2-listener-rule-priority-collision-update-never-resolves.md covers the UPDATE path, not create.

## Evidence
From the behavioral replay — cloudformation__elbv2.deploy-elbv2-2-duplicate-rule-priority-rejected-rolls-back-stack [mismatch]:
  assert broken_status == "ROLLBACK_COMPLETE", (...)
  AssertionError: a stack whose rule collides on priority must end ROLLBACK_COMPLETE, got 'CREATE_COMPLETE'

The stack builds VPC + IGW + two public subnets + internet-facing ALB + Listener + TargetGroup + FirstRule (Priority 10) + SecondRule (Priority 10, DependsOn FirstRule).
Expected (AWS): stack ROLLBACK_COMPLETE; the FIRST CREATE_FAILED event is SecondRule, with a reason matching 'Resource handler returned message: "<msg> (Service: ElasticLoadBalancingV2, Status Code: 400'; and elbv2:CreateRule at the occupied priority on the twin control stack's listener raises code PriorityInUse with that same message, leaving the listener's priorities unchanged.
Actual (emulator): the stack reaches CREATE_COMPLETE, so no CREATE_FAILED event and no handler message exist, and the direct CreateRule at an occupied priority is not refused.

## Steps to Reproduce
Replay the behavioral suite and re-run cloudformation__elbv2.deploy-elbv2-2-duplicate-rule-priority-rejected-rolls-back-stack. Manually: (a) CreateStack with an internet-facing ALB + Listener + TargetGroup and two AWS::ElasticLoadBalancingV2::ListenerRule resources both at Priority 10 (the second DependsOn the first) and poll DescribeStacks for a terminal status — expect ROLLBACK_COMPLETE with a SecondRule CREATE_FAILED event; (b) on a twin stack whose rules use priorities 10 and 20, call elbv2:CreateRule(ListenerArn=<listener>, Priority=10, Conditions=[path-pattern /clash], Actions=[fixed-response 200]) — expect ClientError PriorityInUse (HTTP 400) and DescribeRules unchanged.
