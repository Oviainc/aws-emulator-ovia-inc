Issue-Source: trace-sweep-divergence
Issue-Trace: comprehend-batch-detect-suite
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: comprehend-batch-detect-suite comprehend-detect-suite-single-document
Issue-Shape-Id: com.amazonaws.comprehend#Comprehend_20171127
Issue-Key: comprehend|BatchDetectKeyPhrases|2xx->2xx|$.ResultList[*].KeyPhrases[*]
Issue-Key: comprehend|BatchDetectKeyPhrases|2xx->2xx|$.ResultList[*].KeyPhrases[*].BeginOffset
Issue-Key: comprehend|BatchDetectKeyPhrases|2xx->2xx|$.ResultList[*].KeyPhrases[*].EndOffset
Issue-Key: comprehend|BatchDetectKeyPhrases|2xx->2xx|$.ResultList[*].KeyPhrases[*].Score
Issue-Key: comprehend|BatchDetectKeyPhrases|2xx->2xx|$.ResultList[*].KeyPhrases[*].Text
Issue-Key: comprehend|BatchDetectDominantLanguage|2xx->2xx|$.ResultList[*].Languages[*].Score
Issue-Key: comprehend|BatchDetectSentiment|2xx->2xx|$.ResultList[*].SentimentScore.Mixed
Issue-Key: comprehend|BatchDetectSentiment|2xx->2xx|$.ResultList[*].SentimentScore.Negative
Issue-Key: comprehend|BatchDetectSentiment|2xx->2xx|$.ResultList[*].SentimentScore.Neutral
Issue-Key: comprehend|BatchDetectSentiment|2xx->2xx|$.ResultList[*].SentimentScore.Positive
Issue-Key: comprehend|BatchDetectSyntax|2xx->2xx|$.ResultList[*].SyntaxTokens[*].PartOfSpeech.Score
Issue-Key: comprehend|BatchDetectSyntax|2xx->2xx|$.ResultList[*].SyntaxTokens[*].PartOfSpeech.Tag
Issue-Key: comprehend|DetectTargetedSentiment|2xx->2xx|$.Entities[*].Mentions[*].MentionSentiment.Sentiment
Issue-Key: comprehend|DetectTargetedSentiment|2xx->2xx|$.Entities[*].Mentions[*].MentionSentiment.SentimentScore.Mixed
Issue-Related: 20261029T172519Z-comprehend-single-document-synchronous-inference-omits-output-members.md
Issue-Commit: fdbc3a0ffd43a28553464e8afd6021169d0df514

# Comprehend batch and targeted inference re-segment text and emit uncalibrated ladder scores (5 causes)

> _Note: this issue was identified against the code base at commit `fdbc3a0ffd43a28553464e8afd6021169d0df514`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchDetectKeyPhrases

## Description
FIVE net-new root causes, all in the comprehend crate's synchronous text-analysis layer that the BatchDetect* fan-out and DetectTargetedSentiment share: the analyser re-segments the input (key-phrase chunking, co-reference grouping) and draws every confidence value from a small fixed ladder of constants instead of a per-detection calibrated score. Each ResultList entry is per-document, so each cause reproduces for every document of the batch. All five steps answered HTTP 200 as recorded; only the bodies diverge. A fixer must close all FIVE causes.

NOTE on prior art: 20261029T172519Z-comprehend-batch-synchronous-inference-returns-uncalibrated-scores-and.md is RESOLVED but these four BatchDetect steps still diverge on replay (none of them is owned by an open issue), so causes 1-4 are filed net-new (effectively a reopen). 20261030T053310Z-comprehend-batchdetectentities-truncates-entity-spans-and-emits-bucketed.md covers only BatchDetectEntities (task#2), and 20261029T172519Z-comprehend-single-document-synchronous-inference-omits-output-members.md carries other steps of comprehend-detect-suite-single-document but NOT task#7, so cause 5 is filed here.

Cause 1 of 5: BatchDetectKeyPhrases drops a leading key phrase, merges a modifier into the next phrase, and repeats ladder scores. Fixture comprehend-batch-detect-suite, step task#3.
  AWS: Smithy: BatchDetectKeyPhrases(TextList required, LanguageCode required) -> ResultList of BatchDetectKeyPhrasesItemResult{Index, KeyPhrases: list of KeyPhrase}; KeyPhrase.Text is documented as "The text of a key noun phrase" (@length min 1), BeginOffset/EndOffset as "the zero-based offset from the beginning of the source text to the first/last character in the key phrase", Score as "the level of confidence that Amazon Comprehend has in the accuracy of the detection". Recorded behavior: for a document containing a leading noun phrase, a date expression and a place name, AWS returns THREE key phrases, one per noun phrase, each span bounded exactly by the noun phrase (the determiner-led subject phrase included, the adjunct word before the date expression excluded), and three distinct calibrated Scores.
  Emulator: it returns only TWO key phrases for that document: the subject noun phrase is missing entirely, the first returned phrase starts inside the sentence and swallows the word preceding the date expression (Text 'due next Friday' at BeginOffset 24 where AWS has 'The quarterly report' at 0 and 'next Friday' at 28), the remaining phrase is shifted into that slot, and the recorded third element is absent. Scores repeat across detections (the same value 0.9999399781227112 for two different phrases of document 0).
  Why: a client uses KeyPhrases plus the offsets to index, highlight or summarise the source document. A missing phrase means a topic the document is about is never surfaced; a merged span highlights text that is not part of the noun phrase; and a Score that repeats makes ranking or thresholding key phrases impossible.
  Fix: the BatchDetectKeyPhrases handler's noun-phrase chunker (phrase boundary detection, so every noun phrase of the document is emitted with the span of exactly that phrase) and its score computation (a per-detection confidence rather than a per-shape constant).
  Traces: comprehend-batch-detect-suite#task#3.

Cause 2 of 5: BatchDetectDominantLanguage reports a near-constant confidence that does not track the document's language evidence. Fixture comprehend-batch-detect-suite, step task#1.
  AWS: Smithy: BatchDetectDominantLanguage -> ResultList of BatchDetectDominantLanguageItemResult{Index, Languages: list of DominantLanguage{LanguageCode, Score}}; DominantLanguage.Score is "the level of confidence that Amazon Comprehend has in the accuracy of the detection". Recorded behavior: the Score varies materially per document — a long unambiguous document scores ~0.999 while a short or lexically ambiguous document scores markedly lower (~0.97) for the same detected language code.
  Emulator: every document in the batch gets essentially the same high confidence regardless of how much evidence the text carries; the document AWS scored ~0.973 is reported at ~0.999, i.e. the emulator's score is a constant of the detector rather than a function of the input.
  Why: clients gate on this Score to decide whether to trust the detected language (route for translation, pick a downstream LanguageCode, or fall back to a default). With a flat near-1.0 score, a low-evidence document is never flagged and the fallback branch is unreachable.
  Fix: the BatchDetectDominantLanguage handler's scoring in the language detector — derive the confidence from the detection evidence for that document instead of a fixed constant.
  Traces: comprehend-batch-detect-suite#task#1.

Cause 3 of 5: BatchDetectSentiment's SentimentScore vector is quantized onto a constant ladder and does not concentrate on the chosen sentiment. Fixture comprehend-batch-detect-suite, step task#4.
  AWS: Smithy: BatchDetectSentiment -> ResultList of BatchDetectSentimentItemResult{Index, Sentiment, SentimentScore}; SentimentScore{Positive, Negative, Neutral, Mixed} members are each documented as "the level of confidence that Amazon Comprehend has in the accuracy of its detection of the <SENTIMENT> sentiment". Recorded behavior: the four values form a distribution dominated by the reported Sentiment, with the non-chosen members pushed to very small, distinct magnitudes (order 1e-05 to 1e-03 for a confident document) and with the dominant value reflecting how clear-cut the document is.
  Emulator: the non-chosen members are the SAME constant repeated across members and across documents (0.0009960159659385681 appearing as Mixed, Negative and Positive of different documents), and the dominant member is diluted: a document AWS scores Negative 0.9998 with the rest ~5e-05 comes back Negative 0.8974 with Neutral 0.1006, and a MIXED-dominant document AWS scores Mixed 0.893 comes back Mixed 0.556 / Positive 0.432.
  Why: a client thresholding sentiment ("only act above 0.95", "treat >0.1 on a second sentiment as ambiguous") gets the opposite verdict from the emulator: confident documents look ambiguous and the ladder constants make every document's residual probabilities identical, so no relative ordering survives.
  Fix: the BatchDetectSentiment handler's sentiment scorer — produce one normalised distribution per document that concentrates on the returned Sentiment, instead of assigning members from a fixed table of constants.
  Traces: comprehend-batch-detect-suite#task#4.

Cause 4 of 5: BatchDetectSyntax mis-tags auxiliaries and predicate verbs and scores every token off a fixed ladder. Fixture comprehend-batch-detect-suite, step task#5.
  AWS: Smithy: BatchDetectSyntax -> ResultList of BatchDetectSyntaxItemResult{Index, SyntaxTokens: list of SyntaxToken{TokenId, Text, BeginOffset, EndOffset, PartOfSpeech}}; PartOfSpeechTag.Tag targets PartOfSpeechTagType whose value set includes ADJ, ADP, ADV, AUX, CONJ, CCONJ, DET, INTJ, NOUN, NUM, O, PART, PRON, PROPN, PUNCT, SCONJ, SYM, VERB, and PartOfSpeechTag.Score is "the confidence that Amazon Comprehend has that the part of speech was correctly identified". Recorded behavior: a copula/auxiliary is tagged AUX and the following predicate word is tagged VERB, and per-token Scores are at or near 1.0 and differ token by token.
  Emulator: the tagger never emits AUX — the auxiliary is tagged VERB and the predicate it governs is then tagged ADJ (expected 'AUX' got 'VERB', expected 'VERB' got 'ADJ') — and PartOfSpeech.Score is drawn from a small ladder of constants that recurs across tokens and across documents (0.9999899864196777, 0.9999499917030334, 0.9090826511383057, 0.7499943971633911), including values far below AWS's for tokens AWS tags with confidence 1.0.
  Why: downstream grammar work keys on the tagset: a client extracting predicates or stripping auxiliaries sees the auxiliary as the main verb and the real verb as a modifier, which silently changes every rule written against AUX/VERB. Confidence-based filtering is also unusable, because the same handful of constants recur regardless of the token.
  Fix: the BatchDetectSyntax handler's part-of-speech tagger — the tagset mapping (emit AUX for auxiliary/copular verbs and keep the governed predicate as VERB) and its per-token confidence computation.
  Traces: comprehend-batch-detect-suite#task#5.

Cause 5 of 5: DetectTargetedSentiment groups mentions over the wrong spans, types them all OTHER, drops the last entity, and reports the wrong mention sentiment. Fixture comprehend-detect-suite-single-document, step task#7.
  AWS: Smithy: DetectTargetedSentiment(Text required, LanguageCode required) -> Entities: list of TargetedSentimentEntity{DescriptiveMentionIndex, Mentions}; TargetedSentimentMention carries Score ("model confidence that the entity is relevant", range zero to one), GroupScore, Text ("the text in the document that identifies the entity"), Type (TargetedSentimentEntityType, e.g. ORGANIZATION / PERSON), MentionSentiment{Sentiment, SentimentScore} and BeginOffset/EndOffset ("the offset into the document text where the mention begins/ends"). Recorded behavior: each entity group's mention spans the full named mention at its true offsets, Type names the real entity class, and factual mentions carry MentionSentiment NEUTRAL with SentimentScore.Neutral ~0.9999 and the other members ~1e-06 or 0.0.
  Emulator: it selects different spans altogether (a mention at BeginOffset 31/EndOffset 39 with Text 'language' where AWS has the person mention at 130-140), types mentions 'OTHER' where AWS returns ORGANIZATION and PERSON, reports MentionSentiment 'POSITIVE' with Positive ~0.987 / Neutral ~0.011 for mentions AWS reports NEUTRAL with Neutral ~0.9999, assigns Score and GroupScore from the same ladder of constants (0.8999999761581421, 0.75, 0.0009960159659385681, 0.987051784992218), and returns one entity group FEWER than recorded ($.Entities[5] missing).
  Why: targeted sentiment is consumed as "which entity, where, and how is it spoken of": wrong spans and Type point the client at common nouns instead of the named entities, a dropped group loses an entity the document discusses entirely, and flipping NEUTRAL factual mentions to POSITIVE inverts the per-entity verdict a client aggregates on.
  Fix: the DetectTargetedSentiment handler — mention detection and co-reference grouping (select named-entity mentions at their true offsets and emit one group per entity), the entity typer (real TargetedSentimentEntityType values rather than OTHER), and the mention sentiment scorer (one distribution per mention, concentrated on the reported Sentiment, not ladder constants).
  Traces: comprehend-detect-suite-single-document#task#7.

In every cause the recorded offsets, texts, tags and numeric scores are this recording's values for its own input text; the requirement is the convention — spans bounded by the segment they describe, tags from the modelled enum including AUX, one distribution per detection concentrated on the reported label, and a per-detection confidence rather than a repeated constant — not these literals.

## Evidence
All five steps: HTTP 200 expected, HTTP 200 returned; the divergence is entirely in the response body.

Cause 1 — comprehend-batch-detect-suite#task#3, BatchDetectKeyPhrases (three-document TextList, LanguageCode en):
  $.ResultList[2].KeyPhrases[0].Text: expected 'The quarterly report', got 'due next Friday'
  $.ResultList[2].KeyPhrases[0].BeginOffset: expected 0, got 24; EndOffset: expected 20, got 39
  $.ResultList[2].KeyPhrases[1].Text: expected 'next Friday', got 'Boston'; BeginOffset expected 28 got 43; EndOffset expected 39 got 49
  $.ResultList[2].KeyPhrases[2]: missing (expected {'BeginOffset': 43, 'EndOffset': 49, 'Score': 0.999948501586914, 'Text': 'Boston'}, got None)
  $.ResultList[0].KeyPhrases[0].Score: expected 0.999966561794281, got 0.9999399781227112; $.ResultList[0].KeyPhrases[1].Score: expected 0.9999895691871644, got the SAME 0.9999399781227112

Cause 2 — comprehend-batch-detect-suite#task#1, BatchDetectDominantLanguage:
  $.ResultList[0].Languages[0].Score: expected 0.9994072318077089, got 0.9989872574806213
  $.ResultList[1].Languages[0].Score: expected 0.973076343536377, got 0.9989972114562988 (AWS's markedly lower confidence reported as high)
  $.ResultList[2].Languages[0].Score: expected 0.9928982257843018, got 0.9971480965614319

Cause 3 — comprehend-batch-detect-suite#task#4, BatchDetectSentiment:
  $.ResultList[0].SentimentScore.Mixed: expected 0.893014132976532, got 0.5562507510185242; Positive expected 0.1019948422908783, got 0.4317970871925354
  $.ResultList[1].SentimentScore.Negative: expected 0.9998328685760498, got 0.8974103331565857; Neutral expected 4.58155082014855e-05, got 0.10059761255979538
  the constant 0.0009960159659385681 is returned for ResultList[0].Negative, ResultList[1].Mixed, ResultList[1].Positive, ResultList[2].Mixed, ResultList[2].Negative and ResultList[2].Positive, against six distinct recorded values (5.03e-05, 7.08e-05, 2.23e-05, 8.52e-04, 2.74e-03, 1.55e-03)

Cause 4 — comprehend-batch-detect-suite#task#5, BatchDetectSyntax:
  $.ResultList[0].SyntaxTokens[2].PartOfSpeech.Tag: expected 'AUX', got 'VERB'
  $.ResultList[0].SyntaxTokens[3].PartOfSpeech.Tag: expected 'VERB', got 'ADJ'
  $.ResultList[0].SyntaxTokens[0..8].PartOfSpeech.Score: expected 1.0 (and 0.9999988079071044) per token, got the recurring constants 0.9999899864196777 / 0.9999499917030334 / 0.7499943971633911 / 0.9090826511383057 / 0.9999666810035706; the same constants recur in ResultList[1] and ResultList[2], which also diverge on Tag

Cause 5 — comprehend-detect-suite-single-document#task#7, DetectTargetedSentiment:
  $.Entities[0].Mentions[0].Type: expected 'ORGANIZATION', got 'OTHER'; $.Entities[1].Mentions[0].Type: expected 'PERSON', got 'OTHER'
  $.Entities[1].Mentions[0].Text: expected 'John Smith', got 'language'; BeginOffset expected 130 got 31; EndOffset expected 140 got 39
  $.Entities[0].Mentions[0].MentionSentiment.Sentiment: expected 'NEUTRAL', got 'POSITIVE'; SentimentScore.Neutral expected 0.9999979734420776, got 0.01095617562532425; Positive expected 9.999999974752427e-07, got 0.987051784992218 (same for $.Entities[1])
  $.Entities[0].Mentions[0].Score: expected 0.9986129999160768, got 0.8999999761581421; $.Entities[1].Mentions[0].Score: expected 0.9999549984931946, got 0.75
  $.Entities[5]: missing (one recorded entity group absent from the response)

## Steps to Reproduce
1. Boot the emulator from the checkout and replay the wire corpus (replay_wire_corpus); no step of either fixture is blocked or owned by an open issue.
2. Cause 1: fixture comprehend-batch-detect-suite, step task#3 — BatchDetectKeyPhrases with LanguageCode 'en' and a three-document TextList, the third document containing a determiner-led subject noun phrase, a multi-word date expression preceded by an adjunct word, and a place name. Inspect ResultList[2].KeyPhrases: only two phrases come back, the first starting inside the sentence and covering the adjunct word, and the recorded third phrase is absent; ResultList[0]'s two phrases share one Score value.
3. Cause 2: same fixture, step task#1 — BatchDetectDominantLanguage over the same TextList. Compare ResultList[*].Languages[0].Score across the documents: all three sit near the same high value, including the document recorded at ~0.973.
4. Cause 3: same fixture, step task#4 — BatchDetectSentiment. Inspect each ResultList[*].SentimentScore: the non-dominant members repeat one constant across documents and the dominant member is diluted (Negative 0.8974 with Neutral 0.1006 where the recording has 0.9998 / 4.6e-05).
5. Cause 4: same fixture, step task#5 — BatchDetectSyntax. Inspect ResultList[0].SyntaxTokens: the auxiliary is tagged VERB (never AUX) and the predicate it governs is tagged ADJ, and PartOfSpeech.Score values repeat across tokens and documents.
6. Cause 5: fixture comprehend-detect-suite-single-document, step task#7 — DetectTargetedSentiment over the single document. Inspect Entities[*].Mentions[0]: the chosen spans are common nouns rather than the named mentions, every Type is OTHER, MentionSentiment is POSITIVE for factual mentions, Score/GroupScore repeat ladder constants, and one recorded entity group (Entities[5]) is missing.
