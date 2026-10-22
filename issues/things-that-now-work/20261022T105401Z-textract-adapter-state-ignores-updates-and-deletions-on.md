Issue-Source: trace-sweep-divergence
Issue-Trace: textract-adapter-lifecycle
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: textract-adapter-lifecycle
Issue-Shape-Id: com.amazonaws.textract#Textract
Issue-Key: textract|GetAdapter|2xx->2xx|$.AutoUpdate
Issue-Key: textract|GetAdapter|2xx->2xx|$.Description
Issue-Key: textract|GetAdapter|4xx->2xx|$.Message
Issue-Key: textract|GetAdapter|4xx->2xx|$.__type
Issue-Key: textract|GetAdapter|4xx->2xx|status
Issue-Commit: dcb14db06d6eb8eb19e965c6ac371e7edd3d0be4

# Textract adapter state ignores updates and deletions on GetAdapter

> _Note: this issue was identified against the code base at commit `dcb14db06d6eb8eb19e965c6ac371e7edd3d0be4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAdapter

## Description
GetAdapter serves the adapter as it was first created: neither a mutation nor a removal performed earlier in the same fixture is visible to a subsequent read. This issue bundles 2 distinct root causes in the adapter state layer. A fixer must close BOTH before submitting.

Cause 1 of 2 - a mutated adapter's Description and AutoUpdate are not reflected by GetAdapter.
AWS: Smithy (services/service-com-amazonaws-textract-textract/smithy-model/com-amazonaws-textract-textract.json): UpdateAdapter input {AdapterId required, Description, AdapterName, AutoUpdate} and output {AdapterId, AdapterName, CreationTime, Description, FeatureTypes, AutoUpdate}; GetAdapter output carries the same members. A read after a successful mutation returns the mutated values - in the recording, GetAdapter answers HTTP 200 with AutoUpdate 'ENABLED' and the updated Description (recorded example 'probe adapter v2'). The requirement is that GetAdapter echoes whatever Description/AutoUpdate the preceding mutation set, not those particular strings.
Emulator: HTTP 200 but with the pre-update values - AutoUpdate 'DISABLED' and the original Description (recorded example 'probe adapter v1'). The mutation's response is evidently not persisted into the adapter record that GetAdapter reads.
Why: read-after-write is broken for adapters. A client that updates an adapter and re-reads it to confirm sees its change silently dropped, so configuration flows (enabling AutoUpdate, re-describing an adapter) appear to succeed while the resource never changes - and any logic that branches on AutoUpdate takes the wrong path.
Fix: the UpdateAdapter handler and the adapter record in the service's state - apply the supplied Description/AutoUpdate (and AdapterName) to the stored adapter so GetAdapter/ListAdapters observe them, rather than only echoing them in the mutation response.
Traces: textract-adapter-lifecycle#task#4.

Cause 2 of 2 - a deleted adapter is still readable: GetAdapter returns 200 instead of ResourceNotFoundException.
AWS: Smithy: GetAdapter input {AdapterId required} with ResourceNotFoundException modeled as a client error at HTTP 400. Once the adapter has been deleted, the recorded response for a read of that same AdapterId is HTTP 400 with __type 'ResourceNotFoundException' (recorded message example 'Adapter Id not found.' - the status and the error code are the requirement, the sentence is an example). Hypothesis: the step is the fixture's post-deletion read in its teardown phase, which is why the recording expects the not-found error for an id that resolved moments earlier.
Emulator: HTTP 200 with the adapter body - $.__type and $.Message are both absent - so the deletion left the record readable.
Why: deletion is not observable. A client cannot confirm teardown, idempotent cleanup loops never terminate on the modeled not-found error, and a resource that AWS reports as gone keeps answering reads, so tests that assert deletion pass against AWS and fail against the emulator (or vice versa).
Fix: the DeleteAdapter handler and the adapter state it writes to - remove (or tombstone) the adapter record so GetAdapter raises ResourceNotFoundException (HTTP 400) for that AdapterId afterwards.
Traces: textract-adapter-lifecycle#teardown#15.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator booted from this checkout (state_reset_failures: 0, so each fixture ran against its own state).

Cause 1 - fixture textract-adapter-lifecycle step task#4, GetAdapter: status matched (200) but $.AutoUpdate value_mismatch (expected 'ENABLED', got 'DISABLED') and $.Description value_mismatch (expected 'probe adapter v2', got 'probe adapter v1') - i.e. exactly the two members the preceding update set.

Cause 2 - fixture textract-adapter-lifecycle step teardown#15, GetAdapter: expected status 400, got 200. Detail: $.__type missing (expected 'ResourceNotFoundException'), $.Message missing (recorded example 'Adapter Id not found.').

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Replay fixture textract-adapter-lifecycle.
3. Step task#4 reads back an adapter that the fixture has just updated: the emulator returns the pre-update Description and AutoUpdate 'DISABLED' instead of the updated Description and 'ENABLED'.
4. Step teardown#15 reads the adapter after the fixture has deleted it: the emulator returns 200 with the adapter body instead of 400 with __type ResourceNotFoundException.
5. Both steps pass only when the adapter record is actually mutated by UpdateAdapter and actually removed by DeleteAdapter.
