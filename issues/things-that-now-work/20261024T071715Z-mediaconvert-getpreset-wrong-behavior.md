Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# The account carries no built-in SYSTEM presets, so GetPreset 404s for every AWS-published preset

_Filed: 2026-10-24T07:17:15Z_
_Operation: GetPreset | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every MediaConvert account has a catalogue of built-in (Type SYSTEM) output presets that no caller created — the counterpart of the built-in `Default` queue and the built-in system job templates this emulator already seeds. On the real service `GetPreset` for one of those published names (e.g. 'System-Generic_Hd_Mp4_Avc_Aac_16x9_1920x1080p_24Hz_6Mbps', 'System-Ott_Hls_Ts_Avc_Aac_16x9_1280x720p_30Hz_3.5Mbps') answers HTTP 200 with a Preset whose `Type` is SYSTEM, and those presets cannot be updated or deleted (403).

Expected: a fresh account's preset store holds a set of SYSTEM presets, so GetPreset on one of them returns 200 with Type SYSTEM, UpdatePreset/DeletePreset on one is refused with ForbiddenException, and a job that names a built-in preset in an output resolves.

Actual: `ComAmazonawsMediaconvertMediaconvertState::default()` initialises `presets: BTreeMap::new()` (src/state.rs), and only CreatePreset ever inserts (always with Type CUSTOM). No SYSTEM preset exists in any emulated account, so GetPreset on every AWS-published preset name is a 404 and the SYSTEM branches of DeletePreset/UpdatePreset are dead code (the delete handler's own comment admits: 'That second branch is reachable only once the store seeds built-in presets'). A client that builds jobs from AWS's built-in presets — the normal starting point for MediaConvert users — cannot resolve any of them.

## Evidence

Spec: `Preset$Type` -> `Type` (SYSTEM | CUSTOM) with the doc 'A preset can be of two types: system or custom. System or built-in presets can't be modified or deleted by the user'; `PresetListBy` declares a `SYSTEM` value for listing exactly those built-ins. Well-established service behaviour: MediaConvert publishes system presets in every account and region (the console's 'System presets' tab, the System-* names). Precedent in this repo: the seeded built-in `Default` SYSTEM queue was itself an accepted gap (issue 20261022T040904Z-...-4-fidelity-divergences, cause 3), and src/state.rs already seeds `SYSTEM_JOB_TEMPLATES` for the template plane while leaving the preset plane empty. Handler/state: src/state.rs `fn default()` -> `presets: BTreeMap::new()`; ops/get_preset.rs returns not_found_message("name") for any name the store lacks. Test coverage: no test asserts a SYSTEM preset exists; tests/all/preset_lifecycle.rs line ~459 and preset_deletion.rs line ~293 only use invented System-like names ('System-Ultra-HD', 'System-Generic-Hd-Mp4-Avc-Aac-16x9') as examples of ABSENT names, so no test pins real built-in preset names. (The sibling gap for job templates — SYSTEM templates reported with an empty Settings graph — is open as 20261024T010653Z-mediaconvert-listjobtemplates-wrong-behavior and concerns templates, not presets.)

## Steps to Reproduce

1. Boot the emulator and point a botocore mediaconvert client at it; do not create anything.
2. get_preset(Name='System-Generic_Hd_Mp4_Avc_Aac_16x9_1920x1080p_24Hz_6Mbps') -> expect HTTP 200 with Preset.Type == 'SYSTEM' and its encoder settings; observe NotFoundException (404).
3. delete_preset(Name=<that same system preset>) -> expect ForbiddenException (403, 'not allowed to delete system resources'); observe NotFoundException (404).
4. update_preset(Name=<that same system preset>, Description='x') -> expect ForbiddenException (403); observe NotFoundException (404).
