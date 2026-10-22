Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: quicksight.custom-sql-dataset-column-schema-round-trip quicksight.dataset-direct-query-import-mode-skips-ingestion quicksight.dataset-permissions-grant-revoke-round-trip quicksight.dataset-spice-ingestion-fails-deterministically-on-invalid-source quicksight.datasource-lifecycle-create-update-delete quicksight.delete-namespace-blocked-by-nonempty-precondition quicksight.duplicate-namespace-creation-rejected quicksight.folder-membership-add-remove-reflected-in-listing quicksight.group-membership-add-remove-reflected-in-listing quicksight.iam-policy-assignment-concurrent-creation-contention quicksight.iam-policy-assignment-status-transitions quicksight.namespace-creation-reaches-created-state quicksight.refresh-schedule-quota-exceeded-rejected quicksight.register-user-quicksight-identity-lifecycle quicksight.role-membership-group-lifecycle quicksight.search-datasets-by-name-finds-created-dataset quicksight.theme-alias-nonexistent-version-rejected quicksight.theme-creation-reaches-successful-state quicksight.theme-permissions-grant-revoke-round-trip quicksight.topic-creation-dataset-association-reflected
Issue-Shape-Id: com.amazonaws.quicksight#QuickSight_20180401
Issue-Key: quicksight|DescribeAccountSubscription|NotImplemented
Issue-Commit: 09c29ed8e99ab10517eef5fd4d5c2097ded28d72

# Implement quicksight DescribeAccountSubscription: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `09c29ed8e99ab10517eef5fd4d5c2097ded28d72`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAccountSubscription

## Description
`DescribeAccountSubscription` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-quicksight-quicksight-20180401/src/ops/describe_account_subscription.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

20 recorded unit(s) fail on their `DescribeAccountSubscription` call, so each one unblocks only when `DescribeAccountSubscription` works: `quicksight.custom-sql-dataset-column-schema-round-trip`, `quicksight.dataset-direct-query-import-mode-skips-ingestion`, `quicksight.dataset-permissions-grant-revoke-round-trip`, `quicksight.dataset-spice-ingestion-fails-deterministically-on-invalid-source`, `quicksight.datasource-lifecycle-create-update-delete`, `quicksight.delete-namespace-blocked-by-nonempty-precondition`, `quicksight.duplicate-namespace-creation-rejected`, `quicksight.folder-membership-add-remove-reflected-in-listing`, `quicksight.group-membership-add-remove-reflected-in-listing`, `quicksight.iam-policy-assignment-concurrent-creation-contention`, `quicksight.iam-policy-assignment-status-transitions`, `quicksight.namespace-creation-reaches-created-state`, `quicksight.refresh-schedule-quota-exceeded-rejected`, `quicksight.register-user-quicksight-identity-lifecycle`, `quicksight.role-membership-group-lifecycle`, `quicksight.search-datasets-by-name-finds-created-dataset`, `quicksight.theme-alias-nonexistent-version-rejected`, `quicksight.theme-creation-reaches-successful-state`, `quicksight.theme-permissions-grant-revoke-round-trip`, `quicksight.topic-creation-dataset-association-reflected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-quicksight-quicksight-20180401/src/ops/describe_account_subscription.rs`. Replaying the corpus, the emulator refused the `DescribeAccountSubscription` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeAccountSubscription operation: operation DescribeAccountSubscription is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the quicksight emulator
Call DescribeAccountSubscription and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
