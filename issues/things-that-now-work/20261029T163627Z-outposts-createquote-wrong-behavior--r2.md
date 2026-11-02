Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 98d78e626a1d1edaa8acf37a7e06643e88a0f87c

# Quote pricing overflows f32 and persists +Infinity / NaN prices, past the very guard subscription_price applies to non-finite capacity

_Filed: 2026-10-29T16:36:27Z_
_Operation: CreateQuote | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `98d78e626a1d1edaa8acf37a7e06643e88a0f87c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateQuote` prices the requested capacity through `state::quote_options` -> `quote_pricing` -> `quoted_capacity_vcpus` -> `subscription_price`, and persists the result on the quote record. `subscription_price` explicitly refuses to price a non-finite capacity (`if vcpus <= 0.0 || !vcpus.is_finite() { return None }`), but the three arithmetic steps that follow that guard can each overflow f32 and produce exactly the non-finite figures the guard exists to keep out:

* `monthly_rate = vcpus * SUBSCRIPTION_PRICE_PER_VCPU_MONTH` (x20.0) and `term = monthly_rate * months` (x12/36/60) overflow to +inf for a finite `vcpus` above ~1.7e37;
* `to_the_cent(amount)` computes `(amount * 100.0).round() / 100.0`, so it turns ANY finite price above ~3.4e36 into +inf before rounding;
* for `ALL_UPFRONT` the upfront share is `1.0`, so `monthly_recurring_price = to_the_cent(monthly_rate * (1.0 - share))` evaluates `inf * 0.0` = NaN.

`quoted_capacity_vcpus` only checks each individual `Quantity` (`quantity.is_finite() && *quantity > 0.0`); it never checks the product `quantity * vcpus` or the sum over up to 2000 capacities, and `QuoteCapacity$Quantity` is modelled as a bare `NullableFloat` with no range, so a single model-legal request reaches the overflow.

The result is not a refusal and not a price: the quote is answered 200 and the non-finite `UpfrontPrice`/`MonthlyRecurringPrice` are written into `store.quotes`, so every later `GetQuote`/`ListQuotes` re-serializes them for the life of the process. JSON has no representation for +Infinity or NaN, so the member is either dropped/nulled or renders a token no JSON client can parse — either way the caller is handed a 'priced' quote option whose price is meaningless, instead of the `None` (option left out) path the same function takes for the non-finite input it does check. This is a different defect from the already-fixed 'CreateQuote never produces QuoteOptions' gap: options are produced, but their figures overflow.

The same helper is shared with `CreateRenewal`/`GetRenewalPricing` via `renewal_price`, so the overflow lives in the one rule this service prices subscriptions by.

## Evidence

The guard, and the unguarded arithmetic immediately under it — services/service-com-amazonaws-outposts-outpostsolafservice/src/state.rs:2826-2837:
```
    if vcpus <= 0.0 || !vcpus.is_finite() {
        // There is no capacity here, so there is no subscription on it that could be
        // priced at all.
        return None;
    }
    let monthly_rate = vcpus * SUBSCRIPTION_PRICE_PER_VCPU_MONTH;
    let term = monthly_rate * *months as f32;
    Some(SubscriptionPricingDetails {
        ...
        upfront_price: Some(to_the_cent(term * share)),
        monthly_recurring_price: Some(to_the_cent(monthly_rate * (1.0 - share))),
```
The rounding helper that overflows on its own — state.rs:2796-2799:
```
/// `amount` rounded to the cent, the precision a price is quoted to.
fn to_the_cent(amount: f32) -> f32 {
    (amount * 100.0).round() / 100.0
}
```
`ALL_UPFRONT` has share `1.0`, which makes the monthly figure `inf * 0.0` = NaN — state.rs:2777-2781:
```
pub const PAYMENT_OPTION_UPFRONT_SHARE: &[(&str, f32)] = &[
    ("ALL_UPFRONT", 1.0),
```
The capacity total checks only each quantity, never the product or the sum — state.rs:3871-3884:
```
    pub fn quoted_capacity_vcpus(&self, capacities: &[QuoteCapacity]) -> f32 {
        ...
                let quantity = capacity
                    .quantity
                    .filter(|quantity| quantity.is_finite() && *quantity > 0.0)?;
                Some(quantity * vcpus as f32)
            })
            .sum()
```
The doc this breaks, two lines above the guard — state.rs:2813-2814: "Every figure follows from the capacity it was given, so one price is answered the same in every process and on every call."

The model allows the input: `smithy_query(query="shape", shape="QuoteCapacity")` gives `"Quantity": {"target": "NullableFloat", "type": "float"}` and `"NullableFloat": {"type": "float"}` — no `range` trait, so 1e38 is a value the framework accepts before the handler runs.

The overflowed figures are PERSISTED, not just returned — services/service-com-amazonaws-outposts-outpostsolafservice/src/ops/create_quote.rs:262-266, :293 and :307-316:
```
        let quote_options = store.quote_options(
            &input.requested_capacities,
...
            quote_options: Some(quote_options),
...
        store.quotes.insert(
            quote_id,
            crate::state::QuoteRecord { sequence, region, quote: quote.clone() },
        );
```
A published instance type with 2 vCPUs exists, so a modest quantity suffices — state.rs:1711-1715: `OrderableFamily { family: "c5", ... sizes: &[("large", 2, "Up to 10 Gigabit"), ...`

Test coverage: no test pins any bound on these figures. `tests/all/quote_options.rs:258-271` and `tests/all/renewal_pricing.rs:346-360` only assert the upfront/monthly figures agree with each other and with the term; `grep -E 'is_finite|e38|INFINITY|f32::MAX'` over `tests/` returns no matches, so the overflow path is untested.

## Steps to Reproduce

1. POST /quotes with body {"CountryCode": "US", "RequestedCapacities": [{"QuoteCapacityType": "EC2", "Unit": "c5.large", "Quantity": 1e38}]} — every member is inside its modelled constraints (QuoteCapacity$Quantity is an unconstrained NullableFloat).
2. state::quoted_capacity_vcpus computes 1e38 * 2 vCPUs = 2e38, which is finite, so subscription_price's `!vcpus.is_finite()` guard (state.rs:2826) lets it through.
3. monthly_rate = 2e38 * 20.0 overflows f32 to +Infinity (state.rs:2831); term = +Infinity (state.rs:2832).
4. Read the answered Quote's QuoteOptions[0].PricingOptions[*].SubscriptionPricingDetails: UpfrontPrice is +Infinity, and for the ALL_UPFRONT option MonthlyRecurringPrice is inf * 0.0 = NaN.
5. Call GetQuote (or ListQuotes) on the returned QuoteId: the same non-finite figures come back, because the overflowed QuoteOption was persisted into store.quotes by create_quote.rs:307-316.
6. Repeat with a smaller Quantity (e.g. 2e34, keeping monthly_rate finite): to_the_cent's `amount * 100.0` (state.rs:2798) still overflows the term figure to +Infinity, showing the rounding helper alone is enough to produce a non-finite price.
