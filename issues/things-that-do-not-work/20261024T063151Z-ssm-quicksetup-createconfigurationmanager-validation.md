Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# Reject a ConfigurationDefinitions entry whose Type is not a Quick Setup configuration type

_Filed: 2026-10-24T06:31:51Z_
_Operation: CreateConfigurationManager | Class: validation_

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateConfigurationManager` accepts any `ConfigurationDefinitions[].Type` that satisfies the modeled pattern `^[a-zA-Z0-9_\-.:/]{3,200}$` and stores it verbatim. The crate itself holds the service's type catalog (`crate::state::QUICK_SETUP_TYPE_CATALOG`, the same list `ListQuickSetupTypes` publishes) but deliberately does not use it as an allow-list (`src/state.rs:169-171`: "This is the DISCOVERY catalog, not an allow-list").

Quick Setup deploys a service-owned template per configuration type, so a type the service does not offer cannot be deployed: the real service refuses the create with 400 `ValidationException` rather than minting a manager. Expected: `POST /configurationManager` with `Type: "AWSQuickSetupType-NotAThing"` (or any other pattern-valid but unpublished string) is refused with 400 `ValidationException` and nothing is stored. Actual: the emulator answers 200 with a `ManagerArn`, creates one configuration per definition in that unknown type, and every later `GetConfigurationManager` / `ListConfigurations` / `GetConfiguration` reports a deployed configuration of a type the service does not offer — and the deployment even settles to `SUCCEEDED`.

This is a different rule from the one open issue `20261022T034519Z-ssm-quicksetup-error-paths-...` (cause 5) covers: that one is the cross-member `TargetAccounts`/`TargetOrganizationalUnits` requirement on `Parameters`; this is the `Type` value itself. Closing cause 5 alone still leaves an unsupported type accepted.

## Evidence

Spec grounding: the service models a discovery operation whose sole purpose is to publish the valid set — `ListQuickSetupTypes -> QuickSetupTypeList[QuickSetupTypeOutput{Type, LatestVersion}]` — and `ConfigurationDefinitionInput$Parameters`' documentation enumerates the parameter contract per type (`<name> (Type: AWSQuickSetupType-<type>)`), i.e. `Type` is a closed set the service resolves a template and a parameter contract from; it is modeled as a patterned string only because the set is versioned out of band. Well-established behavior: a control plane that deploys a service-owned template per type rejects an unknown type at validation rather than creating the resource.

Handler evidence: `src/ops/create_configuration_manager.rs:125-179` (`definitions_rejection` checks emptiness, collection sizes and the role-ARN form only) and lines 264-298, which store `definition.r#type` unchecked into both the `ConfigurationDefinitionRecord` and the `ConfigurationRecord`.

Test coverage: none. `tests/all/configuration_managers.rs:1321-1373` exercises refusals for an empty list, a too-short `Type`, a pattern-violating `Type`, a missing `Parameters` member and an over-long `Name`, but no case sends a pattern-valid unsupported type; every success case uses a catalog type.

## Steps to Reproduce

1. POST /configurationManager with `{"Name":"bogus-type","ConfigurationDefinitions":[{"Type":"AWSQuickSetupType-NotAThing","Parameters":{"TargetOrganizationalUnits":"ou-1","TargetRegions":"us-west-2"}}]}`.
2. Emulator answers 200 with a ManagerArn. Expected 400 ValidationException with nothing stored.
3. GET /listQuickSetupTypes: the published catalog does not contain `AWSQuickSetupType-NotAThing`, so the manager just created names a type this service does not offer.
4. GET /configurationManager/{ManagerArn} and POST /listConfigurations: both report the unsupported type, and after the transition span the Deployment status reads SUCCEEDED for a configuration that could never deploy.
