Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 8839c6cff18de024fc6b68ddfb2d1b408a6dd709

# The tag-set admission rule (per-entry check plus merged-count check) is implemented three times in this crate

_Filed: 2026-10-24T23:02:41Z_
_Operation: StartSearchJob | Class: maintainability_

> _Note: this issue was identified against the code base at commit `8839c6cff18de024fc6b68ddfb2d1b408a6dd709`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "a tag set a caller submits is admitted only when every entry passes `check_tag_entry` and the resulting entry count passes `check_tag_count`" is written out three times in this crate's handlers instead of once. Two of the three copies are byte-identical loops; the third differs only in how it computes the count it checks (merged against the tags already stored). There is no shared `check_tag_map`-style helper, even though `src/state.rs` already owns both primitives.

The copies can drift: any change to the admission rule (an added condition, a different order between the per-entry and the count check, a different refusal channel) has to be made in three places, and the open issue 20261024T164315Z-backupsearch-tagresource-validation already asks a fixer to add a character-set guard to this same rule, explicitly noting it should be closed "once in the shared helper rather than per handler" — which there is currently no seam for. Extracting the two calls into one state-module helper changes no response and no refusal text, because both primitives and both messages stay exactly where they are.

## Evidence

Copy 1 — services/service-com-amazonaws-backupsearch-cryobackupsearchservice/src/ops/start_search_job.rs:240-248:
```
        // The tags this job would be stored with have to fit the caps bounding one
        // record, so a create cannot grow a stored job without bound.
        let tags = input.tags.clone().unwrap_or_default();
        for (key, value) in &tags {
            crate::state::check_tag_entry(key, value)
                .map_err(crate::scaffolded::Scaffolded::validation)?;
        }
        crate::state::check_tag_count(tags.len())
            .map_err(crate::scaffolded::Scaffolded::validation)?;
```
Copy 2 — src/ops/start_search_result_export_job.rs:317-325, the same statements verbatim:
```
        // The tags this export job would be stored with have to fit the caps bounding
        // one record, so a create cannot grow a stored job without bound.
        let tags = input.tags.clone().unwrap_or_default();
        for (key, value) in &tags {
            crate::state::check_tag_entry(key, value)
                .map_err(crate::scaffolded::Scaffolded::validation)?;
        }
        crate::state::check_tag_count(tags.len())
            .map_err(crate::scaffolded::Scaffolded::validation)?;
```
Copy 3 — src/ops/tag_resource.rs:178-188, the same two checks with a merged count:
```
        for (key, value) in &input.tags {
            crate::state::check_tag_entry(key, value)
                .map_err(crate::scaffolded::Scaffolded::validation)?;
        }
        let merged = stored.len()
            + input
                .tags
                .keys()
                .filter(|key| !stored.contains_key(*key))
                .count();
        crate::state::check_tag_count(merged).map_err(crate::scaffolded::Scaffolded::validation)?;
```
The primitives both copies call, src/state.rs:1401-1426 (`check_tag_entry`, `check_tag_count`), already live in one place — only the two-step rule above them is duplicated.

Existing tests that pin the behaviour and must stay green unchanged: tests/all/search_jobs.rs:1219-1280 `a_tag_set_past_the_caps_that_bound_one_record_is_refused` walks each cap at its bound and one past it through StartSearchJob and the tag API (400 ValidationException one past the cap); tests/all/resource_tags.rs and tests/all/tag_removal.rs cover the allowed-character round trip through TagResource/UntagResource/ListTagsForResource; tests/all/export_jobs.rs covers tag counts on StartSearchResultExportJob.

## Steps to Reproduce

1. Read src/ops/start_search_job.rs:240-248 and src/ops/start_search_result_export_job.rs:317-325: the per-entry `check_tag_entry` loop plus the `check_tag_count(tags.len())` call are identical statement-for-statement.
2. Read src/ops/tag_resource.rs:178-188: the same two-step rule a third time, differing only in the count it passes.
3. grep the crate for a shared wrapper over the pair — there is none; `src/state.rs:1401-1426` exposes only the two primitives.
4. Confirm the duplication is load-bearing: the open issue 20261024T164315Z-backupsearch-tagresource-validation asks for a character-set guard on this same rule and says to close it "once in the shared helper rather than per handler", which requires a seam that does not exist.
5. Confirm a fix is response-neutral: extracting the two calls into one helper keeps `check_tag_entry`/`check_tag_count` and their messages untouched, so tests/all/search_jobs.rs:1219-1280, tests/all/resource_tags.rs, tests/all/tag_removal.rs and tests/all/export_jobs.rs pass unchanged.
