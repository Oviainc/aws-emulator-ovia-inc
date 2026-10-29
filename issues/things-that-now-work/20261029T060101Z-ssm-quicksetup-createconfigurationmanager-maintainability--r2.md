Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 4373217f8f4cfab6c9129dcbd57530e9c7b55ce8

# state.rs documents the manager's Region as where a definition naming no TargetRegions deploys, the opposite of the rule the crate enforces

_Filed: 2026-10-29T06:01:01Z_
_Operation: CreateConfigurationManager | Class: maintainability_

> _Note: this issue was identified against the code base at commit `4373217f8f4cfab6c9129dcbd57530e9c7b55ce8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ConfigurationManagerRecord::region` is documented as "where a configuration definition of this manager that names no target Region of its own is deployed". No code in the crate does that, and the crate's actual rule is the exact opposite: `deployment_targets` builds targets ONLY from the definition's own `TargetRegions` parameter (the manager's `region` is never consulted there), and `deployment_targets_rejection` REFUSES a parameter map that names no Region at all with `NO_DEPLOYMENT_REGION_MESSAGE`, before anything is stored. `CreateConfigurationManager`'s handler is the first caller of both helpers (create_configuration_manager.rs:189-199 and :406), so the field it writes carries a doc describing a fallback that cannot happen.

The damage is drift, not a wrong response: the sentence documents precisely the behaviour a FIXED issue removed ("A configuration definition whose Parameters omit TargetRegions is accepted and silently deployed into the caller's Region", 20261028T015741Z). A later change that trusts the field doc instead of the helper docs would re-introduce that bug, and the two statements in the same file cannot both be true.

The fix is confined to the doc comment on that one field (make it say what the field IS used for — the Region whose endpoint serves this manager, which `manager_in` enforces). Every response and every existing test stays byte-for-byte unchanged, and the enforced rule (TargetRegions required) must NOT be reversed. This is a different gap from the already-fixed CreateConfigurationManager maintainability issue about the duplicated deployment-target extraction.

## Evidence

The stale rule — services/service-com-amazonaws-ssmquicksetup-quicksetup/src/state.rs:1008-1011:
```
    /// The Region this manager was created in, the one its ARN carries: where a
    /// configuration definition of this manager that names no target Region of its
    /// own is deployed.
    pub region: String,
```

The rule the code actually implements — state.rs:619-628 (doc of `deployment_targets`):
```
/// ... The Regions are exactly the ones the definition's
/// [`PARAMETER_TARGET_REGIONS`] names — nothing stands in for a Region a definition
/// never asked for, so a parameter set naming none is deployed nowhere at all, which
/// is what [`deployment_targets_rejection`] refuses before any of it is stored.
```
and its body, state.rs:629-648, which takes `local_account` but no region: `let regions = comma_separated(targets.regions); ... if accounts.is_empty() { accounts.push(local_account.to_string()); }` — the only fallback is for the ACCOUNT, never for the Region.

The refusal that makes the documented fallback unreachable — state.rs:681-687:
```
    // The Regions a definition deploys into are required of every configuration type
    // the service offers ... so a
    // parameter set that leaves them out is refused rather than deployed into a Region
    // the caller never named.
    if comma_separated(target_parameters.regions).is_empty() {
        return Some(NO_DEPLOYMENT_REGION_MESSAGE.to_string());
    }
```
with state.rs:498: `pub const NO_DEPLOYMENT_REGION_MESSAGE: &str = "TargetRegions must be defined in parameters.";`

The caller that writes the field and runs that refusal — src/ops/create_configuration_manager.rs:189-199 (`crate::state::deployment_targets_rejection("ConfigurationDefinitions.Parameters", &definition.parameters, local_account)`) and :406 (`let targets = crate::state::deployment_targets(&*record.parameters, &account);`), then :414-415 storing `region: region.clone()`.

Test coverage pinning the code side (so the doc, not the code, is what is wrong): tests/all/target_regions_required.rs:10-19 — "a Quick Setup configuration definition is deployed into the Regions its parameters name and into no others, so every request that writes a definition's parameter map ... is accepted only when that map provides a value for `TargetRegions`, and is otherwise refused with a 400 `ValidationException` having written nothing; nothing stands in for the Regions a definition did not name, least of all the Region the request was signed for." and :33 `const NO_REGION_MESSAGE: &str = "TargetRegions must be defined in parameters.";`

## Steps to Reproduce

1. Read services/service-com-amazonaws-ssmquicksetup-quicksetup/src/state.rs:1008-1011: the `region` field is documented as the Region a definition naming no target Region of its own is deployed into.
2. Read state.rs:681-687 and state.rs:498: a parameter map whose `TargetRegions` names nothing is refused with "TargetRegions must be defined in parameters.", so no definition without target Regions is ever stored or deployed.
3. Read state.rs:629-648: `deployment_targets` takes only `parameters` and `local_account`; the manager's `region` is not an argument and no Region fallback exists.
4. Grep the crate for uses of `ConfigurationManagerRecord::region`: it is only written at create (create_configuration_manager.rs:414-415) and compared in `manager_in`/`manager_in_mut` (state.rs:1258-1277) for endpoint scoping — never used to choose a deployment Region.
5. Confirm with the wire path: POST /configurationManager with ConfigurationDefinitions=[{Type:"AWSQuickSetupType-SSMOpsCenter", Parameters:{"TargetOrganizationalUnits":"ou-1"}}] signed for us-west-2 answers 400 ValidationException "TargetRegions must be defined in parameters." — it is not deployed into us-west-2 as the field doc claims (tests/all/target_regions_required.rs pins this).
