Issue-Source: trace-sweep-divergence
Issue-Trace: cost-explorer-cost-and-usage-validation
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cost-explorer-cost-and-usage-validation
Issue-Shape-Id: com.amazonaws.costexplorer#AWSInsightsIndexService
Issue-Key: ce|GetDimensionValues|4xx->4xx|$.message
Issue-Commit: f81229b87ab2500fc3bfee437baf8654766d98fe

# GetDimensionValues applies the generic Context enum set instead of the operation's narrower COST_AND_USAGE/RESERVATIONS rule

> _Note: this issue was identified against the code base at commit `f81229b87ab2500fc3bfee437baf8654766d98fe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDimensionValues

## Description
Cause 1 of 1: GetDimensionValues rejects an invalid `Context` with the generic Smithy enum-constraint rejection computed over the whole `Context` enum, rather than with Cost Explorer's narrower per-operation rule for this member. Fixture `cost-explorer-cost-and-usage-validation`, step `task#9`.

AWS: `GetDimensionValues` accepts only two of the three modeled `Context` values. Smithy: `GetDimensionValues.Context` targets `com.amazonaws.costexplorer#Context`, an enum whose full value set is `[COST_AND_USAGE, RESERVATIONS, SAVINGS_PLANS]`, but the model's own documentation for `GetDimensionValuesRequest$Context` states "The context for the call to GetDimensionValues. This can be RESERVATIONS or COST_AND_USAGE. The default value is COST_AND_USAGE." The recording agrees: the service answers HTTP 400 with the fixed, operation-specific sentence `Only COST_AND_USAGE and RESERVATIONS are valid values for context` — it names the two values this operation permits and does not echo the rejected input. So the rule is known from both the model and the recording: for this operation the admissible `Context` set is {COST_AND_USAGE, RESERVATIONS}, SAVINGS_PLANS is NOT admissible, and the rejection is a fixed sentence rather than a request-derived constraint report. (`GetDimensionValues` also models no ValidationException; its modeled errors are BillExpirationException, BillingViewHealthStatusException, DataUnavailableException, InvalidNextTokenException, LimitExceededException, RequestChangedException and ResourceNotFoundException.)

Emulator: the HTTP status (400) and the error code both match; the response body's `$.message` does not. The emulator emits the framework's generic enum-set validation report instead — `1 validation error detected: Value 'NOT_A_CONTEXT' at 'context' failed to satisfy constraint: Member must satisfy enum value set: [COST_AND_USAGE, RESERVATIONS, SAVINGS_PLANS]`. This is not a wording difference: it shows the emulator is validating `Context` against the raw enum shape's whole value set, so the operation-specific restriction is not applied at all, and the response advertises a third value as acceptable for an operation that does not accept it.

Why: a client that reads the rejection to discover the admissible set is told `SAVINGS_PLANS` is valid for `GetDimensionValues`, and the same generic-enum validation path means a request carrying `Context=SAVINGS_PLANS` is admitted by the emulator where AWS refuses it. Code that is developed and tested against the emulator therefore builds dimension-value queries AWS rejects at 400 in production, and the inverse case (a client asserting on the documented fixed sentence, or parsing it to enumerate contexts) breaks against the emulator.

Fix: in the Cost Explorer service crate's `GetDimensionValues` input validation. `Context` must not be validated by the shared enum-set validator alone; the handler needs an operation-scoped admissible set of {COST_AND_USAGE, RESERVATIONS} and must produce the fixed per-operation message for a value outside it, without interpolating the rejected value or the full enum value list. Leaving the generic validator in place for other operations that legitimately take SAVINGS_PLANS (e.g. the Savings Plans dimension consumers) is correct; only this operation's member is narrower.

No open issue covers Cost Explorer `GetDimensionValues`; the service's bootstrap issue (20261021T215812Z-bootstrap-service-com-amazonaws-costexplorer-awsinsightsindexservice.md) is already resolved, so this is a fidelity gap in a bootstrapped service.

Traces: cost-explorer-cost-and-usage-validation#task#9 (the cluster's only member).

## Evidence
Fixture `cost-explorer-cost-and-usage-validation`, step `task#9`, operation `GetDimensionValues`, service `ce`. Verdict: mismatch. Expected status 400, actual status 400 — status and error code agree; the divergence is confined to `$.message`.

Expected (recorded AWS response body `$.message`):
  Only COST_AND_USAGE and RESERVATIONS are valid values for context

Actual (emulator response body `$.message`):
  1 validation error detected: Value 'NOT_A_CONTEXT' at 'context' failed to satisfy constraint: Member must satisfy enum value set: [COST_AND_USAGE, RESERVATIONS, SAVINGS_PLANS]

Replay detail, verbatim: `$.message: value_mismatch (expected 'Only COST_AND_USAGE and RESERVATIONS are valid values for context', got "1 validation error detected: Value 'NOT_A_CONTEXT' at 'context' failed to satisfy constraint: Member must satisfy enum value set: [COST_AND_USAGE, RESERVATIONS, SAVINGS_PLANS]")`
Divergence key: `service=ce|GetDimensionValues|status=400->400|keys=$.message`

Model evidence (services/service-com-amazonaws-costexplorer-awsinsightsindexservice/smithy-model/com-amazonaws-costexplorer-awsinsightsindexservice.json):
- `com.amazonaws.costexplorer#Context` enum values: COST_AND_USAGE, RESERVATIONS, SAVINGS_PLANS.
- `GetDimensionValuesRequest$Context` documentation: "This can be RESERVATIONS or COST_AND_USAGE. The default value is COST_AND_USAGE." — i.e. the operation's admissible set is a strict subset of the enum, which is the rule the emulator is missing.
- `GetDimensionValues` modeled errors do not include a ValidationException.

The emulator's message enumerates all three enum values, which is the observable signature of the generic enum validator running in place of the operation-scoped check; the recorded response enumerates only the two this operation accepts.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it (the Cost Explorer service is bootstrapped; its bootstrap issue is resolved).
2. Replay the wire corpus: `replay_wire_corpus()` over trace set `simparity-wire-aws-v1`.
3. Observe fixture `cost-explorer-cost-and-usage-validation`, step `task#9` report verdict `mismatch` with divergence key `service=ce|GetDimensionValues|status=400->400|keys=$.message`.
4. Equivalently, by hand: send a `GetDimensionValues` request to the emulator's Cost Explorer endpoint with a required `TimePeriod`, a required `Dimension`, and `Context` set to a value outside the enum (the fixture uses `NOT_A_CONTEXT`). The response is 400 with `$.message` = `1 validation error detected: Value 'NOT_A_CONTEXT' at 'context' failed to satisfy constraint: Member must satisfy enum value set: [COST_AND_USAGE, RESERVATIONS, SAVINGS_PLANS]`; AWS answers 400 with `$.message` = `Only COST_AND_USAGE and RESERVATIONS are valid values for context`.
5. To confirm the underlying rule gap rather than just the wording: send the same request with `Context=SAVINGS_PLANS`. The emulator admits it (it is in the enum value set); AWS rejects it for this operation with the same fixed message.
6. After fixing, re-replay `cost-explorer-cost-and-usage-validation#task#9` and confirm both the rejected-value case and the `SAVINGS_PLANS` case produce the fixed per-operation message at 400.
