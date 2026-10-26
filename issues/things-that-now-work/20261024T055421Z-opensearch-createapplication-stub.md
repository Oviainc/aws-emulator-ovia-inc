Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.access-denied-precedes-not-found opensearch.application-lifecycle-create-update-delete opensearch.application-tagging-lifecycle opensearch.attach-data-source-accepts-nonexistent-arn-without-attaching opensearch.attach-data-source-duplicate-attachment-reuses-attachment-id opensearch.attach-detach-data-source-race-converges-consistent opensearch.capability-duplicate-registration-is-idempotent opensearch.capability-invalid-name-rejected opensearch.capability-lifecycle opensearch.capability-payload-round-trip opensearch.data-plane-list-data-source-attachments-completeness opensearch.describe-data-source-attachment-not-found-vs-malformed-id-distinct opensearch.disabled-operation-feature-off-unreachable opensearch.e2e-data-source-attachment-ingest-search-detach opensearch.e2e-migration-flow-status-progression opensearch.e2e-migration-full-lifecycle-flow opensearch.e2e-migration-lifecycle-domain-state-consequence opensearch.e2e-migration-start-monitor-flow opensearch.get-migration-without-permission-denied opensearch.list-data-source-attachments-stable-creation-ordering opensearch.ordering-data-source-attachment-listing-order opensearch.overly-permissive-attach-data-source-missing-role-rejected opensearch.overly-permissive-data-source-attach-unauthorized-role-rejected opensearch.representation-unicode-application-name-persists opensearch.start-migration-conflict-when-domain-busy-rejected
Issue-Key: opensearch|CreateApplication|NotImplemented
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# Implement opensearch CreateApplication: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplication

## Description
`CreateApplication` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/create_application.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

25 recorded unit(s) fail on their `CreateApplication` call, so each one unblocks only when `CreateApplication` works: `opensearch.access-denied-precedes-not-found`, `opensearch.application-lifecycle-create-update-delete`, `opensearch.application-tagging-lifecycle`, `opensearch.attach-data-source-accepts-nonexistent-arn-without-attaching`, `opensearch.attach-data-source-duplicate-attachment-reuses-attachment-id`, `opensearch.attach-detach-data-source-race-converges-consistent`, `opensearch.capability-duplicate-registration-is-idempotent`, `opensearch.capability-invalid-name-rejected`, `opensearch.capability-lifecycle`, `opensearch.capability-payload-round-trip`, `opensearch.data-plane-list-data-source-attachments-completeness`, `opensearch.describe-data-source-attachment-not-found-vs-malformed-id-distinct`, `opensearch.disabled-operation-feature-off-unreachable`, `opensearch.e2e-data-source-attachment-ingest-search-detach`, `opensearch.e2e-migration-flow-status-progression`, `opensearch.e2e-migration-full-lifecycle-flow`, `opensearch.e2e-migration-lifecycle-domain-state-consequence`, `opensearch.e2e-migration-start-monitor-flow`, `opensearch.get-migration-without-permission-denied`, `opensearch.list-data-source-attachments-stable-creation-ordering` (+5 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/create_application.rs`. Replaying the corpus, the emulator refused the `CreateApplication` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateApplication operation: operation CreateApplication is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the opensearch emulator
Call CreateApplication and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
