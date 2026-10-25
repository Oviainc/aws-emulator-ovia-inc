Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 422492f47e5217da15117465faf05f5c44507550

# CreateVolume rejects valid Availability Zones beyond the third letter of a Region

_Filed: 2026-10-24T19:54:39Z_
_Operation: CreateVolume | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `422492f47e5217da15117465faf05f5c44507550`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`validate_configuration` accepts `AvailabilityZone` only when it is one of `<region>a`, `<region>b`, `<region>c` (`state::AVAILABILITY_ZONE_LETTERS = ['a','b','c']`, used by `state::availability_zones_in_region`). Real AWS Regions have more zones than three — `us-east-1` has `us-east-1a` through `us-east-1f`, `us-west-2` has `us-west-2a`..`us-west-2d`, `ap-northeast-1` has `ap-northeast-1a`, `-1c`, `-1d` — and callers routinely pass the zone of the subnet their WorkSpace Instance sits in.

Expected: `CreateVolume` signed for `us-east-1` with `AvailabilityZone: "us-east-1d"` creates the volume and answers 200 with a `vol-…` id. Actual: HTTP 400 `ValidationException` (`FIELD_VALIDATION_FAILED`, FieldList naming `AvailabilityZone`) and no volume stored, so every follow-on call (AssociateVolume, DeleteVolume) is unreachable for that zone. Note `ap-northeast-1b` does not exist in real AWS yet is accepted here, so the zone set is wrong in both directions.

## Evidence

Well-established AWS fact: the Availability Zone names a Region has are Region-specific and are not limited to the first three letters (us-east-1a..us-east-1f; ap-northeast-1 has no `b`). Model: `CreateVolumeRequest$AvailabilityZone` is a free `String64` (len 0..64) with no enumeration, and `CreateVolume` declares ValidationException (400) only for genuinely invalid input. Handler: src/ops/create_volume.rs::validate_configuration -> `availability_zones_in_region(region)`; src/state.rs AVAILABILITY_ZONE_LETTERS (3 letters, derived for every Region). Tests: tests/all/volume_configuration_validation.rs::the_availability_zone_must_be_one_of_the_signed_regions_zones pins the three-letter set, so the divergence is asserted rather than caught.

## Steps to Reproduce

1. POST / with X-Amz-Target: EUCMIFrontendAPIService.CreateVolume, credentials scoped to us-east-1, body {"AvailabilityZone": "us-east-1d", "SizeInGB": 8, "VolumeType": "gp3"}
2. Observe HTTP 400 ValidationException naming AvailabilityZone; real AWS answers 200 with a VolumeId because us-east-1d is a real zone of us-east-1
3. Repeat with {"AvailabilityZone": "ap-northeast-1b"} signed for ap-northeast-1 — accepted here although that zone does not exist
