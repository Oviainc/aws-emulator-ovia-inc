Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 3938003190abafdb7ac5d97f1c3372e4bb8e6b5a

# The listBy SYSTEM/CUSTOM set-selection and category filter are implemented twice, in ListPresets and ListJobTemplates

_Filed: 2026-10-27T02:23:20Z_
_Operation: ListPresets | Class: maintainability_

> _Note: this issue was identified against the code base at commit `3938003190abafdb7ac5d97f1c3372e4bb8e6b5a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "`listBy=SYSTEM` selects the built-in records, any other `listBy` selects the caller's own CUSTOM records, and `category` keeps only records whose stored category equals it" is written out twice, once per handler, in byte-for-byte parallel code that differs only in the record field it reads. Both copies already delegate ordering/cursor/page-size to the state module's shared `page_of_records` (via `page_of_presets`/`page_of_job_templates`), which is where this service puts a listing rule that more than one listing shares — the same consolidation a previous ListJobTemplates maintainability fix performed for the paging half. The two copies of the remaining half can now drift: a change to which set a `listBy` value selects, or to how `category` matches a record with no category, has to be made in two files to keep presets and job templates agreeing. No response and no existing test changes if the branch moves into `crate::state` beside `ListBySortKey::from_list_by`, which already turns the same `listBy` member into the ordering key for both listings.

## Evidence

Copy 1 — services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/list_job_templates.rs:206-224:
```
        let list_by = input.list_by.as_deref();
        let wanted_type = if list_by == Some(crate::state::LIST_BY_SYSTEM) {
            crate::state::TYPE_SYSTEM
        } else {
            crate::state::TYPE_CUSTOM
        };
        ...
        let category = input.category.as_deref();
        let page = crate::state::page_of_job_templates(
            state.job_templates.values()
                .filter(|record| record.template_type == wanted_type)
                .filter(|record| category.is_none_or(|want| record.category.as_deref() == Some(want))),
```

Copy 2 — services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/list_presets.rs:199-217:
```
        let list_by = input.list_by.as_deref();
        let wanted_type = if list_by == Some(crate::state::LIST_BY_SYSTEM) {
            crate::state::TYPE_SYSTEM
        } else {
            crate::state::TYPE_CUSTOM
        };
        ...
        let category = input.category.as_deref();
        let page = crate::state::page_of_presets(
            state.presets.values()
                .filter(|record| record.preset_type == wanted_type)
                .filter(|record| category.is_none_or(|want| record.category.as_deref() == Some(want))),
```

The crate's stated home for a shared listing rule — src/state.rs:3127-3139:
```
impl ListBySortKey {
    /// The key the `listBy` a request carried names ... A `listBy` value that names neither key, such as
    /// [`LIST_BY_SYSTEM`], selects a SET rather than a key, so it orders on `default` too.
    pub fn from_list_by(list_by: Option<&str>, default: Self) -> Self {
```

Tests that pin both copies (and must keep passing unchanged): tests/all/preset_listing.rs:336-344 `a_listing_reports_only_the_type_its_list_by_selects` (rows `listBy=NAME`/`CREATION_DATE` -> TYPE_CUSTOM, `listBy=SYSTEM` -> TYPE_SYSTEM) and tests/all/job_templates.rs:299-307, the identical table for job templates; plus preset_listing.rs:263-308 and job_templates.rs:228-271 for the full listBy/order matrix.

## Steps to Reproduce

1. Open services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/list_job_templates.rs:206-224 and src/ops/list_presets.rs:199-217 side by side: the `wanted_type` branch and the `category` filter are the same logic twice.
2. Note that the ordering/cursor/page-size half of both listings is already shared through `crate::state::page_of_records`, and that `ListBySortKey::from_list_by` (src/state.rs:3133) already consumes the same `listBy` member in the state module.
3. Change which set `listBy=SYSTEM` selects in one file only: the preset listing and the job-template listing now disagree while both compile.
4. Run tests/all/preset_listing.rs and tests/all/job_templates.rs — `a_listing_reports_only_the_type_its_list_by_selects` in each is the pinned behaviour a consolidation must leave byte-identical.
