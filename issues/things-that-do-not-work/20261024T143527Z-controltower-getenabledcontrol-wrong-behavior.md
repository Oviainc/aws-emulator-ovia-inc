Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# Report an enabled control's targetRegions from the landing zone's current governed regions, not a snapshot taken at enable time

_Filed: 2026-10-24T14:35:27Z_
_Operation: GetEnabledControl | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

An enabled control's `targetRegions` are captured ONCE, when `EnableControl` runs, and are never recomputed afterwards. If the landing zone's governed regions change (an `UpdateLandingZone` whose manifest carries a different `governedRegions` list), `GetEnabledControl` (and `ListEnabledControls`, which reads the same record) keeps reporting the region list that happened to be governed at enable time.

Expected (real AWS): a control is deployed into the Regions the landing zone governs; adding a Region to governance extends the already-enabled controls into it, and removing a Region withdraws them, so `EnabledControlDetails$targetRegions` ("Target Amazon Web Services Regions for the enabled control") tracks the landing zone's current governed-region set. A caller who creates a landing zone governing [us-west-2], enables a control, then updates the landing zone to govern [us-west-2, eu-west-1] sees both Regions reported for that enabled control once the update completes.

Actual (emulator): the enablement stores the snapshot and nothing re-derives it, so the control is reported as deployed only into us-west-2 forever. The reverse case is worse: a control enabled with NO landing zone deployed stores the request's signing region, and that fabricated single-region list survives a later `CreateLandingZone` that governs an entirely different set of Regions.

## Evidence

Smithy model: `EnabledControlDetails$targetRegions` (target `TargetRegions`, member of the required `GetEnabledControl` output) is documented "Target Amazon Web Services Regions for the enabled control" — a property of where the control is currently deployed, not of the call that enabled it. The landing zone manifest's `governedRegions` list is what defines the governed Regions (the emulator itself derives target regions from it). Control Tower applying the governed Regions' controls when a Region is added to governance is well-established service behavior.

Handler: services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/enable_control.rs lines 300-333 — `let mut target_regions = store.governed_regions(); if target_regions.is_empty() { target_regions.push(region); }` is written into `EnabledControl { target_regions, .. }` once. src/state.rs `governed_regions()` (around line 972) reads the deployed landing zone's manifest, and `settle()` (src/state.rs lines 784-921) only flips statuses/drift and deploys pending landing zone configuration — it never revisits `enabled_controls[..].target_regions`. src/ops/get_enabled_control.rs lines 208-214 echoes the stored snapshot straight into the response.

Tests: tests/all/enabled_control_lifecycle.rs `target_regions_are_the_governed_regions_of_the_deployed_landing_zone` (lines 926-998) pins only the enable-time snapshot — every case creates the landing zone BEFORE enabling, and no case changes the governed regions afterwards. No test asserts that an update to the landing zone's governedRegions is reflected in an existing enablement's targetRegions. No existing issue covers GetEnabledControl.

## Steps to Reproduce

1. POST /create-landingzone with {"version":"3.3","manifest":{...,"governedRegions":["us-west-2"]}} and wait past the settle span so the landing zone is ACTIVE.
2. POST /enable-control with {"controlIdentifier":"arn:aws:controltower:us-west-2::control/AWS-GR_ENCRYPTED_VOLUMES","targetIdentifier":"arn:aws:organizations::123456789012:ou/o-exampleorg/ou-aa11-1a"} and note the returned enabled-control arn.
3. POST /get-enabled-control with that arn: targetRegions is [{"name":"us-west-2"}].
4. POST /update-landingzone with {"landingZoneIdentifier":"<lz arn>","version":"3.3","manifest":{...,"governedRegions":["us-west-2","eu-west-1"]}} and wait past the settle span, then POST /get-landingzone to confirm the new governedRegions are deployed.
5. POST /get-enabled-control with the enabled-control arn again: the emulator still reports only [{"name":"us-west-2"}]; AWS reports the control deployed into both governed Regions. The mirror case: enable a control on a fresh emulator with NO landing zone (targetRegions becomes the signing region), then create a landing zone governing eu-central-1 — the enablement still reports the signing region.
