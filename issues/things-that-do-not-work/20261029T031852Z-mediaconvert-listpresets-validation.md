Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: c0c2239955f7987f5942ca86a100c431d5defb9f

# The ListPresets/ListJobTemplates `category` filter never takes the crate's one blank-filter test, so `?category=%20` silently reports an empty page

_Filed: 2026-10-29T03:18:52Z_
_Operation: ListPresets | Class: validation_

> _Note: this issue was identified against the code base at commit `c0c2239955f7987f5942ca86a100c431d5defb9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`category` is a filter member on both `listBy` listings: a bare `__string` in the model, so the framework passes a zero-length value and one made only of whitespace through to the handler. The crate owns exactly one rule for such a member — `crate::state::blank_member_problem`, whose doc says it is "ONE function for every door that reads such a member" and that a filter value carrying no characters must be refused with `BadRequestException` "rather than a comparison resolved from a value nothing could hold, which would report a narrowing that narrowed to the empty set".

`ListJobs`, `SearchJobs` and `StartJobsQuery` all call it. `ListPresets` (src/ops/list_presets.rs:191-202) and `ListJobTemplates` (src/ops/list_job_templates.rs:201-212) pass `input.category` straight into `records_listed_by`, which compares it for equality against each record's stored category. Because every stored `Category` goes through `optional_free_text` — a value carrying no characters is stored as ABSENT — no record can ever hold a blank category, so a blank `category` filter is the exact outcome the rule forbids: an unrefused 200 with an empty page, indistinguishable from "this account has no presets in that category". The identical blank value sent to the jobs readers is a 400 naming the member.

## Evidence

The crate's one rule for a filter member — services/service-com-amazonaws-mediaconvert-mediaconvert/src/state.rs:620-637:

    /// [`carries_no_characters`] for a member a request may leave out, which is what a FILTER
    /// member always is: omitting such a member and sending it with no characters are
    /// DIFFERENT requests. ... So the second is the bad request
    /// [`blank_member_message`] words rather than a comparison resolved from a value nothing
    /// could hold, which would report a narrowing that narrowed to the empty set ... ONE function for every
    /// door that reads such a member, so none of them can disagree about it.
    pub fn blank_member_problem(member: &str, value: Option<&str>) -> Option<String> {

Siblings that apply it: src/ops/list_jobs.rs:175 (`blank_member_problem("Queue", input.queue.as_deref())`), src/ops/search_jobs.rs:180-192 (`Queue` and `InputFile`), src/ops/start_jobs_query.rs:215-216 (`FilterList.Values`).

The two doors that skip it — src/ops/list_presets.rs:191-202:

        let list_by = input.list_by.as_deref();
        let page = crate::state::page_of_presets(
            crate::state::records_listed_by(
                state.presets.values(),
                list_by,
                input.category.as_deref(),
            ),

and the identical block in src/ops/list_job_templates.rs:201-212. Neither handler mentions `category` in any refusal.

Why the unrefused filter can never match — src/state.rs:3632-3645 compares `record.record_category() == Some(want)`, while src/state.rs:584-586 stores the member:

    pub fn optional_free_text(value: Option<String>) -> Option<String> {
        value.filter(|value| !value.trim().is_empty())
    }

and `tests/all/optional_free_text.rs:361-366` pins that a preset/template created with `"category": "   "` reports NO category at all.

The member's own model text (`smithy_query(query="doc",shape="ListPresetsRequest",member="Category")`): "Optionally, specify a preset category to limit responses to only presets from that category." — and the model gives it `__string` with `httpQuery: category` and no length/pattern, so nothing refuses a blank value before the handler.

Test coverage: the blank-filter rule is pinned only for the jobs readers — tests/all/blank_filter_value.rs:7-8: "Every filter member a jobs reader takes is the model's bare `__string`" — and the category tables (tests/all/preset_listing.rs:380-389, tests/all/job_templates.rs:344-352, tests/all/shared_set_selection.rs:380-427) contain no blank-category row, so nothing pins today's silent empty page.

## Steps to Reproduce

1. Boot the emulator and point a mediaconvert client at it.
2. create_preset(Name='p1', Category='promos', Settings={'ContainerSettings':{'Container':'MP4'}}).
3. GET /2017-08-29/presets?category=%20 (i.e. list_presets(Category=' ')).
4. Observe HTTP 200 with an empty `presets` array. Expected: `BadRequestException` 400 with `Category must not be empty.`, the same refusal `GET /2017-08-29/jobs?queue=%20` and `GET /2017-08-29/search?inputFile=%20` answer for a blank filter value.
5. Repeat against GET /2017-08-29/jobTemplates?category=%20 after create_job_template(..., Category='promos'): the same unrefused empty page, from the identical block in list_job_templates.rs.
6. Contrast GET /2017-08-29/jobs?queue=%20, which answers 400 `Queue must not be empty.`
