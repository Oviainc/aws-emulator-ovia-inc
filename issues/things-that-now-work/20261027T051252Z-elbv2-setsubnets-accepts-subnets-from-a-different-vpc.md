Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.set-subnets-mismatched-vpc-rejected
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2.set-subnets-mismatched-vpc-rejected|_error#13|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Commit: 4846872b237bc4d9f92d9945507291eab648a721

# ELBv2 SetSubnets accepts subnets from a different VPC than the load balancer instead of refusing with InvalidConfigurationRequest

> _Note: this issue was identified against the code base at commit `4846872b237bc4d9f92d9945507291eab648a721`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetSubnets

## Description
Cause 1 of 1 — SetSubnets performs no VPC-membership validation across the requested subnets and the load balancer's existing VPC, so a cross-VPC subnet set is accepted with 200 instead of being refused.
AWS: Smithy SetSubnets(LoadBalancerArn required, Subnets, SubnetMappings, IpAddressType, EnablePrefixForIpv6SourceNat) -> SetSubnetsOutput{AvailabilityZones, IpAddressType, EnablePrefixForIpv6SourceNat}, success 200; modeled errors include InvalidConfigurationRequestException (errorKind client, httpError 400, awsQueryError code InvalidConfigurationRequest), InvalidSubnetException (InvalidSubnet 400), SubnetNotFoundException, AvailabilityZoneNotSupportedException, LoadBalancerNotFoundException. Recorded behavior: when the Subnets list mixes a subnet of the load balancer's own VPC with a subnet belonging to a DIFFERENT VPC — the subnets all exist, have free addresses and satisfy the two-distinct-AZ rule, so only VPC membership is violated — AWS refuses the call with HTTP 400, Error.Code=InvalidConfigurationRequest and Error.Message "All subnets must belong to the same VPC: '<vpc-id>'". In that message "All subnets must belong to the same VPC: '" and the closing "'" are fixed text; the one variable part is the VPC id taken from the emulator's state for the load balancer named by LoadBalancerArn (its existing VpcId), not from the request. InvalidSubnet is reserved by AWS for address-exhaustion / not-found subnets; mixing VPCs is a whole-load-balancer configuration error, hence InvalidConfigurationRequest. The load balancer's AvailabilityZones must be left untouched by the refused call, and an immediately following same-VPC SetSubnets must still succeed.
Emulator: the call returns success — no ClientError at all. Replay excerpt: "AssertionError: expected the call to be rejected, but it succeeded: None" (the check's helper returns None on the success path; the point is that no error was raised, not the None itself). Because the check aborts at the refusal assertion, the emulator's post-call AvailabilityZones were not graded, so whether the foreign-VPC subnet was actually attached is unobserved here; either way the refusal AWS requires is absent.
Why: a client gets a 200 for a configuration AWS rejects outright, so the load balancer is left in a state real AWS can never reach — an ALB straddling two VPCs (or silently moved out of its VPC). Infrastructure code and CloudFormation-style reconcilers that depend on SetSubnets failing fast on a wrong-VPC subnet id instead apply the change and diverge from the real account on the next deploy; the InvalidConfigurationRequest signal that tells an operator "you passed a subnet from the wrong VPC" never arrives, and the error is only discovered as broken routing.
Fix: the SetSubnets handler's validation in the elasticloadbalancingv2 service crate — before mutating the load balancer's zone/subnet state it must resolve each requested subnet to its VPC, compare it to the load balancer's stored VpcId, and raise InvalidConfigurationRequestException (400, awsQueryError InvalidConfigurationRequest) with the fixed text above plus the load balancer's VpcId when any subnet does not match, leaving the existing AvailabilityZones unchanged.
Note on an existing open issue: 20261023T205437Z-elbv2-ten-control-plane-operation-families-answer-501.md lists SetSubnets as its Cause 6, but for the OLDER symptom — SetSubnets answering 501 NotImplemented, with the replay key 'elbv2.set-subnets-mismatched-vpc-rejected|...|assert code == "InvalidConfigurationRequest", got NotImplemented'. SetSubnets is now dispatched and succeeds, so that 501 symptom is gone and that key no longer describes this failure: the operation is reachable and the missing piece is the VPC-match validation, which that issue does not carry.
Traces: elbv2.set-subnets-mismatched-vpc-rejected.

## Evidence
Expected (AWS, recorded): SetSubnets(LoadBalancerArn=<ALB in VPC A>, Subnets=[<subnet of VPC A in AZ x>, <subnet of VPC B in AZ y>]) -> HTTP 400, Error.Code = InvalidConfigurationRequest, Error.Message = "All subnets must belong to the same VPC: '<the ALB's VpcId>'" (fixed text plus the load balancer's VPC id from state); the load balancer's AvailabilityZones/SubnetIds are unchanged, and a subsequent same-VPC SetSubnets succeeds.
Actual (emulator): the SetSubnets call returns successfully — no error is raised, so no Error.Code and no 4xx status are produced at all. Replay output (elbv2.set-subnets-mismatched-vpc-rejected, mismatch): File "check_elbv2_set_subnets_mismatched_vpc_rejected.py", line 52, in run -> code, message = _error(_set_bad) -> AssertionError: expected the call to be rejected, but it succeeded: None.
Supporting detail from the same check: CreateVpc/CreateSubnet for both VPCs, CreateLoadBalancer in VPC A and DescribeLoadBalancers all succeeded, and the VPC-B subnet was deliberately chosen in an AZ different from the VPC-A subnet so the request satisfies the two-distinct-AZ rule — VPC membership is the only constraint violated.

## Steps to Reproduce
1. CreateVpc(CidrBlock='10.104.0.0/16') as VPC A with two subnets in two different AZs; CreateVpc(CidrBlock='10.105.0.0/16') as VPC B with one subnet.
2. CreateLoadBalancer(Name=<name>, Subnets=[both VPC A subnets], Scheme='internal', Type='application'); record AvailabilityZones[].SubnetId from DescribeLoadBalancers.
3. DescribeSubnets to find the AZ of the VPC B subnet, then pick the VPC A subnet whose AZ differs from it (so the request still spans two distinct AZs).
4. Call SetSubnets(LoadBalancerArn=<lb>, Subnets=[<that VPC A subnet>, <the VPC B subnet>]).
5. Expected: HTTP 400 with Error.Code=InvalidConfigurationRequest and Error.Message "All subnets must belong to the same VPC: '<VPC A id>'". Observed: the call succeeds with no error.
6. DescribeLoadBalancers(LoadBalancerArns=[lb]) and compare AvailabilityZones[].SubnetId to the set recorded in step 2 — AWS leaves it unchanged after the refusal.
7. Positive control: SetSubnets with both VPC A subnets must still succeed and set AvailabilityZones to exactly those subnets.
