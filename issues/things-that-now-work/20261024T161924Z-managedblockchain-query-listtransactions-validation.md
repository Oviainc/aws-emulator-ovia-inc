Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ec42ee63c3cbc91c5bb52d083d61328ac6a6aee0

# address is never checked against the requested network's address encoding, so a cross-chain address answers an empty 200 page instead of a per-field ValidationException

_Filed: 2026-10-24T16:19:24Z_
_Operation: ListTransactions | Class: validation_

> _Note: this issue was identified against the code base at commit `ec42ee63c3cbc91c5bb52d083d61328ac6a6aee0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListTransactions` validates `address` only against the model's `ChainAddress` pattern `^[-A-Za-z0-9]{13,74}$`, which is one expression for every chain this API indexes. An address that cannot belong to the requested `network` at all — a base58 Bitcoin address sent with `network: ETHEREUM_MAINNET`, or an EVM `0x…` address sent with `network: BITCOIN_MAINNET`, or a 13-character alphanumeric label that is neither — passes validation, reaches the ledger lookup, matches nothing and is answered HTTP 200 with `transactions: []` and no `nextToken`.

Expected: this service validates a chain address against the chain the request names and refuses a mismatch with its modelled `ValidationException` (400) carrying a `fieldList` entry for the offending member — the behaviour already recorded for the sibling operation, where AWS answers a non-Bitcoin address with the entry `{name: "addressIdentifierFilter", message: "Please provide a valid bitcoin address."}`. A caller that mistypes an address, or pairs an address with the wrong network enum (the single most common mistake in a multi-chain API whose address type is chain-agnostic), must learn which field is wrong.

Actual: an empty 200 page, which a client cannot distinguish from "this address exists and has no transactions in the index" — so the request looks answered and the client concludes the wallet is unused. src/ops/list_transactions.rs:176-177 calls `crate::state::validation::list_transactions_input`, whose only check on this member is `chain_address(&mut violations, "address", &input.address)` (the `ChainAddress` @pattern); nothing consults `input.network`. The crate already owns the chain-address validator this check needs — `crate::state::address::is_bitcoin_address` (src/state/address.rs:69) — but it is wired into `ListFilteredTransactionEvents` only (src/ops/list_filtered_transaction_events.rs:257-262).

The same missing check makes the other address-taking reads answer the wrong status for the same class of request, and the fix belongs in the shared validator rather than in one handler: `GetTokenBalance`/`BatchGetTokenBalance` (`ownerIdentifier.address`, `tokenIdentifier.contractAddress`) and `GetAssetContract`/`ListAssetContracts`/`ListTokenBalances` (`contractAddress`, `deployerAddress`, `ownerFilter.address`) answer a cross-chain address with 404 `ResourceNotFoundException` or an empty page instead of 400. `ListTransactions` is the operation where the divergence is most directly observable because it is the entry point of the service's main workflow and its answer is a bare empty list.

## Evidence

Spec: smithy_query(query="operation", operation="ListTransactions") -> `address` (ChainAddress, @required, @pattern ^[-A-Za-z0-9]{13,74}$) and `network` (QueryNetwork, @required, BITCOIN_MAINNET | BITCOIN_TESTNET | ETHEREUM_MAINNET | ETHEREUM_SEPOLIA_TESTNET); `ValidationException` (400) models `message` (required), `reason` (required) and `fieldList` of `ValidationExceptionField{name, message}` — the machine-readable channel for exactly this kind of per-member refusal. The model cannot express "an address of THIS network" because one `ChainAddress` type serves all four networks (src/state/address.rs:5-29 states this), so the check is necessarily service-side.

Recorded AWS behaviour already captured in this repo: fixed issue 20261022T213114Z-managedblockchain-query-query-responses-diverge-wrong-chain-data, cause 3, quotes a real 400 from `ListFilteredTransactionEvents` whose `fieldList` contains `{"name": "addressIdentifierFilter", "message": "Please provide a valid bitcoin address."}` alongside `{"name": "network", ...}` — i.e. this service runs a per-chain address validator in its input-validation layer and reports it as a `fieldList` entry. That validator is not applied by this emulator to `ListTransactions$address`, whose request carries the very `network` needed to pick the chain.

Handler/validator: src/ops/list_transactions.rs:176-177 and src/state/validation.rs:716-729 (`list_transactions_input` -> `chain_address` pattern check, plus `query_network`, `sort`, `confirmationStatusFilter`, `nextToken`, `maxResults` — no address/network cross-check). The existing per-chain validator is src/state/address.rs:69 `is_bitcoin_address`, used only at src/ops/list_filtered_transaction_events.rs:257-262.

Test coverage: nothing asserts a refusal here. tests/all/error_response_shape.rs and tests/all/error_contract.rs exercise the `ChainAddress` @pattern bounds and the `maxResults` range only; tests/all/chain_query_slice.rs exercises addresses the fixture holds. An address the fixture does not hold is only ever asserted to produce an empty page, which is the correct answer for a WELL-FORMED address of that chain and must stay — the gap is the cross-chain/unparseable case.

## Steps to Reproduce

1. POST /list-transactions {"network": "ETHEREUM_MAINNET", "address": "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa"} — a base58 Bitcoin address on an Ethereum network. Emulator: 200 with {"transactions": []}. Expected: 400 ValidationException with reason `fieldValidationFailed` and a `fieldList` entry naming `address`.
2. POST /list-transactions {"network": "BITCOIN_MAINNET", "address": "0xd8da6bf26964af9d7eed9e03e53415d37aa96045"} — an EVM address on a Bitcoin network (`0x` is outside the base58 and bech32 alphabets, as src/state/address.rs documents). Emulator: 200 empty page; expected 400 with the `address` field reported.
3. POST /list-transactions {"network": "BITCOIN_MAINNET", "address": "notanaddress12"} — 14 alphanumerics, inside the ChainAddress pattern, not an address of any indexed chain: still 200 with an empty page.
4. Contrast POST /list-filtered-transaction-events {"network": "BITCOIN_MAINNET", "addressIdentifierFilter": {"transactionEventToAddress": ["0xd8da6bf26964af9d7eed9e03e53415d37aa96045"]}}, which DOES answer 400 with `{name: "addressIdentifierFilter", message: "Please provide a valid bitcoin address."}` — the same validator is simply not applied to the other address-taking operations.
5. Same shape of divergence on the balance/contract reads: POST /get-token-balance {"tokenIdentifier": {"network": "ETHEREUM_MAINNET", "tokenId": "eth"}, "ownerIdentifier": {"address": "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa"}} answers 404 ResourceNotFoundException, and POST /get-asset-contract with a Bitcoin address on ETHEREUM_MAINNET answers 404, where the request is a validation failure rather than a missing resource.
