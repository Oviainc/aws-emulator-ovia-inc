Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: da7bc09c8c1c199d36cb9fc39c5c5796d768dc3a

# The "resolve a policy by its ARN in this region, else ResourceNotFoundException" rule is hand-rolled in all three tag handlers

_Filed: 2026-10-26T23:21:29Z_
_Operation: ListTagsForResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `da7bc09c8c1c199d36cb9fc39c5c5796d768dc3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The three tag operations each re-implement the same two-step resolution — `policy_id_for_arn(&region, arn)` to get an id, then a second `policy`/`policy_mut` lookup with the same `policy_not_found_by_arn(&input.resource_arn)` refusal — instead of one state helper that answers the policy (or the refusal) for an ARN. `state.rs` already owns the ARN index (`policy_id_for_arn`) and the refusal payload (`policy_not_found_by_arn`), so the missing piece is a single `policy_by_arn` / `policy_mut_by_arn` accessor the three handlers call. The copies have already drifted in shape: `list_tags_for_resource.rs` chains the two lookups into one `and_then` with one refusal, while `tag_resource.rs` and `untag_resource.rs` each write TWO separate `else` arms building the same error, the second of which is unreachable (the id came from the same locked store). Hoisting the rule leaves every response byte-identical.

## Evidence

Copy 1 — `src/ops/list_tags_for_resource.rs:150-158`:
```
        let policy = store
            .policy_id_for_arn(&region, &input.resource_arn)
            .and_then(|policy_id| store.policy(&region, &policy_id));
        let Some(policy) = policy else {
            return Err(ListTagsForResourceErrorModeled::ResourceNotFoundException(
                crate::state::policy_not_found_by_arn(&input.resource_arn),
            )
            .into());
        };
```
Copy 2 — `src/ops/tag_resource.rs:161-172`:
```
        let Some(policy_id) = store.policy_id_for_arn(&region, &input.resource_arn) else {
            return Err(TagResourceErrorModeled::ResourceNotFoundException(
                crate::state::policy_not_found_by_arn(&input.resource_arn),
            )
            .into());
        };
        let Some(policy) = store.policy_mut(&region, &policy_id) else {
            return Err(TagResourceErrorModeled::ResourceNotFoundException(
                crate::state::policy_not_found_by_arn(&input.resource_arn),
            )
            .into());
        };
```
Copy 3 — `src/ops/untag_resource.rs:146-157` (identical to copy 2 with `UntagResourceErrorModeled`).

The single-place pieces this rule is otherwise built from — `src/state.rs:848-853`:
```
    pub fn policy_id_for_arn(&self, region: &str, arn: &str) -> Option<String> {
        self.policies_in(region)?
            .values()
            .find(|policy| policy.policy_arn == arn)
            .map(|policy| policy.policy_id.clone())
    }
```
and `src/state.rs:888-897` (`pub fn policy_not_found_by_arn(arn: &str) -> ResourceNotFoundException`).

Test coverage that must stay green and unchanged: `tests/all/lifecycle_policy_crud.rs:1255` `tag_operations_read_and_write_the_stored_tags`, `tests/all/error_paths.rs:402` `a_policy_that_does_not_exist_is_a_resource_not_found_exception` (including the `/tags/not-an-arn` rows at `error_paths.rs:319-342`), and `tests/all/region_isolation.rs:264` `a_policy_is_reachable_only_from_the_region_that_created_it`, which exercises `/tags/{ResourceArn}` across regions.

## Steps to Reproduce

1. Read src/ops/list_tags_for_resource.rs:150-158, src/ops/tag_resource.rs:161-172 and src/ops/untag_resource.rs:146-157 side by side: the same ARN-to-policy resolution and the same ResourceNotFoundException payload are written three times, in two different shapes
2. Grep the crate for `policy_id_for_arn`: state.rs exposes only the id lookup, with no `policy_by_arn` accessor the handlers could share
3. Hoist a `policy_by_arn` / `policy_mut_by_arn` accessor into src/state.rs and have the three handlers call it; re-run tests/all/lifecycle_policy_crud.rs, tests/all/error_paths.rs and tests/all/region_isolation.rs and observe identical responses
