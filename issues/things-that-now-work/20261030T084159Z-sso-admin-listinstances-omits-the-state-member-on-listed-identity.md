Issue-Source: trace-sweep-divergence
Issue-Trace: sso-admin-describe-list-edge-cases-r2
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sso-admin-describe-list-edge-cases-r2
Issue-Shape-Id: com.amazonaws.ssoadmin#SWBExternalService
Issue-Key: sso-admin|ListInstances|2xx->2xx|$.Instances[Name="sim2real-wire-idc-d6f1418e"].State
Issue-Commit: 5a505a9733213b219e0f9709fa73583c4d676128

# ListInstances omits the State member on listed Identity Center instances

> _Note: this issue was identified against the code base at commit `5a505a9733213b219e0f9709fa73583c4d676128`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstances

## Description
ONE cause. The shared InstanceMetadata serializer used by ListInstances never emits the instance's `State` view, so every listed Identity Center instance comes back without it.

Cause 1 of 1: ListInstances entries omit State (fixture sso-admin-describe-list-edge-cases-r2, step task#4).
AWS: ListInstances returns `Instances`, a list of instance metadata members (Smithy: ListInstances -> ListInstancesResponse.Instances : InstanceList @length min 0 max 10, paginated inputToken/outputToken NextToken, pageSize MaxResults, items Instances; errors AccessDeniedException 403, InternalServerException 500, ThrottlingException 429, ValidationException 400). The on-disk model's InstanceMetadata shape stops at InstanceArn / IdentityStoreId / IdentityStoreArn / OwnerAccountId / Name / CreatedDate / Status (InstanceStatus enum) / StatusReason / PrimaryRegion / Regions and declares NO State member, so for State this is recorded behavior only: the recorded 200 carries, on each listed instance, an additional `State` structure whose `Name` is the instance's lifecycle status, e.g. {"Name": "ACTIVE"} beside the flat Status "ACTIVE". State.Name mirrors the instance's own status enum value read from state (ACTIVE for an active instance, CREATE_IN_PROGRESS / CREATE_FAILED etc. otherwise) - it is not a constant. The instance identity values in any one recording (the generated instance name, the instance ARN, the identity-store id, CreatedDate) are per-run values of the recording environment and are NOT requirements.
Emulator: answers 200 with a listed instance that matches on every other member but has no State member at all - $.Instances[Name="<generated instance name>"].State expected {'Name': 'ACTIVE'}, got None. Status/expected status both 200, so this is a body-shape divergence, not a status or error-code divergence.
Why: a client that reads or waits on an instance's lifecycle from the listing's State.Name dereferences a missing member on every instance returned, and cannot distinguish an ACTIVE instance from one still being created or one that failed to create - it either crashes or treats a not-yet-ready instance as usable.
Fix: the shared InstanceMetadata response serializer used by ListInstances (and by the instance list path generally) - populate State from the same instance record the flat Status member is read from, as State: {"Name": <instance status>}.
Traces: sso-admin-describe-list-edge-cases-r2#task#4.

Relationship to an existing open issue: 20261029T184546Z-sso-admin-instance-reads-drop-the-state-member-and-the.md describes the same omission on this same fixture/step, but its recorded replay keys are pinned to a different per-run instance name ($.Instances[Name="sim2real-wire-idc-c56c0824"].State), so the key this replay produced ($.Instances[Name="sim2real-wire-idc-d6f1418e"].State) is not covered by that issue's keys and this cluster replayed as unowned. A fixer closing this issue will very likely close that one's Cause 2 with the same edit; the per-run instance name in either key is an environment value and must not be treated as part of the requirement.

## Evidence
Fixture sso-admin-describe-list-edge-cases-r2, step task#4, operation ListInstances, verdict mismatch, expected status 200 / actual status 200.

Divergence detail as replayed:
  $.Instances[Name="sim2real-wire-idc-d6f1418e"].State: missing (expected {'Name': 'ACTIVE'}, got None)

Requirement vs environment values in that diff:
  - requirement: the listed instance carries a State structure whose Name equals the instance's status (here ACTIVE, matching the flat Status member the emulator already serves correctly).
  - environment value, NOT a requirement: the instance name in the selector (sim2real-wire-idc-d6f1418e) is generated per run, as are the instance ARN, identity-store id/ARN and CreatedDate.

No other member of this step diverged - the cluster's only key is the missing State member (affected_count 1, stub_path empty, owned_by empty).

Model evidence: Smithy InstanceMetadata (com.amazonaws.ssoadmin#InstanceMetadata) declares no State member; ListInstancesResponse.Instances targets InstanceList. State is therefore grounded in the recording, not the model.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from this checkout.
2. Replay the sso-admin wire corpus (trace set simparity-wire-aws-v1).
3. Replay fixture sso-admin-describe-list-edge-cases-r2 up to step task#4 - the setup steps create the Identity Center instance, then task#4 calls ListInstances.
4. Inspect the 200 body: each entry of $.Instances carries InstanceArn, IdentityStoreId, IdentityStoreArn, OwnerAccountId, Name, CreatedDate, Status, PrimaryRegion and Regions, but no State member.
5. Expected: the entry additionally carries State: {"Name": "ACTIVE"} (State.Name equal to the instance's Status). Actual: State is absent, so the replay reports $.Instances[Name=<instance name>].State missing.
