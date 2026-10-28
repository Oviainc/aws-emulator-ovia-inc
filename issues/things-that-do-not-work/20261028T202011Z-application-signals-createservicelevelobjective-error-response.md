Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 31ee5846b7ac7b9895607a192ecc1e91c8df8e5f

# The SLI-config refusals answer a bare prose sentence instead of the crate's single constraint-violation wording the tag-count refusal five lines later goes through

_Filed: 2026-10-28T20:20:11Z_
_Operation: CreateServiceLevelObjective | Class: error-response_

> _Note: this issue was identified against the code base at commit `31ee5846b7ac7b9895607a192ecc1e91c8df8e5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateServiceLevelObjective` enforces three input rules the model cannot express, in one block at the top of `handle`:

1. both SLI configs supplied (`crate::state::exclusive_sli_configs_refusal`);
2. neither SLI config supplied (`crate::state::absent_sli_config_refusal`);
3. more tags than the resource's own API documents (`crate::state::tag_count_refusal`).

Rule 3 is worded through the crate's ONE constraint-violation builder, so it answers the same envelope the boundary validator answers for a modelled constraint: `1 validation error detected: Value at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50`. Rules 1 and 2 answer a bare sentence with no `N validation error(s) detected:` envelope, no `Value at '<memberPath>' failed to satisfy constraint:` clause and no member path at all — so a client that parses `ValidationException.message` to learn which member it must fix gets the member for the tag rule and for every modelled constraint of the same operation, but not for the two rules that decide whether the request defines an SLO.

`crate::state::constraint_clause` declares itself "The ONE place this service words a constraint violation, reached both by the framework-level refusal renderer (`crate::service`) and by a handler that enforces a constraint itself, so the two cannot drift" — and every other handler-enforced constraint of this crate does reach it: `page_bound` (maxResults), `entity_identity_refusal`/`entity_type_refusal_at` (keyAttributes/entity), `code_location_refusal` (location.codeLocation.className), `requested_expiry` (expiresAt), `status_event_window` (startTime/endTime), `PutGroupingConfiguration`'s repeated-grouping-name check, `BatchGetServiceLevelObjectiveBudgetReport`'s sloIds pattern check, and `ExclusionWindowFailure::error_message`'s validation arms. The crate's one deliberate exception, `time_range_refusal`, carries an explicit justification for its free-form wording; `exclusive_sli_configs_refusal` and `absent_sli_config_refusal` carry none.

The same two helpers also serve `UpdateServiceLevelObjective`, which adds a third bare sentence of its own for the evaluation-kind rule, so fixing the helpers fixes both handlers. Only the wording of the message is wrong: the status (400), the error shape (`ValidationException`) and which requests are refused are already right.

## Evidence

THE RULE THE CRATE STATES — services/service-com-amazonaws-applicationsignals-applicationsignals/src/state.rs:729-742:
```
/// One clause of a `ValidationException` message: the member whose value violated a
/// constraint, and the requirement it violated.
///
/// The ONE place this service words a constraint violation, reached both by the
/// framework-level refusal renderer (`crate::service`) and by a handler that enforces a
/// constraint itself, so the two cannot drift.
...
pub fn constraint_clause(member_path: &str, requirement: &str) -> String {
    format!("Value at '{member_path}' failed to satisfy constraint: {requirement}")
}
```
rendered under src/state.rs:759-767 (`validation_envelope`), which src/service.rs:176-185 (`constraint_envelope`) puts every framework-level refusal of this service through.

THE SIDE THAT BREAKS IT — src/state.rs:1102-1109 and 1120-1127, two free-form sentences:
```
pub fn exclusive_sli_configs_refusal(
    sli_config: Option<&ServiceLevelIndicatorConfig>,
    request_based_sli_config: Option<&RequestBasedServiceLevelIndicatorConfig>,
) -> Option<&'static str> {
    (sli_config.is_some() && request_based_sli_config.is_some()).then_some(
        "You can't specify both RequestBasedSliConfig and SliConfig in the same operation.",
    )
}
...
    requested_evaluation_type(sli_config, request_based_sli_config)
        .is_none()
        .then_some("You must specify either SliConfig or RequestBasedSliConfig.")
```

BOTH SIDES IN ONE HANDLER, FIVE LINES APART — src/ops/create_service_level_objective.rs:229-246:
```
        if let Some(message) = crate::state::exclusive_sli_configs_refusal(
            input.sli_config.as_ref(),
            input.request_based_sli_config.as_ref(),
        )
        .or_else(|| {
            crate::state::absent_sli_config_refusal(
                input.sli_config.as_ref(),
                input.request_based_sli_config.as_ref(),
            )
        }) {
            return Err(crate::scaffolded::Scaffolded::validation(message));
        }

        // The maximum this resource's own `Tags` doc states — "You can associate as many as
        // 50 tags with an SLO" — which the modelled `TagList` bound is wider than.
        if let Some(message) = crate::state::tag_count_refusal(input.tags.as_deref()) {
            return Err(crate::scaffolded::Scaffolded::validation(message));
        }
```
and `tag_count_refusal` itself — src/state.rs:782-792 — goes through the builder: `validation_envelope(&[constraint_clause("tags", &format!("Member must have length less than or equal to {MAX_TAGS_PER_RESOURCE}"))])`.

THE SAME HELPERS IN THE SIBLING WRITER, plus a third bare sentence — src/ops/update_service_level_objective.rs:174-179 and 219-224:
```
            return Err(crate::scaffolded::Scaffolded::validation(
                "You cannot change from a period-based SLO to a request-based SLO, or change \
                 from a request-based SLO to a period-based SLO.",
            ));
```

THE DELIBERATE EXCEPTION, which these are not — src/state.rs:1983-1989 (`time_range_refusal`): "the real service refuses an inverted one with `ValidationException` and states the rule with both offending instants in it. The instants are rendered the way the service states them". No such justification exists at either SLI-config helper.

THE CRATE'S OWN CLAIM THAT THERE IS ONE WORDING — tests/all/handler_enforced_constraint_wording.rs:3-13: "One wording for every input constraint this service enforces in a handler, over HTTP. ... Whichever side refuses a request, the client reads ONE form of `ValidationException` message: the `N validation error(s) detected:` envelope over a `Value at '<memberPath>' failed to satisfy constraint: <requirement>` clause that names the member the request must fix". That suite drives only /create-instrumentation-configuration, /get-instrumentation-configuration-status and /service/states; no row covers the SLO writers.

TEST COVERAGE (the fix leaves it passing): nothing asserts these three strings — `grep "You can't specify both|You must specify either|You cannot change from a period-based"` over tests/ returns no match; tests/all/slo_indicator_required.rs asserts the status and the stored state for both SLI rules and pins only the TAG message (`tag_count_message()`, lines 41-48), which is already in builder form.

## Steps to Reproduce

1. Start the emulator and point an `application-signals` client at it.
2. POST /slo with {"Name":"no-indicator"} (neither SliConfig nor RequestBasedSliConfig) — 400 ValidationException whose message is the bare sentence "You must specify either SliConfig or RequestBasedSliConfig.": no `1 validation error detected:` envelope and no member path.
3. POST /slo with {"Name":"both","SliConfig":{...},"RequestBasedSliConfig":{...}} — 400 with the bare sentence "You can't specify both RequestBasedSliConfig and SliConfig in the same operation.".
4. Contrast, in the SAME operation: POST /slo with a valid SliConfig and 60 Tags entries — 400 with "1 validation error detected: Value at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50".
5. Contrast with a modelled constraint of the same operation: POST /slo with {"Name":"a"} — 400 with "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ..." (src/service.rs:138-185).
6. Repeat on the sibling writer: PATCH /slo/{Id} with both configs, and with a config of the other evaluation kind than the stored record — both answer bare sentences through the same two helpers.
