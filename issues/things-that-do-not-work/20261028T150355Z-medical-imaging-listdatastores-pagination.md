Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 8668e0a390f031d37f864cf13043544a41d29fed

# ListDatastores page tokens are not scoped to the region whose collection they are a position in

_Filed: 2026-10-28T15:03:55Z_
_Operation: ListDatastores | Class: pagination_

> _Note: this issue was identified against the code base at commit `8668e0a390f031d37f864cf13043544a41d29fed`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListDatastores` enumerates ONLY the data stores of the region the request was addressed to (each region is a different collection), but the `nextToken` it mints carries a constant marker with no region in it. A token minted while paging region A therefore decodes successfully when replayed against region B and is honoured as a position in region B's collection, which it is not.

The consequence is a silently wrong success rather than a refusal:

* if region B's data stores were all created BEFORE the record the token names, no store in B has a sort key at or past the cursor, `position()` returns `None`, `start` becomes `ordered.len()`, and the handler answers HTTP 200 with an empty `datastoreSummaries` and no `nextToken` — so a caller that carried a token across regions is told region B holds no data stores at all;
* if region B's stores were created after it, the walk resumes at index 0 and re-serves records the caller is resuming past.

Every sibling listing in this crate scopes its marker by the collection the position belongs to (`ListDICOMImportJobs` by data store, `ListImageSetVersions` by data store + image set, `SearchImageSets` by data store + sort field + order) and the state module states that rule for all of them. `ListDatastores` is the one listing whose collection identity (the region) is missing from its marker.

## Evidence

The rule the state module states for every listing — `src/state.rs:1841-1843`:
```
// The codec lives here because more than one listing uses it, and a token format
// spelled twice drifts. Each collection passes its OWN marker, so a token minted for
// one collection is not a position in another and is refused rather than honoured.
```
The collection `ListDatastores` walks is per-REGION — `src/ops/list_datastores.rs:209-222`:
```
        // Total order over the whole collection — creation instant first, id to break a
        // tie — so a page boundary is stable no matter how the underlying map iterates.
        // Only the data stores of the region this request was addressed to are
        // enumerated: a data store exists in ONE region, so another region's stores are
        // not part of this collection at all.
...
        let mut ordered: Vec<&crate::state::Datastore> = store
            .reported_datastores_in(&region)
```
but its marker names no region — `src/ops/list_datastores.rs:260-264`:
```
/// The marker every token this listing mints carries, so a hex string this service never
/// issued — or one minted for another collection — is refused rather than honoured as a
/// resume point. The codec itself is the service's one
/// ([`crate::state::encode_page_token`]), so every listing's token is built one way.
const PAGE_TOKEN_MARKER: &str = "ahi-datastores-v1:";
```
and the resume silently lands past the end — `src/ops/list_datastores.rs:230-238`:
```
        let start = match &resume_from {
            None => 0,
            Some(cursor) => ordered
                .iter()
                .position(|datastore| sort_key(datastore) >= cursor.as_key())
                .unwrap_or(ordered.len()),
        };

        let end = start.saturating_add(page_size).min(ordered.len());
```
The sibling that does scope its marker — `src/ops/list_dicom_import_jobs.rs:279-289`:
```
/// A hex string this service never issued, one minted for another collection, and one
/// minted against another data store are all refused rather than honoured as a resume
/// point — the import jobs of two data stores are two collections, so a position in one
/// is not a position in the other.
fn page_token_marker(datastore_id: &str) -> String {
    format!("ahi-import-jobs-v1:{datastore_id}:")
}
```
and that sibling rule is actively tested, while no test replays a `ListDatastores` token in another region — `tests/all/import_job_listing.rs:612-625, 694-697`:
```
    // A token this service issued, but for a DIFFERENT data store's jobs: a position in
    // another collection, so not one in this one.
...
        RefusalCase {
            case: "a nextToken issued for another data store's jobs",
            query: format!("nextToken={other_token}"),
```
`tests/all/regional_isolation.rs:100-124` pages `ListDatastores` to the end inside each region only, and `tests/all/datastore_lifecycle.rs:1279-1289` covers only a non-hex and a forged hex token — neither row carries a token minted in another region.

## Steps to Reproduce

1. Create two data stores addressed to eu-west-1 (SigV4 credential scope region eu-west-1) and let them settle to ACTIVE.
2. Create two data stores addressed to us-east-1 afterwards, so their `created_at` is later, and let them settle.
3. `GET /datastore?maxResults=1` addressed to us-east-1 and copy the returned `nextToken`.
4. `GET /datastore?maxResults=1&nextToken=<that token>` addressed to eu-west-1.
5. Expected (per state.rs:1841-1843 and the sibling rule tested in import_job_listing.rs:694-697): the token is a position in another collection, so it is refused with 400 ValidationException and INVALID_NEXT_TOKEN_MESSAGE. Actual: HTTP 200 with `datastoreSummaries: []` and no `nextToken` — eu-west-1's two data stores are skipped and the caller's walk terminates as if the region were empty.
