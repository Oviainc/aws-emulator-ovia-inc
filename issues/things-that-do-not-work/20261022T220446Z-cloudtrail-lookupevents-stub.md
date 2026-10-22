Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail.lookup-events-cloudtrail-event-is-json-string cloudtrail.lookup-events-invalid-event-category-fails cloudtrail.lookup-events-invalid-next-token-fails cloudtrail.lookup-events-invalid-time-range-fails cloudtrail.lookup-events-max-results-bound-and-token-stability cloudtrail.lookup-events-ordering-and-shape cloudtrail.lookup-events-single-lookup-attribute-enforced cloudtrail.lookup-events-window-boundary-is-exact-and-future-window-is-empty cloudtrail.op-contract-lookup-events cloudtrail__cognito-identity.obs-cognito-identity-1-describe-missing-pool-is-rejected-and-audited-with-its-error cloudtrail__lambda.obs-lambda-3-duplicate-permission-rejected-and-audited cloudtrail__lambda.obs-lambda-3-management-events cloudtrail__sts.source-identity-propagates-from-assume-role-into-the-sessions-own-audit-records
Issue-Key: cloudtrail|LookupEvents|NotImplemented
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# Implement cloudtrail LookupEvents: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
LookupEvents

## Description
`LookupEvents` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/lookup_events.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

13 recorded unit(s) fail on their `LookupEvents` call, so each one unblocks only when `LookupEvents` works: `cloudtrail.lookup-events-cloudtrail-event-is-json-string`, `cloudtrail.lookup-events-invalid-event-category-fails`, `cloudtrail.lookup-events-invalid-next-token-fails`, `cloudtrail.lookup-events-invalid-time-range-fails`, `cloudtrail.lookup-events-max-results-bound-and-token-stability`, `cloudtrail.lookup-events-ordering-and-shape`, `cloudtrail.lookup-events-single-lookup-attribute-enforced`, `cloudtrail.lookup-events-window-boundary-is-exact-and-future-window-is-empty`, `cloudtrail.op-contract-lookup-events`, `cloudtrail__cognito-identity.obs-cognito-identity-1-describe-missing-pool-is-rejected-and-audited-with-its-error`, `cloudtrail__lambda.obs-lambda-3-duplicate-permission-rejected-and-audited`, `cloudtrail__lambda.obs-lambda-3-management-events`, `cloudtrail__sts.source-identity-propagates-from-assume-role-into-the-sessions-own-audit-records`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/lookup_events.rs`. Replaying the corpus, the emulator refused the `LookupEvents` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the LookupEvents operation: operation amazonaws.cloudtrail.v20131101.CloudTrail_20131101.LookupEvents is not implemented in this emulator

## Steps to Reproduce
Boot the cloudtrail emulator
Call LookupEvents and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
