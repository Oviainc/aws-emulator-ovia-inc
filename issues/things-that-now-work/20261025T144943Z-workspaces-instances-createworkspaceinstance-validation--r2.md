Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 6896ee08cd462b3fd316541ed134a680dfe1fe29

# CreateWorkspaceInstance accepts a Placement.AvailabilityZone that is not a zone of the request's Region, leaving the instance permanently unattachable

_Filed: 2026-10-25T14:49:43Z_
_Operation: CreateWorkspaceInstance | Class: validation_

> _Note: this issue was identified against the code base at commit `6896ee08cd462b3fd316541ed134a680dfe1fe29`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateWorkspaceInstance` stores `ManagedInstance.Placement.AvailabilityZone` verbatim and never resolves it against the Region the create was signed for, although this crate already owns the helper that answers exactly that question (`crate::state::availability_zones_in_region`) and the sibling create `CreateVolume` calls it to refuse a zone the Region does not publish.

The modelled `Placement$AvailabilityZone` pattern `^[a-z]{2}-[a-z]+-\d(?:[a-z]|-[a-z]+-\d[a-z])$` admits any Region's zone name, so a request signed for `us-east-1` may name `eu-west-1a` (or `us-east-1z`, a letter `us-east-1` does not publish — the very letters `availability_zone_catalog.rs` refuses for a volume). The handler accepts it and `validate_launch_configuration` inspects only `ImageId`, `InstanceType`, the subnet duplication and the tenancy/billing combination — never the zone.

The stored zone is not inert: `AssociateVolume` reads it back through `WorkspaceInstanceRecord::placement_availability_zone()` and compares it with the volume's zone via `zones_allow_attachment`. Because `CreateVolume` refuses any `AvailabilityZone` outside `availability_zones_in_region(region)`, no volume in that Region can ever carry the instance's foreign zone, so every attach to such an instance is refused with `ValidationException` naming `VolumeId` — blaming the volume member for a placement the create should have refused, and leaving the instance with no reachable attach path at all. The instance is accepted into a state no later call can repair (nothing can change a placement or a volume's zone).

The two sides of the same zone rule are therefore enforced on one create and not the other.

## Evidence

The sibling create refuses a zone the Region does not publish, through the crate's own helper — `src/ops/create_volume.rs:165`:
```rust
    if !availability_zones_in_region(region)
        .iter()
        .any(|zone| zone == &input.availability_zone)
    {
        return Err(field_validation_failed(
            "AvailabilityZone",
            "AvailabilityZone must name an Availability Zone of the Region this request \
             was signed for",
        ));
```

The helper exists for both — `src/state.rs:1253`:
```rust
pub fn availability_zones_in_region(region: &str) -> Vec<String> {
    availability_zone_letters(region)
        .chars()
        .map(|letter| format!("{region}{letter}"))
        .collect()
}
```

`CreateWorkspaceInstance`'s own cross-field validator never reads the zone — `src/ops/create_workspace_instance.rs:159` through `:238` check `ImageId`, `InstanceType`, `SubnetId` duplication, `Placement.Tenancy` and `BillingConfiguration.BillingMode` only; the only use of `placement` is `src/ops/create_workspace_instance.rs:208`:
```rust
    let tenancy = configuration_tenancy(
        managed_instance
            .placement
            .as_ref()
            .and_then(|placement| placement.tenancy.as_deref())
            .unwrap_or(PLACEMENT_TENANCY_DEFAULT),
    );
```

The unvalidated value is what decides every later attach — `src/ops/associate_volume.rs:191`:
```rust
        let instance_zone = instance
            .placement_availability_zone()
            .map(ToString::to_string);
```
and `src/ops/associate_volume.rs:212`:
```rust
        let zones_allow_attachment = crate::state::zones_allow_attachment(
            instance_zone.as_deref(),
            &volume.availability_zone,
        );
```
with `src/state.rs:2285`: `instance_zone.is_none_or(|zone| zone == volume_zone)` — equality only, so a foreign zone can never match.

No test pins the acceptance: `tests/all/instance_configuration_validation.rs:10-16` states the rule it covers ("no image or no instance type, an instance type the service's own catalog does not publish, a tenancy or billing mode that type publishes no supported configuration for, or a subnet named both on the instance and on its network interfaces") — the zone is absent from it, and every placement in `tests/all/volume_zone_affinity.rs:53` and `tests/all/availability_zone_catalog.rs:185` is a zone drawn from `availability_zones_in_region(region)`. `tests/all/availability_zone_catalog.rs:224` (`a_letter_a_region_does_not_publish_is_refused`) applies the refusal only to `CreateVolume`.

## Steps to Reproduce

1. Sign a request for us-east-1 and call CreateWorkspaceInstance with {"ManagedInstance":{"ImageId":"ami-0123456789abcdef0","InstanceType":"t3.medium","Placement":{"AvailabilityZone":"eu-west-1a"}}} — the zone satisfies the modelled Placement$AvailabilityZone pattern but is not a zone us-east-1 publishes. Observe HTTP 200 and a minted wsinst- id (CreateVolume with the same AvailabilityZone in us-east-1 is refused with HTTP 400 ValidationException naming AvailabilityZone).
2. In the same Region call CreateVolume with {"AvailabilityZone":"us-east-1a","SizeInGB":4} (any zone us-east-1 publishes — no other zone is accepted).
3. Call AssociateVolume {"WorkspaceInstanceId":<the id>,"VolumeId":<the volume>,"Device":"/dev/sdf"}. Observe HTTP 400 ValidationException naming VolumeId with VOLUME_ZONE_MISMATCH_MESSAGE, for every volume the Region can hold: the instance accepted at step 1 can never have a volume attached.
