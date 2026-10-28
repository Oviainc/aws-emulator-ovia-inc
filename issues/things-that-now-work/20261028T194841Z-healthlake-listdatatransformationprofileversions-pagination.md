Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: a67556f919df77bd586efc9ebe2e6b88c4f9f9e7

# Profile versions are paged on the MUTABLE LastUpdatedAt, so an UpdateDataTransformationProfile between two pages serves a version twice

_Filed: 2026-10-28T19:48:41Z_
_Operation: ListDataTransformationProfileVersions | Class: pagination_

> _Note: this issue was identified against the code base at commit `a67556f919df77bd586efc9ebe2e6b88c4f9f9e7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListDataTransformationProfileVersions` pages by looking the request's `NextToken` (a version NUMBER) up by POSITION in the order `DataTransformationProfile::versions_newest_first` computes, and that order is sorted on `ProfileVersionRecord::last_updated_at` — a field `UpdateDataTransformationProfile` REWRITES on the DRAFT. So the position a token resolves to is not the position it was issued for: when the DRAFT is rewritten between two pages it moves to the front of the listing, every entry after it shifts down by one, and resuming at the token re-serves an entry the first page already returned.

Concretely, with a profile whose DRAFT is NOT currently the newest version (the ordinary state right after a publish):
- order is `[v1, DRAFT]`; a page of 1 returns `[v1]` and hands out `NextToken = "0"` (the DRAFT, the first entry of the next page);
- `UpdateDataTransformationProfile` rewrites the DRAFT, so its `last_updated_at` becomes now and the order becomes `[DRAFT, v1]`;
- resuming with `NextToken = "0"` finds the DRAFT at index 0 and returns `[DRAFT, v1]` — `v1` is returned on BOTH pages.

The crate states the opposite rule in two places: the state module claims the sort is "a stable total one and paging through it is resumable" in the same doc paragraph that ADMITS "the DRAFT moves to the front when `UpdateDataTransformationProfile` rewrites it", and the suite that pins the token says "the pages partition the listing, with nothing repeated and nothing skipped". No test exercises a write between two pages, so nothing pins the behaviour the code actually has. Every other paged listing in this crate resumes on an IMMUTABLE key (`ListFHIRDatastores` and `ListDataTransformationProfiles` sort on `created_at`, the job listings on `submitted_at`), so this listing is the only one whose sort key a later call can change.

## Evidence

ORDER SORTED ON A MUTABLE KEY, with the doc both admitting the move and claiming resumability — services/service-com-amazonaws-healthlake-healthlake/src/state.rs:1541-1556:
```
    /// A version's `LastUpdatedAt` is when it last changed, so that is what
    /// "chronological" is measured on: the DRAFT moves to the front when
    /// `UpdateDataTransformationProfile` rewrites it, and a publish puts the
    /// snapshot it took in front of every older version. The version number
    /// breaks a tie between two versions stamped at the same instant,
    /// descending for the same reason, so the order is a stable total one and
    /// paging through it is resumable. The walk is bounded: at most
    /// `1 + MAX_PROFILE_VERSION` versions.
    pub fn versions_newest_first(&self) -> Vec<&ProfileVersionRecord> {
        let mut versions: Vec<&ProfileVersionRecord> = ::std::iter::once(&self.draft)
            .chain(self.published.values())
            .collect();
        versions.sort_by(|a, b| {
            epoch_nanos(&b.last_updated_at)
                .cmp(&epoch_nanos(&a.last_updated_at))
                .then_with(|| b.version.cmp(&a.version))
```

THE WRITE THAT MOVES AN ENTRY — src/ops/update_data_transformation_profile.rs:196-202:
```
        profile.draft.mapping = mapping;
        ...
        profile.draft.change_description = input.change_description;
        profile.draft.last_updated_at = now;
```

THE TOKEN RESOLVED AS A POSITION IN THAT ORDER — src/ops/list_data_transformation_profile_versions.rs:176-192:
```
        let ordered = profile.versions_newest_first();

        // `NextToken` is the version number of the first entry the next page
        // returns, so resuming is a lookup in this same order. ...
        let start = match input.next_token.as_deref() {
            None => 0,
            Some(token) => ordered
                .iter()
                .position(|version| page_token(version.version) == token)
```
and the token handed out — src/ops/list_data_transformation_profile_versions.rs:210-212:
```
        let next_token = ordered
            .get(start + page.len())
            .map(|version| page_token(version.version));
```

TEST COVERAGE pinning the rule the code breaks — tests/all/list_data_transformation_profile_versions.rs:402-404 and 445-469 (`a_next_token_resumes_where_its_page_ended`):
```
/// A `NextToken` resumes the listing exactly where the page that handed it out
/// ended: the pages partition the listing, with nothing repeated and nothing
/// skipped.
```
The test publishes/updates BEFORE it pages (steps at lines 415-422) and never writes between the two pages, so the duplicate above is unpinned.

## Steps to Reproduce

1. Call `CreateDataTransformationProfile` with `SourceFormat: "CSV"` and a `Source.ProfileMapping`; note the `ProfileId` (its DRAFT is version 0).
2. Call `PublishDataTransformationProfile` with that `ProfileId` and `SourceFormat: "CSV"` to create version 1. The listing order is now `[1, 0]` (v1 was stamped after the DRAFT).
3. Call `ListDataTransformationProfileVersions` with `{ProfileId, MaxResults: 1}`. Observe `Items = [version 1]` and `NextToken = "0"`.
4. Call `UpdateDataTransformationProfile` with that `ProfileId` and a `ProfileMapping` (this sets `profile.draft.last_updated_at = now`, src/ops/update_data_transformation_profile.rs:202).
5. Call `ListDataTransformationProfileVersions` with `{ProfileId, NextToken: "0"}`.
6. Observe the resumed page contains version 0 AND version 1 — version 1 is served twice across the two pages, although the crate's own docs say the pages partition the listing (state.rs:1546, tests/all/list_data_transformation_profile_versions.rs:402-404).
