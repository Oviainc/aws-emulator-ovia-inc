Issue-Source: trace-sweep-divergence
Issue-Trace: elbv2-load-balancer-attributes-config-not-found
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elbv2-load-balancer-attributes-config-not-found elbv2-load-balancer-config-and-capacity-nonexistent-lb-errors elbv2-loadbalancer-attributes-notfound-errors elbv2-loadbalancer-config-and-capacity-not-found-conditions elbv2-loadbalancer-resource-not-found-error-conditions elbv2-not-found-error-paths-for-lb-level-ops elbv2-not-found-error-paths-for-listener-and-lb-ops elbv2-rule-target-group-load-balancer-not-found-errors elbv2-loadbalancer-network-config-notfound-errors elbv2-network-topology-reconfiguration-not-found elbv2-ipam-pool-assignment-not-found elbv2-capacity-reservation-lifecycle-not-found elbv2-loadbalancer-capacity-reservation-notfound-errors elbv2-account-limits-and-ssl-policy-discovery elbv2-get-resource-policy-nonexistent-trust-store elbv2-get-resource-policy-not-found elbv2-resource-policy-never-existed-and-malformed-arns elbv2-resource-policy-nonexistent-trust-store elbv2-resource-policy-not-found elbv2-resource-policy-unsupported-resource-type-and-empty-arn elbv2-resourcepolicy-nonexistent-resources
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elasticloadbalancing|DescribeLoadBalancerAttributes|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|DescribeLoadBalancerAttributes|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|DescribeLoadBalancerAttributes|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|ModifyLoadBalancerAttributes|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|ModifyLoadBalancerAttributes|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|ModifyLoadBalancerAttributes|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|SetIpAddressType|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|SetIpAddressType|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|SetIpAddressType|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|SetSecurityGroups|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|SetSecurityGroups|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|SetSecurityGroups|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|SetSubnets|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|SetSubnets|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|SetSubnets|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|ModifyIpPools|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|ModifyIpPools|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|ModifyIpPools|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|DescribeCapacityReservation|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|DescribeCapacityReservation|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|DescribeCapacityReservation|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|ModifyCapacityReservation|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|ModifyCapacityReservation|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|ModifyCapacityReservation|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|GetResourcePolicy|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|GetResourcePolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|GetResourcePolicy|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|DescribeSSLPolicies|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|DescribeSSLPolicies|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|DescribeSSLPolicies|4xx->4xx|$.ErrorResponse.RequestId
Issue-Commit: c0919b0ae1fd753648687abf4b08014f282092d3

# elasticloadbalancing ELBv2 load-balancer attribute, network, capacity and policy actions answer 400 InvalidAction

> _Note: this issue was identified against the code base at commit `c0919b0ae1fd753648687abf4b08014f282092d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLoadBalancerAttributes

## Description
10 causes, one mechanism: the shared 'elasticloadbalancing' front door is the Classic ELB crate, which serves only Version=2012-06-01 and refuses every 2015-12-01 envelope with 400 InvalidAction 'Could not find operation <Action> for version <Version>' (services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/service/api_version.rs); the ELBv2 model (2015-12-01, 51 operations) answers only under its private route key 'elasticloadbalancingv2' (elasticloadbalancingv2 crate src/service.rs ROUTE_KEY), which no SDK client uses. Related OPEN issues that do NOT cover these fixture steps and carry no wire replay key: 20261023T025808Z-elbv2-awsquery-action-table-at-api-version-2015.md, 20261023T004835Z-acm-elbv2-elbv2-createloadbalancer-is-not-dispatchable-could-not-find.md, 20261022T220446Z-elbv2-elastic-load-balancing-v2-is-unreachable-every-request.md. Each cause needs the version dispatch fixed and then the named operation's own not-found check and modeled-error rendering; for all of them the recorded message is request-derived (it echoes the ARN the caller passed), so that RULE is the requirement and the quoted text is only an example.

Cause 1 of 10: DescribeLoadBalancerAttributes on an absent load balancer.
AWS: Smithy declares LoadBalancerNotFoundException, awsQueryError 'LoadBalancerNotFound', HTTP 400; message names the ARN (example "Load balancer 'arn:aws:elasticloadbalancing:us-west-2:000000000000:loadbalancer/app/sim2real-probe-missing-5e12af00/0000000000000000' not found").
Emulator: 400 Code 'InvalidAction', and no ErrorResponse.RequestId.
Why: attribute readers cannot distinguish a deleted load balancer from a nonexistent API.
Fix: version dispatch, then DescribeLoadBalancerAttributes' ARN lookup and error rendering (and the ErrorResponse.RequestId the awsQuery error envelope must carry).
Traces: elbv2-load-balancer-attributes-config-not-found#task#1 (cluster of 9).

Cause 2 of 10: ModifyLoadBalancerAttributes on an absent load balancer.
AWS: LoadBalancerNotFound / 400, same request-derived message rule.
Emulator: 400 InvalidAction.
Why: idle-timeout / access-log configuration cannot be applied or correctly refused.
Fix: version dispatch, then ModifyLoadBalancerAttributes' lookup + attribute validation.
Traces: elbv2-load-balancer-attributes-config-not-found#task#2 (cluster of 8).

Cause 3 of 10: SetIpAddressType on an absent load balancer.
AWS: Smithy SetIpAddressType declares LoadBalancerNotFound (400) alongside InvalidSubnet/InvalidConfigurationRequest.
Emulator: 400 InvalidAction.
Why: dualstack migration code cannot run or branch.
Fix: version dispatch, then SetIpAddressType handler.
Traces: elbv2-load-balancer-config-and-capacity-nonexistent-lb-errors#task#3 (cluster of 8).

Cause 4 of 10: SetSecurityGroups on an absent load balancer.
AWS: LoadBalancerNotFound / 400.
Emulator: 400 InvalidAction.
Why: security-group reconfiguration is unreachable.
Fix: version dispatch, then SetSecurityGroups handler.
Traces: elbv2-load-balancer-config-and-capacity-nonexistent-lb-errors#task#4 (cluster of 8).

Cause 5 of 10: SetSubnets on an absent load balancer.
AWS: Smithy SetSubnets declares LoadBalancerNotFound, SubnetNotFound, InvalidSubnet, AvailabilityZoneNotSupported — all awsQuery 400.
Emulator: 400 InvalidAction.
Why: subnet/AZ reconfiguration is unreachable.
Fix: version dispatch, then SetSubnets handler.
Traces: elbv2-load-balancer-config-and-capacity-nonexistent-lb-errors#task#5 (cluster of 8).

Cause 6 of 10: ModifyIpPools on an absent load balancer.
AWS: Smithy ModifyIpPools -> {IpamPools} with exactly ONE modeled error, LoadBalancerNotFoundException / 'LoadBalancerNotFound' / 400.
Emulator: 400 InvalidAction.
Why: IPAM pool assignment cannot be exercised at all.
Fix: version dispatch, then ModifyIpPools handler.
Traces: elbv2-ipam-pool-assignment-not-found#task#1 (cluster of 10).

Cause 7 of 10: DescribeCapacityReservation on an absent load balancer.
AWS: LoadBalancerNotFound / 400, message naming the ARN.
Emulator: 400 InvalidAction.
Why: capacity-reservation readers cannot run.
Fix: version dispatch, then DescribeCapacityReservation handler.
Traces: elbv2-capacity-reservation-lifecycle-not-found#task#1 (cluster of 8).

Cause 8 of 10: ModifyCapacityReservation refusal for an account that does not support minimum capacity.
AWS: Smithy ModifyCapacityReservation declares OperationNotPermittedException, awsQueryError 'OperationNotPermitted', HTTP 400 (recorded message example: 'This AWS account does not support configuring minimum load balancer capacity reservation. For more information, please contact AWS Support.'), alongside LoadBalancerNotFound and the capacity-limit errors.
Emulator: 400 Code 'InvalidAction'.
Why: a client cannot tell an unsupported-account refusal from a missing API, and the documented remediation never reaches it.
Fix: version dispatch, then ModifyCapacityReservation's account-capability gate.
Traces: elbv2-capacity-reservation-lifecycle-not-found#task#2 (cluster of 9).

Cause 9 of 10: GetResourcePolicy for an absent trust store.
AWS: Smithy GetResourcePolicy -> {Policy} with ONE modeled error, ResourceNotFoundException / awsQueryError 'ResourceNotFound' / 400; message names the ARN (example "Trust store ARN '...' not found").
Emulator: 400 Code 'InvalidAction', ErrorResponse.RequestId missing.
Why: resource-policy reads and their not-found branch are both unreachable.
Fix: version dispatch, then GetResourcePolicy's ARN parsing/lookup.
Traces: elbv2-account-limits-and-ssl-policy-discovery#task#4 (cluster of 16, including malformed-ARN and unsupported-resource-type probes).

Cause 10 of 10: DescribeSSLPolicies for an unknown policy name.
AWS: SSLPolicyNotFoundException / awsQueryError 'SSLPolicyNotFound' / 400 (fixed message, example 'One or more SSL policies not found').
Emulator: 400 Code 'InvalidAction'.
Why: a bad policy name is indistinguishable from a dead API.
Fix: version dispatch, then DescribeSSLPolicies' name lookup.
Traces: elbv2-account-limits-and-ssl-policy-discovery#task#5.

## Evidence
All actual responses are HTTP 400 with $.ErrorResponse.Error.Code 'InvalidAction', Message 'Could not find operation <Action> for version 2015-12-01' and NO $.ErrorResponse.RequestId (the expected awsQuery error envelope carries one).
- elbv2-load-balancer-attributes-config-not-found#task#1 / #task#2: expected 'LoadBalancerNotFound' for DescribeLoadBalancerAttributes / ModifyLoadBalancerAttributes.
- elbv2-load-balancer-config-and-capacity-nonexistent-lb-errors#task#3/#task#4/#task#5: expected 'LoadBalancerNotFound' for SetIpAddressType / SetSecurityGroups / SetSubnets.
- elbv2-ipam-pool-assignment-not-found#task#1: expected 'LoadBalancerNotFound' for ModifyIpPools.
- elbv2-capacity-reservation-lifecycle-not-found#task#1: expected 'LoadBalancerNotFound' for DescribeCapacityReservation; #task#2: expected 'OperationNotPermitted' for ModifyCapacityReservation.
- elbv2-account-limits-and-ssl-policy-discovery#task#4: expected 'ResourceNotFound' for GetResourcePolicy; #task#5: expected 'SSLPolicyNotFound' for DescribeSSLPolicies.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Signing for service 'elasticloadbalancing', POST 'Action=DescribeLoadBalancerAttributes&Version=2015-12-01&LoadBalancerArn=arn:aws:elasticloadbalancing:us-west-2:000000000000:loadbalancer/app/absent/0000000000000000'. Observe 400 InvalidAction instead of 400 LoadBalancerNotFound.
3. Repeat with Action = ModifyLoadBalancerAttributes, SetIpAddressType, SetSecurityGroups, SetSubnets, ModifyIpPools, DescribeCapacityReservation, ModifyCapacityReservation, GetResourcePolicy, DescribeSSLPolicies at Version=2015-12-01 — every one answers the same refusal.
4. Re-replay the fixture steps named in issue_step and trace_ids.
