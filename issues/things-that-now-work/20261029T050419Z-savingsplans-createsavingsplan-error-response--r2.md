Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 314ff66d020c8bfcdd48f21d567324d2b402cbc6

# A negative upfrontPaymentAmount is refused on its FORM, not with the window it missed, breaking the crate's own sign-reading rule that commitment follows

_Filed: 2026-10-29T05:04:19Z_
_Operation: CreateSavingsPlan | Class: error-response_

> _Note: this issue was identified against the code base at commit `314ff66d020c8bfcdd48f21d567324d2b402cbc6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateSavingsPlan` reads its two `Amount` members with two different sign rules, and the one it gives `upfrontPaymentAmount` is the opposite of the rule the crate states for a member of that kind.

`crate::state` splits the reading of a leading `-` into two cases and writes the rule down: a member whose documented rule has a RANGE reads a negative amount as the number it is, so the refusal names the bound the value missed; a member whose rule is simply "an amount" refuses the sign as malformed. `commitment` — documented as "a value between 0.001 and 1 million" — is parsed with `Negative::Read` through `parse_commitment`, and `-1` is therefore refused with the floor (`'commitment' cannot be less than 0.001`).

`upfrontPaymentAmount` is documented with an equally explicit window — "This is a whole number between 50 and 99 percent of the total value of the Savings Plan" — so by that same stated rule it belongs in the RANGE case. But the handler parses it with `crate::state::parse_amount`, which routes through `parse_amount_with_digits` and hard-codes `Negative::Refused`. A caller that sends `upfrontPaymentAmount: "-438"` against a Partial Upfront offering is told its value "must be a non-negative decimal amount" — i.e. that it is not an amount at all — instead of being told the window (`50 to 99 percent …`) its value missed, which is the only place the service reports the bound back.

The crate's own test module states the split it means to implement (FORM = "not a plain decimal number (letters, exponent notation, two decimal points, an empty value)"; MAGNITUDE = "a value that IS a plain decimal number is refused only by the rule governing its member's magnitude, naming the bound or window it missed") and then pins the two members on opposite sides of it: a negative `commitment` is classified as MAGNITUDE ("a negative amount of any width is a number below the minimum, so it names the floor rather than being called malformed"), a negative `upfrontPaymentAmount` as FORM. A negative number is in neither of the FORM examples the module lists, so the `upfrontPaymentAmount` row contradicts the rule the same file opens with.

## Evidence

The rule the crate states for reading a sign — services/service-com-amazonaws-savingsplans-awssavingsplan/src/state.rs:237-249:

```
/// How a leading `-` is read by [`parse_decimal_amount`].
///
/// A member whose documented rule has a RANGE reads a negative amount as the
/// number it is, so the clause a caller gets names the bound the value missed;
/// a member whose rule is simply "an amount" refuses the sign as a malformed
/// value, which is all there is to say about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Negative {
    /// Refused as a malformed amount.
    Refused,
    /// Read as the negative number it is.
    Read,
}
```

The sibling member with a documented range follows it — src/state.rs:320-327:

```
pub fn parse_commitment(value: &str) -> Result<i128, String> {
    let units = parse_decimal_amount("commitment", value, MAX_AMOUNT_DIGITS, Negative::Read)?;
    if units < MIN_COMMITMENT_UNITS {
        return Err(format!(
            "'commitment' cannot be less than {}",
```

The path `upfrontPaymentAmount` takes instead — src/state.rs:225-227 and src/state.rs:251-254:

```
pub fn parse_amount(member: &str, value: &str) -> Result<i128, String> {
    parse_amount_with_digits(member, value, MAX_AMOUNT_DIGITS)
}
...
fn parse_amount_with_digits(member: &str, value: &str, max_digits: usize) -> Result<i128, String> {
    parse_decimal_amount(member, value, max_digits, Negative::Refused)
}
```

called from the handler — src/ops/create_savings_plan.rs:175-181:

```
        let supplied_upfront = match input.upfront_payment_amount.as_deref() {
            Some(amount) => Some((
                amount,
                crate::state::parse_amount("upfrontPaymentAmount", amount).map_err(validation)?,
            )),
```

The member's documented range, as the handler's own modeled-contract header quotes it — src/ops/create_savings_plan.rs:59-61:

```
//! * `upfrontPaymentAmount` (Amount) — The up-front payment amount. This is a whole number
//!   between 50 and 99 percent of the total value of the Savings Plan. This parameter is only
//!   supported if the payment option is Partial Upfront.
```

and the window clause the handler does produce once the value parses — src/ops/create_savings_plan.rs:332-340:

```
                        if upfront < min || upfront > max {
                            return Err(validation(format!(
                                "upfrontPaymentAmount must be a whole number between 50 and 99 \
                                 percent of the total value of the Savings Plan ({} to {}), got \
                                 '{sent}'",
```

Test coverage, which pins the two members on opposite sides of a rule the same file states. The rule — tests/all/amount_magnitude_refusals.rs:11-17:

```
//! * FORM — a value that is not a plain decimal number (letters, exponent notation, two
//!   decimal points, an empty value), or one spelled at more fractional digits than a
//!   request may specify, is refused on its form;
//! * MAGNITUDE — a value that IS a plain decimal number is refused only by the rule
//!   governing its member's magnitude, naming the bound or window it missed. A number
//!   too wide for any fixed-width arithmetic the service happens to read it with is
//!   still a plain decimal number, so it belongs to this half, not the one above.
```

`commitment` on the MAGNITUDE side — tests/all/amount_magnitude_refusals.rs:116-130:

```
/// The same rule under the FLOOR: a negative amount of any width is a number below the
/// minimum, so it names the floor rather than being called malformed.
...
    let expected = format!("'commitment' cannot be less than {MIN_COMMITMENT}");
...
        ("a small negative amount", "-1".to_string()),
```

`upfrontPaymentAmount` on the FORM side — tests/all/amount_magnitude_refusals.rs:373-380:

```
        (
            "a wide negative amount",
            format!("-{}", wide("1", 39)),
            format!(
                "upfrontPaymentAmount must be a non-negative decimal amount, got '-{}'",
                wide("1", 39)
            ),
        ),
```

## Steps to Reproduce

1. POST /DescribeSavingsPlansOfferings with `{"planTypes": ["Compute"], "paymentOptions": ["Partial Upfront"], "maxResults": 1}` and take `searchResults[0].offeringId`.
2. POST /CreateSavingsPlan with `{"savingsPlanOfferingId": "<that id>", "commitment": "0.1", "upfrontPaymentAmount": "-438"}`. The answer is 400 ValidationException with message `upfrontPaymentAmount must be a non-negative decimal amount, got '-438'` (src/state.rs:269 via Negative::Refused at src/state.rs:253).
3. POST /CreateSavingsPlan with the same offering and `{"commitment": "-438"}`. The answer is 400 ValidationException with message `'commitment' cannot be less than 0.001` — the bound the value missed, because `parse_commitment` reads the sign as a number (src/state.rs:321).
4. Compare the two against the rule state.rs:237-242 states ("A member whose documented rule has a RANGE reads a negative amount as the number it is") and the window documented for `upfrontPaymentAmount` ("a whole number between 50 and 99 percent of the total value of the Savings Plan"): the member WITH a documented range is the one refused on form.
5. Positive control that the window clause exists and is reachable: send `upfrontPaymentAmount: "8672"` on the same offering and observe `upfrontPaymentAmount must be a whole number between 50 and 99 percent of the total value of the Savings Plan (… to …), got '8672'` (tests/all/amount_magnitude_refusals.rs:290-299).
