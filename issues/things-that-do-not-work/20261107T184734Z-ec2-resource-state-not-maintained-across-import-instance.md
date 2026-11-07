Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-import-snapshot-permission-error-and-task-inventory
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-import-snapshot-permission-error-and-task-inventory ec2-instance-attribute-reset-roundtrip ec2-madl-traj-vpc-subnet-discovery-filters ec2-pst-traj-route-filters-match-per-table-not-per-route ec2-sshp-traj-keyless-public-instance-eic-push-and-eip-swap ec2-sshr-traj-instances-in-the-open-group-split-on-public-ip ec2-tgw-lifecycle-scoped-describes-and-propagation-error
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|ImportSnapshot|4xx->2xx|$.Response
Issue-Key: ec2|ImportSnapshot|4xx->2xx|status
Issue-Key: ec2|DescribeImportSnapshotTasks|2xx->2xx|$.DescribeImportSnapshotTasksResponse.importSnapshotTaskSet
Issue-Key: ec2|DescribeInstanceAttribute|2xx->2xx|$.DescribeInstanceAttributeResponse.sourceDestCheck.value
Issue-Key: ec2|DescribeSubnets|2xx->2xx|$.DescribeSubnetsResponse.subnetSet.item[subnetId="subnet-0000000000000003d"].dhcpOptionsId
Issue-Key: ec2|DescribeSubnets|2xx->2xx|$.DescribeSubnetsResponse.subnetSet.item[subnetId="subnet-0000000000000003f"].dhcpOptionsId
Issue-Key: ec2|DescribeRouteTables|2xx->2xx|$.DescribeRouteTablesResponse.routeTableSet.item[routeTableId="rtb-00000000000000050"].routeSet.item[*].state
Issue-Key: ec2|RunInstances|2xx->5xx|status
Issue-Key: ec2|DeleteTransitGateway|2xx->2xx|$.DeleteTransitGatewayResponse.transitGateway.options.encryptionSupport
Issue-Key: ec2|DescribeTransitGateways|2xx->4xx|$
Issue-Key: ec2|DescribeTransitGateways|2xx->4xx|$.DescribeTransitGatewaysResponse.transitGatewaySet.item.state
Issue-Key: ec2|DescribeTransitGateways|2xx->4xx|status
Issue-Related: 20261107T071147Z-ec2-describe-projections-omit-launch-template-version-operator.md
Issue-Commit: ab92470025dd8eeb501814706ada4aa2b617e26c

# EC2 resource state not maintained across import, instance, subnet, route-table and transit-gateway reads: 8 net-new causes

> _Note: this issue was identified against the code base at commit `ab92470025dd8eeb501814706ada4aa2b617e26c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ImportSnapshot

## Description
Eight net-new divergences in service ec2 where the emulator's stored resource state (or the projection of it into a read) does not match AWS: a write is accepted that AWS refuses, state transitions are not applied, modeled response members are omitted, a delete removes a resource that AWS keeps visible in a terminal state, and one RunInstances request shape falls into a NotImplemented branch. None of these clusters is blocked by a scaffold stub and none is owned by an open issue. The ec2 Smithy model declares no operation-level errors for these operations, so every status/code/message requirement is taken from the recording and labelled "recorded behavior only".

Cause 1 of 8 - ImportSnapshot is accepted without the vmimport service role.
AWS: recorded behavior only (Smithy: ImportSnapshotRequest{DiskContainer, RoleName, ...}; errors: []). When the service role named by RoleName (default "vmimport") does not exist or lacks permissions, the call is refused HTTP 400 with Response.Errors.Error.Code "InvalidParameter" and Message "The service role <role-name> provided does not exist or does not have sufficient permissions." - fixed text with the role name from the request.
Emulator: HTTP 200 and an ImportSnapshotResult, i.e. it creates an import task with no role check at all.
Why: a client exercising the IAM-prerequisite path sees success and never surfaces the missing role; the failure shows up much later, or never.
Fix: the ImportSnapshot handler's validation (check the import service role before creating the task) and its error mapping.
Traces: ec2-import-snapshot-permission-error-and-task-inventory#task#1.

Cause 2 of 8 - DescribeImportSnapshotTasks reports the task that should never have existed.
AWS: recorded behavior only. After the refusal in cause 1, DescribeImportSnapshotTasks returns an EMPTY importSnapshotTaskSet.
Emulator: returns importSnapshotTaskSet.item with the emulator's import task (importTaskId, snapshotTaskDetail with status "active"/statusMessage "pending").
Why: the inventory read shows work in flight that AWS never started, so a poller waits for a task that cannot complete.
Fix: follows from cause 1 (do not create the task), plus the DescribeImportSnapshotTasks projection of the import-task state.
Traces: ec2-import-snapshot-permission-error-and-task-inventory#task#2 (cluster also covers #task#3).

Cause 3 of 8 - DescribeInstanceAttribute returns the stale sourceDestCheck value.
AWS: recorded behavior only. After the attribute is set, DescribeInstanceAttribute for sourceDestCheck returns value "false".
Emulator: returns value "true" - the attribute write is not reflected in the instance's stored attributes.
Why: a client that disables source/dest checking (NAT instance setup) reads back a value that says the opposite, and any guard on that read loops or fails.
Fix: instance attribute state plus the DescribeInstanceAttribute projection. Note: in this fixture the preceding write is ModifyInstanceAttribute, which is still a scaffold stub with its own filed issue; this read-side divergence is reported separately because the read must honour the stored attribute once the write lands.
Traces: ec2-instance-attribute-reset-roundtrip#task#3.

Cause 4 of 8 - DescribeSubnets omits the DHCP-options members.
AWS: recorded behavior only (the Smithy Subnet shape in this checkout does not declare these members, so the recording is the only source). Each subnetSet.item carries dhcpOptionsId - the DHCP options set id inherited from the subnet's VPC - and dhcpOptionsSource with value "vpc" when it is inherited.
Emulator: both members are absent from every subnetSet.item.
Why: a client resolving DNS/DHCP configuration per subnet cannot tell which options set applies or where it came from.
Fix: the DescribeSubnets serializer (and the subnet state's link to the VPC's DHCP options set).
Traces: ec2-madl-traj-vpc-subnet-discovery-filters#task#8.

Cause 5 of 8 - a route whose target is gone stays "active" instead of "blackhole".
AWS: Smithy: Route.State -> RouteState, documented as "The state of the route. The blackhole state indicates that the route's target isn't available (for example, the specified gateway isn't attached to the VPC, or the specified NAT instance has been terminated)." The recording shows the affected route's state as "blackhole".
Emulator: DescribeRouteTables reports that route with state "active".
Why: a client that detects broken routing by looking for blackhole routes never sees the broken route, and a route-filter query on state returns the wrong set.
Fix: route-table state - recompute each route's state when its target (gateway, interface, instance, peering) is deleted or detached - and the DescribeRouteTables projection.
Traces: ec2-pst-traj-route-filters-match-per-table-not-per-route#task#11 (cluster also covers #task#12 and #task#13).

Cause 6 of 8 - RunInstances answers 501 NotImplemented for a launch shape the corpus uses.
AWS: recorded behavior only. The launch succeeds with HTTP 200 and a RunInstancesResponse.
Emulator: HTTP 501 for this request shape, while other RunInstances steps in the corpus pass - so the operation exists but one request path (keyless launch into a public subnet in these fixtures) reaches a NotImplemented branch.
Why: the fixture's whole trajectory is blocked at setup: no instance, so every dependent step is unreachable.
Fix: the RunInstances handler - implement the member combination that currently falls through to the NotImplemented branch.
Traces: ec2-sshp-traj-keyless-public-instance-eic-push-and-eip-swap#setup#14 (cluster also covers ec2-sshr-traj-instances-in-the-open-group-split-on-public-ip#setup#7).

Cause 7 of 8 - DeleteTransitGateway omits transitGateway.options.encryptionSupport.
AWS: Smithy: TransitGatewayOptions.EncryptionSupport -> encryptionSupport, an EncryptionSupport{encryptionState, stateMessage}. The recorded DeleteTransitGatewayResponse carries options.encryptionSupport with encryptionState "disabled" for a transit gateway created without VPC encryption control.
Emulator: the member is absent from the response.
Why: a client reading the transit gateway's encryption posture from the delete (or any describe) response gets nothing where AWS always reports a state.
Fix: the transit-gateway state (default the encryptionSupport sub-structure at create) and the DeleteTransitGateway/DescribeTransitGateways serializers.
Traces: ec2-tgw-lifecycle-scoped-describes-and-propagation-error#teardown#14.

Cause 8 of 8 - a deleted transit gateway disappears instead of reaching state "deleted".
AWS: recorded behavior only. After DeleteTransitGateway, DescribeTransitGateways still answers HTTP 200 and reports the transit gateway with state transitioning to "deleted".
Emulator: HTTP 400 with Code "InvalidTransitGatewayID.NotFound" and Message "Transit Gateway <tgw-id> was deleted or does not exist." - the record is removed from state at delete time, so the replay's wait for state "deleted" never succeeds (poll capped at 180s after 19 attempts).
Why: every SDK waiter for a deleted transit gateway fails, because it expects a terminal state rather than a not-found refusal.
Fix: the transit-gateway state - keep deleted gateways readable in state "deleted" (as the lifecycle recording does) rather than dropping them - plus the DescribeTransitGateways lookup path.
Traces: ec2-tgw-lifecycle-scoped-describes-and-propagation-error#teardown#15.

## Evidence
Per-cause expected vs actual, from the replay of trace set simparity-wire-aws-v1:
1) ec2-import-snapshot-permission-error-and-task-inventory#task#1 ImportSnapshot: expected 400 with Errors.Error.Code 'InvalidParameter' and Message 'The service role vmimport provided does not exist or does not have sufficient permissions.'; got 200 with no Response error element.
2) #task#2 DescribeImportSnapshotTasks: expected importSnapshotTaskSet '' (empty); got {'item': {'importTaskId': 'import-snap-...', 'snapshotTaskDetail': {'status': 'active', 'statusMessage': 'pending', ...}}}.
3) ec2-instance-attribute-reset-roundtrip#task#3 DescribeInstanceAttribute: $.DescribeInstanceAttributeResponse.sourceDestCheck.value expected 'false', got 'true'.
4) ec2-madl-traj-vpc-subnet-discovery-filters#task#8 DescribeSubnets: for each subnetSet.item, dhcpOptionsId expected (a dopt-* id, 'dopt-03d13527b92584567' in this run) got None, and dhcpOptionsSource expected 'vpc' got None.
5) ec2-pst-traj-route-filters-match-per-table-not-per-route#task#11 DescribeRouteTables: routeSet.item[2].state expected 'blackhole', got 'active'.
6) ec2-sshp-traj-keyless-public-instance-eic-push-and-eip-swap#setup#14 RunInstances: expected 200, got 501.
7) ec2-tgw-lifecycle-scoped-describes-and-propagation-error#teardown#14 DeleteTransitGateway: $.DeleteTransitGatewayResponse.transitGateway.options.encryptionSupport expected {'encryptionState': 'disabled'}, got None.
8) #teardown#15 DescribeTransitGateways: expected 200 with transitGatewaySet.item.state reaching 'deleted'; got 400 Code 'InvalidTransitGatewayID.NotFound', Message 'Transit Gateway tgw-<id> was deleted or does not exist.' (poll capped at 180s).
(The ids and generated values above are this run's; the requirements are the statuses, codes, message templates and member presence.)

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire fixture corpus (trace set simparity-wire-aws-v1).
2. Replay fixture ec2-import-snapshot-permission-error-and-task-inventory, step task#1 (the representative), then the seven fixture#step bindings in trace_ids.
3. Compare, per step, the HTTP status, the error code/message where one is expected, and the presence and value of the response members named above.
