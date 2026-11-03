Issue-Source: trace-sweep-divergence
Issue-Trace: elbv2-alb5xx-traj-arn-from-another-region
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elbv2-alb5xx-traj-arn-from-another-region elbv2-alb5xx-traj-missing-and-conflicting-describe-filters elbv2-alb5xx-traj-name-where-arn-required elbv2-alb5xx-traj-paging-mistakes elbv2-alb5xx-traj-stale-and-cross-type-arns elbv2-albos-alb-lookup-by-dns-and-arn-mistakes elbv2-ntraf-target-registration-port-and-type-mistakes elbv2-ntraf-fargate-target-group-create-mistakes
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elasticloadbalancing|DescribeLoadBalancers|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|DescribeLoadBalancers|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|DescribeLoadBalancerAttributes|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|DescribeLoadBalancerAttributes|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|DescribeTargetGroups|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|DescribeTargetGroups|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|DescribeTargetGroupAttributes|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|DescribeTargetGroupAttributes|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|RegisterTargets|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|RegisterTargets|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|CreateTargetGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 2fe3407ad3a572d7c56bcafd231aa855d1c09ca1

# ELBv2 rejects malformed identifiers and invalid inputs with modeled not-found errors instead of ValidationError

> _Note: this issue was identified against the code base at commit `2fe3407ad3a572d7c56bcafd231aa855d1c09ca1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLoadBalancers

## Description
This bundle carries 7 causes. They share one root fault: ELBv2 performs no syntactic validation of request identifiers and no protocol-level input validation. It goes straight to a state lookup, so a request AWS rejects as malformed before it ever consults state comes back as one of the operation's MODELED not-found errors (LoadBalancerNotFound / TargetGroupNotFound / InvalidTarget) or as MalformedInput, rather than AWS's protocol-level ValidationError, and with the emulator's own wording. A seventh cause is the same validation layer rendering an enum-constraint message with the members in the wrong order. The existing open issue 20261029T185946Z-elasticloadbalancing-elbv2-error-replies-carry-emulator-authored-messages-instead.md covers message WORDING on other operations; it does not own any of the steps below (the replay reported them unowned), and the causes here additionally change the error CODE, which that issue does not cover.

Cause 1 of 7: DescribeLoadBalancers with a syntactically invalid load balancer ARN in LoadBalancerArns.
AWS: Smithy: DescribeLoadBalancers input LoadBalancerArns -> LoadBalancerArns (list of LoadBalancerArn); the only modeled error is LoadBalancerNotFoundException (awsQueryError LoadBalancerNotFound, httpError 400). ValidationError is not in the model, so for the code and message this is recorded behavior only: the recording shows HTTP 400 with Error.Type 'Sender', Error.Code 'ValidationError' and Error.Message "'<arn>' is not a valid load balancer ARN". Fixed text: "'" + "' is not a valid load balancer ARN"; the variable part is the offending ARN string taken verbatim from the request's LoadBalancerArns member. AWS applies this check BEFORE any state lookup, which is why a well-formed-but-absent ARN yields LoadBalancerNotFound while a malformed one yields ValidationError.
Emulator: answers 400 with Error.Code 'LoadBalancerNotFound' and Error.Message 'One or more load balancers not found' — a generic state-miss message that names neither the member nor the offending value.
Why: a client cannot distinguish a programming error (wrong ARN shape, ARN from another region, stale/cross-type ARN) from a resource that simply does not exist. SDK and IaC retry/reconcile logic keyed on LoadBalancerNotFound will treat a malformed ARN as a deleted load balancer and silently recreate or skip, instead of surfacing the input bug.
Fix: DescribeLoadBalancers' input validation — an ARN syntax/partition/region/resource-type check that runs ahead of the state lookup and raises the protocol-level ValidationError.

Cause 2 of 7: DescribeLoadBalancerAttributes with a LoadBalancerArn that is not in ARN format.
AWS: Smithy: DescribeLoadBalancerAttributes.LoadBalancerArn is required; the only modeled error is LoadBalancerNotFoundException (LoadBalancerNotFound, 400). Code/message are recorded behavior only: 400, Error.Code 'ValidationError', Error.Message "'<value>' must be in ARN format". Fixed text: "'" + "' must be in ARN format"; the variable part is the LoadBalancerArn value from the request.
Emulator: 400 with Error.Code 'LoadBalancerNotFound' and Error.Message "Load balancer '<value>' not found".
Why: same confusion of malformed input with absent resource, on the attribute-read path used by Terraform/CloudFormation drift checks.
Fix: DescribeLoadBalancerAttributes' input validation.

Cause 3 of 7: DescribeTargetGroups with a syntactically invalid LoadBalancerArn filter.
AWS: Smithy: DescribeTargetGroups input takes LoadBalancerArn, TargetGroupArns, Names, Marker, PageSize; modeled errors are LoadBalancerNotFoundException and TargetGroupNotFoundException (both 400). Code/message recorded behavior only: 400, 'ValidationError', "'<arn>' is not a valid load balancer ARN" — same template as Cause 1, with the value taken from the request's LoadBalancerArn.
Emulator: 400 'LoadBalancerNotFound' with "Load balancer '<arn>' not found".
Why: as Cause 1, on the target-group discovery path.
Fix: DescribeTargetGroups' input validation.

Cause 4 of 7: DescribeTargetGroupAttributes given a target group NAME where an ARN is required.
AWS: Smithy: DescribeTargetGroupAttributes.TargetGroupArn is required and targets TargetGroupArn; the only modeled error is TargetGroupNotFoundException (TargetGroupNotFound, 400). Code/message recorded behavior only: 400, 'ValidationError', "'<value>' must be in ARN format". Fixed text as in Cause 2; the variable part is the TargetGroupArn value submitted (here a bare target group name).
Emulator: 400 'TargetGroupNotFound' with "Target groups '<value>' not found".
Why: passing a name where an ARN belongs is one of the most common ELBv2 client mistakes; reporting it as TargetGroupNotFound sends the caller looking for a missing resource instead of fixing the argument.
Fix: DescribeTargetGroupAttributes' input validation.

Cause 5 of 7: DescribeLoadBalancerAttributes with a missing/conflicting describe filter is classified MalformedInput.
AWS: Smithy: LoadBalancerArn is required on DescribeLoadBalancerAttributes. Code recorded behavior only: 400 with Error.Code 'ValidationError' (the recording asserts the code; the message was not asserted on this step).
Emulator: 400 with Error.Code 'MalformedInput'.
Why: 'MalformedInput' is not an ELBv2 error code, so client error taxonomies and retry classifiers do not recognise it; a caller matching on ValidationError for required-parameter faults sees an unknown code and may treat it as retryable.
Fix: the request-validation layer shared by the ELBv2 Query-protocol handlers — missing/conflicting required parameters must surface as ValidationError, not MalformedInput.

Cause 6 of 7: RegisterTargets with an IP address supplied as an instance-type target.
AWS: Smithy: RegisterTargets.Targets is required (TargetDescriptions); modeled errors include InvalidTargetException (InvalidTarget, 400) and TargetGroupNotFoundException. Code/message recorded behavior only: 400, Error.Code 'ValidationError', Error.Message "Instance ID '<id>' is not valid". Fixed text: "Instance ID '" + "' is not valid"; the variable part is the Target.Id from the request. AWS treats an ill-formed instance id as an input-syntax fault (ValidationError), reserving InvalidTarget for a well-formed target that cannot be registered.
Emulator: 400 with Error.Code 'InvalidTarget' and Error.Message "The target '<id>' is not valid: this account holds no instance with that id" — emulator-authored prose that also asserts an account-state reason AWS does not give here.
Why: a client distinguishing 'bad argument' (fail fast) from 'target not registerable' (retry after the instance appears) gets the opposite verdict and will poll forever on a value that can never become valid.
Fix: RegisterTargets' validation — target-id syntax check against the target group's TargetType, raised as ValidationError before the InvalidTarget state check.

Cause 7 of 7: CreateTargetGroup's enum-constraint message lists TargetType members in the wrong order.
AWS: Smithy: CreateTargetGroup.TargetType targets TargetTypeEnum, whose declared values are [instance, ip, lambda, alb]. The recorded 400 ValidationError message is "1 validation error detected: Value '<value>' at 'targetType' failed to satisfy constraint: Member must satisfy enum value set: [lambda, ip, instance, alb]" — AWS's enumerated set order is recorded behavior only and does NOT follow the model's declaration order. Fixed text: everything but the rejected value; the variable part is the submitted TargetType value echoed after "Value '".
Emulator: emits the same template but with the set rendered in model-declaration order, "[instance, ip, lambda, alb]".
Why: tests and tooling that assert on AWS's validation message text (a common pattern for enum-constraint errors, since the message is the only place the accepted set is published) fail against the emulator even though the status and code match.
Fix: the ELBv2 enum-constraint message formatter used by CreateTargetGroup's validation — the member order must be the order AWS publishes, not the model's.

## Evidence
Cause 1 — fixture elbv2-alb5xx-traj-arn-from-another-region step task#1, DescribeLoadBalancers, status 400 both sides. Expected $.ErrorResponse.Error.Code 'ValidationError', got 'LoadBalancerNotFound'. Expected $.ErrorResponse.Error.Message "'<lb-arn>' is not a valid load balancer ARN", got 'One or more load balancers not found'. Cluster covers 8 steps across elbv2-alb5xx-traj-arn-from-another-region, elbv2-alb5xx-traj-missing-and-conflicting-describe-filters, elbv2-alb5xx-traj-name-where-arn-required, elbv2-alb5xx-traj-paging-mistakes, elbv2-alb5xx-traj-stale-and-cross-type-arns and elbv2-albos-alb-lookup-by-dns-and-arn-mistakes.

Cause 2 — elbv2-alb5xx-traj-arn-from-another-region task#2, DescribeLoadBalancerAttributes, 400/400. Expected Code 'ValidationError' + Message "'<lb-arn>' must be in ARN format"; got 'LoadBalancerNotFound' + "Load balancer '<lb-arn>' not found". Cluster: 2 steps (also elbv2-alb5xx-traj-name-where-arn-required task#4).

Cause 3 — elbv2-alb5xx-traj-arn-from-another-region task#3, DescribeTargetGroups, 400/400. Expected Code 'ValidationError' + "'<lb-arn>' is not a valid load balancer ARN"; got 'LoadBalancerNotFound' + "Load balancer '<lb-arn>' not found". Cluster: 6 steps across elbv2-alb5xx-traj-arn-from-another-region, elbv2-alb5xx-traj-name-where-arn-required, elbv2-alb5xx-traj-paging-mistakes, elbv2-alb5xx-traj-stale-and-cross-type-arns, elbv2-albos-alb-lookup-by-dns-and-arn-mistakes.

Cause 4 — elbv2-alb5xx-traj-name-where-arn-required task#6, DescribeTargetGroupAttributes, 400/400. Expected Code 'ValidationError' + "'<tg-name>' must be in ARN format"; got 'TargetGroupNotFound' + "Target groups '<tg-name>' not found".

Cause 5 — elbv2-alb5xx-traj-missing-and-conflicting-describe-filters task#2, DescribeLoadBalancerAttributes, 400/400. Expected Code 'ValidationError', got 'MalformedInput'.

Cause 6 — elbv2-ntraf-target-registration-port-and-type-mistakes task#6, RegisterTargets, 400/400. Expected Code 'ValidationError' + "Instance ID '<ip>' is not valid"; got 'InvalidTarget' + "The target '<ip>' is not valid: this account holds no instance with that id".

Cause 7 — elbv2-ntraf-fargate-target-group-create-mistakes task#2, CreateTargetGroup, 400/400. Expected Message "...Member must satisfy enum value set: [lambda, ip, instance, alb]"; got "...[instance, ip, lambda, alb]".

## Steps to Reproduce
1. Replay the wire corpus against the emulator (replay_wire_corpus).
2. Cause 1: fixture elbv2-alb5xx-traj-arn-from-another-region, step task#1 — DescribeLoadBalancers with a load balancer ARN that is syntactically invalid / from another region.
3. Cause 2: same fixture, step task#2 — DescribeLoadBalancerAttributes with a LoadBalancerArn that is not in ARN format.
4. Cause 3: same fixture, step task#3 — DescribeTargetGroups with an invalid LoadBalancerArn filter.
5. Cause 4: fixture elbv2-alb5xx-traj-name-where-arn-required, step task#6 — DescribeTargetGroupAttributes with a bare target group name as TargetGroupArn.
6. Cause 5: fixture elbv2-alb5xx-traj-missing-and-conflicting-describe-filters, step task#2 — DescribeLoadBalancerAttributes with a missing/conflicting filter.
7. Cause 6: fixture elbv2-ntraf-target-registration-port-and-type-mistakes, step task#6 — RegisterTargets with an IP string as an instance target.
8. Cause 7: fixture elbv2-ntraf-fargate-target-group-create-mistakes, step task#2 — CreateTargetGroup with TargetType 'IP' (wrong case).
9. In each case compare $.ErrorResponse.Error.Code and $.ErrorResponse.Error.Message against the recorded response.
