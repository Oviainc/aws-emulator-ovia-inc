Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.deploy-elbv2-2-rule-priority-collision-update-rejected
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2.deploy-elbv2-2-rule-priority-collision-update-rejected|_poll_stable#19|Timeout|time.sleep(interval_s)
Issue-Commit: c748a34e9f14b89e405e1bd9e1ab4e824b33e306

# ELBv2 listener-rule priority-collision UPDATE path never converges: the check never reaches a terminal result inside the harness budget (60s timeout)

> _Note: this issue was identified against the code base at commit `c748a34e9f14b89e405e1bd9e1ab4e824b33e306`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetRulePriorities

## Description
One root cause, grounded in 1 failing behavioral check whose id's leading token is 'elbv2'.

Related open issues that do NOT carry this check's replay key, and so do not cover it: 20261023T025808Z-cloudformation-elbv2-a-cloudformation-stack-whose-elbv2-listener-rules-collide.md (the CREATE-side priority collision, keyed to cloudformation__elbv2.deploy-elbv2-2-duplicate-rule-priority-rejected-rolls-back-stack only), 20261023T205437Z-elbv2-ten-control-plane-operation-families-answer-501.md (CreateRule answers 501) and 20261023T205437Z-cloudformation-reports-create-complete-for-resources-it-never.md (CFN reports CREATE_COMPLETE for ELBv2 resources it never created).

Cause 1 of 1: the ELBv2 listener-rule priority-collision UPDATE/rollback flow never produces a terminal observable state, so the check is killed by the 60s harness budget with no response to compare.

AWS: Smithy com.amazonaws.elasticloadbalancingv2#SetRulePriorities (RulePriorities required) models PriorityInUseException with awsQueryError {code: 'PriorityInUse', httpResponseCode: 400}; CreateRule models the same error (PriorityInUse / 400) and Priority @range 1..50000. Moving a rule onto a priority another rule on the same listener already holds is therefore refused with HTTP 400 PriorityInUse. CloudFormation propagates that resource failure as an UPDATE_FAILED event on the rule's logical id carrying HandlerErrorCode: AlreadyExists, and the stack settles in UPDATE_ROLLBACK_COMPLETE with no partial effect: the rule keeps its ARN, its original priority and its original action body, and the ALB keeps serving the pre-update body. A subsequent update of the same rule onto a FREE priority settles UPDATE_COMPLETE, keeps the same rule ARN, and the served body changes. Recorded behavior only for the terminal status strings the check asserts ('CREATE_COMPLETE', 'UPDATE_ROLLBACK_COMPLETE', 'UPDATE_COMPLETE').

Emulator: the check produces no response to compare at all — the replay reports 'Check timed out after 60.0s'. No stack status, no UPDATE_FAILED event and no rule state is ever observed, so every assertion downstream of the first create is unreachable. Hypothesis: one of the two polling loops the check uses never satisfies its predicate and spins to the budget — either _wait_stack never sees the stack leave *_IN_PROGRESS, or _poll_stable's HTTP GET through the created ALB's DNS name (the first data-plane observation, before the colliding update is even submitted) never returns the expected 200/'a1'. The 60s cutoff is the harness's budget for an emulator, which answers from memory; the check's own real-AWS budgets (900s per stack operation, 420s for data-plane convergence) are provisioning times that do not apply here.

Why: a client that gates a deployment on this behavior gets nothing it can act on. A colliding-priority rule update is never refused with PriorityInUse, the stack never reports UPDATE_ROLLBACK_COMPLETE with HandlerErrorCode AlreadyExists, and the no-partial-effect guarantee (same rule ARN, priority and action body after rollback, unchanged live answer) is unverifiable. Worse, a hung call is indistinguishable from a slow one: client code waiting on a terminal stack status blocks forever instead of seeing a failure.

Fix: the ELBv2 rule-priority handlers and their state — SetRulePriorities' and CreateRule's priority-uniqueness check must raise PriorityInUse (HTTP 400) instead of succeeding or stalling — plus whatever the flow blocks on so a terminal state is always reached: the ALB listener's data-plane responder for a fixed-response rule (it must answer the GET that _poll_stable makes), and the CloudFormation update/rollback state machine that must settle the stack out of *_IN_PROGRESS and emit the UPDATE_FAILED event for the rule's logical id.

Traces: elbv2.deploy-elbv2-2-rule-priority-collision-update-rejected.

## Evidence
Replay report, service elbv2:
  [mismatch] elbv2.deploy-elbv2-2-rule-priority-collision-update-rejected
  Error: Check timed out after 60.0s

Expected (AWS): the stack create settles CREATE_COMPLETE; a GET to http://<AlbDns>/a returns 200 'a1'; the update moving RuleA from Priority 10 to Priority 20 (RuleB's priority) is refused by ELBv2 with PriorityInUse (HTTP 400); the stack settles UPDATE_ROLLBACK_COMPLETE with an UPDATE_FAILED event for logical id 'RuleA' whose reason carries 'HandlerErrorCode: AlreadyExists'; after rollback RuleA's ARN is unchanged, its Priority is '10', its FixedResponseConfig.MessageBody is 'a1' and RuleB's Priority is still '20'; a later update onto free Priority 15 settles UPDATE_COMPLETE, keeps the same rule ARN, and the ALB then serves 'a2'.

Actual (emulator): no terminal state is reached. The check is killed at 60s with no stack status, no stack event and no rule state read back, so none of the above could be compared.

Smithy citations (services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/smithy-model/com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10.json):
  SetRulePriorities errors include PriorityInUseException, awsQueryError {code: 'PriorityInUse', httpResponseCode: 400}.
  CreateRule errors include PriorityInUseException, awsQueryError {code: 'PriorityInUse', httpResponseCode: 400}; input Priority @range min 1 max 50000.

## Steps to Reproduce
1. Boot the emulator.
2. create_stack with a self-contained template: VPC + IGW + two public subnets + security group, an internet-facing application load balancer, an HTTP:80 listener whose default action is fixed-response 404 'no-match', and two AWS::ElasticLoadBalancingV2::ListenerRule resources on that listener — RuleA at Priority 10 (path-pattern /a, fixed-response 200 'a1') and RuleB at Priority 20 (path-pattern /a, fixed-response 200 'b1'). Outputs: the ALB DNSName, the listener ARN and both rule ARNs.
3. Poll describe_stacks until the status leaves *_IN_PROGRESS and GET http://<AlbDns>/a — on AWS this is CREATE_COMPLETE and 200 'a1'. Observe that one of these two never converges in the emulator.
4. update_stack with the same template but RuleA at Priority 20 and body 'a2'. On AWS the stack settles UPDATE_ROLLBACK_COMPLETE with an UPDATE_FAILED event for 'RuleA' carrying 'HandlerErrorCode: AlreadyExists', because ELBv2 refuses the colliding priority with PriorityInUse (HTTP 400); RuleA is left at Priority 10 with body 'a1' and the ALB still serves 'a1'.
5. update_stack again with RuleA at Priority 15 and body 'a2': on AWS this settles UPDATE_COMPLETE, the rule ARN is unchanged and the ALB serves 'a2'.
6. Re-replay elbv2.deploy-elbv2-2-rule-priority-collision-update-rejected.

## Resolution

PARTIAL, and deliberately so — read this before assuming the replayed check converges.

What this report attributes the timeout to (`_poll_stable#19|Timeout|time.sleep(interval_s)`,
the data-plane GET through the created ALB's DNS name) was that **nothing served an
Application Load Balancer's own traffic at all**: the name `LoadBalancer$DNSName` publishes
is not this API's endpoint, so the front door resolved no target service for a request
addressed to it and answered `404 UnknownService`, and a client polling that name never got
an answer it could act on. That is now implemented:
`services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/service/data_plane.rs`
claims a request addressed to a load balancer the ELBv2 store holds and answers it from the
listener it reaches — the actions of the first rule, in ascending priority order, whose
conditions are all met, and the listener's default actions when no rule's are — serving a
`fixed-response` in full, a `redirect` with the `Location` its components build, and a
`forward` with the status the target condition earns (503 with no healthy target, 502 for a
target with no server behind it). `tests/all/alb_data_plane.rs` pins it over a table of
every condition field, each action type and the four addressings a client can reach a load
balancer by.

The other two things this report names were already true in the tree by the time it was
picked up, and are unchanged here: `CreateRule` and `SetRulePriorities` both refuse a
colliding priority with `PriorityInUseException` (HTTP 400 `PriorityInUse`) per listener,
over the order the whole request leaves behind (`src/ops/create_rule.rs`,
`src/ops/set_rule_priorities.rs`), and CloudFormation's update path settles out of
`*_IN_PROGRESS` to `UPDATE_COMPLETE` or rolls back to `UPDATE_ROLLBACK_COMPLETE`
(`services/service-com-amazonaws-cloudformation-cloudformation/src/ops/update_stack.rs`).

What is still OPEN, and why the replayed check still times out: the ALB this check polls is
created by a CloudFormation stack, and CloudFormation creates no Elastic Load Balancing
resource in Elastic Load Balancing at all — it SYNTHESISES the identifiers it reports for
`AWS::ElasticLoadBalancingV2::{LoadBalancer,Listener,ListenerRule,TargetGroup}`
(`services/service-com-amazonaws-cloudformation-cloudformation/src/state/template.rs:3444-3680`),
and its provisioner table has no entry for any of them
(`src/state/provisioner.rs`'s `provisioner_for`). So the ELBv2 store holds no load balancer
for the stack's `AlbDns`, there is no listener configuration for this responder to answer
from, and no ELBv2 call for the colliding-priority update to be refused by. That gap is two
other open issues' causes, in this order:

* `issues/things-that-do-not-work/20261024T235905Z-elbv2-describerules-answers-listenernotfound-for-the-listener-arn.md`
  and cause 2 of
  `issues/things-that-do-not-work/20261023T205437Z-cloudformation-reports-create-complete-for-resources-it-never.md`
  — CloudFormation must create these resources through the ELBv2 API it later reports;
* which in turn needs
  `issues/things-that-do-not-work/20261027T155034Z-cloudformation-ec2-cfn-provisioned-aws-ec2-vpc-and.md`
  — ELBv2's `CreateLoadBalancer` resolves the subnets a request names against EC2 and
  refuses an id EC2 holds no subnet for (`src/state/network.rs:165-194`), and the subnet ids
  a stack reports today are synthesised rather than created in EC2, so provisioning the ALB
  before the VPC is real would make a stack that reaches CREATE_COMPLETE today fail instead.
