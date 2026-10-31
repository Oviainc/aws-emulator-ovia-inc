Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 060392db00c5b3260ef85f15c64a4c64c2352242

# Scope the ListDeploymentPatternVersions nextToken cursor to its workload/pattern

_Filed: 2026-10-29T04:57:48Z_
_Operation: ListDeploymentPatternVersions | Class: pagination_

> _Note: this issue was identified against the code base at commit `060392db00c5b3260ef85f15c64a4c64c2352242`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListDeploymentPatternVersions` builds its pagination cursor from the bare `deploymentPatternVersionName`, but the seeded catalogue publishes the SAME three version names (`1.0.0`, `1.1.0`, `2.0.0`) for EVERY pattern of EVERY workload. The cursor therefore does not identify the item it was issued for: a `nextToken` handed back by one (workloadName, deploymentPatternName) pair is decoded and matched successfully against a DIFFERENT pattern's version list, so the second call answers HTTP 200 with a page resumed in the middle of a result set the token was never issued for (items silently skipped) instead of the fixed invalid-token refusal.

This is the exact failure mode the crate guards against on the sibling paging path: `Deployment::events_with_cursors` deliberately folds the deployment id into each event cursor so a token issued by one deployment's event list is not accepted on another's. `ListDeploymentPatternVersions` — whose result set is likewise scoped by the two required input members — folds nothing in, and `decode_token` performs no scope check (it only strips the `v1:` prefix), so nothing else re-establishes the requirement `paginate` documents for its callers.

## Evidence

The requirement the helper states on its callers — services/service-com-amazonaws-launchwizard-launchwizard/src/state.rs:2409-2418:

    /// `cursor_of` names an item's stable cursor, so a page resumes exactly where the
    /// previous one ended rather than at a remembered offset. "Stable" is a REQUIREMENT on
    /// the caller: the cursor has to be an identity of the item itself that this service
    /// never reassigns to another item (a `deploymentId`, a `workloadName`, a
    /// `deploymentPatternName`, an event's append ordinal) and never a position in a
    /// sequence the store can renumber, so a token either names the item it was issued for
    /// or names none.

The sibling path that meets it — src/state.rs:744-746 (`Deployment::events_with_cursors`):

    /// ... The deployment id is folded in, so a token one deployment's event list issued
    /// is not accepted on another's.

The path that does not — src/ops/list_deployment_pattern_versions.rs:256-266:

        let (page, next_token) = crate::state::paginate(
            summaries,
            input.max_results,
            input.next_token.as_deref(),
            |summary| {
                summary
                    .deployment_pattern_version_name
                    .clone()
                    .unwrap_or_default()
            },

Why that cursor is not an identity "this service never reassigns to another item" — src/state.rs:133 and src/state.rs:1734-1743:

    pub const CATALOGUE_PATTERN_VERSIONS: &[&str] = &["1.0.0", "1.1.0", "2.0.0"];

    fn seeded_pattern_versions(pattern_description: &str) -> Vec<PatternVersion> {
        CATALOGUE_PATTERN_VERSIONS
            .iter()
            .map(|version_name| PatternVersion { version_name: (*version_name).to_string(), ... })

(`seeded_workloads`, src/state.rs:1706, calls `seeded_pattern_versions(pattern.description)` for every one of the 7 seeded patterns, so all of them publish those identical three names.)

No scope is re-checked when the token comes back — src/state.rs:2400-2404 and 2431-2436:

    fn decode_token(token: &str) -> Option<String> {
        let decoded = ::hex::decode(token).ok()?;
        let text = String::from_utf8(decoded).ok()?;
        text.strip_prefix("v1:").map(ToString::to_string)
    }
    ...
            let found = items.iter().position(|item| cursor_of(item) == cursor);

Test coverage: tests/all/list_deployment_pattern_versions.rs:226-257 only ever replays a token against the SAME `(workload, pattern)` body it was issued from, and lines 261-283 pin that "a well-formed token naming nothing in this result set" must answer the fixed `INVALID_NEXT_TOKEN_MESSAGE` with HTTP 400 — the cross-pattern replay is untested, so no test pins the current acceptance.

## Steps to Reproduce

1. POST /listDeploymentPatternVersions with {"workloadName": "SAP", "deploymentPatternName": "SapHanaSingle", "maxResults": 1} — the page holds version "1.0.0" and hands back a nextToken (hex of "v1:1.0.0").
2. POST /listDeploymentPatternVersions again with a DIFFERENT scope and that same token: {"workloadName": "ActiveDirectory", "deploymentPatternName": "AWSManagedAD", "maxResults": 1, "nextToken": "<token from step 1>"}.
3. Observe HTTP 200 with AWSManagedAD's version "1.1.0" — the list resumes after an item the token was never issued for and skips "1.0.0" of that pattern, instead of the HTTP 400 ValidationException carrying state::INVALID_NEXT_TOKEN_MESSAGE that a token naming nothing in this result set is refused with.
4. Compare with the event path: a nextToken from POST /listDeploymentEvents for deployment A, replayed for deployment B, is refused — because that cursor folds the deployment id in (state.rs:747-764).
