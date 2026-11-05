Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-route-server-delete-lifecycle
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-route-server-delete-lifecycle ec2-fast-snapshot-restore-and-public-access-lockdown ec2-managed-prefix-list-lifecycle ec2-snapshot-filter-and-pagination-sweep
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeRouteServers|2xx->4xx|$
Issue-Key: ec2|DescribeRouteServers|2xx->4xx|$.DescribeRouteServersResponse
Issue-Key: ec2|DescribeRouteServers|2xx->4xx|$.DescribeRouteServersResponse.routeServerSet.item.state
Issue-Key: ec2|DescribeRouteServers|2xx->4xx|status
Issue-Key: ec2|DisassociateRouteServer|2xx->2xx|$.DisassociateRouteServerResponse.routeServerAssociation.state
Issue-Key: ec2|DisableFastSnapshotRestores|2xx->2xx|$.DisableFastSnapshotRestoresResponse.successful
Issue-Key: ec2|DisableFastSnapshotRestores|2xx->2xx|$.DisableFastSnapshotRestoresResponse.unsuccessful
Issue-Key: ec2|ModifyManagedPrefixList|2xx->2xx|$.ModifyManagedPrefixListResponse.prefixList.version
Issue-Key: ec2|DescribeSnapshots|2xx->2xx|$.DescribeSnapshotsResponse.snapshotSet.item[*]
Issue-Commit: 68ec0c9cae77cfbc133f0186a4c923306bef5402

# EC2 post-operation state bookkeeping returns terminal or stale results instead of AWS's

> _Note: this issue was identified against the code base at commit `68ec0c9cae77cfbc133f0186a4c923306bef5402`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeRouteServers

## Description
Shared fault: after a mutating call the emulator records the wrong post-operation state — it jumps straight to the terminal value, drops the resource instead of marking it, reports success where AWS reports a per-item failure, bumps a version that AWS leaves alone, or serves the wrong rows for a filtered read. Five causes, all ec2.

Cause 1 of 5: DescribeRouteServers after DeleteRouteServer loses the resource instead of showing it deleted.
AWS: Smithy: DescribeRouteServers takes RouteServerIds and returns RouteServers (xmlName routeServerSet); the recording shows the deleted route server still described, with state 'deleted' and its tagSet intact, which is what the fixture polls for.
Emulator: 400 with Response.Errors.Error Code 'InvalidRouteServerId.NotFound', Message "The route-server ID '<id>' does not exist" — the record is removed at delete time, so the poll can never observe the terminal state and caps out.
Why: every delete-then-wait client (CloudFormation-style waiters included) needs to read the deleted state once; a not-found response is indistinguishable from an id it never created.
Fix: the DeleteRouteServer/DescribeRouteServers state handling — mark the route server deleted and keep serving it rather than evicting it.
Traces: ec2-route-server-delete-lifecycle#task#7.

Cause 2 of 5: DisassociateRouteServer reports the terminal association state immediately.
AWS: Smithy: RouteServerAssociation.State targets the RouteServerAssociationState enum; the recorded DisassociateRouteServer response returns state 'disassociating'.
Emulator: returns state 'disassociated'.
Why: the transitional value is the signal to start polling; a caller handed the terminal value skips its wait and races the rest of the teardown.
Fix: the DisassociateRouteServer handler's association state transition.
Traces: ec2-route-server-delete-lifecycle#task#5.

Cause 3 of 5: DisableFastSnapshotRestores reports success for a snapshot that has no fast snapshot restore enabled.
AWS: Smithy: DisableFastSnapshotRestores returns Successful (xmlName successful) and Unsuccessful (xmlName unsuccessful, DisableFastSnapshotRestoreErrorSet of DisableFastSnapshotRestoreErrorItem {snapshotId, fastSnapshotRestoreStateErrorSet}). The recording puts the snapshot under unsuccessful with an empty successful, and the per-AZ error is {code: 'InvalidParameterValue', message: 'Fast Restore is not enabled for this snapshot'} — that message is fixed text; availabilityZone comes from the request.
Emulator: puts the snapshot under successful with state 'disabled' and leaves unsuccessful empty.
Why: the caller is told it turned off a feature that was never on, so it never surfaces the misconfiguration and its cost/compliance reporting is wrong.
Fix: the DisableFastSnapshotRestores handler — check enablement per snapshot/AZ in state and build the Unsuccessful item.
Traces: ec2-fast-snapshot-restore-and-public-access-lockdown#task#4.

Cause 4 of 5: ModifyManagedPrefixList bumps the version when no entries changed.
AWS: Smithy: ModifyManagedPrefixList returns PrefixList (ManagedPrefixList) whose Version member is a long; the recording keeps Version at its prior value when the modification does not change entries.
Emulator: increments Version on every modify call.
Why: Version is the optimistic-concurrency token passed back as CurrentVersion; an unexpected bump makes the client's next conditional modify fail and makes prefix-list references look stale.
Fix: the ModifyManagedPrefixList handler's versioning rule in prefix-list state — bump only on an entry change.
Traces: ec2-managed-prefix-list-lifecycle#task#3.

Cause 5 of 5: DescribeSnapshots serves the wrong rows for a filtered, paginated sweep and omits modeled members.
AWS: recorded behavior only for the paging order; the recorded page holds the snapshots matching the filter in AWS's order, and every item carries transferType 'standard' and fullSnapshotSizeInBytes alongside storageTier and completionTime.
Emulator: returns different snapshots at every index of the page (the first rows of the unfiltered set rather than the filtered page) and omits transferType and fullSnapshotSizeInBytes from each item.
Why: a client paging a filtered snapshot inventory both sees snapshots it did not ask for and cannot read the members it needs for tiering/size decisions; pages also overlap, so sweeps double-count.
Fix: the DescribeSnapshots handler's filter+NextToken paging and the snapshot item serializer.
Traces: ec2-snapshot-filter-and-pagination-sweep#task#29.

## Evidence
Cause 1 (ec2-route-server-delete-lifecycle#task#7): expected 200 with DescribeRouteServersResponse.routeServerSet.item.state 'deleted'; actual 400 {Code 'InvalidRouteServerId.NotFound', Message "The route-server ID 'rs-00000000000000031' does not exist"}; poll 'not in [deleted] after 13 attempt(s)', capped at 60s.
Cause 2 (ec2-route-server-delete-lifecycle#task#5): routeServerAssociation.state expected 'disassociating', got 'disassociated'.
Cause 3 (ec2-fast-snapshot-restore-and-public-access-lockdown#task#4): unsuccessful expected {'item': {'fastSnapshotRestoreStateErrorSet': {'item': {'availabilityZone': 'us-west-2a', 'error': {'code': 'InvalidParameterValue', 'message': 'Fast Restore is not enabled for this snapshot'}}}, 'snapshotId': 'snap-...'}} got ''; successful expected '' got {'item': {'snapshotId': 'snap-...', 'availabilityZone': 'us-west-2a', 'state': 'disabled'}}.
Cause 4 (ec2-managed-prefix-list-lifecycle#task#3): prefixList.version expected '1', got '2'.
Cause 5 (ec2-snapshot-filter-and-pagination-sweep#task#29): snapshotSet.item[0]..item[6] all differ — e.g. item[0] expected snapshotId 'snap-0000000000000002d'/volumeId 'vol-0000000000000002c' with transferType 'standard' and fullSnapshotSizeInBytes present, actual snapshotId 'snap-00000000000000027'/volumeId 'vol-00000000000000026' with neither member.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay ec2-route-server-delete-lifecycle step task#7 (representative): after the delete, DescribeRouteServers must answer 200 with state 'deleted'.
3. Replay the four other bindings in trace_ids: ec2-route-server-delete-lifecycle#task#5, ec2-fast-snapshot-restore-and-public-access-lockdown#task#4, ec2-managed-prefix-list-lifecycle#task#3, ec2-snapshot-filter-and-pagination-sweep#task#29.
4. All five steps must match the recorded bodies before closing.
