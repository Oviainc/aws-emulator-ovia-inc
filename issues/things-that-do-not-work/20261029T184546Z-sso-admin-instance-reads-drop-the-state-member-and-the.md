Issue-Source: trace-sweep-divergence
Issue-Trace: sso-admin-instance-describe-update-and-quota-r2
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sso-admin-instance-describe-update-and-quota-r2 sso-admin-describe-list-edge-cases-r2 sso-admin-tagging-round-trip-r2
Issue-Shape-Id: com.amazonaws.ssoadmin#SWBExternalService
Issue-Key: sso-admin|DescribeInstance|2xx->2xx|$.Instance
Issue-Key: sso-admin|ListInstances|2xx->2xx|$.Instances[Name="sim2real-wire-idc-c56c0824"].State
Issue-Key: sso-admin|ListInstances|2xx->2xx|$.Instances[Name="IdentityCenter"].State
Issue-Key: sso-admin|ListInstances|2xx->2xx|$.Instances[*]
Issue-Commit: 5dbb2ab27a0402c002f5157a9a56e61b81adc0a2

# Instance reads drop the State member and the Instance envelope, so DescribeInstance and ListInstances omit the instance state AWS serves

> _Note: this issue was identified against the code base at commit `5dbb2ab27a0402c002f5157a9a56e61b81adc0a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstance

## Description
FOUR causes, all the same omission in the instance serializer: the state view AWS serves on an Identity Center instance is never emitted. A fixer must close all four before submitting.

Cause 1 of 4 - DescribeInstance omits the Instance envelope (fixture sso-admin-instance-describe-update-and-quota-r2, steps task#3 and task#7).
AWS: DescribeInstance returns the instance's flat members (Smithy: DescribeInstanceResponse -> InstanceArn, IdentityStoreId, IdentityStoreArn, OwnerAccountId, Name, CreatedDate, Status, StatusReason, PrimaryRegion, Regions, EncryptionConfigurationDetails, PermissionSetsEnabled) AND, recorded behavior only, an additional `Instance` structure repeating those members plus `State: {"Name": <instance status>}`; State.Name mirrors the instance's Status enum value (ACTIVE for an active instance). The on-disk model has no Instance member on DescribeInstanceResponse and no State member on InstanceMetadata, so the recording is the authority here. Every value inside the envelope is read from the instance's own state - the ARNs, identity-store id and CreatedDate in any one recording are per-run values, not requirements.
Emulator: 200 carrying only the flat members; $.Instance is absent (expected the full instance object, got None) on both task#3 and task#7, while the flat members themselves matched.
Why: a client that reads the instance's lifecycle from DescribeInstance's Instance/State view (the shape the current API serves) dereferences a missing member and cannot tell ACTIVE from CREATE_IN_PROGRESS.
Fix: the DescribeInstance response serializer - build the Instance envelope from the same instance record the flat members come from, including State.
Traces: sso-admin-instance-describe-update-and-quota-r2#task#3 (and #task#7, same cluster).

Cause 2 of 4 - ListInstances entries omit State (fixture sso-admin-describe-list-edge-cases-r2, step task#4).
AWS: each InstanceList member carries State as a structure whose Name is the instance's status, e.g. {"Name": "ACTIVE"} alongside Status "ACTIVE" (recorded behavior only - InstanceMetadata in the on-disk model stops at Status/StatusReason). The member's identity values (InstanceArn, Name, IdentityStoreId) come from the instance being listed.
Emulator: 200 whose listed instance matched on every other member but has no State: $.Instances[Name="..."].State expected {'Name': 'ACTIVE'}, got None.
Why: a client that filters or waits on State.Name across a listing sees the member missing on every instance.
Fix: the shared InstanceMetadata serializer used by ListInstances.
Traces: sso-admin-describe-list-edge-cases-r2#task#4.

Cause 3 of 4 - the same State omission on the instance listed after a describe/update/quota sequence (fixture sso-admin-instance-describe-update-and-quota-r2, step task#4): $.Instances[Name="IdentityCenter"].State expected {'Name': 'ACTIVE'}, got None. Same AWS behavior, same emulator defect, same fix site as Cause 2; it is a distinct recorded cluster and must be re-replayed.
Traces: sso-admin-instance-describe-update-and-quota-r2#task#4.

Cause 4 of 4 - the same State omission on the ambient instance listed at setup (fixture sso-admin-tagging-round-trip-r2, step setup#1).
AWS: the ListInstances member carries State {"Name": "ACTIVE"} beside Status "ACTIVE".
Emulator: the served member has no State member at all.
Scoping note for the fixer: the rest of that step's diff is per-environment, NOT a requirement - the recording's instance Name, IdentityStoreId/IdentityStoreArn and CreatedDate are whatever the recording environment's ambient instance happened to carry, and the fixture templates its InstanceArn. Only the missing State member is the emulator's defect here.
Traces: sso-admin-tagging-round-trip-r2#setup#1.

No open sso-admin issue covers this: 20261029T112447Z-sso-admin-listinstances-pagination.md concerns NextToken decoding on the same operation, not the serialized instance shape.

## Evidence
Cause 1 - DescribeInstance, fixture sso-admin-instance-describe-update-and-quota-r2 steps task#3/task#7, expected 200 / actual 200:
  $.Instance: missing - expected {'CreatedDate': ..., 'IdentityStoreArn': 'arn:aws:identitystore::000000000000:identitystore/d-9267cac78c', 'IdentityStoreId': 'd-9267cac78c', 'InstanceArn': 'arn:aws:sso:::instance/ssoins-7223b1d1ac5e1f0e', 'Name': 'IdentityCenter', 'OwnerAccountId': '000000000000', 'PrimaryRegion': 'us-west-2', 'Regions': [{'AddedDate': ..., 'IsPrimaryRegion': True, 'RegionName': 'us-west-2', 'Status': 'ACTIVE'}], 'State': {'Name': 'ACTIVE'}, 'Status': 'ACTIVE'}, got None (ids/dates above are per-run values).

Cause 2 - ListInstances, fixture sso-admin-describe-list-edge-cases-r2 step task#4, expected 200 / actual 200:
  $.Instances[Name="sim2real-wire-idc-c56c0824"].State: expected {'Name': 'ACTIVE'}, got None

Cause 3 - ListInstances, fixture sso-admin-instance-describe-update-and-quota-r2 step task#4, expected 200 / actual 200:
  $.Instances[Name="IdentityCenter"].State: expected {'Name': 'ACTIVE'}, got None

Cause 4 - ListInstances, fixture sso-admin-tagging-round-trip-r2 step setup#1, expected 200 / actual 200:
  $.Instances[0]: expected a member carrying 'State': {'Name': 'ACTIVE'} (plus Status 'ACTIVE'), got {'InstanceArn': ..., 'IdentityStoreId': ..., 'OwnerAccountId': '000000000000', 'Name': 'IdentityCenter', 'CreatedDate': ..., 'Status': 'ACTIVE', 'PrimaryRegion': 'us-west-2', 'Regions': [...]} with no State member. The Name/IdentityStoreId/CreatedDate differences in this diff are ambient per-run values and are excluded from the finding.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the sso-admin wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: replay fixture sso-admin-instance-describe-update-and-quota-r2, step task#3 (and task#7) - DescribeInstance on the instance ARN. Observe the 200 body has no Instance member.
3. Cause 2: replay fixture sso-admin-describe-list-edge-cases-r2, step task#4 - ListInstances. Observe the listed instance has no State member.
4. Cause 3: replay fixture sso-admin-instance-describe-update-and-quota-r2, step task#4 - ListInstances; same missing State.
5. Cause 4: replay fixture sso-admin-tagging-round-trip-r2, step setup#1 - ListInstances; same missing State on the ambient instance.
