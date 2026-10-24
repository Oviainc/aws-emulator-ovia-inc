Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ec42ee63c3cbc91c5bb52d083d61328ac6a6aee0

# Events of non-final transactions are listed by default, so an absent confirmationStatusFilter is not the finality-only listing

_Filed: 2026-10-24T16:19:24Z_
_Operation: ListFilteredTransactionEvents | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ec42ee63c3cbc91c5bb52d083d61328ac6a6aee0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListFilteredTransactionEvents` applies the same default as `ListTransactions`: when the request carries no `confirmationStatusFilter`, events belonging to transactions that have NOT reached finality are returned anyway.

Expected: `ConfirmationStatusFilter` is the opt-in for non-final results ("Transactions that have reached finality are always part of the response"), so a request without the filter lists only events whose `confirmationStatus` is `FINAL`, and `include: ["NONFINAL"]` adds the non-final ones on top.

Actual: src/ops/list_filtered_transaction_events.rs:322-325 and 347-359 compute `refused_status = status != FINAL && wanted_statuses.is_some_and(|w| !w.contains(status))`. With no filter, `wanted_statuses` is `None`, `is_some_and` is false, and the NONFINAL transaction's events are kept. As on `ListTransactions`, the unfiltered page and the `include: ["NONFINAL"]` page are byte-identical, and the only way to obtain AWS's default page is to send `include: ["FINAL"]` explicitly.

The fixture's tip block is NONFINAL on every network (src/state/chain.rs), and that block's BITCOIN_VOUT events are exactly the ones a client reconciling unspent outputs would treat as settled — so the divergence lands on the newest, most-consulted end of the listing and each affected event carries `confirmationStatus: "NONFINAL"` in the body that AWS would not have sent.

## Evidence

Spec: the shared filter shape, smithy_query(query="shape", shape="ConfirmationStatusFilter") -> `include` @required @length(min: 1) of ConfirmationStatus (FINAL | NONFINAL), documented "The container to determine whether to list results that have only reached finality. Transactions that have reached finality are always part of the response." The sibling operation states the rule explicitly for the same filter member: smithy_query(query="doc", shape="ListTransactionsInput", member="confirmationStatusFilter") -> "This filter is used to include transactions in the response that haven't reached finality" — i.e. the filter adds non-final results rather than removing them, so an absent filter is the finality-only listing. `ListFilteredTransactionEventsInput$confirmationStatusFilter` carries no prose of its own and targets the same shape.

Handler: src/ops/list_filtered_transaction_events.rs:322-325 (`wanted_statuses`) and 347-359 (`refused_status ... is_some_and`).

Fixture: src/state/chain.rs marks the last block of each network's ledger CONFIRMATION_NONFINAL, and the Bitcoin fixture emits a BITCOIN_VIN/BITCOIN_VOUT pair per transaction, so those tip events are selected by the recipient filter.

Test coverage: tests/all/chain_query_slice.rs:1680-1714 pins the CURRENT default — it captures `whole` from a request with no `confirmationStatusFilter` and then asserts each filter row equals the subset of `whole`, so the unfiltered baseline is assumed to already contain the NONFINAL events; the fix must update that baseline. No test asserts AWS's default. Separate from the open ListTransactionEvents/wrong-behavior issue (20261024T010728Z), which is about `TransactionEvent$tokenId` and Bitcoin value denomination, not about which transactions' events are selected.

## Steps to Reproduce

1. POST /list-filtered-transaction-events {"network": "BITCOIN_MAINNET", "addressIdentifierFilter": {"transactionEventToAddress": ["1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa"]}, "maxResults": 250} with NO confirmationStatusFilter.
2. Inspect `events[*].confirmationStatus`: entries with "NONFINAL" (the fixture's tip block) are present, where AWS returns only FINAL events for an unfiltered request.
3. Repeat with {"confirmationStatusFilter": {"include": ["NONFINAL"]}}: the page is identical to step 1, so the documented opt-in changes nothing.
4. Repeat with {"confirmationStatusFilter": {"include": ["FINAL"]}} to see the page AWS returns when no filter is sent at all.
