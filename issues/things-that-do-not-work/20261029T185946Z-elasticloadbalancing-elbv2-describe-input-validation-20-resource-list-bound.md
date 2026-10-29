Issue-Source: trace-sweep-divergence
Issue-Trace: describe-input-list-bounds
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: describe-input-list-bounds
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elasticloadbalancing|DescribeLoadBalancers|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|DescribeLoadBalancers|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|DescribeTargetGroups|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|DescribeTargetGroups|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|DescribeTargetGroups|4xx->2xx|$.ErrorResponse
Issue-Key: elasticloadbalancing|DescribeTargetGroups|4xx->2xx|status
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# ELBv2 Describe input validation: 20-resource list bound and mutually exclusive filters not enforced

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLoadBalancers

## Description
The ELBv2 Describe* handlers run state lookup before input validation, so requests that AWS rejects on their own shape are answered with a resource-lookup error (or with success). 3 causes.

Cause 1 of 3: DescribeLoadBalancers with more than 20 names/ARNs.
AWS: the per-call list bound is validated before any lookup and the reply is HTTP 400, ErrorResponse.Error.Type 'Sender', Code 'ValidationError', Message "Up to 20 load balancers can be described at a time, but 21 were specified". Fixed text: "Up to 20 load balancers can be described at a time, but " + <count> + " were specified"; the limit 20 is fixed, the count is taken from the request. Smithy: DescribeLoadBalancers takes LoadBalancerArns/Names lists and models only LoadBalancerNotFound (awsQueryError code LoadBalancerNotFound, httpError 400), which covers absent names — the list bound itself is recorded behavior only.
Emulator: HTTP 400 with Code 'LoadBalancerNotFound' and Message "Load balancers '[<all names echoed>]' not found".
Why: a client batching >20 names is told the load balancers do not exist — names that do exist — so it re-creates or fails over instead of splitting the batch.
Fix: DescribeLoadBalancers handler input validation, ahead of the state lookup.
Traces: describe-input-list-bounds#task#4.

Cause 2 of 3: DescribeTargetGroups with more than 20 target-group ARNs.
AWS: HTTP 400, Type 'Sender', Code 'ValidationError', Message "You cannot describe more than '20' target groups at a time" — entirely fixed text, the quoted limit 20 is part of it. Smithy: DescribeTargetGroups models LoadBalancerNotFound and TargetGroupNotFound only; the bound is recorded behavior.
Emulator: HTTP 400 with Code 'TargetGroupNotFound' and Message 'One or more target groups not found'.
Why: same misdiagnosis as cause 1, on target groups.
Fix: DescribeTargetGroups handler input validation, ahead of the state lookup.
Traces: describe-input-list-bounds#task#6.

Cause 3 of 3: DescribeTargetGroups with Names, TargetGroupArns and LoadBalancerArn all set.
AWS: these three filters are mutually exclusive; HTTP 400, Type 'Sender', Code 'ValidationError', Message 'Target group names, target group ARNs, and a load balancer ARN cannot be specified at the same time' — fixed text, no variable parts.
Emulator: HTTP 200 with a TargetGroups list and no ErrorResponse at all.
Why: a client that builds a filter set incorrectly gets an arbitrary answer the real service would never return, so the bug is invisible until it runs against AWS.
Fix: DescribeTargetGroups handler input validation (filter mutual exclusion).
Traces: describe-input-list-bounds#task#10.

The DescribeTags analogue of the same list bound is already carried by the open issue 20261023T060746Z-elasticloadbalancing-elbv2-trust-store-revocation-ca-bundle-target.md and is not duplicated here.

## Evidence
Cause 1 (describe-input-list-bounds#task#4): expected 400 ValidationError 'Up to 20 load balancers can be described at a time, but 21 were specified'; got 400 LoadBalancerNotFound "Load balancers '[...21 names...]' not found".
Cause 2 (describe-input-list-bounds#task#6): expected 400 ValidationError "You cannot describe more than '20' target groups at a time"; got 400 TargetGroupNotFound 'One or more target groups not found'.
Cause 3 (describe-input-list-bounds#task#10): expected 400 ValidationError 'Target group names, target group ARNs, and a load balancer ARN cannot be specified at the same time'; got 200 with no ErrorResponse.

## Steps to Reproduce
Replay fixture describe-input-list-bounds against the emulator and compare steps task#4, task#6 and task#10 with the recorded responses (replay_wire_corpus, trace set simparity-wire-aws-v1).
