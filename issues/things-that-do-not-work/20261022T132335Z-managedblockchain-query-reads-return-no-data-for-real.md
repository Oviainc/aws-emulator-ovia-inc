Issue-Source: trace-sweep-divergence
Issue-Trace: managedblockchain-query-token-balance-single-and-list
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: managedblockchain-query-token-balance-single-and-list managedblockchain-query-list-transactions-then-get-transaction
Issue-Shape-Id: com.amazonaws.managedblockchainquery#TietonChainQueryService
Issue-Key: managedblockchain-query|GetTokenBalance|2xx->4xx|$.atBlockchainInstant
Issue-Key: managedblockchain-query|GetTokenBalance|2xx->4xx|$.balance
Issue-Key: managedblockchain-query|GetTokenBalance|2xx->4xx|$.lastUpdatedTime
Issue-Key: managedblockchain-query|GetTokenBalance|2xx->4xx|$.ownerIdentifier
Issue-Key: managedblockchain-query|GetTokenBalance|2xx->4xx|$.tokenIdentifier
Issue-Key: managedblockchain-query|GetTokenBalance|2xx->4xx|status
Issue-Key: managedblockchain-query|ListTransactions|2xx->2xx|$.transactions[*]
Issue-Commit: b4ac6d961cdb10c212ccdfac1172ee263c3e4aa3

# managedblockchain-query reads return no data for real mainnet identifiers: GetTokenBalance 404s and ListTransactions comes back empty

> _Note: this issue was identified against the code base at commit `b4ac6d961cdb10c212ccdfac1172ee263c3e4aa3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTokenBalance

## Description
The recorded fixtures read this service with real, publicly-known Ethereum mainnet identifiers (the USDC contract 0xa0b8...eb48 and the wallet 0xd8dA...6045). AWS answers those reads out of an index of the live chains, so they return data; the emulator serves them out of a synthetic seeded ledger that holds neither identifier, so the same reads return not-found or an empty page. Two read surfaces diverge, each needing its own fix in the seeded state (identifier coverage and a ledger time span that covers the queried instants).

--- Cause 1 of 2: GetTokenBalance returns 404 ResourceNotFoundException where AWS returns 200 with a balance ---
AWS: Smithy GetTokenBalance (POST /get-token-balance, http code 200) takes tokenIdentifier (required) + ownerIdentifier (required) + optional atBlockchainInstant, and returns balance (required String), atBlockchainInstant (required) and optional ownerIdentifier / tokenIdentifier / lastUpdatedTime. For a held token of an active mainnet wallet AWS answers HTTP 200 with a base-10 integer balance, echoes the requested ownerIdentifier / tokenIdentifier back, and resolves the instant it priced the balance at. ResourceNotFoundException (404) is modeled, but for a token/owner the service holds nothing for — not for a real, funded holding.
Emulator: HTTP 404 and an empty body for the same request; every output member (balance, atBlockchainInstant, lastUpdatedTime, ownerIdentifier, tokenIdentifier) is absent because the seeded ledger has no holding under that key.
Why: a 404 is the signal "this owner does not hold this token". A balance client, reconciliation job or portfolio view therefore concludes the wallet is empty/nonexistent rather than reading a number, and retry logic that treats 404 as terminal gives up. This is the single most common call of this API.
Fix: the service's seeded state (src/state.rs: seed_networks / seed_ethereum_ledger / seed_bitcoin_ledger holdings and the LEDGER_EPOCH time span) must cover the real mainnet identifiers the corpus reads and a history that spans the queried instants; src/ops/get_token_balance.rs must only raise ResourceNotFoundException when the ledger genuinely holds nothing.
Traces: managedblockchain-query-token-balance-single-and-list#task#1.

--- Cause 2 of 2: ListTransactions returns HTTP 200 with an empty transactions list for a real mainnet address, starving the GetTransaction step that chains off it ---
AWS: Smithy ListTransactions (POST /list-transactions) takes required address (ChainAddress) + network, optional from/toBlockchainInstant, sort, confirmationStatusFilter, maxResults @range(1,250), and returns transactions (required, @length 0..250) plus an optional nextToken; it is @paginated on transactions. For an active mainnet address AWS returns non-empty pages of TransactionOutputItem summaries carrying transactionHash, network, transactionTimestamp and confirmationStatus.
Emulator: HTTP 200 (status agrees) with no element at transactions[0] — an empty list — so the summary the fixture's next step feeds to GetTransaction never exists.
Why: an empty 200 is indistinguishable from "this address has no history", and this operation is the discovery entry point of the whole API: with nothing returned a client cannot obtain a transaction hash, cannot observe sort order or window filtering, and cannot exercise its paging path, so every read chained off a summary is unreachable.
Fix: same seeded-state gap in src/state.rs (ledger identifiers and transaction time span), as read by src/ops/list_transactions.rs; the seeded history must contain transactions for the addresses the corpus queries so a hash is discoverable.
Traces: managedblockchain-query-list-transactions-then-get-transaction#task#1.

Related open issue: "ManagedBlockchain Query fixture ledger holds no data for real mainnet identifiers: token-balance and transaction reads return empty/not-found" describes the same root cause from the behavioral corpus, but it carries no wire replay key, so neither wire step above is covered by it — these fixtures must be replayed to confirm the fix on the wire.

## Evidence
Replay of trace set simparity-wire-aws-v1, both clusters service=managedblockchain-query with stub_path empty and owned_by empty:
Cause 1 — managedblockchain-query-token-balance-single-and-list#task#1 GetTokenBalance: status expected 200, got 404; $.balance missing (expected '119094756568'), $.atBlockchainInstant missing (expected {'time': 1788657866.0}), $.lastUpdatedTime missing (expected {'time': 1747274759.0}), $.ownerIdentifier missing (expected {'address': '0xd8da6bf26964af9d7eed9e03e53415d37aa96045'}), $.tokenIdentifier missing (expected {'contractAddress': '0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48', 'network': 'ETHEREUM_MAINNET'}). The recorded balance and timestamps are examples; the requirement is HTTP 200 with the required balance and atBlockchainInstant members present and the requested identifiers echoed.
Cause 2 — managedblockchain-query-list-transactions-then-get-transaction#task#1 ListTransactions: 200/200 but $.transactions[0] missing (expected a summary {'confirmationStatus': 'FINAL', 'network': 'ETHEREUM_MAINNET', 'transactionHash': '0x7671e8...e219', 'transactionTimestamp': 1619073240.0}, got None) — i.e. an empty transactions list. The particular hash/timestamp are examples; the requirement is a non-empty page of summaries for an address with history.
Model citations: GetTokenBalance output {balance required, atBlockchainInstant required, ownerIdentifier, tokenIdentifier, lastUpdatedTime}, http POST /get-token-balance code 200, ResourceNotFoundException httpError 404; ListTransactions output {transactions required @length 0..250, nextToken}, @paginated(items: transactions, pageSize: maxResults).

## Steps to Reproduce
Point a client at the emulator's managedblockchain-query endpoint (region us-east-1):
1. Cause 1: get_token_balance(tokenIdentifier={'network':'ETHEREUM_MAINNET','contractAddress':'0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48'}, ownerIdentifier={'address':'0xd8da6bf26964af9d7eed9e03e53415d37aa96045'}) -> observe HTTP 404 ResourceNotFoundException instead of HTTP 200 carrying balance + atBlockchainInstant.
2. Cause 2: list_transactions(address='0xd8da6bf26964af9d7eed9e03e53415d37aa96045', network='ETHEREUM_MAINNET') -> observe HTTP 200 with transactions == [] and no nextToken, so no transactionHash is available for the following get_transaction step.
3. Or simply replay the corpus (replay_wire_corpus) and inspect fixtures managedblockchain-query-token-balance-single-and-list (task#1) and managedblockchain-query-list-transactions-then-get-transaction (task#1).
