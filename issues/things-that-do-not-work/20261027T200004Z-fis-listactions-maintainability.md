Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 4e290f8d6b034b90409323db1641d26f5c9d1b54

# The paginated resume/cut/mint walk is open-coded in four list handlers, although the crate already states that walk once for the target-account lists

_Filed: 2026-10-27T20:00:04Z_
_Operation: ListActions | Class: maintainability_

> _Note: this issue was identified against the code base at commit `4e290f8d6b034b90409323db1641d26f5c9d1b54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Four of this crate's list handlers hand-roll the same three-step page walk over an ordered collection instead of stating it once, so the copies can drift apart exactly the way the target-account-configuration pair did before it was factored into `crate::state::target_account_configuration_page`.

Pair A — the service-owned CATALOG walk. `ListActions` and `ListTargetResourceTypes` each: decode the `nextToken` behind this operation's marker (`action_page_token_id` / `target_resource_type_page_token_id`) or refuse it with `crate::state::invalid_next_token()`; size the page with `crate::state::page_size(input.max_results)`; iterate the static catalog skipping every entry whose id sorts before the resumed id (`resume_at.is_some_and(|from| entry.id < from)`); mint the next token off the FIRST entry left over and break; otherwise push the entry's summary. The two bodies are character-identical apart from which catalog they walk, which token helper they call and which summary shape they build.

Pair B — the MINTED-ID walk over a `BTreeMap`. `ListExperiments` and `ListExperimentTemplates` each: refuse a `nextToken` that is not `crate::state::is_minted_id(token, <PREFIX>)`; size the page with `crate::state::page_size(input.max_results)`; skip every key sorting before the token (`id.as_str() < token`); set `next_token = Some(id.clone())` on the first key past a full page and break; otherwise push `record.to_summary()`. The only differences are the id prefix, the map walked and the one extra `experimentTemplateId` filter `ListExperiments` applies before the cut.

Both copies of each rule carry the same resume comparison, the same "cut at the first entry past the page" boundary and the same "a token only arrives with a full page" invariant. Because each is written out twice, a change to one — a different resume comparison, a token minted off a different entry, an off-by-one in the cut — leaves its twin paging differently, which is the drift this crate already removed once (fixed issue: 'The target-account-configuration page walk is implemented twice'). Stating each walk once beside the existing token codecs in `state.rs` — taking the collection, the decoded resume point, the page size and an entry->summary mapper, handing back the page and the next token, exactly as `target_account_configuration_page` already does — leaves every response byte-for-byte identical and every existing test passing unchanged.

## Evidence

Pair A, copy 1 — services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/list_actions.rs:170-184:
```
        let page_size = crate::state::page_size(input.max_results);
        let mut actions = Vec::with_capacity(page_size);
        let mut next_token = None;
        for entry in crate::state::ACTION_CATALOG {
            // Resume at the first id at or after the one the token named, which is the
            // order the catalog is kept in.
            if resume_at.is_some_and(|from| entry.id < from) {
                continue;
            }
            if actions.len() >= page_size {
                next_token = Some(crate::state::action_page_token(entry.id));
                break;
            }
            actions.push(entry.to_summary(&region));
        }
```
Pair A, copy 2 — src/ops/list_target_resource_types.rs:163-177:
```
        let page_size = crate::state::page_size(input.max_results);
        let mut target_resource_types = Vec::with_capacity(page_size);
        let mut next_token = None;
        for entry in crate::state::TARGET_RESOURCE_TYPE_CATALOG {
            // Resume at the first id at or after the one the token named, which is the
            // order the catalog is kept in.
            if resume_at.is_some_and(|from| entry.id < from) {
                continue;
            }
            if target_resource_types.len() >= page_size {
                next_token = Some(crate::state::target_resource_type_page_token(entry.id));
                break;
            }
            target_resource_types.push(entry.to_summary());
        }
```
(The token decode above each is the same shape too: list_actions.rs:153-164 `match crate::state::action_page_token_id(token) { Some(id) => Some(id), None => return Err(...ValidationException(crate::state::invalid_next_token())...) }` vs list_target_resource_types.rs:150-161 with `target_resource_type_page_token_id`.)

Pair B, copy 1 — src/ops/list_experiments.rs:152-196:
```
        if let Some(token) = input.next_token.as_deref() {
            if !crate::state::is_minted_id(token, crate::state::EXPERIMENT_ID_PREFIX) {
                return Err(ListExperimentsErrorModeled::ValidationException(
                    crate::state::invalid_next_token(),
                )
                .into());
            }
        }
        let page_size = crate::state::page_size(input.max_results);
...
            for (id, record) in &store.experiments {
                // Resume at the first id at or after the token.
                if input.next_token.as_deref().is_some_and(|token| id.as_str() < token) { continue; }
                ... if experiments.len() >= page_size { next_token = Some(id.clone()); break; }
                experiments.push(record.to_summary());
            }
```
Pair B, copy 2 — src/ops/list_experiment_templates.rs:150-179:
```
        if let Some(token) = input.next_token.as_deref() {
            if !crate::state::is_minted_id(token, crate::state::TEMPLATE_ID_PREFIX) {
                return Err(ListExperimentTemplatesErrorModeled::ValidationException(
                    crate::state::invalid_next_token(),
                )
                .into());
            }
        }
        let page_size = crate::state::page_size(input.max_results);
...
        for (id, record) in &store.experiment_templates {
            if input.next_token.as_deref().is_some_and(|token| id.as_str() < token) { continue; }
            if experiment_templates.len() >= page_size { next_token = Some(id.clone()); break; }
            experiment_templates.push(record.to_summary());
        }
```
The crate's own precedent that this walk belongs in one place — src/state.rs:2561-2592:
```
/// The ONE page walk over the target account configurations of a template or of an
/// experiment: both are a [`BTreeMap`] keyed by `accountId`, so one and the same rule
/// pages them — walk the keys in order, skip every one sorting before the resumed account
/// id ..., stop once the page is full, and mint the token the next page resumes at
/// from the first entry left over. ... so stating the walk once is what keeps the two
/// paging alike: which map, which page size and which summary shape differ between them,
/// and nothing about the paging does.
pub fn target_account_configuration_page<Summary>(
```
Tests that pin the behaviour and must stay unchanged: tests/all/list_actions.rs (`every_page_size_enumerates_every_published_action_exactly_once`, `a_token_this_operation_never_issued_and_a_max_results_out_of_range_are_refused`), tests/all/list_target_resource_types.rs (same two shapes), tests/all/experiment_lifecycle.rs (the `/experiments` and `/experimentTemplates` page walk at line 843 and the `maxResults` range table at line 1511), tests/all/start_experiment_replay.rs:145-175 (walks `/experiments?maxResults=100&nextToken=…`), plus tests/all/target_account_configuration_page_walk.rs for the already-factored pair.

## Steps to Reproduce

1. Open services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/list_actions.rs and read lines 153-184, then src/ops/list_target_resource_types.rs lines 150-177 — the token decode, the resume comparison, the page cut and the token mint are the same code over a different catalog.
2. Open src/ops/list_experiments.rs lines 152-196 and src/ops/list_experiment_templates.rs lines 150-179 — the same rule again over a BTreeMap keyed by minted ids.
3. Compare both with src/state.rs:2561-2592, where the identical walk is already stated ONCE for the two target-account lists.
4. Change the resume comparison in one copy (for example `entry.id < from` to `entry.id <= from` in list_actions.rs) and run `cargo test -p service-com-amazonaws-fis-faultinjectionsimulator`: only that operation's suite fails, showing the walks are independent copies.
5. Confirm the fix is behaviour-preserving: after extracting one catalog page walk and one minted-id page walk into state.rs, tests/all/list_actions.rs, tests/all/list_target_resource_types.rs, tests/all/experiment_lifecycle.rs and tests/all/start_experiment_replay.rs must pass with no edits.
