Issue-Source: trace-sweep-divergence
Issue-Trace: sso-admin-describe-list-edge-cases-r2
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sso-admin-describe-list-edge-cases-r2
Issue-Shape-Id: com.amazonaws.ssoadmin#SWBExternalService
Issue-Key: sso-admin|ListInstances|2xx->2xx|$.Instances[Name="sim2real-wire-idc-86d0b371"].State
Issue-Commit: 4fd31dd1d271cb10bcbd1385ecc8231faccc1a9e

# ListInstances omits InstanceMetadata.State on the probe-created Identity Center instance

> _Note: this issue was identified against the code base at commit `4fd31dd1d271cb10bcbd1385ecc8231faccc1a9e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstances

## Description
ONE cause: the shared InstanceMetadata serializer used by ListInstances never emits the instance's `State` view, so every listed Identity Center instance comes back without it.

Cause 1 of 1: ListInstances entry has no State member (fixture sso-admin-describe-list-edge-cases-r2, step task#4).
AWS: ListInstances returns `Instances` as a list of InstanceMetadata (Smithy: ListInstances -> ListInstancesResponse.Instances, target InstanceList, @length min 0 max 10; paginated inputToken/outputToken NextToken, pageSize MaxResults, items Instances). On the wire each member carries, in addition to the modeled members (InstanceArn, IdentityStoreId, IdentityStoreArn, OwnerAccountId, Name, CreatedDate, Status, StatusReason, PrimaryRegion, Regions), a `State` structure whose single member `Name` repeats the instance's lifecycle status value - for an active instance `"State": {"Name": "ACTIVE"}` beside `"Status": "ACTIVE"`. This is recorded behavior only: the on-disk model's com.amazonaws.ssoadmin#InstanceMetadata stops at Status/StatusReason and declares no State member, so the recording is the authority. The value of State.Name is read from the listed instance's own status in service state; the instance's Name, ARNs, identity-store id and CreatedDate in any one recording are per-run values and are not requirements.
Emulator: 200 whose listed instance matches on every other member but carries no State at all - `$.Instances[Name="..."].State`: expected `{'Name': 'ACTIVE'}`, got None (the recording's instance happened to be named `sim2real-wire-idc-86d0b371`; the name is a per-run probe value, the missing member is the defect).
Why: a client that reads or waits on an instance's lifecycle through `State.Name` across a listing - the shape the live API serves - dereferences a member that is absent on every instance and cannot distinguish ACTIVE from CREATE_IN_PROGRESS/CREATE_FAILED without a second DescribeInstance call that is equally incomplete.
Fix: the sso-admin InstanceMetadata response serializer shared by ListInstances (and DescribeInstance) - populate State from the same instance record Status is read from, so State.Name mirrors Status.
Traces: sso-admin-describe-list-edge-cases-r2#task#4.

Relation to open issues: 20261029T184546Z-sso-admin-instance-reads-drop-the-state-member-and-the.md describes the same serializer omission, but its replay keys pin the ListInstances cluster of this fixture to a DIFFERENT, per-run probe instance name (`sim2real-wire-idc-c56c0824`), so this run's cluster is not covered by it and the replay report attributes no owner to it. It must be re-replayed in its own right; a fixer closing that issue should verify this step too. 20261029T112447Z-sso-admin-listinstances-pagination.md concerns NextToken decoding on the same operation, not the serialized instance shape.

## Evidence
Fixture sso-admin-describe-list-edge-cases-r2, step task#4, operation ListInstances, expected HTTP 200 / actual HTTP 200 (status and error code agree; the body diverges):
  $.Instances[Name="sim2real-wire-idc-86d0b371"].State: missing - expected {'Name': 'ACTIVE'}, got None

No other member of that listed instance diverged, so the only difference is the absent State structure. Requirement: the member is present and State.Name equals the instance's Status enum value. Excluded as per-run values: the instance Name in the path selector, and the instance's ARNs/IdentityStoreId/CreatedDate.

Smithy consulted: ListInstances (output Instances -> InstanceList, @length 0..10, paginated NextToken/MaxResults; errors AccessDeniedException 403, InternalServerException 500, ThrottlingException 429, ValidationException 400) and com.amazonaws.ssoadmin#InstanceMetadata (members InstanceArn, IdentityStoreId, IdentityStoreArn, OwnerAccountId, Name, CreatedDate, Status, StatusReason, PrimaryRegion, Regions - NO State member, hence "recorded behavior only").

## Steps to Reproduce
1. Clone this repository and boot the emulator built from the checkout.
2. Replay wire fixture sso-admin-describe-list-edge-cases-r2 from trace set simparity-wire-aws-v1 against it.
3. At step task#4 the fixture calls sso-admin ListInstances after the earlier steps have created/described the probe instance.
4. Inspect the 200 response body: each entry of $.Instances carries InstanceArn, IdentityStoreId, IdentityStoreArn, OwnerAccountId, Name, CreatedDate, Status, PrimaryRegion and Regions, but no State member.
5. Expected (per the recording): the entry also carries State as a structure whose Name equals the instance's Status, i.e. State: {"Name": "ACTIVE"} for the ACTIVE instance.
