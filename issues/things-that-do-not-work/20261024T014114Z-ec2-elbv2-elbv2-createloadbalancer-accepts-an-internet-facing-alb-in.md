Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__elbv2.ecsnt-internet-facing-alb-in-a-vpc-without-an-internet-gateway-rejected
Issue-Key: ec2__elbv2.ecsnt-internet-facing-alb-in-a-vpc-without-an-internet-gateway-rejected|_error#8|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# ELBv2 CreateLoadBalancer accepts an internet-facing ALB in a VPC with no internet gateway attached

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLoadBalancer

## Description
ONE root cause.

Cause 1 of 1 - CreateLoadBalancer with Scheme=internet-facing succeeds in a VPC that has no internet gateway attached.
AWS: Smithy CreateLoadBalancer (services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10) takes Name (required), Subnets, SecurityGroups, Scheme -> LoadBalancerSchemeEnum, Type -> LoadBalancerTypeEnum, and models InvalidSubnetException (awsQueryError code InvalidSubnet, httpError 400) among its errors. The rule is server-side: an internet-facing load balancer requires the VPC containing its subnets to have an internet gateway ATTACHED, and the check is on the gateway attachment, not on any route table entry. Without one the create is refused 400 InvalidSubnet and no load balancer of that name exists. Recorded message, as an example of the request-derived rule naming the VPC: "VPC <vpc-id> has no internet gateway". An INTERNAL load balancer in the same subnets must still be accepted (and its DNSName must begin with 'internal-'); once an internet gateway is attached to the VPC - with no route added - the same internet-facing create must be accepted and its DNSName must NOT begin with 'internal-'.
Emulator: the internet-facing CreateLoadBalancer SUCCEEDS in the gateway-less VPC (the check's _error helper reports "the call succeeded; real AWS rejects it, so there is no error code"), so no error code is produced and the load balancer is created.
Why: a client gets a provisioned internet-facing ALB that in AWS cannot exist, so the single most common ALB reachability fault - a public load balancer in a VPC with no internet gateway - is undetectable against this emulator. A deployment or template that AWS rejects synchronously at create time passes here, and the emulator also loses the ability to distinguish the internal from the internet-facing scheme as a function of live EC2 gateway state (state another service call changes), so the follow-on assertions on scheme/DNS-name prefix are untestable.
Fix: subnet/VPC validation in the CreateLoadBalancer handler - resolve the VpcId of the given Subnets and read that VPC's internet-gateway attachment from EC2 state before accepting Scheme=internet-facing; emit InvalidSubnetException (400 InvalidSubnet) when none is attached. (The open issue 20261023T205437Z-elbv2-is-overly-permissive-and-under-populated-registertargets.md bundles ten other CreateLoadBalancer/RegisterTargets permissiveness causes, including the cross-VPC security-group rule, but neither covers the internet-gateway rule nor carries this replay key.)
Traces: ec2__elbv2.ecsnt-internet-facing-alb-in-a-vpc-without-an-internet-gateway-rejected.

## Evidence
Expected: 400 InvalidSubnet, no load balancer created (message example "VPC vpc-... has no internet gateway"), then the internal ALB in the same subnets accepted with an 'internal-' DNS name, then the internet-facing create accepted after the IGW attach.
Actual: CreateLoadBalancer(Name=..., Subnets=<isolated subnets>, SecurityGroups=[sg], Scheme='internet-facing', Type='application') returned success. Check failed with AssertionError "the call succeeded; real AWS rejects it, so there is no error code" at check_ec2__elbv2_ecsnt_internet_facing_alb_in_a_vpc_without_an_internet_gateway_rejected.py:119 (_error), raised from run() line 44.
Smithy: CreateLoadBalancer errors include InvalidSubnetException {errorKind: client, httpError: 400, awsQueryError: {code: InvalidSubnet, httpResponseCode: 400}}.

## Steps to Reproduce
1. CreateVpc and two subnets in different AZs; attach NO internet gateway to the VPC.
2. CreateSecurityGroup in that VPC allowing TCP 80.
3. elbv2 CreateLoadBalancer(Name='sim2real-ecsnt-p', Subnets=<the two subnets>, SecurityGroups=[sg], Scheme='internet-facing', Type='application') -> emulator returns 200 and creates the ALB; AWS returns 400 InvalidSubnet and DescribeLoadBalancers(Names=['sim2real-ecsnt-p']) raises LoadBalancerNotFound.
4. CreateLoadBalancer with Scheme='internal' in the same subnets -> must be accepted, Scheme='internal', VpcId set, DNSName starting with 'internal-'.
5. CreateInternetGateway + AttachInternetGateway(VpcId=vpc) (add no route), then re-issue the step-3 internet-facing create -> it must now be accepted with Scheme='internet-facing' and a DNSName that does not start with 'internal-'.
