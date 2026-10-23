Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.concurrent-invalidation-caller-reference-dedup cloudfront.create-invalidation-via-cli-lands-the-same-record-as-the-sdk cloudfront.distribution-id-resolves-from-its-domain-name-across-pages cloudfront.invalidation-list-ordering-most-recent-first cloudfront.list-invalidations-pagination-and-completeness cloudfront.overly-permissive-invalidation-inconsistent-quantities-rejected cloudfront__events.cfn-deploy-eventbridge-rule-cloudfront-invalidation-lambda cloudfront__events.cloudfront-invalidation-created-by-lambda-reflects-paths cloudfront__events.eventbridge-invalidation-rule-pattern-mismatch-no-invalidation-created cloudfront__events.eventbridge-scheduled-rule-invokes-lambda-for-cloudfront-invalidation cloudfront__events.eventbridge-scheduled-rule-triggers-cloudfront-invalidation-lambda cloudfront__scheduler.xsvc-cloudfront-3-scheduler-one-time-invalidation-applies-exact-paths cloudfront__scheduler.xsvc-cloudfront-3-scheduler-role-denied-invalidation-routes-to-dlq
Issue-Key: cloudfront|ListInvalidations|NotImplemented
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# Implement cloudfront ListInvalidations: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInvalidations

## Description
`ListInvalidations` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/list_invalidations.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

13 recorded unit(s) fail on their `ListInvalidations` call, so each one unblocks only when `ListInvalidations` works: `cloudfront.concurrent-invalidation-caller-reference-dedup`, `cloudfront.create-invalidation-via-cli-lands-the-same-record-as-the-sdk`, `cloudfront.distribution-id-resolves-from-its-domain-name-across-pages`, `cloudfront.invalidation-list-ordering-most-recent-first`, `cloudfront.list-invalidations-pagination-and-completeness`, `cloudfront.overly-permissive-invalidation-inconsistent-quantities-rejected`, `cloudfront__events.cfn-deploy-eventbridge-rule-cloudfront-invalidation-lambda`, `cloudfront__events.cloudfront-invalidation-created-by-lambda-reflects-paths`, `cloudfront__events.eventbridge-invalidation-rule-pattern-mismatch-no-invalidation-created`, `cloudfront__events.eventbridge-scheduled-rule-invokes-lambda-for-cloudfront-invalidation`, `cloudfront__events.eventbridge-scheduled-rule-triggers-cloudfront-invalidation-lambda`, `cloudfront__scheduler.xsvc-cloudfront-3-scheduler-one-time-invalidation-applies-exact-paths`, `cloudfront__scheduler.xsvc-cloudfront-3-scheduler-role-denied-invalidation-routes-to-dlq`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/list_invalidations.rs`. Replaying the corpus, the emulator refused the `ListInvalidations` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListInvalidations operation: operation ListInvalidations is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the cloudfront emulator
Call ListInvalidations and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
