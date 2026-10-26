Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 465e7599899ea277984bee5e564f3564a11f5f81

# The instance/disk/stack creates accept an availability zone letter no Region publishes, the check their relational-database siblings apply

_Filed: 2026-10-26T05:32:57Z_
_Operation: CreateInstances | Class: validation_

> _Note: this issue was identified against the code base at commit `465e7599899ea277984bee5e564f3564a11f5f81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateInstances resolves `availabilityZone` through `crate::state::region_of_availability_zone`, which only checks that the string is "<a RegionName enum value><one lowercase ASCII letter>". It does NOT check that the Region actually publishes that zone letter, even though the handler's own comment says the zone "must name things this service serves" and CreateDisk's says "the zone must name a zone this service serves".

The crate already owns the stricter resolution and two sibling handlers use it: CreateRelationalDatabase and CreateRelationalDatabaseFromSnapshot call `crate::state::region_spec_of_availability_zone`, which resolves the zone against `REGION_SPECS`' `availability_zone_letters`, with a comment stating the rule ("The zone must be one that Region PUBLISHES (`GetRegions` is where a caller resolves it), so a zone letter no Region has is refused rather than stored").

So one and the same zone string is accepted by one half of the crate and refused by the other: `availabilityZone: "us-east-1z"` (us-east-1 publishes "abcdef") or `"ap-northeast-1b"` (ap-northeast-1 publishes "acd" and "publishes no `b`") is refused by CreateRelationalDatabase and accepted by CreateInstances, CreateInstancesFromSnapshot, CreateDisk, CreateDiskFromSnapshot and CreateCloudFormationStack. The instance/disk is then stored with a `ResourceLocation.availabilityZone` that `GetRegions` never lists, and every later read (GetInstance, GetDisks, the operation log, the snapshot taken of it) reports that zone as if the service served it.

One root cause, five call sites: the five handlers listed above each call the loose helper where the two database handlers call the strict one.

## Evidence

The handler comment states the rule and the call that does not enforce it — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/create_instances.rs:200-230:
```
        // them before this body ran. What is left is what the model cannot
        // express: the blueprint, the bundle and the availability zone must name
        // things this service serves, no name may already be taken, ...
        ...
        let region = crate::state::region_of_availability_zone(&input.availability_zone)
            .ok_or_else(|| {
                invalid(format!(
                    "availabilityZone '{}' is not a zone of a Lightsail region: it must be \
                     a region name followed by a zone letter, for one of [{}]",
```
and the same pair in src/ops/create_disk.rs:180-194: "the zone must name a zone this service serves" above the identical `region_of_availability_zone` call.

What that helper actually checks — src/state.rs:3729-3742:
```
/// The enum, not [`REGION_SPECS`]: this is the set of Region names a stored
/// `ResourceLocation` may carry, which is wider than the Regions `GetRegions`
/// lists as served.
pub fn region_of_availability_zone(availability_zone: &str) -> Option<&'static str> {
    let zone_letter = availability_zone.chars().next_back()?;
    if !zone_letter.is_ascii_lowercase() {
        return None;
    }
    let region = &availability_zone[..availability_zone.len() - 1];
    REGION_NAME_VALUES
        .iter()
        .find(|known| **known == region)
        .copied()
}
```
— the zone LETTER is never compared with the Region's published letters.

The sibling that does enforce it — src/ops/create_relational_database.rs:222-238:
```
        // ... The zone must be one that Region PUBLISHES (`GetRegions`
        // is where a caller resolves it), so a zone letter no Region has is
        // refused rather than stored, ...
        let (region, zones) = match &input.availability_zone {
            Some(zone) => {
                let spec =
                    crate::state::region_spec_of_availability_zone(zone).ok_or_else(|| {
                        invalid(format!(
                            "availabilityZone '{zone}' is not an Availability Zone this service \
                             publishes: it must be one of the zones GetRegions reports for a \
                             region of [{}]",
```
(the same block in src/ops/create_relational_database_from_snapshot.rs:208-221).

The published zone letters the loose check ignores — src/state.rs:3885-3895:
```
/// `availability_zone_letters` is the Availability Zones the Lightsail user
/// guide's "Regions and Availability Zones" table lists for each Region: six in
/// `us-east-1`, four in `us-west-2` (`a` through `d`, the recorded set), two in
/// `ca-central-1`, and `a`, `c`, `d` in `ap-northeast-1`, which publishes no
/// `b`.
```

Test coverage: tests/all/relational_database_lifecycle.rs:965-969 pins the strict behaviour —
```
        Case {
            case: "a zone letter the region does not publish",
            patch: vec![("availabilityZone", json!("us-east-1z"))],
            member: "availabilityZone",
        },
```
while the disk/instance suites only exercise malformed or unknown-region zones — tests/all/disk_block_storage.rs:687-696 (`"us-east-2"`, `"xx-east-9a"`, `"us-east-2A"`, `"anywhere"`), so no test pins acceptance of `us-east-1z` for a disk or an instance.

## Steps to Reproduce

1. Call CreateInstances with instanceNames=["zone-check"], blueprintId="amazon_linux_2", bundleId="nano_3_0" and availabilityZone="us-east-1z" (us-east-1 publishes only a-f).
2. Observe HTTP 200 and an instance created.
3. Call GetInstance for "zone-check" and observe location.availabilityZone = "us-east-1z", a zone GetRegions never lists for us-east-1.
4. Call CreateRelationalDatabase with the same availabilityZone="us-east-1z" and observe HTTP 400 InvalidInputException: "availabilityZone 'us-east-1z' is not an Availability Zone this service publishes".
5. Repeat step 1 against CreateDisk (availabilityZone="ap-northeast-1b", a letter that Region does not publish) and observe the same acceptance.
