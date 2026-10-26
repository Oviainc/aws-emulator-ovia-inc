Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2a22026c0f737b6187455fb636429eae77d015a7

# ExportComplianceInquiry announces a documentPresignedUrl on an amazonaws.com host that does not resolve back to the emulator, so the export can never be downloaded

_Filed: 2026-10-24T16:46:19Z_
_Operation: ExportComplianceInquiry | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2a22026c0f737b6187455fb636429eae77d015a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ExportComplianceInquiry` answers 200 with `documentPresignedUrl`, and a presigned URL is a promise that the caller can fetch it. The emulator mints that URL with an AWS-shaped host it does not own:

    https://artifact-documents.<region>.amazonaws.com/compliance-inquiry/<inquiryId>/export/<exportId>/compliance-inquiry-export.json

(`crate::state::inquiry_export_url(&region, ..)`, built from the credential-scope region, not from the endpoint the caller reached).

Expected (AWS, and the rule this crate already applies to the report/term document plane): the URL the control plane hands out resolves to a 200 document for whoever was handed it. For `GetReport` / `GetTermForReport` the crate does exactly this — `crate::state::document_url_host(ctx.parts())` interpolates the caller's own `Host`/authority (falling back to a loopback literal) precisely "because the URL has to resolve for whoever was handed it — a client addressing the emulator by socket is handed that socket" — and the already-fixed issue 20261022T012757Z (cause 5, "the documentPresignedUrl returned by GetReport does not serve a document") was closed on that exact requirement.

Actual: a client that follows the export URL leaves the emulator entirely. `artifact-documents.<region>.amazonaws.com` is not the emulator's socket, so `curl`/`urllib` resolution either fails (NXDOMAIN / connection error) or is sent to the public internet; it never reaches the stored export bytes. The export document plane is reachable ONLY if the request is re-addressed to the emulator socket by hand while keeping `Host: artifact-documents.<region>.amazonaws.com`, because `Service::can_handle` claims the export path by HOST (`crate::state::is_document_host`) while the report/term plane is additionally claimed by PATH (`parse_document_path`). A caller that sends the announced path to the emulator's own endpoint gets the router's unknown-service answer instead of the document.

Net effect: the inquiry-export workflow (create inquiry -> export -> download the exported document) looks like success (200 + a URL) and then dead-links — the same failure class already filed and fixed for the report download path, reintroduced on the export path.

## Evidence

Spec: `ExportComplianceInquiryResponse$documentPresignedUrl` is documented "Presigned S3 URL to access the exported compliance inquiry report" (model shape `PresignedUrl`), i.e. a URL the caller fetches.

Handler: services/service-com-amazonaws-artifact-artifact/src/ops/export_compliance_inquiry.rs lines ~259-264 return `crate::state::inquiry_export_url(&region, &input.compliance_inquiry_id, &export_id)`; src/state.rs `inquiry_export_url` formats `https://{DOCUMENT_HOST}.{region}.amazonaws.com/...` with `DOCUMENT_HOST = "artifact-documents"`, ignoring the request's endpoint. Contrast src/state.rs `document_url_host(parts)` + `report_document_url` / `term_document_url`, which GetReport and GetTermForReport use and which name the caller's own authority. src/service.rs `can_handle` claims the export URL only via `request_host(parts).is_some_and(crate::state::is_document_host)`; `parse_document_path` (the path-based claim) covers only `DOCUMENT_PATH_PREFIX`, not `/compliance-inquiry/<id>/export/...`.

Tests: tests/all/compliance_inquiry_export.rs `follow()` (lines ~178-206) splits the announced URL and re-dispatches it into the in-process router with `host` set to the announced AWS host, so the suite passes without the URL ever having to resolve for an out-of-process client. No test asserts that the announced export URL is fetchable from the endpoint the caller reached. Precedent that followability is the contract: fixed issue 20261022T012757Z-artifact-report-and-compliance-inquiry-error-paths-and, cause 5 and its resolution note (documents "served from state::DOCUMENT_PATH_PREFIX under the endpoint the caller reached, so the documentPresignedUrl both operations announce resolves").

## Steps to Reproduce

1. Boot the emulator and POST /v1/compliance-inquiry/create with {"name":"soc2","inquiryContent":{"query":"Does the SOC 2 report cover eu-west-1?"}}; note complianceInquirySummary.id.
2. POST /v1/compliance-inquiry/export with {"complianceInquiryId":"<id>"} and read documentPresignedUrl from the 200 response: it names https://artifact-documents.<region>.amazonaws.com/compliance-inquiry/<id>/export/<exportId>/compliance-inquiry-export.json.
3. Fetch that URL with an ordinary HTTP client (curl/urllib) from the same place the SDK call was made: the host is not the emulator, so the fetch fails to resolve/connect instead of answering 200 with the export document.
4. Re-send the same path to the emulator's own endpoint (GET http://<emulator-host:port>/compliance-inquiry/<id>/export/<exportId>/compliance-inquiry-export.json with the emulator's Host header): the request is not claimed by the service and is answered as an unrouted/unknown request rather than with the document.
5. For contrast run ListReports -> GetTermForReport -> GetReport and fetch the returned documentPresignedUrl: that URL names the caller's own endpoint and answers 200 with the document, showing the export path diverges from the rule the report path already follows.
