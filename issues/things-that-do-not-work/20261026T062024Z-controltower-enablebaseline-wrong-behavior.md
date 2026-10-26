Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: adb8f986b66b684ed38690a8bd355d4f46d0ebf5

# EnableBaseline's one-baseline-per-target rule compares raw baselineIdentifier strings, so one baseline can be enabled twice on one target

_Filed: 2026-10-26T06:20:24Z_
_Operation: EnableBaseline | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `adb8f986b66b684ed38690a8bd355d4f46d0ebf5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`EnableBaseline` refuses a second enablement of the same baseline/target pair by calling `enabled_baseline_on_target`, which compares the caller's raw `baselineIdentifier` string with `==`; the handler also persists that raw string. But `resolve_baseline` — the function that decided the request names a real baseline — states and implements the opposite rule: a baseline's identity is ONLY the 16-character resource id, and the partition/region/account segments "select nothing and are not compared". `is_baseline_arn` accordingly accepts any `[a-z-]+` partition, any `[a-z0-9-]*` region (the empty string included) and an account segment of 0 to 12 digits.

Consequence: many distinct strings name the same catalog baseline, and a second `EnableBaseline` with any of them on the same target passes the duplicate check and creates a SECOND `EnabledBaseline` resource for one baseline on one target — the exact state the handler's comment says must not exist ("a second enable of the same pair would leave two `EnabledBaseline` resources describing one target"). The two enablements then consume two slots of `MAX_ENABLED_BASELINES`, produce two independent baseline operation lifecycles, and must each be torn down separately by `DisableBaseline`.

The same raw-string comparison also makes `ListEnabledBaselines`'s `filter.baselineIdentifiers` fail to select an enablement created with an equivalent-but-differently-written baseline ARN, because the stored identifier is never normalised to the catalog entry it resolved to.

The fix is to key the uniqueness check (and ideally the stored identifier) on the resolved catalog baseline — which the handler already holds as `baseline` — rather than on the request string.

## Evidence

The crate says the pre-resource segments carry no identity:

- `services/service-com-amazonaws-controltower-awscontroltowerapis/src/state.rs:2022-2026` (doc of `resolve_baseline`): "A baseline's identity is the 16-character resource id after `baseline/`: the same catalog is published in every partition, region and account, so the segments before it select nothing and are not compared beyond being the ones a baseline ARN carries ([`is_baseline_arn`]). The id itself is matched exactly"
- `src/state.rs:2058-2078` (`is_baseline_arn`): accepts `partition.bytes().all(|byte| byte.is_ascii_lowercase() || byte == b'-')`, `region` of `[a-z0-9-]*` (empty allowed) and `account.len() <= 12 && account.bytes().all(..is_ascii_digit)` (empty allowed) — so `arn:aws:controltower:us-west-2::baseline/<ID>`, `arn:aws:controltower:us-east-1::baseline/<ID>`, `arn:aws:controltower::123456789012:baseline/<ID>` and `arn:aws:controltower:::baseline/<ID>` all resolve to ONE catalog entry.

The crate says the pair must be unique:

- `src/ops/enable_baseline.rs:262-266`: "// One baseline per target: the pair is what identifies an enablement, so a second // enable of the same pair would leave two `EnabledBaseline` resources describing // one target. The rule is read off the store, so it holds whichever call enabled // the existing one."
- `src/state.rs:1448-1450` (doc of `enabled_baseline_on_target`): "The ARN of the enabled baseline that already applies `baseline_identifier` to `target_identifier`, or `None` when that pair is not enabled."

But the check is raw string equality, and the raw string is what gets stored:

- `src/state.rs:1458-1463`: `.filter(|enabled| { enabled.baseline_identifier == baseline_identifier && enabled.target_identifier == target_identifier })`
- `src/ops/enable_baseline.rs:267-269`: `if let Some(existing) = store.enabled_baseline_on_target(&input.baseline_identifier, &input.target_identifier)`
- `src/ops/enable_baseline.rs:290-297`: `store.record_enablement(&arn, &input.baseline_identifier, ...)` — the request string is persisted, not `baseline.id`.

Test coverage does not reach it: every enable in `tests/all/enable_baseline_validation.rs` builds the identifier through the single helper at lines 31-33, `format!("arn:aws:controltower:us-west-2::baseline/{baseline_id}")`, so a repeat written with another region/account/partition segment is asserted nowhere.

## Steps to Reproduce

1. POST /enable-baseline with {"baselineIdentifier":"arn:aws:controltower:us-west-2::baseline/17BSJV3IGJ2QSGA2","baselineVersion":"4.0","targetIdentifier":"arn:aws:organizations::000000000000:ou/o-emulatororg/ou-abcd-1111"} — 200, note the returned `arn`.
2. Wait past the settle span (OPERATION_SETTLE_AFTER, 500ms).
3. POST /enable-baseline again with the SAME target and baselineVersion but {"baselineIdentifier":"arn:aws:controltower:us-east-1::baseline/17BSJV3IGJ2QSGA2"} (or "arn:aws:controltower:::baseline/17BSJV3IGJ2QSGA2") — the same catalog baseline per state.rs:2022-2026.
4. Expected per the handler's own rule (enable_baseline.rs:262-279): ConflictException. Actual: 200 with a SECOND, different EnabledBaseline ARN.
5. POST /list-enabled-baselines with {} — two entries report the same `baselineIdentifier` resource id on the same `targetIdentifier`.
6. POST /list-enabled-baselines with {"filter":{"baselineIdentifiers":["arn:aws:controltower:us-west-2::baseline/17BSJV3IGJ2QSGA2"]}} — only the first enablement is selected; the duplicate is invisible to the filter.
