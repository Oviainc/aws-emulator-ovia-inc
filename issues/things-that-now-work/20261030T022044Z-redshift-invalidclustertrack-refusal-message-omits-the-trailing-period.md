Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.create-cluster-invalid-shape-and-in-use-parameter-group-delete-rejected
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift.create-cluster-invalid-shape-and-in-use-parameter-group-delete-rejected|run#11|AssertionError|assert got == want, f"the broken creates returned {got!r}, expected {want!r}"
Issue-Commit: 4fee60863f3ac3b34c6722c9235a070909377b2f

# Redshift InvalidClusterTrack refusal message omits the trailing period of AWS's wording

> _Note: this issue was identified against the code base at commit `4fee60863f3ac3b34c6722c9235a070909377b2f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
One net-new root cause in the redshift provisioned control plane.

Cause 1 of 1: the shared maintenance-track validation renders the `InvalidClusterTrack` error message without the sentence-terminating period AWS emits, so every operation that names a `MaintenanceTrackName` refuses with wording one character short of AWS's.

AWS: Smithy (services/service-com-amazonaws-redshift-redshiftserviceversion20121201/smithy-model/com-amazonaws-redshift-redshiftserviceversion20121201.json) models `CreateCluster` error `InvalidClusterTrackFault`, awsQueryError code `InvalidClusterTrack`, HTTP 400 — "The provided cluster track name is not valid." The message TEXT is recorded behavior only: the recording shows AWS answering HTTP 400 / `InvalidClusterTrack` with the message `Invalid cluster track: <MaintenanceTrackName>.` The fixed text is `Invalid cluster track: ` followed by the `MaintenanceTrackName` value taken verbatim from the request, and terminated by a period. In the failing check the request value is an unpublished track name, and the expected message is that template filled with it (the recording's own value is only an instance of the template, not part of it).

Emulator: the same request is correctly refused HTTP 400 / `InvalidClusterTrack`, but the message is `Invalid cluster track: <MaintenanceTrackName>` — identical except that the terminating period is missing. The check's own diff is the only difference between the whole expected/actual refusal maps: expected `('InvalidClusterTrack', 'Invalid cluster track: nosuch.')`, got `('InvalidClusterTrack', 'Invalid cluster track: nosuch')`. All four sibling refusals in the same check (single-node node count, multi-node node count, missing parameter group, short maintenance window) match AWS exactly, so the code selection and status are right and only this message template diverges.

Why: a client that classifies or asserts on the error message — SDK log scrapers, CLI output assertions, retry/alerting rules and parity test suites that compare the rendered message, and any UI that surfaces it — sees text that differs from AWS. Because the rule is shared by every operation that accepts a maintenance track (create, modify, restore), the divergence is reproduced on all of them, and a caller cannot round-trip AWS's message for this error class from the emulator.

Fix: the shared maintenance-track validation helper that builds the refusal message in the redshift service state, i.e. the `check_maintenance_track` rule consumed by the CreateCluster handler's pre-provisioning validation block. Its `Err(...)` message must terminate the sentence with a period, matching AWS's template `Invalid cluster track: {name}.` with `{name}` taken from the request's `MaintenanceTrackName`. The surrounding error selection (modeled `InvalidClusterTrackFault`, HTTP 400, code `InvalidClusterTrack`) is already correct and must not change.

Related but NOT covering this: open issue 20261029T192501Z-redshift-static-catalogs-are-synthetic-cluster-tracks-event.md concerns the CONTENT of the published maintenance-track catalog (which names are offered), not the wording of the refusal message for a name outside it. Fixed issue 20261024T024248Z-redshift-createcluster-skips-request-validation-and-publishes-an.md added this validation path; the call is correctly refused here, so this is a message-wording defect in that path and not a reopened refusal.

Traces: check redshift.create-cluster-invalid-shape-and-in-use-parameter-group-delete-rejected, which asserts the five broken-CreateCluster refusals as a map and diverges only on the maintenance-track entry. Code citations: services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/state.rs line 839 (`Err(format!("Invalid cluster track: {name}"))` — the missing period) inside `check_maintenance_track` declared at line 835; call site services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/create_cluster.rs lines 525-534 (`st::check_maintenance_track(track)` -> `CreateClusterErrorModeled::InvalidClusterTrackFault`).

## Evidence
Check redshift.create-cluster-invalid-shape-and-in-use-parameter-group-delete-rejected, assertion at line 55 of the check (`assert got == want`).

Expected (AWS), for the request whose only broken field is an unpublished MaintenanceTrackName:
  HTTP 400, Error.Code = InvalidClusterTrack
  Error.Message = "Invalid cluster track: <MaintenanceTrackName>."
  (fixed text: "Invalid cluster track: " + terminating "."; variable part: the MaintenanceTrackName value copied from the request)

Actual (emulator):
  HTTP 400, Error.Code = InvalidClusterTrack
  Error.Message = "Invalid cluster track: <MaintenanceTrackName>"   <-- terminating period absent

Replay diff as reported (the recorded run's generated token fills the variable part and is not itself a requirement):
  'unknown maintenance track': ('InvalidClusterTrack', 'Invalid cluster track: nosuch')      got
  'unknown maintenance track': ('InvalidClusterTrack', 'Invalid cluster track: nosuch.')     expected

The other four entries of the same assertion map are byte-identical between got and want:
  'single-node with 2 nodes': ('InvalidParameterValue', 'Number of nodes must be 1 or not be supplied for cluster type single-node.')
  'multi-node with 1 node': ('InvalidParameterValue', 'Number of nodes for cluster type multi-node must be greater than or equal to 2.')
  'missing parameter group': ('ClusterParameterGroupNotFound', 'ClusterParameterGroup not found: <group>')
  '10-minute maintenance window': ('InvalidParameterValue', 'Maintenance window must be at least 30minutes.')

Source of the divergence in the checkout: services/.../src/state.rs:839 formats the message with no trailing period, while the doc comment at state.rs:829-834 states it carries "the message the modelled InvalidClusterTrackFault carries".

## Steps to Reproduce
1. Create a cluster parameter group:
   aws redshift create-cluster-parameter-group --parameter-group-name <pg> --parameter-group-family redshift-1.0 --description "Parameter group for Redshift clusters"
2. Issue a CreateCluster that is otherwise valid (single-node, an orderable NodeType from DescribeOrderableClusterOptions, MasterUsername/MasterUserPassword, ClusterParameterGroupName=<pg>, PreferredMaintenanceWindow=wed:04:00-wed:04:30) but whose MaintenanceTrackName is a value DescribeClusterTracks does not publish, e.g.:
   aws redshift create-cluster --cluster-identifier <id> --cluster-type single-node --node-type ra3.large --master-username quartz_etl --master-user-password TestPassw0rd123 --cluster-parameter-group-name <pg> --maintenance-track-name nosuch --preferred-maintenance-window wed:04:00-wed:04:30
3. Observe the error: the code is InvalidClusterTrack with HTTP 400 (correct), but Error.Message is "Invalid cluster track: nosuch" where AWS returns "Invalid cluster track: nosuch." with a terminating period.
4. Repeat with ModifyCluster / RestoreFromClusterSnapshot naming the same unpublished track to see the same shared-rule wording.
5. Or replay the whole check: redshift.create-cluster-invalid-shape-and-in-use-parameter-group-delete-rejected.
