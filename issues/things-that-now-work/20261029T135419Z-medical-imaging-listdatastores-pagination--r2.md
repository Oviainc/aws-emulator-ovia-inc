Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 69ff0d5dd7a30b3ec79ed56649baca6290c20cfe

# ListDatastores page tokens are not scoped to the datastoreStatus filter whose collection they are a position in

_Filed: 2026-10-29T13:54:19Z_
_Operation: ListDatastores | Class: pagination_

> _Note: this issue was identified against the code base at commit `69ff0d5dd7a30b3ec79ed56649baca6290c20cfe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListDatastores` pages over the region's data stores AFTER applying the optional `datastoreStatus` filter, so the filtered listing and the unfiltered listing walk two different populations. The `nextToken` it mints, however, is marked only with the operation prefix and the REGION — nothing about the filter that selected the page. A token minted while paging one filter's collection therefore decodes successfully when replayed under a different (or absent) `datastoreStatus` and is honoured as a position in a collection it is not a position in, so the walk silently skips records the caller never saw instead of refusing the token with the fixed `INVALID_NEXT_TOKEN_MESSAGE` the handler's own comment promises for "anything else".

This is the crate's own rule, applied by its own sibling: `SearchImageSets` puts the ORDER into its marker precisely because a position computed under one ranking is not a position under another, and refuses a token minted under another sort rather than honouring it. A filter changes which records are in the walk at all — a stronger change than the order — yet it is absent from the marker. The same omission exists in `ListDICOMImportJobs` (`jobStatus`) and `SearchImageSets` (`searchCriteria.filters`), which share the codec; the pagination gaps of those two operations are already filed separately, so this issue is anchored on `ListDatastores`, whose only filed pagination gap (the missing region, fixed) did not consider the filter dimension.

No test pins the current behaviour: the status-filter tests ask for a single page (`maxResults=50`), and the only paged walk of a filtered listing re-sends the same filter on every page.

## Evidence

The rule the state module states for every listing — services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/state.rs:2110-2112:
```
// The codec lives here because more than one listing uses it, and a token format
// spelled twice drifts. Each collection passes its OWN marker, so a token minted for
// one collection is not a position in another and is refused rather than honoured.
```
and state.rs:2106-2108 on what a wrong resume costs: "the resume can neither land before a page already served (repeating a record) nor past one never seen (skipping it)."

The collection this listing pages is the region's records AFTER the filter — src/ops/list_datastores.rs:220-225:
```
        let mut ordered: Vec<&crate::state::Datastore> = store
            .reported_datastores_in(&region)
            .filter(|datastore| {
                status_filter.is_none_or(|status| datastore.datastore_status == status)
            })
            .collect();
```
but the marker names only the region — src/ops/list_datastores.rs:265-276:
```
/// The marker every token this listing mints carries: this operation's own prefix plus
/// the region the position belongs to.
...
fn page_token_marker(region: &str) -> String {
    format!("ahi-datastores-v1:{region}:")
}
```
while the handler promises the opposite — src/ops/list_datastores.rs:177-181:
```
        // The token is this service's own opaque handle, marked with the region the
        // position belongs to; only one this service minted against THIS region can be
        // honoured, and anything else is a client error, not an empty page.
```
The resume that consumes a foreign-collection cursor — src/ops/list_datastores.rs:233-241:
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
The sibling in this crate that DOES fold the page-selecting choice into its marker — src/ops/search_image_sets.rs:463-470:
```
    /// The marker every token this search mints carries: this operation's own prefix, the
    /// data store the position belongs to, and the ORDER it is a position in.
    ///
    /// A hex string this service never issued, one minted for another collection, one
    /// minted against another data store and one minted under another sort are all
    /// refused rather than honoured as a resume point — a position in one order is not a
    /// position in another.
    fn page_token_marker(self, datastore_id: &str) -> String {
```
and src/ops/search_image_sets.rs:41-45: "is marked with the data store AND the order it was minted under, so a token honoured under another sort — which would silently skip or repeat image sets — is refused instead."

Test coverage, none of which crosses filters: tests/all/datastore_listing_page_tokens.rs:140-143 states the collection rule as region-only ("a listing's continuation token is a position in the one collection it was minted against — for data stores, the collection of the region the minting request was addressed to"); tests/all/datastore_lifecycle.rs:824-825 + 861-864 exercise `datastoreStatus` only with `maxResults=50` (one page, no token); tests/all/import_job_listing.rs:421-423 walks a filtered listing while re-sending the same filter on every page ("The filter is honoured on every page, not only on a page big enough to hold the whole answer").

## Steps to Reproduce

1. Addressed to one region, create three data stores in creation order A, then B, then C, and let A and C settle to ACTIVE; call DeleteDatastore on B so it reports DELETING (B's created_at sits between A's and C's).
2. GET /datastore?datastoreStatus=ACTIVE&maxResults=1 — the page holds A and the response carries a nextToken (the position of C in the ACTIVE-filtered collection).
3. GET /datastore?maxResults=1&nextToken=<that token> — i.e. resume the same walk with the filter dropped.
4. Expected (per state.rs:2110-2112 and the sibling rule in search_image_sets.rs:463-470): the token is a position in another collection, so it is refused with HTTP 400 ValidationException carrying "Failed to perform requested operation due to invalid NextToken. Please try again."
5. Actual: HTTP 200 whose page starts at C — B, which the unfiltered collection holds between A and C, is never served in the walk and the caller is given no indication a record was skipped.
6. Mirror case: mint the token with no filter and replay it with datastoreStatus=ACTIVE to see the same acceptance of a position in a collection the token was not minted in.
