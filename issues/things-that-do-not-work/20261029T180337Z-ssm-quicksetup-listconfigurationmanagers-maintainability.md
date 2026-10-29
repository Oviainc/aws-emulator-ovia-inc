Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# state.rs justifies MANAGER_FILTER_KEYS with a rule its own Name and Description keys break

_Filed: 2026-10-29T18:03:37Z_
_Operation: ListConfigurationManagers | Class: maintainability_

> _Note: this issue was identified against the code base at commit `dfc2bec1feea4e1eda9981ef982ae4c91760421c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The doc comment over `MANAGER_FILTER_KEYS` states the rule the crate used to pick which `Filter` keys `ListConfigurationManagers` accepts, and applies that rule to exclude one key: a key qualifies when it is a string-valued member of `ConfigurationManagerSummary` "whose values the modelled `FilterValues` pattern ... can express", and `ManagerArn` is excluded because an ARN carries `:`, which that pattern refuses.

The rule as written does not select the list it sits above. `ConfigurationManagerSummary` has exactly three string-valued members (`ManagerArn`, `Name`, `Description`), and two of the three the list DOES accept carry values that same pattern cannot express:

* `Name`'s modelled pattern is `^[ A-Za-z0-9._-]{0,120}$` — it admits `.`, which `FilterValues` does not;
* `Description`'s modelled pattern is `^.{0,512}$` — every character, so `:`/`,`/`.` and anything else, none of which `FilterValues` admits.

So by the stated premise `Description` would have to be excluded exactly as `ManagerArn` is (both carry values that can hold `:`), yet it is accepted. The comment is the one place a future change reads to decide whether a key belongs in either list (`CONFIGURATION_FILTER_KEYS` is defined immediately below and says "for the same reason as [`MANAGER_FILTER_KEYS`]"), so the next key added or removed is decided from a premise that is false for two of the three members already there, and a reader cannot tell why `:` in an ARN disqualifies a key while `:` in a `Description` does not.

The defect is in the stated rule, not in the key list: the fix is to word the rule as what the code actually does (ARNs are excluded because EVERY ARN carries `:`, so no filter value can ever name one, while `Name`/`Description` are accepted for the values a filter value CAN express), leaving `MANAGER_FILTER_KEYS`, every response and every existing test byte-for-byte unchanged.

## Evidence

The rule and the exclusion it justifies — services/service-com-amazonaws-ssmquicksetup-quicksetup/src/state.rs:162-169:
```
/// `Filter` keys `ListConfigurationManagers` accepts.
///
/// The model documents no key catalog, so the keys are the listed shape's own
/// string-valued members — `ConfigurationManagerSummary` — whose values the modelled
/// `FilterValues` pattern (`^[A-Za-z0-9+=@_\/\s-]*$`) can express. `ManagerArn` is
/// not among them: an ARN carries `:`, which that pattern refuses, so no filter value
/// can ever name one.
pub const MANAGER_FILTER_KEYS: &[&str] = &["Name", "Description"];
```
The sibling list that inherits the same justification — state.rs:171-173: "`Filter` keys `ListConfigurations` accepts: the string-valued members of `ConfigurationSummary`, for the same reason as [`MANAGER_FILTER_KEYS`]."

The model refutes the premise for the two accepted keys. `smithy_query(query="shape", shape="FilterValues")` -> `"member": {"target": "String", "length": {"max": 256}, "pattern": "^[A-Za-z0-9+=@_\\/\\s-]*$"}` (no `.`, no `:`, no `,`), while `smithy_query(query="operation", operation="CreateConfigurationManager")` -> `"Name": {"target": "String", "pattern": "^[ A-Za-z0-9._-]{0,120}$"}` and `"Description": {"target": "String", "pattern": "^.{0,512}$"}` — the same `smithy_query(query="operation", operation="UpdateConfigurationManager")` repeats both patterns for the update path, and `ConfigurationManagerSummary` reports these two plus `ManagerArn` as its only string-valued members (`smithy_query(query="shape", shape="ConfigurationManagerSummary")`).

Test coverage that must stay unchanged: tests/all/configuration_managers.rs:855-882 exercises `Name` and `Description` filters and asserts the matching managers come back 200, and tests/all/configuration_managers.rs:884-885 pins the exclusion this comment justifies — "// `ManagerArn` is not a filter key on either listing — a filter value cannot even // carry one, since the modelled `FilterValues` pattern refuses `:`." — asserting the refusal on both listings. A comment-only correction leaves both intact.

## Steps to Reproduce

1. Read services/service-com-amazonaws-ssmquicksetup-quicksetup/src/state.rs:162-169: the accepted-key rule is "the listed shape's own string-valued members ... whose values the modelled `FilterValues` pattern (`^[A-Za-z0-9+=@_\/\s-]*$`) can express", and `ManagerArn` is excluded by it because an ARN carries `:`.
2. Ask the model for the patterns of the two members the list accepts: `smithy_query(query="operation", operation="CreateConfigurationManager")` reports `Name` as `^[ A-Za-z0-9._-]{0,120}$` and `Description` as `^.{0,512}$`.
3. Observe that `^.{0,512}$` admits `:` — the very character the comment uses to disqualify `ManagerArn` — and that `^[ A-Za-z0-9._-]{0,120}$` admits `.`, which `FilterValues` also refuses, so the stated rule excludes `Description` and `Name` as surely as it excludes `ManagerArn`.
4. Confirm the premise is observably false: POST /configurationManager with `{"Name": "demo.prod", "Description": "phase 2, rollout.", "ConfigurationDefinitions": [{"Type": "AWSQuickSetupType-SSMOpsCenter", "Parameters": {"TargetOrganizationalUnits": "ou-1", "TargetRegions": "us-east-1"}}]}` (both values are inside their modelled patterns, so the manager is created), then POST /listConfigurationManagers with `{"Filters": [{"Key": "Name", "Values": ["demo.prod"]}]}` — the modelled `FilterValues` pattern refuses the value at the boundary, so the stored `Name` cannot be expressed by the filter the comment says it can.
