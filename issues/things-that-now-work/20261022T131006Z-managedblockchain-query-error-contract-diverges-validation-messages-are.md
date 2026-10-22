Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: managedblockchain-query.get-asset-contract-invalid-address-pattern-rejected managedblockchain-query.list-token-balances-invalid-contract-address-rejected managedblockchain-query.batch-get-token-balance-size-limit-rejected managedblockchain-query.get-transaction-mutually-exclusive-params-rejected managedblockchain-query.get-transaction-not-found-rejected managedblockchain-query.list-filtered-transaction-events-non-bitcoin-network-rejected
Issue-Shape-Id: com.amazonaws.managedblockchainquery#TietonChainQueryService
Issue-Key: managedblockchain-query.get-asset-contract-invalid-address-pattern-rejected|run#6|AssertionError|assert err["Message"] == expected, f"expected message {expected!r}, got {err['Message']!r}"
Issue-Key: managedblockchain-query.list-token-balances-invalid-contract-address-rejected|run#6|AssertionError|assert err["Message"] == expected, f"expected message {expected!r}, got {err['Message']!r}"
Issue-Key: managedblockchain-query.batch-get-token-balance-size-limit-rejected|run#8|AssertionError|assert err["Message"] == expected, f"expected message {expected!r}, got {err['Message']!r}"
Issue-Key: managedblockchain-query.get-transaction-mutually-exclusive-params-rejected|run#4|AssertionError|assert both["Error"]["Message"] == "One or more parameter values were invalid.", (
Issue-Key: managedblockchain-query.get-transaction-not-found-rejected|run#5|AssertionError|assert err["Message"] == "The requested resource was not found.", (
Issue-Key: managedblockchain-query.list-filtered-transaction-events-non-bitcoin-network-rejected|run#3|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Commit: 6f82ed95aacdd57d8c8c0220ec6b50c75b2537e1

# ManagedBlockchain Query error contract diverges: validation messages are not AWS-shaped, ValidationException omits fieldList, and the Bitcoin-only network restriction is not enforced

> _Note: this issue was identified against the code base at commit `6f82ed95aacdd57d8c8c0220ec6b50c75b2537e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTransaction

## Description
Five independent defects in how this service builds its 4xx responses. The HTTP status and error code are correct in four of the five (so these are not status/code regressions) but the rendered detail is not what AWS produces, and in one case the rejection does not happen at all. Each cause needs its own fix.

--- Cause 1 of 5: pattern-constraint validation messages quote the regex WITH its anchors ---
AWS: `ChainAddress` is `@pattern("^[-A-Za-z0-9]{13,74}$")` (Smithy: ChainAddress, referenced by `GetAssetContractInput$contractIdentifier.contractAddress` and `ListTokenBalancesInput$tokenFilter.contractAddress`). A value failing it is rejected with HTTP 400 `ValidationException`, and the standard AWS validation-framework rendering of the message names the member and the constraint with the pattern UNANCHORED: `1 validation error detected: Value '<value>' at '<member path>' failed to satisfy constraint: Member must satisfy regular expression pattern: [-A-Za-z0-9]{13,74}`. The rule is known (the message is built from the request member and the modeled constraint), so the rendering is the requirement; the surrounding literal text is AWS's standard validation framing.
Emulator: HTTP 400 `ValidationException` with the pattern echoed verbatim from the model including the anchors: `... Member must satisfy regular expression pattern: ^[-A-Za-z0-9]{13,74}$`.
Why: clients and test suites that parse the constraint out of the message (and anything that re-applies the quoted regex) see a different pattern than AWS reports, so the emulator cannot be used to validate client-side error handling of pattern failures.
Fix: the shared validation-message renderer in this service's validation layer (the component that formats `@pattern` failures into `ValidationException$message`) must strip the leading `^`/trailing `$` from the modeled pattern before rendering.
Traces: managedblockchain-query.get-asset-contract-invalid-address-pattern-rejected, managedblockchain-query.list-token-balances-invalid-contract-address-rejected.

--- Cause 2 of 5: BatchGetTokenBalance list-length validation message is not in AWS's validation-framework form ---
AWS: Smithy `BatchGetTokenBalance.getTokenBalanceInputs` targets `GetTokenBalanceInputList` with `@length(min: 1, max: 10)`. Eleven well-formed items is rejected with HTTP 400 `ValidationException` whose message follows the same request-derived rule as Cause 1: `1 validation error detected: Value '[<the rendered input list>]' at 'getTokenBalanceInputs' failed to satisfy constraint: Member must have length less than or equal to 10`.
Emulator: rejects with the correct `ValidationException` code (the check's code assertion passes, so the cap IS enforced) but a message that does not match that rendering. The emulator's exact text was clipped out of the replay output; what is established is the divergence on `message` for the length constraint while code/status agree.
Why: same impact as Cause 1 — a client that surfaces or parses the constraint detail for an over-sized batch gets text AWS never emits.
Fix: the same validation-message renderer, for `@length` failures on a list member (render the member path and the `less than or equal to <max>` constraint clause in AWS's form).
Traces: managedblockchain-query.batch-get-token-balance-size-limit-rejected.

--- Cause 3 of 5: GetTransaction identifier-exclusivity rejection uses an ad-hoc message and carries no fieldList ---
AWS: Smithy `GetTransaction` models `transactionHash` and `transactionId` both OPTIONAL (exactly one is required; `transactionId` is documented as Bitcoin-networks-only), and `ValidationException` has members `message` (required), `reason` (required, `ValidationExceptionReason`) and `fieldList` (`ValidationExceptionFieldList` of `ValidationExceptionField{name, message}` — "The container for the fieldList of the exception"). AWS reports this class of failure as HTTP 400 `ValidationException` with the generic top-level message `One or more parameter values were invalid.` and puts the per-field detail in `fieldList`: supplying BOTH on an EVM network yields two entries — `{message: "The request must include either a transactionId or transactionHash but not both."}` and `{name: "transactionId", message: "transactionId can only be specified for Bitcoin networks."}` — and supplying NEITHER yields `[{message: "The request must include a transactionHash."}]`.
Emulator: HTTP 400 `ValidationException` with the whole explanation inlined in `message` (observed: `Only one of transactionHash or transactionId may be specified`) and no `fieldList` member at all.
Why: the modeled `fieldList` is the machine-readable half of this error — a client that inspects `fieldList` to tell "both supplied" from "neither supplied" from "transactionId on a non-Bitcoin network" gets nothing, and a client that switches on the top-level message sees prose instead of AWS's generic sentinel. The emulator also never raises the Bitcoin-only `transactionId` field error, so that rule is unexercised.
Fix: `src/ops/get_transaction.rs` request checks plus this service's `ValidationException` construction helper (`crate::state::field_validation_exception` and friends) — emit the generic message, set `reason`, and populate `fieldList` with one `ValidationExceptionField` per violated rule.
Traces: managedblockchain-query.get-transaction-mutually-exclusive-params-rejected.

--- Cause 4 of 5: GetTransaction not-found message echoes the request instead of AWS's generic not-found text ---
AWS: recorded behavior only (the model documents `ResourceNotFoundException` as "The resource was not found." with members `message`, `resourceId`, `resourceType`, and does not state the message text). The recording shows a well-formed but unmined `transactionHash` is rejected with HTTP 404 `ResourceNotFoundException` and the fixed, request-independent message `The requested resource was not found.` — consistent with Cause 3, where this service's generic top-level message likewise carries no request detail.
Emulator: HTTP 404 `ResourceNotFoundException` (code and status correct) with a message built from the request: `No transaction with transactionHash 0000...0000 exists on network ETHEREUM_MAINNET`.
Why: the emulator leaks request-derived detail into an error AWS keeps generic, which is a behavioural difference for a client that compares or logs these messages; lower severity than the causes above because status and code agree.
Fix: this service's `ResourceNotFoundException` construction helper (`crate::state::not_found` in src/state.rs, as called from src/ops/get_transaction.rs) should carry the service's generic not-found message and leave the identity in `resourceId`/`resourceType`.
Traces: managedblockchain-query.get-transaction-not-found-rejected. (This check's positive control also depends on ListTransactions returning data — see the separate fixture-ledger issue for this service.)

--- Cause 5 of 5: ListFilteredTransactionEvents does not reject a non-Bitcoin network; it answers NotImplemented ---
AWS: Smithy `ListFilteredTransactionEventsInput$network` doc: "The blockchain network where the transaction occurred. Valid Values: BITCOIN_MAINNET | BITCOIN_TESTNET". Calling it with `network=ETHEREUM_MAINNET` is rejected with HTTP 400 `ValidationException` and the generic message `One or more parameter values were invalid.` (same generic-message rule as Cause 3).
Emulator: returns error code `NotImplemented` (the operation is a scaffold stub, so the network restriction is never evaluated).
Why: an over-permissive/mis-typed rejection: a client exercising the Bitcoin-only rule gets a non-modeled `NotImplemented` error instead of the modeled 400 ValidationException, so it cannot distinguish "wrong network" from "service failure" and will not retry/report correctly. Noted separately from the already-filed scaffold-stub issue for `ListFilteredTransactionEvents` (services/service-com-amazonaws-managedblockchainquery-tietonchainqueryservice/src/ops/list_filtered_transaction_events.rs, filed from the checkout for the checks it blocks): that issue covers implementing the operation, this cause records that the Bitcoin-only `network` validation must also exist and must precede any happy-path work.
Fix: `src/ops/list_filtered_transaction_events.rs` — validate `network` against the two Bitcoin values and raise the modeled `ValidationException` before any lookup.
Traces: managedblockchain-query.list-filtered-transaction-events-non-bitcoin-network-rejected.

## Evidence
Cause 1 — expected message: "1 validation error detected: Value '!!!not-an-address!!!' at 'contractIdentifier.contractAddress' failed to satisfy constraint: Member must satisfy regular expression pattern: [-A-Za-z0-9]{13,74}"; actual: "... regular expression pattern: ^[-A-Za-z0-9]{13,74}$". Same divergence on ListTokenBalances for 'tokenFilter.contractAddress' with value 'not-an-address!'. HTTP 400 / ValidationException agree.
Cause 2 — expected message: "1 validation error detected: Value '[<11 rendered BatchGetTokenBalanceInputItem values>]' at 'getTokenBalanceInputs' failed to satisfy constraint: Member must have length less than or equal to 10"; actual: a different message (text clipped from the replay output). Code assertion (ValidationException) passed, so only the message diverges.
Cause 3 — expected: ValidationException with Message == "One or more parameter values were invalid." and fieldList == [{"message": "The request must include either a transactionId or transactionHash but not both."}, {"name": "transactionId", "message": "transactionId can only be specified for Bitcoin networks."}] for both-supplied, and [{"message": "The request must include a transactionHash."}] for neither-supplied. Actual: Message == 'Only one of transactionHash or transactionId may be specified' and no fieldList.
Cause 4 — expected: 404 ResourceNotFoundException, Message == "The requested resource was not found."; actual: 404 ResourceNotFoundException, Message == 'No transaction with transactionHash 0000000000000000000000000000000000000000000000000000000000000000 exists on network ETHEREUM_MAINNET'.
Cause 5 — expected: error code "ValidationException" (HTTP 400) with Message "One or more parameter values were invalid."; actual: error code 'NotImplemented' ("operation ListFilteredTransactionEvents is scaffolded but not implemented in this emulator").

## Steps to Reproduce
Point a botocore client at the emulator's managedblockchain-query endpoint (region us-east-1):
1. Cause 1: `get_asset_contract(contractIdentifier={"network":"ETHEREUM_MAINNET","contractAddress":"!!!not-an-address!!!"})` and `list_token_balances(tokenFilter={"network":"ETHEREUM_MAINNET","contractAddress":"not-an-address!"})` -> inspect Error.Message; the quoted pattern carries ^ and $.
2. Cause 2: `batch_get_token_balance(getTokenBalanceInputs=[{"tokenIdentifier":{"network":"ETHEREUM_MAINNET","tokenId":"eth"},"ownerIdentifier":{"address":"0x28c6c06298d514db089934071355e5743bf21d60"}}]*11)` -> ValidationException, message not in the "1 validation error detected: ... Member must have length less than or equal to 10" form.
3. Cause 3: `get_transaction(network="ETHEREUM_MAINNET", transactionHash="a"*64, transactionId="b"*64)` and `get_transaction(network="ETHEREUM_MAINNET")` -> inspect the full error response: Error.Message is ad-hoc prose and the response carries no `fieldList`.
4. Cause 4: `get_transaction(network="ETHEREUM_MAINNET", transactionHash="0"*64)` -> 404 ResourceNotFoundException whose message echoes the hash and network.
5. Cause 5: `list_filtered_transaction_events(network="ETHEREUM_MAINNET", addressIdentifierFilter={"transactionEventToAddress":["0x28c6c06298d514db089934071355e5743bf21d60"]})` -> error code NotImplemented instead of ValidationException.
