Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2__resourcegroupstaggingapi.load-balancer-and-target-group-tags-visible-through-get-resources
Issue-Key: elbv2__resourcegroupstaggingapi.load-balancer-and-target-group-tags-visible-through-get-resources|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Commit: 71cff0481dbaf9b41262283aed7350a64aff149f

# ELBv2 tags written with AddTags never reach the Resource Groups Tagging index: GetResources with a matching tag filter never returns the load balancer or the target group

> _Note: this issue was identified against the code base at commit `71cff0481dbaf9b41262283aed7350a64aff149f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResources

## Description
ONE root cause (Cause 1 of 1): tags written natively through ELBv2's own AddTags on an Application Load Balancer and on a target group are never published to the Resource Groups Tagging index, so ResourceGroupsTaggingAPI `GetResources` filtered on exactly that key/value never returns those two ARNs. The check polls GetResources every 3s for up to 120s and is killed by the harness's 60s budget without the predicate ever holding — AddTags itself returned 200 for both resources beforehand, so this is not the old 501/InvalidAction symptom.

Overlap notes, neither of which covers this key: the OPEN issue 20261023T025808Z-elbv2-resource-groups-tagging-api-resource-groups-tagging-over-elbv2-resources-is-unreachable.md records the OLDER symptom (CreateTargetGroup answered 400 InvalidAction at version 2015-12-01) — CreateTargetGroup and CreateLoadBalancer now succeed in this replay, so that blocker is gone and the index gap it hid is what remains. The OPEN issue 20261023T205437Z-elbv2-ten-control-plane-operation-families-answer-501.md mentions this check id under its AddTags/RemoveTags 501 cause, whose replay key (`elbv2__resource-groups-tagging-api|AddTags|NotImplemented`) is now FIXED, so closing that cause does not close this one: AddTags answers 200 and the index is still empty. The same "native tag write never reaches the tagging index" shape is already filed for other services (20261024T113144Z-dynamodb-native-tagresource-untagresource-writes-never-reach-the.md, 20261024T113144Z-lambda-function-tags-are-absent-from-the-resource.md, 20261024T043150Z-resource-groups-tagging-api-getresources-never-answers-for.md); this is the ELBv2 member of that family and is grounded in its own check.

Cause 1 of 1 — the ELBv2 tag store is not joined to the Resource Groups Tagging index.
AWS: Resource Groups Tagging API `GetResources(TagFilters=[{Key:k, Values:[v]}]) -> {ResourceTagMappingList:[{ResourceARN, Tags}], PaginationToken}` is the account-wide index over tags written by each service's OWN tagging API. ELBv2's Smithy model binds `AddTags(ResourceArns required, Tags required)` -> 200 and `RemoveTags(ResourceArns required, TagKeys required)` -> 200 over loadbalancer, targetgroup, listener, rule and truststore ARNs; nothing in either model makes the index optional or service-scoped. So after AddTags returns 200 for an `elasticloadbalancing:...:loadbalancer/app/<name>/<id>` ARN and for an `elasticloadbalancing:...:targetgroup/<name>/<id>` ARN with the same key/value, a single GetResources tag filter on that key/value returns BOTH ARNs — one filter spanning two different ELBv2 resource types — and after RemoveTags withdraws the key from both, the same filter converges to an empty ResourceTagMappingList. AWS's index is eventually consistent, hence the poll; the requirement is convergence, not a fixed latency. Recorded behavior only for the latency: the recording converged well inside the check's budget.
Emulator: AddTags returns 200 for both ARNs, then GetResources with that tag filter never returns the two ARNs — the predicate `{m['ResourceARN'] for m in mappings} == {lb_arn, tg_arn}` never holds and the check is terminated by the harness at 60.0s (verdict: mismatch, "Check timed out after 60.0s"). The suite therefore never reaches the RemoveTags half of the check.
Why: tag-based discovery over load balancers and target groups is silently broken. A client that tags ELBv2 resources and then enumerates them through the tagging API (cost allocation, inventory, Resource Groups membership, automation that reconciles by tag) gets an empty or short result with a 200 — no error to branch on — so it concludes the resources do not exist or are untagged. Because the call succeeds, retries never help and the failure surfaces far from its cause.
Fix: the publication path between ELBv2's tag store and the shared Resource Groups Tagging index — the elbv2 AddTags/RemoveTags handlers in services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ops/ must write through to the index the resourcegroupstaggingapi GetResources handler reads (or that handler must read ELBv2's tag store), for BOTH the loadbalancer and targetgroup ARN forms, including withdrawal on RemoveTags. Note that RemoveTags is itself still a 501 stub (filed separately against service elbv2), so the withdrawal half of this cause cannot be verified until that handler exists.
Traces: elbv2__resource-groups-tagging-api.load-balancer-and-target-group-tags-visible-through-get-resources.

## Evidence
Expected: after AddTags(ResourceArns=[lb_arn], Tags=[{Key:k,Value:v}]) and AddTags(ResourceArns=[tg_arn], Tags=[{Key:k,Value:v}]) both return 200, GetResources(TagFilters=[{Key:k, Values:[v]}]) converges to a ResourceTagMappingList whose ResourceARN set is exactly {lb_arn, tg_arn}; after RemoveTags withdraws k from both, the same filter converges to [].
Actual: GetResources with that filter never yields both ARNs; the predicate never holds across repeated polls and the check is killed by the harness — "Error: Check timed out after 60.0s". No error code is returned at any point: AddTags answers 200 and GetResources answers 200 with a list that does not contain the tagged ELBv2 ARNs.
Setup that succeeded first (so these are not the cause): CreateVpc, two CreateSubnet calls in two available AZs, CreateTargetGroup(Protocol=HTTP, Port=80, TargetType='ip', Tags=[...]) and CreateLoadBalancer(Scheme='internal', Type='application', Tags=[...]).
Per-run values not part of the requirement: the key/value are freshly generated per run (sim2real-behav-k-*/sim2real-behav-v-*), as are the VPC/subnet/LB/TG ids in the ARNs.

## Steps to Reproduce
1. CreateVpc(CidrBlock='10.72.0.0/16') and one CreateSubnet in each of two available AZs.
2. CreateTargetGroup(Name=<unique>, Protocol='HTTP', Port=80, VpcId=<vpc>, TargetType='ip') -> tg_arn; CreateLoadBalancer(Name=<unique>, Subnets=[a,b], Scheme='internal', Type='application') -> lb_arn.
3. Pick a fresh key k and value v. AddTags(ResourceArns=[lb_arn], Tags=[{Key:k,Value:v}]) and AddTags(ResourceArns=[tg_arn], Tags=[{Key:k,Value:v}]) — both return 200 today.
4. Poll resourcegroupstaggingapi GetResources(TagFilters=[{Key:k, Values:[v]}]). Expected: within the index's convergence window the ResourceARN set equals {lb_arn, tg_arn}. Observed: the set never contains them; the poll runs until the harness terminates the check at 60s.
5. (Blocked today by the separate RemoveTags 501 stub) RemoveTags(ResourceArns=[lb_arn], TagKeys=[k]) and RemoveTags(ResourceArns=[tg_arn], TagKeys=[k]), then poll the same GetResources filter and expect it to converge to [].
