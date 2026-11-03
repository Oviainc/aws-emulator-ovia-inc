Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda-core.network-connector-create-nonexistent-subnet-rejected
Issue-Shape-Id: com.amazonaws.lambdacore#LambdaCoreApiService
Issue-Key: lambda-core.network-connector-create-nonexistent-subnet-rejected|_poll_until#4|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s (last={last!r})")
Issue-Commit: dd64cbf0b038e7955d699468def5671676941ec2

# lambda-core: a created network connector is never observed ACTIVE by a polling client — PENDING→ACTIVE is gated on a wall-clock span, so connector-dependent workflows never start

> _Note: this issue was identified against the code base at commit `dd64cbf0b038e7955d699468def5671676941ec2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetNetworkConnector

## Description
One net-new root cause on the lambda-core network-connector lifecycle (com.amazonaws.lambdacore#LambdaCoreApiService).

Cause 1 of 1 — the asynchronous PENDING→ACTIVE provisioning never materialises for a client that polls GetNetworkConnector straight after CreateNetworkConnector, so the connector stays PENDING for the whole life of the workflow.

AWS: CreateNetworkConnector (POST /2026-04-04/network-connectors, success code 202) documents the lifecycle explicitly — "This operation is asynchronous. The network connector starts in PENDING state while ENIs are provisioned in your VPC (provisioning typically takes up to 10 minutes). Use GetNetworkConnector to poll the connector state until it reaches ACTIVE." GetNetworkConnector (GET /2026-04-04/network-connectors/{Identifier}, code 200, readonly) reports State from NetworkConnectorState (ACTIVE | DELETE_FAILED | DELETING | FAILED | INACTIVE | PENDING) together with StateReason/StateReasonCode. The contract is therefore convergence: PENDING is a transient state that a poll loop is guaranteed to see leave, and a connector only becomes attachable to compute resources once it reports ACTIVE. The State value, not a message, is the requirement here.

Emulator: CreateNetworkConnector answers State=PENDING / StateReason="Initial creation" (correct), and GetNetworkConnector, polled immediately afterwards, answers HTTP 200 with State=PENDING again and never anything else inside the window the client has. In the replayed check the wait for State in (ACTIVE, FAILED) expired with the connector still PENDING (the last GetNetworkConnector response carried State='PENDING', StateReason='Initial creation', Version=1), so the positive-control connector never became usable and the subject the check exists to exercise — CreateNetworkConnector's refusal of a non-existent subnet with InvalidParameterValueException and the message "Error occurred while DescribeSubnets. EC2 Error Code: InvalidSubnetID.NotFound. EC2 Error Message: The subnet ID '<subnetId>' does not exist" (fixed text; only <subnetId> comes from the request) — was never reached at all.

Why a client gets this wrong: every documented use of a connector is gated on "poll until ACTIVE". Against the emulator that loop does not terminate within a realistic client budget, so a client that follows AWS's own instructions hangs or gives up, and nothing can be attached to the connector. The connector-dependent half of the service (attaching to MicroVMs/WebFunctions) is unreachable, and any assertion downstream of "wait for ACTIVE" is never evaluated.

Emulator-side mechanism, from this checkout: the provisioning change is stored as a PendingTransition due at now + LIFECYCLE_TRANSITION, a fixed 500 ms wall-clock span, and is materialised only lazily by whichever later call reads the record (settle_all). The first read after a create is therefore always PENDING, and ACTIVE appears only on a subsequent poll — a poll a short-budget workflow does not get, because clients poll at multi-second intervals. The emulator's own comment says the span is "deliberately NOT zero so a caller acting straight after the call still observes the transient state"; the transient state is already observable in CreateNetworkConnector's own 202/PENDING answer, so the wall-clock gate buys nothing and costs convergence.

Fix: in lambda-core's connector state, the provisioning transition must settle such that the first GetNetworkConnector following a successful create already reports ACTIVE — i.e. make the PENDING→ACTIVE settlement due at creation time (or drive it from a read-count/step rather than elapsed wall-clock), leaving PENDING observable on the create response itself. The same applies to the update (InProgress→Successful) and deletion (DELETING→removal) transitions that share the constant. This is a state/lifecycle defect, not a serializer or validation one.

Relation to what is already filed: the open issue 20261030T090527Z-lambda-core-network-connector-refusals-diverge-from-aws.md covers the REFUSAL arms of these operations (wrong status/error class/messages, including the DescribeSubnets message above) and carries no key for the PENDING→ACTIVE lifecycle, so this divergence is not covered by it.

Traces: lambda-core.network-connector-create-nonexistent-subnet-rejected. Code citations for the fixer: services/service-com-amazonaws-lambdacore-lambdacoreapiservice/src/state.rs line 153 (LIFECYCLE_TRANSITION = 500 ms) and lines 155-170 (PendingChange/PendingTransition); services/service-com-amazonaws-lambdacore-lambdacoreapiservice/src/ops/create_network_connector.rs lines 344-356 (the pending transition written on create).

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout: 16/29 passed, bootstrap_signal false (the emulator answered every call in this check).

[mismatch] lambda-core.network-connector-create-nonexistent-subnet-rejected:
  AssertionError: condition not met within 1.0s (last={'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'Arn': 'arn:aws:lambda:us-west-2:000000000000:network-connector:nc-0000000000000001', 'Name': 'sim2real-lnc-<id>-ctl0', 'Id': 'nc-0000000000000001', 'Version': 1, 'Configuration': {'VpcEgressConfiguration': {...}}, 'OperatorRole': 'arn:aws:iam::000000000000:role/sim2real-lnc-<id>', 'State': 'PENDING', 'StateReason': 'Initial creation', 'LastModified': ...})
  raised from the wait `lambda resp: resp['State'] in ('ACTIVE', 'FAILED')` over lc.get_network_connector(Identifier=<id>).

Expected (AWS): the poll reports State='ACTIVE' (the operation's own documentation instructs the client to "poll the connector state until it reaches ACTIVE").
Actual (emulator): every GetNetworkConnector inside the client's budget reports State='PENDING'; no call was refused and no error was returned, so this is a state-convergence divergence rather than a wire-shape one.
(The ARN, account id, generated connector id, name suffix and timestamp above are per-run values and are not part of the requirement; the diverging datum is the State member.)

## Steps to Reproduce
1. git_clone, then run_behavioral_suite() and look at check lambda-core.network-connector-create-nonexistent-subnet-rejected.
2. Or by hand against the running emulator: create a VPC, a subnet and a security group in EC2 and an IAM role trusted by lambda.amazonaws.com.
3. POST /2026-04-04/network-connectors with Name=<unique>, Configuration={"VpcEgressConfiguration":{"SubnetIds":[<subnetId>],"SecurityGroupIds":[<sgId>],"NetworkProtocol":"IPv4","AssociatedComputeResourceTypes":["MicroVm"]}}, OperatorRole=<roleArn>. Observe 202 with State=PENDING (correct).
4. Immediately GET /2026-04-04/network-connectors/<Id> once (as a poll loop does on its first iteration): observe State=PENDING. AWS's contract is that this poll converges to ACTIVE; the emulator only flips the state on a read taken more than 500 ms of wall clock after the create, so a client polling on its own schedule inside a short budget never observes ACTIVE and anything gated on the connector being ACTIVE is unreachable.
