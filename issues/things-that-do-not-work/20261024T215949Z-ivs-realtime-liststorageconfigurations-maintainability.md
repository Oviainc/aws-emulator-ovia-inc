Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 564feb857476f3247a0693cb65aa2d44e3322d3a

# ListStorageConfigurations re-implements the shared foreign-page-token message instead of calling the helper

_Filed: 2026-10-24T21:59:49Z_
_Operation: ListStorageConfigurations | Class: maintainability_

> _Note: this issue was identified against the code base at commit `564feb857476f3247a0693cb65aa2d44e3322d3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::foreign_page_token_message` exists so every listing refuses a page token it did not issue in one wording; four of this crate's five listings call it, and `ListStorageConfigurations` inlines a byte-identical `format!` of its own. The two copies can drift, which is exactly what the helper's doc says must not happen. Replacing the inlined `format!` with the helper call leaves the response body and every existing test unchanged.

## Evidence

The one rule, src/state.rs:2430-2439:
```
/// How this service states a `nextToken` it did not issue: the operation, the member
/// and the requirement, …
/// One rule for every listing of this service — each pages over its own records under
/// its own token prefix — so a token from elsewhere is refused in the same words
/// whichever page it was offered to.
pub fn foreign_page_token_message(operation: &str) -> String {
    format!("{operation}.nextToken is required to be a pagination token issued by {operation}")
}
```
Four listings call it: src/ops/list_stages.rs:164, src/ops/list_participants.rs:243, src/ops/list_stage_sessions.rs:190, src/ops/list_participant_replicas.rs:172 — each `crate::state::foreign_page_token_message(Self::NAME)`.

The second copy, src/ops/list_storage_configurations.rs:167-176:
```
ListStorageConfigurationsErrorModeled::ValidationException,
// The construction `crate::state::violation_clause` states a
// member refusal with: the operation, the member and the
// requirement.
format!(
    "{}.nextToken is required to be a pagination token issued by {}",
    Self::NAME,
    Self::NAME,
)
```

Test coverage that keeps the fix honest: tests/all/storage_configurations.rs:627-663 drives five foreign/empty-token cases through `ListStorageConfigurations` and asserts only the status and `error_code(&response) == "ValidationException"`; tests/all/error_envelope.rs:322-326 pins the exact sentence for the helper's wording ("ListStages.nextToken is required to be a pagination token issued by ListStages"), and tests/all/participant_replication.rs:655 pins it for ListParticipantReplicas.

## Steps to Reproduce

1. Open src/ops/list_storage_configurations.rs:167-176 and src/state.rs:2437-2439 and compare the two format strings — they are identical.
2. Open src/ops/list_stages.rs:159-166 and see the same refusal built by `crate::state::foreign_page_token_message(Self::NAME)`.
3. Change the shared helper's wording: four listings follow it, ListStorageConfigurations keeps the old sentence — the drift the helper's doc forbids.
