Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rum.delete-rum-metrics-destination-unknown-conflict rum.put-rum-metrics-destination-access-denied
Issue-Shape-Id: com.amazonaws.rum#RUM
Issue-Key: rum|ListRumMetricsDestinations|NotImplemented
Issue-Key: rum.put-rum-metrics-destination-access-denied|_poll_succeeds#8|AssertionError|raise AssertionError(f"call never succeeded within {timeout_s}s (last refusal={last!r})")
Issue-Commit: f973f78f74c99aad4e85f2736f1fde89ed7327e3

# Implement rum ListRumMetricsDestinations: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `f973f78f74c99aad4e85f2736f1fde89ed7327e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListRumMetricsDestinations

## Description
`ListRumMetricsDestinations` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-rum-rum/src/ops/list_rum_metrics_destinations.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `ListRumMetricsDestinations` call, so each one unblocks only when `ListRumMetricsDestinations` works: `rum.delete-rum-metrics-destination-unknown-conflict`, `rum.put-rum-metrics-destination-access-denied`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-rum-rum/src/ops/list_rum_metrics_destinations.rs`. Replaying the corpus, the emulator refused the `ListRumMetricsDestinations` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListRumMetricsDestinations operation: operation ListRumMetricsDestinations is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the rum emulator
Call ListRumMetricsDestinations and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

## Update: the `rum|ListRumMetricsDestinations|NotImplemented` key is fixed
`ListRumMetricsDestinations` was implemented, and its `//! SCAFFOLDED, NOT IMPLEMENTED:` census line deleted, as the read-back half of the `PutRumMetricsDestination` stub fix (`issues/things-that-now-work/20261022T205048Z-rum-putrummetricsdestination-stub.md`): it pages the per-app-monitor destinations map that put writes, 404s an app monitor that does not exist, and refuses a `NextToken` it never issued. It no longer answers 501, so the "Description", "Evidence" and "Steps to Reproduce" above describe the state of the code BEFORE that change.

## Update: this record is resolved, and why it sits here
Both of this record's keys are answered as far as this record's own subject — the `ListRumMetricsDestinations` stub — reaches:

- `rum|ListRumMetricsDestinations|NotImplemented`: fixed, as the update above says.
- `rum.put-rum-metrics-destination-access-denied|_poll_succeeds#8|AssertionError`: the polled call answered 501 forever while the operation was a scaffold; with `PutRumMetricsDestination` and `ListRumMetricsDestinations` implemented it succeeds. The `AccessDeniedException` that unit ends on needs authorization evaluation, which has since landed service-wide (`src/state/authorization.rs`: the caller is resolved through STS's own store, its identity-based documents are read out of IAM's store, and the decision is the shared `service_common::iam_authorization` evaluation of `(action, resource, principal)` against the app monitor's resource policy). `ListRumMetricsDestinations` makes that decision per call for `rum:ListRumMetricsDestinations` on the app monitor's ARN, so the 403 this unit expects now comes from evaluating the policies in force rather than from the shape of the request.

`DeleteRumMetricsDestination`, which the other blocked unit (`rum.delete-rum-metrics-destination-unknown-conflict`) calls after the listing, was implemented together with this record's closure, so neither unit is left on a 501 from this operation family.

