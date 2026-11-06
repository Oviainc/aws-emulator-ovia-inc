Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.enable-route-server-propagation-without-association-blocked ec2.op-contract-disable-route-server-propagation ec2.route-server-lifecycle-full ec2.route-server-routing-database-reflects-learned-routes ec2.route-table-local-route-and-main-association-immutable ec2.op-contract-send-diagnostic-interrupt
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.enable-route-server-propagation-without-association-blocked|run#13|AssertionError|assert message == f"Route Server {rs_id} is not associated with a VPC.", f"unexpected InvalidRouteServerId.NotAssociated message: {message!r}"
Issue-Key: ec2.op-contract-disable-route-server-propagation|run#17|AssertionError|assert code == "DryRunOperation", f"expected DryRunOperation, got {code}"
Issue-Key: ec2.route-server-lifecycle-full|run#52|AssertionError|assert err.get("Message") == f"Route Server {rs_id} is in an invalid state.", (
Issue-Key: ec2.route-server-routing-database-reflects-learned-routes|run#33|AssertionError|assert database["AreRoutesPersisted"] is False, (
Issue-Key: ec2|DisassociateRouteTable|InvalidAssociationID.NotFound
Issue-Key: ec2.op-contract-send-diagnostic-interrupt|run#11|AssertionError|assert message == f"The instance ID '{bogus_id}' is malformed", message
Issue-Commit: 7ff327d821bcd1f37fa31be7f3b9efbce7107294

# ec2 route-server / route-table / diagnostic-interrupt refusals: 6 net-new causes — DryRun not short-circuited, main-association disassociate answered NotFound, and four emulator-authored refusal messages

> _Note: this issue was identified against the code base at commit `7ff327d821bcd1f37fa31be7f3b9efbce7107294`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
EnableRouteServerPropagation

## Description
Six net-new root causes in EC2's route-server, route-table and diagnostic-interrupt paths. Each is a separate defect; all six must be closed before this issue is submitted. Near neighbours that do NOT cover these: 20261029T175114Z-ec2-dryrun-contract-authorized-dryrun-requests-answer-notimplemented.md (DryRun answered 501 NotImplemented, not the state-check inversion in Cause 2), 20261104T142958Z-ec2-ten-ec2-refusal-messages-are-emulator-authored-prose.md and 20261029T175114Z-ec2-refusal-messages-are-emulator-authored-prose-instead.md (different operations), 20261103T021956Z-ec2-route-server-transit-gateway-and-resource-arn.md (create/validate paths, not these five).

Cause 1 of 6: EnableRouteServerPropagation's not-associated refusal message is emulator-authored.
AWS: EnableRouteServerPropagation on a route table whose VPC is not associated with the route server is refused with error code InvalidRouteServerId.NotAssociated and the message "Route Server <route-server-id> is not associated with a VPC." — fixed text "Route Server " ... " is not associated with a VPC." with the RouteServerId from the request as the only variable part. The route-table id and the VPC id are NOT part of AWS's message. Recorded behavior only for the message (the EC2 Smithy model declares no modeled errors for this operation: errors [] on EnableRouteServerPropagation).
Emulator: the error code is correct but the message is "The route-server ID 'rs-<id>' is not associated with the vpc ID 'vpc-<id>' of routeTable ID 'rtb-<id>'" — different fixed text, and it interpolates two ids AWS never mentions.
Why: a client that matches or surfaces AWS's refusal text (log scrapers, runbooks, error-message assertions) sees a message it cannot match, and a human reading it is told the refusal is about a route-table/VPC pairing rather than about the missing route-server↔VPC association.
Fix: the refusal message template in EnableRouteServerPropagation's validation (the operation's handler/validation in the ec2 service crate).
Traces: ec2.enable-route-server-propagation-without-association-blocked.

Cause 2 of 6: DisableRouteServerPropagation evaluates state before the DryRun short-circuit.
AWS: with DryRun=true, EC2 performs the permission check only and then short-circuits: an authorized caller is answered error code DryRunOperation with the fixed message "Request would have succeeded, but DryRun flag is set." It does not run the operation's state/existence validation first, so a DryRun call naming a propagation that is already gone still answers DryRunOperation. Smithy: DisableRouteServerPropagation input carries DryRun (Boolean); recorded behavior only for the DryRun precedence and message.
Emulator: the same DryRun=true call is answered IncorrectState — the state check runs ahead of the dry-run short-circuit, so a permission probe is reported as a state conflict.
Why: DryRun is the documented way to probe authorization without mutating anything. A client that uses it to pre-flight a disable (CLI --dry-run, IaC plan paths) reads IncorrectState and concludes it lacks the resource or is in the wrong state, when AWS would have told it the call is permitted.
Fix: order DryRun handling ahead of state validation in the DisableRouteServerPropagation handler (and its shared DryRun helper), so the authorized answer is DryRunOperation with AWS's fixed message.
Traces: ec2.op-contract-disable-route-server-propagation.

Cause 3 of 6: the IncorrectState message for a mutation on a deleted route server is emulator-authored.
AWS: a deleted route server still describes (State "deleted") rather than 404ing, and a mutation against it — here AssociateRouteServer — is refused with code IncorrectState and the message "Route Server <route-server-id> is in an invalid state." — fixed text "Route Server " ... " is in an invalid state.", with the RouteServerId from the request as the only variable part; the concrete state is NOT named. Recorded behavior only for the message.
Emulator: the code is correct but the message is "route-server '<rs-id>' is in the 'deleted' state." — different fixed text, different capitalisation/quoting, and it leaks the internal state value into the message.
Why: same as Cause 1 — a client matching AWS's wording cannot match it, and the message is not the one AWS's documentation and SDK retry/branch logic were written against.
Fix: the IncorrectState refusal message template on the route-server mutation paths (AssociateRouteServer and the sibling route-server mutations that share it).
Traces: ec2.route-server-lifecycle-full.

Cause 4 of 6: GetRouteServerRoutingDatabase echoes the PersistRoutes setting into AreRoutesPersisted.
AWS: Smithy: GetRouteServerRoutingDatabase output member AreRoutesPersisted (Boolean, wire areRoutesPersisted) reports whether routes are actually being persisted in the routing database — not the route server's configured PersistRoutes value. A route server created with PersistRoutes='enable' that has no BGP session and therefore no learned routes reports AreRoutesPersisted=false with an empty Routes list.
Emulator: the same read returns AreRoutesPersisted=true — a direct echo of the PersistRoutes request parameter, with no learned routes behind it.
Why: AreRoutesPersisted is how a client distinguishes "configured to persist" from "actually persisting learned routes". Echoing the setting makes the field useless: a client diagnosing a dead BGP session is told routes are persisted while the database is empty.
Fix: derive AreRoutesPersisted from the routing-database state (learned/persisted routes) in the GetRouteServerRoutingDatabase handler, rather than copying the route server's PersistRoutes configuration.
Traces: ec2.route-server-routing-database-reflects-learned-routes.

Cause 5 of 6: DisassociateRouteTable does not recognise the main association, so it answers NotFound instead of refusing the disassociate.
AWS: the implicit main route-table association is a real, addressable association (DescribeRouteTables reports it with Main=true and a RouteTableAssociationId), and it cannot be disassociated: DisassociateRouteTable on that association id is refused with code InvalidParameterValue and the message "cannot disassociate the main route table association <rtbassoc-id>" — fixed text "cannot disassociate the main route table association " plus the AssociationId from the request. Smithy: DisassociateRouteTable.AssociationId targets RouteTableAssociationId (required); the model declares no modeled errors (errors []), so the code/message are recorded behavior.
Emulator: the same call is refused InvalidAssociationID.NotFound — the main association id the emulator itself just reported in DescribeRouteTables is not resolvable by DisassociateRouteTable, so the refusal says the association does not exist instead of saying the main association may not be disassociated.
Why: two different wrong signals in one answer. A client is told an id it just read back does not exist (so it retries or reconciles against a phantom deletion), and the real invariant — the main association is immutable — is never communicated. Error-code branching on InvalidParameterValue vs InvalidAssociationID.NotFound takes the wrong branch.
Fix: register the implicit main association in the route-table association lookup and refuse it in the DisassociateRouteTable handler with InvalidParameterValue and AWS's message template (the handler's validation plus route-table association state).
Traces: ec2.route-table-local-route-and-main-association-immutable.

Cause 6 of 6: SendDiagnosticInterrupt's malformed-instance-id message is emulator-authored.
AWS: SendDiagnosticInterrupt rejects an instance id it does not accept as well-formed with code InvalidInstanceID.Malformed and the message "The instance ID '<instance-id>' is malformed" — fixed text "The instance ID '" ... "' is malformed", with the InstanceId from the request as the only variable part. Smithy: SendDiagnosticInterrupt.InstanceId targets InstanceId (required); errors [] — the code/message are recorded behavior.
Emulator: the code is correct but the message is `Invalid id: "<instance-id>"` — different fixed text, and the id is double-quoted rather than single-quoted inside AWS's sentence.
Why: a client or operator matching AWS's malformed-id wording (and the common practice of parsing the quoted id out of it) cannot read the emulator's message; the generic "Invalid id" text also hides which request member was rejected.
Fix: the malformed-id refusal message in SendDiagnosticInterrupt's input validation (and the shared id-validation helper it uses, so the EC2 template is emitted rather than a generic one).
Traces: ec2.op-contract-send-diagnostic-interrupt.

## Evidence
Cause 1 — ec2.enable-route-server-propagation-without-association-blocked (mismatch):
  expected code InvalidRouteServerId.NotAssociated (matched) and message "Route Server <rs-id> is not associated with a VPC."
  got message: "The route-server ID 'rs-00000000000000045' is not associated with the vpc ID 'vpc-00000000000000026' of routeTable ID 'rtb-00000000000000044'"

Cause 2 — ec2.op-contract-disable-route-server-propagation (mismatch):
  call: disable_route_server_propagation(RouteServerId=..., RouteTableId=..., DryRun=True)
  expected: DryRunOperation / "Request would have succeeded, but DryRun flag is set."
  got: AssertionError: expected DryRunOperation, got IncorrectState

Cause 3 — ec2.route-server-lifecycle-full (mismatch):
  call: associate_route_server on a route server in state 'deleted'; code IncorrectState matched
  expected message: "Route Server <rs-id> is in an invalid state."
  got: "route-server 'rs-00000000000000047' is in the 'deleted' state."

Cause 4 — ec2.route-server-routing-database-reflects-learned-routes (mismatch):
  route server created with PersistRoutes='enable', no BGP session, no learned routes
  expected: GetRouteServerRoutingDatabase AreRoutesPersisted=False
  got: True (echo of the PersistRoutes setting)

Cause 5 — ec2.route-table-local-route-and-main-association-immutable (mismatch):
  call: disassociate_route_table(AssociationId=<main association id read back from DescribeRouteTables>)
  expected: InvalidParameterValue / "cannot disassociate the main route table association <rtbassoc-id>"
  got: AssertionError: expected InvalidParameterValue disassociating main association rtbassoc-0000000000000002b, got InvalidAssociationID.NotFound

Cause 6 — ec2.op-contract-send-diagnostic-interrupt (mismatch):
  call: send_diagnostic_interrupt(InstanceId=<well-formed id EC2 rejects as malformed>); code InvalidInstanceID.Malformed matched
  expected message: "The instance ID '<id>' is malformed"
  got: Invalid id: "i-00000000000000002"

## Steps to Reproduce
Replay the behavioral suite and re-run exactly these checks:
1. ec2.enable-route-server-propagation-without-association-blocked — create a route table and a route server, wait for 'available', call EnableRouteServerPropagation without associating the VPC; inspect the InvalidRouteServerId.NotAssociated message.
2. ec2.op-contract-disable-route-server-propagation — associate the VPC, enable then disable propagation, wait until the propagation is gone, then call DisableRouteServerPropagation again with DryRun=True; expect DryRunOperation.
3. ec2.route-server-lifecycle-full — delete a route server, wait until it describes as 'deleted', then call AssociateRouteServer on it; inspect the IncorrectState message.
4. ec2.route-server-routing-database-reflects-learned-routes — create a route server with PersistRoutes='enable' and no BGP peer session, then call GetRouteServerRoutingDatabase; expect AreRoutesPersisted=false and an empty Routes list.
5. ec2.route-table-local-route-and-main-association-immutable — create a VPC, read its main route table association id from DescribeRouteTables (Main=true), then call DisassociateRouteTable with that association id; expect InvalidParameterValue with the main-association message.
6. ec2.op-contract-send-diagnostic-interrupt — call SendDiagnosticInterrupt with a 17-hex-character instance id; expect InvalidInstanceID.Malformed with AWS's "The instance ID '<id>' is malformed" message.
