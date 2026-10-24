Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: b7068a3aaca27128a9512527e96d46285251fa2e

# ListLexicons truncates at an emulator-chosen 20-lexicon page instead of returning the Region's lexicons in one response

_Filed: 2026-10-24T03:33:51Z_
_Operation: ListLexicons | Class: pagination_

> _Note: this issue was identified against the code base at commit `b7068a3aaca27128a9512527e96d46285251fa2e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ListLexicons caps each response at 20 lexicon descriptions (LEXICON_PAGE_SIZE, documented in the handler as the emulator's own choice) and mints a NextToken beyond that. Amazon Polly stores at most 100 lexicons per Region, the operation is not marked @paginated in the model (only ListSpeechSynthesisTasks is) and the AWS SDKs ship no paginator for it, so a single ListLexicons call returns the Region's whole lexicon list.

Expected: after storing e.g. 25 lexicons, one ListLexicons call reports all 25 and no NextToken.
Actual: it reports the first 20 by name order plus a NextToken, so a client that does not loop concludes five of its lexicons do not exist (and a subsequent GetLexicon/SynthesizeSpeech on an unreported name then succeeds, contradicting the list).

## Evidence

Smithy model: OPERATIONS.md shows ListLexicons with only NextToken in/out and NO @paginated trait, while ListSpeechSynthesisTasks carries @paginated(inputToken=NextToken, outputToken=NextToken) — the modeled difference between the service's one paginated read and this one. Handler: src/ops/list_lexicons.rs line 113 `pub const LEXICON_PAGE_SIZE: usize = 20;` whose doc states "CHOSEN: Amazon Polly documents NextToken on this operation but no page size, so this is the emulator's own", and lines 173-187 truncate the page and mint the token. Quota context: src/ops/put_lexicon.rs MAX_LEXICONS = 100, so a whole Region's lexicons fit in one realistic response. Tests: tests/all/lexicons_and_speech.rs exercises the lexicon list but no test asserts that a store of >20 lexicons is reported in one page, so the gap is uncovered.

## Steps to Reproduce

1. Boot the emulator and PutLexicon 25 distinct valid PLS lexicons (names Lex0001..Lex0025)
2. Call GET /v1/lexicons once (botocore: polly.list_lexicons())
3. Observe 20 entries and a NextToken, where real Polly returns all 25 entries and no NextToken
4. GetLexicon on one of the unreported names (Lex0025) and observe 200, contradicting the truncated list
