Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.account-data-protection-policy-masks-a-new-log-group logs.account-policy-enforced-globally-without-per-group-config logs.account-policy-lifecycle-transitions logs.account-policy-limit-exceeded-enforced logs.account-policy-malformed-document-rejected logs.deep-logs-3-logacct-metric-extraction-policy-enabled-status-rejected logs.deep-logs-3-logacct-metric-extraction-policy-suppresses-emf-only-for-selected-group logs.deep-logs-3-logacct-transformer-policy-applies-to-new-group-and-group-transformer-overrides logs.deep-logs-3-logacct-transformer-policy-overlapping-prefix-rejected logs.delete-account-policy-nonexistent-rejected logs.metric-extraction-policy-overlapping-selection-rejected logs.put-account-policy-concurrent-update-no-corruption logs.put-account-policy-invalid-scope-fails logs.put-account-policy-then-describe
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.deep-logs-3-logacct-metric-extraction-policy-enabled-status-rejected|run#11|AssertionError|assert code == "InvalidParameterException", (
Issue-Related: 20261023T113357Z-cloudwatch-logs-putaccountpolicy-stub.md
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Implement logs PutAccountPolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAccountPolicy

## Description
`PutAccountPolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_account_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

14 recorded unit(s) fail on their `PutAccountPolicy` call, so each one unblocks only when `PutAccountPolicy` works: `logs.account-data-protection-policy-masks-a-new-log-group`, `logs.account-policy-enforced-globally-without-per-group-config`, `logs.account-policy-lifecycle-transitions`, `logs.account-policy-limit-exceeded-enforced`, `logs.account-policy-malformed-document-rejected`, `logs.deep-logs-3-logacct-metric-extraction-policy-enabled-status-rejected`, `logs.deep-logs-3-logacct-metric-extraction-policy-suppresses-emf-only-for-selected-group`, `logs.deep-logs-3-logacct-transformer-policy-applies-to-new-group-and-group-transformer-overrides`, `logs.deep-logs-3-logacct-transformer-policy-overlapping-prefix-rejected`, `logs.delete-account-policy-nonexistent-rejected`, `logs.metric-extraction-policy-overlapping-selection-rejected`, `logs.put-account-policy-concurrent-update-no-corruption`, `logs.put-account-policy-invalid-scope-fails`, `logs.put-account-policy-then-describe`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_account_policy.rs`. Replaying the corpus, the emulator refused the `PutAccountPolicy` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutAccountPolicy operation: operation PutAccountPolicy is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the logs emulator
Call PutAccountPolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
