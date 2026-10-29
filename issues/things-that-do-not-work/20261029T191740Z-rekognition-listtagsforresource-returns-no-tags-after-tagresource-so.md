Issue-Source: trace-sweep-divergence
Issue-Trace: rekognition-tag-untag-resource-round-trip
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rekognition-tag-untag-resource-round-trip
Issue-Shape-Id: com.amazonaws.rekognition#RekognitionService
Issue-Key: rekognition|ListTagsForResource|2xx->2xx|$.Tags.env
Issue-Key: rekognition|ListTagsForResource|2xx->2xx|$.Tags.owner
Issue-Commit: 0e13384b9161da1d0d65830e33a6518406bea9f8

# Rekognition ListTagsForResource returns no tags after TagResource, so collection tags are never persisted

> _Note: this issue was identified against the code base at commit `0e13384b9161da1d0d65830e33a6518406bea9f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
Two causes, one fault: tags applied to a Rekognition collection are not stored, so ListTagsForResource answers 200 with an empty Tags map at every point in the tag/untag round trip.

Cause 1 of 2: ListTagsForResource immediately after TagResource returns none of the applied tags.
AWS: Smithy: TagResource input ResourceArn (required, length 20..2048) and Tags (required, TagMap, size 0..200); ListTagsForResource output Tags (TagMap). Recorded behavior: after TagResource succeeds (HTTP 200, empty body) on a collection ARN, ListTagsForResource on that same ARN returns HTTP 200 with Tags containing exactly the key/value pairs that were applied -- here both keys submitted by the preceding TagResource call.
Emulator: HTTP 200 but Tags is empty -- both submitted keys are missing from the response.
Why: a client that tags a collection and then reads its tags back (cost allocation, ownership/environment labelling, tag-based access or reconciliation loops) sees an untagged resource and will either re-tag in a loop or conclude the tagging call silently failed. TagResource itself returns success, so the failure is invisible until the read.
Fix: the TagResource handler/collection state -- persist the tag map against the collection and have ListTagsForResource read it back. Hypothesis: the ARN the collection is keyed under does not match the ARN the request carries, because CreateCollection mints an "arn:aws:rekognition:..." CollectionArn while the recorded request uses Rekognition's "aws:rekognition:..." form (filed separately as the CreateCollection ARN-prefix divergence); if so the tag store never finds the resource. Tag lookup should accept the ARN form CreateCollection returns.
Traces: rekognition-tag-untag-resource-round-trip#task#3.

Cause 2 of 2: ListTagsForResource after UntagResource removes one key returns an empty map instead of the surviving key.
AWS: Recorded behavior: after UntagResource removes one of the two keys, ListTagsForResource on the collection ARN returns HTTP 200 with the remaining key/value pair still present -- untagging removes only the named keys.
Emulator: HTTP 200 with Tags empty; the surviving key is missing.
Why: a client cannot distinguish "untag removed only what I asked for" from "untag wiped every tag", so a selective untag appears destructive and tag-driven bookkeeping loses state.
Fix: same component as cause 1 -- once the tag map is persisted and keyed by the ARN the service hands out, UntagResource must delete only the named keys and leave the rest readable.
Traces: rekognition-tag-untag-resource-round-trip#task#5.

## Evidence
Replay of simparity-wire-aws-v1, verdict=mismatch; expected status == actual status (200) in both -- only the body diverges.

Cause 1 (rekognition-tag-untag-resource-round-trip#task#3, ListTagsForResource): $.Tags.env missing (expected 'sim2real', got None); $.Tags.owner missing (expected 'behavioral', got None).
Cause 2 (rekognition-tag-untag-resource-round-trip#task#5, ListTagsForResource): $.Tags.env missing (expected 'sim2real', got None) -- the key that UntagResource did not remove.

Related (filed separately, same service): CreateCollection at rekognition-tag-untag-resource-round-trip#task#1 returns CollectionArn with an extra "arn:" prefix, which is the ARN the tagging steps of this fixture carry.

## Steps to Reproduce
1. Replay fixture rekognition-tag-untag-resource-round-trip. Step task#1 creates the collection; step task#2 calls TagResource on its ARN with two tag keys.
2. Step task#3 (ListTagsForResource on the same ResourceArn): assert HTTP 200 and that Tags contains both keys applied by task#2 with their submitted values.
3. Step task#4 calls UntagResource removing one of the two keys.
4. Step task#5 (ListTagsForResource again): assert HTTP 200 and that Tags still contains the key that was NOT untagged, with its original value.
