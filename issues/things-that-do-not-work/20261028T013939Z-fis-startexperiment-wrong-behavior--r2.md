Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c550c8d329f766442e2f60b5ca19de7c84eb66f9

# StartExperiment's idempotency fingerprint renders the RAW request instead of what the start persists, so a retry that spells out the default actionsMode (or sends an empty tags map) is refused 409

_Filed: 2026-10-28T01:39:39Z_
_Operation: StartExperiment | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c550c8d329f766442e2f60b5ca19de7c84eb66f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartExperiment`'s `clientToken` is `@idempotencyToken`, and this crate decides "is this request a retry" by comparing a canonical rendering of the request (`start_experiment_fingerprint`) against the one stored with the token. That rendering is taken from the RAW input: `actionsMode` is rendered as whatever the request carried (`null` when `experimentOptions` is absent) and `tags` is rendered as `null` for an absent map and `{}` for an explicitly empty one.

But neither difference changes what the start persists. The handler resolves `actionsMode` through `crate::state::resolve_actions_mode`, which fills `DEFAULT_ACTIONS_MODE` ("run-all") for an absent value, and that RESOLVED value is what lands on the experiment record (`experiment_options.actions_mode: Some(actions_mode.to_string())`). Likewise `tags: tags.unwrap_or_default()` makes an omitted and an empty map the same stored value.

Consequence: a client that retries a start with `experimentOptions: {"actionsMode": "run-all"}` where the first call omitted `experimentOptions` (or that normalises `tags` to `{}` on retry — exactly what a generated client or a retrying wrapper may do) gets HTTP 409 `ConflictException` "clientToken <T> is not assigned to this resource." instead of the experiment that token already started, even though both requests start a byte-identical experiment. The idempotency token therefore fails at its one job for a request pair the service itself cannot tell apart once stored.

This is the normalisation the crate's two SIBLING fingerprints already perform. `target_account_configuration_fingerprint` renders `description` through `stored_description` and says why in its own doc; `template_fingerprint` is deliberately taken from the stored RECORD "so every member the service persists is covered, including the defaults it filled in". `start_experiment_fingerprint` is the one of the three that skips it, although the resolved `actionsMode` is already in hand at the call site.

## Evidence

The odd one out — `services/service-com-amazonaws-fis-faultinjectionsimulator/src/state.rs:4306-4315`:
```
pub fn start_experiment_fingerprint(input: &StartExperimentInput) -> String {
    ::serde_json::json!({
        "experimentTemplateId": input.experiment_template_id,
        "actionsMode": input
            .experiment_options
            .as_ref()
            .and_then(|options| options.actions_mode.clone()),
        "tags": ordered_entries(input.tags.as_ref()),
    })
    .to_string()
}
```

The sibling that DOES normalise, with its stated reason — `src/state.rs:4325-4338`:
```
/// ... The description is rendered as the store
/// would hold it ([`stored_description`]), because an omitted description and an empty one
/// leave the very same configuration behind. See [`start_experiment_fingerprint`] for why
/// the rendering is JSON.
pub fn target_account_configuration_fingerprint(
    input: &CreateTargetAccountConfigurationInput,
) -> String {
    ::serde_json::json!({
        "experimentTemplateId": input.experiment_template_id,
        "accountId": input.account_id,
        "roleArn": input.role_arn,
        "description": stored_description(input.description.as_deref()),
    })
```

The other sibling, which states the rule in general — `src/state.rs:4341-4348`:
```
/// The canonical rendering of the `CreateExperimentTemplate` request a `clientToken`
/// belongs to, taken from the record that request builds.
///
/// Rendered from the RECORD rather than the request so every member the service persists
/// is covered, including the defaults it filled in; ...
pub fn template_fingerprint(record: &ExperimentTemplateRecord) -> String {
```

That the two renderings describe the SAME persisted experiment — `src/state.rs:3211-3219`:
```
pub fn resolve_actions_mode(requested: Option<&str>) -> Result<String, ValidationException> {
    ...
    Ok(requested.unwrap_or(DEFAULT_ACTIONS_MODE).to_string())
}
```
(`src/state.rs:397`: `pub const DEFAULT_ACTIONS_MODE: &str = "run-all";`)

and what is stored — `src/state.rs:1575-1583`:
```
            tags: tags.unwrap_or_default(),
            experiment_options: ExperimentOptions {
                account_targeting: template.experiment_options.account_targeting.clone(),
                empty_target_resolution_mode: template
                    .experiment_options
                    .empty_target_resolution_mode
                    .clone(),
                actions_mode: Some(actions_mode.to_string()),
            },
```

The resolved value is already available where the fingerprint is taken — `src/ops/start_experiment.rs:399-409` resolves it, then `src/ops/start_experiment.rs:176` and `:225` both call `crate::state::start_experiment_fingerprint(input)` with the raw `input` instead.

The refusal that results — `src/state.rs:3398-3403`:
```
pub fn token_conflict(token: &str) -> ConflictException {
    ConflictException {
        message: Some(format!(
            "clientToken {token} is not assigned to this resource."
        )),
```

Test coverage: `tests/all/control_plane_rules.rs:595-624` (`reusing_a_client_token_for_a_different_start_request_is_a_conflict`) exercises four rows — "identical", "another template", "other tags", "another actions mode" — all of which spell `experimentOptions` out on BOTH calls, so no test covers the omitted-vs-default-spelled pair or the absent-vs-empty `tags` pair. Normalising the fingerprint through `resolve_actions_mode` and through the stored tag map keeps every one of those rows answering exactly as it does today (`skip-all` still differs from `run-all`, `{"Run":"second"}` still differs from `{"Run":"first"}`).

## Steps to Reproduce

1. Create an experiment template (e.g. one `aws:fis:wait` action with `duration: PT5M`) and note its id T.
2. POST /experiments with body {"clientToken": "tok-1", "experimentTemplateId": T} — no `experimentOptions`, no `tags`. Observe HTTP 200 and that the returned experiment reports `experimentOptions.actionsMode: "run-all"` (the filled-in default) and `tags: {}`.
3. Retry the SAME logical request, spelling the default out: POST /experiments with {"clientToken": "tok-1", "experimentTemplateId": T, "experimentOptions": {"actionsMode": "run-all"}, "tags": {}}.
4. Observe HTTP 409 ConflictException "clientToken tok-1 is not assigned to this resource." instead of the experiment the token already started — although step 2's stored experiment is identical in every member to the one step 3 describes.
5. Compare with the sibling: create a target account configuration with {"clientToken": "tok-2", ... } omitting `description`, then retry with `description: ""` — that retry REPLAYS (200) because `target_account_configuration_fingerprint` renders the member as the store holds it.
