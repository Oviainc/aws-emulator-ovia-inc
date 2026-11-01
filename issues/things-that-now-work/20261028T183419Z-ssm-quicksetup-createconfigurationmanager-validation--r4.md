Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: af40c6e1c3301a14f83f727add020b300628421a

# A configuration definition whose Parameters omit a parameter its own type's table marks (Required) is accepted and deployed

_Filed: 2026-10-28T18:34:19Z_
_Operation: CreateConfigurationManager | Class: validation_

> _Note: this issue was identified against the code base at commit `af40c6e1c3301a14f83f727add020b300628421a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateConfigurationManager` validates a definition's `Parameters` through `crate::state::deployment_targets_rejection`, whose own doc states the rule it implements as "a parameter set must provide every parameter the documented parameter tables require of it". It then enforces only TWO of those requirements: one of `TargetAccounts`/`TargetOrganizationalUnits`, and `TargetRegions`. Every OTHER parameter the model's per-type tables mark `(Required)` is accepted as absent, so a manager is minted and a configuration is deployed from a parameter set the model says that type cannot be deployed from:

* `AWSQuickSetupType-Scheduler` with no `TargetTagKey`, `TargetTagValue` or `ICalendarString`;
* `AWSQuickSetupType-SSMOpsCenter` with no `DelegatedAccountId`;
* `AWSQuickSetupType-SSMChangeMgr` with no `DelegatedAccountId` or `JobFunction`;
* `AWSQuickSetupType-CFGCPacks` with no `CPackNames`;
* `AWSQuickSetupType-ResourceExplorer` with no `SelectedAggregatorRegion` or `ReplaceExistingAggregator`.

Second symptom of the same class: for the types whose table marks `TargetOrganizationalUnits` itself `(Required)` (SSMOpsCenter, DHMC, ResourceExplorer, SSMChangeMgr) the crate applies only its cross-type "either/or" rule, so a definition of one of those types naming `TargetAccounts` and no OU is accepted too, although the "A value must be provided for either TargetAccounts or TargetOrganizationalUnits" wording the crate derived that rule from appears in the tables of the OTHER types (Scheduler, DevOpsGuru, CFGCPacks, CFGRecording), not in these.

This is not a rule the crate declines to implement: `QUICK_SETUP_TYPE_CATALOG` is already the per-type authority this crate validates `Type` and `TypeVersion` against, and `NO_DEPLOYMENT_REGION_MESSAGE` exists precisely because the same tables mark `TargetRegions` `(Required)` — so the `(Required)` marker is already accepted here as the authority for a 400. The parameter requirement is simply applied to 2 keys instead of to the tables' whole set. The same helper serves `UpdateConfigurationDefinition`, so the update path replaces a parameter map with the same unchecked set.

## Evidence

Model, `smithy_query(query="doc", shape="ConfigurationDefinitionInput", member="Parameters")` — "The following tables outline the parameters for each configuration type." and then, verbatim:
* "<dt>Resource Scheduler (Type: Amazon Web ServicesQuickSetupType-Scheduler)</dt> ... <code>TargetTagKey</code> ... Description: (Required) The tag key assigned to the instances you want to target." ... "<code>TargetTagValue</code> ... Description: (Required) The value of the tag key assigned to the instances you want to target." ... "<code>ICalendarString</code> ... Description: (Required) An iCalendar formatted string containing the schedule you want Change Manager to use."
* "<dt>OpsCenter (Type: Amazon Web ServicesQuickSetupType-SSMOpsCenter)</dt> ... <code>DelegatedAccountId</code> ... Description: (Required) The ID of the delegated administrator account." ... "<code>TargetOrganizationalUnits</code> ... Description: (Required) A comma separated list of organizational units (OUs) you want to deploy the configuration to."
* "<dt>Conformance Packs (Type: Amazon Web ServicesQuickSetupType-CFGCPacks)</dt> ... <code>CPackNames</code> ... Description: (Required) A comma separated list of Config conformance packs."
* "<dt>Resource Explorer (Type: Amazon Web ServicesQuickSetupType-ResourceExplorer)</dt> ... <code>SelectedAggregatorRegion</code> ... Description: (Required) The Amazon Web Services Region where you want to create the aggregator index." ... "<code>ReplaceExistingAggregator</code> ... Description: (Required) ..."

The crate's own statement of the rule, `services/service-com-amazonaws-ssmquicksetup-quicksetup/src/state.rs:653-658`: "`ConfigurationParametersMap` bounds each key and value but says nothing about what a value means, so these rules live here: a parameter set must provide every parameter the documented parameter tables require of it — somewhere to deploy to ([`NO_DEPLOYMENT_TARGET_MESSAGE`]) and the Regions to deploy into ([`NO_DEPLOYMENT_REGION_MESSAGE`])".

What the code under it actually checks, `src/state.rs:671-707`: `let accounts = comma_separated(target_parameters.accounts);` … `if accounts.is_empty() && organizational_units.is_empty() { return Some(NO_DEPLOYMENT_TARGET_MESSAGE.to_string()); }` … `if comma_separated(target_parameters.regions).is_empty() { return Some(NO_DEPLOYMENT_REGION_MESSAGE.to_string()); }` — then only an account-id form check and a deployment-count bound. The definition's `Type` is not consulted at all, so no per-type key can be required.

The only parameter checks the create runs, `src/ops/create_configuration_manager.rs:184-199`: "// A definition's parameters name where it is deployed ... so the targets are checked before anything is stored" → `crate::state::deployment_targets_rejection("ConfigurationDefinitions.Parameters", &definition.parameters, local_account)`.

The crate already treats the `(Required)` marker as the authority for a 400, `tests/all/target_regions_required.rs:16-19`: "The model's documentation for the member is where the rule is stated: the parameter table documented for a configuration type marks `TargetRegions` `(Required)` (`smithy_query(query=\"doc\", shape=\"ConfigurationDefinitionInput\", member=\"Parameters\")`)."

Test coverage of the current, unchecked behaviour (no test states a reason for exempting the per-type keys): `tests/all/configuration_managers.rs:729-730` creates and asserts 200 for `definition("AWSQuickSetupType-SSMOpsCenter", json!({ "TargetRegions": "us-east-1" }))` and `definition("AWSQuickSetupType-Scheduler", json!({ "TargetTagKey": "Env" }))`, and the helper at `tests/all/configuration_managers.rs:213-232` only ever adds `TargetOrganizationalUnits` and `TargetRegions` ("The minimum a create needs ... whose `Parameters` name somewhere to deploy to and the Regions to deploy into"). `tests/all/first_class_parameters.rs:328-344` and `tests/all/target_regions_required.rs:322-328` do send the Scheduler/OpsCenter type-specific keys, so a complete set is already exercised.

## Steps to Reproduce

1. POST /configurationManager signed for us-east-1 with body {"ConfigurationDefinitions": [{"Type": "AWSQuickSetupType-Scheduler", "Parameters": {"TargetOrganizationalUnits": "ou-1", "TargetRegions": "us-east-1"}}]} — no TargetTagKey, no TargetTagValue, no ICalendarString, each marked (Required) for that type.
2. Observe 200 and a minted ManagerArn instead of a 400 ValidationException.
3. GET /configurationManager/{ManagerArn} and POST /listConfigurations: the definition and a deployed configuration are reported with a parameter map that names none of the type's required parameters.
4. Repeat with {"Type": "AWSQuickSetupType-SSMOpsCenter", "Parameters": {"TargetAccounts": "<12-digit account>", "TargetRegions": "us-east-1"}} — no DelegatedAccountId and no TargetOrganizationalUnits, both marked (Required) for that type — and observe 200.
5. PUT /configurationDefinition/{ManagerArn}/{Id} with {"Parameters": {"TargetOrganizationalUnits": "ou-1", "TargetRegions": "us-east-1"}} against the Scheduler definition: the same unchecked set replaces the stored one and the definition is redeployed.
