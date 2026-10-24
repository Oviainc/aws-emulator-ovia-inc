Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: f0788135e0bb0704427b37eee4190850f5fa33c6

# Paginated listings default to a 250-item page instead of the documented default of 100

_Filed: 2026-10-24T12:29:23Z_
_Operation: ListTransactions | Class: pagination_

> _Note: this issue was identified against the code base at commit `f0788135e0bb0704427b37eee4190850f5fa33c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated operation of this service documents `maxResults` as "Default: 100". The emulator's shared pager defaults to 250 when the request carries no `maxResults`, so a first page can hold up to 250 items and omits the `nextToken` the real service would hand back after 100.

Expected (ListTransactions, no `maxResults`): at most 100 `transactions` in the response, plus a `nextToken` when the address has more than 100 transactions in the indexed window.

Actual: the handler passes `input.max_results` straight to `crate::state::page`, which substitutes `MAX_PAGE_SIZE` (250) for an absent value (`let limit = max_results.unwrap_or(MAX_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE)`, src/state.rs:1018). The fixture ledger holds 160 transactions per network (`FIXTURE_TX_COUNT = 160`, src/state/chain.rs:91) and its busiest wallet takes part in every one of them, so a `ListTransactions` call for that address with no `maxResults` answers 160 items and `nextToken: null`, where the real service answers 100 items and a continuation token. A client that paginates by presence of `nextToken` therefore sees a single page where AWS gives it two, and a client that sizes buffers from the documented default over-reads.

The same default is used by `ListTokenBalances`, `ListAssetContracts`, `ListTransactionEvents` and `ListFilteredTransactionEvents` (all five call `crate::state::page`, and all five document "Default: 100"), so the fix belongs in the shared pager rather than in one handler; `ListTransactions` is the operation where today's fixture volume makes the divergence observable. Note the 250 clamp itself is correct as the MAXIMUM (the `@range(max: 250)` on `maxResults` and the `@length(max: 250)` on each output list) — only its use as the DEFAULT is wrong.

## Evidence

Spec: smithy_query(query="doc", shape="ListTransactionsInput", member="maxResults") -> "The maximum number of transactions to list.\n Default: 100 ... To retrieve the next set of results, make another request with the returned nextToken value." The identical "Default: 100" sentence appears on `ListTokenBalancesInput$maxResults`, `ListTransactionEventsInput$maxResults`, `ListAssetContractsInput$maxResults` and `ListFilteredTransactionEventsInput$maxResults`. The operations are `@paginated(inputToken=nextToken, outputToken=nextToken, items=transactions, pageSize=maxResults)`.

Handler: src/ops/list_transactions.rs:259 `crate::state::page(all, input.max_results, input.next_token.as_deref())`; the default is applied at src/state.rs:1015-1018 where the comment states "absent, the page is the modelled maximum" — i.e. 250 is deliberately used as the default. Fixture volume: src/state/chain.rs:91 `FIXTURE_TX_COUNT: i64 = 160`, with the busiest wallet in every transaction (src/state/chain.rs:20-22).

Test coverage: no test pins the default page size. tests/all/chain_query_slice.rs:400-460 (`drain`) only asserts a page never EXCEEDS an explicitly requested `maxResults`, that no item repeats and that the walk terminates; its `None` rows (lines 1426, 1569) exercise paging invariance, not the default. tests/all/error_response_shape.rs and tests/all/error_contract.rs only exercise the `@range` bounds 1/250/0/251 of an explicit `maxResults`.

## Steps to Reproduce

1. POST /list-transactions {"network": "ETHEREUM_MAINNET", "address": "0xd8da6bf26964af9d7eed9e03e53415d37aa96045"} with NO maxResults member.
2. Count the `transactions` array: the emulator returns more than 100 items (up to the 160 the fixture holds for that address) and `nextToken` is absent/null.
3. Expected: at most 100 items and a `nextToken` that continues the listing, because `maxResults` documents "Default: 100".
4. Repeat with an explicit {"maxResults": 100} and observe that only this explicit form produces the page size the service documents as the default.
