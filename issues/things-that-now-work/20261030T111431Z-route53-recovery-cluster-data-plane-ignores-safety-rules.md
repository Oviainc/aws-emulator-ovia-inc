Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53-recovery-cluster.assertion-rule-blocks-violating-update-rejected route53-recovery-cluster.gating-rule-blocks-target-until-gate-open route53-recovery-cluster.safety-rule-override-bypasses-assertion-conflict
Issue-Shape-Id: com.amazonaws.route53recoverycluster#ToggleCustomerAPI
Issue-Key: route53-recovery-cluster.assertion-rule-blocks-violating-update-rejected|_assert_update_eventually_conflicts#11|Timeout|time.sleep(interval_s)
Issue-Key: route53-recovery-cluster.gating-rule-blocks-target-until-gate-open|_assert_update_eventually_conflicts#11|Timeout|time.sleep(interval_s)
Issue-Key: route53-recovery-cluster.safety-rule-override-bypasses-assertion-conflict|_assert_update_eventually_conflicts#11|Timeout|time.sleep(interval_s)
Issue-Commit: bc82654c409a33f503ba5ef7384156730ff2d370

# route53-recovery-cluster data plane ignores safety rules entirely (3 net-new causes): assertion rules, gating rules and SafetyRulesToOverride are all unevaluated, so every rule-violating UpdateRoutingControlState is accepted with 200

> _Note: this issue was identified against the code base at commit `bc82654c409a33f503ba5ef7384156730ff2d370`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateRoutingControlState

## Description
Three distinct, independently-fixable root causes in the route53-recovery-cluster (ToggleCustomerAPI, awsJson1_0) data plane. All three stem from the same omission area — the UpdateRoutingControlState handler performs NO safety-rule evaluation at all — but they are three separate behaviors a fixer must implement, and each is pinned by its own failing check. The handler's own module header states the gap: "`SafetyRulesToOverride` is accepted and IGNORED in this first slice: safety-rule assertion and gating is left to its own gap issue." This is that issue. Note that safety rules can now be CREATED: the control plane's CreateSafetyRule stub was fixed (20261029T191943Z-route53-recovery-control-config-createsafetyrule-stub.md), so these checks get real rules to DEPLOYED and only then discover the data plane does not honour them. The one open issue for this service (20261029T203927Z-route53-recovery-cluster-data-plane-3-net-new.md) covers GetRoutingControlState authorization and the ServiceLimitExceededException / malformed-ARN message wording; it does not cover safety-rule evaluation, and none of the three causes below are among its Issue-Trace-Ids.

Shared symptom shape: each of the three checks drives the violating update through a bounded probe loop (`_assert_update_eventually_conflicts`, 90s budget) that tolerates brief control-plane-to-data-plane propagation lag by restoring the control and retrying whenever the violating update unexpectedly SUCCEEDS. Against the emulator the violating update succeeds on every attempt forever, so the loop never exits and the check is killed by the 60s harness timeout. The reported verdict is therefore "Check timed out after 60.0s" — the timeout is the SHAPE of the failure, not a slow emulator: the emulator answered every one of those calls promptly, it just answered 200 where AWS refuses.

Cause 1 of 3: an assertion safety rule does not block an UpdateRoutingControlState that would violate it.
AWS: an assertion rule attaches to a control panel with AssertedControls plus RuleConfig {Type, Threshold, Inverted}. For Type ATLEAST with Threshold N and Inverted false, AWS evaluates the proposed post-update state of the asserted control set and refuses any UpdateRoutingControlState that would leave fewer than N of those controls On. The refusal is the modeled ValidationException (Smithy: UpdateRoutingControlState errors include ValidationException, errorKind client, httpError 400; shape doc "There was a validation error on the request."), and the targeted control's state is left UNCHANGED. The operation's own model documentation names this contract explicitly — "you can add safety rules for routing controls, which are safeguards for routing control state updates that help prevent unexpected outcomes, like fail open traffic routing". Message: recorded behavior only — the recording asserts the error CODE (ValidationException); it does not capture the message string, so the message text is not pinned by this cause. The requirement here is the 400 ValidationException refusal and the absence of the state change.
Emulator: the update is applied unconditionally. With rc_a=On, rc_b=Off and a deployed ATLEAST/Threshold=1 rule over {rc_a, rc_b}, setting rc_a to Off — which would leave ZERO of the two asserted controls On — returns HTTP 200 with an empty UpdateRoutingControlStateResponse and the control really moves to Off. services/service-com-amazonaws-route53recoverycluster-togglecustomerapi/src/ops/update_routing_control_state.rs lines 171-251: `handle` parses the state, resolves the control in the registry and calls `plane.set_state(...)` directly; there is no control-panel lookup, no rule set lookup and no assertion evaluation anywhere between validation and the write.
Why: a client gets the exact outcome ARC's safety rules exist to prevent — fail-open traffic routing. A failover runbook or test that proves "the last healthy cell cannot be switched off" passes against the emulator and then takes the application fully offline against AWS, and code with a ValidationException handler for the blocked-by-safety-rule path (back off, pick a different cell, page a human, or re-issue with an override) never executes that branch and is shipped untested.
Fix: the UpdateRoutingControlState handler (and the batch UpdateRoutingControlStates handler, which must apply the same gate over the whole proposed entry set atomically) — it must resolve the control's control panel, load that panel's deployed assertion rules from state, evaluate each rule's RuleConfig against the proposed post-update state of its AssertedControls, and return the modeled ValidationException (400) without mutating state when any rule would be violated.
Traces: route53-recovery-cluster.assertion-rule-blocks-violating-update-rejected.

Cause 2 of 3: a gating safety rule does not block updates to its target routing controls while the gate is closed.
AWS: a gating rule attaches to a control panel with GatingControls, TargetControls and RuleConfig {Type, Threshold, Inverted}. AWS permits an UpdateRoutingControlState against a control listed in TargetControls only while the gating control set satisfies the rule config; while the gate is Off the update to the target is refused with the modeled ValidationException (400) and the target's state is left unchanged. The gating control itself is not a target of its own rule, so updating the GATE is unaffected by the rule; once the gate is turned On, the identical update to the target must be accepted and the target must transition. Message: recorded behavior only — the recording asserts the error CODE (ValidationException) and the unchanged target state, not the message string.
Emulator: with gate=Off, target=Off and a deployed OR/Threshold=0 gating rule over gate -> target, setting the target to On returns HTTP 200 and the target transitions. The gate's state is never consulted because the handler never loads the panel's gating rules at all (same code path as Cause 1).
Why: the gate is the emulator's only representation of "this cell is not cleared to take traffic". Ignoring it means a client can route traffic to a cell its operator has deliberately gated shut, and — symmetrically — a client that implements the gate-then-target ordering has no way to verify that ordering is actually required, so a regression that drops the gate step is invisible in testing.
Fix: the same evaluation point as Cause 1 in the UpdateRoutingControlState / UpdateRoutingControlStates handlers, extended to gating rules: when the requested control appears in a deployed rule's TargetControls, evaluate that rule's RuleConfig against the CURRENT state of its GatingControls and refuse with ValidationException (400), leaving state untouched, when the gate does not satisfy it.
Traces: route53-recovery-cluster.gating-rule-blocks-target-until-gate-open.

Cause 3 of 3: SafetyRulesToOverride is read and discarded, so the break-glass override is not implemented.
AWS: SafetyRulesToOverride (Smithy: UpdateRoutingControlState.SafetyRulesToOverride -> list Arns, optional) carries the ARNs of safety rules to EXCLUDE from evaluation for this one call. An update that the named rule would otherwise refuse with ValidationException must then succeed, and the routing control must actually transition to the requested rule-violating state. Rules NOT named in the list must still be evaluated and must still refuse. The operation's model documentation describes exactly this: "In a 'break glass' scenario like this, you can override one or more safety rules to change a routing control state and fail over your application. The SafetyRulesToOverride property enables you override one or more safety rules and update routing control states." Message: not applicable — the required outcome is a 200 and the observable state transition.
Emulator: update_routing_control_state.rs lines 180-182 read the member only to silence the unused warning — `let _ = &input.safety_rules_to_override;` under the comment "accepted and ignored in this slice ... no safety rule is asserted yet, so there is none to override". Today this is masked by Causes 1 and 2: because nothing is ever refused, an overridden update trivially appears to succeed. That makes this cause a SEPARATE work item rather than a consequence of the others — a fixer who implements assertion evaluation (Cause 1) but leaves this line as-is will convert the check's final beat into a hard ValidationException failure, and the check still fails. Both halves must land together.
Why: break-glass is the escape hatch operators reach for during an incident, when the safety rule is the thing standing between them and a recovery. A client cannot develop or rehearse an override-based failover path against the emulator: either it looks like it works for the wrong reason (today), or it breaks the moment enforcement is added (after a partial fix).
Fix: the UpdateRoutingControlState / UpdateRoutingControlStates handlers — thread SafetyRulesToOverride into the rule-evaluation step added by Causes 1 and 2 and skip any deployed rule whose SafetyRuleArn appears in it, while still evaluating every rule that does not.
Traces: route53-recovery-cluster.safety-rule-override-bypasses-assertion-conflict.

## Evidence
Suite: route53-recovery-cluster 11/17 passed, 6 mismatches. 3 are OWNED by 20261029T203927Z-route53-recovery-cluster-data-plane-3-net-new.md and are excluded here. The 3 clustered into this issue are the safety-rule checks, each reported as "[mismatch] ... Error: Check timed out after 60.0s".

Why a timeout is the observed verdict for all three: each check calls the inlined helper

    def _assert_update_eventually_conflicts(cluster, rc_arn, violating_state, restore_state, timeout_s=90.0, interval_s=5.0):
        deadline = time.monotonic() + timeout_s
        while time.monotonic() < deadline:
            try:
                cluster.update_routing_control_state(RoutingControlArn=rc_arn, RoutingControlState=violating_state)
            except ClientError as e:
                if e.response["Error"]["Code"] == "ValidationException": return
                raise AssertionError(...)
            else:
                cluster.update_routing_control_state(RoutingControlArn=rc_arn, RoutingControlState=restore_state)
                time.sleep(interval_s)
        raise AssertionError(f"safety rule never blocked the violating update within {timeout_s}s ...")

The `else` branch is the "it SUCCEEDED" branch. Against the emulator every attempt takes it, so the loop runs its 90s budget and the 60s harness timeout fires first. The emulator answered each UpdateRoutingControlState promptly with 200; the timeout reflects an unending success loop, not latency.

Cause 1 — route53-recovery-cluster.assertion-rule-blocks-violating-update-rejected [mismatch]
Setup: two routing controls rc_a, rc_b in the shared cluster's default control panel, both DEPLOYED; rc_a driven to On, rc_b to Off (both confirmed via GetRoutingControlState); then CreateSafetyRule with AssertionRule {WaitPeriodMs 5000, AssertedControls [rc_a, rc_b], RuleConfig {Type "ATLEAST", Threshold 1, Inverted false}}, waited to Status DEPLOYED via DescribeSafetyRule.
Call: UpdateRoutingControlState(RoutingControlArn=rc_a, RoutingControlState="Off") — would leave 0 of 2 asserted controls On, below Threshold 1.
Expected (AWS): ClientError with code "ValidationException" (HTTP 400), and GetRoutingControlState(rc_a) still reporting "On" afterwards. The positive control that follows — UpdateRoutingControlState(rc_b, "On"), which keeps the rule satisfied — must succeed.
Actual (emulator): HTTP 200, empty UpdateRoutingControlStateResponse, rc_a actually moves to Off; the probe loop restores it to On and retries indefinitely until the harness kills the check at 60s.

Cause 2 — route53-recovery-cluster.gating-rule-blocks-target-until-gate-open [mismatch]
Setup: routing controls gate and target, both DEPLOYED and both driven to Off; CreateSafetyRule with GatingRule {WaitPeriodMs 5000, GatingControls [gate], TargetControls [target], RuleConfig {Type "OR", Threshold 0, Inverted false}}, waited to DEPLOYED.
Call: UpdateRoutingControlState(RoutingControlArn=target, RoutingControlState="On") while gate is Off.
Expected (AWS): code "ValidationException" (400) and GetRoutingControlState(target) still "Off". Then UpdateRoutingControlState(gate, "On") succeeds (the gate is not a target of its own rule), after which the identical target update succeeds and GetRoutingControlState(target) reports "On".
Actual (emulator): the target update returns HTTP 200 and the target transitions to On with the gate still Off; the probe loop restores and retries to the 60s harness timeout.

Cause 3 — route53-recovery-cluster.safety-rule-override-bypasses-assertion-conflict [mismatch]
Setup: identical to Cause 1 (rc_a On, rc_b Off, ATLEAST/Threshold 1 AssertionRule over both, DEPLOYED), capturing the rule's SafetyRuleArn.
Calls: (a) UpdateRoutingControlState(rc_a, "Off") with NO override — must be refused with "ValidationException" (400), rc_a unchanged at "On"; (b) the same call with SafetyRulesToOverride=[<that rule's SafetyRuleArn>] — must return 200 and rc_a must actually become "Off".
Actual (emulator): beat (a) succeeds with HTTP 200 instead of being refused, so the check never reaches beat (b) and times out at 60s. Beat (b)'s requirement is not exercised at all today; source inspection shows it cannot hold once (a) is fixed, because services/service-com-amazonaws-route53recoverycluster-togglecustomerapi/src/ops/update_routing_control_state.rs lines 180-182 discard the member (`let _ = &input.safety_rules_to_override;`).

Source: services/service-com-amazonaws-route53recoverycluster-togglecustomerapi/src/ops/update_routing_control_state.rs — module header lines 12-13 declare the gap ("SafetyRulesToOverride is accepted and IGNORED in this first slice: safety-rule assertion and gating is left to its own gap issue"); `handle` lines 171-251 go validation -> registry lookup -> `plane.set_state(...)` with no rule evaluation; lines 180-182 discard SafetyRulesToOverride. ValidationException is modeled on both UpdateRoutingControlState and UpdateRoutingControlStates as errorKind client, httpError 400, so no new error shape is needed. All cluster ids, control panel/routing control ARNs, control names and timestamps in the replay output are per-run values and are not part of the requirement.

## Steps to Reproduce
Shared setup for all three causes: via route53-recovery-control-config, CreateCluster (or reuse the shared cluster) and wait for Status DEPLOYED; read its ClusterEndpoints and build the route53-recovery-cluster client against the endpoint for the current region (there is no generic regional endpoint for this data plane); take the cluster's default control panel ARN from ListControlPanels.

Cause 1 (assertion rule not enforced):
1. CreateRoutingControl twice in the cluster (rc_a, rc_b); wait for each to reach Status DEPLOYED via DescribeRoutingControl.
2. UpdateRoutingControlState(rc_a, "On") and UpdateRoutingControlState(rc_b, "Off"); confirm both via GetRoutingControlState.
3. CreateSafetyRule with AssertionRule {Name, ControlPanelArn, WaitPeriodMs 5000, AssertedControls [rc_a, rc_b], RuleConfig {Type "ATLEAST", Threshold 1, Inverted false}}; poll DescribeSafetyRule until Status is DEPLOYED.
4. Call UpdateRoutingControlState(RoutingControlArn=rc_a, RoutingControlState="Off").
   Expected: ValidationException (400); GetRoutingControlState(rc_a) still "On". Observed: HTTP 200 and rc_a becomes "Off".
5. Positive control (must keep working after the fix): UpdateRoutingControlState(rc_b, "On") — does not violate the rule — must succeed and rc_b must report "On".

Cause 2 (gating rule not enforced):
1. CreateRoutingControl twice (gate, target); wait for DEPLOYED; drive both to "Off" and confirm via GetRoutingControlState.
2. CreateSafetyRule with GatingRule {Name, ControlPanelArn, WaitPeriodMs 5000, GatingControls [gate], TargetControls [target], RuleConfig {Type "OR", Threshold 0, Inverted false}}; poll DescribeSafetyRule until DEPLOYED.
3. Call UpdateRoutingControlState(RoutingControlArn=target, RoutingControlState="On").
   Expected: ValidationException (400); GetRoutingControlState(target) still "Off". Observed: HTTP 200 and target becomes "On".
4. UpdateRoutingControlState(gate, "On") — the gate is not a target of its own rule, so this must succeed; confirm gate reports "On".
5. Repeat step 3: it must now succeed and GetRoutingControlState(target) must report "On". (Allow a bounded retry here: the rule's WaitPeriodMs is a flap-prevention cooldown on the TARGET control that ticks from any prior change to it, so a legitimate update may be refused with ValidationException for a few seconds after an earlier toggle.)

Cause 3 (SafetyRulesToOverride ignored):
1. Repeat Cause 1 steps 1-3, keeping the AssertionRule's SafetyRuleArn from the CreateSafetyRule response.
2. Call UpdateRoutingControlState(rc_a, "Off") with no override.
   Expected: ValidationException (400), rc_a unchanged at "On". Observed: HTTP 200.
3. Call UpdateRoutingControlState(RoutingControlArn=rc_a, RoutingControlState="Off", SafetyRulesToOverride=[<SafetyRuleArn from step 1>]).
   Expected: HTTP 200 and GetRoutingControlState(rc_a) reporting "Off" — the override forces the otherwise-blocked transition through. This beat is unreachable today because step 2 does not refuse, and it will FAIL if assertion evaluation is added without wiring SafetyRulesToOverride into it.
4. Regression guard for the override scope: with a SECOND deployed rule that the same update also violates and that is NOT named in SafetyRulesToOverride, step 3 must still be refused with ValidationException (400).
