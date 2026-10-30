Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: 02c390dd8e861c3d98b16a550da31d4601fa546f

# The shared listing page walk accepts nextToken spellings it never issues: decode_page_token honours uppercase/mixed-case hex that encode_page_token never mints

_Filed: 2026-10-29T13:57:59Z_
_Operation: ListAssets | Class: pagination_

> _Note: this issue was identified against the code base at commit `02c390dd8e861c3d98b16a550da31d4601fa546f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated listing of this crate pages through the one shared helper `crate::state::page_of`, which turns a resume key into a token with `encode_page_token` (`hex::encode`, which emits LOWERCASE hex only) and reads one back with `decode_page_token` (`hex::decode`, which accepts both cases). The helper documents the contract as "`None` for a `next_token` this service never issued", and each listing handler repeats it ("refusing a token whose resume point is not in this enumeration rather than answering an empty page", "HTTP 400 `InvalidRequestException` for a `nextToken` this operation never issued").

The code does not implement that contract for the spelling of the token itself: a caller who sends the uppercase (or mixed-case) spelling of a token this service issued — e.g. a token mangled by an intermediary that upper-cases query strings, or a hand-forged token — decodes to the same resume key and is served a page, with 200, as though the service had issued it. The modelled `NextToken` pattern `^[A-Za-z0-9+/=]+$` admits uppercase, so such a request reaches the handler. The result is that the one thing the crate says it refuses — text it never issued — is accepted, and the refusal it documents becomes dependent on which hex spelling the caller happens to send.

## Evidence

The asymmetric codec, in the crate's own single page-walk helper:
  services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/state.rs:6823-6834 —
    "/// A resume key as the opaque `nextToken` that resumes at it.
     pub fn encode_page_token(resume: &str) -> String {
         ::hex::encode(resume.as_bytes())
     }

     /// The resume key `token` encodes, or `None` for text this service never issued
     /// as a token.
     pub fn decode_page_token(token: &str) -> Option<String> {
         ::hex::decode(token)
             .ok()
             .and_then(|bytes| String::from_utf8(bytes).ok())
     }"
  (`hex::encode` writes lowercase digits only; `hex::decode` accepts `0-9a-fA-F`, so every uppercase/mixed-case spelling of an issued token decodes to the same resume key.)

Where the page walk relies on that `None` to refuse:
  src/state.rs:6786-6789 — "/// `None` for a `next_token` this service never issued — text that is not the hex\n/// of a resume key, or a key no entry of this enumeration carries. Its caller\n/// answers that with the modelled refusal for an invalid `nextToken`, rather than\n/// with an empty page, which would report absence as success."
  src/state.rs:6801-6807 — "    let start = match next_token {\n        Some(token) => {\n            let resume = decode_page_token(token)?;\n            items.iter().position(|item| key(item).as_ref() == resume)?\n        }\n        None => 0,\n    };"

The handler that states the same rule and answers 400 for it (one of ~30 listings on this helper):
  src/ops/list_assets.rs:232-239 — "        let page = crate::state::page_of(\n            &matching,\n            input.next_token.as_deref(), ... )\n        .ok_or_else(invalid_token)?;"
  src/ops/list_assets.rs:257-264 — "/// HTTP 400 `InvalidRequestException` for a `nextToken` this operation never issued.\nfn invalid_token() -> ListAssetsError { ... message: \"The nextToken is not a token this operation issued.\" ... }"

Test coverage: the refusal is pinned only for tokens that are malformed or name a foreign resume point, never for a spelling the service does not mint — tests/all/asset_lifecycle.rs:1168-1191 — "// A `nextToken` that this operation never issued is refused — whether it is not\n// a token at all, or well-formed but naming a resume point outside this\n// enumeration" with the three rows `("not hex at all", "zzz")`, `("hex of an unknown id", hex_token(...))`, `("hex of another model's asset", hex_token(&elsewhere))`. The sibling listing suites (tests/all/list_bulk_import_jobs.rs:96, tests/all/list_datasets.rs:710, tests/all/list_pipelines.rs:532, ...) forge tokens the same three ways, so no test pins acceptance of an uppercase spelling.

## Steps to Reproduce

1. Create an asset model and three assets of it, so ListAssets pages.
2. GET /assets?assetModelId={model}&maxResults=1 -> 200 with a nextToken, e.g. "33316162..." (lowercase hex of the next asset's id).
3. Send the SAME token upper-cased: GET /assets?assetModelId={model}&nextToken=33316162... with every hex letter capitalised (the modelled NextToken pattern ^[A-Za-z0-9+/=]+$ admits it, so the request reaches the handler).
4. Observed: 200 with the next page — the service honours a token spelling `encode_page_token` never mints.
5. Expected per state.rs:6828-6830 ("`None` for text this service never issued as a token") and src/ops/list_assets.rs:257-264: InvalidRequestException (400), the same answer the three forged-token rows in tests/all/asset_lifecycle.rs:1172-1176 get.
6. The same sequence reproduces on every listing that pages through `crate::state::page_of` (ListAssociatedAssets, ListAccessPolicies, ListDatasets, ListPipelines, ...), since all of them decode through the one helper.
