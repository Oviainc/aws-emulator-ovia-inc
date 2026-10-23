Issue-Source: trace-sweep-divergence
Issue-Trace: managedblockchain-query-batch-get-token-balance-mixed
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: managedblockchain-query-batch-get-token-balance-mixed managedblockchain-query-get-asset-contract-lookup managedblockchain-query-get-transaction-error-paths managedblockchain-query-token-balance-single-and-list managedblockchain-query-list-asset-contracts-by-deployer managedblockchain-query-list-transactions-sort-and-range-error
Issue-Shape-Id: com.amazonaws.managedblockchainquery#TietonChainQueryService
Issue-Key: managedblockchain-query|BatchGetTokenBalance|2xx->4xx|$.errors
Issue-Key: managedblockchain-query|BatchGetTokenBalance|2xx->4xx|$.tokenBalances
Issue-Key: managedblockchain-query|BatchGetTokenBalance|2xx->4xx|status
Issue-Key: managedblockchain-query|GetAssetContract|4xx->4xx|$.message
Issue-Key: managedblockchain-query|GetAssetContract|4xx->4xx|$.reason
Issue-Key: managedblockchain-query|GetTransaction|4xx->4xx|$.message
Issue-Key: managedblockchain-query|GetTransaction|4xx->4xx|$.reason
Issue-Key: managedblockchain-query|GetTokenBalance|4xx->4xx|$.message
Issue-Key: managedblockchain-query|GetTokenBalance|4xx->4xx|$.reason
Issue-Key: managedblockchain-query|ListAssetContracts|4xx->4xx|$.message
Issue-Key: managedblockchain-query|ListAssetContracts|4xx->4xx|$.reason
Issue-Key: managedblockchain-query|GetTransaction|4xx->4xx|$.fieldList[*]
Issue-Key: managedblockchain-query|ListTransactions|4xx->4xx|$.fieldList[name="BlockchainInstant"]
Issue-Key: managedblockchain-query|ListTransactions|4xx->4xx|$.message
Issue-Commit: b4ac6d961cdb10c212ccdfac1172ee263c3e4aa3

# managedblockchain-query error responses are not AWS-shaped: ValidationException omits required reason, per-field detail and batch item errors

> _Note: this issue was identified against the code base at commit `b4ac6d961cdb10c212ccdfac1172ee263c3e4aa3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetTokenBalance

## Description
The wire corpus exercises eight rejection paths of this service. In all but one the HTTP status and the error code are right, but the error BODY the emulator renders is not the body AWS renders: the modeled-required ValidationException$reason member is never emitted, per-field detail is inlined into the top-level message instead of being placed in fieldList, pattern constraints are quoted with their model anchors, and a per-item batch failure fails the whole request instead of being reported inside a 200 response. Eight independent causes, all in this service's error-construction/validation layer; each needs its own fix.

--- Cause 1 of 8: BatchGetTokenBalance rejects the whole request instead of reporting the bad item in the 200 response's errors list ---
AWS: Smithy BatchGetTokenBalance (POST /batch-get-token-balance, http code 200) has output BatchGetTokenBalanceOutput with BOTH tokenBalances (required, @length 0..10) and errors (required, BatchGetTokenBalanceErrors, @length 0..10). BatchGetTokenBalanceErrorItem requires errorCode, errorMessage and errorType (ErrorType enum: VALIDATION_EXCEPTION | RESOURCE_NOT_FOUND_EXCEPTION) and echoes tokenIdentifier / ownerIdentifier / atBlockchainInstant. The existence of a required per-item error list is the contract: an item that cannot be resolved (here an unknown network value on one item's tokenIdentifier) is reported as one entry of errors with errorType VALIDATION_EXCEPTION in an HTTP 200 body (recorded errorCode "400", errorMessage "Please use a valid network from the QueryNetwork." — the texts are examples, the placement and errorType are the requirement), not as a top-level failure.
Emulator: HTTP 400 for the whole batch; the response carries neither tokenBalances nor errors, so no part of the batch is answered.
Why: batch partial-failure semantics are inverted. A client that submits a mixed batch loses every good item's balance and must parse a top-level error it is not supposed to receive; code that iterates response['errors'] to retry or report individual items sees a 400 exception instead and can never reach the per-item path.
Fix: src/ops/batch_get_token_balance.rs — validate each GetTokenBalanceInput item independently and accumulate failures into the output's errors list (with the item identifiers echoed and errorType VALIDATION_EXCEPTION), reserving a top-level ValidationException for request-level violations such as the getTokenBalanceInputs @length(1,10) cap.
Traces: managedblockchain-query-batch-get-token-balance-mixed#task#2.

--- Cause 2 of 8: GetAssetContract pattern rejection quotes the regex WITH anchors and omits ValidationException$reason ---
AWS: Smithy ChainAddress is @pattern("^[-A-Za-z0-9]{13,74}$") and is the target of GetAssetContractInput$contractIdentifier.contractAddress. ValidationException (httpError 400) models message (required), reason (REQUIRED, ValidationExceptionReason: unknownOperation | cannotParse | fieldValidationFailed | other) and fieldList. A value failing a member pattern is rejected 400 ValidationException with reason=fieldValidationFailed and the AWS validation-framework message built from the request member and the modeled constraint, with the pattern rendered UNANCHORED: "1 validation error detected: Value '<v>' at '<member path>' failed to satisfy constraint: Member must satisfy regular expression pattern: [-A-Za-z0-9]{13,74}". Because the message is derived from the request plus the model, that rendering is the requirement.
Emulator: HTTP 400 ValidationException with NO reason member at all, and the pattern echoed verbatim from the model including ^ and $.
Why: reason is a required member, so a client (or SDK model-validating layer) that switches on reason to tell a parse failure from a field-validation failure gets an absent field; anything that re-applies or displays the quoted pattern sees a regex AWS never sends.
Fix: this service's ValidationException construction helper in src/state.rs plus the shared validation-message renderer — always set reason, and strip the leading ^ / trailing $ from a modeled @pattern before rendering it into the message.
Traces: managedblockchain-query-get-asset-contract-lookup#task#2.

--- Cause 3 of 8: GetTransaction transactionHash pattern rejection has the same two defects ---
AWS: Smithy GetTransactionInput$transactionHash targets QueryTransactionHash, @pattern("^(0x[A-Fa-f0-9]{64}|[A-Fa-f0-9]{64})$"). A malformed hash is 400 ValidationException with reason=fieldValidationFailed and the message quoting the pattern unanchored: "... at 'transactionHash' failed to satisfy constraint: Member must satisfy regular expression pattern: (0x[A-Fa-f0-9]{64}|[A-Fa-f0-9]{64})".
Emulator: 400 ValidationException, reason missing, pattern rendered as "^(0x[A-Fa-f0-9]{64}|[A-Fa-f0-9]{64})$".
Why: as Cause 2 — the required discriminator is absent and the quoted constraint differs, on the most-used identifier of this API.
Fix: same renderer/helper as Cause 2, reached from src/ops/get_transaction.rs input validation.
Traces: managedblockchain-query-get-transaction-error-paths#task#1.

--- Cause 4 of 8: GetTokenBalance ownerIdentifier.address pattern rejection has the same two defects ---
AWS: OwnerIdentifier$address targets ChainAddress (@pattern ^[-A-Za-z0-9]{13,74}$, required on GetTokenBalanceInput$ownerIdentifier). A too-short address is 400 ValidationException, reason=fieldValidationFailed, message "... at 'ownerIdentifier.address' failed to satisfy constraint: Member must satisfy regular expression pattern: [-A-Za-z0-9]{13,74}".
Emulator: 400 ValidationException, reason missing, pattern rendered with anchors.
Why: as Cause 2, on a third independent member — confirming the defect is in the shared renderer rather than one handler.
Fix: same renderer/helper as Cause 2, reached from src/ops/get_token_balance.rs.
Traces: managedblockchain-query-token-balance-single-and-list#task#3.

--- Cause 5 of 8: ListAssetContracts enum rejection omits ValidationException$reason ---
AWS: ContractFilter$tokenStandard is required and targets QueryTokenStandard (values ERC20, ERC721, ERC1155). An unknown value is 400 ValidationException with reason=fieldValidationFailed and an enum-value-set message naming the member.
Emulator: 400 ValidationException with the correct status and code but no reason member.
Why: same client impact as Cause 2 — the required reason discriminator is absent on an enum failure too.
Fix: same ValidationException construction helper (src/state.rs) as used by src/ops/list_asset_contracts.rs.
Note (NOT filed as a defect): the recorded message lists the enum set as [ERC721, ERC20, ERC1155] while the emulator lists it in model declaration order [ERC20, ERC721, ERC1155]. Neither the model nor AWS documentation states the ordering rule, so the recorded order is an example only and the ordering difference alone is not a requirement; only the missing reason is.
Traces: managedblockchain-query-list-asset-contracts-by-deployer#task#2.

--- Cause 6 of 8: GetTransaction with no identifier inlines prose into message instead of using the generic message + fieldList ---
AWS: Smithy GetTransaction models transactionHash and transactionId both OPTIONAL (exactly one must be supplied; transactionId is documented Bitcoin-only), and ValidationException carries fieldList (ValidationExceptionFieldList of ValidationExceptionField{name, message}) described as "The container for the fieldList of the exception". AWS reports this class of failure as HTTP 400 ValidationException whose TOP-LEVEL message is the generic sentinel "One or more parameter values were invalid." with the per-field explanation moved into fieldList (recorded entry: {"message": "The request must include a transactionHash."}). The generic top-level sentinel is corroborated by a second, unrelated recording (Cause 7), so the rule — generic message at the top, detail in fieldList — is the requirement; the individual fieldList text is an example.
Emulator: HTTP 400 ValidationException whose message is the whole explanation ("One of transactionHash or transactionId must be specified") and whose single fieldList entry repeats that prose under name "transactionHash".
Why: the modeled fieldList is the machine-readable half of this error. A client that switches on the top-level message sees handler prose instead of AWS's sentinel, and a client that reads fieldList to distinguish "neither supplied" from "both supplied" from "transactionId on a non-Bitcoin network" cannot.
Fix: src/ops/get_transaction.rs identifier checks plus the ValidationException construction helper in src/state.rs — emit the generic message, set reason, and add one ValidationExceptionField per violated rule.
Traces: managedblockchain-query-get-transaction-error-paths#task#2.

--- Cause 7 of 8: ListTransactions instant-range rejection inlines prose into message and emits no fieldList entry for the violated field ---
AWS: Smithy ListTransactions takes optional fromBlockchainInstant / toBlockchainInstant (BlockchainInstant{time}). An inverted window is 400 ValidationException with the same generic top-level message "One or more parameter values were invalid." and a fieldList entry named for the offending field (recorded: name "BlockchainInstant", message "The 'time' field in fromBlockchainInstant must be less than or equal to the 'time' field in toBlockchainInstant.").
Emulator: HTTP 400 ValidationException whose message is "fromBlockchainInstant must not be later than toBlockchainInstant" and whose response contains NO fieldList entry with that name — the cross-field violation is reported only as prose.
Why: as Cause 6, plus a structural omission: there is no per-field entry at all, so a client cannot programmatically learn WHICH field pair it got wrong.
Fix: src/ops/list_transactions.rs range check plus the same ValidationException helper — generic message, reason set, and a ValidationExceptionField for the instant pair.
Traces: managedblockchain-query-list-transactions-sort-and-range-error#task#2.

--- Cause 8 of 8: GetTransaction not-found message echoes the request instead of this service's request-independent not-found text ---
AWS: recorded behavior only — Smithy documents ResourceNotFoundException (httpError 404) as "The resource was not found." with required members message, resourceId and resourceType, and does not state the message text. The recording shows a well-formed but unknown transactionHash answered with HTTP 404 ResourceNotFoundException and the fixed, request-independent message "The requested resource was not found.", consistent with this service keeping top-level error messages generic (Causes 6 and 7) and carrying the identity in resourceId / resourceType.
Emulator: HTTP 404 ResourceNotFoundException (status and code correct) with a message built from the request: "No transaction with transactionHash 0x0000...0000 exists on network ETHEREUM_MAINNET".
Why: the emulator leaks request-derived detail into an error AWS keeps constant, so clients and tests that compare or log this message see per-request text where AWS emits one string. Lowest-impact cause in this bundle (status and code agree) and the only one grounded in the recording rather than the model.
Fix: the ResourceNotFoundException construction helper in src/state.rs as called from src/ops/get_transaction.rs — use the service's generic not-found message and keep the identity in resourceId / resourceType.
Traces: managedblockchain-query-get-transaction-error-paths#task#3.

Related open issues (neither carries these wire replay keys, so none of the eight causes above is covered by them): "ManagedBlockchain Query error contract diverges: validation messages are not AWS-shaped, ValidationException omits fieldList, and the Bitcoin-only net..." (behavioral corpus, overlaps Causes 2-4 and 6) and the scaffold-stub issues for BatchGetTokenBalance / GetAssetContract / ListFilteredTransactionEvents. Replaying this bundle's fixtures is the only check that these wire paths are fixed.

## Evidence
Replay of trace set simparity-wire-aws-v1 (8 fixtures, 20 steps, 2 passed, score 10%), all clusters service=managedblockchain-query, stub_path empty, owned_by empty:
Cause 1 — managedblockchain-query-batch-get-token-balance-mixed#task#2 BatchGetTokenBalance: status expected 200, got 400; $.errors missing (expected [{errorCode: '400', errorMessage: 'Please use a valid network from the QueryNetwork.', errorType: 'VALIDATION_EXCEPTION', ownerIdentifier: {...}, tokenIdentifier: {contractAddress: ..., network: 'NOT_A_NETWORK'}}], got None); $.tokenBalances missing (expected [], got None).
Cause 2 — managedblockchain-query-get-asset-contract-lookup#task#2 GetAssetContract: 400/400; $.message expected "...pattern: [-A-Za-z0-9]{13,74}", got "...pattern: ^[-A-Za-z0-9]{13,74}$"; $.reason expected 'fieldValidationFailed', got None.
Cause 3 — managedblockchain-query-get-transaction-error-paths#task#1 GetTransaction: 400/400; $.message expected "...pattern: (0x[A-Fa-f0-9]{64}|[A-Fa-f0-9]{64})", got the anchored "^(...)$" form; $.reason expected 'fieldValidationFailed', got None.
Cause 4 — managedblockchain-query-token-balance-single-and-list#task#3 GetTokenBalance: 400/400; $.message expected "...at 'ownerIdentifier.address' ... pattern: [-A-Za-z0-9]{13,74}", got the anchored form; $.reason expected 'fieldValidationFailed', got None.
Cause 5 — managedblockchain-query-list-asset-contracts-by-deployer#task#2 ListAssetContracts: 400/400; $.reason expected 'fieldValidationFailed', got None (enum-set ordering difference in $.message recorded as an example only).
Cause 6 — managedblockchain-query-get-transaction-error-paths#task#2 GetTransaction: 400/400; $.message expected 'One or more parameter values were invalid.', got 'One of transactionHash or transactionId must be specified'; $.fieldList[0] expected {'message': 'The request must include a transactionHash.'}, got {'name': 'transactionHash', 'message': 'One of transactionHash or transactionId must be specified'}.
Cause 7 — managedblockchain-query-list-transactions-sort-and-range-error#task#2 ListTransactions: 400/400; $.message expected 'One or more parameter values were invalid.', got 'fromBlockchainInstant must not be later than toBlockchainInstant'; $.fieldList[name="BlockchainInstant"] expected {'name': 'BlockchainInstant', 'message': "The 'time' field in fromBlockchainInstant must be less than or equal to the 'time' field in toBlockchainInstant."}, got None.
Cause 8 — managedblockchain-query-get-transaction-error-paths#task#3 GetTransaction: 404/404; $.message expected 'The requested resource was not found.', got 'No transaction with transactionHash 0x0000000000000000000000000000000000000000000000000000000000000000 exists on network ETHEREUM_MAINNET'.
Model citations: ValidationException{message required, reason required (ValidationExceptionReason: unknownOperation|cannotParse|fieldValidationFailed|other), fieldList}; ValidationExceptionField{name required, message required}; ChainAddress @pattern ^[-A-Za-z0-9]{13,74}$; QueryTransactionHash @pattern ^(0x[A-Fa-f0-9]{64}|[A-Fa-f0-9]{64})$; QueryTokenStandard = [ERC20, ERC721, ERC1155]; BatchGetTokenBalanceOutput{tokenBalances required, errors required}; BatchGetTokenBalanceErrorItem{errorCode, errorMessage, errorType required}; ErrorType = [VALIDATION_EXCEPTION, RESOURCE_NOT_FOUND_EXCEPTION]; ResourceNotFoundException httpError 404 {message, resourceId, resourceType required}.

## Steps to Reproduce
Replay the wire corpus (replay_wire_corpus) and inspect the eight steps named under Traces, or drive the emulator's managedblockchain-query endpoint directly (region us-east-1):
1. Cause 1: POST /batch-get-token-balance with a mixed getTokenBalanceInputs list where one item's tokenIdentifier.network is 'NOT_A_NETWORK' -> observe HTTP 400 for the whole batch instead of HTTP 200 with tokenBalances + a VALIDATION_EXCEPTION entry in errors.
2. Cause 2: get_asset_contract(contractIdentifier={network:'ETHEREUM_MAINNET', contractAddress:'0x123'}) -> message quotes ^...$ and no reason member.
3. Cause 3: get_transaction(network='ETHEREUM_MAINNET', transactionHash='not-a-hash') -> same two defects.
4. Cause 4: get_token_balance(tokenIdentifier={network:'ETHEREUM_MAINNET', tokenId:'eth'}, ownerIdentifier={address:'short'}) -> same two defects.
5. Cause 5: list_asset_contracts(contractFilter={network:'ETHEREUM_MAINNET', tokenStandard:'NOT_A_STANDARD', deployerAddress:<valid address>}) -> reason member absent.
6. Cause 6: get_transaction(network='ETHEREUM_MAINNET') with neither identifier -> top-level message is handler prose, fieldList entry repeats it.
7. Cause 7: list_transactions(address=<valid address>, network='ETHEREUM_MAINNET', fromBlockchainInstant={time:T2}, toBlockchainInstant={time:T1}) with T2 > T1 -> prose message and no fieldList entry named 'BlockchainInstant'.
8. Cause 8: get_transaction(network='ETHEREUM_MAINNET', transactionHash='0x' + '0'*64) -> 404 whose message echoes the hash and network.
