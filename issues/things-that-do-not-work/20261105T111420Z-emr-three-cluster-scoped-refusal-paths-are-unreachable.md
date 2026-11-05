Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.auto-termination-policy-invalid-idle-timeout-rejected emr.op-app-ui-ops-reject-unknown-targets emr.op-cluster-read-ops-reject-unknown-cluster
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr.auto-termination-policy-invalid-idle-timeout-rejected|run#6|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Key: emr.op-app-ui-ops-reject-unknown-targets|_assert_codes#12|AssertionError|assert not wrong, f"expected {expected} for every op, got mismatches: {wrong}"
Issue-Key: emr.op-cluster-read-ops-reject-unknown-cluster|run#10|AssertionError|assert code == "InvalidRequestException", (
Issue-Commit: 8524570065cf0f73f283419a239f018e52a5ab39

# emr: three cluster-scoped refusal paths are unreachable — PutAutoTerminationPolicy range validation, GetOnClusterAppUIPresignedURL and ListInstances unknown-cluster refusals all answer NotImplemented

> _Note: this issue was identified against the code base at commit `8524570065cf0f73f283419a239f018e52a5ab39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAutoTerminationPolicy, GetOnClusterAppUIPresignedURL, ListInstances

## Description
Three net-new root causes in the EMR (elasticmapreduce) control plane. All three are refusal paths: the emulator answers an `NotImplemented` error code for the operation instead of the modelled/recorded 400 refusal, so a client cannot distinguish an invalid argument from an unsupported API. Each cause below is a separate work item; three must be closed before this issue is submitted.

These three failing checks are NOT the scaffold-stub clusters the replay report already attributes: the report marks sibling checks for these same operations ALREADY FILED, but these three swallow the 501 into their own assertion and so carry replay keys no open issue holds. The existing stub issues (`20261029T175551Z-emr-putautoterminationpolicy-stub.md`, `20261105T093917Z-emr-getonclusterappuipresignedurl-stub.md`, `20261105T083215Z-ec2-emr-emr-listinstances-is-a-scaffold-stub-so-an.md`) ask only that the operations be stood up; none of them states the refusal code, message or state effect required below, and none carries these check ids.

=== Cause 1 of 3: PutAutoTerminationPolicy does not enforce the IdleTimeout bounds, and the refusal leaves no policy behind ===
AWS: Smithy (com.amazonaws.emr#ElasticMapReduce) PutAutoTerminationPolicy takes ClusterId (@required, @length 0..256) and AutoTerminationPolicy; the operation models no errors, so recorded behavior only for the refusal identity. EMR refuses an AutoTerminationPolicy.IdleTimeout outside the documented 60..604800-second window with HTTP 400, error code `ValidationException`, and the message `Auto-termination policy IdleTimeout must be within a minimum of 60 seconds, and a maximum of 604800 seconds.` Every word of that sentence is FIXED text — the two numbers are the documented bounds of the member, not values echoed from the request, and the offending IdleTimeout does not appear in the message. The refused call has no effect: a following GetAutoTerminationPolicy returns a response with no AutoTerminationPolicy.IdleTimeout. A valid IdleTimeout on the same cluster is accepted and GetAutoTerminationPolicy echoes it back unchanged.
Emulator: PutAutoTerminationPolicy answers the error code `NotImplemented` with the message `operation PutAutoTerminationPolicy is scaffolded but not implemented in this emulator`, for both the out-of-range and the valid IdleTimeout, so neither the refusal nor the accepted round-trip is reachable.
Why: a client that relies on EMR's 60-second floor to validate its own cost-safety configuration gets an unmodelled `NotImplemented` instead of `ValidationException`, so its error branch never runs and an out-of-range idle timeout looks like an unsupported API rather than a bad argument; and a client that sets a legitimate idle timeout cannot set one at all.
Fix: the PutAutoTerminationPolicy handler and its input validation, plus the cluster state that holds the auto-termination policy for GetAutoTerminationPolicy to read back.
Traces: emr.auto-termination-policy-invalid-idle-timeout-rejected

=== Cause 2 of 3: GetOnClusterAppUIPresignedURL does not refuse an unknown ClusterId ===
AWS: Smithy GetOnClusterAppUIPresignedURL takes ClusterId (@required, XmlStringMaxLen256) plus OnClusterAppUIType/ApplicationId/DryRun/ExecutionRoleArn, and models InvalidRequestException (400) and InternalServerError (500). Recorded behavior: a ClusterId that was never valid is refused with HTTP 400, error code `ValidationException`, and the message `Cluster id '<ClusterId>' is not valid.` — the fixed text is `Cluster id '` … `' is not valid.` and the quoted value is the ClusterId taken verbatim from the request. The same call against a REAL cluster that is merely not yet runnable (DryRun=true) must NOT answer ValidationException: a wrong-state cluster is refused as InvalidRequestException, so the operation has to tell an unresolvable id apart from a resolvable one in the wrong state.
Emulator: the operation answers error code `NotImplemented` for every input, so the unknown-cluster arm reports `NotImplemented` where `ValidationException` is required, and the real-cluster positive control proves nothing.
Why: a console or client that probes app-UI availability cannot tell a typo'd/expired cluster id from a cluster whose Spark History Server is not up yet; it sees one unmodelled code for both and cannot retry the second case.
Fix: the GetOnClusterAppUIPresignedURL handler and its cluster-id resolution/validation.
Traces: emr.op-app-ui-ops-reject-unknown-targets

=== Cause 3 of 3: ListInstances does not refuse an unknown ClusterId, breaking the cluster-read refusal family ===
AWS: Smithy ListInstances takes ClusterId (@required, @length 0..256) with InstanceGroupId/InstanceGroupTypes/InstanceFleetId/InstanceFleetType/InstanceStates/Marker, and models InvalidRequestException (400) and InternalServerException (500). Recorded behavior: every cluster read operation — DescribeCluster, ListInstanceGroups, ListInstanceFleets, ListInstances, ListBootstrapActions, ListSteps, DescribeStep — refuses an unknown ClusterId uniformly with HTTP 400, error code `InvalidRequestException`, and the message `Cluster id '<ClusterId>' is not valid.`, whose fixed text is `Cluster id '` … `' is not valid.` with the ClusterId copied from the request. Conversely, none of those operations against a real, active cluster id may be refused with a message blaming the cluster id.
Emulator: ListInstances answers error code `NotImplemented` (`operation ListInstances is scaffolded but not implemented in this emulator`), so the shared unknown-cluster refusal is absent on that operation while its siblings implement it — the check fails with `ListInstances: expected InvalidRequestException, got 'NotImplemented'`.
Why: a client walking a cluster's instances cannot distinguish a stale/mistyped cluster id (retry impossible, fail fast) from a transient service problem, and the one operation that diverges from the family makes the refusal contract untrustworthy for all of them.
Fix: the ListInstances handler plus the shared cluster-id resolution used by the EMR cluster-read operations, so the refusal code and message are produced from one place.
Traces: emr.op-cluster-read-ops-reject-unknown-cluster

## Evidence
Behavioral replay against the booted emulator, service heading `### emr`.

Cause 1 — check emr.auto-termination-policy-invalid-idle-timeout-rejected:
  expected: PutAutoTerminationPolicy(ClusterId=<live cluster>, AutoTerminationPolicy={IdleTimeout: 10}) -> HTTP 400, Code=ValidationException, Message="Auto-termination policy IdleTimeout must be within a minimum of 60 seconds, and a maximum of 604800 seconds."; then GetAutoTerminationPolicy -> no AutoTerminationPolicy.IdleTimeout; then PutAutoTerminationPolicy(IdleTimeout=3600) -> 200 and GetAutoTerminationPolicy echoes 3600.
  actual: AssertionError: expected ValidationException, got NotImplemented
  (emulator error message: "operation PutAutoTerminationPolicy is scaffolded but not implemented in this emulator")

Cause 2 — check emr.op-app-ui-ops-reject-unknown-targets:
  expected: GetOnClusterAppUIPresignedURL(ClusterId="j-ZZZZZZZZZZZZZ") -> HTTP 400, Code=ValidationException, Message="Cluster id 'j-ZZZZZZZZZZZZZ' is not valid." (the quoted id is the request's ClusterId; the surrounding sentence is fixed). The same op against a real, still-active cluster with DryRun=True must NOT answer ValidationException.
  actual: AssertionError: expected ValidationException for every op, got mismatches: {'GetOnClusterAppUIPresignedURL': 'NotImplemented'}
  (CreatePersistentAppUI in the same check already answers ValidationException correctly, so the divergence is isolated to this operation.)

Cause 3 — check emr.op-cluster-read-ops-reject-unknown-cluster:
  expected: each of DescribeCluster, ListInstanceGroups, ListInstanceFleets, ListInstances, ListBootstrapActions, ListSteps, DescribeStep with ClusterId="j-ZZZZZZZZZZZZZ" -> HTTP 400, Code=InvalidRequestException, Message="Cluster id 'j-ZZZZZZZZZZZZZ' is not valid."
  actual: AssertionError: ListInstances: expected InvalidRequestException, got 'NotImplemented'
  (the sibling read ops reached their assertions, so only ListInstances diverges on the code.)

## Steps to Reproduce
1. Boot the emulator and point an EMR client at it.
2. Cause 1: RunJobFlow a keep-alive cluster, wait for RUNNING/WAITING, then call PutAutoTerminationPolicy(ClusterId=<id>, AutoTerminationPolicy={"IdleTimeout": 10}). Observe error code NotImplemented instead of a 400 ValidationException carrying "Auto-termination policy IdleTimeout must be within a minimum of 60 seconds, and a maximum of 604800 seconds." Then call GetAutoTerminationPolicy to confirm nothing was persisted, and PutAutoTerminationPolicy with IdleTimeout=3600 followed by GetAutoTerminationPolicy to confirm the valid value round-trips.
3. Cause 2: call GetOnClusterAppUIPresignedURL(ClusterId="j-" + "Z"*13). Observe NotImplemented instead of a 400 ValidationException with Message "Cluster id 'j-ZZZZZZZZZZZZZ' is not valid.". Repeat against a real, still-starting cluster with DryRun=True and confirm that case is NOT ValidationException.
4. Cause 3: call ListInstances(ClusterId="j-" + "Z"*13). Observe NotImplemented instead of a 400 InvalidRequestException with Message "Cluster id 'j-ZZZZZZZZZZZZZ' is not valid."; compare with DescribeCluster/ListSteps/ListInstanceGroups on the same unknown id, which already refuse. Then call ListInstances on a real, active cluster id and confirm it is not refused as an unknown cluster.
5. Re-replay the three checks named in trace_ids to confirm the fix.
