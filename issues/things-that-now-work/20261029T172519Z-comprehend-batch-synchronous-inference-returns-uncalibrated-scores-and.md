Issue-Source: trace-sweep-divergence
Issue-Trace: comprehend-batch-detect-suite
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: comprehend-batch-detect-suite
Issue-Shape-Id: com.amazonaws.comprehend#Comprehend_20171127
Issue-Key: comprehend|BatchDetectDominantLanguage|2xx->2xx|$.ResultList[*].Languages[*]
Issue-Key: comprehend|BatchDetectDominantLanguage|2xx->2xx|$.ResultList[*].Languages[*].Score
Issue-Key: comprehend|BatchDetectKeyPhrases|2xx->2xx|$.ResultList[*].KeyPhrases[*]
Issue-Key: comprehend|BatchDetectKeyPhrases|2xx->2xx|$.ResultList[*].KeyPhrases[*].BeginOffset
Issue-Key: comprehend|BatchDetectKeyPhrases|2xx->2xx|$.ResultList[*].KeyPhrases[*].EndOffset
Issue-Key: comprehend|BatchDetectKeyPhrases|2xx->2xx|$.ResultList[*].KeyPhrases[*].Score
Issue-Key: comprehend|BatchDetectKeyPhrases|2xx->2xx|$.ResultList[*].KeyPhrases[*].Text
Issue-Key: comprehend|BatchDetectSentiment|2xx->2xx|$.ResultList[*].Sentiment
Issue-Key: comprehend|BatchDetectSentiment|2xx->2xx|$.ResultList[*].SentimentScore.Mixed
Issue-Key: comprehend|BatchDetectSentiment|2xx->2xx|$.ResultList[*].SentimentScore.Negative
Issue-Key: comprehend|BatchDetectSentiment|2xx->2xx|$.ResultList[*].SentimentScore.Neutral
Issue-Key: comprehend|BatchDetectSentiment|2xx->2xx|$.ResultList[*].SentimentScore.Positive
Issue-Key: comprehend|BatchDetectSyntax|2xx->2xx|$.ResultList[*].SyntaxTokens[*].PartOfSpeech.Score
Issue-Key: comprehend|BatchDetectSyntax|2xx->2xx|$.ResultList[*].SyntaxTokens[*].PartOfSpeech.Tag
Issue-Commit: 7b78f885090352093daa8ac758bd09ec0b97c628

# Comprehend batch synchronous inference returns uncalibrated scores and wrong segmentation across all four BatchDetect operations

> _Note: this issue was identified against the code base at commit `7b78f885090352093daa8ac758bd09ec0b97c628`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchDetectDominantLanguage

## Description
Four net-new root causes, one per BatchDetect operation. The emulator's batch analysers are placeholder heuristics: they emit confidence scores drawn from a small set of hard-coded constants (1.0, 0.99, 0.85, 0.6, 0.25, 0.0) instead of calibrated per-document scores, they do not reproduce AWS's candidate-list or span conventions, and in two cases they choose a different class/tag outright. Each ResultList entry is per-document, so the defects reproduce for every document in the batch.

Cause 1 of 4: BatchDetectDominantLanguage emits a one-element Languages list with a saturated score, where AWS returns one ranked candidate list per document.
  AWS: Smithy: BatchDetectDominantLanguage output ResultList of BatchDetectDominantLanguageItemResult, each with Languages (list of DominantLanguage{LanguageCode, Score:Float}); recorded behavior shows, per document, the detected language first with a calibrated Score strictly below 1.0 and NO additional candidates in the recorded responses for these inputs.
  Emulator: Score pinned to 1.0 (or 0.9999990463256836) for the top language, plus five extra candidate entries (de/fr/it/es/pt) with vanishing scores that AWS does not serve for these documents.
  Why: a client thresholding on Score can never distinguish a confident detection from a guess (everything is 1.0), and a client that reads Languages[1] as "second-best candidate" sees candidates AWS did not report.
  Fix: the BatchDetectDominantLanguage handler's analyser — candidate selection and score calibration.
  Traces: comprehend-batch-detect-suite#task#1.

Cause 2 of 4: BatchDetectKeyPhrases extracts single tokens rather than noun phrases, so Text, BeginOffset, EndOffset and the phrase count are all wrong.
  AWS: Smithy: BatchDetectKeyPhrases output ResultList of BatchDetectKeyPhrasesItemResult with KeyPhrases (list of KeyPhrase{Score, Text, BeginOffset, EndOffset}); recorded behavior: a key phrase is a full noun phrase INCLUDING its determiner, its offsets span that phrase in the input text, and its Score is calibrated.
  Emulator: it strips the determiner and splits phrases into bare head words, so offsets start inside the phrase, extra per-word entries appear, and Score is a constant (1.0, or 0.25 for low-confidence picks).
  Why: offsets are used to highlight spans in the source document; a client highlighting the emulator's spans marks the wrong text, and the phrase set it summarises does not match the real service.
  Fix: the BatchDetectKeyPhrases handler's phrase-extraction and scoring.
  Traces: comprehend-batch-detect-suite#task#3.

Cause 3 of 4: BatchDetectSentiment picks the wrong Sentiment class and emits degenerate SentimentScore distributions.
  AWS: Smithy: BatchDetectSentiment output ResultList entries with Sentiment (SentimentType enum POSITIVE|NEGATIVE|NEUTRAL|MIXED) and SentimentScore{Positive, Negative, Neutral, Mixed}; recorded behavior: the four scores are a calibrated distribution and Sentiment is the argmax, with MIXED chosen for a document carrying both polarities.
  Emulator: a document the recording classifies MIXED is classified POSITIVE, and the score vectors are coarse (components pinned to exactly 0.0, or split evenly between two components), so MIXED is effectively never reachable.
  Why: a client that routes on Sentiment routes a mixed-polarity document as positive; a client thresholding on SentimentScore sees zeros where AWS reports small non-zero confidences.
  Fix: the BatchDetectSentiment handler's classifier and score normalisation.
  Traces: comprehend-batch-detect-suite#task#4.

Cause 4 of 4: BatchDetectSyntax assigns the wrong part-of-speech tag and bucketed PartOfSpeech.Score.
  AWS: Smithy: BatchDetectSyntax output ResultList entries with SyntaxTokens (list of SyntaxToken{TokenId, Text, BeginOffset, EndOffset, PartOfSpeech:PartOfSpeechTag{Tag:PartOfSpeechTagType, Score}}); recorded behavior: Scores are calibrated and near 1.0 for unambiguous tokens, and auxiliary/copular verbs are tagged VERB.
  Emulator: Scores come from a fixed ladder (0.99 / 0.85 / 0.6) and tags diverge — copulas are tagged AUX where AWS tags VERB, and adjectives are tagged VERB.
  Why: any consumer doing POS-based filtering (e.g. keeping adjectives) gets a different token set from the real service, and Score carries no information.
  Fix: the BatchDetectSyntax handler's tagger and its tag-set mapping onto PartOfSpeechTagType.
  Traces: comprehend-batch-detect-suite#task#5.

## Evidence
Cause 1 (task#1): $.ResultList[0].Languages[0].Score expected 0.9994072318077089, got 1.0; $.ResultList[0].Languages[1..5] extra (de/fr/it/es/pt with ~1e-10..1e-16 scores) where the recording has none; same pattern for ResultList[1] and ResultList[2].
Cause 2 (task#3): $.ResultList[0].KeyPhrases[0].Text expected 'The flight', got 'flight'; BeginOffset expected 0, got 4; KeyPhrases[1].Text expected 'the staff', got 'delayed'; KeyPhrases[2] and [3] extra single-word phrases; Score expected 0.999966561794281, got 1.0; ResultList[2].KeyPhrases[1].Score expected 0.9115143418312072, got 0.25.
Cause 3 (task#4): $.ResultList[0].Sentiment expected 'MIXED', got 'POSITIVE'; SentimentScore.Mixed expected 0.893014132976532, got 0.23868313431739807; SentimentScore.Negative expected 0.0015593204880133271, got 0.0; ResultList[1].SentimentScore.Negative expected 0.9998328685760498, got 0.5256301164627075.
Cause 4 (task#5): $.ResultList[0].SyntaxTokens[7].PartOfSpeech.Tag expected 'VERB', got 'AUX'; ResultList[1].SyntaxTokens[3].PartOfSpeech.Tag expected 'ADJ', got 'VERB'; PartOfSpeech.Score expected 1.0 / 0.9999988079071044, got 0.9900000095367432 / 0.8500000238418579 / 0.6000000238418579.
(The recorded numeric scores are the recording's values for its own documents; the requirement is the convention — calibrated per-document scores, AWS's span and candidate conventions, and AWS's tag choices — not these literals.)

## Steps to Reproduce
1. Boot the emulator from mainline and replay the wire corpus (replay_wire_corpus).
2. Fixture comprehend-batch-detect-suite: step task#1 (BatchDetectDominantLanguage), task#3 (BatchDetectKeyPhrases), task#4 (BatchDetectSentiment), task#5 (BatchDetectSyntax), each with a three-document TextList.
3. Compare each ResultList entry against the recorded response: scores are constants, key-phrase spans exclude determiners, a MIXED document comes back POSITIVE, and copulas are tagged AUX.
