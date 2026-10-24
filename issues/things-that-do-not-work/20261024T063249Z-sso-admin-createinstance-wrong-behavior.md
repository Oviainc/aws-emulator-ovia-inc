Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# A just-created instance reports Regions[].Status = CREATE_IN_PROGRESS, a value outside the modeled RegionStatus enum

_Filed: 2026-10-24T06:32:48Z_
_Operation: CreateInstance | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateInstance stores the new instance with every entry of its `Regions` list carrying Status "CREATE_IN_PROGRESS". That value is not a member of `RegionStatus`, which the model declares as exactly `ACTIVE | ADDING | REMOVING` (and whose documentation spells them out: "Valid values are ACTIVE (Region is operational), ADDING (Region extension workflow is in progress), or REMOVING (Region removal workflow is in progress)"). `InstanceMetadata$Regions` repeats it: "The list of Regions enabled in the IAM Identity Center instance, including Regions with ACTIVE, ADDING, or REMOVING status."

Expected: during the window in which the instance itself is `CREATE_IN_PROGRESS` (a legal `InstanceStatus`), each `RegionMetadata$Status` is one of the three modeled `RegionStatus` values — `ADDING` while the Region's enablement is in flight, `ACTIVE` once it is operational. `CREATE_IN_PROGRESS` belongs to `InstanceStatus`, not to `RegionStatus`; the two enums are distinct and only the instance-level member may carry it.

Actual: for `state::INSTANCE_ACTIVATION_SPAN` after the create, both `DescribeInstance` and `ListInstances` report `Regions[0].Status = "CREATE_IN_PROGRESS"`, an unmodeled enum value an SDK deserializes as an unknown variant, so a client that branches on the Region status (e.g. waiting for ACTIVE/ADDING) matches nothing.

## Evidence

Spec: `RegionStatus` enum = [ACTIVE, ADDING, REMOVING] (smithy_query shape RegionStatus / RegionMetadata$Status doc); `InstanceStatus` enum = [CREATE_IN_PROGRESS, CREATE_FAILED, DELETE_IN_PROGRESS, ACTIVE] is a different shape bound only to `DescribeInstance$Status` / `InstanceMetadata$Status`.

Handler/state: `services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/create_instance.rs` builds the record with `crate::state::Instance::creating(...)`, which (src/state.rs, `Instance::creating`, ~lines 572-576) does `instance.status = STATUS_CREATE_IN_PROGRESS` AND then `for region in &mut instance.regions { region.status = STATUS_CREATE_IN_PROGRESS.to_string(); }`. `Instance::materialize_due` later resets both to `ACTIVE`. The projection to the wire is `Region::to_shape` (src/state.rs ~line 435), which copies the stored string straight into `RegionMetadata$Status`.

Test coverage: the current behavior is asserted, not the modeled one — `tests/all/instance_create.rs:255-260` expects `pending["Regions"][0]["Status"] == "CREATE_IN_PROGRESS"`, so fixing the handler requires updating that assertion. Recorded wire data in the earlier sso-admin trace sweep shows real `Regions[0]` as `{'RegionName': 'us-west-2', 'IsPrimaryRegion': True, 'Status': 'ACTIVE', ...}`, i.e. the live service uses RegionStatus values here.

## Steps to Reproduce

1. Boot the emulator with this service mounted and make sure the account holds no instance yet.
2. POST / with X-Amz-Target: SWBExternalService.CreateInstance and body {"Name":"IdentityCenter"}; note the returned InstanceArn.
3. Immediately (within the activation span) call SWBExternalService.DescribeInstance with that InstanceArn.
4. Observe Regions[0].Status == "CREATE_IN_PROGRESS" — not one of the modeled RegionStatus values ACTIVE | ADDING | REMOVING.
5. Call SWBExternalService.ListInstances in the same window and observe the same unmodeled value in Instances[0].Regions[0].Status.
