Issue-Source: trace-sweep-divergence
Issue-Trace: sso-admin-describe-list-edge-cases-r2
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sso-admin-describe-list-edge-cases-r2
Issue-Shape-Id: com.amazonaws.ssoadmin#SWBExternalService
Issue-Key: sso-admin|ListInstances|2xx->2xx|$.Instances[Name="sim2real-wire-idc-b3e2282c"].State
Issue-Commit: 4ac27352e7b3d7cddd4c7ab08e294db28bfa593a

# ListInstances serves instance entries without the State member

> _Note: this issue was identified against the code base at commit `4ac27352e7b3d7cddd4c7ab08e294db28bfa593a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstances

## Description
ONE cause: the InstanceMetadata serializer used by ListInstances never emits the instance's `State` view, so the listed instance is served without the state member AWS includes.

Cause 1 of 1: ListInstances entry omits State (fixture sso-admin-describe-list-edge-cases-r2, step task#4).
AWS: ListInstances returns `Instances` (Smithy: ListInstances -> output members Instances: InstanceList @length 0..10, NextToken: Token @length 0..2048 pattern ^[-a-zA-Z0-9+=/_]*$; errors AccessDeniedException 403, InternalServerException 500, ThrottlingException 429, ValidationException 400). Each member is InstanceMetadata, whose on-disk modelled members are InstanceArn, IdentityStoreId, IdentityStoreArn, OwnerAccountId, Name, CreatedDate, Status (enum InstanceStatus), StatusReason, PrimaryRegion, Regions. The on-disk model declares NO `State` member on InstanceMetadata, so for State this is recorded behavior only: the recording shows each listed instance additionally carrying a `State` structure whose `Name` mirrors that instance's Status enum value, i.e. `State: {"Name": "ACTIVE"}` beside `Status: "ACTIVE"` for an active instance. The requirement is the member and the mirroring rule; the instance's identity values (InstanceArn, Name, IdentityStoreId/IdentityStoreArn, CreatedDate) are read from the instance being listed and the generated instance name in any one recording is a per-run value, not part of the requirement.
Emulator: HTTP 200 whose listed instance matches on every other member but has no State member at all: `$.Instances[Name="<instance name>"].State` expected `{'Name': 'ACTIVE'}`, got None. No error code or message is involved — the status is the correct 200 and only the serialized shape diverges.
Why: a client that reads an instance's lifecycle from `State.Name` when enumerating instances (the shape the live API serves) finds the member absent on every instance, so it cannot distinguish ACTIVE from a non-active state such as CREATE_IN_PROGRESS or CREATE_FAILED and either dereferences a missing member or treats every instance as state-unknown.
Fix: the shared InstanceMetadata response serializer in the sso-admin service crate, as used by the ListInstances handler — derive `State.Name` from the same instance record whose `Status` it already serializes.
Traces: sso-admin-describe-list-edge-cases-r2#task#4.

Relation to existing issues: open issue 20261029T184546Z-sso-admin-instance-reads-drop-the-state-member-and-the.md describes the same family of State omissions, but its recorded replay keys bind to a different generated instance name on this fixture (`$.Instances[Name="sim2real-wire-idc-c56c0824"].State`) and do not cover this cluster's key, so this cluster is replayed and re-filed here rather than assumed closed by it. Open issue 20261029T112447Z-sso-admin-listinstances-pagination.md concerns NextToken handling on the same operation, not the serialized instance shape.

## Evidence
Cause 1 — ListInstances, fixture sso-admin-describe-list-edge-cases-r2, step task#4. Expected status 200, actual status 200 (status agrees).
  Divergence key: service=sso-admin|ListInstances|status=200->200|keys=$.Instances[Name="..."].State
  Detail: $.Instances[Name="sim2real-wire-idc-b3e2282c"].State: missing (expected {'Name': 'ACTIVE'}, got None)
  The instance name in that path (`sim2real-wire-idc-b3e2282c`) is a per-run generated value from the fixture's own setup, not a requirement; the requirement is that the listed instance carries a State structure whose Name equals its Status value (here ACTIVE).
  Model citation: Smithy InstanceMetadata (com.amazonaws.ssoadmin#InstanceMetadata) members stop at Status/StatusReason/PrimaryRegion/Regions — no State member is modelled, hence "recorded behavior only" for State.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the sso-admin wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture sso-admin-describe-list-edge-cases-r2 up to step task#4, which issues ListInstances after the fixture's instance setup steps.
3. Inspect the 200 response body's Instances list entry for the instance created by that fixture.
4. Observe the entry carries Status "ACTIVE" but no State member; AWS serves State {"Name": "ACTIVE"} alongside it.
