Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.data-api-batch-auto-commit-mode-keeps-the-writes-a-transaction-would-roll-back redshift.data-api-batch-execute-statement-substatement-ordering redshift.data-api-batch-partial-failure-rolls-back-whole-batch redshift.data-api-session-transaction-hides-uncommitted-rows-from-a-second-session redshift.data-api-sql-failure-surfaces-as-async-failed-status redshift.data-api-statement-result-is-a-snapshot-reread-without-rerunning redshift.data-api-temporal-types-round-trip-and-the-engine-computes-their-span redshift.data-api-wait-time-seconds-returns-a-terminal-status-without-a-poll redshift.get-cluster-credentials-autocreate-creates-db-user redshift.rdapi-batch-substatements-are-individually-reportable redshift.rdapi-cancelled-statement-aborts-and-second-cancel-rejected redshift.rdapi-failed-statement-reports-error-and-result-read-rejected redshift.rdapi-statement-result-set-rows-and-column-metadata redshift.redshift-analyze-compression-picks-a-non-raw-encoding-visible-in-the-catalog redshift.redshift-distribution-and-sort-keys-are-materialised-in-the-cluster-catalog redshift.redshift-two-concurrent-transactions-on-the-same-row-serialize-to-one-winner redshift.redshift-unload-writes-the-table-to-s3-and-the-manifest-parts-reassemble-it redshift.snapshot-restore-preserves-table-data redshift__sqs.data-api-with-event-emits-a-real-statement-event-and-the-default-emits-none
Issue-Key: redshift|DescribeOrderableClusterOptions|NotImplemented
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# Implement redshift DescribeOrderableClusterOptions: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeOrderableClusterOptions

## Description
`DescribeOrderableClusterOptions` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/describe_orderable_cluster_options.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

19 recorded unit(s) fail on their `DescribeOrderableClusterOptions` call, so each one unblocks only when `DescribeOrderableClusterOptions` works: `redshift.data-api-batch-auto-commit-mode-keeps-the-writes-a-transaction-would-roll-back`, `redshift.data-api-batch-execute-statement-substatement-ordering`, `redshift.data-api-batch-partial-failure-rolls-back-whole-batch`, `redshift.data-api-session-transaction-hides-uncommitted-rows-from-a-second-session`, `redshift.data-api-sql-failure-surfaces-as-async-failed-status`, `redshift.data-api-statement-result-is-a-snapshot-reread-without-rerunning`, `redshift.data-api-temporal-types-round-trip-and-the-engine-computes-their-span`, `redshift.data-api-wait-time-seconds-returns-a-terminal-status-without-a-poll`, `redshift.get-cluster-credentials-autocreate-creates-db-user`, `redshift.rdapi-batch-substatements-are-individually-reportable`, `redshift.rdapi-cancelled-statement-aborts-and-second-cancel-rejected`, `redshift.rdapi-failed-statement-reports-error-and-result-read-rejected`, `redshift.rdapi-statement-result-set-rows-and-column-metadata`, `redshift.redshift-analyze-compression-picks-a-non-raw-encoding-visible-in-the-catalog`, `redshift.redshift-distribution-and-sort-keys-are-materialised-in-the-cluster-catalog`, `redshift.redshift-two-concurrent-transactions-on-the-same-row-serialize-to-one-winner`, `redshift.redshift-unload-writes-the-table-to-s3-and-the-manifest-parts-reassemble-it`, `redshift.snapshot-restore-preserves-table-data`, `redshift__sqs.data-api-with-event-emits-a-real-statement-event-and-the-default-emits-none`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/describe_orderable_cluster_options.rs`. Replaying the corpus, the emulator refused the `DescribeOrderableClusterOptions` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeOrderableClusterOptions operation: operation DescribeOrderableClusterOptions is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the redshift emulator
Call DescribeOrderableClusterOptions and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
