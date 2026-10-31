Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: e39df06442d4f2a86a0fa0e3a1f1c22c399555b8

# TagResource's tag-ceiling ValidationException is a bare sentence, not the clause+envelope every other refusal of this crate is worded with

_Filed: 2026-10-28T16:01:56Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `e39df06442d4f2a86a0fa0e3a1f1c22c399555b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

TagResource enforces a bound the model does not carry — the number of tag entries the record holds AFTER the merge — and reports it with the raw string `crate::state::bound_tag_count` hands back, instead of routing it through this crate's one refusal wording (`crate::state::constraint_clause` / `crate::state::length_clause` wrapped in `crate::state::validation_envelope`).

So a caller that exceeds the per-resource entry bound gets `"tags may carry at most 50 entries, got 51"`, while EVERY other ValidationException this service produces — both the ones the boundary renders and the ones handlers enforce themselves — reads `"1 validation error detected: Value ... at '<member>' failed to satisfy constraint: ..."`. That is exactly the drift `crate::service` says cannot happen, and the crate already does it the other way in two sibling handlers for the *same* helper's message.

The same bare rendering appears on the other `bound_tags` call sites (`create_graph.rs`, `create_graph_snapshot.rs`, `start_export_task.rs`, and `tag_resource.rs`'s first check), while `create_graph_using_import_task.rs` and `restore_graph_from_snapshot.rs` push the identical string through `validation_envelope`. TagResource is the one whose path a wire request can actually reach (each request is bounded to 50 entries by `TagMap`'s `@length`, but the merge of a stored 50-entry map with one new key is not), so it is filed here; all of them are symptoms of the same class and should be fixed together.

## Evidence

The rule this crate states for itself — `services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/service.rs:223-227`:
```
        // This service states a constraint violation through the one builder in
        // `crate::state`, which is also what a handler enforcing a constraint itself
        // reaches, so a refusal found at the boundary and one found in a handler read
        // identically.
```
and the boundary renderer it points at, `src/service.rs:124`:
```
    Some(crate::state::validation_envelope(&clauses))
```
`src/state.rs:3078-3086` — what that envelope is:
```
pub fn validation_envelope(clauses: &[String]) -> String {
    match clauses {
        [only] => format!("1 validation error detected: {only}"),
```

The code that breaks it — `src/ops/tag_resource.rs:187-193`:
```
        if let Err(message) = crate::state::bound_tag_count(tags.len() + added) {
            return Err(TagResourceErrorModeled::ValidationException(ValidationException {
                message,
                reason: Some("CONSTRAINT_VIOLATION".to_string()),
            })
            .into());
        }
```
with `src/state.rs:2996-3002` supplying a bare sentence that is neither a clause nor an envelope:
```
pub fn bound_tag_count(total: usize) -> Result<(), String> {
    if total > MAX_TAGS {
        return Err(format!(
            "tags may carry at most {MAX_TAGS} entries, got {total}"
        ));
```

The sibling that does it right for its own unmodelled bound — `src/ops/list_queries.rs:160-174`:
```
        // `maxResults` carries no range in the model, so the published minimum is
        // enforced here rather than at the boundary: ... The refusal is worded by the same
        // builder the boundary refusal renderer uses (`crate::service`), so a caller
        // cannot tell which side of the boundary caught the violation.
        if input.max_results < MIN_MAX_RESULTS {
            return Err(ListQueriesErrorModeled::ValidationException(ValidationException {
                message: crate::state::validation_envelope(&[crate::state::constraint_clause(
```

The direct repo inconsistency on the SAME helper string — `src/ops/create_graph_using_import_task.rs:307-321` envelopes it:
```
        if let Some(tags) = input.tags.as_ref() {
            if let Err(message) = crate::state::bound_tags(tags) {
                violations.push(message);
            }
        }
        ...
                    message: crate::state::validation_envelope(&violations),
```
while `src/ops/create_graph.rs:224-232` emits it raw:
```
        if let Some(tags) = input.tags.as_ref() {
            if let Err(message) = crate::state::bound_tags(tags) {
                return Err(CreateGraphErrorModeled::ValidationException(ValidationException {
                    message,
```
(same raw form at `src/ops/create_graph_snapshot.rs:195-205`, `src/ops/start_export_task.rs:239-249`, `src/ops/tag_resource.rs:153-159`).

Test coverage: `tests/all/error_message_rendering.rs:125,137,215,238,247,269` pin the `"1 validation error detected: ..."` envelope for every ValidationException the suite asserts; `tests/all/resource_tagging.rs:473-482` asserts only the status and `error_code` for the over-limit TagResource (`"case one key past the entry bound: expected ValidationException, got {body}"`), so no existing test pins the bare message.

## Steps to Reproduce

1. CreateGraph with a `tags` map carrying 50 entries (`MAX_TAGS`), e.g. key0..key49.
2. POST /tags/{graphArn} (TagResource) with `{"tags":{"oneTooMany":"v"}}` — one key the resource does not already carry.
3. Observe HTTP 400 `ValidationException` whose `message` is `tags may carry at most 50 entries, got 51`.
4. Compare with any other 400 from this service, e.g. CreateGraph with `graphName: "my--graph"`, whose message is `1 validation error detected: Value 'my--graph' at 'graphName' failed to satisfy constraint: Member must satisfy regular expression pattern: ...` — the two refusals of the same class read as if they came from different services.
