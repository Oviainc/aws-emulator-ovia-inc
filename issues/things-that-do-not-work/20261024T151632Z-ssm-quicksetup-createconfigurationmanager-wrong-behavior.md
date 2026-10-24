Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d0704f2fe7a69fbeda61cd50377e188330130aec

# Configurations are not deployed per target account/Region: the definition's TargetRegions/TargetAccounts are ignored and one row is stamped with the caller's Region

_Filed: 2026-10-24T15:16:32Z_
_Operation: CreateConfigurationManager | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d0704f2fe7a69fbeda61cd50377e188330130aec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A Quick Setup configuration definition names where it is deployed through its `Parameters` map (`TargetRegions` plus `TargetAccounts` or `TargetOrganizationalUnits`), and the service then deploys ONE configuration per targeted account+Region pair. That is why `ConfigurationSummary` carries a per-row `Account` ("The ID of the Amazon Web Services account where the configuration was deployed") and `Region` ("The Amazon Web Services Region where the configuration was deployed"), and why `GetConfiguration` reports an `Account`/`Region` pair for a single configuration id.

The emulator creates exactly ONE `ConfigurationRecord` per supplied configuration definition and stamps it with `region = crate::state::request_region(ctx.parts())` and `account = ::engine::emulator_account_id()` — the Region the request happened to be signed for and the emulator's own account — never looking at the definition's target parameters. The parameters are stored and echoed back verbatim, but they have no effect on what is deployed.

Expected: a create whose definition carries `TargetRegions: "us-east-1,us-west-2"` (and target accounts/OUs) produces one configuration per deployed account+Region combination, so `ListConfigurations` reports several rows whose `Region`/`Account` members span the named targets, and each is individually readable with `GetConfiguration`.
Actual: `ListConfigurations` reports exactly one row per definition, whose `Region` is always the caller's signing Region (or the `us-east-1` default when the request carries no SigV4 scope) and whose `Account` is always the emulator account — even when the definition targets other Regions/accounts. A client that drives a multi-Region/multi-account Quick Setup rollout and then enumerates the deployed configurations sees a single, mis-stamped deployment.

## Evidence

Spec grounding: the model's documentation for `ConfigurationDefinitionInput$Parameters` enumerates, for every Quick Setup type, `TargetRegions` — "(Required) A comma separated list of Amazon Web Services Regions you want to deploy the configuration to" — and `TargetOrganizationalUnits`/`TargetAccounts` as the accounts to deploy to (smithy_query doc, ConfigurationDefinitionInput$Parameters). `ConfigurationSummary$Account` / `$Region` and `GetConfigurationOutput$Account` / `$Region` are documented as the account and Region where that configuration WAS DEPLOYED, i.e. one configuration record per deployed target, not per API caller.

Handler evidence: `src/ops/create_configuration_manager.rs:282-316` — one `ConfigurationRecord` per definition, with `account: account.clone()` (from `::engine::emulator_account_id()`, line 258) and `region: region.clone()` (from `crate::state::request_region`, line 257); nothing in the handler or in `src/state.rs` parses `TargetRegions`, `TargetAccounts` or `TargetOrganizationalUnits`.

Test coverage: none asserts per-target fan-out. `tests/all/configuration_managers.rs:872` (`the_configuration_listing_reports_one_record_per_stored_definition`) pins the opposite — exactly one configuration row per definition — and `tests/all/configuration_managers.rs:1510` reads that single row back with the caller's Region.

## Steps to Reproduce

1. POST /configurationManager signed for us-west-2 with {"Name":"fanout","ConfigurationDefinitions":[{"Type":"AWSQuickSetupType-SSMHostMgmt","Parameters":{"TargetOrganizationalUnits":"ou-1","TargetRegions":"us-east-1,us-west-2"}}]} and keep the returned ManagerArn.
2. POST /listConfigurations with {"ManagerArn":"<ManagerArn>"}.
3. Observe a single entry in ConfigurationsList whose Region is "us-west-2" (the signing Region) — no row for the other targeted Region us-east-1, and no row per targeted account.
4. GET /getConfiguration/{Id} for that single id: Account is the emulator account and Region is the signing Region, neither derived from the definition's deployment targets.
