Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild.codebuild-junit-report-ingestion-reports-each-test-case-with-its-exact-status codebuild.delete-resource-policy-absent-policy-rejected codebuild.describe-test-cases-absent-report-not-found codebuild.get-resource-policy-absent-resource-not-found codebuild.list-report-groups-reflects-created codebuild.list-reports-for-report-group-sort-order-consistent codebuild.put-resource-policy-malformed-document-rejected codebuild.put-resource-policy-write-action-rejected codebuild.report-group-create-get-delete codebuild.report-group-tags-roundtrip codebuild.report-group-trend-empty-state-read codebuild.report-group-trend-out-of-range-rejected codebuild.report-group-trend-reflects-created-reports codebuild.report-group-trend-reflects-report-data codebuild.report-group-update-export-config codebuild.resource-policy-crud-roundtrip codebuild.resource-policy-delete-then-get-not-found codebuild.resource-policy-invalid-document-rejected codebuild.resource-policy-put-get-delete-roundtrip codebuild.resource-policy-put-get-round-trip codebuild.resource-policy-roundtrip-report-group codebuild.resource-policy-sharing-not-listed-for-owner
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild|CreateReportGroup|NotImplemented
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Implement codebuild CreateReportGroup: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateReportGroup

## Description
`CreateReportGroup` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/create_report_group.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

22 recorded unit(s) fail on their `CreateReportGroup` call, so each one unblocks only when `CreateReportGroup` works: `codebuild.codebuild-junit-report-ingestion-reports-each-test-case-with-its-exact-status`, `codebuild.delete-resource-policy-absent-policy-rejected`, `codebuild.describe-test-cases-absent-report-not-found`, `codebuild.get-resource-policy-absent-resource-not-found`, `codebuild.list-report-groups-reflects-created`, `codebuild.list-reports-for-report-group-sort-order-consistent`, `codebuild.put-resource-policy-malformed-document-rejected`, `codebuild.put-resource-policy-write-action-rejected`, `codebuild.report-group-create-get-delete`, `codebuild.report-group-tags-roundtrip`, `codebuild.report-group-trend-empty-state-read`, `codebuild.report-group-trend-out-of-range-rejected`, `codebuild.report-group-trend-reflects-created-reports`, `codebuild.report-group-trend-reflects-report-data`, `codebuild.report-group-update-export-config`, `codebuild.resource-policy-crud-roundtrip`, `codebuild.resource-policy-delete-then-get-not-found`, `codebuild.resource-policy-invalid-document-rejected`, `codebuild.resource-policy-put-get-delete-roundtrip`, `codebuild.resource-policy-put-get-round-trip` (+2 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/create_report_group.rs`. Replaying the corpus, the emulator refused the `CreateReportGroup` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateReportGroup operation: operation CreateReportGroup is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the codebuild emulator
Call CreateReportGroup and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
