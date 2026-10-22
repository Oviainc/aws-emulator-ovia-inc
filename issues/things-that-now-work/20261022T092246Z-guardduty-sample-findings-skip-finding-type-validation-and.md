Issue-Source: trace-sweep-divergence
Issue-Trace: guardduty-error-handling-invalid-inputs
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: guardduty-error-handling-invalid-inputs guardduty-sample-findings-generation-and-retrieval guardduty-list-findings-pagination-and-criteria
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty|CreateSampleFindings|4xx->2xx|$
Issue-Key: guardduty|CreateSampleFindings|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: guardduty|CreateSampleFindings|4xx->2xx|status
Issue-Key: guardduty|GetFindings|2xx->2xx|$.findings[*].service.additionalInfo.type
Issue-Key: guardduty|GetFindings|2xx->2xx|$.findings[*].service.additionalInfo.value
Issue-Key: guardduty|GetFindings|2xx->2xx|$.findings[*].severity
Issue-Commit: 0b8884c4ab978d23b4fe746f795a1277a2a36df3

# GuardDuty sample findings skip finding-type validation and are generated with wrong additionalInfo and severity

> _Note: this issue was identified against the code base at commit `0b8884c4ab978d23b4fe746f795a1277a2a36df3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSampleFindings

## Description
The sample-finding path diverges three ways: the request is not validated at all, and the generated finding's additionalInfo and severity do not match what AWS produces. Smithy: CreateSampleFindings {detectorId (httpLabel, required), findingTypes: FindingTypes (list of FindingType, length 0..50, member length 1..100)} -> empty output, errors BadRequestException(400) / InternalServerErrorException(500); GetFindings -> Findings; Finding.severity is a required Double; Service.additionalInfo is a ServiceAdditionalInfo {value, type}.

Cause 1 of 3: CreateSampleFindings accepts an unmodeled finding type and succeeds. AWS: a findingTypes entry that is not a real GuardDuty finding type is rejected with HTTP 400, header x-amzn-errortype: BadRequestException, body {"message": "The request is rejected because of invalid finding type is specified.", "__type": "InvalidInputException", "type": "InvalidInputException"} — the model constrains only the string length, so the type-catalog check is recorded behavior, but the 400 status and the error class are what the recording shows. Emulator: HTTP 200 with an empty body and no x-amzn-errortype header, i.e. the invalid type is silently accepted. Why: a client testing its finding-type handling gets a false success and (worse) a detector seeded with a bogus finding type; any negative test of this validation passes against the emulator and fails against AWS. Fix: CreateSampleFindings input validation (finding-type catalog check) and its error path. Traces: guardduty-error-handling-invalid-inputs#task#3.

Cause 2 of 3: generated sample findings carry the wrong service.additionalInfo. AWS: recorded behavior only — a sample finding reports service.additionalInfo = {"value": "{\"sample\":true}", "type": "default"}, i.e. `value` is a JSON document string and `type` is "default". Emulator: value is the bare string 'true' and type is 'SAMPLE'. Why: a client that JSON-parses additionalInfo.value (the documented shape is an opaque document string) fails to parse 'true' as the sample marker object, and a client that branches on additionalInfo.type sees a value AWS never sends. Fix: the sample-finding generator (the Service/additionalInfo projection). Traces: guardduty-sample-findings-generation-and-retrieval#task#4.

Cause 3 of 3: generated finding severity is wrong for the finding type. AWS: recorded behavior only — a sample finding is generated with the canonical severity of its finding type, 9.0 in the recorded case. Emulator: 8.0. Hypothesis: the emulator uses one fixed severity rather than the per-finding-type severity, so severity-based filtering and sorting diverge. Why: a client filtering or sorting findings by severity threshold (e.g. >= 8.5 for high) classifies the finding differently than on AWS. Fix: the sample-finding generator's severity derivation (per finding type). Traces: guardduty-list-findings-pagination-and-criteria#task#5.

## Evidence
Replay of simparity-wire-aws-v1.

Cause 1 — guardduty-error-handling-invalid-inputs#task#3, CreateSampleFindings, expected 400 / actual 200:
  status: expected 400, got 200
  $: empty_body (expected {'message': 'The request is rejected because of invalid finding type is specified.', '__type': 'InvalidInputException', 'type': 'InvalidInputException'}, got {})
  $headers.x-amzn-errortype: missing (expected 'BadRequestException', got None)

Cause 2 — guardduty-sample-findings-generation-and-retrieval#task#4, GetFindings, 200/200:
  $.findings[0].service.additionalInfo.value: value_mismatch (expected '{"sample":true}', got 'true')
  $.findings[0].service.additionalInfo.type: value_mismatch (expected 'default', got 'SAMPLE')

Cause 3 — guardduty-list-findings-pagination-and-criteria#task#5, GetFindings, 200/200:
  $.findings[0].severity: value_mismatch (expected 9.0, got 8.0)

Finding ids, arns and timestamps in the recordings are environment-specific and are not part of the requirement; the 400 status + error class, the additionalInfo value/type shape and the per-finding-type severity are.

## Steps to Reproduce
1. Boot the emulator built from mainline and replay the wire corpus (simparity-wire-aws-v1).
2. guardduty-error-handling-invalid-inputs#task#3: POST /detector/{detectorId}/findings/create with findingTypes containing a non-existent finding type — the emulator answers 200 with an empty body instead of 400 InvalidInputException.
3. guardduty-sample-findings-generation-and-retrieval#task#4: CreateSampleFindings, then ListFindings + POST /detector/{detectorId}/findings/get and read findings[0].service.additionalInfo — value is 'true'/type 'SAMPLE' instead of '{"sample":true}'/'default'.
4. guardduty-list-findings-pagination-and-criteria#task#5: same GetFindings read — findings[0].severity is 8.0 instead of the finding type's 9.0.
