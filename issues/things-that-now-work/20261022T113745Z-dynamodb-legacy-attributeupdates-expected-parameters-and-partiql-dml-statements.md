Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-legacy-attribute-updates-semantics
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-legacy-attribute-updates-semantics dynamodb-legacy-expected-conditional-semantics dynamodb-transaction-failure-class-discrimination dynamodb-transactional-write-and-read-multi-item dynamodb-partiql-and-classic-api-one-item-store
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.counter.N
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.data.S
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.defaulted
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.tags.SS[*]
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.status.S
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.note
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.body.S
Issue-Commit: 38aaeb6c250810a0cf0ab5b6ac35c7222e2099c5

# Legacy AttributeUpdates/Expected parameters and PartiQL DML statements are silently ignored

> _Note: this issue was identified against the code base at commit `38aaeb6c250810a0cf0ab5b6ac35c7222e2099c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateItem

## Description
Mutations expressed through the legacy (non-expression) parameters and through PartiQL return 200 but change nothing in the stored item. This bundle has 4 causes; all 4 must be closed.

Cause 1 of 4: UpdateItem's legacy AttributeUpdates actions (ADD / PUT / DELETE) are not applied.
AWS: Smithy: UpdateItem input AttributeUpdates (AttributeUpdates map of AttributeValueUpdate{Value, Action}) is the legacy equivalent of UpdateExpression: ADD on a number increments it, ADD on a string set unions members, DELETE removes set members, PUT overwrites or creates an attribute. The recording shows counter 10 -> 15, tags gaining 'c'/'d', data 'before' -> 'after' and a new 'defaulted' attribute.
Emulator: the item is unchanged after the update (counter 10, tags still ['x','b'], data 'before', no 'defaulted').
Why: SDK calls and older application code that still use AttributeUpdates get a 200 for a write that never happened - silent data loss with no error to catch.
Fix: the UpdateItem handler's legacy AttributeUpdates path (map each Action onto the same state mutation the expression path uses).
Traces: dynamodb-legacy-attribute-updates-semantics#task#10 (also #task#16).

Cause 2 of 4: UpdateItem with a legacy Expected condition does not apply its update. AWS: Smithy: UpdateItem input Expected (ExpectedAttributeMap) is the legacy conditional form; when the condition holds the update is applied (recording: status 'updated'). Emulator: the item still reads status 'active'. Why: conditional writes appear to succeed while leaving stale data; a client's optimistic-concurrency loop corrupts its own invariants. Fix: UpdateItem's legacy Expected evaluation and the mutation it gates. Traces: dynamodb-legacy-expected-conditional-semantics#task#9 (also dynamodb-transaction-failure-class-discrimination#task#5, dynamodb-transactional-write-and-read-multi-item#task#6).

Cause 3 of 4: a legacy Expected condition combined with ConditionalOperator OR does not apply its update. AWS: Smithy: UpdateItem input ConditionalOperator (AND/OR) over the Expected map; when one branch holds the write lands (recording: attribute note 'or-passed'). Emulator: the attribute is absent. Why: as Cause 2. Fix: UpdateItem's ConditionalOperator/Expected evaluation. Traces: dynamodb-legacy-expected-conditional-semantics#task#19.

Cause 4 of 4: a PartiQL UPDATE statement does not modify the item. AWS: Smithy: ExecuteStatement takes a PartiQLStatement and operates on the same item store as the classic API; after the recorded UPDATE the item reads body 'partiql-updated' through GetItem. Emulator: GetItem still returns body 'written'. Why: PartiQL and the classic API disagree about the same item, so an application that mixes them (the normal case for a migration) loses writes. Fix: the ExecuteStatement handler's DML path - UPDATE/INSERT/DELETE must mutate the shared item state. Traces: dynamodb-partiql-and-classic-api-one-item-store#task#8.

## Evidence
Representative: fixture dynamodb-legacy-attribute-updates-semantics step task#10, GetItem, expected 200 / actual 200:
  $.Item.counter.N: value_mismatch (expected '15', got '10')
  $.Item.tags.SS[1]: expected 'c', got 'b'; $.Item.tags.SS[2]: expected 'd', got None
  $.Item.data.S: value_mismatch (expected 'after', got 'before')
  $.Item.defaulted: missing (expected {'S': 'written'}, got None)
Other causes: dynamodb-legacy-expected-conditional-semantics#task#9 - $.Item.status.S expected 'updated' got 'active' (3 steps in cluster); #task#19 - $.Item.note missing (expected {'S': 'or-passed'}); dynamodb-partiql-and-classic-api-one-item-store#task#8 - $.Item.body.S expected 'partiql-updated' got 'written'. The attribute names/values are the recordings' examples; the requirement is that the legacy and PartiQL write paths mutate state like the expression path.

## Steps to Reproduce
1. Replay the wire corpus.
2. Fixture dynamodb-legacy-attribute-updates-semantics, step task#10: UpdateItem with AttributeUpdates {counter: ADD 5, tags: ADD/DELETE, data: PUT}, then GetItem - nothing changed.
3. Fixture dynamodb-legacy-expected-conditional-semantics, steps task#9 and task#19: UpdateItem with Expected (and with ConditionalOperator OR), then GetItem - the update is absent.
4. Fixture dynamodb-partiql-and-classic-api-one-item-store, step task#8: ExecuteStatement with a PartiQL UPDATE, then GetItem through the classic API - the old value is returned.
