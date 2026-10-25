Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 30fa86cd244b801d1c6064a0a321e2bf32f37967

# TokenBalance$balance is reported in the chain's smallest unit (wei/satoshi/raw token units) instead of the main-unit decimal

_Filed: 2026-10-24T13:54:06Z_
_Operation: GetTokenBalance | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `30fa86cd244b801d1c6064a0a321e2bf32f37967`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetTokenBalance` answers out of `ComAmazonawsManagedblockchainqueryTietonchainqueryserviceState::read_token_balance` (src/state.rs:739-776), which finishes in `StoredHolding::token_balance` (src/state.rs:506-524). That projection renders the stored amount as the raw integer the ledger holds: `balance: record.balance.to_string()` over an `i128`.

The stored `i128` is in the chain's SMALLEST unit, by the fixture's own declaration: `WALLET_OPENING_WEI = 1_000 * ETH` and the transfer amounts `NATIVE_IN_WEI`/`NATIVE_OUT_WEI`/`NATIVE_INTERNAL_WEI` are wei (src/state/chain.rs:132-184, documented as \"in its smallest unit (wei, satoshi)\"), `WALLET_OPENING_SATOSHI = 1_000 * BTC` and `NATIVE_OUT_SATOSHI` are satoshi, and the well-known ERC-20 holdings are explicitly scaled into raw units by each contract's own `decimals` (`TOKEN_UNITS_HELD * unit` where `unit = 10^decimals`, src/state/chain.rs:160-164 and 479-504).

Expected: this service denominates a currency amount in the token's MAIN unit as a decimal string. A wallet holding 1,000 ETH reports `\"1000\"` (or `\"1000.000000000000000000\"`), not `\"1000000000000000000000\"`; a 1,000-BTC holding reports `\"1000\"`, not `\"100000000000\"`; and a USDC (6-decimal) holding of 1,000,000 whole tokens reports `\"1000000\"`, not `\"1000000000000\"`.

Actual: the balance comes back as a whole wei / satoshi / raw-token-unit integer — off by 10^18 (ETH), 10^8 (BTC) and 10^decimals (ERC-20) in both magnitude and format. A client that renders, sums, compares or thresholds a balance reads a number many orders of magnitude too large, and `lastUpdatedTime`/`atBlockchainInstant` beside it are correct, so nothing signals the mismatch.

The same projection serves two other operations — `ListTokenBalances` (src/ops/list_token_balances.rs:228-235 calls `holding.token_balance(...)`) and `BatchGetTokenBalance` (src/ops/batch_get_token_balance.rs:225-240 copies the same `TokenBalance` members into `BatchGetTokenBalanceOutputItem`) — so the fix belongs in `StoredHolding::token_balance` (plus the denomination the holdings are derived in, `derive_holdings`, src/state.rs:1471-1511) rather than in one handler; `GetTokenBalance` is the single-value read where the divergence is most directly observable.

Not covered by the open ListTransactionEvents issue (20261024T010728Z), which is scoped to `TransactionEvent$tokenId` and the Bitcoin EVENT `value`: it mentions btc balances only in passing and says nothing about the Ethereum (wei) or ERC-20 (raw-unit) balances, which are the bulk of this service's balance traffic, and no issue covers the `TokenBalance` projection as such.

## Evidence

Spec: smithy_query(query="shape", shape="TokenBalance") -> `balance` (String, @required) \"The container of the token balance\"; the model fixes no unit, so the denomination is recorded behaviour. smithy_query(query="shape", shape="ContractMetadata") -> `decimals` (Integer) \"The decimals used by the token contract\" — the scaling factor this service itself publishes, and which the emulator already stores for every contract it holds (ETHEREUM_MAINNET_ERC20 rows carry USDC/USDT 6, DAI/WETH/LINK 18, WBTC 8, src/state.rs:1231-1268; the synthetic EFT carries 18, src/state/chain.rs:300-304).

Recorded AWS behaviour already captured in this repo: issue 20261022T213114Z-managedblockchain-query-query-responses-diverge-wrong-chain-data quotes a real `BITCOIN_VOUT` event as `{'eventType': 'BITCOIN_VOUT', ..., 'tokenId': 'btc', 'value': '0.00000546', ...}` — 546 satoshi rendered as a BTC-denominated decimal, i.e. this service reports native-currency amounts in the main unit. The repo has already accepted that convention for a second member on the same evidence: open issue 20261024T122923Z-managedblockchain-query-gettransaction-wrong-behavior (`Transaction$transactionFee` must be the main-unit decimal, not wei/satoshi). A balance is the same kind of quantity as that fee and that event value, and the three are read together (the btc balance is literally the sum of those event values, `derive_holdings`), so a fee in ETH beside a balance in wei cannot be reconciled by any client.

Handler/code: src/ops/get_token_balance.rs:162-210 returns the members of the `TokenBalance` that `read_token_balance` produced verbatim; the projection is src/state.rs:506-524 (`balance: record.balance.to_string()`); the balances are replayed as raw integers in src/state.rs:1471-1511; the smallest-unit constants are src/state/chain.rs:132-184 (`ETH`/`BTC` scale factors at src/state.rs ~1152).

Test coverage: no test pins the balance unit or format. tests/all/derived_chain_history.rs:533-608 asserts only that a balance is a decimal-digit string and that a historical read is stable/monotone; tests/all/chain_query_slice.rs:1152-1260 cross-checks GetTokenBalance against ListTokenBalances for EQUALITY between the two operations (so both can be wrong together) and against the event sum; tests/all/batch_token_balance.rs:102 asserts a batch item equals the single read. Nothing asserts the denomination, so nothing fails today.

## Steps to Reproduce

1. POST /get-token-balance {"tokenIdentifier": {"network": "ETHEREUM_MAINNET", "tokenId": "eth"}, "ownerIdentifier": {"address": "0xd8da6bf26964af9d7eed9e03e53415d37aa96045"}}
2. Inspect `balance`: it is a whole-wei integer of ~22 digits (the fixture opens each wallet at 1,000 ETH = 10^21 wei and moves ETH/50, ETH/100, ETH/200 amounts), where the real service sends an ETH-denominated decimal such as "1000.03".
3. POST /get-token-balance {"tokenIdentifier": {"network": "BITCOIN_MAINNET", "tokenId": "btc"}, "ownerIdentifier": {"address": "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa"}}: `balance` is a whole satoshi count (opening 1,000 BTC = 10^11 satoshi) where the real service sends a BTC decimal — the same 10^8 scaling the already-recorded BITCOIN_VOUT value '0.00000546' demonstrates.
4. POST /get-token-balance {"tokenIdentifier": {"network": "ETHEREUM_MAINNET", "contractAddress": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"}, "ownerIdentifier": {"address": "0xd8da6bf26964af9d7eed9e03e53415d37aa96045"}}: `balance` is the raw 6-decimal USDC unit count (e.g. "1000000000000" for 1,000,000 USDC) while POST /get-asset-contract for the same contract reports `metadata.decimals = 6`, so the response contradicts the scaling factor the service publishes for that very contract.
5. POST /list-token-balances {"tokenFilter": {"network": "ETHEREUM_MAINNET"}, "ownerFilter": {"address": "0xd8da6bf26964af9d7eed9e03e53415d37aa96045"}} and POST /batch-get-token-balance with the same item: every `tokenBalances[*].balance` carries the same smallest-unit denomination, confirming the shared projection is the source.
