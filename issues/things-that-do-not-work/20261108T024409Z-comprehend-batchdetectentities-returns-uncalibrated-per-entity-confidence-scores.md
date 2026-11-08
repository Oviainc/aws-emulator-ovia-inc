Issue-Source: trace-sweep-divergence
Issue-Trace: comprehend-batch-detect-suite
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: comprehend-batch-detect-suite
Issue-Shape-Id: com.amazonaws.comprehend#Comprehend_20171127
Issue-Key: comprehend|BatchDetectEntities|2xx->2xx|$.ResultList[*].Entities[*].Score
Issue-Commit: 5772abe834f24ca0a7d173ddfaf967982b48def5

# Comprehend BatchDetectEntities returns uncalibrated per-entity confidence scores

> _Note: this issue was identified against the code base at commit `5772abe834f24ca0a7d173ddfaf967982b48def5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchDetectEntities

## Description
ONE root cause in the comprehend crate's synchronous entity-detection layer: BatchDetectEntities now returns the right entity spans and types, but Entity.Score is not a per-detection calibrated confidence — it is produced by the emulator's own scoring routine and lands materially below the confidence AWS reports for the same unambiguous named entity. The step answers HTTP 200 as recorded; only the body diverges.

PRIOR ART (all RESOLVED, so none of them covers this replay failure; the cluster is reported with no owning open issue): 20261030T053310Z-comprehend-batchdetectentities-truncates-entity-spans-and-emits-bucketed.md and 20261103T080324Z-comprehend-batchdetectentities-scores-detections-from-a-shared-constant.md both targeted this operation's scores, and 20261029T203023Z-comprehend-batchdetectentities-stub.md / 20261022T171625Z-comprehend-answers-http-501-notimplemented-for-batchdetectentities-the.md its implementation. The span and Type truncation those issues describe is indeed gone from this replay — ResultList[2].Entities[0] and [1] now agree with the recording on Text, Type, BeginOffset and EndOffset — but Score still diverges, so the scoring half of the fault is filed net-new (effectively a reopen limited to Score). The sibling open issue 20261030T173304Z-comprehend-batch-and-targeted-inference-re-segment-text.md describes the same class of ladder scoring for BatchDetectKeyPhrases/DominantLanguage/Sentiment/Syntax and DetectTargetedSentiment, but it does NOT carry BatchDetectEntities or this step, so it does not cover this cause.

Cause 1 of 1: BatchDetectEntities reports a per-entity confidence that does not track the detection's evidence. Fixture comprehend-batch-detect-suite, step task#2.
  AWS: Smithy: BatchDetectEntities(TextList required @length min 1, LanguageCode required) -> ResultList: ListOfDetectEntitiesResult (required) of BatchDetectEntitiesItemResult{Index, Entities: list of Entity}, plus ErrorList (required); modeled errors BatchSizeLimitExceededException (400), InvalidRequestException (400), TextSizeLimitExceededException (400), UnsupportedLanguageException (400), InternalServerException (500). Entity.Score is documented as "The level of confidence that Amazon Comprehend has in the accuracy of the detection"; Entity.Type targets the EntityType enum (PERSON, LOCATION, ORGANIZATION, COMMERCIAL_ITEM, EVENT, DATE, QUANTITY, TITLE, OTHER); Entity.Text is "the text of the entity" and BeginOffset/EndOffset are "the zero-based offset from the beginning of the source text to the first/last character in the entity". The model fixes neither the numeric value nor the ladder, so for the magnitude this is recorded behavior only: AWS scores a clearly delimited, unambiguous named entity very close to 1.0 (order 0.99x, with the residual well under one percent), and the value differs from detection to detection because it is a function of that detection's evidence.
  Emulator: for the same two entities of the same document it returns noticeably lower, differently-spaced confidences — roughly 0.98 and 0.96 against recorded 0.992 and 0.998 — so a confident detection is reported as a borderline one, and the gap is not a per-detection refinement but a systematically different scoring scale. Spans and Types match, which isolates the defect to the score computation rather than to detection or typing.
  Why: clients threshold on Entity.Score to decide whether a detected entity is trustworthy (write it to a record, redact it, route it for review, or discard it). A confidence scale that sits one to four percentage points low turns unambiguous entities into sub-threshold ones for any client gating at 0.99, so entities AWS would hand through are dropped; and because the emulator's spacing does not follow the evidence, the relative ordering clients use to pick the best of several candidate detections does not survive either.
  Fix: the BatchDetectEntities handler's score computation in the entity detector — derive each Entity.Score from that detection's own evidence so an unambiguous full-span named entity scores near 1.0, instead of emitting a value from the detector's internal scale. Detection, typing and offsets need no change in this step.
  Traces: comprehend-batch-detect-suite#task#2.

The recorded numbers above are this recording's values for its own input text. The requirement is the convention — a per-detection confidence that concentrates near 1.0 for an unambiguous named entity and varies with the evidence — not these literals; hosts, ids and timestamps are likewise not requirements.

## Evidence
Fixture comprehend-batch-detect-suite, step task#2, operation BatchDetectEntities (three-document TextList, LanguageCode en). Expected HTTP 200, returned HTTP 200 — status and error code agree, the divergence is entirely in the response body:

  $.ResultList[2].Entities[0].Score: expected 0.9917884469032288, got 0.979899525642395
  $.ResultList[2].Entities[1].Score: expected 0.9983819723129272, got 0.963302731513977

No other key of the response diverges: Text, Type, BeginOffset and EndOffset of both entities, and every entry of ResultList[0] and ResultList[1], match the recording. Every expected value is strictly above 0.99 while every returned value is below it, and the returned pair is ordered opposite to the recorded pair (the entity AWS is MORE confident about is the one the emulator scores LOWER), so the divergence is not a rounding difference.

The cluster carries no owning open issue and no stub path: replay reports it with owned_by empty, i.e. the resolved BatchDetectEntities issues no longer cover it.

## Steps to Reproduce
1. Clone the checkout and boot the emulator from it, then replay the wire corpus (replay_wire_corpus). No step of comprehend-batch-detect-suite is blocked, and task#2 is reported with an empty owned_by.
2. Call BatchDetectEntities with LanguageCode 'en' and a three-document TextList whose third document contains several clearly delimited named entities (a product/commercial item, a person name, an organization, a place).
3. Inspect $.ResultList[2].Entities[*]: Text, Type, BeginOffset and EndOffset agree with the recording, so the detections themselves are right.
4. Compare each Entity.Score against the recorded response: the emulator's values sit below 0.99 where AWS records 0.992 and 0.998, and their relative order is inverted. Any client gating entities at a 0.99 confidence threshold keeps both recorded entities but discards both emulator ones.
