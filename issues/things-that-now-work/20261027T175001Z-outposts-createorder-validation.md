Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: f9882e4630ff46bd32c7db4f9b7db9452b91004b

# CreateOrder never checks the quote it is taken from: a quote of another Outpost, failing requirements, and a quote option no quote ever minted are all accepted

_Filed: 2026-10-27T17:50:01Z_
_Operation: CreateOrder | Class: validation_

> _Note: this issue was identified against the code base at commit `f9882e4630ff46bd32c7db4f9b7db9452b91004b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When `CreateOrder` is given a `QuoteIdentifier`, the only thing the handler checks about that quote is its `QuoteStatus` (it must be `CREATED`). Three things the crate itself computes about a quote are never consulted:

1. **The quote's Outpost association.** The quote record carries `Quote$OutpostArn`, and `state.rs` states that that association "is also what makes an order submitted from the quote an order OF an Outpost". `CreateOrder` resolves its own `OutpostIdentifier` independently and never compares the two, so an order placed on Outpost B can be submitted from a quote associated with Outpost A. The handler then marks A's quote `ORDER_SUBMITTED` with `SubmittedOrderId` naming an order that lives on B. That corrupts the second record the crate keeps of an outstanding order: `quote_ordered_from_on_outpost` (state.rs:3730) matches on the QUOTE's Outpost, so `DeleteOutpost(A)` / `StartOutpostDecommission(A)` are afterwards refused with a `ConflictException` naming an order that was never placed on A, while nothing of A is actually outstanding.

2. **The quote's `OrderingRequirements`.** `CreateQuote` and `UpdateQuote` both compute them through the existing `store.ordering_requirements(...)` helper, documented as "The requirements that must be met before an order can be submitted for a quote" — including `OUTPOST_ID_MISSING_ON_QUOTE_ERROR`, `OUTPOST_ACTIVE_CHECK_ERROR`, the two address checks and `COUNTRY_CODE_MISMATCH_CHECK_ERROR`. `CreateOrder`, the one operation that submits an order from a quote, never calls that helper, so a quote whose requirements are all `FAIL` (no Outpost association at all, a site with no operating/shipping address, a country code that is not the site's) still takes an order.

3. **The `QuoteOptionIdentifier`.** `state.rs` says the identifier "can only ever be obtained from one of them" and is "persisted WITH the quote … so every later read of that quote names the same option and an order can be submitted against it". The handler stores whatever well-formed `oqo-…` string arrives, without checking that it is an option of the named quote — or that any quote was named at all — so the order persists a dangling reference nothing in the store can resolve.

This is the same defect class the crate already fixed for the sibling operation: `StartCapacityTask` resolves each of its OPTIONAL references (`OrderId`, `AssetId`) against the store and answers `NotFoundException` rather than storing a dangling reference (fixed issue 20261024T134249Z-outposts-startcapacitytask-validation).

## Evidence

The crate's own rule about what a quote's Outpost association means — `src/state.rs:2513-2516`:

    /// The `OrderingRequirementType` of the check that the quote names an Outpost: "the
    /// quote must be associated with an Outpost before submitting an order", which is
    /// also what makes an order submitted from the quote an order OF an Outpost.
    pub const ORDERING_REQUIREMENT_OUTPOST_ID_MISSING: &str = "OUTPOST_ID_MISSING_ON_QUOTE_ERROR";

What the handler actually checks about the quote — `src/ops/create_order.rs:259-284` (the whole quote check; `status` is the only thing read):

            let status = store
                .quote(&region, quote_id)
                .map(|record| record.quote.quote_status.clone().unwrap_or_default())
                ...
            if status != crate::state::QUOTE_STATUS_CREATED {
                return Err(CreateOrderErrorModeled::ConflictException(ConflictException {

and the references it then persists unchecked — `src/ops/create_order.rs:330-331`:

            quote_identifier: quote_id.clone(),
            quote_option_identifier: crate::state::non_empty(input.quote_option_identifier),

The helper the crate already has for the submit-time checks, called by two siblings and not by this one — `src/state.rs:3381-3383` / `3410`:

    /// The requirements that must be met before an order can be submitted for a quote
    /// of `country_code` associated with the Outpost `outpost_id` names in `region`
    pub fn ordering_requirements(

called at `src/ops/create_quote.rs:267-268` (`let ordering_requirements = store.ordering_requirements(&region, &input.country_code, outpost_id.as_deref());`) and `src/ops/update_quote.rs:367`; `grep` shows no call in `create_order.rs`.

The rule about where a `QuoteOptionIdentifier` can come from — `src/state.rs:3258-3268`:

    /// "A quote provides pricing and configuration options based on the requested
    /// capacity", and `CreateOrder`'s `QuoteOptionIdentifier` can only ever be
    /// obtained from one of them, so a quote this service answers carries an option a
    /// caller can order from.
    /// * `QuoteOptionIdentifier` (`^oqo-[a-f0-9]{17}$`), minted here and persisted
    ///   WITH the quote, so every later read of that quote names the same option and
    ///   an order can be submitted against it.

The sibling that DOES resolve its optional references — `src/ops/start_capacity_task.rs:286-298`:

        if let Some(order_id) = order_id.as_deref() {
            if store.order(&region, order_id).is_none() {
                return Err(StartCapacityTaskErrorModeled::NotFoundException(

The knock-on wrong refusal — `src/state.rs:3735-3752` matches a quote by the QUOTE's `OutpostArn` and then only asks whether the SUBMITTED order is outstanding:

            let quote_outpost = record.quote.outpost_arn.as_deref().and_then(outpost_id_in_arn);
            if quote_outpost != Some(outpost_id)
                || record.quote.quote_status.as_deref() != Some(QUOTE_STATUS_ORDER_SUBMITTED)

Test coverage today: `tests/all/order_lifecycle.rs:56-57` defines `const QUOTE_OPTION_ID: &str = "oqo-abcdef01234567890";` ("A `QuoteOptionIdentifier` of the modelled form" — an id no quote ever minted) and `:251` sends it while `:304-308` asserts it is echoed back, so the current acceptance of a dangling option id is pinned by a case that states no reason for it; no test covers an order submitted from a quote associated with a different Outpost, or from a quote whose ordering requirements fail.

## Steps to Reproduce

1. CreateSite, then CreateOutpost twice in the same region — call them A and B.
2. CreateQuote with OutpostIdentifier=A and a CountryCode that is not the site's country; note the minted QuoteOptionIdentifier in Quote.QuoteOptions[0] and that Quote.OrderingRequirements reports FAIL rows (operating/shipping address, country-code mismatch).
3. CreateOrder with OutpostIdentifier=B, QuoteIdentifier=<the quote of A>, QuoteOptionIdentifier="oqo-abcdef01234567890" (an id this service never minted), one LineItem and PaymentOption=ALL_UPFRONT — the call answers 200 and the Order echoes the unknown quote option.
4. GetQuote on A's quote: QuoteStatus is ORDER_SUBMITTED and SubmittedOrderId names the order placed on B, although every ordering requirement of that quote failed.
5. DeleteOutpost(A) — refused 409 ConflictException naming the order that was placed on B, so A can never be deleted even though nothing is outstanding on it.
