Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: a86005fe1cb54e661d08c01cc116ff714a84de11

# The document-download serving rule is written out twice, once per document plane

_Filed: 2026-10-28T20:13:57Z_
_Operation: GetReport | Class: maintainability_

> _Note: this issue was identified against the code base at commit `a86005fe1cb54e661d08c01cc116ff714a84de11`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate serves two document planes — the report/term plane behind the URL `GetReport` and `GetTermForReport` announce, and the inquiry-export plane behind the URL `ExportComplianceInquiry` announces. Both implement the same five-step rule in full, in two separate copies: refuse anything but `GET`/`HEAD` as a method; answer 500 when the store is unavailable; read the receipt token out of the query string defaulting to the empty string; ask the authorisation predicate and answer `403 AccessDenied` with `INVALID_TOKEN_MESSAGE` BEFORE any existence check; then resolve the object and answer `404 NoSuchKey`; then log and hand off to `document_response`. Only two expressions differ between the copies — which map is looked up and which predicate authorises.

The copies can drift, and they already have: the open issue `20261028T043042Z-artifact-exportcomplianceinquiry-wrong-behavior--r2` is exactly a divergence between the two planes' path handling. Hoisting the shared walk into one helper parameterised by the lookup+authorisation step leaves every status, body, header and log line as it is.

## Evidence

Copy A, the report/term plane — `services/service-com-amazonaws-artifact-artifact/src/service.rs:405-468`:
```
// The same method policy the export plane answers under: a document is READ,
// so `GET` carries it and `HEAD` reports its metadata without the bytes;
// anything else is refused as the method it is.
if !matches!(req.method(), "GET" | "HEAD") {
    return method_not_allowed();
}
...
// DELIBERATE ORDERING: authorization answers first, so a path naming a
// report this service does not hold is refused as unauthorised (403) rather
// than reported as absent
...
if !authorised { ... return document_error(403, "AccessDenied", crate::state::INVALID_TOKEN_MESSAGE); }
let Some(document) = held.filter(|held| held.object == wanted.object) else {
    return document_error(404, "NoSuchKey", DOCUMENT_NOT_FOUND_MESSAGE);
};
```

Copy B, the export plane — `services/service-com-amazonaws-artifact-artifact/src/service.rs:951-1007`:
```
/// DELIBERATE ORDERING, the same the report plane answers under: authorization first,
/// so a path naming an export this store does not hold ... is
/// refused as unauthorised (403) rather than reported as absent
...
if !matches!(req.method(), "GET" | "HEAD") {
    return method_not_allowed();
}
...
if !store.export_download_authorised(&token, target) { ... return document_error(403, "AccessDenied", crate::state::INVALID_TOKEN_MESSAGE); }
let Some(export) = found else {
    return document_error(404, "NoSuchKey", DOCUMENT_NOT_FOUND_MESSAGE);
};
```

The crate names the duplication itself four times: `service.rs:405` ("The same method policy the export plane answers under"), `service.rs:423-424` ("A download is authorised by the SAME rule `GetReport` applies: one rule, one place."), `service.rs:951` ("DELIBERATE ORDERING, the same the report plane answers under") and `service.rs:1003-1004` ("The same serving rule both document planes answer under").

Existing coverage that must stay green and unchanged: `tests/all/document_data_plane.rs`, `tests/all/export_document_authorisation.rs`, `tests/all/compliance_inquiry_export_url_resolves.rs` and `tests/all/report_workflow.rs`.

## Steps to Reproduce

1. Open `services/service-com-amazonaws-artifact-artifact/src/service.rs` and read `Documents::run` (lines 397-469).
2. Read `EmulatedService::serve_export_document` (lines 938-1012).
3. Line up the two: method guard, store-unavailable 500, `query_param(&query, <token param>).unwrap_or_default()`, authorise-then-403, resolve-then-404, tracing, `document_response` — the same sequence twice, differing only in which store is consulted and which predicate authorises.
4. Note the four comments at service.rs:405, 423, 951 and 1003 stating that both planes answer under one rule, and the open issue 20261028T043042Z-artifact-exportcomplianceinquiry-wrong-behavior--r2 recording a place where the two copies already diverged.
