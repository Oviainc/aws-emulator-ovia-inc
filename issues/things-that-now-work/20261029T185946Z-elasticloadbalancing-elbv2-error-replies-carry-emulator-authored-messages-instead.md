Issue-Source: trace-sweep-divergence
Issue-Trace: elasticloadbalancing-lambda-target-group-lifecycle
Issue-Step: task#13
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticloadbalancing-lambda-target-group-lifecycle elbv2-tag-management-lifecycle elasticloadbalancing-listener-rule-path-based-routing elbv2rule-condition-combination-limits elbv2rule-default-rule-immutable elbv2err-listener-create-rejections elbv2err-nlb-elastic-ip-rejections
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elasticloadbalancing|AddTags|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|CreateRule|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|DeleteRule|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|ModifyRule|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|SetRulePriorities|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|CreateListener|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|CreateListener|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|AddListenerCertificates|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|CreateRule|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|CreateLoadBalancer|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# ELBv2 error replies carry emulator-authored messages instead of AWS wording

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddTags

## Description
These ELBv2 error paths fire at the right time and (mostly) with the right HTTP status, but the Error.Message — and in three cases the Error.Code — is the emulator's own prose rather than AWS's. The message is part of the contract, so each is a divergence. 10 causes.

Cause 1 of 10: AddTags with the same tag key twice.
AWS: HTTP 400, Code 'DuplicateTagKeys' (Smithy AddTags errors: DuplicateTagKeysException awsQueryError DuplicateTagKeys/400), Message "Tag key '<key>' has two values, but tag keys must have exactly one value" — fixed text around the key from the request.
Emulator: "The tag key '<key>' was specified twice".
Fix: AddTags validation message.
Traces: elasticloadbalancing-lambda-target-group-lifecycle#task#13 (also elbv2-tag-management-lifecycle#task#6).

Cause 2 of 10: CreateRule with a priority already used on the listener.
AWS: HTTP 400, Code 'PriorityInUse' (Smithy CreateRule errors: PriorityInUseException awsQueryError PriorityInUse/400), Message "Priority '<priority>' is currently in use" — fixed text with the priority from the request; the listener ARN is NOT part of AWS's message.
Emulator: "Priority '<priority>' is in use on listener '<listener arn>'".
Fix: CreateRule priority-conflict message.
Traces: elasticloadbalancing-listener-rule-path-based-routing#task#13 (also elbv2rule-condition-combination-limits#task#8/#task#10).

Cause 3 of 10: DeleteRule on a listener's default rule.
AWS: HTTP 400, Code 'OperationNotPermitted' (Smithy DeleteRule errors: OperationNotPermittedException, RuleNotFoundException), Message "Default rule '<rule ARN>' cannot be deleted" — fixed text with the ARN from the request.
Emulator: "Rule '<arn>' is the default rule of its listener and cannot be deleted; it is released with the listener".
Fix: DeleteRule default-rule guard message.
Traces: elasticloadbalancing-listener-rule-path-based-routing#task#17.

Cause 4 of 10: ModifyRule on a default rule.
AWS: HTTP 400, Code 'OperationNotPermitted', Message "Default rule '<rule ARN>' cannot be modified".
Emulator: "Rule '<arn>' is the default rule of its listener and cannot be modified; its actions are the listener's default actions".
Fix: ModifyRule default-rule guard message.
Traces: elbv2rule-default-rule-immutable#task#9.

Cause 5 of 10: SetRulePriorities including a default rule.
AWS: HTTP 400, Code 'OperationNotPermitted', Message 'The priority of a default rule cannot be changed' — entirely fixed text, no ARN.
Emulator: "Rule '<arn>' is the default rule of its listener and its priority cannot be set".
Fix: SetRulePriorities default-rule guard message.
Traces: elbv2rule-default-rule-immutable#task#10.

Cause 6 of 10: CreateListener on a port that already has a listener.
AWS: HTTP 400, Code 'DuplicateListener' (Smithy CreateListener errors: DuplicateListenerException awsQueryError DuplicateListener/400), Message "A listener already exists on this port for this load balancer '<load balancer ARN>'" — fixed text with the load balancer ARN; AWS does not name the port.
Emulator: "A listener already exists on port <port> of load balancer '<arn>'".
Fix: CreateListener duplicate-port message.
Traces: elbv2err-listener-create-rejections#task#11.

Cause 7 of 10: CreateListener for an HTTPS listener with two Certificates entries.
AWS: HTTP 400, Code 'TooManyCertificates' (Smithy CreateListener errors: TooManyCertificatesException awsQueryError TooManyCertificates/400), Message "Up to '1' certificate ARNs can be specified, but '2' were specified" — fixed text; the limit 1 is fixed, the count comes from the request.
Emulator: Code 'ValidationError' with Message 'You must provide exactly one certificate for HTTPS listeners, got <n>' — wrong code AND wrong message, so a client cannot match on the modeled TooManyCertificatesException.
Fix: CreateListener certificate-count validation (raise TooManyCertificates).
Traces: elbv2err-listener-create-rejections#task#13.

Cause 8 of 10: AddListenerCertificates with more certificates than allowed.
AWS: HTTP 400 with Message "Up to '1' certificate ARNs can be specified, but '2' were specified" (same template as cause 7).
Emulator: 'Certificates cannot be added to a listener whose protocol is HTTP: a certificate list applies only to HTTPS and TLS listeners' — it rejects on listener protocol rather than on the certificate count.
Fix: AddListenerCertificates count validation and message.
Traces: elbv2err-listener-create-rejections#task#14.

Cause 9 of 10: CreateRule with a malformed condition set.
AWS: HTTP 400 with Code 'ValidationError', Type 'Sender'.
Emulator: Code 'MalformedInput' — a code the ELBv2 model does not define, so SDK error matching falls through to the unmodeled branch.
Fix: CreateRule condition validation error classification.
Traces: elbv2rule-condition-combination-limits#task#9.

Cause 10 of 10: CreateLoadBalancer (NLB) with an Elastic IP allocation id that does not exist.
AWS: HTTP 400 with a human-readable Message naming the allocation id (the recording shape-checks it rather than asserting the literal text; Smithy CreateLoadBalancer models AllocationIdNotFoundException awsQueryError AllocationIdNotFound/400).
Emulator: Error.Message is the bare EC2 error code token 'InvalidAllocationID.NotFound' — a code placed in the message field, not a sentence.
Fix: CreateLoadBalancer subnet-mapping/allocation validation — map the EC2 lookup failure to an ELBv2 error with a message, not a code token.
Traces: elbv2err-nlb-elastic-ip-rejections#task#4 (also #task#5).

## Evidence
All causes: HTTP status matches (400 expected, 400 returned); the diff is on $.ErrorResponse.Error.Message, plus $.ErrorResponse.Error.Code for causes 7, 9 (and the code-in-message defect of cause 10).
Cause 1: expected "Tag key 'dup' has two values, but tag keys must have exactly one value", got "The tag key 'dup' was specified twice".
Cause 2: expected "Priority '10' is currently in use", got "Priority '10' is in use on listener '<arn>'".
Cause 3: expected "Default rule '<arn>' cannot be deleted", got the long emulator sentence.
Cause 4: expected "Default rule '<arn>' cannot be modified".
Cause 5: expected 'The priority of a default rule cannot be changed'.
Cause 6: expected "A listener already exists on this port for this load balancer '<arn>'", got "A listener already exists on port 80 of load balancer '<arn>'".
Cause 7: expected Code 'TooManyCertificates' / "Up to '1' certificate ARNs can be specified, but '2' were specified", got Code 'ValidationError' / 'You must provide exactly one certificate for HTTPS listeners, got 2'.
Cause 8: expected the same 'Up to 1 certificate ARNs...' message, got the HTTP-protocol sentence.
Cause 9: expected Code 'ValidationError', got 'MalformedInput'.
Cause 10: expected a shaped message, got 'InvalidAllocationID.NotFound'.

## Steps to Reproduce
Replay the eight fixtures named under Traces and diff each listed step's ErrorResponse against its recorded response (replay_wire_corpus, trace set simparity-wire-aws-v1).
