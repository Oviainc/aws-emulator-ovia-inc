Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 03bb735b5164c6b554d75cb5873a72ff1e3f303a

# CreateSavingsPlan's idempotency fingerprint keys on the raw spelling of upfrontPaymentAmount and purchaseTime, which the stored plan does not depend on

_Filed: 2026-10-27T16:50:50Z_
_Operation: CreateSavingsPlan | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `03bb735b5164c6b554d75cb5873a72ff1e3f303a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The `clientToken` fingerprint is built from four request members, but only ONE of them (`commitment`) is normalised to the value the service actually stores; `upfrontPaymentAmount` and `purchaseTime` go into the fingerprint as the caller spelled them, even though the stored plan depends only on their PARSED/TRUNCATED values.

Consequence: a replay of the very same purchase under the same `clientToken`, spelled differently in a way the service itself treats as identical, is refused with a 400 `ValidationException` ("clientToken '<t>' was already used for a different CreateSavingsPlan request") instead of answering idempotently with the plan the first call created.

Two concrete symptoms, one class:

1. `upfrontPaymentAmount`: `"438"` and `"438.00"` are read by `crate::state::parse_amount` to the same `i128` (`438 * AMOUNT_SCALE`), the plan record stores only that number (`SavingsPlanRecord::upfront_payment_units`, state.rs:1641-1642), and the response renders it through the one fixed scale — yet the two spellings fingerprint differently, so the second call is refused.
2. `purchaseTime`: the fingerprint uses the request's raw epoch millis (`millis.to_string()`), while `crate::state::purchase` truncates it to whole seconds before storing (`let start_millis = truncate_to_second(purchase_millis);`, state.rs:2105). Two spellings of one second therefore fingerprint differently although they produce a byte-identical plan.

The handler already knows the rule and applies it to `commitment` only — it deliberately feeds the fingerprint `format_amount(commitment)` (the parsed decimal) rather than the request's spelling. The same treatment is simply not given to the other two members, so one fingerprint computation follows two contradictory rules.

## Evidence

The crate's own stated rule for what a fingerprint must contain — services/service-com-amazonaws-savingsplans-awssavingsplan/src/state.rs:512-513:

    /// The identity of an idempotent request: everything about it that the stored
    /// resource depends on.

Side A, the normalised member — services/service-com-amazonaws-savingsplans-awssavingsplan/src/ops/create_savings_plan.rs:211-217:

        let stored_commitment = crate::state::format_amount(commitment);
        let mut named: Vec<(String, String)> = vec![
            ...
            ("commitment".to_string(), stored_commitment),

with the reason spelled out at create_savings_plan.rs:351-354:

            // The commitment as the service holds it: the parsed decimal, not
            // the request's spelling of it, so it is rendered in the one
            // `Amount` form every other amount is rendered in.

Side B, the un-normalised members in the SAME vec — create_savings_plan.rs:218-222:

            (
                "upfrontPaymentAmount".to_string(),
                input.upfront_payment_amount.clone().unwrap_or_default(),
            ),
            ("purchaseTime".to_string(), supplied_purchase_time),

and create_savings_plan.rs:158-163, where `supplied_purchase_time` is the raw millis:

        let supplied_purchase_time = input
            .purchase_time
            .as_ref()
            .and_then(|time| time.to_millis().ok())
            .map(|millis| millis.to_string())
            .unwrap_or_default();

What the stored plan actually depends on — state.rs:2105 (`purchase`):

        let start_millis = truncate_to_second(purchase_millis);

and state.rs:1641-1642 (the record holds the up-front payment as a NUMBER):

    /// The up-front payment, in [`AMOUNT_SCALE`] units.
    pub upfront_payment_units: i128,

The crate elsewhere states that an amount is one value however it is spelled — state.rs:1637-1639:

    /// The hourly commitment, in [`AMOUNT_SCALE`] units. Held as a NUMBER, not
    /// as the string it is rendered to, so one amount has exactly one spelling
    /// on the wire ([`format_amount`]) and a filter on it compares numbers.

Test coverage: tests/all/savings_plans_lifecycle.rs:725-790 `a_replayed_client_token_buys_nothing_more` replays the IDENTICAL body three times and asserts the mismatch refusal only for a different `commitment` (:760-770) and different `tags` (:771-781). No test sends a re-spelled `upfrontPaymentAmount` or `purchaseTime` under the same token, so nothing pins the current behaviour and no comment states a reason for the asymmetry.

## Steps to Reproduce

1. POST /DescribeSavingsPlansOfferings with {"planTypes":["Compute"],"paymentOptions":["Partial Upfront"],"durations":[31536000],"maxResults":1000} and take an `offeringId`.
2. POST /CreateSavingsPlan with {"savingsPlanOfferingId":"<offeringId>","commitment":"0.1","upfrontPaymentAmount":"438","clientToken":"t"} — 200, note the returned `savingsPlanId` P.
3. Replay the SAME purchase with the up-front amount spelled "438.00" (same clientToken "t", same commitment): expected the idempotent 200 with `savingsPlanId` P, because `parse_amount` reads "438" and "438.00" to the identical stored `upfront_payment_units`; actual is 400 ValidationException "clientToken 't' was already used for a different CreateSavingsPlan request".
4. Contrast: replay the same purchase with the commitment re-spelled "0.10000" instead — that one IS answered idempotently with P, because `commitment` is normalised through `format_amount` before it reaches the fingerprint. One fingerprint, two rules.
5. Same class for the timestamp: purchase with a `purchaseTime` carrying a sub-second fraction, then replay the same token with the instant spelled at whole-second precision. The stored plan is identical (state.rs:2105 truncates to the second) but the fingerprint differs, so the replay is refused.
