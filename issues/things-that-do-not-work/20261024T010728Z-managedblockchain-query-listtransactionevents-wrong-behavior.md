Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# TransactionEvent omits tokenId for native-token events, and Bitcoin values are reported in satoshi instead of BTC

_Filed: 2026-10-24T01:07:28Z_
_Operation: ListTransactionEvents | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListTransactionEvents` projects stored events onto the modelled `TransactionEvent` in `StoredTransaction::events()` (src/state.rs:258-285). Two members of that projection diverge from the real service.

(1) `TransactionEvent$tokenId` is never populated for a NATIVE-token event. The fixture builds `ETH_TRANSFER` / `INTERNAL_ETH_TRANSFER` events through `native_transfer` (src/state/chain.rs:251-259) and `BITCOIN_VIN` / `BITCOIN_VOUT` events in `bitcoin_fixture` (src/state/chain.rs:581-601); none of them sets `token_id`, so it defaults to `None` and the member is omitted from the response. Expected: a native-token event carries the 3-character abbreviation of the network's own currency — `btc` on the Bitcoin networks, `eth` on the Ethereum ones. The emulator ALREADY knows this value: `StoredEvent::holding()` (src/state.rs:342-362) maps exactly these event types to `NATIVE_TOKEN_BTC`/`NATIVE_TOKEN_ETH` to key the holdings index, and `ListTokenBalances` reports the same `tokenId` inside `TokenBalance$tokenIdentifier`. So an event and the balance it moves disagree about which token moved, and a client that joins an event stream to a token (the normal way to attribute a native transfer) finds no token on the event.

(2) `value` is rendered as the raw integer the ledger stores — `value: event.value.map(|amount| amount.to_string())` over an `i128` held in satoshi on the Bitcoin networks (`BTC: i128 = 100_000_000`, src/state.rs:1152; `NATIVE_OUT_SATOSHI`, `BITCOIN_FEE`, `WALLET_OPENING_SATOSHI`, src/state/chain.rs:144-184). The real service denominates Bitcoin event values in BTC with 8 decimal places, so a 546-satoshi output is reported as `"0.00000546"` and not as `"546"`; the emulator's fixture outputs (BTC/50 = 2,000,000 satoshi) come back as `"2000000"` where the real service would send `"0.02"` — off by 10^8 in magnitude and in format. The btc `TokenBalance$balance` values derived from those same event amounts (`derive_holdings`) carry the same unit error.

The projection is shared with `ListFilteredTransactionEvents`, so one fix covers both; that operation's event body is already owned by the closed issue 20261022T213114Z, this one is `ListTransactionEvents`, which no issue has covered.

## Evidence

Spec: smithy_query(query="shape", shape="TransactionEvent") -> `tokenId` (QueryTokenId, @pattern ^[a-zA-Z0-9]{1,66}$) "The unique identifier for the token involved in the transaction"; `value` (String) "The value that was transacted". smithy_query(query="shape", shape="TokenIdentifier") member `tokenId` documents the rule for the value: "For native tokens, use the 3 character abbreviation that best matches your token. For example, btc for Bitcoin, eth for Ether".

Recorded AWS behaviour, quoted verbatim in this repo's own issue 20261022T213114Z-managedblockchain-query-query-responses-diverge-wrong-chain-data (Cause 2 evidence): a real BITCOIN_VOUT event reads `{'eventType': 'BITCOIN_VOUT', 'network': 'BITCOIN_MAINNET', 'to': '1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa', 'tokenId': 'btc', 'value': '0.00000546', 'voutIndex': 0, 'voutSpent': False, ...}` — i.e. AWS sends tokenId 'btc' on a native Bitcoin event and denominates `value` in BTC (546 satoshi -> 0.00000546).

Handler/code: src/ops/list_transaction_events.rs:197-205 collects `StoredTransaction::events()` verbatim; src/state.rs:258-285 is the projection (`token_id: event.token_id.clone()`, `value: event.value.map(|amount| amount.to_string())`); src/state/chain.rs:251-259 and :581-601 are the native-event constructors that leave `token_id` unset.

Test coverage: tests/all/derived_chain_history.rs:480-507 is the only place event bodies are asserted — it checks `transactionHash`, `network` and that `eventType` is a modelled enum value, and never looks at `tokenId` or `value`. No test anywhere in tests/all asserts an event's `tokenId` (grep for token_id/tokenId in tests/ returns only tokenIdentifier/balance rows).

## Steps to Reproduce

1. POST /list-transactions {"address": <the Ethereum mainnet fixture's busiest wallet, 0xd8da6bf26964af9d7eed9e03e53415d37aa96045>, "network": "ETHEREUM_MAINNET"} and take a transactionHash from the page.
2. POST /list-transaction-events {"network": "ETHEREUM_MAINNET", "transactionHash": <that hash>} and inspect an ETH_TRANSFER or INTERNAL_ETH_TRANSFER entry: `tokenId` is absent where the real service sends "eth".
3. POST /list-transactions {"address": "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa", "network": "BITCOIN_MAINNET"}, take a transactionHash, then POST /list-transaction-events for it.
4. Inspect the BITCOIN_VIN / BITCOIN_VOUT entries: `tokenId` is absent where the real service sends "btc", and `value` is a whole satoshi count (e.g. "2000000") where the real service sends a BTC-denominated decimal (e.g. "0.02").
5. Cross-check the same holding via POST /get-token-balance {"tokenIdentifier": {"network": "BITCOIN_MAINNET", "tokenId": "btc"}, "ownerIdentifier": {"address": "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa"}} — the balance is the satoshi sum of those same event values, carrying the same unit error.
