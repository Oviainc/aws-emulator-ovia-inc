Issue-Source: trace-sweep-divergence
Issue-Trace: elbv2env-alb-create-provisioning-and-subnet-reconfig
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elbv2env-alb-create-provisioning-and-subnet-reconfig elbv2env-listener-attributes-targets-and-rule-pagination elbv2err-create-load-balancer-input-rejections
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elasticloadbalancing|SetSubnets|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|SetSubnets|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|SetIpAddressType|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|SetIpAddressType|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|ModifyListenerAttributes|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|ModifyListenerAttributes|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|CreateLoadBalancer|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Related: 20261029T185946Z-elasticloadbalancing-elbv2-error-replies-carry-emulator-authored-messages-instead.md
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# ELBv2 configuration rejections use the wrong error code and the wrong rejection rule

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetSubnets

## Description
Four ELBv2 rejection paths return a different Error.Code class than AWS — AWS treats these as request validation ('ValidationError', or InvalidConfigurationRequest for the security-group case), the emulator classifies them as resource-configuration faults (or vice versa) and words them its own way. Two of them also apply a rule AWS does not have. 4 causes.

Cause 1 of 4: SetSubnets on an Application Load Balancer with a single subnet.
AWS: HTTP 400, Type 'Sender', Code 'ValidationError', Message 'At least two subnets in two different Availability Zones must be specified' — entirely fixed text. Smithy SetSubnets models InvalidConfigurationRequestException/InvalidSubnetException, but the recording shows the subnet-count check landing as ValidationError.
Emulator: Code 'InvalidConfigurationRequest' with Message 'An application load balancer must be placed in at least 2 subnets, in as many Availability Zones, and this request names <n>'.
Why: a client matching the validation class retries with the same bad input, and the message it surfaces is not the one AWS documents.
Fix: SetSubnets input validation — error class and message.
Traces: elbv2env-alb-create-provisioning-and-subnet-reconfig#task#8.

Cause 2 of 4: SetIpAddressType 'dualstack' on an internal ALB whose subnets have no IPv6 CIDR.
AWS: HTTP 400, Type 'Sender', Code 'ValidationError', Message 'You must specify subnets with an associated IPv6 CIDR block.' — fixed text. The rejection is about the SUBNETS, not the scheme: AWS permits dualstack on internal Application Load Balancers once the subnets carry IPv6 CIDRs (Smithy SetIpAddressType input IpAddressType -> IpAddressType enum, documented values for ALB are ipv4, dualstack, dualstack-without-public-ipv4).
Emulator: Code 'InvalidConfigurationRequest' with Message "The IP address type 'dualstack' is not valid for an internal load balancer, which must use 'ipv4'" — it rejects on scheme, so even a correctly configured dualstack internal ALB can never be created against the emulator.
Fix: SetIpAddressType validation — check subnet IPv6 CIDRs, drop the scheme rule, and use AWS's code and message.
Traces: elbv2env-alb-create-provisioning-and-subnet-reconfig#task#11.

Cause 3 of 4: ModifyListenerAttributes with an attribute key the service does not support.
AWS: HTTP 400, Type 'Sender', Code 'ValidationError', Message "Listener attribute key '<key>' is not recognized" — fixed text with the key from the request.
Emulator: Code 'InvalidConfigurationRequest' with Message "'<key>' is not a listener attribute this service supports".
Fix: ModifyListenerAttributes key validation — error class and message.
Traces: elbv2env-listener-attributes-targets-and-rule-pagination#task#16.

Cause 4 of 4: CreateLoadBalancer re-using an existing name while also naming invalid security groups.
AWS: validates the security groups first and answers HTTP 400, Type 'Sender', Code 'InvalidConfigurationRequest', Message 'One or more security groups are invalid' — fixed text, no variable parts.
Emulator: Code 'DuplicateLoadBalancerName' with Message "A load balancer with the same name '<name>' exists, but with different settings" — the name-collision check runs first and the security-group check is absent, so the caller is told to rename rather than to fix the groups.
Fix: CreateLoadBalancer validation order and the missing security-group validity check.
Traces: elbv2err-create-load-balancer-input-rejections#task#7 (also #task#8).

## Evidence
All causes: 400 expected and 400 returned; $.ErrorResponse.Error.Code and .Message diverge.
Cause 1: expected ValidationError / 'At least two subnets in two different Availability Zones must be specified'; got InvalidConfigurationRequest / 'An application load balancer must be placed in at least 2 subnets, in as many Availability Zones, and this request names 1'.
Cause 2: expected ValidationError / 'You must specify subnets with an associated IPv6 CIDR block.'; got InvalidConfigurationRequest / "The IP address type 'dualstack' is not valid for an internal load balancer, which must use 'ipv4'".
Cause 3: expected ValidationError / "Listener attribute key 'sim2real.not.an.attribute' is not recognized"; got InvalidConfigurationRequest / "'sim2real.not.an.attribute' is not a listener attribute this service supports".
Cause 4: expected InvalidConfigurationRequest / 'One or more security groups are invalid'; got DuplicateLoadBalancerName / "A load balancer with the same name '<name>' exists, but with different settings".

## Steps to Reproduce
Replay the three fixtures named under Traces and diff the listed steps' ErrorResponse against their recorded responses (replay_wire_corpus, trace set simparity-wire-aws-v1).
