Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: ec42ee63c3cbc91c5bb52d083d61328ac6a6aee0

# Non-final transactions are listed by default: an absent confirmationStatusFilter must restrict the listing to transactions that have reached finality

_Filed: 2026-10-24T16:19:24Z_
_Operation: ListTransactions | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ec42ee63c3cbc91c5bb52d083d61328ac6a6aee0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListTransactions` treats an ABSENT `confirmationStatusFilter` as "include every confirmation status", so transactions that have not reached finality are returned to a caller that never asked for them.

Expected (per the member's own documentation): the filter is the ONLY way a caller opts in to non-final transactions, so a request that carries no `confirmationStatusFilter` lists only transactions whose `confirmationStatus` is `FINAL`. A request carrying `confirmationStatusFilter: {include: ["NONFINAL"]}` (or `["FINAL","NONFINAL"]`) adds the non-final ones on top of the final ones, which are "always part of the response".

Actual: src/ops/list_transactions.rs:214-238 builds `wanted_statuses` as `Option<Vec<String>>` and keeps a transaction when `status == FINAL || wanted_statuses.is_none_or(|w| w.contains(status))`. The `is_none_or` makes an absent filter a pass-through, so a NONFINAL transaction is listed in the default (unfiltered) call. The emulator therefore answers the three-way distinction AWS draws (no filter / include NONFINAL / include FINAL) with only a two-way one: no filter and `include: ["NONFINAL"]` return the identical page.

The fixture makes this directly observable: `chain.rs` marks the transactions of each network's LAST block `CONFIRMATION_NONFINAL` (src/state/chain.rs, `confirmation_status: if block == last_block { CONFIRMATION_NONFINAL } else { CONFIRMATION_FINAL }`), so a plain `ListTransactions` for the busiest wallet returns those tip transactions, and `TransactionOutputItem$confirmationStatus` in the response shows `NONFINAL` for them. A client that lists an address's history to reconcile settled funds (the documented purpose of finality) counts tip transactions that AWS would have withheld until it asked for them, and a client that uses the filter to DISCOVER pending activity sees no difference between asking and not asking.

## Evidence

Spec: smithy_query(query="doc", shape="ListTransactionsInput", member="confirmationStatusFilter") -> "This filter is used to include transactions in the response that haven't reached finality. Transactions that have reached finality are always part of the response." The filter is described as the mechanism that INCLUDES non-final transactions — which presupposes they are excluded when it is absent; if they were included by default, the member's only possible use would be to EXCLUDE them, which contradicts the second sentence (`ConfirmationStatusFilter$include` repeats it: "The container to determine whether to list results that have only reached finality. Transactions that have reached finality are always part of the response."). `ConfirmationStatus` = FINAL | NONFINAL (smithy_query(query="shape", shape="ConfirmationStatusFilter"): `include` is @required, @length(min: 1)).

Handler: src/ops/list_transactions.rs:214-238 (`wanted_statuses ... .is_none_or(...)`). The FINAL-always-included half of the rule is implemented correctly; only the default for an absent filter is wrong.

Fixture: src/state/chain.rs sets `confirmation_status` to CONFIRMATION_NONFINAL for the last block of every network's ledger, so each network has NONFINAL transactions reachable from the busiest wallet's default listing.

Test coverage: no test asserts the AWS default; two tests PIN the emulator's current default and will need updating with the fix — tests/all/derived_chain_history.rs:342-372 computes `expected = whole` (the unfiltered listing) for `include` containing NONFINAL and `final_only` otherwise, i.e. it asserts that the unfiltered listing already equals the NONFINAL-inclusive one; tests/all/chain_query_slice.rs:593-600 uses the same three filter rows over a baseline captured without a filter. Not covered by the open ListTransactions/pagination issue (20261024T122923Z), which is only about the default page size.

## Steps to Reproduce

1. POST /list-transactions {"network": "ETHEREUM_MAINNET", "address": "0xd8da6bf26964af9d7eed9e03e53415d37aa96045", "maxResults": 250} with NO confirmationStatusFilter.
2. Inspect the returned items' `confirmationStatus`: entries with "NONFINAL" are present (the fixture's tip block), where AWS returns only FINAL items for an unfiltered request.
3. POST the same body with {"confirmationStatusFilter": {"include": ["NONFINAL"]}} and compare: the two pages are identical, so the opt-in has no observable effect — against AWS the second page is a superset of the first.
4. POST the same body with {"confirmationStatusFilter": {"include": ["FINAL"]}}: this is the only way to get the page AWS would have returned with no filter at all.
