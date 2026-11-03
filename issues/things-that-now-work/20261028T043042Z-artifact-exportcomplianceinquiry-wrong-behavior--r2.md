Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 21d30b53374928fd663edcc8ae486344ab510a4e

# The inquiry-export document plane compares raw path segments, so a percent-encoded spelling of the URL it announced answers 404 where the sibling report plane serves it

_Filed: 2026-10-28T04:30:42Z_
_Operation: ExportComplianceInquiry | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `21d30b53374928fd663edcc8ae486344ab510a4e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate publishes TWO document planes, and the crate states they answer under ONE convention. Both are reached by claiming a path (`Service::can_handle`), and each has its own path parser:

* the report/term plane's parser (`parse_document_path`, src/service.rs:487-505) percent-DECODES every segment it extracts before it is used to look a document up;
* the inquiry-export plane's parser (`crate::state::parse_export_document_path`, src/state.rs:1354-1376) keeps every segment exactly as the request spelled it, and `serve_export_document` then compares those RAW segments against the stored record (`export.inquiry_id == target.inquiry_id && target.object == crate::state::INQUIRY_EXPORT_OBJECT`, src/service.rs:956-959).

Consequence: a fetch of the very URL `ExportComplianceInquiry` announced, written with any segment percent-encoded (e.g. the object as `compliance-inquiry-report%2Ejson`, or the inquiry id as `compliance%2Dinquiry-0000000000000001`) — a spelling that is equivalent per RFC 3986 and that a client, SDK or proxy may produce — matches no export and is answered `404 NoSuchKey`, while the same spelling on the report plane is decoded and served. The export plane therefore serves its documents under a second convention, which is exactly what the shared serving helper's doc says this service does not do, and it skips a decoding helper the crate already uses one parser over (`smithy_server_runtime::percent_decode`). The crate elsewhere treats a percent-encoded path spelling as naming the same resource and has a test for it on the tag operations; nothing in the crate states a reason for the export plane to differ, and no test pins the raw comparison.

This is NOT the already-open export-plane issue (that one is about the plane requiring no token/credential) and NOT the already-fixed export-URL issue (that one was about the announced HOST not resolving): the host now names the caller's endpoint, and the defect here is the PATH spelling the resolved plane accepts.

## Evidence

Sibling plane decodes — services/service-com-amazonaws-artifact-artifact/src/service.rs:499-504:
```
    Some(WantedDocument {
        kind: smithy_server_runtime::percent_decode(kind),
        report_id: smithy_server_runtime::percent_decode(report_id),
        version,
        object: smithy_server_runtime::percent_decode(object),
    })
```

Export plane does not — services/service-com-amazonaws-artifact-artifact/src/state.rs:1371-1375:
```
    Some(ExportDocumentTarget {
        inquiry_id: inquiry_id.to_string(),
        export_id: export_id.to_string(),
        object: object.to_string(),
    })
```
and the raw comparison it feeds, src/service.rs:956-959:
```
        let found = store.inquiry_exports.get(&target.export_id).filter(|export| {
            export.inquiry_id == target.inquiry_id
                && target.object == crate::state::INQUIRY_EXPORT_OBJECT
        });
```
with the refusal at src/service.rs:960-962: `return document_error(404, "NoSuchKey", DOCUMENT_NOT_FOUND_MESSAGE);`

The rule the divergence breaks — src/service.rs:507-511: "/// Serve one stored document: its own bytes, under its own content type and length. … BOTH document planes of this service answer through here, so one service does not serve its documents under two conventions." and src/service.rs:540-542: "It is the one path reader in this module: both document planes (the report documents below and the inquiry exports above) resolve their path through it."

The crate's own position on percent-encoded path spellings — services/service-com-amazonaws-artifact-artifact/tests/all/resource_tags.rs:423-443: "/// Rule 1, on the spelling of the label: the ARN an SDK percent-encodes whole names the /// same resource as the one that leaves `:` alone." … `"case {case:?}: both spellings of the label must resolve the same resource"`.

Test coverage of the gap: tests/all/compliance_inquiry_export_url_resolves.rs exercises only the literal announced path (`an_announced_export_url_names_the_caller_s_endpoint_and_serves_the_document`, line 253) and tests/all/document_data_plane.rs only the literal report paths (`/v1/report/download/report/{id}/v{version}/...`, lines 432-438); a grep for `%2E`/`%2D`/`percent` over tests/all finds hits only in the tag suites, so no test asserts either behaviour for a document path and nothing justifies the divergence.

## Steps to Reproduce

1. POST /v1/compliance-inquiry/create with {"name":"soc2","inquiryContent":{"query":"Does the SOC 2 report cover eu-west-1?"}} and note complianceInquirySummary.id.
2. POST /v1/compliance-inquiry/export with {"complianceInquiryId":"<id>"} and read documentPresignedUrl: https://<caller endpoint>/v1/compliance-inquiry/download/<inquiryId>/export/<exportId>/compliance-inquiry-report.json.
3. GET that URL verbatim: 200 with the export document (the current test does exactly this).
4. GET the same URL with one segment written in an equivalent percent-encoded spelling, e.g. .../download/<inquiryId>/export/<exportId>/compliance-inquiry-report%2Ejson (or compliance%2Dinquiry-... for the id): the plane answers 404 NoSuchKey, because parse_export_document_path hands serve_export_document the raw segment and it is compared to INQUIRY_EXPORT_OBJECT unchanged.
5. For contrast, run ListReports -> GetTermForReport -> GetReport, take the announced report URL and re-spell one segment the same way (e.g. AWS%2DOutposts-Rack-Information-Security-FAQ.pdf): parse_document_path percent-decodes it and the document is served 200, so the two document planes of one service disagree about which spellings of a path they publish.
