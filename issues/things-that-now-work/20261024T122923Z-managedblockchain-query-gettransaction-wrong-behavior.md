Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f0788135e0bb0704427b37eee4190850f5fa33c6

# Transaction$transactionFee is reported in the chain's smallest unit (wei/satoshi) instead of the main-unit decimal

_Filed: 2026-10-24T12:29:23Z_
_Operation: GetTransaction | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f0788135e0bb0704427b37eee4190850f5fa33c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetTransaction` projects the stored transaction's fee onto `Transaction$transactionFee` as the raw integer the ledger holds: `transaction_fee: self.transaction_fee.map(|amount| amount.to_string())` (src/state.rs:233). The stored value is in the chain's SMALLEST unit — on the Ethereum networks `gas_used * EFFECTIVE_GAS_PRICE` in wei (src/state.rs:1384, with `EFFECTIVE_GAS_PRICE = 20_000_000_000` wei = 20 gwei), and on the Bitcoin networks the satoshi difference between the `BITCOIN_VIN` and `BITCOIN_VOUT` event totals (src/state.rs:1381-1383).

Expected: the fee is denominated in the chain's MAIN unit as a decimal string — a 21,000-gas transaction at 20 gwei reports `"0.00042"` ETH, and a 20,000-satoshi Bitcoin fee reports `"0.0002"` BTC. `gasUsed` (gas units) and `effectiveGasPrice` (wei) stay raw integers; `transactionFee` is the one member of the trio that is a currency amount, and this service denominates native-currency amounts in the main unit.

Actual: `transactionFee` comes back as `"420000000000000"` (wei) on Ethereum and as a whole satoshi count on Bitcoin — off by 10^18 / 10^8 in magnitude and in format from what a client gets from AWS. A client that renders or sums fees, or that cross-checks `transactionFee` against `gasUsed * effectiveGasPrice / 10^18`, reads a number 18 orders of magnitude too large.

## Evidence

Spec: smithy_query(query="shape", shape="Transaction") -> `transactionFee` (String) "The transaction fee", alongside `gasUsed` (String) "The amount of gas used for the transaction" and `effectiveGasPrice` (String) "The effective gas price" — the model fixes no unit, so the denomination is recorded behaviour.

Recorded AWS behaviour already captured in this repo: issue 20261022T213114Z-managedblockchain-query-query-responses-diverge-wrong-chain-data quotes a real `BITCOIN_VOUT` event as `{'eventType': 'BITCOIN_VOUT', ..., 'tokenId': 'btc', 'value': '0.00000546', ...}` — 546 satoshi rendered as a BTC-denominated decimal. `transactionFee` is the same kind of quantity (an amount of the chain's native currency), so the service's convention makes it a main-unit decimal, not a smallest-unit integer. The fee is additionally derivable from the two raw members AWS does send in base units: gasUsed x effectiveGasPrice / 10^18 is the ETH fee, which only reconciles if transactionFee is ETH-denominated.

Handler/code: src/ops/get_transaction.rs:193-195 returns `transaction.transaction()` verbatim; the projection is src/state.rs:212-238 (`transaction_fee: self.transaction_fee.map(|amount| amount.to_string())`); the stored value is computed at src/state.rs:1378-1385 and written at src/state.rs:1423.

Test coverage: tests/all/derived_chain_history.rs:462-468 is the only assertion touching these members and it only checks that `gasUsed`, `effectiveGasPrice` and `transactionFee` are STRINGS for an Ethereum transaction — no test pins the unit or the format, so nothing fails today.

## Steps to Reproduce

1. POST /list-transactions {"network": "ETHEREUM_MAINNET", "address": "0xd8da6bf26964af9d7eed9e03e53415d37aa96045", "maxResults": 1} and take the returned transactionHash.
2. POST /get-transaction {"network": "ETHEREUM_MAINNET", "transactionHash": <that hash>}.
3. Inspect `transaction.transactionFee`: it is a whole-wei integer string (e.g. "420000000000000" for 21000 gas at 20 gwei) where the real service sends the ETH-denominated decimal ("0.00042"); `gasUsed` x `effectiveGasPrice` equals it exactly, confirming the smallest-unit denomination.
4. POST /list-transactions for a Bitcoin fixture address (e.g. {"network": "BITCOIN_MAINNET", "address": "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa", "maxResults": 1}), then POST /get-transaction for that hash: `transactionFee` is a whole satoshi count where the real service sends a BTC decimal, matching the already-recorded '0.00000546' convention for Bitcoin event values.
