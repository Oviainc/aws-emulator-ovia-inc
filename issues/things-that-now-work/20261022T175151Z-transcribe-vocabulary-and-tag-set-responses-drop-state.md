Issue-Source: trace-sweep-divergence
Issue-Trace: transcribe-vocabulary-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: transcribe-vocabulary-lifecycle transcribe-tag-resource-list-untag-roundtrip
Issue-Shape-Id: com.amazonaws.transcribe#Transcribe
Issue-Key: transcribe|DeleteVocabulary|2xx->4xx|$
Issue-Key: transcribe|DeleteVocabulary|2xx->4xx|status
Issue-Key: transcribe|ListTagsForResource|2xx->2xx|$.Tags
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# Transcribe vocabulary and tag-set responses drop state AWS reports

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteVocabulary

## Description
This issue bundles TWO causes in the vocabulary/tagging state of this service. A fixer must close both before submitting.

Cause 1 of 2: DeleteVocabulary cannot find a vocabulary the same fixture established, and answers not-found instead of deleting it.
AWS: HTTP 200 with an empty JSON body `{}` — the vocabulary exists and the delete succeeds (recorded response). Smithy: DeleteVocabulary output shape Unit; errors BadRequestException/NotFoundException only, so a 200 here is AWS asserting the record was present.
Emulator: HTTP 404 with body {"__type": "NotFoundException", "Message": "The requested vocabulary <name> couldn't be found. Check the vocabulary name and try your request again."} — i.e. services/service-com-amazonaws-transcribe-transcribe/src/ops/delete_vocabulary.rs lines 185-194 found nothing in `store.vocabularies` for that name. (Note the status/shape of that 404 is separately wrong for this protocol; that aspect is covered by the bundled issue 'Transcribe client errors answer the modeled @httpError instead of the HTTP 400 AWS returns'. What THIS cause is about is that the lookup missed at all, where AWS answers 200.) The record is genuinely absent rather than merely non-deletable: a stored-but-PENDING vocabulary would have taken the state branch at lines 200-213 and answered BadRequestException, not NotFoundException.
Hypothesis: the create that established this vocabulary supplied VocabularyFileUri rather than inline Phrases, which this emulator refuses outright instead of recording (create_vocabulary.rs lines 248-254, the ContentSource::File branch: "The VocabularyFileUri <uri> can't be read by this emulator"), leaving nothing for the later steps to find; the same refusal exists on the update path (update_vocabulary.rs lines 236-242). AWS accepts such a create and builds the vocabulary asynchronously.
Why: the whole custom-vocabulary lifecycle breaks at the delete — a caller that creates, uses and tears down a vocabulary gets a hard not-found on teardown, leaks the resource in its own bookkeeping, and cannot distinguish a real missing-resource case from this one.
Fix: the vocabulary family's state and the CreateVocabulary/UpdateVocabulary content-source handling in services/service-com-amazonaws-transcribe-transcribe/src/{state.rs,ops/create_vocabulary.rs,ops/delete_vocabulary.rs} — a vocabulary AWS accepts must be recorded and must stay reachable by name until deleted.
Traces: transcribe-vocabulary-lifecycle#task#5.

Cause 2 of 2: ListTagsForResource omits the Tags member for a resource whose tags were all removed, instead of reporting an empty list.
AWS: HTTP 200 with `"Tags": []` present (recorded response, after the preceding UntagResource removed the last tag).
Emulator: HTTP 200 with Tags absent. src/ops/list_tags_for_resource.rs line 203: `tags: (!tags.is_empty()).then(|| tags.clone())`, with the comment that TagList is modelled @length(min 1) so an empty set is reported by leaving the member out. The model constraint governs INPUT tag lists; the recording shows the service itself returns an empty array on output, so the recorded behavior is the requirement here.
Why: a client that reads `response.Tags.length` (or iterates it without a null check) throws on a resource whose tags were all removed, and 'member absent' is indistinguishable from 'tags unknown' — the empty list is the signal that the untag took effect.
Fix: ListTagsForResource's output projection/serializer — emit Tags as an empty list when the resource exists and holds no tags.
Traces: transcribe-tag-resource-list-untag-roundtrip#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1, 2 divergence clusters:
- DeleteVocabulary (transcribe-vocabulary-lifecycle#task#5): expected HTTP 200 with body {}; got HTTP 404 with {'__type': 'NotFoundException', 'Message': "The requested vocabulary <probe-name> couldn't be found. Check the vocabulary name and try your request again."}. Earlier in the same fixture the vocabulary was established and AWS could delete it; the emulator's store holds no such key (delete_vocabulary.rs:185-194), and a PENDING record would instead have produced BadRequestException (delete_vocabulary.rs:200-213).
- ListTagsForResource (transcribe-tag-resource-list-untag-roundtrip#task#5): expected 200 with $.Tags == []; got $.Tags missing (None). Code: list_tags_for_resource.rs:198-204.
Replay summary for context: emulator_up true, state_reset_failures 0, so neither divergence is an artifact of leaked state between fixtures.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Fixture transcribe-vocabulary-lifecycle: run it in order to step task#5, which calls DeleteVocabulary for the vocabulary the fixture established earlier. The emulator answers HTTP 404 NotFoundException; the recording is HTTP 200 with an empty body.
3. Fixture transcribe-tag-resource-list-untag-roundtrip: at task#3/task#4 the fixture tags a resource and then untags the last tag; at task#5 it calls ListTagsForResource for the same ARN. The emulator omits the Tags member; the recording carries "Tags": [].
