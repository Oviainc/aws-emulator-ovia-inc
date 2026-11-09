Issue-Source: trace-sweep-divergence
Issue-Trace: sso-admin-describe-list-edge-cases-r2
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sso-admin-describe-list-edge-cases-r2
Issue-Shape-Id: com.amazonaws.ssoadmin#SWBExternalService
Issue-Key: sso-admin|ListInstances|2xx->2xx|$.Instances[Name="sim2real-wire-idc-d82a3b58"].State
Issue-Commit: 8a62bc5d8898770ac63ef3e09ff3986bf7746acd

# ListInstances omits State on each listed Identity Center instance

> _Note: this issue was identified against the code base at commit `8a62bc5d8898770ac63ef3e09ff3986bf7746acd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstances

## Description
ONE cause: the shared InstanceMetadata serializer used by ListInstances never emits the instance's `State` view, so every listed Identity Center instance comes back without it.

Cause 1 of 1: ListInstances entries omit the State member (fixture sso-admin-describe-list-edge-cases-r2, step task#4).
AWS: each member of ListInstancesResponse.Instances carries, in addition to the modeled flat members, a `State` structure whose `Name` is the instance's lifecycle state, mirroring the member's `Status` enum value — an active instance is served as `"Status": "ACTIVE"` alongside `"State": {"Name": "ACTIVE"}`. This is recorded behavior only: the on-disk model (Smithy: com.amazonaws.ssoadmin#InstanceMetadata) declares InstanceArn, IdentityStoreId, IdentityStoreArn, OwnerAccountId, Name, CreatedDate, Status, StatusReason, PrimaryRegion and Regions and has no State member, and com.amazonaws.ssoadmin#ListInstancesResponse declares only Instances (@length max 10) and NextToken, so the recording is the authority. The member's identity values (InstanceArn, Name, IdentityStoreId/IdentityStoreArn, CreatedDate) are read from the instance being listed and are per-run values, not requirements; the requirement is that State is present and that State.Name equals the same instance's Status.
Emulator: HTTP 200 whose listed instance matched on every other asserted member but carries no State at all — $.Instances[Name="<instance name>"].State expected {'Name': 'ACTIVE'}, got None. No error code or message is involved; the status is correct and only the output member is missing.
Why: a client that reads or waits on an instance's lifecycle through State.Name across a listing (the shape the live API serves) dereferences a member that is absent on every instance, and cannot distinguish ACTIVE from CREATE_IN_PROGRESS or CREATE_FAILED while polling.
Fix: the ListInstances response serializer / the shared InstanceMetadata serializer — populate State from the same instance record that already supplies Status.
Traces: sso-admin-describe-list-edge-cases-r2#task#4.

Related open issue, which does NOT cover this step: 20261029T184546Z-sso-admin-instance-reads-drop-the-state-member-and-the.md describes the same omission but its recorded replay keys pin the listed instance by a per-run generated name (Instances[Name="sim2real-wire-idc-c56c0824"]), so this step's key — Instances[Name="sim2real-wire-idc-d82a3b58"] — does not join to it and the replay reports this cluster as unowned. Filing it so the step is actually re-replayed; a fixer closing that issue should confirm this binding too.

## Evidence
Fixture sso-admin-describe-list-edge-cases-r2, step task#4, ListInstances. Expected HTTP 200, actual HTTP 200 — the divergence is a missing output member, not a status or error-code difference:
  $.Instances[Name="sim2real-wire-idc-d82a3b58"].State: missing — expected {'Name': 'ACTIVE'}, got None
The instance name in that path is a per-run generated value from the recording environment; the requirement it witnesses is that the listed member carries a State structure whose Name equals that member's Status ('ACTIVE' here). Every other asserted member of the listed instance matched. Divergence key as replayed: service=sso-admin|ListInstances|status=200->200|keys=$.Instances[Name="sim2real-wire-idc-d82a3b58"].State.
Model cited: Smithy com.amazonaws.ssoadmin#InstanceMetadata (no State member) and com.amazonaws.ssoadmin#ListInstancesResponse (Instances, NextToken) — hence "recorded behavior only" for State.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the sso-admin wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture sso-admin-describe-list-edge-cases-r2 up to step task#4, which calls ListInstances after the describe/list edge-case setup has created an instance.
3. Inspect the 200 response body: each entry of Instances carries Status 'ACTIVE' but no State member.
4. Expected: every entry also carries State: {"Name": "<that entry's Status>"}, e.g. {"Name": "ACTIVE"} for an active instance.
