Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 15c0cfc29e2643a7b1edb3bac08891ffd2eb9b9c

# The instant-window rule (forwards-only refusal + inclusive bounds) is hand-rolled in both window-filtered listings

_Filed: 2026-10-27T18:06:47Z_
_Operation: ListFilteredTransactionEvents | Class: maintainability_

> _Note: this issue was identified against the code base at commit `15c0cfc29e2643a7b1edb3bac08891ffd2eb9b9c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two handlers of this crate each write out the same two-part instant-window rule instead of sharing one implementation, and the two copies are already spelled with opposite polarity, which is exactly how they can drift:

1. THE REFUSAL: "both bounds present and the start is after the end -> this service's own ValidationException". `ListTransactions` builds it from `fromBlockchainInstant`/`toBlockchainInstant`; `ListFilteredTransactionEvents` builds the identical branch from `timeFilter.from`/`timeFilter.to`. Only the `ValidationExceptionField$name` and the sentence differ — the rule, the `instant_to_secs` decode and the `if let (Some, Some)` + `>` test are the same code twice.

2. THE FILTER: "keep a record whose instant is inside the (inclusive) window". `ListTransactions` writes it as two `is_none_or` filters with `>=` / `<=`; `ListFilteredTransactionEvents` writes the same predicate inverted, as `is_some_and` with `<` / `>` plus a `continue`. Two spellings of one inclusive-bounds rule: a later change to one boundary (e.g. making a bound exclusive, or treating an absent bound differently) silently applies to only one listing, and nothing in the crate ties them together.

Every other cross-operation rule in this crate IS single-homed — `crate::state::confirmation_status_listed` for the finality rule, `crate::state::page` for the page walk, `crate::state::TransactionSelector::from_input` for the hash/id pair, `crate::state::HoldingKey::from_identifiers` for a balance key — so the window rule is the outlier. Factoring it into `crate::state` / `crate::state::validation` (parameterised by the two member paths the entry names) leaves every response byte-for-byte identical and every existing test passing.

## Evidence

Copy 1 — the refusal, services/service-com-amazonaws-managedblockchainquery-tietonchainqueryservice/src/ops/list_transactions.rs:196-209:
```
        let from = crate::state::instant_to_secs(input.from_blockchain_instant.as_ref());
        let to = crate::state::instant_to_secs(input.to_blockchain_instant.as_ref());
        if let (Some(earliest), Some(latest)) = (from, to) {
            if earliest > latest {
                return Err(ListTransactionsErrorModeled::ValidationException(
                    crate::state::field_validation_exception(
                        "BlockchainInstant",
                        "The 'time' field in fromBlockchainInstant must be less than or equal \
                         to the 'time' field in toBlockchainInstant.",
                    ),
                )
                .into());
```

Copy 2 — the same refusal, src/ops/list_filtered_transaction_events.rs:266-288:
```
        let from = crate::state::instant_to_secs(
            input
                .time_filter
                .as_ref()
                .and_then(|filter| filter.from.as_ref()),
        );
        ...
        // A window that ends before it starts selects nothing a caller could have meant,
        // so it is refused rather than answered with an empty page.
        if let (Some(earliest), Some(latest)) = (from, to) {
            if earliest > latest {
                fields.push(crate::state::validation_field(
                    "timeFilter.from",
                    "The 'time' field in timeFilter.from must be less than or equal to \
                     the 'time' field in timeFilter.to.",
                ));
```
(The comment "A window that ends before it starts selects nothing a caller could have meant, so it is refused rather than answered with an empty page" is itself duplicated — src/ops/list_transactions.rs:190-195 states the same sentence.)

Copy 1 — the inclusive-window filter, src/ops/list_transactions.rs:230-235:
```
            .filter(|transaction| {
                from.is_none_or(|earliest| transaction.transaction_timestamp >= earliest)
            })
            .filter(|transaction| {
                to.is_none_or(|latest| transaction.transaction_timestamp <= latest)
            })
```

Copy 2 — the same rule, inverted, src/ops/list_filtered_transaction_events.rs:362-367:
```
            if from.is_some_and(|earliest| instant < earliest)
                || to.is_some_and(|latest| instant > latest)
                || refused_status
            {
                continue;
            }
```

That this crate single-homes its other cross-operation rules, src/state.rs:87-102:
```
/// Whether a record whose `confirmationStatus` is `status` belongs in a listing whose
/// request carried `include` as its `ConfirmationStatusFilter$include` ...
pub fn confirmation_status_listed(status: &str, include: Option<&[String]>) -> bool {
```
and src/state.rs:1058-1061: "How a request names one transaction: by `transactionHash` or by `transactionId`. /// Shared by `GetTransaction` and `ListTransactionEvents`, which model the same pair of members and so accept the same combinations."

Test coverage that must stay green and unchanged (both copies are already pinned):
- tests/all/error_response_shape.rs:745-756 — `case: "ListTransactions: a window that ends before it starts"` with `field: "BlockchainInstant"` ("Both members target `BlockchainInstant`, and the rule is about the pair.").
- tests/all/response_fidelity.rs:340-382 — the `bad_window` rows for ListFilteredTransactionEvents expecting the `"timeFilter.from"` field entry, alone and combined with `"network"`/`"addressIdentifierFilter"`.
- tests/all/derived_chain_history.rs:303-322 — every month-long window listing, asserting `instant >= from && instant <= to` (the inclusive-bounds half).
- tests/all/chain_query_slice.rs:740-772 — window rows including the reversed `fromBlockchainInstant`/`toBlockchainInstant` pair.

## Steps to Reproduce

1. Read services/service-com-amazonaws-managedblockchainquery-tietonchainqueryservice/src/ops/list_transactions.rs:196-209 and src/ops/list_filtered_transaction_events.rs:266-288: the same 'both bounds present and start > end -> ValidationException' branch is written twice, differing only in the field name and sentence.
2. Read src/ops/list_transactions.rs:230-235 and src/ops/list_filtered_transaction_events.rs:362-367: the same inclusive-window predicate is written twice, once as `is_none_or` with `>=`/`<=` and once as the inverted `is_some_and` with `<`/`>`.
3. Compare with src/state.rs:87-102 (confirmation_status_listed), src/state.rs:1058-1150 (TransactionSelector) and src/state.rs:1152-1185 (page): every other rule two operations share is defined once in state.rs and called from both handlers.
4. Change one boundary in either copy (for example make `to` exclusive) and observe that only one of the two listings changes, with no compile error and no shared test tying the two together.
