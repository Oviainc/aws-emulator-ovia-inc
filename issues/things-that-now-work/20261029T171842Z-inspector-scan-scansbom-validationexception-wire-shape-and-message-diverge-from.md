Issue-Source: trace-sweep-divergence
Issue-Trace: inspector-scan-error-sbom-cannot-parse
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: inspector-scan-error-sbom-cannot-parse inspector-scan-error-sbom-missing-required-fields inspector-scan-error-sbom-unsupported-type
Issue-Shape-Id: com.amazonaws.inspectorscan#InspectorScan
Issue-Key: inspector-scan|ScanSbom|4xx->4xx|$.fields
Issue-Key: inspector-scan|ScanSbom|4xx->4xx|$.message
Issue-Key: inspector-scan|ScanSbom|4xx->4xx|$.reason
Issue-Commit: daf74f430223eff1dc28404b54e4f3c7221767a2

# ScanSbom ValidationException wire shape and message diverge from recorded Inspector Scan refusals

> _Note: this issue was identified against the code base at commit `daf74f430223eff1dc28404b54e4f3c7221767a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ScanSbom

## Description
Single cause (Cause 1 of 1): every ScanSbom input refusal is serialized with the emulator's own prose and with members the recorded service does not send.

AWS: ScanSbom refuses a bad SBOM document with HTTP 400 ValidationException (Smithy: ScanSbom errors include ValidationException @httpError 400; ValidationException members message [required], reason -> ValidationExceptionReason {UNKNOWN_OPERATION, CANNOT_PARSE, FIELD_VALIDATION_FAILED, UNSUPPORTED_SBOM_TYPE, OTHER}, fields -> ValidationExceptionFields). Recorded behavior only for the wire body: all three recorded refusals (an unparseable document, a document missing required fields, an unsupported SBOM type) carry a body whose ONLY member is `message`, phrased as the fixed text "Input SBOM failed validation: " followed by the condition that failed — for the unparseable-document recording the message is exactly 'Input SBOM failed validation: Invalid json'. Neither `reason` nor `fields` is present in the recorded body, even though the model marks `reason` required. The fixed part of the message is "Input SBOM failed validation: "; the trailing clause is the variable part and is decided by the class of the input defect, not by a value copied out of the request.

Emulator: the status is right (400) but the body is a different shape and a different sentence. For the unparseable-document step the emulator answers message 'sbom must be a CycloneDX 1.5 JSON object', reason 'CANNOT_PARSE' and fields [{name: 'sbom', message: 'sbom must be a CycloneDX 1.5 JSON object'}]; the recording has message 'Input SBOM failed validation: Invalid json' and no reason/fields members at all. The other two recorded refusals in the same cluster (missing required fields, unsupported SBOM type) diverge on exactly the same three keys ($.message, $.reason, $.fields).

Why: a client that matches on the refusal text, or that logs/branches on the absence of `reason`/`fields` the real service does not populate, sees a different error contract than AWS gives it — the emulator's sentence names an emulator-internal member path instead of the service's "Input SBOM failed validation: …" wording, so error-message assertions and user-facing surfacing of the failure are both wrong.

Fix: the operation's validation/error channel — the ValidationException constructor used by every refusal in the ScanSbom handler, which currently always fills reason and a one-element fields list and passes the internal member-path sentence through as the message. It must emit the service's "Input SBOM failed validation: <condition>" wording and must not add members the service does not send.

Traces: inspector-scan-error-sbom-cannot-parse#task#1 (representative), plus the same cluster's inspector-scan-error-sbom-missing-required-fields#task#1 and inspector-scan-error-sbom-unsupported-type#task#1. Code: services/service-com-amazonaws-inspectorscan-inspectorscan/src/ops/scan_sbom.rs — `fn invalid` (~L239-253, the "every refusal fills all three" comment) and the parse refusal at ~L866-872.

## Evidence
Replay cluster service=inspector-scan|ScanSbom|status=400->400|keys=$.fields,$.message,$.reason — expected_status 400, actual_status 400, 3 affected steps.
detail:
  $.message: value_mismatch (expected 'Input SBOM failed validation: Invalid json', got 'sbom must be a CycloneDX 1.5 JSON object')
  $.reason: type_mismatch (expected None, got 'CANNOT_PARSE')
  $.fields: type_mismatch (expected None, got [{'name': 'sbom', 'message': 'sbom must be a CycloneDX 1.5 JSON object'}])

## Steps to Reproduce
Replay the wire corpus (trace set simparity-wire-aws-v1) against the emulator and inspect fixture inspector-scan-error-sbom-cannot-parse, step task#1: POST /scan/sbom with a `sbom` document that is not a CycloneDX 1.5 JSON object. The response is HTTP 400, but the JSON body carries message 'sbom must be a CycloneDX 1.5 JSON object' plus reason and fields, where the recording carries only message 'Input SBOM failed validation: Invalid json'. The same applies to inspector-scan-error-sbom-missing-required-fields#task#1 and inspector-scan-error-sbom-unsupported-type#task#1.
