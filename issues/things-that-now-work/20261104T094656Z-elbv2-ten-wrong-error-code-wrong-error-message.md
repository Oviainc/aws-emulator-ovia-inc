Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.create-target-group-health-check-protocol-incompatible-rejected elbv2.modify-listener-attributes-out-of-range-timeout-rejected elbv2.modify-load-balancer-attributes-idle-timeout-enforced elbv2.modify-target-group-health-check-settings elbv2.drop-invalid-header-fields-strips-the-header-before-the-target elbv2.listener-attributes-tcp-idle-timeout-roundtrip elbv2.add-listener-certificates-too-many-fails elbv2.add-tags-exceeding-quota-rejected elbv2.create-rule-condition-value-and-action-limits-rejected elbv2.remove-default-listener-certificate-rejected elbv2.remove-listener-certificates-default-cert-forbidden elbv2.load-balancer-attribute-legality-by-type-rejected elbv2.modify-listener-attributes-invalid-config-fails elbv2.set-ip-address-type-unsupported-for-load-balancer-type-rejected elbv2.set-rule-priorities-conflict-with-existing-fails elbv2.set-rule-priorities-duplicate-fails elbv2.set-rule-priorities-duplicate-priority-fails elbv2.register-target-invalid-id-fails elbv2.network-config-error-semantics
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2.create-target-group-health-check-protocol-incompatible-rejected|run#5|AssertionError|assert code == "ValidationError", f"expected ValidationError, got {code!r}"
Issue-Key: elastic-load-balancing-v2|ModifyListenerAttributes|InvalidConfigurationRequest
Issue-Key: elbv2.modify-load-balancer-attributes-idle-timeout-enforced|run#9|AssertionError|assert code == "ValidationError", f"expected a non-numeric idle_timeout value to be rejected, got {code}"
Issue-Key: elbv2.modify-target-group-health-check-settings|run#17|AssertionError|assert code == "ValidationError", f"expected ValidationError, got {code!r}"
Issue-Key: elbv2.drop-invalid-header-fields-strips-the-header-before-the-target|run#11|AssertionError|assert malformed_code == "ValidationError", (
Issue-Key: elbv2.listener-attributes-tcp-idle-timeout-roundtrip|run#17|AssertionError|assert code == "ValidationError", (
Issue-Key: elbv2.add-listener-certificates-too-many-fails|run#16|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: elbv2.add-tags-exceeding-quota-rejected|run#19|AssertionError|assert message == "The specified resource contains too many tags", message
Issue-Key: elbv2.create-rule-condition-value-and-action-limits-rejected|run#13|AssertionError|assert msg == want_msg, f"priority {priority}: expected message {want_msg!r}, got {msg!r}"
Issue-Key: elbv2.remove-default-listener-certificate-rejected|run#13|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: elbv2.remove-listener-certificates-default-cert-forbidden|run#11|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: elbv2.load-balancer-attribute-legality-by-type-rejected|_reject_lb_attrs#15|AssertionError|assert message == message_is, (
Issue-Key: elbv2.modify-listener-attributes-invalid-config-fails|run#14|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: elbv2.set-ip-address-type-unsupported-for-load-balancer-type-rejected|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: elbv2.set-rule-priorities-conflict-with-existing-fails|run#25|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: elbv2.set-rule-priorities-duplicate-fails|run#22|AssertionError|assert code == "ValidationError", f"expected validation error, got {code}"
Issue-Key: elbv2.set-rule-priorities-duplicate-priority-fails|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: elbv2.register-target-invalid-id-fails|run#14|AssertionError|assert code == "ValidationError", f"expected validation error, got {code}"
Issue-Key: elbv2.network-config-error-semantics|run#43|AssertionError|assert err == ("SubnetNotFound", "InvalidSubnetID.NotFound"), err
Issue-Commit: 7fd8acd3be042b6d00a1996e0a0d05df0b6f87ae

# elbv2: ten wrong-error-code / wrong-error-message root causes on refusals (InvalidConfigurationRequest for ValidationError, emulator-authored limit messages, PriorityInUse for RuleNotFound)

> _Note: this issue was identified against the code base at commit `7fd8acd3be042b6d00a1996e0a0d05df0b6f87ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyTargetGroupAttributes

## Description
Ten net-new root causes. In every one the emulator DOES refuse the call, but with the wrong error code or an error message that is not what AWS words for that error class. An HTTP status, an error code and an error message are requirements, so each of these is a defect. Each cause is independent and must be closed separately.

Cause 1 of 10: parameter-range and parameter-type violations are answered InvalidConfigurationRequest instead of ValidationError.
AWS: the awsQuery protocol answers a member whose value violates a modeled constraint with HTTP 400 `ValidationError` and a message of the form "Value '<value>' at '<memberName>' failed to satisfy constraint: Member must ...". Smithy: InvalidConfigurationRequestException (code `InvalidConfigurationRequest`) is modeled for semantically-illegal-but-well-typed combinations, not for out-of-range or non-boolean/non-numeric values. Relevant constraints: CreateTargetGroup/ModifyTargetGroup HealthCheckIntervalSeconds @range 5-300, HealthCheckTimeoutSeconds @range 2-120; the listener attribute tcp.idle_timeout.seconds is documented 60-6000.
Emulator: every one of these is answered `InvalidConfigurationRequest` — e.g. tcp.idle_timeout.seconds=6001 returns InvalidConfigurationRequest with "The value '6001' is not valid for the attribute 'tcp.idle_timeout.seconds': it must be an integer from 60 to 6000"; a non-boolean routing.http.drop_invalid_header_fields.enabled returns InvalidConfigurationRequest; a non-numeric idle_timeout returns InvalidConfigurationRequest; an incompatible health-check protocol on CreateTargetGroup returns InvalidConfigurationRequest.
Why: clients and IaC tools branch on the error code — ValidationError is a non-retryable malformed-input signal, InvalidConfigurationRequest is a resource-state signal. Mapping all input validation onto InvalidConfigurationRequest makes malformed input look like a transient/resource problem and hides which member was wrong.
Fix: the shared awsQuery input-validation layer for these operations (ELBv2 request deserialization/validation) must emit ValidationError for constraint violations, leaving InvalidConfigurationRequest for legal-but-incompatible combinations.
Traces: elbv2.create-target-group-health-check-protocol-incompatible-rejected, elbv2.modify-listener-attributes-out-of-range-timeout-rejected, elbv2.modify-load-balancer-attributes-idle-timeout-enforced, elbv2.modify-target-group-health-check-settings, elbv2.drop-invalid-header-fields-strips-the-header-before-the-target, elbv2.listener-attributes-tcp-idle-timeout-roundtrip.

Cause 2 of 10: the TooManyCertificates message is emulator-authored and names the emulator.
AWS: Smithy: AddListenerCertificates error TooManyCertificatesException (code `TooManyCertificates`, HTTP 400). AWS's message is the fixed text "The maximum number of certificates has been reached" — it has no variable part.
Emulator: "This emulator holds at most 25 certificates in a listener's certificate list".
Why: the message names the emulator rather than the AWS error, so any client or test that matches AWS's message text fails, and the text leaks that this is not AWS.
Fix: the AddListenerCertificates handler's limit branch (error message construction).
Traces: elbv2.add-listener-certificates-too-many-fails.

Cause 3 of 10: the TooManyTags message is emulator-authored.
AWS: Smithy: AddTags error TooManyTagsException (code `TooManyTags`, HTTP 400). AWS's message is the fixed text "The specified resource contains too many tags" — no variable part.
Emulator: "You've reached the limit of 50 tags for a resource; the request carries 60 tags" (the 50 and the 60 are the quota and the request's tag count).
Why: same as cause 2 — the fixed text differs entirely, so message-matching clients break.
Fix: the AddTags handler's quota branch (error message construction).
Traces: elbv2.add-tags-exceeding-quota-rejected.

Cause 4 of 10: the CreateRule condition-limit message is emulator-authored.
AWS: Smithy: CreateRule declares the rule-limit family; for exceeding the per-rule condition count AWS's message is the fixed text "You can specify up to 5 conditions" (the 5 is the documented per-rule condition limit).
Emulator: "A rule can only have '5' condition values and regex values".
Why: the refusal is right but the wording is not AWS's, and it conflates conditions with condition values, so a client cannot tell which limit it hit.
Fix: the CreateRule handler's condition/action limit validation (error message construction).
Traces: elbv2.create-rule-condition-value-and-action-limits-rejected.

Cause 5 of 10: the default-certificate removal refusal message is emulator-authored.
AWS: Smithy: RemoveListenerCertificates error OperationNotPermittedException (code `OperationNotPermitted`, HTTP 400). AWS's message is the fixed text "Default certificate cannot be removed" — no variable part.
Emulator: "Certificate '<cert-arn>' is the default certificate of listener '<listener-arn>' and cannot be removed; call ModifyListener to replace it" (the two ARNs come from the request).
Why: AWS's fixed text is replaced with a longer emulator-authored sentence plus remediation advice AWS does not emit; message assertions fail.
Fix: the RemoveListenerCertificates handler's default-certificate branch (error message construction).
Traces: elbv2.remove-default-listener-certificate-rejected, elbv2.remove-listener-certificates-default-cert-forbidden.

Cause 6 of 10: the "attribute not supported for this load-balancer/listener type" messages are emulator-authored.
AWS: recorded behavior only for the exact wording; AWS refuses a type-illegal attribute with "Load balancer attribute key '<key>' is not supported on load balancers with type '<type>'" (fixed frame; the key and the type come from the request and the target resource), and the listener-attribute form with "The provided listener attribute is not supported".
Emulator: "The attribute 'dns_record.client_routing_policy' is not supported for a application load balancer" and "The attribute 'tcp.idle_timeout.seconds' is not supported for a listener of a application load balancer" — a different frame (and ungrammatical "a application").
Why: the refusal class is right but the message is not AWS's, so clients that surface or match the message diverge.
Fix: the ModifyLoadBalancerAttributes and ModifyListenerAttributes handlers' attribute-legality branches (error message construction).
Traces: elbv2.load-balancer-attribute-legality-by-type-rejected, elbv2.modify-listener-attributes-invalid-config-fails.

Cause 7 of 10: the SetIpAddressType type-mismatch message is emulator-authored.
AWS: recorded behavior only: AWS refuses an IP address type illegal for the load balancer's type with the fixed frame "The specified IP address type is not supported on load balancers with type '<type>'." (the type comes from the target load balancer; note the trailing period).
Emulator: "The IP address type 'dualstack-without-public-ipv4' is only valid for an application load balancer".
Why: different fixed text, so message-matching clients break and the message states a positive rule AWS does not state.
Fix: the SetIpAddressType handler's type-legality branch (error message construction).
Traces: elbv2.set-ip-address-type-unsupported-for-load-balancer-type-rejected.

Cause 8 of 10: SetRulePriorities answers PriorityInUse where AWS answers RuleNotFound / ValidationError.
AWS: Smithy: SetRulePriorities errors are OperationNotPermittedException, PriorityInUseException (code `PriorityInUse`) and RuleNotFoundException (code `RuleNotFound`). For a RuleArn that is not a rule of the listener AWS answers RuleNotFound with the fixed text "One or more priorities not found"; for a request list that repeats the same priority AWS answers a 400 ValidationError. PriorityInUse is for assigning a priority already held by a rule that is NOT part of the same reorder.
Emulator: answers PriorityInUse with "Priority '<n>' is currently in use" in all three situations (the <n> is the priority from the request).
Why: a client cannot distinguish "you named a rule I don't have" from "that priority is taken"; the reorder-with-duplicates case reports a resource conflict instead of malformed input, so retry logic loops on an input error.
Fix: the SetRulePriorities handler — resolve every RuleArn first (RuleNotFound), reject duplicate priorities inside the request as ValidationError, and reserve PriorityInUse for collisions with rules outside the request.
Traces: elbv2.set-rule-priorities-conflict-with-existing-fails, elbv2.set-rule-priorities-duplicate-fails, elbv2.set-rule-priorities-duplicate-priority-fails.

Cause 9 of 10: RegisterTargets answers InvalidTarget for a malformed target id.
AWS: Smithy: RegisterTargets declares InvalidTargetException (code `InvalidTarget`) for a target that exists but cannot be registered (wrong VPC, wrong type, unsupported instance). A syntactically invalid target Id is malformed input and is answered HTTP 400 `ValidationError`.
Emulator: answers `InvalidTarget` for the malformed id.
Why: ValidationError is non-retryable malformed input; InvalidTarget reads as a resource/topology problem, so a client retries or goes looking for a VPC mismatch that does not exist.
Fix: the RegisterTargets handler's target-id parsing/validation, ahead of the target-resolution branch that raises InvalidTarget.
Traces: elbv2.register-target-invalid-id-fails.

Cause 10 of 10: the SubnetNotFound message from CreateLoadBalancer carries an EC2-shaped sentence instead of AWS's message.
AWS: recorded behavior only: ELBv2 refuses a nonexistent subnet with code `SubnetNotFound` (HTTP 400) and the message is the fixed text "InvalidSubnetID.NotFound".
Emulator: code is right (`SubnetNotFound`) but the message is "The subnet ID '<subnet-id>' does not exist (Service: AmazonEC2; Status Code: 400; Error Code: InvalidSubnetID.NotFound; Request ID: <uuid>; Proxy: null)" — an EC2-service-framed sentence with an embedded request id.
Why: the message is not ELBv2's; it attributes the failure to AmazonEC2 and embeds a per-run request id, so clients that parse or display the ELBv2 message see a foreign error envelope.
Fix: the CreateLoadBalancer handler's subnet-resolution branch — emit ELBv2's own SubnetNotFound message rather than forwarding an EC2-shaped string.
Traces: elbv2.network-config-error-semantics.

## Evidence
All quotes are from the behavioral replay output; ARNs, ids, request ids and dates are per-run values, not requirements.

Cause 1: `AssertionError: expected ValidationError, got 'InvalidConfigurationRequest'` (create-target-group-health-check-protocol-incompatible-rejected and modify-target-group-health-check-settings); `AssertionError: expected error code in ('ValidationError',), got 'InvalidConfigurationRequest': "The value '6001' is not valid for the attribute 'tcp.idle_timeout.seconds': it must be an integer from 60 to 6000"`; `AssertionError: expected a non-numeric idle_timeout value to be rejected, got InvalidConfigurationRequest`; `AssertionError: a non-boolean value for routing.http.drop_invalid_header_fields.enabled was rejected with 'InvalidConfigurationRequest', expected ValidationError`; `AssertionError: tcp.idle_timeout.seconds=10 is outside the documented 60-6000 range but returned 'InvalidConfigurationRequest'`.
Cause 2: `AssertionError: expected message 'The maximum number of certificates has been reached', got "This emulator holds at most 25 certificates in a listener's certificate list"`.
Cause 3: assertion `message == "The specified resource contains too many tags"` failed with `You've reached the limit of 50 tags for a resource; the request carries 60 tags`.
Cause 4: `AssertionError: priority 14: expected message 'You can specify up to 5 conditions', got "A rule can only have '5' condition values and regex values"`.
Cause 5: `AssertionError: expected message 'Default certificate cannot be removed', got "Certificate 'arn:aws:acm:...:certificate/<id>' is the default certificate of listener 'arn:...:listener/app/<lb>/<id>/<lid>' and cannot be removed; call ModifyListener to replace it"` (twice, from two independent checks).
Cause 6: `AssertionError: dns_record.client_routing_policy is NLB-only: expected the refusal to explain itself with "Load balancer attribute key 'dns_record.client_routing_policy' is not supported on load balancers with type 'application'", got "The attribute 'dns_record.client_routing_policy' is not supported for a application load balancer"`; `AssertionError: expected message 'The provided listener attribute is not supported', got "The attribute 'tcp.idle_timeout.seconds' is not supported for a listener of a application load balancer"`.
Cause 7: `AssertionError: expected message "The specified IP address type is not supported on load balancers with type 'network'.", got "The IP address type 'dualstack-without-public-ipv4' is only valid for an application load balancer"`.
Cause 8: `AssertionError: expected message 'One or more priorities not found', got "Priority '21' is currently in use"`; `AssertionError: expected message 'One or more priorities not found', got "Priority '10' is currently in use"`; `AssertionError: expected validation error, got PriorityInUse`.
Cause 9: `AssertionError: expected validation error, got InvalidTarget`.
Cause 10: assertion `err == ("SubnetNotFound", "InvalidSubnetID.NotFound")` failed with `('SubnetNotFound', "The subnet ID 'subnet-0000000000000000' does not exist (Service: AmazonEC2; Status Code: 400; Error Code: InvalidSubnetID.NotFound; Request ID: <uuid>; Proxy: null)")`.

## Steps to Reproduce
Replay the behavioral suite and re-run exactly the checks in trace_ids:
1. Cause 1 — ModifyListenerAttributes tcp.idle_timeout.seconds=6001 and =10; ModifyLoadBalancerAttributes idle_timeout.timeout_seconds=<non-numeric>; ModifyLoadBalancerAttributes routing.http.drop_invalid_header_fields.enabled=<non-boolean>; CreateTargetGroup with an incompatible HealthCheckProtocol; ModifyTargetGroup with out-of-range health-check settings. Expect HTTP 400 ValidationError naming the member in each case.
2. Cause 2 — AddListenerCertificates past the 25-certificate limit; expect 400 TooManyCertificates, message exactly "The maximum number of certificates has been reached".
3. Cause 3 — AddTags with 60 tags; expect 400 TooManyTags, message exactly "The specified resource contains too many tags".
4. Cause 4 — CreateRule with more than 5 conditions; expect message exactly "You can specify up to 5 conditions".
5. Cause 5 — RemoveListenerCertificates naming the listener's default certificate; expect 400 OperationNotPermitted, message exactly "Default certificate cannot be removed".
6. Cause 6 — ModifyLoadBalancerAttributes dns_record.client_routing_policy on an ALB; ModifyListenerAttributes tcp.idle_timeout.seconds on an ALB listener; compare the message frames.
7. Cause 7 — SetIpAddressType dualstack-without-public-ipv4 on a network load balancer; expect the AWS message frame.
8. Cause 8 — SetRulePriorities naming a rule of another listener; SetRulePriorities repeating one priority twice; expect RuleNotFound "One or more priorities not found" and ValidationError respectively.
9. Cause 9 — RegisterTargets with a syntactically invalid target Id; expect 400 ValidationError.
10. Cause 10 — CreateLoadBalancer with Subnets=['subnet-0000000000000000']; expect 400 SubnetNotFound with message "InvalidSubnetID.NotFound".
