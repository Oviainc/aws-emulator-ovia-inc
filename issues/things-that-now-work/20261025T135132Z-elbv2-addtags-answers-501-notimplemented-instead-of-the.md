Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.alb-listener-target-group-lifecycle
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2.alb-listener-target-group-lifecycle|run#74|AssertionError|assert code == "LoadBalancerNotFound" and isinstance(http, int) and 400 <= http < 500 and msg.strip() != "", (
Issue-Commit: eccc631279b459acd1f5ae10e81d9e1b78dcc975

# ELBv2 AddTags answers 501 NotImplemented instead of the modeled LoadBalancerNotFound refusal, so the ALB lifecycle check cannot prove a deleted load balancer refuses mutation

> _Note: this issue was identified against the code base at commit `eccc631279b459acd1f5ae10e81d9e1b78dcc975`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddTags

## Description
ONE root cause (Cause 1 of 1): the ELBv2 `AddTags` handler is still a scaffold stub, so every AddTags call — including one aimed at a load balancer ARN that has been deleted — is answered with HTTP 501 `NotImplemented` instead of the modeled 200 or the modeled 400 not-found error.

Note on overlap: the open issue 20261023T205437Z-elbv2-ten-control-plane-operation-families-answer-501.md describes AddTags/RemoveTags 501 as its Cause 1, but it does NOT carry this check's replay key (elbv2.alb-listener-target-group-lifecycle), so that check is not covered by it. This issue is grounded in that key and in the post-delete refusal path specifically: AddTags against a DELETED load balancer must be refused as LoadBalancerNotFound. The fixed issue 20261023T135552Z-cloudtrail-elbv2-elbv2-describetags-answers-notimplemented.md fixed DescribeTags only; AddTags was never claimed fixed, so this is not a reopen.

Cause 1 of 1 — AddTags is a 501 stub and therefore never raises LoadBalancerNotFound for a deleted load balancer.
AWS: Smithy `AddTags(ResourceArns: ResourceArns required, Tags: TagList required @length min 1) -> AddTagsOutput` with SUCCESS status 200. Its modeled errors are DuplicateTagKeysException (awsQueryError DuplicateTagKeys, 400), ListenerNotFoundException (ListenerNotFound, 400), LoadBalancerNotFoundException (LoadBalancerNotFound, 400), RuleNotFoundException (RuleNotFound, 400), TargetGroupNotFoundException (TargetGroupNotFound, 400), TooManyTagsException (TooManyTags, 400), TrustStoreNotFoundException (TrustStoreNotFound, 400). Model doc for LoadBalancerNotFoundException: "The specified load balancer does not exist." So tagging an ARN of a load balancer that has already been deleted is a client refusal: HTTP 400, awsQuery Error.Code `LoadBalancerNotFound`, with the non-empty LoadBalancerNotFound message AWS uses for that error class — fixed text naming the resource class plus the load-balancer ARN/name taken verbatim from the request's ResourceArns (the same template ELBv2 uses elsewhere, e.g. "Load balancer '<arn>' not found"); nothing in that message is a per-run value other than the ARN the caller supplied. The recording only pins the class (code LoadBalancerNotFound, a 4xx status, a non-empty message), so the exact wording beyond that is "recorded behavior only" at the level of "non-empty".
Emulator: every AddTags request returns HTTP 501 with Error.Code `NotImplemented` and message "operation AddTags is scaffolded but not implemented in this emulator". Replay excerpt: "AddTags on the deleted load balancer: expected LoadBalancerNotFound/4xx with a non-empty message, got NotImplemented/501: 'operation AddTags is scaffolded but not implemented in this emulator'". Everything earlier in the same check (CreateLoadBalancer, CreateTargetGroup, CreateListener, CreateRule, the ResourceInUse refusal of DeleteTargetGroup, the RuleNotFound/ListenerNotFound/TargetGroupNotFound post-delete reads, and the LoadBalancerNotFound post-delete reads) passed; AddTags is the only remaining divergence in it.
Why: a client cannot tag any ELBv2 resource at all, and cannot distinguish "this load balancer is gone" from "this service is unavailable". 501 is not a client error, so SDK retry/branch logic treats it as a server fault rather than a terminal 400, and tag-driven workflows (cost allocation, tag-based discovery, infrastructure reconciliation after a delete) break with a non-modeled code they have no case for.
Fix: the AddTags handler (and the tag store plus the resource-ARN resolution it needs) — services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ops/add_tags.rs, whose `handle` currently returns `Err(crate::scaffolded::Scaffolded::not_implemented("AddTags"))` (line 163) under the SCAFFOLDED marker at line 4. Resolution must classify each ResourceArn and raise the matching *NotFoundException (LoadBalancerNotFoundException for a loadbalancer ARN with no live load balancer behind it) rather than the scaffold channel.
Traces: elbv2.alb-listener-target-group-lifecycle (assertion at check line 199).

## Evidence
Expected: AddTags(ResourceArns=[<arn of a load balancer that was just deleted>], Tags=[{Key:'after',Value:'delete'}]) -> HTTP 400, Error.Code = LoadBalancerNotFound, non-empty Error.Message naming the load balancer from the request (AWS's LoadBalancerNotFound text, e.g. "Load balancer '<arn>' not found"; the ARN is the request value, the rest is fixed text).
Actual: HTTP 501, Error.Code = NotImplemented, Error.Message = "operation AddTags is scaffolded but not implemented in this emulator".
Replay output (elbv2.alb-listener-target-group-lifecycle, mismatch): AssertionError: AddTags on the deleted load balancer: expected LoadBalancerNotFound/4xx with a non-empty message, got NotImplemented/501: 'operation AddTags is scaffolded but not implemented in this emulator'.
Source: services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ops/add_tags.rs:4 ("SCAFFOLDED, NOT IMPLEMENTED") and :163 (returns not_implemented("AddTags")).

## Steps to Reproduce
1. Create a VPC with one subnet in each of two available AZs.
2. CreateLoadBalancer(Name=<name>, Subnets=[a,b], Type='application', Scheme='internal', Tags=[...]) and wait for State.Code='active'.
3. CreateTargetGroup(Name=<tg>, Protocol='HTTP', Port=80, VpcId=<vpc>, TargetType='ip'); CreateListener(HTTP:80, fixed-response default action); CreateRule(Priority=10, path-pattern '/sim2real/*', forward to the target group).
4. Tear down in dependency order: DeleteRule, DeleteListener, DeleteTargetGroup, DeleteLoadBalancer; poll DescribeLoadBalancers(LoadBalancerArns=[lb_arn]) until it answers LoadBalancerNotFound.
5. Call AddTags(ResourceArns=[lb_arn], Tags=[{Key:'after',Value:'delete'}]) on the now-deleted load balancer ARN.
6. Expected: HTTP 400 with Error.Code LoadBalancerNotFound and a non-empty message. Observed: HTTP 501 with Error.Code NotImplemented and message "operation AddTags is scaffolded but not implemented in this emulator". (The same 501 is returned for AddTags against a live load balancer, target group, listener or rule.)
