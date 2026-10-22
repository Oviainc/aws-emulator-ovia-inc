Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53-recovery-cluster.routing-control-state-toggle-round-trip route53-recovery-cluster.list-routing-controls-reflects-current-state route53-recovery-cluster.list-routing-controls-pagination-next-token route53-recovery-cluster.routing-controls-scoped-to-control-panel route53-recovery-cluster.routing-control-independent-state-isolation route53-recovery-cluster.update-routing-control-state-idempotent-same-state-no-conflict route53-recovery-cluster.update-routing-control-states-batch-atomic-multi-control route53-recovery-cluster.update-routing-control-states-preserves-unspecified-controls-state route53-recovery-cluster.batch-update-service-limit-exceeded-rejected route53-recovery-cluster.assertion-rule-blocks-violating-update-rejected route53-recovery-cluster.gating-rule-blocks-target-until-gate-open route53-recovery-cluster.safety-rule-override-bypasses-assertion-conflict route53-recovery-cluster.concurrent-updates-same-routing-control-no-corruption route53-recovery-cluster.validation-exception-malformed-routing-control-arn-rejected route53-recovery-cluster.access-denied-missing-permission-rejected route53-recovery-cluster.resource-not-found-nonexistent-routing-control route53-recovery-cluster.list-routing-controls-nonexistent-control-panel-not-found
Issue-Shape-Id: com.amazonaws.route53recoverycluster#ToggleCustomerAPI
Issue-Key: route53-recovery-cluster|UpdateRoutingControlState|ResourceNotFoundException
Issue-Key: route53-recovery-cluster|ListRoutingControls|ResourceNotFoundException
Issue-Key: route53-recovery-cluster.resource-not-found-nonexistent-routing-control|run#10|AssertionError|assert get_message == expected_message, f"expected {expected_message!r}, got {get_message!r}"
Issue-Key: route53-recovery-cluster.list-routing-controls-nonexistent-control-panel-not-found|run#11|AssertionError|assert message == expected_message, f"expected {expected_message!r}, got {message!r}"
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# route53-recovery-cluster data plane is backed by a fixed seed fleet instead of the recovery-control-config registry, and its ResourceNotFoundException message templates are wrong

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateRoutingControlState

## Description
Three distinct root causes in the route53-recovery-cluster (ToggleCustomerAPI) data plane. All 17 behavioral checks for this service fail; 15 of them never get past their first data-plane call.

=== Cause 1 of 3: the data plane cannot see any routing control or control panel created through route53-recovery-control-config (15 checks) ===

AWS: Route 53 ARC splits one resource namespace across two APIs. route53-recovery-control-config (control plane) CREATES clusters, control panels and routing controls; route53-recovery-cluster (data plane, reached at one of the cluster's 5 ClusterEndpoints) only READS and TOGGLES them. Smithy: the route53-recovery-cluster model (sdk_id "Route53 Recovery Cluster", awsJson1_0) binds exactly 4 operations — GetRoutingControlState, ListRoutingControls, UpdateRoutingControlState, UpdateRoutingControlStates — and models NO create/delete operation, confirming that every resource the data plane serves originates in the control plane. Once CreateRoutingControl's resource reaches Status=DEPLOYED (DescribeRoutingControl), GetRoutingControlState/UpdateRoutingControlState on that RoutingControlArn and ListRoutingControls on its ControlPanelArn must succeed (initial RoutingControlState "Off"), and the listed entry must carry accurate RoutingControlName / ControlPanelArn / ControlPanelName / Owner. Smithy: RoutingControl members ControlPanelArn, ControlPanelName, RoutingControlArn, RoutingControlName, RoutingControlState, Owner.

Emulator: the data-plane registry is a hard-coded fixture, unrelated to anything the control plane created. services/service-com-amazonaws-route53recoverycluster-togglecustomerapi/src/state.rs defines SEED_FLEET (EmulatorCluster1/2, EmulatorControlPanelA/B/C, EmulatorRoutingControl1..5) and materialises it in `impl Default for ComAmazonawsRoute53recoveryclusterTogglecustomerapiState`; the module doc itself says "When the recovery-control-config emulator grows a registry of its own, this module's Default is the single place that should read it (through a crate shared by both services) instead of the fixed table". That wiring was never done. So every check that creates a cluster + routing control via route53-recovery-control-config, waits for DEPLOYED, and then calls the data plane gets ResourceNotFoundException (404) for a resource that demonstrably exists: e.g. UpdateRoutingControlState -> "Routing control arn:aws:route53-recovery-control::<acct>:controlpanel/<panel-id>/routingcontrol/<rc-id> was not found", and ListRoutingControls(ControlPanelArn=<the real panel>) -> "Control panel arn:aws:route53-recovery-control::<acct>:controlpanel/<panel-id> was not found".

Why: the data plane is unusable for any resource a client actually owns. Nothing a client creates can ever be read or toggled, so no real ARC workflow (create cluster -> create routing control -> flip it On to shift traffic) can run at all; the only addressable routing controls are ones no client can discover from the control plane. Every state, safety-rule, pagination, batch-update and authorization behavior of this service is unreachable behind this one gap.

Fix: the data-plane state module — ComAmazonawsRoute53recoveryclusterTogglecustomerapiState must read the routing-control/control-panel registry owned by route53-recovery-control-config (a store shared by both service crates) rather than materialising SEED_FLEET, so a control-plane create is visible to GetRoutingControlState/ListRoutingControls/UpdateRoutingControlState(s) and a control-plane delete removes it. The four operation handlers need no change once the registry is shared.

Traces: route53-recovery-cluster.routing-control-state-toggle-round-trip, .list-routing-controls-reflects-current-state, .list-routing-controls-pagination-next-token, .routing-controls-scoped-to-control-panel, .routing-control-independent-state-isolation, .update-routing-control-state-idempotent-same-state-no-conflict, .update-routing-control-states-batch-atomic-multi-control, .update-routing-control-states-preserves-unspecified-controls-state, .batch-update-service-limit-exceeded-rejected, .assertion-rule-blocks-violating-update-rejected, .gating-rule-blocks-target-until-gate-open, .safety-rule-override-bypasses-assertion-conflict, .concurrent-updates-same-routing-control-no-corruption, .validation-exception-malformed-routing-control-arn-rejected, .access-denied-missing-permission-rejected. State source: services/service-com-amazonaws-route53recoverycluster-togglecustomerapi/src/state.rs lines 86-106 (SEED_FLEET) and 209-252 (Default).

=== Cause 2 of 3: the routing-control ResourceNotFoundException message does not follow AWS's template (1 check) ===

AWS: for a syntactically valid but non-existent RoutingControlArn, GetRoutingControlState and UpdateRoutingControlState both answer ResourceNotFoundException (HTTP 404 — Smithy: ResourceNotFoundException errorKind client, httpError 404, "The request references a routing control or control panel that was not found"). The message is built from the request and its rule is therefore known: it is the literal template `Routing Control '<RoutingControlArn>' not found` — capital "Control", the ARN single-quoted, trailing " not found". Recorded example: "Routing Control 'arn:aws:route53-recovery-control::000000000000:controlpanel/d631056f-.../routingcontrol/980947c3-...' not found".

Emulator: returns the right code and status but a different template, `Routing control <arn> was not found` — lowercase "control", no quotes around the ARN, "was not found" suffix. See get_routing_control_state.rs lines 184-196.

Why: a client that classifies or asserts on the data-plane not-found message (ARC runbooks and failover tooling commonly log/compare it, and the ARN is extracted from the quoted segment) parses nothing from the emulator's text. The code is right, so the failure is silent until the message is consumed.

Fix: the not-found message construction in the GetRoutingControlState and UpdateRoutingControlState handlers (the same template is used by both; also set resource_id/resource_type consistently).

Traces: route53-recovery-cluster.resource-not-found-nonexistent-routing-control. Emulator source: services/service-com-amazonaws-route53recoverycluster-togglecustomerapi/src/ops/get_routing_control_state.rs lines 186-193.

=== Cause 3 of 3: the control-panel ResourceNotFoundException message does not follow AWS's template (1 check) ===

AWS: ListRoutingControls filtered by a well-formed but non-existent ControlPanelArn answers ResourceNotFoundException (HTTP 404, same modeled shape as above). The message is built from the request, so its rule is known: the template is `Control panel not found: '<ControlPanelArn>'` — the ARN single-quoted and placed AFTER a "not found:" prefix. Recorded example: "Control panel not found: 'arn:aws:route53-recovery-control::000000000000:controlpanel/db4d8d98ff1a450c8ce8864435b4278f'".

Emulator: returns code ResourceNotFoundException with `Control panel <arn> was not found` — the opposite word order, no quoting.

Why: same as cause 2, for the control-panel filter path; note this template DIFFERS from the routing-control one in AWS (prefix vs suffix form), so a single shared message helper cannot satisfy both.

Fix: the ListRoutingControls handler's ControlPanelArn-not-found branch.

Traces: route53-recovery-cluster.list-routing-controls-nonexistent-control-panel-not-found.

## Evidence
Suite: route53-recovery-cluster 0/17 passed (0%). 15 errored, 2 mismatched, 0 skipped, 0 unloadable. No failure was marked ALREADY FILED or OWNED by an existing issue, and no open issue exists for route53-recovery-cluster (open route53 issues cover route53resolver, route53-recovery-control-config and route53-recovery-readiness only).

Cause 1 — expected: after cfg.create_cluster + cfg.create_routing_control and DescribeRoutingControl Status=DEPLOYED, the data-plane call on the returned ARN succeeds. Actual: botocore.errorfactory.ResourceNotFoundException when calling the UpdateRoutingControlState operation: "Routing control arn:aws:route53-recovery-control::000000000000:controlpanel/19e6323f-563f-17ba-9588-8c289dee015a/routingcontrol/95888c28-9dee-015a-2e99-6771f283017f was not found" (routing-control-state-toggle-round-trip; 12 other checks show the identical shape with their own ARNs). ListRoutingControls form: ResourceNotFoundException "Control panel arn:aws:route53-recovery-control::000000000000:controlpanel/8c890345-4431-b865-c0b8-cd05df530e69 was not found" (list-routing-controls-pagination-next-token, routing-controls-scoped-to-control-panel). Root cause visible in the checkout: state.rs SEED_FLEET + Default materialise fixed EmulatorRoutingControl1..5 ARNs and nothing reads the control-plane registry.

Cause 2 — AssertionError: expected "Routing Control 'arn:aws:route53-recovery-control::000000000000:controlpanel/d631056f-7dc8-3638-9809-47c3a38192af/routingcontrol/980947c3-a381-92af-116c-f0ad5d227c1e' not found", got 'Routing control arn:aws:route53-recovery-control::000000000000:controlpanel/d631056f-7dc8-3638-9809-47c3a38192af/routingcontrol/980947c3-a381-92af-116c-f0ad5d227c1e was not found'. Error code matched (ResourceNotFoundException); only the template diverges. Emulator source of the diverging template: get_routing_control_state.rs format!("Routing control {} was not found", input.routing_control_arn).

Cause 3 — AssertionError: expected "Control panel not found: 'arn:aws:route53-recovery-control::000000000000:controlpanel/db4d8d98ff1a450c8ce8864435b4278f'", got 'Control panel arn:aws:route53-recovery-control::000000000000:controlpanel/db4d8d98ff1a450c8ce8864435b4278f was not found'. Error code matched (ResourceNotFoundException).

## Steps to Reproduce
Cause 1:
1. cfg = client("route53-recovery-control-config"); create_cluster(ClusterName=...); poll describe_cluster until Status == DEPLOYED.
2. panel_arn = list_control_panels(ClusterArn=arn)["ControlPanels"][0]["ControlPanelArn"] (the cluster's default panel).
3. rc_arn = create_routing_control(ClusterArn=arn, RoutingControlName=...)["RoutingControl"]["RoutingControlArn"]; poll describe_routing_control until Status == DEPLOYED.
4. cluster = client("route53-recovery-cluster", endpoint_url=<one of describe_cluster's ClusterEndpoints>).
5. cluster.update_routing_control_state(RoutingControlArn=rc_arn, RoutingControlState="On") -> expect 200/empty; observe ResourceNotFoundException 404.
6. cluster.list_routing_controls(ControlPanelArn=panel_arn) -> expect the routing control listed with state Off/On; observe ResourceNotFoundException 404 for the control panel.

Cause 2:
1. Take a real rc_arn as above (or any well-formed ARN) and flip one hex character of its final segment to get a non-existent, same-shaped ARN.
2. cluster.get_routing_control_state(RoutingControlArn=bogus_arn) and cluster.update_routing_control_state(RoutingControlArn=bogus_arn, RoutingControlState="On").
3. Both must fail with ResourceNotFoundException and Message exactly "Routing Control '<bogus_arn>' not found"; observe "Routing control <bogus_arn> was not found".

Cause 3:
1. bogus_panel = f"arn:aws:route53-recovery-control::{account}:controlpanel/{uuid4().hex}".
2. cluster.list_routing_controls(ControlPanelArn=bogus_panel).
3. Must fail with ResourceNotFoundException and Message exactly "Control panel not found: '<bogus_panel>'"; observe "Control panel <bogus_panel> was not found".
