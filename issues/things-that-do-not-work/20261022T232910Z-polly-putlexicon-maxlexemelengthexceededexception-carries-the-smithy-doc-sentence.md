Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: polly.put-lexicon-max-lexeme-length-exceeded-rejected
Issue-Shape-Id: com.amazonaws.polly#Parrot_v1
Issue-Key: polly.put-lexicon-max-lexeme-length-exceeded-rejected|run#8|AssertionError|assert message == "Lexeme rule length has been exceeded", message
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# Polly PutLexicon: MaxLexemeLengthExceededException carries the Smithy doc sentence instead of Polly's fixed message for that code

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutLexicon

## Description
1 cause.

Cause 1 of 1: the fixed per-code message for MaxLexemeLengthExceededException is wrong.
AWS: Smithy: PutLexicon's MaxLexemeLengthExceededException is errorKind client, httpError 400, with one optional `message` member; the model's doc text is 'The maximum size of the lexeme would be exceeded by this operation.' Polly's error messages are a fixed sentence per error code, a function of the code and of nothing in the request (this emulator's own services/service-com-amazonaws-polly-parrot-v1/src/service.rs states that rule and marks which sentences are OBSERVED from Polly's responses). Recorded behavior: for a lexeme whose alias exceeds the 100-character limit, PutLexicon answers HTTP 400 with code MaxLexemeLengthExceededException and the fixed message 'Lexeme rule length has been exceeded', and stores nothing (a following GetLexicon is LexiconNotFoundException / 'Lexicon not found').
Emulator: status and code are correct and nothing is stored, but the message is the model's documentation sentence, 'The maximum size of the lexeme would be exceeded by this operation.' — the constant MAX_LEXEME_LENGTH_EXCEEDED in service.rs, which that file flags as model-derived rather than observed, i.e. a placeholder for the unknown real sentence. The recording supplies it.
Why: this code's message is a fixed constant, so a client or test that matches Polly's wording (or a human reading an error log) sees text Polly never sends; the message is the only part of the response that distinguishes it, since the status and code already match.
Fix: the service's error-wording table (the MAX_LEXEME_LENGTH_EXCEEDED constant and its CANONICAL_MESSAGES entry), not the PutLexicon handler.
Traces: polly.put-lexicon-max-lexeme-length-exceeded-rejected.

## Evidence
polly.put-lexicon-max-lexeme-length-exceeded-rejected:
  assert message == "Lexeme rule length has been exceeded", message
  AssertionError: The maximum size of the lexeme would be exceeded by this operation.

The code assertion immediately before it passes (code == 'MaxLexemeLengthExceededException'), as does the follow-up that the rejected lexicon was not stored (GetLexicon -> LexiconNotFoundException / 'Lexicon not found') and the positive control that a 50-character alias is stored and read back byte-identical. Source: services/service-com-amazonaws-polly-parrot-v1/src/service.rs lines 195-197 and 209-238 define MAX_LEXEME_LENGTH_EXCEEDED from the model's doc text, annotated as the model's sentence rather than an observed one.

## Steps to Reproduce
1. Boot the polly emulator.
2. Call PutLexicon with a valid PLS document whose single lexeme has an <alias> of 150 'x' characters.
3. Observe HTTP 400 with code MaxLexemeLengthExceededException and message 'The maximum size of the lexeme would be exceeded by this operation.'; AWS sends 'Lexeme rule length has been exceeded'.
4. Replay polly.put-lexicon-max-lexeme-length-exceeded-rejected.
