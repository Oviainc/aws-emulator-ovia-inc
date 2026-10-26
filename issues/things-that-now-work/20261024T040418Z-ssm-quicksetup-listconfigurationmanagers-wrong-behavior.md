Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5b1d79d167dfa72d8fff60e6631ca11c09e39e94

# Scope the configuration-manager store to the Region the request is signed for

_Filed: 2026-10-24T04:04:18Z_
_Operation: ListConfigurationManagers | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5b1d79d167dfa72d8fff60e6631ca11c09e39e94`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Quick Setup is a regional control plane: `ssm-quicksetup` has a per-Region endpoint (`ssm-quicksetup.<region>.amazonaws.com`) and a configuration manager's ARN embeds the Region it was created in (`arn:aws:ssm-quicksetup:<region>:<account>:configuration-manager/<uuid>`). A manager created through the us-west-2 endpoint is therefore NOT returned by a `ListConfigurationManagers` call made against eu-west-1, and the configurations it owns are not returned by `ListConfigurations` there either.

The emulator holds one global, Region-less store. `crate::state::request_region(ctx.parts())` is consulted in exactly three places — `CreateConfigurationManager` (to mint the ARN and stamp `ConfigurationRecord.region`/`account`), `UpdateServiceSettings` and `GetServiceSettings` (which correctly key the settings record by Region). Every other handler ignores the signed Region entirely:

* `ListConfigurationManagers` walks `store.managers` in full, so a request signed for Region B reports every manager created in Region A (their `ManagerArn` carrying Region A).
* `ListConfigurations` walks `store.configurations` in full, so it reports configuration summaries whose `Region` member is a Region other than the one the caller asked.
* `GetConfigurationManager`, `UpdateConfigurationManager` and `DeleteConfigurationManager` look the ARN up by key only, so a manager minted in Region A can be read, renamed and deleted through Region B's endpoint, where the real service has no such resource.

Expected: a manager (and its configurations) is visible and mutable only through the Region whose endpoint created it; a listing signed for another Region reports an empty `ConfigurationManagersList` / `ConfigurationsList` (and no `NextToken`), and an ARN belonging to another Region is not found by the per-ARN operations.
Actual: the store is shared across all Regions, so every Region sees and can mutate every other Region's managers.

The same crate already demonstrates the intended rule for its other resource (`service_settings: BTreeMap<String /* region */, ServiceSettingsRecord>` plus the test `an_unconfigured_region_reports_no_service_settings`), so the manager/configuration slice is inconsistent with it.

## Evidence

Spec/wire grounding: the Smithy model's `aws.api#service` endpointPrefix/ARN namespace is `ssm-quicksetup` (regional endpoint) and `ListConfigurationsInput$ManagerArn` carries the pattern `^arn:aws:ssm-quicksetup:([^:]+):(\d{12}):configuration-manager/<uuid>$`, i.e. the Region is part of the resource identity; `ConfigurationSummary$Region` is documented as "The Amazon Web Services Region where the configuration was deployed". Well-established AWS behavior: a regional control-plane resource is not listed by another Region's endpoint.

Handler evidence: `src/state.rs` keys managers as `managers: BTreeMap<String /* ManagerArn */, ConfigurationManagerRecord>` and configurations as `configurations: BTreeMap<String /* Id */, ConfigurationRecord>` with no Region dimension; `grep request_region src/` matches only `create_configuration_manager.rs:239`, `get_service_settings.rs:151` and `update_service_settings.rs:196`. `src/ops/list_configuration_managers.rs:197-206` paginates `store.managers.range(..)` unconditionally; `src/ops/list_configurations.rs:198-230` paginates `store.configurations.range(..)` and only filters on `ManagerArn`/`ConfigurationDefinitionId`/`Filters`; `src/ops/get_configuration_manager.rs:172`, `update_configuration_manager.rs:181` and `delete_configuration_manager.rs:180` do a bare key lookup.

Test coverage: none. `tests/all/configuration_managers.rs` signs every request for a single Region (`const REGION: &str = "us-west-2"`), so no case exercises a cross-Region read; `tests/all/service_settings_and_types.rs` asserts Region isolation only for the service-settings record.

## Steps to Reproduce

1. POST /configurationManager signed for us-west-2 with `{"Name":"west","ConfigurationDefinitions":[{"Type":"AWSQuickSetupType-SSMHostMgmt","Parameters":{"TargetOrganizationalUnits":"ou-1","TargetRegions":"us-west-2"}}]}` and keep the returned ManagerArn (it carries `us-west-2`).
2. POST /listConfigurationManagers signed for eu-west-1 with an empty body: the emulator returns the us-west-2 manager in `ConfigurationManagersList`; the real service returns an empty list for that Region.
3. POST /listConfigurations signed for eu-west-1: the emulator returns the configuration created in us-west-2, whose `Region` member reads `us-west-2`.
4. GET /configurationManager/<us-west-2 ARN> signed for eu-west-1: the emulator answers 200 with the manager; the real service has no such resource in that Region. Same for PUT (rename) and DELETE on that ARN from eu-west-1 — the emulator mutates/removes another Region's resource.
