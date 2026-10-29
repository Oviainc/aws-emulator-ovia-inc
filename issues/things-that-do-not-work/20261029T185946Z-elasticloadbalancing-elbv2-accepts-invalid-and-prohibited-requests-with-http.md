Issue-Source: trace-sweep-divergence
Issue-Trace: elbv2err-deletion-protection-chain
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elbv2err-deletion-protection-chain elbv2-listener-attribute-and-certificate-error-paths elbv2env-https-listener-certificate-list elbv2-target-group-lambda-lifecycle-with-tags elbv2env-alb-create-provisioning-and-subnet-reconfig elbv2err-create-load-balancer-input-rejections elbv2err-listener-create-rejections elbv2rule-condition-legacy-values-mirroring elbv2rule-redirect-component-pattern-validation elbv2rule-redirect-self-loop-rejected elbv2rule-redirect-omitted-components-default-to-placeholders
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elasticloadbalancing|DeleteLoadBalancer|4xx->2xx|$.ErrorResponse
Issue-Key: elasticloadbalancing|DeleteLoadBalancer|4xx->2xx|status
Issue-Key: elasticloadbalancing|AddListenerCertificates|4xx->2xx|$.ErrorResponse
Issue-Key: elasticloadbalancing|AddListenerCertificates|4xx->2xx|status
Issue-Key: elasticloadbalancing|RegisterTargets|4xx->2xx|$.ErrorResponse
Issue-Key: elasticloadbalancing|RegisterTargets|4xx->2xx|status
Issue-Key: elasticloadbalancing|DeregisterTargets|4xx->2xx|$.ErrorResponse
Issue-Key: elasticloadbalancing|DeregisterTargets|4xx->2xx|status
Issue-Key: elasticloadbalancing|ModifyIpPools|4xx->2xx|$.ErrorResponse
Issue-Key: elasticloadbalancing|ModifyIpPools|4xx->2xx|status
Issue-Key: elasticloadbalancing|CreateLoadBalancer|4xx->2xx|$.ErrorResponse
Issue-Key: elasticloadbalancing|CreateLoadBalancer|4xx->2xx|status
Issue-Key: elasticloadbalancing|CreateListener|4xx->2xx|$.ErrorResponse
Issue-Key: elasticloadbalancing|CreateListener|4xx->2xx|status
Issue-Key: elasticloadbalancing|CreateRule|4xx->2xx|$.ErrorResponse
Issue-Key: elasticloadbalancing|CreateRule|4xx->2xx|status
Issue-Key: elasticloadbalancing|ModifyRule|4xx->2xx|$.ErrorResponse
Issue-Key: elasticloadbalancing|ModifyRule|4xx->2xx|status
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# ELBv2 accepts invalid and prohibited requests with HTTP 200 instead of the modeled 400

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteLoadBalancer

## Description
Nine ELBv2 operations have no validation or precondition check at all: requests AWS rejects with a modeled 400 error are executed and answered HTTP 200, mutating emulator state. 9 causes.

Cause 1 of 9: DeleteLoadBalancer on a load balancer with deletion_protection.enabled 'true'.
AWS: HTTP 400, Type 'Sender', Code 'OperationNotPermitted' (Smithy DeleteLoadBalancer errors: LoadBalancerNotFoundException, OperationNotPermittedException awsQueryError OperationNotPermitted/400, ResourceInUseException), Message "Load balancer '<load balancer ARN>' cannot be deleted because deletion protection is enabled" — fixed text with the ARN from the request. The load balancer survives.
Emulator: HTTP 200, no ErrorResponse, and the load balancer is deleted.
Why: deletion protection is a safety control; an emulator that ignores it validates destructive automation that AWS would have blocked.
Fix: DeleteLoadBalancer handler precondition check against the stored deletion_protection.enabled attribute.
Traces: elbv2err-deletion-protection-chain#task#7.

Cause 2 of 9: AddListenerCertificates with a certificate ARN that does not exist.
AWS: HTTP 400, Type 'Sender', Code 'CertificateNotFound' (Smithy AddListenerCertificates errors: CertificateNotFoundException awsQueryError CertificateNotFound/400), Message "Certificate '<certificate ARN>' not found" — fixed text with the ARN from the request.
Emulator: HTTP 200 and the unknown certificate is attached to the listener.
Fix: AddListenerCertificates validation against ACM/certificate state.
Traces: elbv2-listener-attribute-and-certificate-error-paths#task#15 (also elbv2env-https-listener-certificate-list#task#10).

Cause 3 of 9: RegisterTargets into a lambda-type target group with a target id that is not a Lambda function ARN.
AWS: HTTP 400, Type 'Sender', Code 'ValidationError', Message 'Invalid Lambda function ARN' — entirely fixed text (recorded behavior; Smithy models InvalidTargetException for this operation, but the recording shows ValidationError).
Emulator: HTTP 200 and the bogus target is registered.
Fix: RegisterTargets target validation keyed on the target group's TargetType.
Traces: elbv2-target-group-lambda-lifecycle-with-tags#task#10.

Cause 4 of 9: DeregisterTargets from a lambda-type target group with a non-Lambda target id.
AWS: HTTP 400, Type 'Sender', Code 'ValidationError', Message 'Invalid Lambda function ARN' (fixed text).
Emulator: HTTP 200.
Fix: DeregisterTargets target validation, same rule as cause 3.
Traces: elbv2-target-group-lambda-lifecycle-with-tags#task#12.

Cause 5 of 9: ModifyIpPools with neither IpamPools nor RemoveIpamPools.
AWS: HTTP 400, Type 'Sender', Code 'ValidationError', Message "Either 'IpamPools' or 'RemoveIpamPools' must be specified." — fixed text, quoted member names are the modeled member names (Smithy ModifyIpPools input: LoadBalancerArn required, IpamPools, RemoveIpamPools).
Emulator: HTTP 200.
Fix: ModifyIpPools handler input validation (at-least-one-of).
Traces: elbv2env-alb-create-provisioning-and-subnet-reconfig#task#13.

Cause 6 of 9: CreateLoadBalancer naming a security group that does not exist.
AWS: HTTP 400, Type 'Sender', Code 'InvalidSecurityGroup' (Smithy CreateLoadBalancer errors include InvalidSecurityGroupException awsQueryError InvalidSecurityGroup/400); the recording does not assert the message text literally.
Emulator: HTTP 200 and the load balancer is created with the nonexistent group.
Fix: CreateLoadBalancer validation of SecurityGroups against EC2 state.
Traces: elbv2err-create-load-balancer-input-rejections#task#6.

Cause 7 of 9: CreateListener whose default forward target group uses a protocol incompatible with the listener protocol.
AWS: HTTP 400, Type 'Sender', Code 'IncompatibleProtocols' (Smithy CreateListener errors: IncompatibleProtocolsException awsQueryError IncompatibleProtocols/400), Message 'The listener and the following target groups have incompatible protocols: <target group ARNs, comma separated>' — fixed prefix with the offending ARNs from the request.
Emulator: HTTP 200 and the listener is created.
Fix: CreateListener protocol-compatibility check between listener and default-action target groups.
Traces: elbv2err-listener-create-rejections#task#12.

Cause 8 of 9: CreateRule with a source-ip condition that carries the legacy top-level Values list.
AWS: HTTP 400, Type 'Sender', Code 'ValidationError', Message "'Values' is not supported for a condition of type 'source-ip'" — fixed text with the member name 'Values' and the condition Field from the request.
Emulator: HTTP 200 and the rule is created.
Fix: CreateRule condition validation (legacy Values only legal for host-header/path-pattern).
Traces: elbv2rule-condition-legacy-values-mirroring#task#13 (also elbv2rule-redirect-component-pattern-validation#task#7/#task#9, elbv2rule-redirect-self-loop-rejected#task#7/#task#8).

Cause 9 of 9: ModifyRule changing a redirect action into one that redirects to itself.
AWS: HTTP 400, Type 'Sender', Code 'InvalidLoadBalancerAction' (Smithy ModifyRule errors: InvalidLoadBalancerActionException awsQueryError InvalidLoadBalancerAction/400), Message 'The redirect configuration is not valid because it creates a loop.' — fixed text.
Emulator: HTTP 200 and the looping redirect is stored.
Fix: redirect-action validation shared by CreateRule/ModifyRule (self-loop detection).
Traces: elbv2rule-redirect-omitted-components-default-to-placeholders#task#12.

## Evidence
Every cause: expected HTTP 400 with the ErrorResponse quoted above, got HTTP 200 with the operation's success payload and $.ErrorResponse absent.
Cause 1: expected {'Error': {'Type': 'Sender', 'Code': 'OperationNotPermitted', 'Message': "Load balancer '<arn>' cannot be deleted because deletion protection is enabled"}}.
Cause 2: expected Code 'CertificateNotFound', Message "Certificate '<arn>' not found".
Causes 3/4: expected Code 'ValidationError', Message 'Invalid Lambda function ARN'.
Cause 5: expected Code 'ValidationError', Message "Either 'IpamPools' or 'RemoveIpamPools' must be specified.".
Cause 6: expected Code 'InvalidSecurityGroup'.
Cause 7: expected Code 'IncompatibleProtocols', Message 'The listener and the following target groups have incompatible protocols: <tg arn>'.
Cause 8: expected Code 'ValidationError', Message "'Values' is not supported for a condition of type 'source-ip'".
Cause 9: expected Code 'InvalidLoadBalancerAction', Message 'The redirect configuration is not valid because it creates a loop.'.

## Steps to Reproduce
Replay the eight fixtures named under Traces and diff each listed step against its recorded response (replay_wire_corpus, trace set simparity-wire-aws-v1).
