Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.put-managed-insight-rules-invalid-resource-arn-rejected
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch|PutManagedInsightRules|NotImplemented
Issue-Commit: d26de989ce4778e024797c8509c4713b425f0550

# Implement cloudwatch PutManagedInsightRules: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d26de989ce4778e024797c8509c4713b425f0550`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutManagedInsightRules

## Description
`PutManagedInsightRules` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/ops/put_managed_insight_rules.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `PutManagedInsightRules` call, so each one unblocks only when `PutManagedInsightRules` works: `cloudwatch.put-managed-insight-rules-invalid-resource-arn-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/ops/put_managed_insight_rules.rs`. Replaying the corpus, the emulator refused the `PutManagedInsightRules` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutManagedInsightRules operation: operation PutManagedInsightRules is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the cloudwatch emulator
Call PutManagedInsightRules and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

## Resolution
`PutManagedInsightRules` is implemented in
`services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/ops/put_managed_insight_rules.rs`
and carries no `//! SCAFFOLDED, NOT IMPLEMENTED:` header line any more: the handler reads and
writes this service's Contributor Insights rule store, enabling one rule per (template,
resource) pair, re-enabling the rule a repeat call names rather than creating a second one,
refusing with HTTP 400 `InvalidParameterValueException` a `ResourceARN` that is not an Amazon
Resource Name and a `ManagedRules` list that names no rule, and reporting in `Failures` an
entry it could not enable (a rule name another rule already holds, or the store's rule
capacity). The rules it writes read back through `DescribeInsightRules` and
`ListManagedInsightRules`, and `tests/all/managed_insight_rules.rs` covers the loop end to end.

Remaining gap, carried by the recorded unit rather than by this report: the unit
`cloudwatch.put-managed-insight-rules-invalid-resource-arn-rejected` is no longer refused with
501 — its `ResourceARN` case is served, and refused with the 400 above — but its control call
enables a `TemplateName` no managed rule exists for and expects that entry back in `Failures`.
The handler accepts any template name a request carries, so that control call answers HTTP 200
with an empty `Failures`. Which template names a resource has is not something the Smithy model
carries, and the recorded `PartialFailure` entry's own wording is not recoverable from the
corpus, so no template catalog and no recorded prose are invented here.

