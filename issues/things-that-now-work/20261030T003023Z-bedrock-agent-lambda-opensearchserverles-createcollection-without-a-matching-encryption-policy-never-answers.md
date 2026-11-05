Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent__lambda__opensearchserverless.bedrock-kb-opensearchserverless-encryption-policy-missing-negative
Issue-Key: bedrock-agent__lambda__opensearchserverless.bedrock-kb-opensearchserverless-encryption-policy-missing-negative|_create_vector_index#19|Timeout|time.sleep(15)
Issue-Commit: c1b8e21acf710dd592fea1840f63adefca5c3a99

# CreateCollection without a matching encryption policy never answers, so the ValidationException refusal is never produced

> _Note: this issue was identified against the code base at commit `c1b8e21acf710dd592fea1840f63adefca5c3a99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCollection

## Description
Cause 1 of 1: opensearchserverless CreateCollection for a VECTORSEARCH collection that has no matching encryption security policy never returns, so the modeled refusal is never delivered and the dependent knowledge-base path cannot be reached.

AWS: Smithy (com.amazonaws.opensearchserverless, CreateCollection) models input name (required, @length 3..64, @pattern ^[a-z][a-z0-9-]+$) and type, and declares ValidationException as a client error with httpError 400 (alongside ConflictException 409, OcuLimitExceededException 402, ServiceQuotaExceededException 402, InternalServerException 500). Real AWS refuses a CreateCollection whose name is not covered by a pre-existing encryption-type security policy with ValidationException (HTTP 400) and the message `No matching security policy of encryption type found for collection name: <name>. Please create security policy of encryption type for this collection.` — the fixed text is everything except `<name>`, which is the `name` member taken verbatim from the request; the sentence ends with the literal second sentence shown. Nothing is created: BatchGetCollection(names=[<name>]) then returns collectionDetails == [] (Smithy: BatchGetCollection output members collectionDetails and collectionErrorDetails). Creating the missing encryption policy — and only that — lets the identical CreateCollection succeed and reach ACTIVE, after which network and data-access policies, a vector index and a Bedrock knowledge base on top of it all succeed.

Emulator: the call never completes. Verdict `mismatch`, detail `Error: Check timed out after 60.0s`; no error code, no HTTP status and no message were ever produced for the un-covered CreateCollection, and the positive control (encryption policy -> CreateCollection -> wait ACTIVE -> knowledge base) is never reached.

Why: a client gets no refusal and no success — an SDK call that AWS answers 400 in one round trip blocks until the caller's own timeout. The security-policy precondition that makes Serverless collections safe by construction is unenforced and unreportable, so infrastructure code that depends on the refusal (or on BatchGetCollection showing nothing was created) cannot work, and a knowledge base can be pointed at a collection that was never validated.

Fix: the opensearchserverless CreateCollection handler and its validation — it must look up the encryption-type security policies whose resource pattern matches the requested collection name and, when none matches, return ValidationException (400) with the message above and create nothing, instead of blocking.

Related open issues 20261029T191222Z-bedrock-agent-lambda-opensearchserverles-bedrock-agent-lambda-opensearchserverless-4-net-new-causes.md and 20261029T171944Z-...-2-net-new-causes.md cover other causes in this service group (including the network- and data-access-policy negatives); neither carries this check's replay key, so the encryption-policy precondition is filed here.

Traces: bedrock-agent__lambda__opensearchserverless.bedrock-kb-opensearchserverless-encryption-policy-missing-negative.

## Evidence
Expected (real AWS): client.create_collection(name=<coll-name>, type='VECTORSEARCH') with no matching encryption policy raises ValidationException (HTTP 400) with message exactly `No matching security policy of encryption type found for collection name: <coll-name>. Please create security policy of encryption type for this collection.`, and client.batch_get_collection(names=[<coll-name>]) then returns {'collectionDetails': []}. After CreateSecurityPolicy(type='encryption') covering that name, the identical CreateCollection succeeds and the collection becomes ACTIVE.

Actual (emulator): verdict `mismatch`, detail `Error: Check timed out after 60.0s`. The CreateCollection call produced neither a response nor an error before the harness budget expired, so no error code or message was observable.

## Steps to Reproduce
1. Pick a collection name matching ^[a-z][a-z0-9-]+$ (3..64 chars) for which no encryption-type security policy exists.
2. Call opensearchserverless CreateCollection(name=<name>, type='VECTORSEARCH') — observe the call never returns instead of raising ValidationException 400 with the 'No matching security policy of encryption type found for collection name: <name>.' message.
3. Call BatchGetCollection(names=[<name>]) and confirm collectionDetails is empty (nothing was created).
4. Create the encryption security policy covering <name>, then repeat step 2 and confirm the collection is created and reaches ACTIVE, and that a Bedrock knowledge base can be built on it.
