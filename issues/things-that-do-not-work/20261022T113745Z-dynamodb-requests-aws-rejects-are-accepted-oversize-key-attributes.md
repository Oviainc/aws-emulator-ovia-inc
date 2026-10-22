Issue-Source: trace-sweep-divergence
Issue-Trace: ddbval-partition-key-over-2048-bytes-rejected
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ddbval-partition-key-over-2048-bytes-rejected ddbval-sort-key-over-1024-bytes-rejected dynamodb-number-precision-38-significant-digits dynamodb-set-empty-and-duplicate-members-rejected dynamodb-validation-error-wording-across-the-data-plane dynamodb-validation-message-names-the-violated-concept ddbctl-resource-policy-deny-vs-not-found
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|PutItem|4xx->2xx|$
Issue-Key: dynamodb|PutItem|4xx->2xx|status
Issue-Key: dynamodb|GetItem|4xx->2xx|$.__type
Issue-Key: dynamodb|GetItem|4xx->2xx|$.message
Issue-Key: dynamodb|GetItem|4xx->2xx|status
Issue-Key: dynamodb|GetItem|4xx->2xx|$
Issue-Key: dynamodb|GetItem|4xx->2xx|$.Message
Issue-Commit: 38aaeb6c250810a0cf0ab5b6ac35c7222e2099c5

# Requests AWS rejects are accepted: oversize key attributes stored and resource-policy denials ignored

> _Note: this issue was identified against the code base at commit `38aaeb6c250810a0cf0ab5b6ac35c7222e2099c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutItem

## Description
Three clusters where the emulator answers 200 for a request AWS refuses with 400. This bundle has 3 causes; all 3 must be closed.

Cause 1 of 4... (corrected) Cause 1 of 3: key attributes above the documented key-size limits are accepted and stored.
AWS: Smithy: PutItem input Item (PutItemInputAttributeMap, required) and the service-common ValidationException; DynamoDB limits a partition-key value to 2048 bytes and a sort-key value to 1024 bytes. The recording shows 400 with __type com.amazon.coral.validate#ValidationException and a message of the form 'One or more parameter values were invalid: Size of hashkey has exceeded the maximum size limit of 2048 bytes' (the sort-key case names the range key).
Emulator: 200 with an empty body - the item is written.
Why: a client that would be told its key is too large instead stores an item it can never store on real AWS, so the failure moves from a clear 400 at write time to a surprise at migration time; items keyed beyond the limit also break key-comparison assumptions.
Fix: PutItem's input validation (key-size checks for HASH and RANGE key attributes, raising ValidationException before the item reaches state).
Traces: ddbval-partition-key-over-2048-bytes-rejected#task#4 (14 steps in cluster, incl. ddbval-sort-key-over-1024-bytes-rejected#task#4, dynamodb-set-empty-and-duplicate-members-rejected#task#4..#task#9, dynamodb-number-precision-38-significant-digits#task#5 - the same missing-validation path also lets empty/duplicate set members and over-precision numbers through).

Cause 2 of 3: GetItem with an oversize key is accepted instead of rejected. AWS: Smithy: GetItem input Key (required) is subject to the same key-size limits; the recording shows 400 with __type com.amazon.coral.validate#ValidationException. Emulator: 200 and no error members. Why: the read path agrees with the broken write path instead of with AWS, hiding the defect from a client that probes with a read. Fix: GetItem's key validation (share the key-size validator with the write path). Traces: ddbval-partition-key-over-2048-bytes-rejected#task#7 (also ddbval-sort-key-over-1024-bytes-rejected#task#7).

Cause 3 of 3: a resource-policy denial is not enforced - the item is returned. AWS: Smithy: GetItem errors include ResourceNotFoundException; an identity denied by the table's resource policy gets 400 with __type com.amazon.coral.service#AccessDeniedException (recorded message not asserted). Emulator: 200 with the item body, for the entire 60s / 13-attempt poll. Why: an authorization test passes against the emulator and fails - or worse, leaks data - against AWS; a client cannot develop or verify resource-policy behavior at all, and the deny-vs-not-found distinction AWS draws is lost. Fix: the GetItem handler's authorization check against the table resource policy stored by PutResourcePolicy. Traces: ddbctl-resource-policy-deny-vs-not-found#task#6.

## Evidence
Representative: fixture ddbval-partition-key-over-2048-bytes-rejected step task#4, PutItem, expected 400 / actual 200:
  status: expected 400, got 200
  $: empty_body (expected {'__type': 'com.amazon.coral.validate#ValidationException', 'message': 'One or more parameter values were invalid: Size of hashkey has exceeded the maximum size limit of2048 bytes'}, got {})
  - 14 steps in this cluster, covering oversize partition and sort keys, empty/duplicate set members and over-precision numbers.
Cause 2: ddbval-partition-key-over-2048-bytes-rejected#task#7, GetItem, expected 400 / actual 200 with $.__type and $.message missing.
Cause 3: ddbctl-resource-policy-deny-vs-not-found#task#6, GetItem, expected 400 with $.__type 'com.amazon.coral.service#AccessDeniedException', actual 200 with {'Item': {'pk': {'S': 'secret'}, 'v': {'S': 's1'}}} after 13 poll attempts. The recorded message text is an example; the 400 status and the error codes are the requirement.

## Steps to Reproduce
1. Replay the wire corpus.
2. Fixture ddbval-partition-key-over-2048-bytes-rejected, step task#4: PutItem whose partition-key string exceeds 2048 bytes - the emulator returns 200 and stores it (same for a >1024-byte sort key in ddbval-sort-key-over-1024-bytes-rejected#task#4).
3. Same fixture, step task#7: GetItem with that oversize key also returns 200 instead of ValidationException.
4. Fixture ddbctl-resource-policy-deny-vs-not-found, step task#6: attach a denying resource policy, then GetItem - the item is returned with 200 instead of 400 AccessDeniedException.
