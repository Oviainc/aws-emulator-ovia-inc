Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53-recovery-control-config.create-safety-rule-both-rule-types-rejected
Issue-Shape-Id: com.amazonaws.route53recoverycontrolconfig#Route53RecoveryControlConfig
Issue-Key: route53-recovery-control-config.create-safety-rule-both-rule-types-rejected|run#9|AssertionError|assert code == "ValidationException", code
Issue-Commit: 5b71589754f95f05f88c1a0ee9ccd691bbc5f701

# CreateSafetyRule does not enforce the exactly-one-of AssertionRule/GatingRule rule: a request carrying both is not refused with ValidationException "Invalid Rule provided."

> _Note: this issue was identified against the code base at commit `5b71589754f95f05f88c1a0ee9ccd691bbc5f701`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSafetyRule

## Description
One cause.

Cause 1 of 1: CreateSafetyRule has no mutual-exclusion validation between its AssertionRule and GatingRule input members, and no observable no-effect guarantee on the refusal path. Failing check: route53-recovery-control-config.create-safety-rule-both-rule-types-rejected.

AWS: CreateSafetyRule (POST /safetyrule) models AssertionRule (NewAssertionRule) and GatingRule (NewGatingRule) as two independent optional members, and models ValidationException as a client error with HTTP 400 (Smithy: CreateSafetyRule input {AssertionRule: NewAssertionRule, GatingRule: NewGatingRule, ClientToken, Tags}; errors ValidationException (client, httpError 400), InternalServerException (server, 500)). The model does NOT express the exactly-one-of constraint, so for the refusal itself this is recorded behavior only: the recording shows that supplying BOTH members in one request is refused with error code ValidationException and the message "Invalid Rule provided." — that whole sentence is fixed text, with no variable part interpolated from the request (no rule name, no control-panel ARN, no member name appears in it). The refusal also has no effect: after it, ListSafetyRules on the control panel returns an empty SafetyRules list. A request with ONLY AssertionRule set succeeds (200) and then appears in ListSafetyRules as a single entry under the ASSERTION key with the submitted Name.

Emulator: the call is refused with error code NotImplemented and the message "operation CreateSafetyRule is scaffolded but not implemented in this emulator" instead of ValidationException / "Invalid Rule provided." — the check asserts on the error CODE of the both-rules request and gets `NotImplemented` (AssertionError: NotImplemented at `assert code == "ValidationException"`). The handler is still the unmodified scaffold, so neither the mutual-exclusion refusal, its no-effect guarantee, nor the single-rule success path exists.

Why: a client that relies on CreateSafetyRule to police its own request shape gets the wrong signal twice. Botocore classifies NotImplemented as a non-retryable, non-validation fault, so code that catches ClientError on code "ValidationException" to report "supply exactly one of AssertionRule/GatingRule" never matches and the error surfaces as an unexpected service fault; and because the emulator never implements the operation at all, a caller cannot distinguish "my request was malformed" from "the service is broken", nor confirm that a rejected request left no half-created safety rule behind on the control panel.

Fix: the operation's validation, in the CreateSafetyRule handler — services/service-com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig/src/ops/create_safety_rule.rs. It must reject a request where both AssertionRule and GatingRule are present (and, symmetrically, where neither is) with ValidationException/400 and the fixed message "Invalid Rule provided.", persisting nothing to the service state, and accept the single-rule form so the created rule is visible via ListSafetyRules under its ASSERTION/GATING key.

Related but not covering this: the open issue 20261029T191943Z-route53-recovery-control-config-createsafetyrule-stub.md asks for CreateSafetyRule to be implemented, but its Issue-Trace-Ids carries only route53-recovery-control-config.gating-rule-controls-and-rule-config-persist, .list-safety-rules-returns-mixed-assertion-and-gating, .safety-rule-threshold-exceeds-controls-rejected and .update-safety-rule-wait-period-persists — it does NOT carry this check's replay key, and none of those four exercises the both-rule-types refusal. This issue is what gates that negative class.

Traces: route53-recovery-control-config.create-safety-rule-both-rule-types-rejected.

## Evidence
Check route53-recovery-control-config.create-safety-rule-both-rule-types-rejected, verdict [mismatch]:

  File "check_route53_recovery_control_config_create_safety_rule_both_rule_types_rejected.py", line 64, in run
    assert code == "ValidationException", code
  AssertionError: NotImplemented

Expected (recorded AWS behavior): CreateSafetyRule with BOTH AssertionRule and GatingRule populated ->
  HTTP 400, Error.Code = "ValidationException", Error.Message = "Invalid Rule provided." (entire message is fixed text; no request- or state-derived values)
  followed by ListSafetyRules(ControlPanelArn=<panel>) -> SafetyRules == []  (refusal left no rule behind)
  then CreateSafetyRule with ONLY AssertionRule -> HTTP 200, AssertionRule.SafetyRuleArn returned, and
  ListSafetyRules(ControlPanelArn=<panel>) -> exactly 1 entry, entry["ASSERTION"]["Name"] == the submitted Name.

Actual (emulator): HTTP 501, Error.Code = "NotImplemented", Error.Message = "operation CreateSafetyRule is scaffolded but not implemented in this emulator". The both-rules request, the no-effect assertion and the single-rule positive control are all unreachable.

Smithy model (com-amazonaws-route53recoverycontrolconfig): CreateSafetyRule http POST /safetyrule code 200; input members AssertionRule -> NewAssertionRule, GatingRule -> NewGatingRule (both optional, no mutual-exclusion trait); errors ValidationException (errorKind client, httpError 400), InternalServerException (errorKind server, httpError 500). The exactly-one-of rule and the message text are recorded behavior only.

## Steps to Reproduce
1. Boot the route53-recovery-control-config emulator.
2. CreateCluster and wait for Status == DEPLOYED; CreateControlPanel in that cluster; CreateRoutingControl twice in that control panel and wait for each Status == DEPLOYED (call them rc_a, rc_b).
3. Call CreateSafetyRule with BOTH members populated against the same control panel:
   AssertionRule = {Name: <any>, ControlPanelArn: <panel>, AssertedControls: [rc_a], RuleConfig: {Type: ATLEAST, Threshold: 1, Inverted: false}, WaitPeriodMs: 5000}
   GatingRule   = {Name: <any>, ControlPanelArn: <panel>, GatingControls: [rc_b], TargetControls: [rc_a], RuleConfig: {Type: ATLEAST, Threshold: 1, Inverted: false}, WaitPeriodMs: 5000}
   Observe: emulator answers 501 NotImplemented. Expected: 400 ValidationException with message "Invalid Rule provided.".
4. Call ListSafetyRules(ControlPanelArn=<panel>) and observe SafetyRules == [] (the refusal must have created nothing).
5. Call CreateSafetyRule with ONLY the AssertionRule member and observe it succeeds (200) and then appears as the single ListSafetyRules entry under key "ASSERTION" with the submitted Name.
6. Replay: route53-recovery-control-config.create-safety-rule-both-rule-types-rejected
