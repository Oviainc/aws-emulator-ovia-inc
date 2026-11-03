Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53-recovery-control-config.concurrent-duplicate-routing-control-name-single-winner
Issue-Shape-Id: com.amazonaws.route53recoverycontrolconfig#Route53RecoveryControlConfig
Issue-Key: route53-recovery-control-config.concurrent-duplicate-routing-control-name-single-winner|run#15|AssertionError|assert message.startswith(prefix), f"expected message starting {prefix!r}, got {message!r}"
Issue-Commit: 085aca81986f06b9d958c778a1487573b77f6255

# CreateRoutingControl duplicate-name refusal uses non-AWS message wording

> _Note: this issue was identified against the code base at commit `085aca81986f06b9d958c778a1487573b77f6255`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRoutingControl

## Description
One root cause (Cause 1 of 1).

Cause 1 of 1: the ValidationException message the emulator returns when CreateRoutingControl is called with a RoutingControlName that already exists in the target control panel does not match AWS's wording for that error class. Failing check: route53-recovery-control-config.concurrent-duplicate-routing-control-name-single-winner.

AWS: recorded behavior only — the Smithy model for CreateRoutingControl declares ValidationException (HTTP 400, "400 response - Multiple causes. For example, you might have a malformed query string and input parameter might be out of range...") but models no message text, so the requirement comes from the recording. AWS refuses the duplicate name with code ValidationException and a message whose FIXED text is the prefix "A routing control with the specified name already exists: " and which ends with ", <RoutingControlName>". The VARIABLE parts are a server-chosen id of the already-existing resource (immediately after the prefix) and the requested RoutingControlName (the trailing component, taken verbatim from the request). Template: "A routing control with the specified name already exists: <id>, <RoutingControlName>". The status (400) and the error code (ValidationException) the emulator returns are already correct; only the message diverges.

Emulator: the handler returns ValidationException with the message "Routing Control <name> already exists in Control Panel <control-panel-arn>." — e.g. the replay observed 'Routing Control sim2real-r53arc-race-8c31a2b07a96 already exists in Control Panel arn:aws:route53-recovery-control::000000000000:controlpanel/ab261035-d689-0e64-bde2-4bcee02239c0.' (the name and ARN shown are that run's values, not part of the requirement). It has neither AWS's fixed prefix nor the trailing ", <name>" form, and it substitutes the control panel ARN where AWS names the colliding resource's id.

Why: clients and tooling that classify or parse this refusal by message — distinguishing a name collision from the other ValidationException causes this operation multiplexes (unknown cluster ARN, unknown control panel ARN, cross-cluster panel), since all four share code ValidationException and status 400 — cannot recognise the collision against the emulator. Anything matching AWS's documented prefix, or extracting the offending name from the message tail, silently fails, so a racing creator cannot tell "name taken" from "malformed/unknown reference" and may retry forever or surface the wrong diagnosis.

Fix: the shared refusal wording used by the duplicate-name branch of the CreateRoutingControl handler — the already_exists_message helper in the service's state module, as called from the name-collision check in the handler. The branch's error class and status are correct and must not change; only the message template must be reworded to AWS's, with the colliding resource's id and the requested RoutingControlName filled from state and from the request respectively.

Traces: route53-recovery-control-config.concurrent-duplicate-routing-control-name-single-winner (5 concurrent CreateRoutingControl calls for one name: 1 success + 4 ValidationException losers, each loser's message diverging). File/line citations: services/service-com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig/src/ops/create_routing_control.rs lines 273-289 (the collision branch, calling crate::state::already_exists_message with ROUTING_CONTROL_RESOURCE / CONTROL_PANEL_RESOURCE); the wording itself lives in services/service-com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig/src/state.rs (already_exists_message).

## Evidence
Check route53-recovery-control-config.concurrent-duplicate-routing-control-name-single-winner, assertion at check_route53_recovery_control_config_concurrent_duplicate_routing_control_name_single_winner.py line 66:

AssertionError: expected message starting 'A routing control with the specified name already exists: ', got 'Routing Control sim2real-r53arc-race-8c31a2b07a96 already exists in Control Panel arn:aws:route53-recovery-control::000000000000:controlpanel/ab261035-d689-0e64-bde2-4bcee02239c0.'

Expected (AWS): HTTP 400, Error.Code = "ValidationException", Error.Message = "A routing control with the specified name already exists: <id>, <RoutingControlName>" — fixed text "A routing control with the specified name already exists: " and the ", " separator; variable parts: the existing resource's server-chosen id and the RoutingControlName from the request.
Actual (emulator): HTTP 400, Error.Code = "ValidationException" (correct), Error.Message = "Routing Control <RoutingControlName> already exists in Control Panel <ControlPanelArn>."

The rest of the check passed: exactly one of the 5 concurrent creates succeeded, the other 4 were refused with ValidationException, and ListRoutingControls showed exactly one control of that name with the winner's ARN. The divergence is confined to the message text.

## Steps to Reproduce
1. CreateCluster, then CreateControlPanel in that cluster.
2. Call CreateRoutingControl(ClusterArn, ControlPanelArn, RoutingControlName="dup-name") once — it succeeds.
3. Call CreateRoutingControl again with the SAME ClusterArn/ControlPanelArn/RoutingControlName (or issue 5 such calls concurrently, as the check does).
4. Observe the refusal: status 400 and code ValidationException are correct, but Error.Message is "Routing Control dup-name already exists in Control Panel <panel-arn>." instead of AWS's "A routing control with the specified name already exists: <id>, dup-name".
