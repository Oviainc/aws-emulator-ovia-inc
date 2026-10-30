Issue-Source: trace-sweep-divergence
Issue-Trace: sso-admin-describe-list-edge-cases-r2
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sso-admin-describe-list-edge-cases-r2
Issue-Shape-Id: com.amazonaws.ssoadmin#SWBExternalService
Issue-Key: sso-admin|ListInstances|2xx->2xx|$.Instances[Name="sim2real-wire-idc-35fd1412"].State
Issue-Commit: 43252aeb825d83cca4ce2cc37a2476b8a6fd6233

# ListInstances omits the State member on the listed Identity Center instance

> _Note: this issue was identified against the code base at commit `43252aeb825d83cca4ce2cc37a2476b8a6fd6233`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstances

## Description
ONE cause: the shared Identity Center instance serializer used by ListInstances never emits the per-instance `State` view, so every listed instance is served without its lifecycle state.

Cause 1 of 1: ListInstances entry has no State member (fixture sso-admin-describe-list-edge-cases-r2, step task#4).
AWS: each member of ListInstances' `Instances` list carries, besides the modeled members, a `State` structure whose `Name` is the instance's lifecycle state - for an active instance `State: {"Name": "ACTIVE"}`, mirroring the member's `Status` enum value. Recorded behavior only: the on-disk model (Smithy: ListInstances.output -> Instances: InstanceList; shape InstanceMetadata members InstanceArn, IdentityStoreId, IdentityStoreArn, OwnerAccountId, Name, CreatedDate, Status, StatusReason, PrimaryRegion, Regions) declares NO State member, so the recording is the authority. The structure's single member `Name` takes the instance's current status value from state; the instance's own identity values (InstanceArn, Name, IdentityStoreId, CreatedDate) are per-run and are not part of the requirement.
Emulator: 200 whose listed instance matches on every other member but carries no State at all - `$.Instances[Name="sim2real-wire-idc-35fd1412"].State` expected `{'Name': 'ACTIVE'}`, got None. The instance name in that path selector is a per-run generated name, not a requirement.
Why: a client that reads or waits on `State.Name` across a listing (the shape the current API serves) dereferences a missing member on every instance and cannot distinguish ACTIVE from CREATE_IN_PROGRESS / CREATE_FAILED without a second DescribeInstance call - which omits the same view.
Fix: the shared InstanceMetadata response serializer used by ListInstances (sso-admin instance read path) - populate State from the same instance record that already supplies Status.
Traces: sso-admin-describe-list-edge-cases-r2#task#4.

Related but not covering: open issue 20261029T184546Z-sso-admin-instance-reads-drop-the-state-member-and-the.md describes the same omission, but its recorded replay keys bind to a listing whose instance was named `sim2real-wire-idc-c56c0824`; this cluster's key (`$.Instances[Name="sim2real-wire-idc-35fd1412"].State`) is a distinct, currently unowned cluster and the replay report attributes it to no open issue, so it is filed here and must be re-replayed in its own right. Open issue 20261029T112447Z-sso-admin-listinstances-pagination.md concerns NextToken decoding on the same operation, not the serialized instance shape.

## Evidence
Fixture sso-admin-describe-list-edge-cases-r2, step task#4, operation ListInstances, expected HTTP 200 / actual HTTP 200 (no status or error-code divergence; the body shape diverges):
  $.Instances[Name="sim2real-wire-idc-35fd1412"].State: missing - expected {'Name': 'ACTIVE'}, got None

Fixed requirement: the listed instance member includes a `State` structure with member `Name`. Value part: `Name` equals the instance's current status (here ACTIVE, read from state). Excluded as per-run values: the instance's generated name (sim2real-wire-idc-35fd1412), its InstanceArn, IdentityStoreId/IdentityStoreArn and CreatedDate.

Smithy evidence that this is recorded-behavior-only: com.amazonaws.ssoadmin#InstanceMetadata declares InstanceArn, IdentityStoreId, IdentityStoreArn, OwnerAccountId, Name, CreatedDate, Status, StatusReason, PrimaryRegion, Regions - and no State member.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the sso-admin wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture sso-admin-describe-list-edge-cases-r2 up to step task#4 (ListInstances after the describe/list edge-case setup).
3. Inspect the 200 response body: the entry in $.Instances for the instance created by that fixture carries Status 'ACTIVE' but has no 'State' member.
4. Expected: that entry also carries 'State': {'Name': 'ACTIVE'}.
