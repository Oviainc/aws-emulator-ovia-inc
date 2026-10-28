Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a7f4b46fd13e3042ad499c1371bfc738191ffa75

# A configuration definition whose Parameters omit TargetRegions is accepted and silently deployed into the caller's Region

_Filed: 2026-10-28T01:57:41Z_
_Operation: CreateConfigurationManager | Class: validation_

> _Note: this issue was identified against the code base at commit `a7f4b46fd13e3042ad499c1371bfc738191ffa75`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model's documentation for `ConfigurationDefinitionInput$Parameters` enumerates the parameter table of every Quick Setup configuration type, and in every one of those tables `TargetRegions` is marked `(Required)`. The crate reads that same documentation as the authority for its deployment-target rules — `state::NO_DEPLOYMENT_TARGET_MESSAGE` implements the doc's `TargetAccounts`/`TargetOrganizationalUnits` sentence verbatim — but it never implements the `TargetRegions` requirement.

`state::deployment_targets_rejection`, the single helper both `CreateConfigurationManager` and `UpdateConfigurationDefinition` call to decide whether a definition's parameter map can be served, checks only (a) that one of `TargetAccounts`/`TargetOrganizationalUnits` is present, (b) that each `TargetAccounts` entry is a 12-digit account id, and (c) the fan-out bound. A definition carrying no `TargetRegions` at all passes, and `state::deployment_targets` then substitutes the manager's own Region, so the create answers 200 and deploys a configuration into a Region the caller never named.

The consequence is not only a missing 400: the stored state is wrong too. A caller that forgets `TargetRegions` gets a configuration stamped with whatever Region the request happened to be signed for (or `DEFAULT_REGION` when the request carries no SigV4 scope), which `ListConfigurations`/`GetConfiguration` then report as the Region "where the configuration was deployed". The fix belongs in the one shared helper, so it closes the same hole on `UpdateConfigurationDefinition`'s `Parameters` replacement as well.

## Evidence

Smithy model, `smithy_query(query="doc", shape="ConfigurationDefinitionInput", member="Parameters")` — every per-type table marks TargetRegions required, e.g. under "Default Host Management Configuration (Type: Amazon Web ServicesQuickSetupType-DHMC)":
  "<code>TargetRegions</code> ... Description: (Required) A comma separated list of Amazon Web Services Regions you want to deploy the configuration to."
The identical "(Required)" marking for `TargetRegions` appears in the tables for SSMOpsCenter, Scheduler, DHMC, ResourceExplorer, SSMChangeMgr, DevOpsGuru and CFGCPacks — in deliberate contrast to the neighbouring key, which the same doc marks optional: "<code>TargetAccounts</code> ... Description: (Optional) ... A value must be provided for either <code>TargetAccounts</code> or <code>TargetOrganizationalUnits</code>."

The crate implements the second sentence and not the first:
- `src/state.rs:480-481`: `pub const NO_DEPLOYMENT_TARGET_MESSAGE: &str = "Either TargetAccounts or TargetOrganizationalUnits must be defined in parameters.";` (doc above it at `src/state.rs:474-479` cites the same `member="Parameters"` doc as its source).
- `src/state.rs:646-676` `deployment_targets_rejection(...)` — the whole of the rule: `if accounts.is_empty() && organizational_units.is_empty() { return Some(NO_DEPLOYMENT_TARGET_MESSAGE.to_string()); }`, then the account-id form check, then the fan-out bound. `target_parameters.regions` is read (line 650 / `src/state.rs:593`) but never required.
- `src/state.rs:614-618`: `let mut regions = comma_separated(targets.regions); if regions.is_empty() { regions.push(local_region.to_string()); }` — an absent `TargetRegions` is silently replaced by the manager's Region.
- `src/ops/create_configuration_manager.rs:179-193` is the only deployment-target check the create makes (`crate::state::deployment_targets_rejection("ConfigurationDefinitions.Parameters", &definition.parameters)`), under a handler doc that claims the full rule: `src/ops/create_configuration_manager.rs:126-129` — "The deployment targets a definition's parameters name must be servable, because each targeted account and Region pair becomes a stored configuration".
- `src/ops/update_configuration_definition.rs:125-131` calls the same helper for the parameter set it writes, so an update may also drop `TargetRegions`.

Test coverage: no test exercises a missing `TargetRegions` as a refusal; the opposite is assumed — `tests/all/deployment_targets.rs:27-29` ("the Region a created manager owns, and so the Region a definition naming no `TargetRegions` deploys in") and `tests/all/deployment_target_required.rs:10-18`, whose stated rule covers only "a value for either `TargetAccounts` or `TargetOrganizationalUnits`" from the very same model doc.

## Steps to Reproduce

1. Boot the emulator and sign a request for ssm-quicksetup in us-west-2.
2. POST /configurationManager with {"Name":"no-regions","ConfigurationDefinitions":[{"Type":"AWSQuickSetupType-DHMC","Parameters":{"TargetOrganizationalUnits":"ou-1"}}]} — the definition names no TargetRegions, which the model's DHMC parameter table marks (Required).
3. Observe 200 and a minted ManagerArn instead of a 400 ValidationException naming the missing required parameter.
4. POST /listConfigurations with {"ManagerArn":"<ManagerArn>"} and observe one configuration whose Region is "us-west-2" — the Region the request was signed for, which the definition never asked to deploy to.
5. Repeat the same body with no SigV4 credential scope at all: the configuration is stamped with the us-east-1 default (src/state.rs:30), still with no error.
