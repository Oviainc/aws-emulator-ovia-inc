Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 5b028d5a37462f071f2cc1a7c0f151a69190cfd2

# Deduplicate the deployment-target extraction duplicated between CreateConfigurationManager and UpdateConfigurationDefinition

_Filed: 2026-10-25T00:39:53Z_
_Operation: CreateConfigurationManager | Class: maintainability_

> _Note: this issue was identified against the code base at commit `5b028d5a37462f071f2cc1a7c0f151a69190cfd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "a configuration definition's deployment targets are the `TargetRegions` and `TargetAccounts` entries of its parameter map" is implemented FOUR times in this crate's own handler code — once to validate and once to deploy, in each of the two handlers that write a definition. `crate::state` deliberately owns the two halves that follow from that extraction (`deployment_targets`, documented as "The one place a deployed configuration is built" / `deployment_targets_rejection`, documented as "The one place the rule lives, so a create and an update refuse the same target sets"), but the extraction step itself — pulling the two keys out of the parameter map and handing them to those helpers — lives in the handlers, written two different ways: `CreateConfigurationManager` goes through its own local `parameter()` helper for the validation call and inline `.get(..).map(String::as_str)` for the deploy call, while `UpdateConfigurationDefinition` writes the inline form in both places.

The copies can drift, and the crate's own comment says what drift costs: `update_configuration_definition.rs:313-319` warns that "leaving the two copies to disagree would let `ListConfigurations` report a parameter set no definition holds, or a deployment in an account or Region the definition no longer targets." Any change to which parameter keys name a target (the model's `ConfigurationDefinitionInput$Parameters` documentation also names `TargetOrganizationalUnits` as a target source, and an open issue asks for target validation on update) has to be made in all four places in lockstep; missing one leaves the validation path and the deployment path disagreeing about where a definition is deployed.

The fix is a single accessor in `crate::state` (e.g. a `deployment_target_parameters(&parameters)` pair-extractor, or a `ConfigurationDefinitionRecord::deployment_targets(&self, account, region)`) that both handlers call for both the rejection and the fan-out. No response changes: both call sites already pass the same two keys to the same two state helpers, the refusal messages stay where they are (they are built inside `deployment_targets_rejection` from the `member` label each caller passes), and every existing test keeps asserting the same bytes.

## Evidence

Copy 1 — validation, `src/ops/create_configuration_manager.rs:183-189`:
```
            .find_map(|definition| {
                crate::state::deployment_targets_rejection(
                    "ConfigurationDefinitions.Parameters",
                    parameter(&definition.parameters, crate::state::PARAMETER_TARGET_REGIONS),
                    parameter(&definition.parameters, crate::state::PARAMETER_TARGET_ACCOUNTS),
                )
            })
```
with the local accessor that exists only for it, `src/ops/create_configuration_manager.rs:220-225`:
```
fn parameter<'p>(
    parameters: &'p std::collections::HashMap<String, String>,
    key: &str,
) -> Option<&'p str> {
    parameters.get(key).map(String::as_str)
}
```

Copy 2 — deployment, `src/ops/create_configuration_manager.rs:330-341`:
```
            let targets = crate::state::deployment_targets(
                record
                    .parameters
                    .get(crate::state::PARAMETER_TARGET_REGIONS)
                    .map(String::as_str),
                record
                    .parameters
                    .get(crate::state::PARAMETER_TARGET_ACCOUNTS)
                    .map(String::as_str),
                &account,
                &region,
            );
```

Copy 3 — validation, `src/ops/update_configuration_definition.rs:114-126`:
```
    if let Some(why) = input.parameters.as_ref().and_then(|parameters| {
        crate::state::deployment_targets_rejection(
            "Parameters",
            parameters
                .get(crate::state::PARAMETER_TARGET_REGIONS)
                .map(String::as_str),
            parameters
                .get(crate::state::PARAMETER_TARGET_ACCOUNTS)
                .map(String::as_str),
        )
    })
```

Copy 4 — deployment, `src/ops/update_configuration_definition.rs:320-331`:
```
        let targets = crate::state::deployment_targets(
            definition
                .parameters
                .get(crate::state::PARAMETER_TARGET_REGIONS)
                .map(String::as_str),
            definition
                .parameters
                .get(crate::state::PARAMETER_TARGET_ACCOUNTS)
                .map(String::as_str),
            ::engine::emulator_account_id(),
            &manager_region,
        );
```

The crate already states that this rule is supposed to live in one place — `src/state.rs:318-321`: "so both rules live here: a targeted account is named by its account id, and the number of account-and-Region deployments one definition fans out to is bounded by [`MAX_DEPLOYMENT_TARGETS`]. The one place the rule lives, so a create and an update refuse the same target sets." — and `src/state.rs:709-710`: "The one place the fan-out lives, so a create and an update that changes a definition's targets leave the same deployed set."

And the cost of drift, in the crate's own words, `src/ops/update_configuration_definition.rs:313-319`: "leaving the two copies to disagree would let `ListConfigurations` report a parameter set no definition holds, or a deployment in an account or Region the definition no longer targets."

Test coverage that pins the behaviour the refactor must not change: `tests/all/deployment_targets.rs:321` `a_definition_deploys_one_configuration_per_targeted_account_and_region` (create fan-out over `TargetRegions`/`TargetAccounts`, including padding/empty/repeat entries), `tests/all/deployment_targets.rs:671` (the `MAX_DEPLOYMENT_TARGETS` bound over accounts and regions), `tests/all/deployment_targets.rs:454-503` (an update that changes the target set), `tests/all/configuration_definitions.rs:463` `the_deployed_configuration_reports_what_the_update_wrote`, and `tests/all/configuration_definitions.rs:632` `an_update_the_service_cannot_accept_is_refused_and_writes_nothing` (400 + `ValidationException`, no message assertions).

## Steps to Reproduce

1. Open `services/service-com-amazonaws-ssmquicksetup-quicksetup/src/ops/create_configuration_manager.rs` and read lines 183-189, 220-225 and 330-341: the pair of parameter keys `PARAMETER_TARGET_REGIONS` / `PARAMETER_TARGET_ACCOUNTS` is pulled out of a definition's parameter map twice, through two different accessors.
2. Open `services/service-com-amazonaws-ssmquicksetup-quicksetup/src/ops/update_configuration_definition.rs` and read lines 114-126 and 320-331: the same pair is pulled out twice more, inline.
3. Confirm `crate::state` owns the consumers but not the extraction: `grep -n 'PARAMETER_TARGET_' src/` shows the constants defined once in `src/state.rs:237`/`:242` and read in four handler call sites.
4. Note the drift the duplication allows: adding a third target source (the model's `ConfigurationDefinitionInput$Parameters` doc names `TargetOrganizationalUnits` as a required target parameter) to only the deploy copies, or only the validation copies, leaves validation and deployment disagreeing — exactly what the comment at `update_configuration_definition.rs:313-319` warns against.
5. Run `cargo test -p service-com-amazonaws-ssmquicksetup-quicksetup` before and after collapsing the four copies onto one `crate::state` accessor: `tests/all/deployment_targets.rs` and `tests/all/configuration_definitions.rs` must pass unchanged, with identical response bodies.
