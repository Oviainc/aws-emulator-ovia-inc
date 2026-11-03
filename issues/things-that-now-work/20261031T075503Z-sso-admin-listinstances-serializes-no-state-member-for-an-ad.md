Issue-Source: trace-sweep-divergence
Issue-Trace: sso-admin-describe-list-edge-cases-r2
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sso-admin-describe-list-edge-cases-r2
Issue-Shape-Id: com.amazonaws.ssoadmin#SWBExternalService
Issue-Key: sso-admin|ListInstances|2xx->2xx|$.Instances[Name="sim2real-wire-idc-3e2efb01"].State
Issue-Commit: 9a7530f5beb5c94af0d9f6e087fc2caa8680119d

# ListInstances serializes no State member for an ad-hoc named Identity Center instance

> _Note: this issue was identified against the code base at commit `9a7530f5beb5c94af0d9f6e087fc2caa8680119d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstances

## Description
ONE cause: the InstanceMetadata serializer used by ListInstances never emits the `State` view AWS serves on an Identity Center instance, so a listed instance arrives without its state structure. A fixer must close this one cause before submitting.

Cause 1 of 1: ListInstances entries omit `State` (fixture sso-admin-describe-list-edge-cases-r2, step task#4).
AWS: ListInstances returns 200 with `Instances`, a list of InstanceMetadata (Smithy: ListInstancesResponse -> Instances: InstanceList @length max 10, NextToken: Token @length 0..2048 @pattern ^[-a-zA-Z0-9+=/_]*$). The on-disk model's InstanceMetadata stops at InstanceArn / IdentityStoreId / IdentityStoreArn / OwnerAccountId / Name / CreatedDate / Status (InstanceStatus enum) / StatusReason / PrimaryRegion / Regions and declares NO State member, so for `State` this is recorded behavior only: the recording shows each listed instance additionally carrying a `State` structure whose `Name` mirrors that instance's Status enum value, i.e. {"Name": "ACTIVE"} alongside Status "ACTIVE". The rule is the mirroring, not one run's value: State.Name is whatever the instance's own lifecycle status is (ACTIVE, CREATE_IN_PROGRESS, ...), and the surrounding identity values (InstanceArn, Name, IdentityStoreId/Arn, CreatedDate) are read from the instance record and are per-environment, not requirements.
Emulator: the step is answered 200 and every other member of the listed instance matches, but the `State` member is absent: $.Instances[Name="..."].State expected {'Name': 'ACTIVE'}, got None. No status or error code diverges here — the divergence is purely the omitted member in the 200 body.
Why: a client that reads an instance's lifecycle from State.Name while enumerating instances (the shape the current API serves) dereferences a missing member and cannot distinguish an ACTIVE instance from one still being created or failed, so waiters and filters over a listing never settle.
Fix: the shared InstanceMetadata response serializer that ListInstances uses — populate State from the same instance record that already supplies Status, so State.Name tracks the instance status enum.
Traces: sso-admin-describe-list-edge-cases-r2#task#4.

Relation to open issues: 20261029T184546Z-sso-admin-instance-reads-drop-the-state-member-and-the.md describes the same family of omission, but its recorded replay keys bind to instance names from an earlier recording (e.g. $.Instances[Name="sim2real-wire-idc-c56c0824"].State); this cluster's key is $.Instances[Name="sim2real-wire-idc-3e2efb01"].State and is NOT claimed by that issue's keys, so the replay reports it unowned and it is filed here so the fixer actually re-replays it. 20261029T112447Z-sso-admin-listinstances-pagination.md concerns NextToken decoding on the same operation, not the serialized instance shape, and does not cover it.

## Evidence
Fixture sso-admin-describe-list-edge-cases-r2, step task#4, operation ListInstances, expected HTTP 200 / actual HTTP 200 (no status or error-code divergence):
  $.Instances[Name="sim2real-wire-idc-3e2efb01"].State: missing — expected {'Name': 'ACTIVE'}, got None

The instance name in that JSONPath is a per-run generated value from the fixture's own setup, not part of the requirement; the requirement is that EVERY member of Instances carries a State structure whose Name equals that instance's Status enum value (here 'ACTIVE', which the emulator does serve in the sibling Status member).

Smithy evidence that this is recorded behavior only: com.amazonaws.ssoadmin#InstanceMetadata declares InstanceArn, IdentityStoreId, IdentityStoreArn, OwnerAccountId, Name, CreatedDate, Status, StatusReason, PrimaryRegion, Regions — and no State member; com.amazonaws.ssoadmin#ListInstancesResponse declares only Instances and NextToken.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the sso-admin wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture sso-admin-describe-list-edge-cases-r2 up to step task#4 (ListInstances after the fixture's instance setup).
3. Inspect the 200 body: each entry of $.Instances carries Status 'ACTIVE' but has no State member.
4. Expected per the recording: the same entry additionally carries State: {"Name": "ACTIVE"}, i.e. State.Name mirroring the entry's Status.
