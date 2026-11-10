Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ecs__elbv2.autoscaling-group-without-elasticloadbalancing-permission-fails-attach autoscaling__ecs__elbv2.autoscaling-event-rule-mismatched-pattern-no-delivery
Issue-Key: autoscaling__ecs__elbv2.autoscaling-group-without-elasticloadbalancing-permission-fails-attach|_wait_for_denied#10|AssertionError|raise AssertionError(f"deny policy never took effect on propagation probe; last code={last!r}")
Issue-Key: autoscaling__ecs__elbv2.autoscaling-event-rule-mismatched-pattern-no-delivery|_poll_for_message#34|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Commit: e0dc03155a79e3340fd5a6f413ff0e6f3f485ded

# autoscaling__ecs__elbv2: 2 net-new causes — autoscaling DescribeTags answers 501 NotImplemented (unimplemented scaffold stub), and an ASG scale-out never delivers its EC2 Instance Launch Successful event to a corrected EventBridge rule

> _Note: this issue was identified against the code base at commit `e0dc03155a79e3340fd5a6f413ff0e6f3f485ded`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTags

## Description
Two net-new root causes behind the two surviving autoscaling__ecs__elbv2 failures.

Related open filings that do NOT cover either replay: 20261107T124656Z-autoscaling-auto-scaling-argument-validation-never-happens-8-operations.md covers exactly eight autoscaling scaffold stubs (AttachInstances, AttachLoadBalancers, DescribeAutoScalingInstances, EnableMetricsCollection, PutNotificationConfiguration, PutScalingPolicy, PutScheduledUpdateGroupAction, SetInstanceHealth) — DescribeTags is not among them and no open issue names it; 20261107T080429Z-autoscaling-ecs-elbv2-6-net-new-causes-asg.md and 20261104T042502Z-autoscaling-ecs-elbv2-6-net-new-root-causes.md own other checks in this service but not these two replay keys.

Cause 1 of 2: autoscaling DescribeTags is an unimplemented scaffold stub, so every call answers HTTP 501 NotImplemented and an IAM-denial probe built on it can never observe a refusal.
AWS: Smithy: autoscaling DescribeTags(Filters, NextToken: XmlString, MaxRecords) -> TagsType{Tags: TagDescriptionList, NextToken}, paginated (inputToken NextToken / outputToken NextToken / items Tags / pageSize MaxRecords), modeled errors InvalidNextToken (client, HTTP 400, awsQuery code 'InvalidNextToken') and ResourceContentionFault (server, HTTP 500, awsQuery code 'ResourceContention'). autoscaling is awsQuery, so a successful call is HTTP 200 with a <DescribeTagsResult><Tags>...</Tags></DescribeTagsResult> body (an empty Tags list when nothing matches — the model's own documentation says "If there's no match, no special message is returned"), and a caller denied autoscaling:DescribeTags is refused HTTP 403 AccessDenied naming the principal and the action. There is no modeled outcome at 501.
Emulator: the handler returns `Err(crate::scaffolded::Scaffolded::not_implemented("DescribeTags"))` — HTTP 501 with code 'NotImplemented' and the message "operation DescribeTags is scaffolded but not implemented in this emulator" — for every request, authorized or not. The check uses DescribeTags as its cheap propagation probe for a deny policy and reports `AssertionError: deny policy never took effect on propagation probe; last code='NotImplemented'`.
Why: 501 is a server-class answer, so botocore does not classify it as a client error at all: retry logic treats it as retryable and loops, a least-privilege test cannot tell 'denied' from 'not built', and no tag listing or tag-based discovery of Auto Scaling groups/instances is possible. Because the stub answers before any authorization runs, it also masks the authorization behavior of every flow that probes with DescribeTags.
Fix: services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/describe_tags.rs — implement the handler over the autoscaling tag store (filter support, MaxRecords/NextToken paging with InvalidNextToken for a bad token) and delete the SCAFFOLDED marker, so the operation reaches the authorization check and can answer 403 AccessDenied for a denied principal.
Traces: autoscaling__ecs__elbv2.autoscaling-group-without-elasticloadbalancing-permission-fails-attach.

Cause 2 of 2: an ASG scale-out's lifecycle event never reaches a matching EventBridge rule, so the corrected-pattern leg of the mismatched-pattern scenario never completes.
AWS: recorded behavior only. An Auto Scaling group that scales out publishes an event on the default bus with source 'aws.autoscaling' and detail-type 'EC2 Instance Launch Successful' (and 'EC2 Instance Terminate Successful' on scale-in) naming the group and the instance. A rule whose pattern matches only the terminate detail-type receives nothing from a launch; editing that SAME rule's pattern to the launch detail-type makes the NEXT scale-out arrive at its SQS target.
Emulator: the scenario never reaches a verdict — the check exhausts the harness cap with `Error: Check timed out after 180.0s`. Hypothesis (the report gives no per-step detail, so the step attribution is inferred from the scenario's waiters and from the sibling failures in this service, where an ASG never launches an instance that converges): no ASG scale-out event is published to the default event bus, so the corrected rule's positive leg waits forever.
Why: the negative half of the contract ('a mismatched pattern delivers nothing') is indistinguishable from 'nothing is ever published' — which is precisely the distinction this check exists to make. A client debugging a non-firing rule gets no way to tell a wrong pattern from a missing producer, and every ASG-event-driven automation silently never runs.
Fix: the Auto Scaling scale-out/scale-in path in the autoscaling service crate — publish the EC2 Instance Launch Successful / Terminate Successful envelopes onto the default event bus so EventBridge pattern matching and target delivery can run; plus whatever in the group's launch convergence keeps the scenario from reaching its assertions.
Traces: autoscaling__ecs__elbv2.autoscaling-event-rule-mismatched-pattern-no-delivery.

## Evidence
Replay of suite simparity-behavioral-aws-v1: 202/554 passed; autoscaling__ecs__elbv2 0/34 (0%), 34 failed. 32 are marked OWNED/ALREADY FILED; the two below are not marked.

Cause 1 — [mismatch] .autoscaling-group-without-elasticloadbalancing-permission-fails-attach:
  File "check_autoscaling__ecs__elbv2_autoscaling_group_without_elasticloadbalancing_permission_fails_attach.py", line 44, in run
    _wait_for_denied(lambda: restricted_asg.describe_tags(), timeout_s=90)
  File "...", line 272, in _wait_for_denied
  AssertionError: deny policy never took effect on propagation probe; last code='NotImplemented'
Expected: HTTP 403 AccessDenied for the restricted principal (or HTTP 200 with a Tags list before the deny propagates). Actual: HTTP 501 'NotImplemented' for all 90 s.
Confirmed in the checkout: services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/describe_tags.rs carries the "//! SCAFFOLDED, NOT IMPLEMENTED" marker (line 4) and its handle() body ends with `Err(crate::scaffolded::Scaffolded::not_implemented("DescribeTags"))` (line 171). The replay did not mark this check ALREADY FILED because the check swallows the ClientError inside its own _wait_for_denied helper, so the stub was never attributed.
Smithy consulted: autoscaling DescribeTags -> TagsType, paginated, errors InvalidNextToken (client/400) and ResourceContentionFault (server/500).

Cause 2 — [mismatch] .autoscaling-event-rule-mismatched-pattern-no-delivery: "Error: Check timed out after 180.0s". Check docstring: "An EventBridge rule matching only 'EC2 Instance Terminate Successful' never receives the launch event an ASG scale-out emits; correcting the pattern on the same rule delivers the next launch." Expected: the corrected rule's SQS target receives the launch envelope. Actual: the check never completes, so the positive leg is never satisfied.

## Steps to Reproduce
Cause 1:
1. Boot the emulator and point an awsQuery autoscaling client at it.
2. Call autoscaling.describe_tags() with no arguments, as any principal. AWS answers HTTP 200 with a (possibly empty) Tags list; a principal denied autoscaling:DescribeTags is refused HTTP 403 AccessDenied. The emulator answers HTTP 501, code 'NotImplemented', message "operation DescribeTags is scaffolded but not implemented in this emulator".
3. Attach an IAM policy denying the call and repeat — the emulator still answers 501, so the denial is unobservable.
4. Re-replay autoscaling__ecs__elbv2.autoscaling-group-without-elasticloadbalancing-permission-fails-attach.

Cause 2:
1. Create a VPC/subnets, a launch template and an Auto Scaling group; create an SQS queue and an EventBridge rule whose pattern is {"source":["aws.autoscaling"],"detail-type":["EC2 Instance Terminate Successful"]} with the queue as target.
2. Scale the group out (SetDesiredCapacity) and confirm nothing is delivered (correct).
3. events.put_rule on the SAME rule with detail-type ["EC2 Instance Launch Successful"], scale out again and receive from the queue: AWS delivers the launch envelope; the emulator delivers nothing and the check hits its 180 s cap.
4. Re-replay autoscaling__ecs__elbv2.autoscaling-event-rule-mismatched-pattern-no-delivery.
