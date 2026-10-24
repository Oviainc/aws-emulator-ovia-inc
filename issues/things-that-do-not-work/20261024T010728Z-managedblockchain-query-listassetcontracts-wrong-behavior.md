Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# Well-known mainnet contracts are stored under synthesized deployer addresses, so the required deployerAddress filter never matches the real deployer

_Filed: 2026-10-24T01:07:28Z_
_Operation: ListAssetContracts | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ContractFilter` makes all three members `@required`, so `ListAssetContracts` can only be called with a concrete `deployerAddress` — the listing is "the contracts of this network, of this token standard, deployed by this address". The emulator matches that filter against `StoredContract::deployer_address`, and for the well-known Ethereum-mainnet ERC-20 contracts that field is not the contract's deployer at all: `well_known_mainnet_erc20` sets `deployer_address: ethereum_address(network, &format!("deployer-{symbol}"))` (src/state.rs:1276-1291), a SHA-256-derived synthetic address, with a comment that the deployment transaction is not indexed.

Expected: a client that knows a mainnet token contract's real deployer (public chain data — e.g. USD Coin at 0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48 was deployed by 0x95ba4cf87d6723ad9c0db21737d862be80e93911) calls `ListAssetContracts` with `contractFilter = {network: ETHEREUM_MAINNET, tokenStandard: ERC20, deployerAddress: 0x95ba4cf8...}` and gets that contract back, and `GetAssetContract` on the contract reports the same deployer.

Actual: `ListAssetContracts` answers 200 with `contracts: []` for every real deployer address, because the only deployer values in the ledger are digest-derived strings no caller can know or guess. The required filter member therefore has exactly one usable value per contract, and it has to be discovered by first calling `GetAssetContract` — which itself reports a `deployerAddress` the chain does not record. The operation is effectively unusable with real-world identifiers even though the emulator does hold the real contract addresses and metadata (name/symbol/decimals from `ETHEREUM_MAINNET_ERC20`, src/state.rs:1231-1268), making this the one remaining member of the well-known-contract catalog that is invented rather than looked up.

## Evidence

Spec: smithy_query(query="shape", shape="ContractFilter") -> `{network: required, tokenStandard: required, deployerAddress: required (ChainAddress, @pattern ^[-A-Za-z0-9]{13,74}$) "The network address of the deployer"}`; `GetAssetContractOutput$deployerAddress` is `@required` and documented "The address of the deployer of contract". Because the filter member is required, there is no way to list a network's contracts without naming a deployer.

Recorded AWS behaviour, quoted in this repo's own issue 20261022T213114Z (Cause 5 evidence): "the queried contract reports deployerAddress 0x95ba4cf87d6723ad9c0db21737d862be80e93911" for contractAddress 0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48 — the real USDC deployer — against the emulator's 0x6117ad6d33522adfc168aa29a7c4fca3597710a8. The remedy was applied to the contract catalog's addresses and metadata but NOT to the deployer, which src/state.rs:1272-1275 still documents as "derived the way every other fixture address is".

Handler/code: src/ops/list_asset_contracts.rs:195-203 filters `contract.deployer_address() == normalize_address(&filter.deployer_address)`; src/state.rs:1283 is where the synthetic deployer is written; src/state.rs:1231-1268 shows the catalog holds real contract addresses, names, symbols and decimals, so the deployer is the only invented column.

Test coverage: tests/all/asset_contracts.rs:166-174 asserts only that the reply's deployerAddress equals the STORED one and satisfies the ChainAddress pattern; tests/all/error_response_shape.rs:1110-1209 reads the deployer out of GetAssetContract and feeds it back into ListAssetContracts. Both tests are self-referential against the fixture, so neither pins the deployer to the chain's real value and neither fails today.

## Steps to Reproduce

1. POST /list-asset-contracts {"contractFilter": {"network": "ETHEREUM_MAINNET", "tokenStandard": "ERC20", "deployerAddress": "0x95ba4cf87d6723ad9c0db21737d862be80e93911"}} — the real deployer of USD Coin.
2. Observe HTTP 200 with `contracts: []`; the real service returns the USDC contract (0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48).
3. POST /get-asset-contract {"contractIdentifier": {"network": "ETHEREUM_MAINNET", "contractAddress": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"}} — metadata (name "USD Coin", symbol "USDC", decimals 6) is correct, but `deployerAddress` is a digest-derived address unrelated to the chain's deployer.
4. Feed that reported deployerAddress back into /list-asset-contracts: only then does the contract appear, confirming the listing is keyed on an address no client can obtain from the chain.
