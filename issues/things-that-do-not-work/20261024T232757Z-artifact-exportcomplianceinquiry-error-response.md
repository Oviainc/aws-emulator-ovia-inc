Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: d2256e9f418694a843ad23bc5e22619b2e324aa6

# The inquiry-export document plane serves an export to any caller with no token and no credential, where the sibling report document plane refuses an unauthorised fetch with 403

_Filed: 2026-10-24T23:27:57Z_
_Operation: ExportComplianceInquiry | Class: error-response_

> _Note: this issue was identified against the code base at commit `d2256e9f418694a843ad23bc5e22619b2e324aa6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ExportComplianceInquiry` renders the inquiry (its query text, its tags and its stored feedback comments) into a document and publishes it on this service's own data plane, which is reached from `Service::override_routing_error` BEFORE authentication (that is what lets a presigned URL be followed unsigned). The report/term document plane in the SAME file reads a `termToken` off the URL, checks it with the one `grant_covers` rule and answers `403 AccessDenied` for a fetch that carries no grant. `serve_export_document` performs no such check at all: it never reads the query string, never consults any grant, and the ONLY thing it verifies is that the export id, the inquiry id and the object name agree with a record the store holds. The URL therefore carries nothing a caller has to have been handed: export ids are minted from a monotonic counter (`export-{:016x}` starting at 1) and inquiry ids likewise (`compliance-inquiry-{:016x}` starting at 1), so `/compliance-inquiry/compliance-inquiry-0000000000000001/export/export-0000000000000001/compliance-inquiry-report.json` is derivable without ever having called the control plane, and a GET of it answers 200 with the whole document.

That contradicts this crate's own stated rules twice over: (1) the document-plane refusal is deliberately content-free "so a caller cannot probe for exports it was not handed" — but the ids are enumerable and the fetch needs no authorisation, so probing is not necessary; (2) the control plane goes out of its way NOT to confirm an inquiry's existence (every compliance-inquiry handler answers the front door's 401 rather than the modelled 404 for an id the account holds no inquiry for), while the data plane hands out that same inquiry's full `@sensitive` content to an unauthenticated caller. The handler's own log line records that the inquiry's name, content and queries are `@sensitive` and must never be logged; the export plane publishes all three to anyone.

## Evidence

The sibling plane's rule, services/service-com-amazonaws-artifact-artifact/src/service.rs:296-311 — "// The term grant is what authorises a download, so it is checked first and // by the SAME rule `GetReport` applies: one rule, one place. A fetch // carrying NO token carries no grant, so it is refused before the catalog // is consulted rather than looked up as the empty token." followed by `return document_error(403, "AccessDenied", crate::state::INVALID_TOKEN_MESSAGE);` — and src/service.rs:355-358: "/// Served UNSIGNED. A document URL is handed to a caller to fetch directly, so the /// fetch carries no SigV4 `Authorization` header; the term grant the URL carries is /// what authorises it, and a fetch without one is refused."

The export plane, src/service.rs:759-781, applies no credential or token check at all (the request's query string is never parsed here):
    let found = store.inquiry_exports.get(&target.export_id).filter(|export| {
        export.inquiry_id == target.inquiry_id
            && target.object == crate::state::INQUIRY_EXPORT_OBJECT
    });
    let Some(export) = found else { return document_error(404, "NoSuchKey", DOCUMENT_NOT_FOUND_MESSAGE); };
and it is reached pre-authentication by construction, src/service.rs:82-85: "- [`Service::override_routing_error`] serves it. ... and reaches it BEFORE authentication — which is what lets a presigned URL be followed without a signature".

The rule the enumerable ids break, src/service.rs:793-795: "/// The body of a document-plane refusal that found no document: one sentence carrying /// nothing from the request, so a caller cannot probe for exports it was not handed." Against src/state.rs:1538-1544 `let candidate = format!("export-{:016x}", self.inquiry_export_seq);` with `inquiry_export_seq: 0` at src/state.rs:1351, and src/state.rs:1392 `let candidate = format!("compliance-inquiry-{:016x}", self.inquiry_seq);` with `inquiry_seq: 0` at src/state.rs:1344 — so the first inquiry/export pair is always `compliance-inquiry-0000000000000001` / `export-0000000000000001`.

What is disclosed, src/state.rs:1080-1120 (`export_document`) writes `"query": query.query`, `"tags": self.tags` and `feedback` entries carrying `"comment": entry.comment`; src/ops/export_compliance_inquiry.rs:248-249 states these are "NEVER the inquiry's name, content or queries — all `@sensitive` in the model — and never the URL, whose `PresignedUrl` shape is `@sensitive` too". The control plane's refusal for an inquiry the caller may not read, src/ops/export_compliance_inquiry.rs:185-191: "a well-formed id this account holds no inquiry for names no // inquiry it may export, so the front door's refusal is what it sees" + `return Err(crate::scaffolded::Scaffolded::unauthorized());`

Test coverage: tests/all/compliance_inquiry_export.rs:178-188 `follow()` fetches the announced URL "with NO credential of any kind", and tests/all/document_data_plane.rs covers the report plane's token refusals; no test fetches an export URL the caller was never handed, so nothing pins the 200 this plane answers for it.

## Steps to Reproduce

1. Boot the emulator with only this service and POST /v1/compliance-inquiry/create with {"name":"soc2","inquiryContent":{"query":"Does the SOC 2 report cover eu-west-1?"}} — the minted id is compliance-inquiry-0000000000000001.
2. POST /v1/compliance-inquiry/export with {"complianceInquiryId":"compliance-inquiry-0000000000000001"} to produce export-0000000000000001.
3. From a caller that sent neither of the two calls above and presents NO Authorization header, GET /compliance-inquiry/compliance-inquiry-0000000000000001/export/export-0000000000000001/compliance-inquiry-report.json with Host: artifact-documents.us-east-1.amazonaws.com: the emulator answers 200 with the full export document, including the inquiry's query text, its tags and any feedback comment.
4. Repeat step 3 with a guessed-forward id pair (compliance-inquiry-0000000000000002 / export-0000000000000002 after a second create+export): it is served the same way, so the document plane requires nothing the caller was handed.
5. Contrast the report plane: GET /v1/report/download/report/report-073F0fUCdVVNKJrF/v1/AWS-Outposts-Rack-Information-Security-FAQ.pdf with no termToken query parameter is refused 403 AccessDenied ("The provided token is invalid."), which is the rule this crate says both planes answer under.
