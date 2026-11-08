Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling.load-balancer-attach-detach-lifecycle
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling|AddTags|NotImplemented
Issue-Commit: 637c0a54c7e9b591effc4af895837fd7874d2d9c

# Classic ELB AddTags answers 501 NotImplemented, so the ASG load-balancer attach/detach lifecycle cannot be set up (1 cause)

> _Note: this issue was identified against the code base at commit `637c0a54c7e9b591effc4af895837fd7874d2d9c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddTags

## Description
Cause 1 of 1: Classic Elastic Load Balancing `AddTags` is an unimplemented scaffold stub, so any scenario that tags a Classic load balancer (here, the tagging step that marks the run-owned CLB before AttachLoadBalancers/DetachLoadBalancers are exercised) cannot proceed. Failing check id: autoscaling.load-balancer-attach-detach-lifecycle.

AWS: Smithy (services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/smithy-model/com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7.json) models `AddTags` with input `LoadBalancerNames` (required) and `Tags` (TagList, @length min 1, required), an empty `AddTagsOutput`, and exactly three modeled refusals: `AccessPointNotFoundException` (awsQuery code `LoadBalancerNotFound`, HTTP 400), `DuplicateTagKeysException` (awsQuery code `DuplicateTagKeys`, HTTP 400) and `TooManyTagsException` (awsQuery code `TooManyTags`, HTTP 400). On success AWS answers HTTP 200 with an awsQuery XML `AddTagsResponse`/`AddTagsResult` and persists the tag set on the named load balancer (same key overwrites its value; max 10 tags per load balancer). There is no code `NotImplemented` anywhere in this operation's contract.

Emulator: the call is refused with HTTP 501 and error code `NotImplemented`, message `operation AddTags is scaffolded but not implemented in this emulator`. The handler is still the generated stub: services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/ops/add_tags.rs returns `Err(crate::scaffolded::Scaffolded::not_implemented("AddTags"))` at the end of `AddTags::handle` and still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker.

Why: a client that tags a Classic load balancer — which every tag-scoped teardown, cost-allocation and resource-discovery flow does — gets a 5xx `NotImplemented` where AWS returns 200. botocore surfaces it as a ClientError that no retry or error branch written against the modeled contract handles, so the whole downstream scenario (AttachLoadBalancers -> DescribeLoadBalancers -> DetachLoadBalancers) is unreachable even though those operations may themselves be fine.

Fix: implement the `AddTags` handler (services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/ops/add_tags.rs) over the Classic ELB service state — store the tags on the named load balancer, upsert on duplicate key within one request's distinct keys, and return the empty 200 output; refuse an unknown load balancer with `LoadBalancerNotFound` (400), a key repeated in the request with `DuplicateTagKeys` (400), and more than 10 tags with `TooManyTags` (400). Then delete the SCAFFOLDED marker line.

Traces: autoscaling.load-balancer-attach-detach-lifecycle (the ELB client's AddTags call raises `An error occurred (NotImplemented) when calling the AddTags operation`). Note: this check is listed under the suite's `autoscaling` heading, but the diverging operation belongs to the Classic ELB crate named above.

## Evidence
Expected (AWS): HTTP 200, awsQuery XML `<AddTagsResponse><AddTagsResult/>...</AddTagsResponse>`; the tags are then readable via DescribeTags. Modeled refusals are only LoadBalancerNotFound / DuplicateTagKeys / TooManyTags, all HTTP 400.
Actual (emulator): HTTP 501, error code `NotImplemented`, message `operation AddTags is scaffolded but not implemented in this emulator` (fixed scaffold prose; the only variable part is the operation name `AddTags`). Replay output: `botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the AddTags operation: operation AddTags is scaffolded but not implemented in this emulator`. Handler evidence: services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/ops/add_tags.rs lines 4-6 (SCAFFOLDED marker) and line 156 (`Err(crate::scaffolded::Scaffolded::not_implemented("AddTags"))`).

## Steps to Reproduce
1. Create a Classic load balancer: `elb.create_load_balancer(LoadBalancerName=<name>, Listeners=[{Protocol: HTTP, LoadBalancerPort: 80, InstanceProtocol: HTTP, InstancePort: 80}], Subnets=[<subnet-id>])`.
2. Call `elb.add_tags(LoadBalancerNames=[<name>], Tags=[{'Key': 'sim2real:managed', 'Value': 'true'}])`.
3. Observe HTTP 501 / `NotImplemented` instead of the empty 200 AddTagsResult. Re-replay check autoscaling.load-balancer-attach-detach-lifecycle to confirm.
