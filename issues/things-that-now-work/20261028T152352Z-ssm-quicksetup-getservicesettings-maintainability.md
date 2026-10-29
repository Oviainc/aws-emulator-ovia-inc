Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: ea262c1f0c770608da2cb6c8011fa8bda70e1973

# state.rs documents GetServiceSettings as carrying no ServiceSettings for an unconfigured Region, the opposite of what the handler does

_Filed: 2026-10-28T15:23:52Z_
_Operation: GetServiceSettings | Class: maintainability_

> _Note: this issue was identified against the code base at commit `ea262c1f0c770608da2cb6c8011fa8bda70e1973`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The two sides of this crate state opposite rules for the same operation.

`crate::state`'s doc on the `service_settings` field says that a region with no stored record is reported by `GetServiceSettings` "by carrying no `ServiceSettings` at all". The handler does the opposite, and says so in its own comment: it ALWAYS carries the structure, filling in `ServiceSettings::default()` when the region has no record. The handler's behaviour is the one the crate's test pins, and it is the behaviour a FIXED issue (`20261024T213038Z-ssm-quicksetup-getservicesettings-wrong-behavior.md`, "GetServiceSettings omits the ServiceSettings structure entirely for a Region no update has configured") was filed to obtain — so the state-module doc is the stale copy of the rule and it describes precisely the defect that was removed.

That makes the duplicate dangerous rather than merely untidy: the field doc is where a maintainer looks to learn what an absent entry means, and it instructs them to omit the member, which would reintroduce the fixed defect.

The fix is to make the state doc state the rule the handler implements (an unconfigured region still answers the `ServiceSettings` structure, with every member it has nothing for left unset). No response changes and no existing test changes: `an_unconfigured_region_reports_empty_service_settings` and `every_read_reports_a_service_settings_structure` must keep passing exactly as they do.

## Evidence

Side 1 — services/service-com-amazonaws-ssmquicksetup-quicksetup/src/state.rs:1201-1208:
```
    /// The service settings record of each region one has been written for.
    ///
    /// Quick Setup's settings are per-account and per-Region, and this emulator
    /// serves one account, so the region the request was signed for
    /// ([`request_region`]) is the key. A region with no entry has never been
    /// configured, which `GetServiceSettings` reports by carrying no
    /// `ServiceSettings` at all. Bounded by [`MAX_SETTINGS_REGIONS`].
    pub service_settings: BTreeMap<String, ServiceSettingsRecord>,
```

Side 2 — services/service-com-amazonaws-ssmquicksetup-quicksetup/src/ops/get_service_settings.rs:162-168:
```
        Ok(GetServiceSettingsOutput {
            // Always present. A region no update has configured still HAS settings —
            // the model gives the operation no way to report them missing — so its
            // answer is the structure with every member it has nothing for left unset,
            // which is exactly `ServiceSettings::default()`.
            service_settings: Some(stored.unwrap_or_default()),
        })
```

Test coverage that pins side 2 and must stay green — services/service-com-amazonaws-ssmquicksetup-quicksetup/tests/all/service_settings_and_types.rs:423-443:
```
/// A Region that was never configured reports an EMPTY `ServiceSettings` structure —
/// a 200 carrying the structure with `ExplorerEnablingRoleArn` unset, because the
/// operation models no way for settings to be absent — and configuring one Region
/// leaves every other Region unconfigured.
#[tokio::test]
async fn an_unconfigured_region_reports_empty_service_settings() {
...
        assert_eq!(
            reported_settings(region, &answer),
            json!({}),
            "case {region}: an unconfigured Region must report an empty ServiceSettings \
             structure, got {answer:?}"
        );
```
and tests/all/service_settings_and_types.rs:225-229:
```
fn reported_settings(case: &str, answer: &Value) -> Value {
    let settings = answer.get("ServiceSettings").cloned().unwrap_or(Value::Null);
    assert!(
        ...
        "case {case}: expected a ServiceSettings structure in every answer, got {answer:?}"
```

## Steps to Reproduce

1. Read services/service-com-amazonaws-ssmquicksetup-quicksetup/src/state.rs:1201-1208: the `service_settings` field doc says a region with no entry "`GetServiceSettings` reports by carrying no `ServiceSettings` at all".
2. Read services/service-com-amazonaws-ssmquicksetup-quicksetup/src/ops/get_service_settings.rs:162-168: the handler answers `service_settings: Some(stored.unwrap_or_default())`, commented "Always present."
3. Call GET /serviceSettings signed for a region no update has configured: the answer is `{"ServiceSettings":{}}`, i.e. the structure IS carried — the handler's rule, not the state doc's.
4. Note that tests/all/service_settings_and_types.rs:428 `an_unconfigured_region_reports_empty_service_settings` asserts exactly that, and that the fixed issue 20261024T213038Z-ssm-quicksetup-getservicesettings-wrong-behavior.md exists to forbid the omission the state doc still describes.
