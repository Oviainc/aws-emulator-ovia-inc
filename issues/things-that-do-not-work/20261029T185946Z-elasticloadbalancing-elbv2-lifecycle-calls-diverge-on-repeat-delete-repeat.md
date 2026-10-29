Issue-Source: trace-sweep-divergence
Issue-Trace: elasticloadbalancing-delete-listener-idempotent-redelete
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticloadbalancing-delete-listener-idempotent-redelete elbv2err-create-load-balancer-input-rejections elbv2err-deletion-protection-chain elbv2err-listener-create-rejections elbv2-listener-attribute-and-certificate-error-paths elbv2env-https-listener-certificate-list elbv2-target-group-lambda-lifecycle-with-tags
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elasticloadbalancing|DeleteListener|2xx->4xx|$.DeleteListenerResponse
Issue-Key: elasticloadbalancing|DeleteListener|2xx->4xx|status
Issue-Key: elasticloadbalancing|CreateLoadBalancer|2xx->4xx|$.CreateLoadBalancerResponse
Issue-Key: elasticloadbalancing|CreateLoadBalancer|2xx->4xx|status
Issue-Key: elasticloadbalancing|ModifyLoadBalancerAttributes|2xx->4xx|$.ModifyLoadBalancerAttributesResponse
Issue-Key: elasticloadbalancing|ModifyLoadBalancerAttributes|2xx->4xx|status
Issue-Key: elasticloadbalancing|DeleteTargetGroup|2xx->4xx|status
Issue-Key: elasticloadbalancing|DescribeListenerCertificates|2xx->2xx|$.DescribeListenerCertificatesResponse.DescribeListenerCertificatesResult.Certificates.member
Issue-Key: elasticloadbalancing|DescribeTargetHealth|2xx->2xx|$.DescribeTargetHealthResponse.DescribeTargetHealthResult.TargetHealthDescriptions
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# ELBv2 lifecycle calls diverge on repeat-delete, repeat-create and post-rejection state

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteListener

## Description
Six steps where the emulator's resource state after a repeated or rejected call does not match AWS's. The first two are idempotency rules the emulator does not implement; the last four are reads whose content is wrong because an earlier call that AWS rejects was accepted (see the companion issue 'ELBv2 accepts invalid and prohibited requests with HTTP 200 instead of the modeled 400') — they are listed here because each is its own recorded step that a fixer must re-replay. 6 causes.

Cause 1 of 6: DeleteListener on a listener that was already deleted.
AWS: recorded behavior only — the second DeleteListener for the same ARN answers HTTP 200 with an empty DeleteListenerResult and a ResponseMetadata.RequestId; the delete is idempotent (Smithy DeleteListener models ListenerNotFoundException and ResourceInUseException, but the recording shows no error for the re-delete).
Emulator: HTTP 400 and no DeleteListenerResponse.
Why: teardown and retry logic that deletes twice (a retried request, or a cleanup that runs after a partial failure) fails against the emulator where AWS succeeds.
Fix: DeleteListener handler — treat an unknown/already-removed listener ARN as a successful no-op.
Traces: elasticloadbalancing-delete-listener-idempotent-redelete#task#6.

Cause 2 of 6: CreateLoadBalancer repeated with the same name AND the same settings.
AWS: recorded behavior only — answers HTTP 200 and returns the EXISTING load balancer (same LoadBalancerArn, Scheme, AvailabilityZones, VpcId, Type, IpAddressType, State.Code 'provisioning'); DuplicateLoadBalancerName is reserved for a same-name request with DIFFERENT settings.
Emulator: HTTP 400 and no CreateLoadBalancerResponse.
Why: a retried or re-applied create (the common CloudFormation/Terraform path) fails instead of returning the existing resource.
Fix: CreateLoadBalancer handler — settings-equality check before raising DuplicateLoadBalancerName.
Traces: elbv2err-create-load-balancer-input-rejections#task#9.

Cause 3 of 6: ModifyLoadBalancerAttributes on the deletion-protected load balancer after the protected DeleteLoadBalancer.
AWS: HTTP 200 with the full load balancer attribute set (deletion_protection.enabled now 'false'), because the load balancer still exists — DeleteLoadBalancer was refused.
Emulator: HTTP 400 and no ModifyLoadBalancerAttributesResponse: the load balancer is gone, because the emulator honoured the delete.
Why: downstream state is lost after a call AWS never performs.
Fix: DeleteLoadBalancer deletion-protection precondition (see the companion issue) — this step is the state check for it.
Traces: elbv2err-deletion-protection-chain#task#8.

Cause 4 of 6: DeleteTargetGroup in teardown.
AWS: HTTP 200 — the incompatible-protocol CreateListener was refused, so nothing references the target group.
Emulator: HTTP 400 — the wrongly-created listener still references it.
Fix: CreateListener protocol-compatibility check (companion issue); this step verifies the target group becomes deletable again.
Traces: elbv2err-listener-create-rejections#teardown#24.

Cause 5 of 6: DescribeListenerCertificates after adding a nonexistent certificate.
AWS: Certificates.member holds exactly the default certificate ({CertificateArn: <the listener's cert>, IsDefault 'true'}).
Emulator: a two-entry list whose second member is the unknown certificate with IsDefault 'false'.
Fix: AddListenerCertificates existence check (companion issue); this step verifies the listener's certificate list.
Traces: elbv2-listener-attribute-and-certificate-error-paths#task#16 (also elbv2env-https-listener-certificate-list#task#12).

Cause 6 of 6: DescribeTargetHealth on a lambda-type target group after an invalid RegisterTargets.
AWS: DescribeTargetHealthResult.TargetHealthDescriptions is EMPTY — the invalid registration never happened.
Emulator: one description for an EC2 instance id with TargetHealth.State 'unused', Reason 'Target.NotInUse'.
Fix: RegisterTargets target-type validation (companion issue); this step verifies the target group's membership.
Traces: elbv2-target-group-lambda-lifecycle-with-tags#task#11.

## Evidence
Cause 1: expected 200 with {'DeleteListenerResult': '', 'ResponseMetadata': {...}}, got 400.
Cause 2: expected 200 with the existing load balancer (State.Code 'provisioning'), got 400.
Cause 3: expected 200 with the 22-key attribute set, got 400 and no body.
Cause 4: expected 200, got 400.
Cause 5: expected Certificates.member = {'CertificateArn': <default cert>, 'IsDefault': 'true'}, got a 2-element list including the unknown ARN.
Cause 6: expected TargetHealthDescriptions '' (empty), got {'member': {'Target': {'Id': 'i-...'}, 'TargetHealth': {'State': 'unused', ...}}}.

## Steps to Reproduce
Replay the five fixtures named under Traces and diff the listed steps against their recorded responses (replay_wire_corpus, trace set simparity-wire-aws-v1).
