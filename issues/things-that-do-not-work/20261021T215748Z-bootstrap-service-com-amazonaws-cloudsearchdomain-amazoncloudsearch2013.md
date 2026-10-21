Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 01973b2fa0798bae2c2d50e81b185c07c4a43584

# Bootstrap the CloudSearch Domain (cloudsearchdomain) emulator

_Filed: 2026-10-21T21:57:48Z_
_Operation: service-com-amazonaws-cloudsearchdomain-amazoncloudsearch2013 | Class: missing_

> _Note: this issue was identified against the code base at commit `01973b2fa0798bae2c2d50e81b185c07c4a43584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 3 operations of this service are unmodified scaffolds answering HTTP 501 and holding zero state. Implement the full service surface as ONE small stateful happy path — this service has only three operations, so the first slice is the whole core loop:

1. **UploadDocuments** (`POST /2013-01-01/documents/batch?format=sdk`) — input: `documents` (blob, @httpPayload, required), `contentType` (`application/json` | `application/xml`, from the `Content-Type` header, required). Parse the SDF (Search Data Format) batch out of the payload: a JSON array of objects each with `type` (`add` | `delete`), `id`, and for adds a `fields` map of field-name -> value (string or list of strings). Apply each entry to the document index: `add` inserts or replaces the document under its `id`; `delete` removes it. Output must be COMPUTED from what was applied: `status` (`"success"`), `adds` (count of add entries applied), `deletes` (count of delete entries applied), `warnings` (list of `DocumentServiceWarning`, e.g. for an unknown field or an ignored entry). A malformed batch, an unparsable body, or a `contentType` other than the two modeled values must return the modeled `DocumentServiceException` (HTTP 400) with a `message`, NOT a 200.

2. **Search** (`GET /2013-01-01/search?format=sdk&pretty=true`) — input: `query` (`q`, required), `size` (default 0), `start` (default 0), `cursor`, `return`, `sort`, `filterQuery` (`fq`), `facet`, `expr`, `highlight`, `queryOptions` (`q.options`), `queryParser` (`q.parser`: dismax|lucene|simple|structured), `stats`, `partial`. Must match against the documents actually uploaded in step 1. Minimum viable matching: `simple` parser — case-insensitive substring/term match of `q` across all text field values; plus `structured`/`lucene` support for the common `field:value` form, and `q=matchall` (with `q.parser=structured`) returning every document. Output: `hits` (`Hits{found, start, hit[]}`) where `found` is the TOTAL number of matching documents (not the page size) and `hit[]` is the page selected by `start`/`size` — each `Hit{id, fields}` with `fields` holding the stored field values (restricted to the comma-separated `return` list when given, and supporting `return=_no_fields`). `status` must carry a real `timems` and a generated `rid`. Honour `sort` (`field asc|desc`, with `_score` and `_id` defaults) and `size=0` meaning "counts only". When `facet` is requested for a field, compute `facets` as a map field -> `BucketInfo{buckets:[Bucket{value,count}]}` from the matching documents; when `stats` is requested, compute `FieldStats{min,max,count,missing,sum,sumOfSquares,mean,stddev}` from the matching values. An invalid `q`/`q.parser`/`fq`/unknown field must return the modeled `SearchException` (HTTP 400).

3. **Suggest** (`GET /2013-01-01/suggest?format=sdk&pretty=true`) — input: `query` (`q`, required), `suggester` (required), `size` (default 0). Return `suggest` = `SuggestModel{query, found, suggestions:[SuggestionMatch{suggestion, score, id}]}` computed by prefix-matching `q` (case-insensitive) against the suggester's source field values over the uploaded documents, truncated to `size` (a sensible default such as 10 when `size` is 0), with `found` = total prefix matches. `status.timems`/`status.rid` populated. An unknown `suggester` must return `SearchException` (HTTP 400) rather than an empty 200.

Every one of these three responses must be computed from persisted state held in the shared engine — no canned bodies, no echoing the request, no `found: 0` placeholder. A document uploaded by UploadDocuments MUST be findable by a subsequent Search and Suggest in the same process, and a document deleted by a later batch MUST stop matching. Because CloudSearch Domain endpoints are per-domain, key the index by the request's domain/host (or endpoint path prefix) so two domains do not share documents.

## State Model

```
CloudSearchDomainState {
    // domain endpoint host (or a single default key) -> index
    domains: HashMap<String, DomainIndex>,
}

DomainIndex {
    // document id -> stored document
    documents: HashMap<String, Document>,   // insertion-ordered (IndexMap) for stable _id ordering
    // suggester name -> source field (seeded with a permissive default so any
    // suggester name used in a test resolves, or registered on first use)
    suggesters: HashMap<String, String>,
    // open search cursors: cursor token -> next offset into the sorted result set
    cursors: HashMap<String, usize>,
}

Document {
    id: String,
    // field name -> one or more values, as uploaded in the SDF `fields` object
    fields: HashMap<String, Vec<String>>,
}
```

Done = a client can POST an SDF batch of a few documents, get back `adds` equal to the number uploaded, Search for a term that appears in one of them and receive exactly that document's `id` and `fields` with `hits.found` correct, page a larger result set with `start`/`size`, request a `facet` and get real bucket counts, Suggest a prefix and get the matching document back, then upload a `delete` batch and see the document disappear from both Search and Suggest.

## Evidence

OPERATIONS.md for this crate lists exactly 3 operations (Search, Suggest, UploadDocuments) with the input members, output members and errors quoted above; wire protocol is `restJson1`, SDK id `CloudSearch Domain`, signing name `cloudsearch`. All three handler files (`src/ops/search.rs`, `src/ops/suggest.rs`, `src/ops/upload_documents.rs`) still carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501, so the service holds no state at all. Output shapes `Hits{found,start,cursor,hit}`, `Hit{id,fields,exprs,highlights}`, `BucketInfo{buckets:[Bucket{value,count}]}`, `FieldStats{min,max,count,missing,sum,sumOfSquares,mean,stddev}`, `SuggestModel{query,found,suggestions}`, `SuggestionMatch{suggestion,score,id}`, `SearchStatus`/`SuggestStatus{timems,rid}` are taken from the Smithy model via smithy_query. Errors `SearchException` and `DocumentServiceException` are both modeled at HTTP 400.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK CloudSearchDomain client at the service endpoint.
2. Call UploadDocuments with contentType=application/json and a payload of three SDF add entries (ids doc1/doc2/doc3, each with a `title` and `genre` field). Expect 200 with status="success", adds=3, deletes=0 — today: HTTP 501 NotImplemented.
3. Call Search with q=<a word appearing only in doc2> (q.parser=simple). Expect hits.found=1 and hits.hit[0].id="doc2" with its stored fields, plus a non-zero status.timems.
4. Call Search with q=matchall, q.parser=structured, size=2, start=1. Expect hits.found=3 with exactly 2 hits starting at offset 1.
5. Call Search with q=matchall, q.parser=structured, facet={"genre":{}}. Expect facets.genre.buckets to contain real value/count pairs derived from the uploaded documents.
6. Call Suggest with q=<prefix of doc1's title> and suggester=<any configured suggester>, size=5. Expect suggest.found>=1 and a SuggestionMatch naming doc1.
7. Call UploadDocuments again with a single SDF delete entry for doc2. Expect adds=0, deletes=1, then re-run the Search from step 3 and expect hits.found=0.
8. Call Search with an invalid q.parser or a malformed fq and expect the modeled SearchException with HTTP 400 (not 501, not 200).
