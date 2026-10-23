Issue-Source: trace-sweep-divergence
Issue-Trace: health-core-read-operations-require-subscription
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: health-core-read-operations-require-subscription
Issue-Shape-Id: com.amazonaws.health#AWSHealth_20160804
Issue-Key: health|DescribeEventTypes|4xx->2xx|$.__type
Issue-Key: health|DescribeEventTypes|4xx->2xx|status
Issue-Key: health|DescribeEvents|4xx->2xx|$.__type
Issue-Key: health|DescribeEvents|4xx->2xx|status
Issue-Key: health|DescribeEventAggregates|4xx->2xx|$.__type
Issue-Key: health|DescribeEventAggregates|4xx->2xx|status
Issue-Key: health|DescribeEventDetails|4xx->2xx|$.__type
Issue-Key: health|DescribeEventDetails|4xx->2xx|status
Issue-Key: health|DescribeAffectedEntities|4xx->2xx|$.__type
Issue-Key: health|DescribeAffectedEntities|4xx->2xx|status
Issue-Key: health|DescribeEntityAggregates|4xx->2xx|$.__type
Issue-Key: health|DescribeEntityAggregates|4xx->2xx|status
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# health read operations return 200 instead of SubscriptionRequiredException

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeEventTypes

## Description
The emulator's AWS Health (sdkId Health, awsJson1_1 / rpcv2Cbor, endpointPrefix 'health') read surface answers every Describe* call with a successful 200 payload. On AWS, the Health API is gated on the caller's Support plan: an account without a Business, Enterprise On-Ramp or Enterprise Support subscription is refused with HTTP 400 and error type SubscriptionRequiredException, before any filtering or pagination work happens. The emulator has no such gate anywhere on the Health surface, so the refusal is never produced for any of the six recorded read operations. This is ONE shared fault (a missing subscription/entitlement gate in front of the Health read handlers) manifesting on six operations; all six causes below must be closed before this issue is submitted.

There are 6 causes.

Cause 1 of 6: DescribeEventTypes.
  AWS: recorded behavior only — SubscriptionRequiredException is not present in the service's Smithy model (Smithy: DescribeEventTypes errors are only InvalidPaginationToken, UnsupportedLocale; query shape=SubscriptionRequiredException -> 'shape not found'), and it is not a modeled common error of AWSHealth_20160804. The recorded response is HTTP 400 with the awsJson1_1 error discriminator $.__type = 'SubscriptionRequiredException'. Hypothesis: the recording account has no qualifying Support plan, which AWS documents as the precondition for calling the Health API at all.
  Emulator: HTTP 200 with a success body; $.__type absent entirely.
  Why: a client that probes Health to discover whether the account is entitled (a very common bootstrap step, since the SDK surfaces SubscriptionRequiredException as the signal to fall back or prompt for a plan upgrade) sees success and proceeds as if entitled, then mis-reports an empty event-type list as 'no event types' rather than 'not subscribed'. Error-handling paths for this exception are never exercised.
  Fix: the health service's DescribeEventTypes handler, or better a shared pre-dispatch entitlement check in the health service's request routing/validation layer so every read operation is gated once; the gate must emit HTTP 400 with __type SubscriptionRequiredException.
  Traces: health-core-read-operations-require-subscription#task#1.

Cause 2 of 6: DescribeEvents.
  AWS: recorded behavior only — not modeled (Smithy: DescribeEvents errors are InvalidPaginationToken, UnsupportedLocale only). Recorded response: HTTP 400, $.__type = 'SubscriptionRequiredException'.
  Emulator: HTTP 200 with a success body; $.__type missing.
  Why: callers enumerating account events treat the 200 as an authoritative empty/filled event list instead of an entitlement refusal, so dashboards and alerting built on Health silently show 'no active events' for an unsubscribed account.
  Fix: same shared entitlement gate ahead of the DescribeEvents handler in the health service.
  Traces: health-core-read-operations-require-subscription#task#2.

Cause 3 of 6: DescribeEventAggregates.
  AWS: recorded behavior only — not modeled (Smithy: DescribeEventAggregates errors are InvalidPaginationToken only). Recorded response: HTTP 400, $.__type = 'SubscriptionRequiredException'.
  Emulator: HTTP 200 with a success body; $.__type missing.
  Why: aggregate counts are returned as if valid, so a client cannot distinguish 'zero events' from 'not entitled to ask'.
  Fix: same shared entitlement gate ahead of the DescribeEventAggregates handler in the health service.
  Traces: health-core-read-operations-require-subscription#task#3.

Cause 4 of 6: DescribeEventDetails.
  AWS: recorded behavior only — not modeled (Smithy: DescribeEventDetails errors are UnsupportedLocale only). Recorded response: HTTP 400, $.__type = 'SubscriptionRequiredException'.
  Emulator: HTTP 200 with a success body; $.__type missing.
  Why: DescribeEventDetails normally reports per-event success/failure sets; a 200 here hides the entitlement refusal in what looks like a partial-result response, which is the hardest case for a client to detect.
  Fix: same shared entitlement gate ahead of the DescribeEventDetails handler in the health service.
  Traces: health-core-read-operations-require-subscription#task#4.

Cause 5 of 6: DescribeAffectedEntities.
  AWS: recorded behavior only — not modeled (Smithy: DescribeAffectedEntities errors are InvalidPaginationToken, UnsupportedLocale only). Recorded response: HTTP 400, $.__type = 'SubscriptionRequiredException'.
  Emulator: HTTP 200 with a success body; $.__type missing.
  Why: automation that maps events to affected resources receives an empty/synthetic entity list and concludes nothing is affected, instead of surfacing that the account may not query Health.
  Fix: same shared entitlement gate ahead of the DescribeAffectedEntities handler in the health service.
  Traces: health-core-read-operations-require-subscription#task#5.

Cause 6 of 6: DescribeEntityAggregates.
  AWS: recorded behavior only — not modeled (Smithy: DescribeEntityAggregates declares no operation-specific errors). Recorded response: HTTP 400, $.__type = 'SubscriptionRequiredException'.
  Emulator: HTTP 200 with a success body; $.__type missing.
  Why: same as above — affected-entity counts are accepted as real, so the unsubscribed state is invisible to the caller.
  Fix: same shared entitlement gate ahead of the DescribeEntityAggregates handler in the health service.
  Traces: health-core-read-operations-require-subscription#task#6.

Requirements vs examples: the HTTP status (400) and the error code/discriminator (SubscriptionRequiredException, carried in awsJson1_1's $.__type) are requirements. No error-message text is asserted here and none is claimed as a requirement, since neither the model nor AWS documentation fixes the wording for this refusal. No existing open issue covers the health service; the nearest-named open issues (connecthealth error responses omit Message; healthlake non-RPC HTTP surfaces unroutable) are different services and do not cover this.

## Evidence
Replay of fixture 'health-core-read-operations-require-subscription' (trace set simparity-wire-aws-v1), all 6 steps mismatched, 0 passed. Each step reports the same shape of divergence:

  task#1 DescribeEventTypes        — status: expected 400, got 200; $.__type: missing (expected 'SubscriptionRequiredException', got None)
  task#2 DescribeEvents            — status: expected 400, got 200; $.__type: missing (expected 'SubscriptionRequiredException', got None)
  task#3 DescribeEventAggregates   — status: expected 400, got 200; $.__type: missing (expected 'SubscriptionRequiredException', got None)
  task#4 DescribeEventDetails      — status: expected 400, got 200; $.__type: missing (expected 'SubscriptionRequiredException', got None)
  task#5 DescribeAffectedEntities  — status: expected 400, got 200; $.__type: missing (expected 'SubscriptionRequiredException', got None)
  task#6 DescribeEntityAggregates  — status: expected 400, got 200; $.__type: missing (expected 'SubscriptionRequiredException', got None)

Expected (AWS, per recording): HTTP 400, body carrying the awsJson1_1 error discriminator __type = 'SubscriptionRequiredException'.
Actual (emulator): HTTP 400 never produced — HTTP 200 with a success payload and no __type member at all.

Emulator was up for the whole replay (/__admin/health 200, data listener accepting); state_reset_failures = 0, steps_errored = 0, steps_blocked = 0, so these are genuine response-shape divergences and not state bleed or transport failures. Model cross-check: smithy_query on services/service-com-amazonaws-health-awshealth-20160804/smithy-model/com-amazonaws-health-awshealth-20160804.json shows shape 'SubscriptionRequiredException' is absent from the model, so the emulator cannot be generating it from the modeled error set — the gate has to be implemented in the health service itself.

## Steps to Reproduce
1. Boot the emulator from this checkout and confirm /__admin/health answers 200.
2. Replay wire fixture 'health-core-read-operations-require-subscription' from trace set simparity-wire-aws-v1 (steps task#1 through task#6). It issues six awsJson1_1 POSTs to the health endpoint with X-Amz-Target AWSHealth_20160804.DescribeEventTypes, .DescribeEvents, .DescribeEventAggregates, .DescribeEventDetails, .DescribeAffectedEntities and .DescribeEntityAggregates, from a caller whose account has no qualifying AWS Support subscription.
3. Observe each response: the emulator returns HTTP 200 with a success body.
4. Expected for every one of the six: HTTP 400 with a body whose __type member is 'SubscriptionRequiredException'.
5. After the fix, re-replay all six steps (task#1..task#6) — fixing one operation's handler does not close the others, since the divergence is reported independently per operation.
