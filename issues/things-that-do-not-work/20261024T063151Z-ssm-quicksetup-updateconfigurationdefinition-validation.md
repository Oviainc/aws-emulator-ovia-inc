Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# Reject an UpdateConfigurationDefinition whose Parameters define no deployment target

_Filed: 2026-10-24T06:31:51Z_
_Operation: UpdateConfigurationDefinition | Class: validation_

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateConfigurationDefinition` replaces a stored definition's whole `Parameters` set (`src/ops/update_configuration_definition.rs:277-279`: `definition.parameters = crate::state::to_btree_map(Some(parameters))`), and the only checks it applies are the map-size guard (`MAX_PARAMETERS`) and the `IAMRoleArn` form of `LocalDeploymentAdministrationRoleArn` (`update_rejection`, lines 98-116).

Quick Setup's cross-member rule for a configuration definition's parameter map — the map must define either `TargetAccounts` or `TargetOrganizationalUnits`, because a definition that names no deployment target cannot be deployed — is enforced by the real service on this input too, not just on create. Expected: `PUT /configurationDefinition/{ManagerArn}/{Id}` carrying a `Parameters` map with neither key is refused with 400 `ValidationException` (`Either TargetAccounts or TargetOrganizationalUnits must be defined in parameters.`) and the stored definition is left untouched. Actual: the emulator answers 200, overwrites the stored parameter set with the target-less one, restarts the deployment (`restart_deployment`, line 294) and propagates the new map onto every configuration deployed from that definition (lines 300-310) — so a later `GetConfigurationManager` / `ListConfigurations` describe a definition and a configuration AWS would never have accepted, even when the definition DID have a valid target before the update.

Note this is the same rule the open issue `20261022T034519Z-ssm-quicksetup-error-paths-...` establishes from a recording for `CreateConfigurationManager` (its cause 5); that issue scopes the fix to "CreateConfigurationManager input validation" only, so the update path — which can strip an existing target — is not covered by it.

## Evidence

Spec grounding: `ConfigurationDefinitionInput$Parameters` / `UpdateConfigurationDefinitionInput$Parameters` both target `ConfigurationParametersMap`, and the model's documentation for that member lists `TargetOrganizationalUnits` / `TargetAccounts` plus `TargetRegions` as Required parameters for each Quick Setup type (smithy_query doc, ConfigurationDefinitionInput$Parameters). The recorded AWS rejection sentence for this exact rule is cited in open issue 20261022T034519Z (cause 5): 400 ValidationException, `$.Message = 'Either TargetAccounts or TargetOrganizationalUnits must be defined in parameters.'`.

Handler evidence: `src/ops/update_configuration_definition.rs:98-116` (`update_rejection` — only size and role-ARN checks) and lines 274-310 (unconditional replacement + redeploy).

Test coverage: none asserts the rule; on the contrary `tests/all/configuration_definitions.rs:316-460` and `:632-727` replace `Parameters` with target-less maps (`parameters(count)` builds `Key0..KeyN`) and assert 200 plus the replacement, so the permissive behavior is currently pinned by tests that will need updating with the fix.

## Steps to Reproduce

1. POST /configurationManager with `{"Name":"demo","ConfigurationDefinitions":[{"Type":"AWSQuickSetupType-SSMHostMgmt","Parameters":{"TargetOrganizationalUnits":"ou-1","TargetRegions":"us-west-2"}}]}` and keep the returned ManagerArn.
2. GET /configurationManager/{ManagerArn} and read the service-assigned `ConfigurationDefinitions[0].Id`.
3. PUT /configurationDefinition/{ManagerArn}/{Id} with `{"Parameters":{"UpdateSSMAgent":"true"}}` — a map defining neither TargetAccounts nor TargetOrganizationalUnits.
4. Emulator answers 200. Expected 400 ValidationException with Message `Either TargetAccounts or TargetOrganizationalUnits must be defined in parameters.`
5. GET /configurationManager/{ManagerArn} again: the stored definition now carries only `UpdateSSMAgent` (the target parameters are gone) and its Deployment status has been restarted, i.e. the emulator persisted a definition that targets nothing.
