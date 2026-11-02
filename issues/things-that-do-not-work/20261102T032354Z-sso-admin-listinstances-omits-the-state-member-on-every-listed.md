Issue-Source: trace-sweep-divergence
Issue-Trace: sso-admin-describe-list-edge-cases-r2
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sso-admin-describe-list-edge-cases-r2
Issue-Shape-Id: com.amazonaws.ssoadmin#SWBExternalService
Issue-Key: sso-admin|ListInstances|2xx->2xx|$.Instances[Name="sim2real-wire-idc-327d3474"].State
Issue-Commit: 9d7649b3311435a15d98db18f1b7b1fd192e85c4

# ListInstances omits the State member on every listed Identity Center instance

> _Note: this issue was identified against the code base at commit `9d7649b3311435a15d98db18f1b7b1fd192e85c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstances

## Description
ONE cause.

Cause 1 of 1: ListInstances serializes each listed instance without its `State` member (fixture sso-admin-describe-list-edge-cases-r2, step task#4).

AWS: ListInstances returns `Instances`, a list of InstanceMetadata (Smithy: ListInstances -> ListInstancesResponse.Instances : InstanceList @length(min 0, max 10); paginated with NextToken/MaxResults; errors AccessDeniedException 403, InternalServerException 500, ThrottlingException 429, ValidationException 400). The on-disk model's InstanceMetadata stops at InstanceArn, IdentityStoreId, IdentityStoreArn, OwnerAccountId, Name, CreatedDate, Status, StatusReason, PrimaryRegion, Regions and declares NO State member, so for this member the recording is the authority: recorded behavior only — each listed instance additionally carries `State`, a structure whose single member `Name` mirrors that instance's lifecycle status enum, i.e. `State: {"Name": "<the instance's Status>"}` served alongside the flat `Status` member (an active instance shows `State.Name` = `ACTIVE` beside `Status` = `ACTIVE`). The structure is the requirement; the particular instance name, identity-store id, ARNs and CreatedDate in any one recording are per-run values of the listing environment, not requirements. Status 200 and no error code are involved — the divergence is purely a missing output member on a 200.

Emulator: answers 200 and matches the listed instance on every other member, but emits no `State` member at all. Replay reports `$.Instances[Name="..."].State: missing (expected {'Name': 'ACTIVE'}, got None)` for the instance listed by this step.

Why: a client that reads an instance's lifecycle from the listing — filtering a multi-instance listing for the ACTIVE one, or polling ListInstances until a newly enabled instance leaves CREATE_IN_PROGRESS — dereferences a member that is absent on every entry. It either crashes on the missing key or silently treats every instance as state-unknown, so it can never distinguish an active instance from one still being created or one that failed, and a wait loop built on State.Name never terminates.

Fix: the shared InstanceMetadata response serializer used by ListInstances — emit `State` as `{"Name": <instance status>}` derived from the same stored instance status the flat `Status` member is written from, for every entry in the list.

Related open issue, which does NOT cover this cluster: 20261029T184546Z-sso-admin-instance-reads-drop-the-state-member-and-the.md describes the same omission class, but its recorded clusters are keyed on other instances (its ListInstances causes name `$.Instances[Name="sim2real-wire-idc-c56c0824"].State`, `$.Instances[Name="IdentityCenter"].State` and `$.Instances[0]`). The replay reports this cluster as unowned, because its key carries a different per-run instance name, so it is filed separately and must be re-replayed in its own right even if that issue is closed first.

## Evidence
Fixture sso-admin-describe-list-edge-cases-r2, step task#4, operation ListInstances, expected HTTP 200 / actual HTTP 200 (no status or error-code divergence):

  $.Instances[Name="sim2real-wire-idc-327d3474"].State: missing — expected {'Name': 'ACTIVE'}, got None

All other members of that listed instance matched. The instance name in the JSONPath key (`sim2real-wire-idc-327d3474`) is a per-run generated value of the recording environment and is not part of the requirement; the requirement is that whichever instance is listed carries a `State` structure whose `Name` equals that instance's `Status` enum value.

Model evidence cited: Smithy ListInstancesResponse.Instances -> InstanceList @length(0,100 max 10); shape com.amazonaws.ssoadmin#InstanceMetadata declares no `State` member, hence "recorded behavior only" for this member.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the sso-admin wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture sso-admin-describe-list-edge-cases-r2 up to and including step task#4 (ListInstances over the ambient/created Identity Center instance).
3. Inspect the 200 response body: each entry of $.Instances has InstanceArn, IdentityStoreId, IdentityStoreArn, OwnerAccountId, Name, CreatedDate, Status, PrimaryRegion and Regions, but no State member.
4. Expected: every entry additionally carries State: {"Name": "<that instance's Status>"} — "ACTIVE" for an active instance.
