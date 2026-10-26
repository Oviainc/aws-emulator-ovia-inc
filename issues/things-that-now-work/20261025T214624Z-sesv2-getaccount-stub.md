Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2.account-sending-attributes-toggle sesv2.account-suppression-attributes-state sesv2.batch-get-metric-data-invalid-metric-namespace-rejected sesv2.batch-get-metric-data-unknown-resource-not-found sesv2.disabling-account-vdm-withdraws-the-metrics-surface-then-enabling-restores-it sesv2.export-job-lifecycle sesv2.export-job-message-insights-completes-and-carries-its-destination sesv2.get-message-insights-unknown-message-id-rejected sesv2.list-export-jobs-filters-partition-the-jobs-this-check-created sesv2.metrics-export-presigned-url-serves-a-file-whose-shape-derives-from-the-request sesv2.put-account-pricing-attributes-conflict-rejected sesv2.suppressed-destination-delete-then-get-not-found sesv2.suppressed-destination-invalid-reason-rejected sesv2.suppressed-destination-put-get-delete-lifecycle
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2|GetAccount|NotImplemented
Issue-Commit: ca265ed85df5a1b6b7b5ff6c2c813551ab1096d8

# Implement sesv2 GetAccount: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ca265ed85df5a1b6b7b5ff6c2c813551ab1096d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccount

## Description
`GetAccount` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/get_account.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

14 recorded unit(s) fail on their `GetAccount` call, so each one unblocks only when `GetAccount` works: `sesv2.account-sending-attributes-toggle`, `sesv2.account-suppression-attributes-state`, `sesv2.batch-get-metric-data-invalid-metric-namespace-rejected`, `sesv2.batch-get-metric-data-unknown-resource-not-found`, `sesv2.disabling-account-vdm-withdraws-the-metrics-surface-then-enabling-restores-it`, `sesv2.export-job-lifecycle`, `sesv2.export-job-message-insights-completes-and-carries-its-destination`, `sesv2.get-message-insights-unknown-message-id-rejected`, `sesv2.list-export-jobs-filters-partition-the-jobs-this-check-created`, `sesv2.metrics-export-presigned-url-serves-a-file-whose-shape-derives-from-the-request`, `sesv2.put-account-pricing-attributes-conflict-rejected`, `sesv2.suppressed-destination-delete-then-get-not-found`, `sesv2.suppressed-destination-invalid-reason-rejected`, `sesv2.suppressed-destination-put-get-delete-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/get_account.rs`. Replaying the corpus, the emulator refused the `GetAccount` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetAccount operation: operation GetAccount is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the sesv2 emulator
Call GetAccount and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
