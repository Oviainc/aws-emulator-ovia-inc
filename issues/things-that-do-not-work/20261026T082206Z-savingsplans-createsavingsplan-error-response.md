Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: b827135cf833dfec17047e08babbbec809d0cc25

# A well-formed commitment above the documented ceiling is refused on its FORM once it exceeds i128, breaking the rule parse_commitment's own doc states

_Filed: 2026-10-26T08:22:06Z_
_Operation: CreateSavingsPlan | Class: error-response_

> _Note: this issue was identified against the code base at commit `b827135cf833dfec17047e08babbbec809d0cc25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateSavingsPlan` routes `commitment` through `crate::state::parse_commitment`, whose doc comment states exactly which refusal applies to which input: a value OUTSIDE the documented 0.001..1,000,000 interval must be refused with the bound it missed, and ONLY a value that is not a decimal number at all (or one with too many fractional digits) may be refused on its form.

The code under that comment breaks the rule for a plain decimal integer large enough to exceed `i128`: `whole.parse::<i128>()` (or the following `checked_mul(AMOUNT_SCALE)`) fails, and the handler answers with the FORM clause ("commitment must be a decimal amount, got '…'") instead of the ceiling clause ("'commitment' cannot be greater than 1000000"). Because the value is first multiplied by `AMOUNT_SCALE` (1e8), the crossover is around 1.7e30 — i.e. every commitment spelled with ~31 or more integer digits, all of which are well-formed decimal amounts above the ceiling.

The same handler hits the same path for `upfrontPaymentAmount` (`crate::state::parse_amount`, whose doc says it refuses "anything that is not a plain non-negative decimal number with at most MAX_AMOUNT_DIGITS decimal places" — an overflowing value IS such a number): a Partial Upfront purchase sent a 31-digit upfront amount is told the value is malformed rather than told the 50–99 % window it missed.

`Amount` is an unconstrained string in the model, so nothing before the handler refuses these values; the crate's own classification is therefore the only rule, and it is self-contradicted.

## Evidence

The rule, in this crate's own doc comment — services/service-com-amazonaws-savingsplans-awssavingsplan/src/state.rs:286-290:

    /// A value OUTSIDE that interval is a well-formed amount the member's range
    /// refuses, so the clause names the member and the bound it missed — the floor
    /// for anything below it, including zero and a negative amount, and the ceiling
    /// for anything above. Only a value that is not a decimal number at all, or one
    /// spelled at more digits than a request may carry, is refused on its form.

The code under it — services/service-com-amazonaws-savingsplans-awssavingsplan/src/state.rs:270-277:

        let whole: i128 = whole.parse().map_err(|_| invalid())?;
        let mut units = whole.checked_mul(AMOUNT_SCALE).ok_or_else(invalid)?;

where `invalid` is the FORM message — state.rs:249:

        let invalid = || format!("{member} must be a {form}, got '{value}'");

so the range checks below it are never reached — state.rs:293-304:

        if units < MIN_COMMITMENT_UNITS { … "'commitment' cannot be less than {}" … }
        if units > MAX_COMMITMENT_UNITS { … "'commitment' cannot be greater than {}" … }

The handler calls it on the request path — services/service-com-amazonaws-savingsplans-awssavingsplan/src/ops/create_savings_plan.rs:153:

        let commitment = crate::state::parse_commitment(&input.commitment).map_err(validation)?;

and on `upfrontPaymentAmount` — create_savings_plan.rs:280-285:

        let supplied_upfront = match input.upfront_payment_amount.as_deref() {
            Some(amount) => Some(
                crate::state::parse_amount("upfrontPaymentAmount", amount).map_err(validation)?,
            ),

The second doc copy broken by the same line — state.rs:199-201:

    /// Parse an `Amount` into [`AMOUNT_SCALE`] units, refusing anything that is not
    /// a plain non-negative decimal number with at most [`MAX_AMOUNT_DIGITS`]
    /// decimal places.

Test coverage stops short of the crossover: tests/all/commitment_bounds.rs:79-80 pins the ceiling clause only for "1000000.00001" and "10000000" —

        ("one step above the ceiling", "1000000.00001", &above),
        ("an order of magnitude above the ceiling", "10000000", &above),

and tests/all/commitment_bounds.rs:203-204 only asserts the form rows do not say "cannot be less than", so no test pins a 31-digit value either way.

## Steps to Reproduce

1. POST /DescribeSavingsPlansOfferings with {"maxResults": 1} and take an `offeringId` (e.g. a Compute / No Upfront / 1-year row).
2. POST /CreateSavingsPlan with {"savingsPlanOfferingId": "<that id>", "commitment": "10000000000000000000000000000000"} (a 32-digit, well-formed decimal, far above the documented 1-million ceiling).
3. Observe 400 ValidationException whose message is the FORM clause "commitment must be a decimal amount, got '…'".
4. Repeat with {"commitment": "10000000"} and observe the bound clause "'commitment' cannot be greater than 1000000" (tests/all/commitment_bounds.rs:80) — the two over-ceiling values are classified differently, which state.rs:286-290 says must not happen.
5. On a Partial Upfront offering, POST /CreateSavingsPlan with a valid commitment and {"upfrontPaymentAmount": "10000000000000000000000000000000"} and observe "upfrontPaymentAmount must be a non-negative decimal amount" instead of the 50–99 % window message state.rs:199-201 and create_savings_plan.rs:314-323 describe.
