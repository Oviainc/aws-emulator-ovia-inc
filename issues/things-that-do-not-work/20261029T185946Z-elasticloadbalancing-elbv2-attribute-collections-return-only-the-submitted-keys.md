Issue-Source: trace-sweep-divergence
Issue-Trace: elasticloadbalancing-modify-load-balancer-attributes-idle-timeout-round-trip
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticloadbalancing-modify-load-balancer-attributes-idle-timeout-round-trip elbv2err-deletion-protection-chain elbv2-op-contract-describe-target-group-attributes elbv2-target-group-deregistration-delay-numeric-precision-round-trip elbv2-target-group-deregistration-delay-attribute-round-trip elbv2env-listener-attributes-targets-and-rule-pagination
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elasticloadbalancing|ModifyLoadBalancerAttributes|2xx->2xx|$.ModifyLoadBalancerAttributesResponse.ModifyLoadBalancerAttributesResult.Attributes.member
Issue-Key: elasticloadbalancing|DescribeLoadBalancerAttributes|2xx->2xx|$.DescribeLoadBalancerAttributesResponse.DescribeLoadBalancerAttributesResult.Attributes.member[*]
Issue-Key: elasticloadbalancing|DescribeTargetGroupAttributes|2xx->2xx|$.DescribeTargetGroupAttributesResponse.DescribeTargetGroupAttributesResult.Attributes.member[*]
Issue-Key: elasticloadbalancing|DescribeTargetGroupAttributes|2xx->2xx|$.DescribeTargetGroupAttributesResponse.DescribeTargetGroupAttributesResult.Attributes.member[Key="stickiness.app_cookie.cookie_name"]
Issue-Key: elasticloadbalancing|ModifyTargetGroupAttributes|2xx->2xx|$.ModifyTargetGroupAttributesResponse.ModifyTargetGroupAttributesResult.Attributes.member
Issue-Key: elasticloadbalancing|DescribeListenerAttributes|2xx->2xx|$.DescribeListenerAttributesResponse.DescribeListenerAttributesResult.Attributes
Issue-Key: elasticloadbalancing|ModifyListenerAttributes|2xx->2xx|$.ModifyListenerAttributesResponse.ModifyListenerAttributesResult.Attributes.member
Issue-Key: elasticloadbalancing|DescribeListenerAttributes|2xx->2xx|$.DescribeListenerAttributesResponse.DescribeListenerAttributesResult.Attributes.member
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# ELBv2 attribute collections return only the submitted keys instead of the full attribute set

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyLoadBalancerAttributes

## Description
For all three ELBv2 attribute families (load balancer, target group, listener) AWS answers both Modify*Attributes and Describe*Attributes with the resource's COMPLETE attribute set — every supported key, including the ones still at their default and the string-valued ones whose default is the empty string. The emulator returns only the keys the caller just submitted, and its Describe sets are missing members. 8 causes.

AWS (all causes): Smithy Modify{LoadBalancer,TargetGroup,Listener}Attributes output member Attributes -> the same attribute list shape as the matching Describe, and the recordings show the full set echoed after a single-key modify. Key membership is the requirement: which keys exist for a resource type, and the default value each carries ('' for the S3/cookie-name string keys, 'off'/'false'/numeric defaults for the rest). Individual values come from the request or the emulator's state.
Emulator: Modify*Attributes echoes exactly the one key submitted; Describe*Attributes returns a short or empty set.
Why: clients (and the AWS CLI/Terraform) treat the Modify reply as the resource's new state. Getting one key back makes every other attribute look unset, so a client either re-submits defaults it should not touch or reports the resource as mis-configured; a client that reads a key the emulator omits (stickiness.app_cookie.cookie_name, access_logs.s3.bucket, the routing.http.response.* header keys) gets no entry at all and cannot distinguish 'default' from 'unsupported'.
Fix: the attribute state/serializer shared by the three Modify*Attributes and Describe*Attributes handlers — persist a complete default attribute map per resource and serialize all of it.

Cause 1 of 8: ModifyLoadBalancerAttributes returns a single Attributes member (idle_timeout.timeout_seconds) where AWS returns all 22 load balancer attributes. Traces: elasticloadbalancing-modify-load-balancer-attributes-idle-timeout-round-trip#task#3 (also elbv2err-deletion-protection-chain#task#6).
Cause 2 of 8: DescribeLoadBalancerAttributes omits access_logs.s3.bucket, access_logs.s3.prefix, health_check_logs.s3.bucket, health_check_logs.s3.prefix, connection_logs.s3.bucket and connection_logs.s3.prefix (all default ''), returns an extra ipv6.deny_all_igw_traffic key AWS does not list here, and orders the remaining members differently from AWS. Traces: elasticloadbalancing-modify-load-balancer-attributes-idle-timeout-round-trip#task#2.
Cause 3 of 8: DescribeTargetGroupAttributes member order/membership diverges — the position AWS fills with stickiness.app_cookie.cookie_name holds target_group_health.unhealthy_state_routing.minimum_healthy_targets.percentage. Traces: elbv2-op-contract-describe-target-group-attributes#task#2.
Cause 4 of 8: DescribeTargetGroupAttributes omits stickiness.app_cookie.cookie_name (default '') entirely. Traces: elbv2-target-group-deregistration-delay-attribute-round-trip#task#3.
Cause 5 of 8: ModifyTargetGroupAttributes returns only deregistration_delay.timeout_seconds where AWS returns all 14 target group attributes. Traces: elbv2-target-group-deregistration-delay-attribute-round-trip#task#4.
Cause 6 of 8: DescribeListenerAttributes returns an EMPTY Attributes member where AWS returns the 11 routing.http.response.* keys (ten header_value keys defaulting to '' plus routing.http.response.server.enabled 'true'). Traces: elbv2env-listener-attributes-targets-and-rule-pagination#task#13.
Cause 7 of 8: ModifyListenerAttributes returns only routing.http.response.server.enabled instead of all 11 listener attributes. Traces: elbv2env-listener-attributes-targets-and-rule-pagination#task#14.
Cause 8 of 8: DescribeListenerAttributes after the modify returns only routing.http.response.server.enabled instead of all 11. Traces: elbv2env-listener-attributes-targets-and-rule-pagination#task#15.

## Evidence
Cause 1: $.ModifyLoadBalancerAttributesResponse...Attributes.member expected a 22-entry list (idle_timeout.timeout_seconds, access_logs.s3.*, health_check_logs.s3.*, deletion_protection.enabled, routing.http2.enabled, routing.http.*, load_balancing.cross_zone.enabled, client_keep_alive.seconds, waf.fail_open.enabled, zonal_shift.config.enabled, connection_logs.s3.*), got the single entry {'Key': 'idle_timeout.timeout_seconds', 'Value': '120'}.
Cause 2: members[1],[2],[4],[5],[20],[21] expected the six ''-valued S3 keys, got other keys/None; member[6] extra 'ipv6.deny_all_igw_traffic'.
Cause 5: expected 14-entry target group set, got {'Key': 'deregistration_delay.timeout_seconds', 'Value': '45'}.
Cause 6: $.DescribeListenerAttributesResponse...Attributes expected an 11-member list, got '' (empty).
Causes 7/8: expected the same 11-member list, got only routing.http.response.server.enabled.

## Steps to Reproduce
Replay the four fixtures named under Traces and diff the listed steps against their recorded responses (replay_wire_corpus, trace set simparity-wire-aws-v1).
