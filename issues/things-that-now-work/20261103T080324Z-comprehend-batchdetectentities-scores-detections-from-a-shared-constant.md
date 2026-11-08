Issue-Source: trace-sweep-divergence
Issue-Trace: comprehend-batch-detect-suite
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: comprehend-batch-detect-suite
Issue-Shape-Id: com.amazonaws.comprehend#Comprehend_20171127
Issue-Key: comprehend|BatchDetectEntities|2xx->2xx|$.ResultList[*].Entities[*].Score
Issue-Commit: 1c0413a0db44f6903ae94b4a2eeb83f1af34b87e

# Comprehend BatchDetectEntities scores detections from a shared constant ladder

> _Note: this issue was identified against the code base at commit `1c0413a0db44f6903ae94b4a2eeb83f1af34b87e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchDetectEntities

## Description
ONE net-new root cause in the comprehend crate's BatchDetectEntities handler. The step answers HTTP 200 as recorded and the entity spans and types now agree with the recording; what still diverges is Entity.Score, which the emulator draws from a small fixed ladder of constants shared with the other entity detectors instead of computing a per-detection confidence. Each ResultList entry is per-document, so the cause reproduces for any document whose entities AWS scored at distinct confidences.

NOTE on prior art: 20261030T053310Z-comprehend-batchdetectentities-truncates-entity-spans-and-emits-bucketed.md is RESOLVED but this step still diverges on replay, and the replay report shows no open issue owning it (owned_by is empty for comprehend-batch-detect-suite#task#2), so this is filed net-new (effectively a reopen of the score half of that issue). The open issue 20261030T173304Z-comprehend-batch-and-targeted-inference-re-segment-text.md covers the SAME ladder-score fault in BatchDetectDominantLanguage, BatchDetectKeyPhrases, BatchDetectSentiment, BatchDetectSyntax and DetectTargetedSentiment, but it does not carry BatchDetectEntities or its replay key.

Cause 1 of 1: BatchDetectEntities returns bucketed, non-calibrated Entity.Score values. Fixture comprehend-batch-detect-suite, step task#2.
  AWS: Smithy: BatchDetectEntities(TextList required @length min 1 sensitive, LanguageCode required) -> ResultList: list of BatchDetectEntitiesItemResult{Index, Entities: list of Entity}, ErrorList; errors BatchSizeLimitExceededException/InvalidRequestException/TextSizeLimitExceededException/UnsupportedLanguageException (400), InternalServerException (500). Entity.Score is documented as "The level of confidence that Amazon Comprehend has in the accuracy of the detection" and Entity.Type targets EntityType (PERSON, LOCATION, ORGANIZATION, COMMERCIAL_ITEM, EVENT, DATE, QUANTITY, TITLE, OTHER). Recorded behavior: every detected entity carries its own confidence, derived from the evidence for that particular mention, so two entities of the same document and the same type come back at different values near but below 1.0.
  Emulator: Score is assigned per entity TYPE/shape rather than per detection, from a handful of recurring constants: for document 2 both of the recorded confidences are replaced by ladder values (one entity gets 0.9793814420700073, the next 0.963302731513977), and 0.963302731513977 is the very same constant the emulator also returns for unrelated detections elsewhere in the corpus (DetectEntities and BatchDetectKeyPhrases steps of this corpus return it too), which is what identifies it as a constant of the detector rather than a function of the input.
  Why: a client uses Score to decide whether to accept a detection - threshold low-confidence entities out, rank candidate mentions, or route an ambiguous mention to review. With scores snapped onto a shared ladder, the relative ordering of two entities in one document is destroyed and a threshold either admits every mention or none, so the confidence-gated branch of the client is never exercised and never behaves as it does against AWS.
  Fix: the BatchDetectEntities handler's entity-scoring step in the comprehend crate's synchronous text-analysis layer - compute one confidence per detected mention from that detection's own evidence instead of looking the value up per entity type/shape in a fixed table.
  Traces: comprehend-batch-detect-suite#task#2.

The recorded numbers below are this recording's values for its own input text; the requirement is the convention - a distinct, per-detection confidence in [0,1] for each Entity - not these literals.

## Evidence
Step comprehend-batch-detect-suite#task#2, BatchDetectEntities (three-document TextList, LanguageCode en): HTTP 200 expected, HTTP 200 returned; the divergence is entirely in the response body, and only in Entity.Score.

  $.ResultList[2].Entities[0].Score: expected 0.9917884469032288, got 0.9793814420700073
  $.ResultList[2].Entities[1].Score: expected 0.9983819723129272, got 0.963302731513977

All other members of every ResultList entry (Index, Entities[*].Text, Type, BeginOffset, EndOffset) match the recording, so segmentation and typing are correct and the scorer alone diverges.

Corroboration that the returned values are detector constants and not computed confidences: the identical value 0.963302731513977 is also what the emulator returns for $.Entities[3].Score of comprehend-detect-suite-single-document#task#2 (DetectEntities) against a different recorded value, i.e. the same constant is reused across operations and inputs.

## Steps to Reproduce
1. Boot the emulator from the checkout and replay the wire corpus (replay_wire_corpus). Step comprehend-batch-detect-suite#task#2 is neither blocked by a stub nor owned by an open issue.
2. Fixture comprehend-batch-detect-suite, step task#2 - BatchDetectEntities with LanguageCode 'en' and the three-document TextList, the third document containing two named entities that AWS scores at two different confidences.
3. Inspect $.ResultList[2].Entities[*].Score: both entities come back at ladder values (0.9793814420700073 and 0.963302731513977) instead of the recorded per-detection confidences (0.9917884469032288 and 0.9983819723129272), and the ordering of the two detections by confidence is inverted relative to the recording.
4. Replay comprehend-detect-suite-single-document#task#2 (DetectEntities) and observe the same constant 0.963302731513977 returned for an unrelated entity, confirming the value comes from a fixed table rather than from the detection.
