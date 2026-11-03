Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: observabilityadmin.validate-telemetry-pipeline-configuration-malformed-yaml-rejected
Issue-Shape-Id: com.amazonaws.observabilityadmin#ObservabilityAdmin
Issue-Key: observabilityadmin.validate-telemetry-pipeline-configuration-malformed-yaml-rejected|run#17|AssertionError|assert processor_error is not None, (
Issue-Commit: 99b5a6d96e663b4229db53d5b35b1ad38a3d9b5d

# ValidateTelemetryPipelineConfiguration reports an unknown processor as Reason=UNSUPPORTED_PROCESSOR with an emulator-authored message instead of UNKNOWN_PROPERTY

> _Note: this issue was identified against the code base at commit `99b5a6d96e663b4229db53d5b35b1ad38a3d9b5d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ValidateTelemetryPipelineConfiguration

## Description
Cause 1 of 1: the validation-error classification for an unrecognized processor name in a telemetry pipeline configuration body is wrong, and its message is emulator-authored rather than AWS-shaped.

AWS: Smithy (services/service-com-amazonaws-observabilityadmin-observabilityadmin/smithy-model/com-amazonaws-observabilityadmin-observabilityadmin.json) models ValidateTelemetryPipelineConfiguration as POST /ValidateTelemetryPipelineConfiguration -> 200 with output member Errors: ValidationErrors (list of ValidationError{Message: String, Reason: String, FieldMap: map}). The model does not enumerate the Reason vocabulary (Reason is a bare String, doc: "The reason code or category for the validation error"), so this is recorded behavior only: the recording shows that a configuration body whose `processor` list names a processor the pipeline schema does not define is a content-level schema mismatch reported, in the same 200 Errors list, with Reason=UNKNOWN_PROPERTY and a Message that names the offending property (the processor name taken verbatim from the submitted body), pinned by FieldMap to the processor entry that named it (path = `processor[<index>]`, the index taken from the request body). The sibling classes in the same recording follow the same shape: unparseable YAML -> exactly one Reason=PIPELINE_CONFIGURATION_PARSE_ERROR, and a non-ARN-shaped `aws.sts_role_arn` -> Reason=INVALID_PATTERN pinned to the sts_role_arn path. Those two pass; only the unknown-processor class diverges.

Emulator: for the same body it returns 200 with a single error whose Reason is `UNSUPPORTED_PROCESSOR` — a reason code AWS does not use — and whose Message is `the pipeline configuration names the processor '<processor-name>', which this emulator does not run; it runs [<comma-separated list of processors the emulator implements>]`. The FieldMap path (`processor[0]`, index from the request) is correct; the Reason and the Message text are not. The message's fixed text is entirely emulator-authored: it describes the emulator's own capability set ("which this emulator does not run; it runs [...]") rather than stating that the configuration names an unknown property. The only request-derived variable part it carries is the processor name.

Why: a client that classifies validation feedback by Reason (the documented reason code/category) never sees UNKNOWN_PROPERTY and so cannot distinguish an unknown-property schema mismatch from the other reason classes; code switching on the reason code falls through to an unknown-reason branch, and tooling that surfaces the Message to a user leaks emulator-implementation prose ("this emulator") and the emulator's processor inventory, which is not part of the AWS contract and will differ from the real service's supported set.

Fix: the ValidateTelemetryPipelineConfiguration handler's pipeline-configuration schema validator, where processor entries are resolved against the known-processor set and a ValidationError is constructed. It must emit Reason=UNKNOWN_PROPERTY for a processor name the schema does not define, and a Message in AWS's wording for the unknown-property class whose variable part is the offending processor name taken from the request body — not a description of the emulator's own supported-processor list. Keep the existing FieldMap path derivation (`processor[<index>]`), which already matches.

Traces: observabilityadmin.validate-telemetry-pipeline-configuration-malformed-yaml-rejected.

## Evidence
Check: observabilityadmin.validate-telemetry-pipeline-configuration-malformed-yaml-rejected [mismatch]

Request (3rd call in the check) — ValidateTelemetryPipelineConfiguration with Configuration.Body:
  pipeline:
    source:
      cloudwatch_logs:
        log_event_metadata:
          data_source_name: "sim2real_src"
          data_source_type: "default"
        aws:
          sts_role_arn: "arn:aws:iam::<account>:role/<role-name>"
    processor:
      - not_a_real_processor_xyz: {}
    sink:
      - cloudwatch_logs:
          log_group: "@original"

Expected (AWS, per the recording): HTTP 200, Errors contains an entry with
  Reason = "UNKNOWN_PROPERTY"
  Message = AWS's unknown-property wording naming the offending processor (fixed text from AWS; variable part = the processor name from the request, here `not_a_real_processor_xyz`)
  FieldMap = {"path": "processor[0]"}   (index from the request body)

Actual (emulator): HTTP 200, Errors = [{
  'Message': "the pipeline configuration names the processor 'not_a_real_processor_xyz', which this emulator does not run; it runs [csv, parse_json, add_entries, delete_entries, rename_keys, copy_values, lowercase_string, uppercase_string, trim_string, substitute_string, split_string]",
  'Reason': 'UNSUPPORTED_PROCESSOR',
  'FieldMap': {'path': 'processor[0]'}
}]

Assertion raised:
  AssertionError: expected an UNKNOWN_PROPERTY error naming the bad processor, got [{'Message': "the pipeline configuration names the processor 'not_a_real_processor_xyz', which this emulator does not run; it runs [...]", 'Reason': 'UNSUPPORTED_PROCESSOR', 'FieldMap': {'path': 'processor[0]'}}]

Diverging fields: Reason ('UNSUPPORTED_PROCESSOR' vs 'UNKNOWN_PROPERTY') and the Message's fixed text (emulator capability prose vs AWS unknown-property wording). HTTP status (200) and FieldMap.path are correct. The same check's PIPELINE_CONFIGURATION_PARSE_ERROR and INVALID_PATTERN assertions, and the valid-configuration positive control, all pass.

## Steps to Reproduce
1. Call observabilityadmin ValidateTelemetryPipelineConfiguration with Configuration={"Body": <a structurally valid pipeline YAML whose `processor` list's first entry names a processor the schema does not define, e.g. `not_a_real_processor_xyz: {}`, and whose source/sink/sts_role_arn are otherwise valid>}.
2. Inspect the 200 response's Errors list.
3. Observe the single entry carries Reason='UNSUPPORTED_PROCESSOR' and an emulator-authored Message ("...which this emulator does not run; it runs [...]"), instead of Reason='UNKNOWN_PROPERTY' with AWS's unknown-property message naming the offending processor. FieldMap.path is 'processor[0]' in both.
4. For contrast, repeat with (a) unparseable YAML (e.g. `pipeline:\n  source: [unterminated\n  processor`) -> one Reason=PIPELINE_CONFIGURATION_PARSE_ERROR, and (b) sts_role_arn='not-a-valid-arn-at-all' -> Reason=INVALID_PATTERN pinned to the sts_role_arn path; both already match AWS, showing the divergence is isolated to the unknown-processor class.

Replay: run_behavioral_suite, check observabilityadmin.validate-telemetry-pipeline-configuration-malformed-yaml-rejected.
