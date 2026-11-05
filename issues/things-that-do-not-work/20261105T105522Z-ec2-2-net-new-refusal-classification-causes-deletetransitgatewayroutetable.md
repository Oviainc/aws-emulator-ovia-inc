Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.delete-transit-gateway-route-table-active-association-rejected ec2.op-contract-describeInstances
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.delete-transit-gateway-route-table-active-association-rejected|run#12|AssertionError|assert error["Code"] == "IncorrectState", error
Issue-Key: ec2.op-contract-describeInstances|run#15|AssertionError|assert code == "InvalidInstanceID.Malformed", (
Issue-Commit: 8524570065cf0f73f283419a239f018e52a5ab39

# ec2: 2 net-new refusal-classification causes — DeleteTransitGatewayRouteTable answers DependencyViolation instead of IncorrectState, and DescribeInstances answers InvalidInstanceID.NotFound for a well-formed never-issued instance id

> _Note: this issue was identified against the code base at commit `8524570065cf0f73f283419a239f018e52a5ab39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteTransitGatewayRouteTable

## Description
Two net-new root causes, both in EC2's refusal classification. EC2 is an ec2Query/awsQuery service and the Smithy model binds NO per-operation error shapes (smithy_query operation=DeleteTransitGatewayRouteTable,DescribeInstances -> "errors": []), so for both causes the status/code/message triple is recorded behavior only; the quoted messages name which part is fixed text and which part comes from the request or from emulator state.

Cause 1 of 2: DeleteTransitGatewayRouteTable on a route table that still has an associated attachment is refused with the wrong error code and an emulator-authored message. AWS: recorded behavior only (DeleteTransitGatewayRouteTable models no errors). The recording shows HTTP 400 with code `IncorrectState` and message `<transit-gateway-route-table-id> has associated attachments` — the fixed text is the trailing ` has associated attachments`, the leading value is the TransitGatewayRouteTableId taken from the request. Emulator: HTTP 400 with code `DependencyViolation` and the message "The transit gateway route table 'tgw-rtb-00000000000000041' is associated with the transit gateway attachments tgw-attach-00000000000000040, which must be disassociated from it before it can be deleted" — a different error code and emulator-authored prose that enumerates the attachment ids (ids in that text are per-run state values, not part of any template). Why: the error code is the contract SDKs, waiters and retry/backoff policies branch on. `IncorrectState` tells a client the resource is in a transient state that clearing the association will resolve, so a client retries after DisassociateTransitGatewayRouteTable; `DependencyViolation` is EC2's code for a delete blocked by a different dependent resource, so a client written against AWS either takes the wrong remediation branch or never matches the refusal at all, and a client matching on the message text sees nothing it expects. Fix: the state precondition check in DeleteTransitGatewayRouteTable's handler/validation must emit the IncorrectState code and AWS's fixed message for an associated route table (the rejected delete must also leave the table in `available`, which the same check asserts next). Failing check: ec2.delete-transit-gateway-route-table-active-association-rejected.

Cause 2 of 2: DescribeInstances misclassifies a syntactically well-formed but never-issued instance id. AWS: recorded behavior only (DescribeInstances models no errors). EC2 validates a new-format instance id's internal checksum before it performs the existence lookup, so a randomly generated well-formed `i-<hex>` of the right width is refused HTTP 400 with code `InvalidInstanceID.Malformed` and message `Invalid id: "<instance-id>"` — the fixed text is `Invalid id: "..."`, the value inside the quotes is the id taken from the request. Emulator: refuses with code `InvalidInstanceID.NotFound` instead. Why: a client that treats NotFound as "the instance was terminated / has not propagated yet" (exactly what this check's own poll loop does) will retry forever against an id AWS says can never exist, instead of failing fast on an unparseable id; SDK error classes and the retry policies keyed on these two codes diverge. Note this is the same defect family as the open issue '20261029T200817Z-ec2-id-validation-verdict-is-inverted-malformed-ids.md' (and '20261029T175114Z-ec2-client-error-classification-wrong-error-codes-for.md'), but neither carries DescribeInstances or this check's replay key, so neither covers it. Fix: the id-screening step in DescribeInstances' validation must test instance-id syntax (including the checksum) and emit InvalidInstanceID.Malformed with the `Invalid id: "<id>"` message before any lookup, and emit InvalidInstanceID.NotFound only for a syntactically valid id that fails lookup. Failing check: ec2.op-contract-describeInstances.

Traces (examples): ec2.delete-transit-gateway-route-table-active-association-rejected (Cause 1), ec2.op-contract-describeInstances (Cause 2). Handlers live under services/service-com-amazonaws-ec2-amazonec2/src/ops/ (delete_transit_gateway_route_table.rs, describe_instances.rs).

## Evidence
Cause 1 — ec2.delete-transit-gateway-route-table-active-association-rejected [mismatch]:
replay detail, verbatim: `AssertionError: {'Code': 'DependencyViolation', 'Message': "The transit gateway route table 'tgw-rtb-00000000000000041' is associated with the transit gateway attachments tgw-attach-00000000000000040, which must be disassociated from it before it can be deleted"}` at `assert error["Code"] == "IncorrectState"`.
Expected (recorded AWS): code `IncorrectState`, message `<tgw-rtb-id> has associated attachments`, HTTP 400, and the route table still `available` afterwards.
Actual (emulator): code `DependencyViolation` with emulator-authored prose naming the attachment ids.

Cause 2 — ec2.op-contract-describeInstances [mismatch]:
replay detail, verbatim: `AssertionError: expected a syntactically-valid but nonexistent instance id to be rejected as Malformed, got 'InvalidInstanceID.NotFound'`.
Expected (recorded AWS): code `InvalidInstanceID.Malformed`, message `Invalid id: "<instance-id>"` (fixed text `Invalid id: "…"`, value from the request).
Actual (emulator): code `InvalidInstanceID.NotFound`.

Smithy (both): services/service-com-amazonaws-ec2-amazonec2/smithy-model — DeleteTransitGatewayRouteTable and DescribeInstances both report `errors: []`, so code/message are recorded behavior only.

## Steps to Reproduce
1. git_clone the repo and run the behavioral suite (run_behavioral_suite) so the emulator is built and booted.
2. Cause 1: replay check ec2.delete-transit-gateway-route-table-active-association-rejected — create a transit gateway, a VPC attachment, wait for `available`, create a transit gateway route table, AssociateTransitGatewayRouteTable and wait for state `associated`, then call DeleteTransitGatewayRouteTable on that table. Observe the 400's Error.Code/Error.Message: the emulator answers DependencyViolation with emulator prose where AWS answers IncorrectState / "<tgw-rtb-id> has associated attachments".
3. Cause 2: replay check ec2.op-contract-describeInstances — launch a t3.micro, then call DescribeInstances with a randomly generated well-formed instance id (`i-` + the same number of hex chars as the launched id) that was never issued. Observe the 400's Error.Code: the emulator answers InvalidInstanceID.NotFound where AWS answers InvalidInstanceID.Malformed with message `Invalid id: "<that id>"`.
