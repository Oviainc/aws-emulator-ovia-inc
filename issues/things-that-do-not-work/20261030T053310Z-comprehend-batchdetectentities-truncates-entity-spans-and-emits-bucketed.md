Issue-Source: trace-sweep-divergence
Issue-Trace: comprehend-batch-detect-suite
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: comprehend-batch-detect-suite
Issue-Shape-Id: com.amazonaws.comprehend#Comprehend_20171127
Issue-Key: comprehend|BatchDetectEntities|2xx->2xx|$.ResultList[*].Entities[*].BeginOffset
Issue-Key: comprehend|BatchDetectEntities|2xx->2xx|$.ResultList[*].Entities[*].Score
Issue-Key: comprehend|BatchDetectEntities|2xx->2xx|$.ResultList[*].Entities[*].Text
Issue-Commit: 2d11c019ca89f17fce399c4a7a8c59c866e2a978

# Comprehend BatchDetectEntities truncates entity spans and emits bucketed confidence scores

> _Note: this issue was identified against the code base at commit `2d11c019ca89f17fce399c4a7a8c59c866e2a978`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchDetectEntities

## Description
One net-new root cause in the BatchDetectEntities analyser: the per-document entity recogniser clips multi-token entity mentions to their head token and reports confidence from a small fixed ladder of constants instead of a calibrated per-detection score. Each ResultList entry is per-document, so the defect reproduces for every document in the batch that contains a multi-token entity.

Cause 1 of 1: BatchDetectEntities returns a shortened entity span (wrong Text and BeginOffset) and a bucketed Score.
  AWS: Smithy: BatchDetectEntities output ResultList of BatchDetectEntitiesItemResult{Index, Entities: list of Entity}; Entity.Text is documented as "The text of the entity" (@length min 1), Entity.BeginOffset/EndOffset as "the zero-based offset from the beginning of the source text to the first/last character in the entity", and Entity.Score as "the level of confidence that Amazon Comprehend has in the accuracy of the detection". Recorded behavior: a DATE entity is returned as the WHOLE date expression including its leading modifier, with BeginOffset at the first character of that whole expression, and Score is a calibrated value that differs per detection (never one of a handful of repeated constants).
  Emulator: it emits only the head token of the mention, so Text is the bare weekday instead of the full date expression and BeginOffset is advanced past the dropped modifier (EndOffset is unchanged, so the reported span is strictly shorter than AWS's), and Score comes from a fixed ladder of constants (0.99, 0.95, ... as float32 values such as 0.9900000095367432 and 0.949999988079071) rather than from the detection itself.
  Why: BeginOffset/EndOffset are what a client uses to highlight or redact the mention in the source document; with a truncated span the client highlights/redacts only part of the entity and leaves the modifier in place, and the Text it stores does not match what AWS extracted. A client thresholding on Score cannot rank or filter detections, because every detection of a given type reports the same constant.
  Fix: the BatchDetectEntities handler's entity recogniser — mention-span extraction (include the full entity mention, and set BeginOffset to the first character of the mention) and score computation (a per-detection calibrated score, not a per-type constant).
  Traces: comprehend-batch-detect-suite#task#2.

Related open issue: 20261029T172519Z-comprehend-batch-synchronous-inference-returns-uncalibrated-scores-and.md carries the same family of defects for BatchDetectDominantLanguage, BatchDetectKeyPhrases, BatchDetectSentiment and BatchDetectSyntax, but it does NOT cover BatchDetectEntities (no Issue-Key of its own for this operation and no replay key for task#2), so this cause is filed separately.

## Evidence
Fixture comprehend-batch-detect-suite, step task#2 (BatchDetectEntities, three-document TextList), HTTP 200 expected and 200 returned, body diverges:
  $.ResultList[2].Entities[0].Text: expected 'next Friday', got 'Friday'
  $.ResultList[2].Entities[0].BeginOffset: expected 28, got 33 (EndOffset unchanged, so the emulator's span covers only the trailing token)
  $.ResultList[2].Entities[0].Score: expected 0.9917884469032288, got 0.9900000095367432
  $.ResultList[2].Entities[1].Score: expected 0.9983819723129272, got 0.949999988079071
The recorded offsets, texts and numeric scores are this recording's values for its own input documents; the requirement is the convention — the span must cover the complete entity mention with BeginOffset at its first character, and Score must be a per-detection calibrated confidence rather than a repeated constant — not these literals.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the wire corpus (replay_wire_corpus).
2. Fixture comprehend-batch-detect-suite, step task#2: BatchDetectEntities with LanguageCode 'en' and a three-document TextList, one document of which contains a multi-token date expression (a modifier plus a weekday).
3. Inspect ResultList[2].Entities: the emulator reports only the weekday token with BeginOffset moved forward past the modifier while EndOffset is unchanged, and the Score values repeat the same constants across detections instead of matching the recorded calibrated confidences.
