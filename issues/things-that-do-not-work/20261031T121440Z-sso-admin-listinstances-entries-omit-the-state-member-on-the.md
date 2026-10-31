Issue-Source: trace-sweep-divergence
Issue-Trace: sso-admin-describe-list-edge-cases-r2
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sso-admin-describe-list-edge-cases-r2
Issue-Shape-Id: com.amazonaws.ssoadmin#SWBExternalService
Issue-Key: sso-admin|ListInstances|2xx->2xx|$.Instances[Name="sim2real-wire-idc-6249efb3"].State
Issue-Commit: fae970997573727cb749ee4b22e6e053e501c03c

# ListInstances entries omit the State member on the generated edge-case instance

> _Note: this issue was identified against the code base at commit `fae970997573727cb749ee4b22e6e053e501c03c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstances

## Description
ONE cause: the sso-admin InstanceMetadata serializer used by ListInstances never emits the instance's `State` view, so every listed Identity Center instance is served without it.

Cause 1 of 1: ListInstances omits State on the listed instance (fixture sso-admin-describe-list-edge-cases-r2, step task#4).
AWS: ListInstances returns Instances as an InstanceList (Smithy: ListInstances -> ListInstancesResponse.Instances, target InstanceList @length min 0 max 10; paginated inputToken/outputToken NextToken, items Instances). Each listed instance carries, recorded behavior only, a `State` structure whose single member `Name` holds the instance's lifecycle value, served alongside the flat `Status` enum — i.e. State {"Name": "ACTIVE"} beside Status "ACTIVE" for an active instance. The on-disk model's InstanceMetadata structure stops at InstanceArn/IdentityStoreId/IdentityStoreArn/OwnerAccountId/Name/CreatedDate/Status/StatusReason/PrimaryRegion/Regions and declares no State member, so the recording is the authority for this member. State.Name mirrors whatever lifecycle value the listed instance is in (ACTIVE, CREATE_IN_PROGRESS, CREATE_FAILED ...); the instance's identity values — its Name, InstanceArn, IdentityStoreId/IdentityStoreArn and CreatedDate — are read from the instance being listed and are per-run values, not requirements of this finding (the recorded instance name sim2real-wire-idc-6249efb3 is a generated name and only identifies which list member the assertion selected).
Emulator: 200 whose listed instance matches on every other member but carries no State member at all: $.Instances[Name="sim2real-wire-idc-6249efb3"].State expected {'Name': 'ACTIVE'}, got None. Status/HTTP code are correct (200 expected, 200 actual) and no error is involved; the defect is purely the omitted output member.
Why: a client that reads an instance's lifecycle from the listing's State.Name — the shape the current API serves — dereferences a missing member on every instance returned, so it cannot distinguish an ACTIVE instance from one still being created or failed, and a wait/filter loop keyed on State.Name never terminates.
Fix: the shared InstanceMetadata response serializer in the sso-admin service crate (the one ListInstances renders each Instances member through): populate State from the same instance record that already supplies Status.
Traces: sso-admin-describe-list-edge-cases-r2#task#4.

Relation to an existing open issue: 20261029T184546Z-sso-admin-instance-reads-drop-the-state-member-and-the.md describes the same omission and lists this fixture/step among its causes, but its recorded replay key pins the list member by a per-run generated instance name (sim2real-wire-idc-c56c0824) that this replay did not produce (sim2real-wire-idc-6249efb3), so the replay reported this cluster as unowned. That issue does not cover this cluster's replay key; closing it must also clear this one.

## Evidence
Fixture sso-admin-describe-list-edge-cases-r2, step task#4, operation ListInstances, expected HTTP 200 / actual HTTP 200, verdict mismatch:
  $.Instances[Name="sim2real-wire-idc-6249efb3"].State: missing — expected {'Name': 'ACTIVE'}, got None

All other members of the listed instance matched. The selector's instance name (sim2real-wire-idc-6249efb3) is a per-run generated value used only to pick the list member; the requirement is that each listed instance carries a State structure whose Name equals that instance's lifecycle status (ACTIVE here, matching the Status member served beside it). No error code or error message is involved: both expected and actual responses are 200.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the sso-admin wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture sso-admin-describe-list-edge-cases-r2 up to step task#4 — the step creates/uses an Identity Center instance and then calls ListInstances.
3. Inspect the 200 body: the instance returned in $.Instances has Status "ACTIVE" but no State member.
4. Expected: that list member also carries State: {"Name": "ACTIVE"}, i.e. a State structure whose Name equals the instance's lifecycle status.
