Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3ca5cccf5768308b81885b90d5cd9b0626890960

# ListConfigurations accepts a TypeVersion filter that can never match a stored configuration

_Filed: 2026-10-28T17:21:52Z_
_Operation: ListConfigurations | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3ca5cccf5768308b81885b90d5cd9b0626890960`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListConfigurations` advertises `TypeVersion` as a supported `Filters` key (`crate::state::CONFIGURATION_FILTER_KEYS`) and refuses any key that is not in that list, precisely so that "silently ignoring it would answer a narrowing request with unnarrowed results". But a `TypeVersion` filter is dead on arrival in this crate for two reinforcing reasons:

1. Every version this service can store contains a `.`: `QUICK_SETUP_TYPE_CATALOG` publishes only `N.M` spellings, and `resolve_type_version` stores "the spelling the catalog publishes it under", which `ConfigurationRecord::type_version` then carries. The modelled `FilterValues` member pattern `^[A-Za-z0-9+=@_\/\s-]*$` does NOT admit `.`, so a filter value naming a stored version (`"1.0"`) is refused by the boundary walk with a 400 `ValidationException` and never reaches the handler.
2. The only version spellings the boundary DOES admit (`"1"`, `"2"`) are compared as raw text by `ConfigurationRecord::filter_field` + `matches_filters` (exact string equality), so they match nothing — even though this crate's own version rule says a request naming `2` means the version published as `2.0`.

So every `TypeVersion` filter either 400s or answers 200 with an empty `ConfigurationsList` while configurations deployed at that version exist. This is exactly the condition under which the crate excluded `ManagerArn` from both key lists ("no filter value can ever name one"); the same reason applies to `TypeVersion`, yet it was left in the accepted set.

## Evidence

The key is accepted, with the stated selection rule: `services/service-com-amazonaws-ssmquicksetup-quicksetup/src/state.rs:171-181` — "/// `Filter` keys `ListConfigurations` accepts: the string-valued members of `ConfigurationSummary`, for the same reason as [`MANAGER_FILTER_KEYS`]." … `pub const CONFIGURATION_FILTER_KEYS: &[&str] = &[ "Id", "ConfigurationDefinitionId", "Type", "TypeVersion", "Account", "Region", ];`

The rule it says it follows, state.rs:164-168: "the keys are the listed shape's own string-valued members … whose values the modelled `FilterValues` pattern (`^[A-Za-z0-9+=@_\/\s-]*$`) can express. `ManagerArn` is not among them: an ARN carries `:`, which that pattern refuses, so no filter value can ever name one."

The model confirms the pattern: `smithy_query(query="shape", shape="FilterValues")` -> `"member": {"target": "String", "length": {"max": 256}, "pattern": "^[A-Za-z0-9+=@_\\/\\s-]*$"}` — no `.`.

Every stored version carries a `.`: state.rs:298-322 `QUICK_SETUP_TYPE_CATALOG` entries are `versions: &["1.0", "1.1"]`, `&["1.0", "1.1", "2.0"]`, …; state.rs:365-366 "a request naming a version the type publishes is served that version, in the spelling the catalog publishes it under"; state.rs:1129 `type_version: definition.type_version.clone()` on the deployed configuration.

The match is raw text, contradicting the crate's own version-matching rule: state.rs:1179 `"TypeVersion" => self.type_version.as_deref(),` and state.rs:1571-1572 `field(&filter.key).is_some_and(|value| filter.values.iter().any(|wanted| wanted.as_str() == value))`, versus state.rs:260-262 "Versions are matched by what they NUMBER rather than by their text, so a request naming `2` means the version published as `2.0`".

Test coverage: `tests/all/configuration_managers.rs:869-870` pins the exclusion rule for the analogous key — "// `ManagerArn` is not a filter key on either listing — a filter value cannot even // carry one, since the modelled `FilterValues` pattern refuses `:`." — and asserts a 400 for it on both listings. No test exercises a `TypeVersion` filter on `ListConfigurations` (grep `Filters` under `tests/`: only `Name`, `Description`, `ManagerArn`, `Type`, `Region` appear), so nothing pins the current dead key.

## Steps to Reproduce

1. Create a manager: POST /configurationManager with ConfigurationDefinitions=[{Type: "AWSQuickSetupType-SSMHostMgmt", TypeVersion: "2.0", Parameters: {TargetOrganizationalUnits: "ou-1", TargetRegions: "us-east-1"}}], signed for us-east-1.
2. POST /listConfigurations with {} and observe the row reporting TypeVersion "2.0".
3. POST /listConfigurations with {"Filters": [{"Key": "TypeVersion", "Values": ["2.0"]}]} — the request is refused with 400 ValidationException by the modelled FilterValues pattern (it contains `.`), so the advertised key is unusable with any stored version spelling.
4. POST /listConfigurations with {"Filters": [{"Key": "TypeVersion", "Values": ["2"]}]} — the only admissible spelling of that version — and observe 200 with an empty ConfigurationsList, although the configuration deployed at version 2.0 exists and the crate's own rule treats `2` and `2.0` as one version.
