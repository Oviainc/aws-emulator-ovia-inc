Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0f071a90384ece90fe46e5865745f415e3851c94

# UpdateQuote re-prices a quote an order was already submitted from, orphaning the order's QuoteOptionIdentifier and moving the quote's record of that order to another Outpost

_Filed: 2026-10-28T20:31:27Z_
_Operation: UpdateQuote | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0f071a90384ece90fe46e5865745f415e3851c94`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateQuote` applies and re-derives without ever looking at the status of the quote it is changing, so a quote in `ORDER_SUBMITTED` — one `CreateOrder` has already taken an order from, and whose `SubmittedOrderId` names that order — is rewritten like a fresh one. Two concrete consequences, both of which break invariants this crate states for itself:

1. Any update that carries `RequestedCapacities`, `RequestedPaymentOptions` or `RequestedPaymentTerms` sets `reprice` and replaces `record.quote.quote_options` with a freshly derived list, and `state::quote_options` MINTS A NEW `QuoteOptionIdentifier` every time it runs. The order that was submitted from the quote keeps the OLD identifier on its `Order$QuoteOptionIdentifier`, which the quote no longer offers: `QuoteRecord::offers_option` now returns false for it, so `GetOrder` reports an option identifier that names nothing this service holds and a `CreateOrder` naming that same value is refused 404. The crate asserts the opposite in two places ("every later read of that quote names the same option", "the options this quote reports are the only ones an order can be submitted against").
2. An update that carries `OutpostIdentifier` moves the association, including on an `ORDER_SUBMITTED` quote. The quote is this store's SECOND record of an order outstanding on an Outpost (`quote_ordered_from_on_outpost`), so while the submitted order is still outstanding, pointing the quote at Outpost B makes `DeleteOutpost(B)` answer `ConflictException` with a `ResourceId` naming an order that was never placed on B, and `StartOutpostDecommission(B)` report `OUTPOST_ORDER_CANCELLABLE` for it — while the Outpost the order really was placed on loses that second record.

The crate refuses to rewrite exactly this history elsewhere: `settle_quotes` deliberately leaves a quote an order was submitted from alone because moving it "would be rewriting history", and `CreateOrder` refuses to submit from any quote not still `CREATED`. `UpdateQuote` has no such gate and no reconciliation of the order it invalidates.

## Evidence

The invariant the crate states for a quote option, in `state.rs`:

- `services/service-com-amazonaws-outposts-outpostsolafservice/src/state.rs:3453-3455` (doc of `quote_options`): "`QuoteOptionIdentifier` (`^oqo-[a-f0-9]{17}$`), minted here and persisted WITH the quote, so every later read of that quote names the same option and an order can be submitted against it."
- `src/state.rs:1053-1056` (doc of `QuoteRecord::offers_option`): "A `QuoteOptionIdentifier` is minted with the quote's options and persisted with the quote ..., so the options this quote reports are the only ones an order can be submitted against."
- `src/state.rs:3476`: `let (quote_option_identifier, _) = self.mint_id(QUOTE_OPTION_ID_PREFIX);` — a NEW id on every call of `quote_options`.

The order that references it, in `create_order.rs`:

- `src/ops/create_order.rs:316-326`: `if !record.offers_option(quote_option_identifier) { return Err(... NotFoundException ...) }` under the comment "An option of another quote, or one no quote ever minted, names no quote option this service holds".
- `src/ops/create_order.rs:463-470`: "An order submitted from a quote is what makes that quote's status `ORDER_SUBMITTED` and its `SubmittedOrderId` present" — `record.quote.quote_status = Some(QUOTE_STATUS_ORDER_SUBMITTED...); record.quote.submitted_order_id = Some(order_id);`

The code that breaks it, in `update_quote.rs` — no status is read anywhere in the file (`grep -n 'quote_status|ORDER_SUBMITTED|submitted_order_id' src/ops/update_quote.rs` → no matches):

- `src/ops/update_quote.rs:294-296`: `let reprice = input.requested_capacities.is_some() || input.requested_payment_options.is_some() || input.requested_payment_terms.is_some();`
- `src/ops/update_quote.rs:348-355` and `372-374`: `let derived_options = match reprice { true => Some(store.quote_options(...)), ... }` then `record.quote.quote_options = Some(quote_options);`
- `src/ops/update_quote.rs:329-333`: `Association::Removed => record.quote.outpost_arn = None, Association::Set(outpost_arn) => record.quote.outpost_arn = Some(outpost_arn),`

The same history the crate refuses to rewrite elsewhere:

- `src/state.rs:3793-3795` (doc of `settle_quotes`): "Only a quote still [`QUOTE_STATUS_CREATED`] is moved: the expiry of a quote an order was already submitted from would be rewriting history, and one already `EXPIRED` has nowhere left to go."
- `src/ops/create_order.rs:274-279`: "only a quote still `CREATED` can take one".

The second record of an outstanding order that the association move relocates:

- `src/state.rs:3986-4003` `quote_ordered_from_on_outpost` matches on `record.associated_outpost_id() != Some(outpost_id)`; it is consumed by `src/state.rs:4069` (`unfinished_work_on_outpost`, what `DeleteOutpost` refuses on) and `src/state.rs:4194-4199` (`blocking_resource_types`, what `StartOutpostDecommission` reports).

Test coverage: `tests/all/quote_update.rs:6-31` states the rule the suite pins ("applies exactly the members its request carries ... re-derives the ones whose source it replaced") and never mentions a submitted order — `grep -n 'ORDER_SUBMITTED|SubmittedOrderId|QuoteOptionIdentifier' tests/all/quote_update.rs` → no matches, and `tests/all/order_quote_references.rs` (which does drive `ORDER_SUBMITTED`, lines 406 and 610) never calls `UpdateQuote`. So the post-submit update is untested in either suite.

## Steps to Reproduce

1. Create a site with an operating and a shipping address in country `US`, create Outpost A on it, and place a `CreateOrder` with `LineItems` so the Outpost settles `ACTIVE` (the state the quote's ordering requirements check).
2. `CreateQuote` with `CountryCode: US` and `OutpostIdentifier: <A>`; note `QuoteId` and `QuoteOptions[0].QuoteOptionIdentifier` (call it OQO1).
3. `CreateOrder` with `OutpostIdentifier: <A>`, `QuoteIdentifier: <QuoteId>`, `QuoteOptionIdentifier: OQO1` → 200. `GetQuote` now reports `QuoteStatus: ORDER_SUBMITTED` and `SubmittedOrderId: <OrderId>`.
4. `PATCH /quotes/<QuoteId>` with `{"RequestedPaymentTerms":["ONE_YEAR"]}` → 200. `GetQuote` now reports a DIFFERENT `QuoteOptions[0].QuoteOptionIdentifier` (OQO2), while `GetOrder <OrderId>` still reports `QuoteOptionIdentifier: OQO1` — an option the quote no longer offers (a fresh `CreateOrder` naming OQO1 is refused 404 by the `offers_option` check).
5. Second symptom: create Outpost B, and while the submitted order is still outstanding send `PATCH /quotes/<QuoteId>` with `{"OutpostIdentifier":"<B>"}`. `DELETE /outposts/<B>` is now refused 409 `ConflictException` with `ResourceId` naming the order that was placed on Outpost A, and `POST /outposts/<B>/decommission` reports `BlockingResourceTypes: [OUTPOST_ORDER_CANCELLABLE]` for that same foreign order.
