Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 511f9d11774345a530a86fa6799cabc26a1e1916

# SearchImageSets honours a nextToken whose sort key this service never mints, silently hiding every matching image set instead of refusing the token

_Filed: 2026-10-29T09:19:52Z_
_Operation: SearchImageSets | Class: pagination_

> _Note: this issue was identified against the code base at commit `511f9d11774345a530a86fa6799cabc26a1e1916`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

SearchImageSets' continuation token is `hex("<marker><sortKeyText>:<imageSetId>")`. The identifier half IS validated (`decode_keyed_page_token` refuses a tail that is not `is_resource_id(..)`), but the SORT KEY half is not: `SortKey::parse` feeds the text straight to `str::parse` for the `updatedAt`/`createdAt` fields, and accepts ANY text at all for `DICOMStudyDateAndTime`.

`SortKey::as_text` only ever writes `{secs}.{nanos}` where `secs` comes from `crate::state::to_date_time`, which clamps the instant into a non-negative `i64` (`i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX)`). So a key such as `-1.0` (and the `+N` / leading-zero spellings) is a key this service would never mint — yet `secs.parse().ok()?` accepts it and the handler resumes the walk from it.

Because the resume is `position(|(key, image_set)| sort.reaches(..))` and `reaches` is `candidate <= cursor` in the default DESC order, a cursor forged to `-1.0` matches no candidate, `start` becomes `matched.len()`, the page is EMPTY and no `nextToken` is returned. The caller gets HTTP 200 and a walk that terminates having seen none of the image sets the search matched — exactly the "skip" outcome the handler's own comments say the fixed `INVALID_NEXT_TOKEN_MESSAGE` refusal exists to prevent. The same forging works in ASC order with a key above every stored one, and for `DICOMStudyDateAndTime`, where `SortKey::parse` validates nothing whatsoever.

This is an inconsistency inside this one module family: the sibling codec `decode_ordinal_page_token` in `state.rs` refuses precisely these spellings, and says so in a comment; `decode_keyed_page_token` — the codec this search calls — refuses a forged identifier. Only the key half this handler parses itself is unguarded.

## Evidence

What the handler promises — services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/ops/search_image_sets.rs:201-205:
```
        // The token is this service's own opaque handle, marked with the data store and
        // the ORDER the position belongs to; only one this service minted for that pair
        // can be honoured, and anything else is a client error rather than an empty page.
```
and search_image_sets.rs:463-469: "A hex string this service never issued, one minted for another collection, one minted against another data store and one minted under another sort are all refused rather than honoured as a resume point".

What the key parser actually accepts — search_image_sets.rs:521-531:
```
    /// The key [`SortKey::as_text`] wrote, read back for the field it was minted under,
    /// or `None` when the text is not one this service would have written for that field.
    fn parse(field: SortField, text: &str) -> Option<Self> {
        match field {
            SortField::UpdatedAt | SortField::CreatedAt => {
                let (secs, nanos) = text.split_once('.')?;
                Some(Self::Instant(secs.parse().ok()?, nanos.parse().ok()?))
            }
            SortField::StudyDateAndTime => Some(Self::Text(text.to_string())),
        }
    }
```
What `as_text` ever writes — search_image_sets.rs:513-519: `Self::Instant(secs, nanos) => format!("{secs}.{nanos}")`, with `secs` produced by state.rs:2092-2098 `i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX)` — never negative, never signed, never zero-padded.

The sibling codec in the same module DOES validate, with the rule spelled out — services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/state.rs:2197-2209:
```
pub fn decode_ordinal_page_token(marker: &str, token: &str) -> Option<u32> {
    ...
    // Spelled exactly as this service spells an ordinal: ASCII digits only, so a signed
    // or space-padded payload `str::parse` would otherwise accept is not honoured as a
    // position. Every ordinal this service mints is numbered from one, so zero is not one
    // either.
    if payload.is_empty() || !payload.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
```
and the codec this search calls guards its OTHER half — state.rs:2236-2243:
```
pub fn decode_keyed_page_token(marker: &str, token: &str) -> Option<(String, String)> {
    ...
    let (key, resource_id) = payload.rsplit_once(':')?;
    if !is_resource_id(resource_id) {
        return None;
    }
```

The forged key is consumed as the resume point — search_image_sets.rs:275-286:
```
        let start = match &resume_from {
            None => 0,
            Some((cursor_key, cursor_id)) => matched
                .iter()
                .position(|(key, image_set)| {
                    sort.reaches(
                        (key, image_set.image_set_id.as_str()),
                        (cursor_key, cursor_id.as_str()),
                    )
                })
                .unwrap_or(matched.len()),
        };
```
with search_image_sets.rs:456-461 `SortOrder::Descending => candidate <= cursor`.

Test coverage is absent for this payload class: tests/all/image_set_search.rs:1006-1021 only covers "a nextToken that is not hex" (`nextToken=not-a-token`) and "a nextToken that is hex this service never minted" (`nextToken=abcdef`) — both rejected at the marker check — and tests/all/image_set_search.rs:1072-1074 covers only a real token replayed under another sort ("honouring it under another sort would skip or repeat image sets, so it is refused exactly like a token the service never issued"). No row drives a real marker over a forged key payload, so the acceptance is unpinned.

## Steps to Reproduce

1. Create a data store and let the clock settle it to ACTIVE; seed three or more image sets in it (e.g. by three CopyImageSet calls) so a search over it matches several records.
2. POST /datastore/{datastoreId}/searchImageSets?maxResults=1 with no searchCriteria. Note the 200 response's nextToken and that the default sort is reported as updatedAt / DESC.
3. Hex-decode the nextToken: it reads `ahi-image-set-search-v1:<datastoreId>:updatedAt:DESC:<secs>.<nanos>:<imageSetId>`.
4. Replace the `<secs>.<nanos>` key segment with `-1.0`, leaving the marker and the trailing <imageSetId> untouched, and hex-encode the result.
5. POST the same search again with maxResults=1 and that forged nextToken.
6. Observe HTTP 200 with `imageSetsMetadataSummaries: []` and no nextToken — the walk ends having reported none of the matching image sets — instead of the HTTP 400 ValidationException carrying "Failed to perform requested operation due to invalid NextToken. Please try again." the handler comments promise for a token this service did not mint.
7. Repeat with searchCriteria.sort = {sortField: DICOMStudyDateAndTime, sortOrder: ASC} and the key segment replaced by any text that sorts above every stored study date (e.g. `zzzzzz`) to see the same silent acceptance through the unvalidated `SortKey::Text` branch.
