Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: e172822ed36ddbdf21d6296d81819842e39b78b2

# decode_page_token honours a cursor payload the service would never mint, so a tampered nextToken silently skips records instead of being refused

_Filed: 2026-10-28T20:34:35Z_
_Operation: ListDICOMImportJobs | Class: pagination_

> _Note: this issue was identified against the code base at commit `e172822ed36ddbdf21d6296d81819842e39b78b2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The instant-based continuation codec `crate::state::decode_page_token` validates the token's MARKER and the two numeric halves of the instant, but accepts ANY text as the cursor's `resource_id` tie-breaker — including the empty string and text outside the `[0-9a-z]{32}` form this service mints ids in. Its two sibling codecs in the same module do validate their payload: `decode_keyed_page_token` refuses a tail that is not `is_resource_id(..)`, and `decode_ordinal_page_token` refuses a payload that is not a positive run of ASCII digits.

Consequence: a caller who takes a token this service really issued, hex-decodes it and replaces the trailing id with text the service would never mint (e.g. `zzzzzzzz...`) gets an HTTP 200 page whose resume point is computed from that forged key. Because the walk resumes at the first record whose FULL sort key is at or after the cursor (`position(|job| sort_key(job) >= cursor.as_key())`), a forged id that sorts above the real one makes the listing SKIP the record the token pointed at, silently losing it from the caller's walk — exactly the outcome the handlers' own comments say the fixed `INVALID_NEXT_TOKEN_MESSAGE` refusal exists to prevent. Both `ListDICOMImportJobs` and `ListDatastores` page through this one codec, so both are affected.

The regression is invisible to the suite: the rows in `tests/all/error_message_texts.rs` that were written to cover a payload-level refusal hard-code the marker `"ahi-datastores-v1:"` without the region segment the marker has carried since tokens became region-scoped, so they are now rejected at the marker check and never reach the payload branches they name.

## Evidence

The codec accepts any tail, while its own doc claims the opposite — services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/state.rs:1927-1943:
```
/// Decode a token [`encode_page_token`] produced for the collection `marker` names, or
/// `None` when it was not one — a string that is not hex, does not decode to UTF-8,
/// carries another collection's marker (or none), or holds a payload this service would
/// never mint.
pub fn decode_page_token(marker: &str, token: &str) -> Option<PageCursor> {
    ...
    let (instant, resource_id) = payload.split_once(':')?;
    let (secs, nanos) = instant.split_once('.')?;
    Some(PageCursor { instant_secs: secs.parse().ok()?, instant_nanos: nanos.parse().ok()?, resource_id: resource_id.to_string() })
```
The sibling codec in the SAME module does validate it — state.rs:2004-2012:
```
pub fn decode_keyed_page_token(marker: &str, token: &str) -> Option<(String, String)> {
    ...
    let (key, resource_id) = payload.rsplit_once(':')?;
    if !is_resource_id(resource_id) {
        return None;
    }
```
and so does the ordinal codec — state.rs:1969-1977: "Spelled exactly as this service spells an ordinal: ASCII digits only, so a signed or space-padded payload `str::parse` would otherwise accept is not honoured as a position."

What the handlers promise — src/ops/list_dicom_import_jobs.rs:181-185: "only one this service minted against THIS data store can be honoured, and anything else is a client error rather than an empty page", and list_dicom_import_jobs.rs:282-285: "A hex string this service never issued, one minted for another collection, and one minted against another data store are all refused rather than honoured as a resume point". The same sentence is in src/ops/list_datastores.rs:177-181. The resume that consumes the forged key is list_dicom_import_jobs.rs:242-248 (`position(|job| sort_key(job) >= cursor.as_key()).unwrap_or(ordered.len())`).

Test coverage is stale, not absent: tests/all/error_message_texts.rs:535-558 builds its "the marker over an unparseable instant" and "the marker over a payload with no instant separator" rows from
```
// The marker every token this service mints carries (`PAGE_TOKEN_MARKER` in
// `src/ops/list_datastores.rs`), over a payload it would never mint.
"ahi-datastores-v1:notanumber.0:{}"
```
while the marker is now `format!("ahi-datastores-v1:{region}:")` (src/ops/list_datastores.rs:274-276) and no `PAGE_TOKEN_MARKER` item exists anywhere in the crate (grep finds the name only in that test comment). The remaining token rows in tests/all/import_job_listing.rs:676-701 and tests/all/datastore_lifecycle.rs:1276-1293 only cover "not hex" and "hex without the marker", so no test drives a real marker over a forged payload.

## Steps to Reproduce

1. Create three data stores in one region and submit three import jobs against one of them, letting the clock settle them.
2. Call ListDICOMImportJobs on that data store with maxResults=1 and keep the returned nextToken.
3. Hex-decode the token: it reads `ahi-import-jobs-v1:<datastoreId>:<secs>.<nanos>:<jobId>`.
4. Replace the trailing <jobId> with a value this service never mints that sorts above it (for example 32 'z' characters), re-hex-encode, and pass it as nextToken.
5. Observe HTTP 200 with a page that starts PAST the job the real token pointed at (that job never appears in the walk), instead of the HTTP 400 ValidationException carrying "Failed to perform requested operation due to invalid NextToken. Please try again." the handler comments promise for a token this service did not issue.
6. Repeat on ListDatastores with the token mutated the same way to see the identical acceptance through the same codec.
