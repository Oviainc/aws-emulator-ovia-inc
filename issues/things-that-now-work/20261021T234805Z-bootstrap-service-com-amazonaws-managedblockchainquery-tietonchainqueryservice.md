Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 9e4273988d175f89e8a017aa0737ee7107d62a89

# Bootstrap the ManagedBlockchain Query emulator

_Filed: 2026-10-21T23:48:05Z_
_Operation: service-com-amazonaws-managedblockchainquery-tietonchainqueryservice | Class: stub_

> _Note: this issue was identified against the code base at commit `9e4273988d175f89e8a017aa0737ee7107d62a89`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 9 operations in `src/ops/` are unmodified scaffolds that answer HTTP 501 `NotImplemented`, and `src/state.rs` holds no fields (`// TODO: add service-specific state fields`). ManagedBlockchain Query is a READ-ONLY query service over blockchain data (every operation carries `@readonly`; there is no Create*), so the bootstrap slice is: a seeded in-memory ledger plus the query loop that reads it. Every one of the operations below MUST compute its response by looking up that persisted ledger — no canned bodies, no echoing the request back, no randomly generated transaction hashes that are never stored. A `GetTransaction` for a hash that is in the ledger must return exactly the stored transaction, and a hash that is not in the ledger must return `ResourceNotFoundException` (HTTP 404), not an invented success.

First slice (restJson1, 5 operations, in workflow order):

1. **ListTransactions** — `POST /list-transactions`. Input: `address` (required, `^[-A-Za-z0-9]{13,74}$`), `network` (required enum BITCOIN_MAINNET|BITCOIN_TESTNET|ETHEREUM_MAINNET|ETHEREUM_SEPOLIA_TESTNET), optional `fromBlockchainInstant`/`toBlockchainInstant` (BlockchainInstant{time}), `sort` (ListTransactionsSort), `confirmationStatusFilter`, `nextToken`, `maxResults`. Output: `transactions` (list<TransactionOutputItem>, required) + `nextToken`. Must return only the ledger transactions whose `from` or `to` equals `address` on that `network`, honour the instant range, apply `maxResults` as a real page size and return an opaque `nextToken` that resumes exactly where the page ended (and is absent on the last page).
2. **GetTransaction** — `POST /get-transaction`. Input: `network` (required) plus `transactionHash` or `transactionId` (both `^(0x[A-Fa-f0-9]{64}|[A-Fa-f0-9]{64})$`). Output: `transaction` (Transaction, required) — must be the stored record, with `network`, `transactionHash`, `transactionTimestamp`, `transactionIndex`, `numberOfTransactions`, `to` populated (all `@required` on Transaction) and `from`/`blockHash`/`blockNumber`/`gasUsed`/`confirmationStatus`/`executionStatus` as stored. Unknown hash → `ResourceNotFoundException` 404.
3. **ListTransactionEvents** — `POST /list-transaction-events`. Input: `network` (required) + `transactionHash`/`transactionId`, `nextToken`, `maxResults`. Output: `events` (list<TransactionEvent>, required) + `nextToken`. Must return the events stored against that one transaction (each with required `network`, `transactionHash`, `eventType`, plus `from`/`to`/`value`/`contractAddress`/`tokenId` as stored), paginated like above.
4. **GetTokenBalance** — `POST /get-token-balance`. Input: `tokenIdentifier` (TokenIdentifier{network required, contractAddress, tokenId}, required), `ownerIdentifier` (OwnerIdentifier{address} required), optional `atBlockchainInstant`. Output: `balance` (required), `atBlockchainInstant` (required — echo the requested instant or the current time when omitted), `ownerIdentifier`, `tokenIdentifier`, `lastUpdatedTime`. The balance must come from the stored holdings for (network, contractAddress, tokenId, owner), not a constant.
5. **ListTokenBalances** — `POST /list-token-balances`. Input: `tokenFilter` (TokenFilter{network required, contractAddress, tokenId}, required), optional `ownerFilter` (OwnerFilter{address}), `nextToken`, `maxResults`. Output: `tokenBalances` (list<TokenBalance>, required) + `nextToken`. Must enumerate the stored holdings matching the filter, consistent with what GetTokenBalance returns for the same key.

Validation for the slice: missing required members (`network`, `address`, `tokenFilter`, `tokenIdentifier`, `ownerIdentifier`, `contractFilter`) and values failing the modeled pattern/enum constraints must produce `ValidationException` (HTTP 400); an unknown transaction or token/owner must produce `ResourceNotFoundException` (HTTP 404). A malformed `nextToken` must be rejected with `ValidationException`, not silently treated as page one.

Because there is no write API, the ledger must be populated by a deterministic fixture/seed loaded when the service state is first created (and ideally a test-only seeding hook), so that the five operations above are mutually consistent: a transaction listed by ListTransactions is fetchable by GetTransaction, its events are listable by ListTransactionEvents, and the token transfers in those events agree with the balances reported by GetTokenBalance/ListTokenBalances.

## State Model

In `ComAmazonawsManagedblockchainqueryTietonchainqueryserviceState`:

- `networks: HashMap<QueryNetwork, NetworkLedger>` — one ledger per modeled network enum value.
- `NetworkLedger { transactions: BTreeMap<TxKey, StoredTransaction>, by_address: HashMap<String, Vec<TxKey>>, holdings: HashMap<HoldingKey, StoredHolding>, contracts: HashMap<String, StoredContract> }` where `TxKey = (transactionTimestamp, transactionIndex, transaction_hash)` so iteration is naturally sortable for ListTransactions `sort`.
- `StoredTransaction { network, transaction_hash, transaction_id: Option<String>, block_hash, block_number, transaction_timestamp, transaction_index, number_of_transactions, to, from, contract_address, gas_used, cumulative_gas_used, effective_gas_price, transaction_fee, signature_v/r/s, confirmation_status, execution_status, events: Vec<StoredEvent> }`.
- `StoredEvent { event_type, from, to, value, contract_address, token_id, transaction_id, vout_index, vout_spent, spent_vout_*, blockchain_instant, confirmation_status }` — stored in emission order, which is the order ListTransactionEvents must return.
- `HoldingKey = (contract_address: Option<String>, token_id: Option<String>, owner_address: String)`; `StoredHolding { balance: String, last_updated_time: Timestamp }` (optionally a small history of (timestamp, balance) so `atBlockchainInstant` can resolve a point-in-time balance rather than ignoring the parameter).
- `StoredContract { contract_identifier: {network, contract_address}, token_standard, deployer_address, metadata }` — present now so the later GetAssetContract/ListAssetContracts slice reads the same store.
- `page_tokens: HashMap<String, PageCursor>` (or an opaque encoded cursor) shared by every paginated op, so `nextToken` is a real resumable cursor rather than a constant.

Out of scope for this first slice and expected to keep returning 501 (filed separately later): BatchGetTokenBalance, GetAssetContract, ListAssetContracts, ListFilteredTransactionEvents.

## Evidence

`OPERATIONS.md` for this crate lists all 9 operations with restJson1 routes, required input/output members and error sets (e.g. GetTransaction: `network` required + `transactionHash`/`transactionId` patterned, output `transaction` required, `ResourceNotFoundException (HTTP 404)`; ListTransactions/ListTokenBalances/ListTransactionEvents are `@paginated(inputToken=nextToken, outputToken=nextToken, items=...)`). Smithy shapes confirm the state fields: `Transaction` (@required network, transactionHash, transactionTimestamp, transactionIndex, numberOfTransactions, to), `TransactionEvent` (@required network, transactionHash, eventType), `TokenBalance` (@required balance, atBlockchainInstant), `TokenIdentifier`/`TokenFilter` (@required network + optional contractAddress/tokenId), `OwnerIdentifier` (@required address), `ContractIdentifier` (@required network, contractAddress). All 9 files under `src/ops/` still carry `//! SCAFFOLDED, NOT IMPLEMENTED:` at line 4 and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501, and `src/state.rs` has only `// TODO: add service-specific state fields`, so no state exists to query.

## Steps to Reproduce

1. Start the emulator with the managedblockchain-query service mounted (state seeded with the deterministic fixture ledger described above).
2. `aws managedblockchain-query list-transactions --address <seeded address> --network ETHEREUM_MAINNET --max-results 1` → expect HTTP 200 with exactly one TransactionOutputItem for that address plus a nextToken; today HTTP 501 NotImplemented.
3. Re-issue list-transactions with the returned --next-token → expect the NEXT transaction (no repeat of page one), and no nextToken on the final page.
4. `aws managedblockchain-query get-transaction --network ETHEREUM_MAINNET --transaction-hash <hash from step 2>` → expect the same stored transaction (matching timestamp, index, to/from, blockNumber) rather than a canned body.
5. `aws managedblockchain-query get-transaction --network ETHEREUM_MAINNET --transaction-hash 0x<64 hex not in the ledger>` → expect ResourceNotFoundException with HTTP 404.
6. `aws managedblockchain-query list-transaction-events --network ETHEREUM_MAINNET --transaction-hash <hash from step 2>` → expect the events stored for that transaction, each with network/transactionHash/eventType set and the transfer value/contractAddress/tokenId of the seeded transfer.
7. `aws managedblockchain-query get-token-balance --network ... --token-identifier network=ETHEREUM_MAINNET,contractAddress=<seeded contract>,tokenId=<seeded token> --owner-identifier address=<seeded owner>` → expect the stored balance for that holder plus a populated atBlockchainInstant and lastUpdatedTime.
8. `aws managedblockchain-query list-token-balances --token-filter network=ETHEREUM_MAINNET,contractAddress=<seeded contract> --max-results 1` → expect the holdings for that contract, paginated, with the same balance for the owner as step 7 returned.
9. `aws managedblockchain-query get-token-balance ... --owner-identifier address=<owner with no holding>` → expect ResourceNotFoundException 404; call list-token-balances with no --token-filter → expect ValidationException 400.
