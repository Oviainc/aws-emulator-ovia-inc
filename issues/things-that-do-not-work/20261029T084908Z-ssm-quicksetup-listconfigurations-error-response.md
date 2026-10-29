Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: b5beeec43dab1903d141b50657328b7d4cca24f8

# ListConfigurations answers an empty page for a ConfigurationDefinitionId that names no definition, while the same handler 404s for an absent ManagerArn

_Filed: 2026-10-29T08:49:08Z_
_Operation: ListConfigurations | Class: error-response_

> _Note: this issue was identified against the code base at commit `b5beeec43dab1903d141b50657328b7d4cca24f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListConfigurations` takes two resource identifiers: `ManagerArn` and `ConfigurationDefinitionId`. The handler resolves the first against the store and answers the modelled `ResourceNotFoundException` when it names nothing this Region's endpoint holds, under a comment that states the rule it is applying in general terms ("the request named a resource that does not exist here"). The second identifier is never resolved at all: it is only used as a predicate inside the `keep` closure, so a request naming a configuration definition that does not exist — or one that exists but belongs to a different manager than the `ManagerArn` the same request names — answers 200 with an empty `ConfigurationsList` and an empty `NextToken` instead of the not-found error the operation binds.

The crate already treats an unknown configuration-definition id as a missing resource on the one other operation that addresses one: `UpdateConfigurationDefinition` resolves `{Id}` against the manager's stored definitions and answers `ResourceNotFoundException` when it names none. So the check exists in this crate; `ListConfigurations` is the path that skips it, and it skips it for one of the two identifiers it accepts while honouring the rule for the other in the very same `handle` body.

The effect is that a client cannot distinguish "that configuration definition deploys nothing" from "that configuration definition does not exist": both are an empty page.

## Evidence

The rule the handler states and applies to one identifier — services/service-com-amazonaws-ssmquicksetup-quicksetup/src/ops/list_configurations.rs:204-215:
```
        // A manager this Region's endpoint does not hold is reported as absent, not as
        // an empty page: the request named a resource that does not exist here.
        if let Some(manager_arn) = input.manager_arn.as_deref() {
            if store.manager_in(&region, manager_arn).is_none() {
                return Err(ListConfigurationsErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(crate::state::manager_not_found_message(manager_arn)),
                    },
                )
                .into());
            }
        }
```
The same handler's treatment of the other identifier — list_configurations.rs:229-232 (inside the `paginate` `keep` closure, the ONLY use of the member):
```
                    && input
                        .configuration_definition_id
                        .as_ref()
                        .is_none_or(|id| *id == configuration.configuration_definition_id)
```
The sibling handler that DOES resolve a configuration-definition id and refuses an unknown one — src/ops/update_configuration_definition.rs:307-323:
```
        let Some(definition) = manager
            .definitions
            .iter_mut()
            .find(|definition| definition.id == input.id)
        else {
            return Err(
                UpdateConfigurationDefinitionErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(format!(
                            "configuration manager {} holds no configuration definition with ID {}",
                            input.manager_arn, input.id
                        )),
                    },
                )
                .into(),
            );
        };
```
The operation binds the error it never raises for this member — OPERATIONS.md:78: "**Errors:** `AccessDeniedException (HTTP 403)`, `InternalServerException (HTTP 500)`, `ResourceNotFoundException (HTTP 404)`, `ThrottlingException (HTTP 429)`, `ValidationException (HTTP 400)`".

Test coverage: tests/all/region_scoping.rs:216 (`list_configurations`) and tests/all/configuration_managers.rs exercise `ListConfigurations` with a valid `ManagerArn`; no test in tests/all/ sends a `ConfigurationDefinitionId` that names no stored definition, so the empty-page path is unpinned.

## Steps to Reproduce

1. POST /configurationManager with one ConfigurationDefinitions entry (Type `AWSQuickSetupType-SSMHostMgmt`, Parameters `{"TargetOrganizationalUnits":"ou-1","TargetRegions":"us-east-1"}`) and note the returned ManagerArn.
2. POST /listConfigurations with body `{"ManagerArn":"<that ARN>","ConfigurationDefinitionId":"zzz"}` — an id the manager holds no definition for (it satisfies the modelled `^[a-z0-9-]{1,20}$` pattern, so the boundary walk admits it).
3. Observe 200 with `{"ConfigurationsList":[],"NextToken":""}`.
4. Compare with PUT /configurationDefinition/<that ARN>/zzz, which answers 404 ResourceNotFoundException ("configuration manager ... holds no configuration definition with ID zzz"), and with POST /listConfigurations naming an unknown ManagerArn, which answers 404 from the same handler.
