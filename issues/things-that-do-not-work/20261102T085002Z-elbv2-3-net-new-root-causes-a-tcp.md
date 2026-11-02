Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.add-tags-duplicate-keys-rejected elbv2.duplicate-tag-key-in-single-request-rejected elbv2.asgdx-tcp-health-check-with-path-rejected elbv2.register-targets-exceeding-target-group-quota-rejected
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2.add-tags-duplicate-keys-rejected|run#6|AssertionError|assert message == "Tag key 'dup' has two values, but tag keys must have exactly one value", message
Issue-Key: elbv2.duplicate-tag-key-in-single-request-rejected|run#8|AssertionError|assert message == "Tag key 'dup' has two values, but tag keys must have exactly one value", message
Issue-Key: elbv2.asgdx-tcp-health-check-with-path-rejected|_error#6|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: elbv2.register-targets-exceeding-target-group-quota-rejected|run#27|AssertionError|assert message == f"The maximum number of targets on target group '{tg_arn}' has been reached", (
Issue-Commit: b1a4a993822b0574a1f24a3d622a9d42702ca481

# elbv2: 3 net-new root causes — a TCP health check with a path is accepted, and the DuplicateTagKeys and TooManyTargets refusal messages are emulator-authored

> _Note: this issue was identified against the code base at commit `b1a4a993822b0574a1f24a3d622a9d42702ca481`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTargetGroup

## Description
Three net-new ELBv2 root causes from the single-service elbv2 checks. Not covered by the open issues: 20261029T185946Z-elasticloadbalancing-elbv2-accepts-invalid-and-prohibited-requests-with-http enumerates nine other operations/preconditions (no TCP-health-check-path rule), and 20261029T185946Z-elasticloadbalancing-elbv2-error-replies-carry-emulator-authored-messages-instead is a wire issue keyed to other replay units and carries neither of the tag/quota messages below.

Cause 1 of 3: AddTags' duplicate-key refusal message is emulator-authored.
AWS: HTTP 400, Code 'DuplicateTagKeys' (Smithy AddTags errors include DuplicateTagKeysException, awsQueryError DuplicateTagKeys/400), Message "Tag key '<the duplicated key>' has two values, but tag keys must have exactly one value" — fixed text whose only variable part is the duplicated Key from the request; none of the request's tags are applied, and a later AddTags with unique keys on the same resource applies both.
Emulator: correct code, wrong text: "The tag key '<key>' was specified twice".
Why: the code is matchable but the sentence is what surfaces to operators and to tooling that asserts on refusal text; it also misstates the rule (AWS's message explains the one-value-per-key invariant).
Fix: AddTags duplicate-key validation message template (shared with the other tagging entry points).
Traces: elbv2.add-tags-duplicate-keys-rejected, elbv2.duplicate-tag-key-in-single-request-rejected.

Cause 2 of 3: CreateTargetGroup accepts HealthCheckPath on a TCP health check.
AWS: HTTP 400, Code 'InvalidConfigurationRequest' (Smithy CreateTargetGroup errors include InvalidConfigurationRequestException, awsQueryError InvalidConfigurationRequest/400), Message 'Health check paths are not supported for TCP health checks' — entirely fixed text; no target group is created, so DescribeTargetGroups by that name answers TargetGroupNotFound 'One or more target groups not found'. HealthCheckPath is only meaningful for HTTP/HTTPS probes (Smithy CreateTargetGroup.HealthCheckPath -> Path, length 1..1024; HealthCheckProtocol -> ProtocolEnum).
Emulator: HTTP 200 — the TCP group is created carrying the path.
Why: automation validated against the emulator ships a target group AWS refuses, and the refusal only appears in the real deployment.
Fix: CreateTargetGroup (and ModifyTargetGroup) cross-member validation of HealthCheckPath against HealthCheckProtocol.
Traces: elbv2.asgdx-tcp-health-check-with-path-rejected.

Cause 3 of 3: the TooManyTargets refusal message is emulator-authored.
AWS: HTTP 400, Code 'TooManyTargets' (Smithy RegisterTargets errors include TooManyTargetsException, awsQueryError TooManyTargets/400), Message "The maximum number of targets on target group '<target group ARN>' has been reached" — fixed text whose only variable part is the ARN from the request; the registered set is unchanged, and after deregistering the first target the second registers successfully (a lambda-type target group holds exactly one target).
Emulator: correct code, wrong text: 'A target group of this kind holds at most 1 targets, and this request would take it to 2' — a sentence of the emulator's own that names neither the group nor AWS's wording.
Fix: RegisterTargets quota-refusal message template in the target-group registration validation.
Traces: elbv2.register-targets-exceeding-target-group-quota-rejected.

## Evidence
Cause 1: expected message "Tag key 'dup' has two values, but tag keys must have exactly one value" with Code 'DuplicateTagKeys'; got "The tag key 'dup' was specified twice" (both elbv2.add-tags-duplicate-keys-rejected and elbv2.duplicate-tag-key-in-single-request-rejected assert the same sentence and fail identically). 'dup' is the request's Key; the rest of the sentence is fixed text.
Cause 2: expected ('InvalidConfigurationRequest', 'Health check paths are not supported for TCP health checks') for CreateTargetGroup(Protocol='TCP', Port=44300, HealthCheckProtocol='TCP', HealthCheckPath='/sync-health-check'); got a 200 success ("the call succeeded; real AWS rejects it, so there is no error code").
Cause 3: expected message "The maximum number of targets on target group '<tg arn>' has been reached"; got 'A target group of this kind holds at most 1 targets, and this request would take it to 2'.

## Steps to Reproduce
Replay the behavioral suite and re-run the elbv2 checks in trace_ids. (1) CreateTargetGroup TargetType=lambda, then AddTags with Tags=[{'Key':'dup','Value':'first'},{'Key':'dup','Value':'second'}] and compare code+message. (2) CreateTargetGroup Protocol=TCP Port=44300 VpcId=<vpc> HealthCheckProtocol=TCP HealthCheckPath=/sync-health-check and expect a 400 InvalidConfigurationRequest plus no group created. (3) Create a lambda target group, RegisterTargets one function, then RegisterTargets a second function and compare the TooManyTargets message.
