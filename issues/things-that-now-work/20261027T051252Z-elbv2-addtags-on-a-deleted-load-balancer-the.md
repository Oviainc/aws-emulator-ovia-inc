Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.alb-listener-target-group-lifecycle
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2.alb-listener-target-group-lifecycle|run#75|AssertionError|assert (code, msg) == ("LoadBalancerNotFound", message) and isinstance(http, int) and 400 <= http < 500, (
Issue-Commit: 4846872b237bc4d9f92d9945507291eab648a721

# ELBv2 AddTags on a deleted load balancer: the LoadBalancerNotFound message uses a per-resource template instead of AWS's "One or more load balancers not found"

> _Note: this issue was identified against the code base at commit `4846872b237bc4d9f92d9945507291eab648a721`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddTags

## Description
REOPENS 20261025T135132Z-elbv2-addtags-answers-501-notimplemented-instead-of-the.md: its fix left the call refused. That issue is grounded in exactly this check's replay key (elbv2.alb-listener-target-group-lifecycle) and claimed AddTags would answer the modeled LoadBalancerNotFound refusal for a deleted load balancer. AddTags is no longer a 501 stub and the code and status are now right, but the refusal it emits still diverges from AWS, so the same check is still red on the same AddTags call. This is not a new defect; it is the un-finished part of that fix.

Cause 1 of 1 — the LoadBalancerNotFound message AddTags emits is the single-resource template, not the collection template AWS uses for AddTags.
AWS: Smithy AddTags(ResourceArns: ResourceArns required, Tags: TagList required @length min 1) -> AddTagsOutput, success 200; modeled errors include LoadBalancerNotFoundException (errorKind client, httpError 400, awsQueryError code LoadBalancerNotFound), model doc "The specified load balancer does not exist." Recorded behavior: AddTags aimed at the ARN of an already-deleted load balancer is refused with HTTP 400 / Error.Code=LoadBalancerNotFound and Error.Message "One or more load balancers not found". That message is entirely FIXED text — it has no variable part: AddTags takes a LIST (ResourceArns), so AWS reports the collection-level not-found wording and never interpolates the individual ARN. The same recording pins the two neighbouring templates ELBv2 does parameterise, and the emulator matches both of them: DescribeLoadBalancers(LoadBalancerArns=[gone]) -> "One or more load balancers not found" (fixed text) and DescribeLoadBalancers(Names=[name]) -> "Load balancers '[<name>]' not found" (fixed text plus the Names list from the request). So the rule is per-operation-shape, not per-service.
Emulator: HTTP 400 and Error.Code=LoadBalancerNotFound (both correct), but Error.Message is "Load balancer '<loadbalancer-arn>' not found" — the singular template with the requested ARN interpolated. Replay excerpt: "AddTags on the deleted load balancer: expected LoadBalancerNotFound/4xx 'One or more load balancers not found', got LoadBalancerNotFound/400: \"Load balancer '<arn>' not found\"" (the ARN in the recording is a per-run value and is not part of the requirement).
Why: the error message is part of the contract a client reads. Operators and tools that match, log or surface ELBv2's not-found text get wording AWS never emits for AddTags, and a client that distinguishes the collection form ("one or more of the resources you named is gone", no resource identified) from the singular form ("this exact resource is gone") is told the wrong thing about which resource failed — AddTags can be given many ARNs, and AWS deliberately does not name one.
Fix: the message construction in the AddTags handler's resource-ARN resolution path in the elasticloadbalancingv2 service crate — the branch that raises LoadBalancerNotFoundException must use the collection-level fixed text for AddTags rather than reusing the singular per-resource template used by the single-ARN mutators (e.g. ModifyTargetGroup's "Target groups '<arn>' not found"). Nothing else about the refusal changes: still HTTP 400, still awsQueryError code LoadBalancerNotFound.
Traces: elbv2.alb-listener-target-group-lifecycle (the AddTags leg of the post-delete refusal loop; the DescribeLoadBalancers-by-ARN and by-Names legs of the same loop pass).

## Evidence
Expected (AWS, recorded): AddTags(ResourceArns=[<arn of a just-deleted application load balancer>], Tags=[{Key:'after',Value:'delete'}]) -> HTTP 400, Error.Code = LoadBalancerNotFound, Error.Message = "One or more load balancers not found" (all fixed text, no interpolated value).
Actual (emulator): HTTP 400, Error.Code = LoadBalancerNotFound, Error.Message = "Load balancer '<loadbalancer-arn-from-the-request>' not found".
Replay output (elbv2.alb-listener-target-group-lifecycle, mismatch): AssertionError: AddTags on the deleted load balancer: expected LoadBalancerNotFound/4xx 'One or more load balancers not found', got LoadBalancerNotFound/400: "Load balancer 'arn:aws:elasticloadbalancing:<region>:<account>:loadbalancer/app/<name>/<id>' not found".
Control, same check, same loop, both passing: DescribeLoadBalancers(LoadBalancerArns=[<gone arn>]) -> LoadBalancerNotFound "One or more load balancers not found"; DescribeLoadBalancers(Names=[<gone name>]) -> LoadBalancerNotFound "Load balancers '[<name>]' not found". Only AddTags uses the wrong one of the two templates.
Prior state of this key: 20261025T135132Z-elbv2-addtags-answers-501-notimplemented-instead-of-the.md (fixed) recorded HTTP 501 NotImplemented for the same call; the 501 is gone, the refusal's wording is not yet AWS's.

## Steps to Reproduce
1. Create a VPC with one subnet in each of two available AZs.
2. CreateLoadBalancer(Name=<name>, Subnets=[a,b], Type='application', Scheme='internal', Tags=[...]); poll DescribeLoadBalancers until State.Code='active'.
3. CreateTargetGroup(Protocol='HTTP', Port=80, VpcId=<vpc>, TargetType='ip'); CreateListener(HTTP:80, fixed-response default action); CreateRule(Priority=10, path-pattern '/sim2real/*', forward to the target group).
4. Tear down in dependency order: DeleteRule, DeleteListener, DeleteTargetGroup, DeleteLoadBalancer; poll DescribeLoadBalancers(LoadBalancerArns=[lb_arn]) until it answers LoadBalancerNotFound.
5. Call AddTags(ResourceArns=[lb_arn], Tags=[{Key:'after',Value:'delete'}]) on the deleted load balancer ARN and read Error.Code, the HTTP status and Error.Message.
6. Expected: 400 / LoadBalancerNotFound / "One or more load balancers not found". Observed: 400 / LoadBalancerNotFound / "Load balancer '<lb_arn>' not found".
7. For contrast, in the same state call DescribeLoadBalancers(LoadBalancerArns=[lb_arn]) — it returns the correct "One or more load balancers not found" — showing the collection template exists but is not used by AddTags.
