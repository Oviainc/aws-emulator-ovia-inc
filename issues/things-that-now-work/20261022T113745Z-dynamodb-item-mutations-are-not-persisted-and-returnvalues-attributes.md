Issue-Source: trace-sweep-divergence
Issue-Trace: ddbdepth-ccfe-deleteitem-returnvalues
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ddbdepth-ccfe-deleteitem-returnvalues ddbval-expression-over-4kb-rejected dynamodb-batch-write-and-get-across-items dynamodb-batch-write-item-mixed-put-delete dynamodb-item-crud-and-expressions dynamodb-item-crud-roundtrip-with-conditions dynamodb-legacy-attribute-updates-semantics dynamodb-legacy-expected-conditional-semantics dynamodb-number-precision-38-significant-digits dynamodb-numeric-key-canonicalized-on-every-path dynamodb-partiql-and-classic-api-one-item-store dynamodb-partiql-select-result-body-shape dynamodb-set-empty-and-duplicate-members-rejected dynamodb-transactional-write-and-read-multi-item ddbdepth-ccfe-putitem-returnvalues dynamodb-return-values-all-new-rejected-on-put-and-delete
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|GetItem|2xx->2xx|$
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.blob
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.meta
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.tags
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.ver.N
Issue-Key: dynamodb|Query|2xx->2xx|$.Count
Issue-Key: dynamodb|Query|2xx->2xx|$.Items[*]
Issue-Key: dynamodb|Query|2xx->2xx|$.ScannedCount
Issue-Key: dynamodb|DeleteItem|2xx->2xx|$
Issue-Key: dynamodb|DeleteItem|2xx->2xx|$.Attributes.via.S
Issue-Key: dynamodb|PutItem|2xx->2xx|$.Attributes.body.S
Issue-Commit: 38aaeb6c250810a0cf0ab5b6ac35c7222e2099c5

# Item mutations are not persisted and ReturnValues attributes are dropped on the write path

> _Note: this issue was identified against the code base at commit `38aaeb6c250810a0cf0ab5b6ac35c7222e2099c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateItem

## Description
Writes acknowledged with 200 do not change stored state: deletes leave the item, puts do not replace it, updates do not apply, and ReturnValues=ALL_OLD echoes are empty or stale. This bundle has 6 causes; all 6 must be closed.

Cause 1 of 6: DeleteItem does not remove the item - a following GetItem still returns it.
AWS: Smithy: DeleteItem removes the item addressed by Key; a GetItem for a deleted key returns 200 with no Item member (the recording shows an empty body).
Emulator: 200 with the full item still present.
Why: a client that deletes and then re-reads sees the deleted item, so dedup/cleanup logic and 'delete then recreate' flows silently operate on stale data - the most dangerous class of emulator divergence because nothing errors.
Fix: the DeleteItem handler's state mutation (the item must actually be removed from the table's item map, including on the conditional/ReturnValues path).
Traces: ddbdepth-ccfe-deleteitem-returnvalues#task#9 (21 steps in cluster, incl. dynamodb-batch-write-item-mixed-put-delete#task#5, dynamodb-transactional-write-and-read-multi-item#task#7).

Cause 2 of 6: PutItem does not replace the existing item. AWS: Smithy: PutItem 'creates a new item, or replaces an old item with a new item' - after a put the stored item is exactly the request Item (recording: ver 2 and no other attributes). Emulator: GetItem returns ver 1 plus the earlier item's meta, tags and blob attributes. Why: a client relying on put-as-replace keeps attributes it deliberately removed. Fix: PutItem's state mutation must overwrite the whole item, not merge into it. Traces: ddbdepth-ccfe-putitem-returnvalues#task#10.

Cause 3 of 6: a Query after an UpdateItem finds nothing. AWS: Smithy: Query -> 200 with Items/Count/ScannedCount; the recording shows Count 1 and the updated item (status 'UPDATED', count 1). Emulator: Count 0, ScannedCount 0, Items empty. Why: the update was acknowledged but is invisible to reads, so read-after-write is broken. Fix: UpdateItem's state mutation and/or the Query index over stored items. Traces: dynamodb-item-crud-roundtrip-with-conditions#task#9.

Cause 4 of 6: DeleteItem with ReturnValues=ALL_OLD returns an empty body. AWS: Smithy: DeleteItem -> output Attributes (AttributeMap); ReturnValue enum includes ALL_OLD, and the recording returns the full pre-delete item. Emulator: 200 with {} - no Attributes. Why: a client that needs the deleted item (to archive it or emit an event) loses it irrecoverably. Fix: DeleteItem's ReturnValues handling/serializer. Traces: dynamodb-item-crud-roundtrip-with-conditions#task#11.

Cause 5 of 6: DeleteItem's ALL_OLD Attributes reflect a pre-update version of the item. AWS: the returned Attributes are the item as it stood immediately before the delete (recording: via 'updated'). Emulator: via 'put' - the value from before the intervening UpdateItem. Why: the archived 'old' copy is wrong, silently. Fix: UpdateItem state mutation (the update never landed) and DeleteItem's ALL_OLD snapshot. Traces: dynamodb-numeric-key-canonicalized-on-every-path#task#13.

Cause 6 of 6: PutItem's ALL_OLD Attributes are stale. AWS: Smithy: PutItem -> output Attributes with ReturnValues=ALL_OLD returning the item as replaced (recording: body 'updated'). Emulator: body 'original'. Why: as Cause 5 - the caller's 'previous value' is wrong. Fix: PutItem's ALL_OLD snapshot plus the missing UpdateItem mutation. Traces: dynamodb-return-values-all-new-rejected-on-put-and-delete#task#13.

## Evidence
Representative: fixture ddbdepth-ccfe-deleteitem-returnvalues step task#9, GetItem, expected 200 / actual 200:
  $: value_mismatch (expected {}, got {'Item': {'pk': {'S': 'k1'}, 'ver': {'N': '1'}, 'tags': {'SS': ['x','y']}, 'meta': {...}, 'blob': {'B': 'AAH+/w=='}}})
  - 21 steps in this cluster across 16 fixtures.
Other causes: ddbdepth-ccfe-putitem-returnvalues#task#10 - $.Item.ver.N expected '2' got '1', with meta/tags/blob 'extra'; dynamodb-item-crud-roundtrip-with-conditions#task#9 - $.Count expected 1 got 0, $.ScannedCount expected 1 got 0, Items[0] missing; #task#11 - DeleteItem body empty_body where Attributes were expected; dynamodb-numeric-key-canonicalized-on-every-path#task#13 - $.Attributes.via.S expected 'updated' got 'put'; dynamodb-return-values-all-new-rejected-on-put-and-delete#task#13 - $.Attributes.body.S expected 'updated' got 'original'. Attribute values are the recordings' examples; the requirement is that the mutation is persisted and the ALL_OLD snapshot is the immediately preceding item.

## Steps to Reproduce
1. Replay the wire corpus.
2. Fixture ddbdepth-ccfe-deleteitem-returnvalues, step task#9: PutItem, DeleteItem (200), then GetItem - the item is still returned.
3. Fixture ddbdepth-ccfe-putitem-returnvalues, step task#10: PutItem a smaller item over an existing one, then GetItem - the old attributes survive.
4. Fixture dynamodb-item-crud-roundtrip-with-conditions, steps task#9/task#11: UpdateItem then Query (no rows), then DeleteItem with ReturnValues=ALL_OLD (empty body).
5. Steps dynamodb-numeric-key-canonicalized-on-every-path#task#13 and dynamodb-return-values-all-new-rejected-on-put-and-delete#task#13 for the stale ALL_OLD snapshots.
