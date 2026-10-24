Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a

# The published Quick Setup type catalog reports a synthetic LatestVersion of 1.0 for every type

_Filed: 2026-10-24T15:59:09Z_
_Operation: ListQuickSetupTypes | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListQuickSetupTypes` answers from a hardcoded catalog in which all twelve `AWSQuickSetupType-*` entries report the same `LatestVersion`, the invented constant `"1.0"` (`crate::state::CATALOG_LATEST_VERSION`). `QuickSetupTypeOutput$LatestVersion` is documented as "The latest version number of the configuration", i.e. the version of the service-owned template the service would deploy for that type — a per-type value that the real service advances independently as it ships new configuration versions, so the real response carries DIFFERENT and (for most types) higher versions per entry.

The synthetic version is also inert: nothing ties a configuration definition's `TypeVersion` back to the catalog. A `CreateConfigurationManager` or `UpdateConfigurationDefinition` naming `TypeVersion: "99.99"` — a version the catalog says does not exist for that type — is accepted and stored (only the 1..128 length the model declares is enforced), and a definition created without `TypeVersion` is stored and reported with none at all, so a caller cannot learn which version was deployed. The handler already refuses a `Type` that is not in this catalog, so the catalog is the declared authority for types but never for versions.

Expected: `LatestVersion` values matching the versions the real service publishes per type (and a `TypeVersion` the service resolves/validates against them). Actual: `"1.0"` for all twelve entries, and arbitrary unpublished `TypeVersion` values accepted elsewhere.

## Evidence

Spec grounding: `smithy_query(query="shape", shape="QuickSetupTypeOutput")` — `LatestVersion` is "The latest version number of the configuration", a per-type value; `ConfigurationDefinition$TypeVersion` is "The version of the Quick Setup type used" (length 1..128, no enumeration), which only means something against the published catalog. Well-established service behavior: Quick Setup's configuration types are versioned independently and their latest versions have advanced past the initial release.

Handler evidence: `src/state.rs:147-201` — `CATALOG_LATEST_VERSION: &str = "1.0"` is used as `latest_version` for all twelve `QUICK_SETUP_TYPE_CATALOG` entries; `src/ops/list_quick_setup_types.rs:140-150` maps that constant straight onto the response. `src/ops/create_configuration_manager.rs:282-316` stores `definition.type_version` verbatim (no catalog check, no default) and `src/ops/update_configuration_definition.rs:274-276` likewise.

Test coverage: `tests/all/service_settings_and_types.rs:222` (`the_quick_setup_type_catalog_is_reported_in_full_whatever_the_request_and_store_hold`) only asserts the `Type` pattern and that the version is 1..128 characters long (`assert_version_length`), so no test pins a version value; `tests/all/configuration_managers.rs:384-398` even creates a definition with `TypeVersion: "1.1"`, a version the catalog does not publish, and asserts it is accepted and echoed back.

## Steps to Reproduce

1. GET /listQuickSetupTypes.
2. Observe every entry of `QuickSetupTypeList` carries `"LatestVersion": "1.0"` — the same invented version for all twelve types, rather than the per-type versions the real service publishes.
3. POST /configurationManager with {"ConfigurationDefinitions":[{"Type":"AWSQuickSetupType-SSMHostMgmt","TypeVersion":"99.99","Parameters":{"TargetOrganizationalUnits":"ou-1","TargetRegions":"us-west-2"}}]} — accepted with 200 even though the catalog publishes no such version for that type.
4. GET /configurationManager/{ManagerArn}: the stored definition reports `TypeVersion: "99.99"`. Repeat the create without `TypeVersion` and the definition (and the configuration deployed from it) reports no version at all.
