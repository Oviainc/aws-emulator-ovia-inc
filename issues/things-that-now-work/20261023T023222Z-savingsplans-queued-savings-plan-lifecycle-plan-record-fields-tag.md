Issue-Source: trace-sweep-divergence
Issue-Trace: savingsplans-queued-plan-delete-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: savingsplans-queued-plan-delete-lifecycle
Issue-Shape-Id: com.amazonaws.savingsplans#AWSSavingsPlan
Issue-Key: savingsplans|DescribeSavingsPlans|2xx->2xx|$.savingsPlans[*]
Issue-Key: savingsplans|ListTagsForResource|2xx->2xx|$.tags.aws
Issue-Key: savingsplans|ReturnSavingsPlan|4xx->2xx|$.message
Issue-Key: savingsplans|ReturnSavingsPlan|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: savingsplans|ReturnSavingsPlan|4xx->2xx|status
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# Queued Savings Plan lifecycle: plan record fields, tag map and return rule diverge

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSavingsPlans

## Description
Replaying the queued-plan lifecycle shows three defects around a plan in state 'queued': its record is serialized differently, its tag map loses a key, and returning it is accepted instead of rejected.

Cause 1 of 3: the SavingsPlan record's money and time members are rendered in the wrong form, and returnableUntil uses the wrong return window.
  AWS: Smithy: SavingsPlan.commitment / upfrontPaymentAmount / recurringPaymentAmount all target Amount, which is a *string* — so the rendering is part of the wire value, not a number the client reformats; SavingsPlan.start / end / returnableUntil also target String. returnableUntil is documented as 'The time until when a return for the Savings Plan can be requested. If the Savings Plan is not returnable, the field reflects the Savings Plans start time.' Recorded behavior: Amount strings are rendered with 8 fractional digits ('0.00100000', '8.76000000', '0.00000000'); the time strings are ISO-8601 UTC with millisecond precision ('...T01:10:48.000Z'); and for the queued plan returnableUntil is start + 7 days, not start.
  Emulator: Amounts are rendered shortest-form ('0.001', '8.76', '0.0'); times are rendered without the millisecond field ('2027-04-04T23:30:31Z' — state.rs format_millis uses aws_smithy_types Format::DateTime, lines 93-97); and returnableUntil equals start, because the return window is a one-hour constant (RETURN_WINDOW_MILLIS, state.rs:74) and is_returnable() only admits payment-pending/active (state.rs:658), so a queued plan is reported non-returnable. The description text also differs ('Compute Savings Plans 1 year All Upfront' vs the recorded '1 year All Upfront Compute Savings Plan') — an example of the same catalog wording divergence filed for DescribeSavingsPlansOfferings. Secondary: the recorded row shows region, ec2InstanceFamily and properties as JSON null where the emulator omits the keys; absent-vs-null here may be recorder normalization, so it is not the substantive part of this cause.
  Why: because Amount is a string, a client that compares, displays or hashes the value sees a different value than AWS gives it, and a strict decimal parser configured for 8-dp billing amounts can reject it; a timestamp parser that expects millisecond precision fails to parse start/end/returnableUntil. returnableUntil is the field a client uses to decide whether to offer a return at all, so reporting start (a closed window) hides a window AWS says is open for 7 days.
  Fix: the SavingsPlan serializer plus the amount/timestamp formatting helpers and the return-window constant in the service state.

Cause 2 of 3: ListTagsForResource drops a tag key that AWS stores and returns.
  AWS: Smithy: TagMap is map<TagKey, TagValue> with no constraint excluding a key that begins 'aws:'. Recorded behavior: the plan tagged in this fixture carries tag key 'aws:tag:domain' with value 'prod', and ListTagsForResource returns it in $.tags along with the other keys.
  Emulator: answers 200 but with that entry absent from the tag map ($.tags['aws:tag:domain'] is None), so a key the caller set cannot be read back.
  Why: a client that writes a tag and reads it back silently loses it; code that keys off the tag (and UntagResource for that key) cannot see it, and tag-based reconciliation loops rewrite the tag forever.
  Fix: the tag intake shared by TagResource/CreateSavingsPlan or the ListTagsForResource serializer — whichever filters keys containing a colon or the 'aws:' prefix.

Cause 3 of 3: ReturnSavingsPlan accepts the return of a queued plan instead of rejecting it.
  AWS: Smithy: ReturnSavingsPlan declares ValidationException (client error, httpError 400) and the recorded response is HTTP 400 with header x-amzn-errortype: ValidationException and message 'Savings Plans in queued state cannot be returned.' — a plan that has not started yet is not returnable; it is deleted via DeleteQueuedSavingsPlan instead.
  Emulator: answers HTTP 200 with no error body at all for the same request on the same queued plan, so the return appears to succeed.
  Why: this is a state-machine divergence, not a message difference. A client that returns a queued plan gets success from the emulator and never exercises its error path; worse, the plan's subsequent state is whatever the accepted return produced, so every later assertion in that lifecycle runs against a state AWS would never reach. Note the emulator's own state helper already classifies a queued plan as non-returnable (is_returnable, state.rs:658), so the handler is not consulting it.
  Fix: the ReturnSavingsPlan handler's state guard and its ValidationException error channel.

Traces: savingsplans-queued-plan-delete-lifecycle#task#3 (cause 1; also #task#11), savingsplans-queued-plan-delete-lifecycle#task#5 (cause 2; also #task#7), savingsplans-queued-plan-delete-lifecycle#task#9 (cause 3).

## Evidence
Cause 1 — fixture savingsplans-queued-plan-delete-lifecycle step task#3 (DescribeSavingsPlans), status 200 vs 200, $.savingsPlans[0]:
  commitment expected '0.00100000', got '0.001'
  upfrontPaymentAmount expected '8.76000000', got '8.76'
  recurringPaymentAmount expected '0.00000000', got '0.0'
  start expected '2026-09-08T01:10:49.000Z' form (millisecond precision), got '2027-04-04T23:30:31Z' form (no millis)
  returnableUntil expected start + 7 days ('2026-09-15T01:10:49.000Z' against start '2026-09-08T01:10:49.000Z'), got a value equal to start ('2027-04-04T23:30:31Z')
  description expected '1 year All Upfront Compute Savings Plan', got 'Compute Savings Plans 1 year All Upfront'
  (savingsPlanId/offeringId matched; absolute instants differ per run and are not asserted as values — the format and the start-to-returnableUntil offset are.)

Cause 2 — same fixture step task#5 (ListTagsForResource), status 200 vs 200:
  $.tags['aws:tag:domain'] missing (expected 'prod', got None). Repeats at task#7.

Cause 3 — same fixture step task#9 (ReturnSavingsPlan):
  status: expected 400, got 200
  $.message missing (expected 'Savings Plans in queued state cannot be returned.', got None)
  $headers.x-amzn-errortype missing (expected 'ValidationException', got None)

state_reset_failures was 0 for this replay, so these steps ran against their own fixture's state.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the wire corpus (replay_wire_corpus).
2. Cause 1: fixture savingsplans-queued-plan-delete-lifecycle, step task#3 — POST /DescribeSavingsPlans for the queued plan and compare commitment/upfrontPaymentAmount/recurringPaymentAmount rendering, start/end/returnableUntil timestamp precision, and returnableUntil minus start, against the recording. Repeats at task#11.
3. Cause 2: same fixture, step task#5 — POST /ListTagsForResource for the plan ARN; the recorded tag map contains key 'aws:tag:domain', the emulator's does not. Repeats at task#7.
4. Cause 3: same fixture, step task#9 — POST /ReturnSavingsPlan with the queued plan's savingsPlanId; AWS answers 400 ValidationException, the emulator answers 200.
