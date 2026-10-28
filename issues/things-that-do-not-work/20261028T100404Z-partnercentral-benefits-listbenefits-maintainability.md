Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 7959e6ca7cb81d5c32ad32f8e1e29786f97ad065

# Hoist the one-extra-record page/cursor computation the three listings each re-implement

_Filed: 2026-10-28T10:04:04Z_
_Operation: ListBenefits | Class: maintainability_

> _Note: this issue was identified against the code base at commit `7959e6ca7cb81d5c32ad32f8e1e29786f97ad065`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other step of this service's pagination rule is single-homed in `crate::state`: the page size (`state::page_size`), the cursor encoding (`state::encode_page_token`) and the resume position (`state::resume_from`). The one step that is NOT is the lookahead-and-cursor computation itself — take one record past the page, issue the cursor from that extra record, then drop it — which is written out verbatim three times, once in each `@paginated` handler (`ListBenefits`, `ListBenefitApplications`, `ListBenefitAllocations`).

The three copies are byte-for-byte the same computation over three different record types, so they can drift independently: a change to one copy (dropping the `truncate`, or reading the cursor off `page.last()` instead of `page.get(page_size)`) would make that one listing issue a cursor for a record it has already reported, or report `page_size + 1` summaries, while the other two keep the correct behaviour. Nothing in the crate states a reason for the three copies — the sibling rules of exactly this kind are deliberately written once (`state::not_found_message` is documented as "One construction for every lookup this service does", `state::member_validation_exception` as "The single construction every in-handler refusal of a member goes through"), so this is the one pagination step that escaped that policy.

The fix is a single helper in `crate::state` beside the three it already owns (taking the filtered iterator, the page size and the catalog, returning the page and the optional cursor). Every response and every existing test stays unchanged.

## Evidence

The same computation, three times:

1. src/ops/list_benefits.rs:191, 242-248:
        let page_size = crate::state::page_size(input.max_results);
        ...
            .take(page_size.saturating_add(1))
            .collect();

        let next_token = page
            .get(page_size)
            .map(|benefit| crate::state::encode_page_token(&input.catalog, &benefit.id));
        page.truncate(page_size);

2. src/ops/list_benefit_applications.rs:198, 288-296:
        let page_size = crate::state::page_size(input.max_results);
        ...
            .take(page_size.saturating_add(1))
            .collect();

        let next_token = page
            .get(page_size)
            .map(|application| {
                crate::state::encode_page_token(&input.catalog, &application.id)
            });
        page.truncate(page_size);

3. src/ops/list_benefit_allocations.rs:198, 282-290:
        let page_size = crate::state::page_size(input.max_results);
        ...
            .take(page_size.saturating_add(1))
            .collect();

        let next_token = page
            .get(page_size)
            .map(|allocation| {
                crate::state::encode_page_token(&input.catalog, &allocation.id)
            });
        page.truncate(page_size);

The comment that states the rule — identical in all three (src/ops/list_benefits.rs:223-226, src/ops/list_benefit_applications.rs:259-262, src/ops/list_benefit_allocations.rs:253-256):
    // The benefits are keyed by id in a `BTreeMap`, so this walk is in one stable
    // order and the token is simply the id to resume at — the range starts directly
    // at that key rather than rescanning from the front. One extra matching record is
    // taken to decide whether a further page exists, and is not reported.

The state module that already owns every other step of the same rule — src/state.rs:2863-2873:
    /// The page size a list operation uses, from the `MaxResults` the request asked for.
    pub fn page_size(max_results: Option<i32>) -> usize {
src/state.rs:2883-2893:
    /// A pagination token naming the record the NEXT page starts at, for a listing of
    /// `catalog`.
    pub fn encode_page_token(catalog: &str, id: &str) -> String {
src/state.rs:2920-2934:
    /// The key a list operation resumes `records` at, given the `NextToken` the request
    /// carried — `None` when it carried none, so the page starts at the front.
    pub fn resume_from<Record>(

The crate's own single-construction policy for a rule shared by several handlers — src/state.rs:2566-2573:
    /// One construction for every lookup this service does, so a client reading the message
    /// learns WHICH resource was missing from the one place the sentence is written.
    pub fn not_found_message(resource: &str, identifier: &str) -> String {

Test coverage that pins the behaviour a refactor must not change: tests/all/pagination_cursor_issuance.rs:51-67 drives all three listings over the wire (`const LISTINGS: [Listing; 3] = [ ListBenefits, ListBenefitApplications, ListBenefitAllocations ]`) across "every seeded catalog, several page sizes" (file header, lines 19-23); tests/all/benefit_allocations.rs and tests/all/list_applications_filter_members.rs page the allocation and application listings with filters.

## Steps to Reproduce

1. Open src/ops/list_benefits.rs and read lines 191 and 242-248 — the page-size resolution, the `take(page_size + 1)` lookahead, the `page.get(page_size)` cursor and the `page.truncate(page_size)`.
2. Open src/ops/list_benefit_applications.rs lines 198 and 288-296 and src/ops/list_benefit_allocations.rs lines 198 and 282-290 and observe the identical four steps, differing only in the record type and the closure variable name.
3. Open src/state.rs lines 2863-2942 and observe that `page_size`, `encode_page_token` and `resume_from` — every other step of the same pagination rule — are already single-homed there.
4. Change one copy (for example drop `page.truncate(page_size)` in list_benefit_allocations.rs:290) and observe that only that listing drifts: the other two keep reporting exactly `page_size` summaries, which is what the triplication makes possible.
