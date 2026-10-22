Issue-Source: trace-sweep-divergence
Issue-Trace: firehose-untag-removes-specific-key-idempotent
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: firehose-untag-removes-specific-key-idempotent
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose|ListTagsForDeliveryStream|2xx->2xx|$.Tags[*]
Issue-Commit: 659e25b97046a77449047daf7fe310a5a83846e1

# Firehose UntagDeliveryStream drops every tag instead of only the keys named in TagKeys

> _Note: this issue was identified against the code base at commit `659e25b97046a77449047daf7fe310a5a83846e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UntagDeliveryStream

## Description
One root cause in the firehose service's tag state. firehose is reported bootstrapped by get_bootstrap_status (service-com-amazonaws-firehose-firehose-20150804), so this is a fidelity defect rather than a bootstrap gap.

Cause 1 of 1: after an UntagDeliveryStream that names a subset of the stream's tag keys, ListTagsForDeliveryStream reports no tags at all — the keys that were NOT named are gone too.
AWS: Smithy: UntagDeliveryStream takes TagKeys (@required, list, length 1..50) and is documented as 'Removes tags from the specified Firehose stream... If you specify a tag that doesn't exist, the operation ignores it' — i.e. exactly the named keys are removed, unnamed keys are retained, and re-removing an absent key is a no-op (idempotent). ListTagsForDeliveryStream returns Tags (@required, length 0..50) and HasMoreTags (@required), so a stream that still carries a tag must report that tag.
Emulator: ListTagsForDeliveryStream answers HTTP 200 — the expected status — but with an empty Tags list where the retained tag must appear: $.Tags[0] is missing, expected the entry for the key that was never passed in TagKeys (recorded example {'Key': 'keep', 'Value': '1'}).
Why: tags survive as the client's own index — cost allocation, ABAC conditions, inventory and cleanup selectors all read them back. An untag of one key silently destroying the rest means a client that removes a single tag loses unrelated state the API promised to retain, and the loss is unrecoverable ('Removed tags are deleted, and you can't recover them'). A client cannot distinguish 'tag removed' from 'all tags removed' without a full re-tag.
Fix: the UntagDeliveryStream handler's state mutation must delete only the keys present in TagKeys from the delivery stream's tag map (ignoring keys that are absent) rather than clearing the map; ListTagsForDeliveryStream must then serialize the surviving entries.
Alternative to rule out while fixing: the empty list could instead originate upstream, if the tag write earlier in this fixture never landed — the open issue 'Implement firehose TagDeliveryStream: unimplemented scaffold stub' (20261022T064119Z-firehose-tagdeliverystream-stub.md) covers that operation being a stub. That issue does NOT cover this divergence: the replay report lists no owner and no stub path for this cluster, and the step that diverged is ListTagsForDeliveryStream, not TagDeliveryStream. Confirm whether the tag write or the selective delete is at fault, but this fixture step must pass before it is closed.
Traces: firehose-untag-removes-specific-key-idempotent#task#8.

## Evidence
Fixture firehose-untag-removes-specific-key-idempotent, step task#8, operation ListTagsForDeliveryStream, verdict mismatch. Expected HTTP 200 with $.Tags[0] present — the tag key that was deliberately not passed to UntagDeliveryStream (recorded example {'Key': 'keep', 'Value': '1'}). Actual HTTP 200 with that element absent: replay detail "$.Tags[0]: missing (expected {'Key': 'keep', 'Value': '1'}, got None)". Status and shape are otherwise correct; the tag content is gone.

Replay context: trace set simparity-wire-aws-v1, 14 fixtures replayed, 32 steps, 22 passed, 10 mismatched, score 68.8%, state_reset_failures 0 (so this is not a leaked-state artifact from a previous fixture), emulator_up true.

## Steps to Reproduce
1. Clone the repo and boot the emulator from this checkout, then replay the wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture firehose-untag-removes-specific-key-idempotent in full; it creates a delivery stream, tags it with several keys (one of which is to be retained), then calls UntagDeliveryStream naming only a subset of those keys, including a repeat removal to assert idempotence.
3. At step task#8 the fixture calls ListTagsForDeliveryStream on that stream. Observe HTTP 200 with an empty Tags list instead of a list still containing the key that was never named in TagKeys.
4. While fixing, check both the UntagDeliveryStream state mutation (does it clear the whole tag map?) and whether the preceding tag write landed at all.
