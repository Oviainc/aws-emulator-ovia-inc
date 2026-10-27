Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 65fd8797851bb5c5937d1ea0792f5c718c73df47

# The definition fan-out count is computed twice, once as a product formula in the rejection and once as the real cross product

_Filed: 2026-10-27T22:22:05Z_
_Operation: UpdateConfigurationDefinition | Class: maintainability_

> _Note: this issue was identified against the code base at commit `65fd8797851bb5c5937d1ea0792f5c718c73df47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

"How many account-and-Region deployments one configuration definition fans out to" is computed two different ways in this crate, and both are used to enforce a bound on the same request.

`crate::state::deployment_targets` builds the actual target set (defaulting an empty Region list to the manager's Region and an empty account list to the local account, then taking the cross product), and is documented as the one place that fan-out lives; `crate::ops::create_configuration_manager` and `crate::ops::update_configuration_definition` both size their capacity check from it with `.len()`. But `crate::state::deployment_targets_rejection` -- the per-definition `MAX_DEPLOYMENT_TARGETS` bound, called from `update_rejection` in UpdateConfigurationDefinition and from `definitions_rejection` in CreateConfigurationManager -- re-derives the same number with its own arithmetic, `comma_separated(regions).len().max(1) * accounts.len().max(1)`, where the two `.max(1)` calls are a second, open-coded copy of the "empty list means one default target" rule that `deployment_targets` implements by pushing the default.

The two expressions agree today, which is exactly the drift risk: the moment `deployment_targets` changes what a parameter set fans out to (its own doc already contemplates resolving `TargetOrganizationalUnits`, "the only account the organizational units a definition may target instead could resolve to here"), the rejection keeps counting with the old formula and the bound it guards is silently wrong -- a definition could deploy more targets than `deployment_targets_rejection` admits. The crate's own doc on that function already promises the opposite.

The fix is to count once: have `deployment_targets_rejection` take its number from `deployment_targets(parameters, <default account>, <default region>).len()` instead of its own product (the defaults contribute exactly one entry each, so the counted value is identical). No response changes: the refusal messages, their member labels and every status code stay as they are. Note this is NOT the already-fixed extraction duplication (issue 20261025T003953Z, which moved the parameter-key extraction into `deployment_target_parameters`); the remaining duplicate is the count itself.

## Evidence

Copy 1 -- the product formula, services/service-com-amazonaws-ssmquicksetup-quicksetup/src/state.rs:669-675:
```
    let targets = comma_separated(target_parameters.regions).len().max(1) * accounts.len().max(1);
    (targets > MAX_DEPLOYMENT_TARGETS).then(|| {
        format!(
            "{member} names {targets} account and Region deployments, more than the \
             {MAX_DEPLOYMENT_TARGETS} this service deploys for one configuration definition"
        )
    })
```

Copy 2 -- the real cross product (and the defaulting rule the `.max(1)`s duplicate), services/service-com-amazonaws-ssmquicksetup-quicksetup/src/state.rs:614-631:
```
    let targets = deployment_target_parameters(parameters);
    let mut regions = comma_separated(targets.regions);
    if regions.is_empty() {
        regions.push(local_region.to_string());
    }
    let mut accounts = comma_separated(targets.accounts);
    if accounts.is_empty() {
        accounts.push(local_account.to_string());
    }
    accounts
        .iter()
        .flat_map(|account| {
            regions.iter().map(move |region| DeploymentTarget {
                account: account.clone(),
                region: region.clone(),
            })
        })
        .collect()
```
and that same quantity taken as a length by the handlers -- src/ops/update_configuration_definition.rs:329-335:
```
        if let Some(parameters) = input.parameters.as_ref() {
            let planned = crate::state::deployment_targets(
                parameters,
                ::engine::emulator_account_id(),
                &region,
            )
            .len();
```
src/ops/create_configuration_manager.rs:347-353:
```
        let planned_deployments: usize = input
            .configuration_definitions
            .iter()
            .map(|definition| {
                crate::state::deployment_targets(&definition.parameters, &account, &region).len()
            })
            .sum();
```

What the crate says is supposed to hold -- src/state.rs:641-644: "The one place the rule lives, so a create and an update refuse the same target sets -- and, reading the same parameters [`deployment_targets`] deploys from, refuse exactly the sets that cannot be deployed." and src/state.rs:1275-1276: "The one place the fan-out lives, so a create and an update ...".

Tests that pin the behaviour the refactor must leave untouched: tests/all/deployment_targets.rs:689 `a_definition_past_the_deployment_bound_is_refused` (at the bound, one past it, and with the product split across accounts as well as Regions -- lines 710-746), tests/all/deployment_targets.rs:323 `a_definition_deploys_one_configuration_per_targeted_account_and_region`, tests/all/deployment_targets.rs:812 `a_create_whose_definitions_outgrow_the_store_is_refused`, tests/all/deployment_targets.rs:955 `an_update_whose_targets_outgrow_the_store_is_refused`, and tests/all/deployment_target_parameters.rs.

## Steps to Reproduce

1. Open services/service-com-amazonaws-ssmquicksetup-quicksetup/src/state.rs and read line 669: the fan-out size is computed as `comma_separated(regions).len().max(1) * accounts.len().max(1)`.
2. Read lines 609-631 of the same file: `deployment_targets` computes the same size by defaulting each empty list and taking the cross product -- two implementations of one quantity, including two copies of the "empty means one default" rule.
3. Read src/ops/update_configuration_definition.rs:249-256 and :329-335, and src/ops/create_configuration_manager.rs:288-293 and :347-357: one request passes through BOTH implementations, the formula for the per-definition bound and the cross product for the store bound.
4. Simulate the drift: add a third target source (e.g. resolving `TargetOrganizationalUnits` into accounts) to `deployment_targets` only; `deployment_targets_rejection` keeps returning the old count, so a definition is admitted that fans out past MAX_DEPLOYMENT_TARGETS.
5. Collapse the two onto one count (`deployment_targets_rejection` calling `deployment_targets(..).len()`) and run `cargo test -p service-com-amazonaws-ssmquicksetup-quicksetup`: tests/all/deployment_targets.rs and tests/all/deployment_target_parameters.rs must pass unchanged, with identical response bodies.
