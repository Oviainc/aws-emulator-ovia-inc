Issue-Source: trace-sweep-divergence
Issue-Trace: route53-recovery-control-config-assertion-rule-over-deployed-routing-controls
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53-recovery-control-config-assertion-rule-over-deployed-routing-controls
Issue-Shape-Id: com.amazonaws.route53recoverycontrolconfig#Route53RecoveryControlConfig
Issue-Key: route53-recovery-control-config|DescribeRoutingControl|2xx->2xx|$.RoutingControl.Route53HealthCheckSequenceNumber
Issue-Key: route53-recovery-control-config|CreateSafetyRule|2xx->2xx|$.AssertionRule.Arn
Issue-Key: route53-recovery-control-config|CreateSafetyRule|2xx->2xx|$.AssertionRule.SafetyRuleArn
Issue-Key: route53-recovery-control-config|DescribeSafetyRule|2xx->2xx|$.AssertionRule.Arn
Issue-Key: route53-recovery-control-config|DescribeControlPanel|2xx->2xx|$.ControlPanel.Arn
Issue-Commit: 4d9e3fb251011fe781d884d5ecb8db5bb1e34e6b

# route53-recovery-control-config responses omit wire-only members Arn and Route53HealthCheckSequenceNumber

> _Note: this issue was identified against the code base at commit `4d9e3fb251011fe781d884d5ecb8db5bb1e34e6b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeRoutingControl

## Description
Four causes, all one family: the route53-recovery-control-config response serializers emit only the members declared in the public Smithy shapes and drop the additional members the real Recovery Control Configuration endpoint puts on the wire for the same resources. The HTTP status is correct (200) in every case; the response BODY is short of members a client can read. The four causes must all be closed before this issue is submitted.

Cause 1 of 4: DescribeRoutingControl omits RoutingControl.Route53HealthCheckSequenceNumber.
  AWS: recorded behavior only - the Smithy shape com.amazonaws.route53recoverycontrolconfig#RoutingControl declares only ControlPanelArn, Name, RoutingControlArn, Status and Owner, so the model says nothing about this member. The recording shows that for a routing control in a DEPLOYED control panel the service additionally returns RoutingControl.Route53HealthCheckSequenceNumber on the GET /routingcontrol/{RoutingControlArn} 200 response. The fixture asserts the member is PRESENT and shaped, not any particular value (the sequence number is a per-run counter and is not a requirement).
  Emulator: the 200 body has no $.RoutingControl.Route53HealthCheckSequenceNumber key at all (replay detail: "$.RoutingControl.Route53HealthCheckSequenceNumber: missing (expected '<shaped-not-asserted>', got None)"). Two steps in the fixture hit this.
  Why: a client that maps a routing control onto a Route 53 health check reads this sequence number to order/guard health-check state updates. Against the emulator the field is absent, so such a client either sees a null and skips the update or faults on a missing key - behavior that cannot happen against AWS.
  Fix: DescribeRoutingControl's response serializer / the routing-control state that backs it - the routing control record needs to carry a health-check sequence number and project it onto the response.
  Traces: route53-recovery-control-config-assertion-rule-over-deployed-routing-controls#task#7 (and #task#8).

Cause 2 of 4: CreateSafetyRule omits AssertionRule.Arn.
  AWS: recorded behavior only - the Smithy shape AssertionRule declares AssertedControls, ControlPanelArn, Name, RuleConfig, SafetyRuleArn, Status, WaitPeriodMs, Owner; it has no Arn member. The recording of POST /safetyrule shows the 200 response's AssertionRule carrying BOTH SafetyRuleArn and an Arn member, and both hold the SAME safety-rule ARN value (arn:aws:route53-recovery-control::<account>:controlpanel/<control-panel-id>/safetyrule/<safety-rule-id>). The requirement is that Arn is present and equal to SafetyRuleArn for the rule just created; the concrete control-panel id, safety-rule id and account in the recording are per-run values that fill that template, not part of it.
  Emulator: the 200 body's AssertionRule has no Arn key ("$.AssertionRule.Arn: missing (expected 'arn:aws:route53-recovery-control::000000000000:controlpanel/.../safetyrule/...', got None)"). The step also reports a SafetyRuleArn value_mismatch, but that is only the generated safety-rule id differing between the recording and this run (recorded 16-hex-character id vs the emulator's UUID-shaped id) - a per-run generated value, NOT part of this cause. Hypothesis: the emulator mints safety-rule ids as dashed UUIDs where AWS mints a flat hex id; that is noted only to explain the mismatch line and is not what must be fixed.
  Why: SDKs and tools that key a freshly created safety rule off the response's Arn member get null from the emulator and lose the handle to the rule they just created, so follow-up Describe/Update/Delete calls cannot be addressed.
  Fix: CreateSafetyRule's response serializer for the AssertionRule (and GatingRule) projection - emit Arn alongside SafetyRuleArn with the same value.
  Traces: route53-recovery-control-config-assertion-rule-over-deployed-routing-controls#task#9.

Cause 3 of 4: DescribeSafetyRule omits AssertionRule.Arn.
  AWS: recorded behavior only - same model gap as cause 2. The recording of GET /safetyrule/{SafetyRuleArn} shows the 200 response's AssertionRule carrying an Arn member equal to the safety rule's own ARN (the same value the request addressed and the same value SafetyRuleArn holds).
  Emulator: the 200 body's AssertionRule has no Arn key ("$.AssertionRule.Arn: missing (... got None)").
  Why: a client that reads the rule back and uses response.AssertionRule.Arn - for tagging, for logging, or to compare against the ARN it asked for - gets null and either fails its equality check or writes an empty ARN into downstream state.
  Fix: DescribeSafetyRule's response serializer / the shared AssertionRule projection used by Describe, List and Update.
  Traces: route53-recovery-control-config-assertion-rule-over-deployed-routing-controls#task#10.

Cause 4 of 4: DescribeControlPanel omits ControlPanel.Arn.
  AWS: recorded behavior only - the Smithy shape ControlPanel declares ClusterArn, ControlPanelArn, DefaultControlPanel, Name, RoutingControlCount, Status, Owner and no Arn member. The recording of GET /controlpanel/{ControlPanelArn} shows the 200 response's ControlPanel carrying an Arn member equal to the control panel's own ARN (arn:aws:route53-recovery-control::<account>:controlpanel/<control-panel-id>), i.e. the same value as ControlPanelArn; the id and account in the recording are per-run values.
  Emulator: the 200 body's ControlPanel has no Arn key ("$.ControlPanel.Arn: missing (... got None)").
  Why: clients and infrastructure tools that address a control panel by the generic Arn member of the returned resource read null against the emulator, so they cannot re-address the panel they just described.
  Fix: DescribeControlPanel's response serializer / the shared ControlPanel projection used by Describe, List, Create and Update.
  Traces: route53-recovery-control-config-assertion-rule-over-deployed-routing-controls#task#11.

Not covered by any open issue: the open issues for this service concern a missing ListAssociatedRoute53HealthChecks stub, the safety-rule error-path refusal classes (20261030T082444Z-route53-recovery-control-config-safety-rule-error-paths.md, which already owns the two 401/400 error-class steps of route53-recovery-control-config-validation-error-paths and is NOT re-filed here), safety rules never being evaluated, and CreateRoutingControl duplicate-name message wording. None of them concerns missing response members on successful 200 responses.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 74.1%, 7 of 27 steps mismatched). All four causes return the expected 200 status with a short body:

1. DescribeRoutingControl - fixture route53-recovery-control-config-assertion-rule-over-deployed-routing-controls, step task#7 (also task#8), expected 200 / actual 200:
   $.RoutingControl.Route53HealthCheckSequenceNumber: missing (expected '<shaped-not-asserted>', got None)

2. CreateSafetyRule - same fixture, step task#9, expected 200 / actual 200:
   $.AssertionRule.Arn: missing (expected 'arn:aws:route53-recovery-control::000000000000:controlpanel/6f5e3eee-197b-66c8-6ca6-2aea792eaf7a/safetyrule/3b9571c3891149ed', got None)
   $.AssertionRule.SafetyRuleArn: value_mismatch (expected '.../safetyrule/3b9571c3891149ed', got '.../safetyrule/358f8b3b-e158-85ab-de19-43a5df6e52d5') - generated-id difference only; the requirement is Arn present and equal to SafetyRuleArn.

3. DescribeSafetyRule - same fixture, step task#10, expected 200 / actual 200:
   $.AssertionRule.Arn: missing (expected the rule's own safety-rule ARN, got None)

4. DescribeControlPanel - same fixture, step task#11, expected 200 / actual 200:
   $.ControlPanel.Arn: missing (expected the panel's own control-panel ARN, got None)

Model evidence (smithy_query on services/service-com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig/smithy-model/com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig.json): shapes RoutingControl, AssertionRule and ControlPanel declare no Route53HealthCheckSequenceNumber and no Arn member, so these members are recorded wire behavior the public model does not describe. Operations confirmed: DescribeRoutingControl GET /routingcontrol/{RoutingControlArn} 200 -> {RoutingControl}; CreateSafetyRule POST /safetyrule 200 -> {AssertionRule, GatingRule}; DescribeSafetyRule GET /safetyrule/{SafetyRuleArn} 200 -> {AssertionRule, GatingRule}; DescribeControlPanel GET /controlpanel/{ControlPanelArn} 200 -> {ControlPanel}.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture route53-recovery-control-config-assertion-rule-over-deployed-routing-controls, which creates a cluster, a control panel and routing controls, waits for them to reach DEPLOYED, then creates and reads back an assertion safety rule over them.
3. Step task#7 (and task#8): GET /routingcontrol/{RoutingControlArn} for a DEPLOYED routing control. Observe the 200 body's RoutingControl has no Route53HealthCheckSequenceNumber member.
4. Step task#9: POST /safetyrule with an AssertionRule over those routing controls. Observe the 200 body's AssertionRule has SafetyRuleArn but no Arn member.
5. Step task#10: GET /safetyrule/{SafetyRuleArn} for the rule just created. Observe the 200 body's AssertionRule again has no Arn member.
6. Step task#11: GET /controlpanel/{ControlPanelArn}. Observe the 200 body's ControlPanel has no Arn member.
7. Expected in 3-6: each resource projection also carries the wire-only member - Route53HealthCheckSequenceNumber on RoutingControl, and an Arn equal to the resource's own ARN (SafetyRuleArn / ControlPanelArn) on AssertionRule and ControlPanel.
