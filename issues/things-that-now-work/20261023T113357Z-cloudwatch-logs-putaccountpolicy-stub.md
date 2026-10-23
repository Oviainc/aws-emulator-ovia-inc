Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__logs.metric-extraction-policy-invalid-status-rejected cloudwatch__logs.metric-extraction-resumes-after-policy-deleted cloudwatch__logs.obs-logs-3-service-usage-and-emf-policy-metrics
Issue-Key: cloudwatch__logs.metric-extraction-policy-invalid-status-rejected|run#12|AssertionError|assert err == expected, (
Issue-Key: cloudwatch-logs|PutAccountPolicy|NotImplemented
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Implement cloudwatch-logs PutAccountPolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAccountPolicy

## Description
`PutAccountPolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_account_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `PutAccountPolicy` call, so each one unblocks only when `PutAccountPolicy` works: `cloudwatch__logs.metric-extraction-policy-invalid-status-rejected`, `cloudwatch__logs.metric-extraction-resumes-after-policy-deleted`, `cloudwatch__logs.obs-logs-3-service-usage-and-emf-policy-metrics`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_account_policy.rs`. Replaying the corpus, the emulator refused the `PutAccountPolicy` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_cloudwatch__logs_metric_extraction_policy_invalid_status_rejected.py", line 136, in run
    assert err == expected, (
           ^^^^^^^^^^^^^^^
AssertionError: PutAccountPolicy with EmbeddedMetricFormat.Status='Enabled' returned ('NotImplemented', 'operation PutAccountPolicy is scaffolded but not implemented in this emulator'); real AWS accepts only the exact string 'Disabled' and rejects this with ('InvalidParameterException', 'Invalid json policy document')

## Steps to Reproduce
Boot the cloudwatch-logs emulator
Call PutAccountPolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
