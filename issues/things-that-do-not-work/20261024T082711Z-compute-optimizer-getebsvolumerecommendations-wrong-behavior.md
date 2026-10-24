Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a651a577464eaac3ffa428a0c9cc2aa4b0e9484e

# Analyse the account's real EBS volumes instead of a hardcoded in-crate volume inventory

_Filed: 2026-10-24T08:27:11Z_
_Operation: GetEBSVolumeRecommendations | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a651a577464eaac3ffa428a0c9cc2aa4b0e9484e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: volume recommendations describe the EBS volumes that exist in the account/Region, so a client that creates a volume in the EC2 emulator, enrolls, and calls GetEBSVolumeRecommendations sees a recommendation (or a documented absence) for THAT volume ARN, and is not told about volumes that do not exist.

Actual: the handler reports four volumes baked into this crate's own state module — vol-0a1b2c3d4e5f60001 (gp2 500 GiB), ...0002 (gp3 100 GiB), ...0003 (io1 200 GiB) and ...0004 (gp3 50 GiB) — and nothing else. EC2's own volume inventory is never consulted, so a volume the client actually created selects nothing (and, per the handler's own comment, is not even reported in `errors`, which is always returned empty), while the response names four volume ARNs that EC2 does not hold. Every figure is computed from the fixture, so the response is internally consistent but describes resources that do not exist in the emulated account.

The same crate already shows the correct pattern: GetECSServiceRecommendations computes over the real ECS inventory and the real CloudWatch metric store.

## Evidence

Spec: `GetEBSVolumeRecommendations` — "Returns Amazon EBS volume recommendations" for the account's volumes; `VolumeRecommendation$volumeArn`/`currentConfiguration` identify a real volume. Handler: `src/ops/get_ebs_volume_recommendations.rs` lines ~271-313 selects from `guard.visible_volumes()` and always answers `errors: []`. State: `src/state.rs` — `volumes` is filled from `observed_volumes()` (the four hardcoded `ObservedVolume` fixtures at ~line 3668); no operation or sibling service writes it. Contrast: `src/ops/get_ecs_service_recommendations.rs` lines ~340-351 reads `ecs::state::service_state` / `cloudwatch::state::service_state`. Tests: `tests/all/ebs_volume_recommendations.rs` asserts the fixture inventory over the wire; no test creates a volume in EC2 and asks Compute Optimizer about it.

## Steps to Reproduce

1. Boot the emulator with EC2 and Compute Optimizer mounted.
2. Create an EBS volume with EC2 CreateVolume (e.g. 500 GiB gp2) and note its volume id/ARN.
3. Call Compute Optimizer UpdateEnrollmentStatus(status=Active) and wait for the analysis delay.
4. Call GetEBSVolumeRecommendations with volumeArns=[that volume's ARN]: observe HTTP 200 with an empty volumeRecommendations list and an empty errors list.
5. Call GetEBSVolumeRecommendations with no volumeArns: observe recommendations for vol-0a1b2c3d4e5f60001..0004, volumes EC2 does not hold.
