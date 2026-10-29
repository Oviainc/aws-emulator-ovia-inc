Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a

# App monitors are not scoped to the request's region: one global name index shared by every region

_Filed: 2026-10-24T15:51:59Z_
_Operation: CreateAppMonitor | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CloudWatch RUM is a regional service: an app monitor lives in the region the request was signed for, its ARN carries that region (`arn:aws:rum:<region>:<account>:appmonitor/<name>`), and a name only has to be unique WITHIN a region. The emulator keys its one `app_monitors` map by `Name` alone (`ComAmazonawsRumRumState.app_monitors: BTreeMap<String, AppMonitorRecord>` in `src/state.rs`), with no region component in the key and no region filter on any reader, even though `CreateAppMonitor` reads the signed region to build the ARN it stores.

Expected (AWS): `CreateAppMonitor(Name="shop")` signed for us-east-1 and the same call signed for eu-west-2 both succeed and create two independent monitors; `GetAppMonitor(Name="shop")` / `ListAppMonitors` signed for eu-west-2 report only the eu-west-2 monitor; a monitor created in us-east-1 is NOT visible from eu-west-2 (`ResourceNotFoundException`).

Actual (emulator): the second create answers `ConflictException` 409 ("already exists") because the name is globally unique; `GetAppMonitor` and `ListAppMonitors` signed for ANY region report every monitor in the store, each carrying the ARN of the region it was created in. This is also internally inconsistent: the ARN-addressed tagging operations resolve a monitor by its FULL stored ARN (`app_monitor_by_arn`), so they only accept the create-region ARN, while the `Name`-addressed operations ignore region entirely — the same monitor is simultaneously "in" one region and "in" all of them.

## Evidence

Spec/semantics: the model's `Arn`/app-monitor ARN pattern includes the region segment, and the emulator itself derives the ARN from the SigV4 credential scope (`crate::state::request_region` in `src/ops/create_app_monitor.rs`, asserted by `tests/all/app_monitor_lifecycle.rs::an_arn_is_derived_from_the_region_the_request_was_signed_for`), so the service is region-aware for naming but not for identity. Handler/state: `src/state.rs` (`app_monitors: BTreeMap<String, AppMonitorRecord>`, `monitor_ids: HashMap<String, String>`) has no region key; `src/ops/create_app_monitor.rs` checks `store.app_monitors.contains_key(&input.name)` globally; `src/ops/list_app_monitors.rs` ranges over the whole map; `src/ops/get_app_monitor.rs` looks up by name only. Test coverage: no test creates in two regions and asserts isolation — the region test builds a FRESH emulator per region, so cross-region leakage is unexercised. Sibling emulators in this repo do implement per-region scoping (e.g. kms `an_alias_is_unique_per_region_and_visible_only_there`, kafka `a_configuration_name_is_unique_per_region`, eventbridge `a_bus_name_is_unique_per_region_and_a_listing_is_scoped_to_one`).

## Steps to Reproduce

1. Boot the emulator and point a rum client signed for us-east-1 at it.
2. CreateAppMonitor(Name='shop', Domain='example.com') signed for us-east-1 -> 200 with an Id.
3. CreateAppMonitor(Name='shop', Domain='example.com') signed for eu-west-2 -> emulator answers ConflictException 409; AWS creates a second, independent eu-west-2 monitor.
4. GetAppMonitor(Name='shop') signed for eu-west-2 -> emulator returns the us-east-1 monitor (its AppMonitor.Id and an ARN naming us-east-1); AWS answers ResourceNotFoundException.
5. ListAppMonitors signed for ap-southeast-1 -> emulator lists every monitor in the store regardless of the region it was created in; AWS lists only ap-southeast-1 monitors.
6. ListTagsForResource with the us-east-1 ARN, signed for eu-west-2 -> succeeds, showing the ARN-addressed and Name-addressed resolutions disagree about which region owns the monitor.
