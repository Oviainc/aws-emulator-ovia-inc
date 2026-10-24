Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: aa34a7732176a121f53f6fa0a60cefb8de4c89bc

# FirstClassParameters echoes a definition's entire parameter map instead of the common parameter subset

_Filed: 2026-10-24T21:30:38Z_
_Operation: ListConfigurations | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `aa34a7732176a121f53f6fa0a60cefb8de4c89bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model carries two distinct parameter members for a configuration definition: the full set (`ConfigurationDefinition$Parameters` / `GetConfigurationOutput$Parameters`, documented as the key-value pairs containing the parameters for the configuration type) and a narrower summary member (`ConfigurationSummary$FirstClassParameters` and `ConfigurationDefinitionSummary$FirstClassParameters`, documented as 'The common parameters and values for the configuration definition'). The summary member exists precisely because a listing reports only the privileged/common parameters of a definition rather than its whole — potentially very large, 40 KB-per-value — parameter map.

The emulator makes no distinction: both summary builders serialize the complete stored parameter map, so `ListConfigurations` (and `ListConfigurationManagers`, which shares the defect through `ConfigurationDefinitionSummary`) reports `FirstClassParameters` byte-identical to the definition's `Parameters` — including every type-specific option the real service keeps out of the summary. A caller that pages a listing to learn the common parameters of many definitions gets the full payload for each, and a caller that compares the summary's `FirstClassParameters` against the full `Parameters` from `GetConfiguration`/`GetConfigurationManager` can never observe the difference the model defines.

Expected: `FirstClassParameters` carries only the common parameters of the configuration type (the subset the real service surfaces in a summary), distinct from the definition's full `Parameters`.
Actual: it carries every parameter the create/update stored, so the two members are always equal.

## Evidence

Spec grounding: `smithy_query(query="shape", shape="ConfigurationSummary,ConfigurationDefinitionSummary,ConfigurationDefinition")` — `ConfigurationDefinitionSummary$FirstClassParameters` and `ConfigurationSummary$FirstClassParameters` are documented as 'The common parameters and values for the configuration definition', while `ConfigurationDefinition$Parameters` (also `GetConfigurationOutput$Parameters`) is the full parameter map for the type; both target `ConfigurationParametersMap` whose values may be up to 40960 characters each, which is why a summary reports a subset.

Handler/state evidence: `src/state.rs` `ConfigurationRecord::to_summary` sets `first_class_parameters: Some(to_hash_map(&self.parameters))` (the configuration's whole parameter map, served by `src/ops/list_configurations.rs:232-239`), and `ConfigurationDefinitionRecord::to_summary` likewise sets `first_class_parameters: Some(to_hash_map(&self.parameters))` (served by `src/ops/list_configuration_managers.rs:208-215`). The doc comment on `ConfigurationDefinitionRecord::to_summary` states the collapse explicitly: 'this emulator stores every parameter the create supplied and distinguishes no subset of them'.

Test coverage: none asserts a subset. `tests/all/configuration_definitions.rs` and `tests/all/deployment_targets.rs` compare the listed `Parameters`/`FirstClassParameters` against everything the create supplied, i.e. they pin the all-parameters behavior rather than the modeled distinction.

## Steps to Reproduce

1. POST /configurationManager with {"Name":"params","ConfigurationDefinitions":[{"Type":"AWSQuickSetupType-SSMHostMgmt","Parameters":{"TargetOrganizationalUnits":"ou-1","TargetRegions":"us-west-2","UpdateSSMAgent":"true","CollectInventory":"true","ScanInstances":"true"}}]} and keep the returned ManagerArn.
2. POST /listConfigurations with {"ManagerArn":"<ManagerArn>"}: each `ConfigurationsList` entry's `FirstClassParameters` holds all five parameters — identical to the full `Parameters` map returned by GET /getConfiguration/<Id>.
3. POST /listConfigurationManagers with an empty body: the manager's `ConfigurationDefinitionSummaries[0].FirstClassParameters` is likewise the complete parameter map, identical to `ConfigurationDefinitions[0].Parameters` from GET /configurationManager/<ManagerArn>, so the modeled common-parameter subset is never observable.
