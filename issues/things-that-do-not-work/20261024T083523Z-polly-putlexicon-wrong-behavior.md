Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: cdf8bb57609ff190f9228b1bff313b67e030c44d

# Lexicon store is not Region-scoped: a lexicon put in one Region is readable, listable and deletable from every other Region

_Filed: 2026-10-24T08:35:23Z_
_Operation: PutLexicon | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `cdf8bb57609ff190f9228b1bff313b67e030c44d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Amazon Polly lexicons are a REGIONAL resource: a lexicon stored in us-east-1 does not exist in eu-west-1, where GetLexicon/DeleteLexicon answer LexiconNotFoundException and ListLexicons omits it.

Expected: PutLexicon(Name="Shared") signed for us-east-1, followed by GetLexicon(Name="Shared") signed for eu-west-1, answers 404 LexiconNotFoundException ("Lexicon not found"); ListLexicons in eu-west-1 reports nothing; the 100-lexicon count quota is counted per Region; and the LexiconArn a read returns always names the Region the read was made in.

Actual: `ComAmazonawsPollyParrotV1State::lexicons` is a single `BTreeMap<String, StoredLexicon>` keyed by lexicon NAME only (src/state.rs). PutLexicon reads the request's region solely to mint the ARN (`crate::state::request_region(ctx.parts())`, src/ops/put_lexicon.rs line 416) and then inserts under the bare name (line 447). So GetLexicon, ListLexicons, DeleteLexicon and SynthesizeSpeech in ANY Region see every lexicon ever put, a cross-Region GetLexicon returns 200 with a LexiconArn that names a DIFFERENT Region than the caller's (an internally inconsistent response real Polly never produces), a cross-Region DeleteLexicon destroys another Region's lexicon, and the MAX_LEXICONS=100 cap (line 438) is global instead of per Region.

## Evidence

Spec grounding: the model's own `LexiconNotFoundException` documentation states the error is caused by "specifying a lexicon that is in a different region. Verify that the lexicon exists, is in the region (see ListLexicons)" — i.e. the lexicon namespace is per Region; `LexiconAttributes$LexiconArn` carries the Region, and PutLexicon is documented as overwriting "a lexicon with the same name … in the region". Handler/state: src/state.rs `lexicons: BTreeMap<String, StoredLexicon>` (no region in the key or value beyond the ARN string); src/ops/put_lexicon.rs lines 416-447; src/ops/get_lexicon.rs line 153, src/ops/list_lexicons.rs lines 168-180, src/ops/delete_lexicon.rs line 147 all look up by bare name. Other crates in this repo do scope by Region (e.g. services/service-com-amazonaws-sfn-awsstepfunctions/src/state.rs `state_machines_in_region(region)`), so the engine does not partition state for you. Test coverage: tests/all/lexicons_and_speech.rs asserts only that the ARN is minted for the request's Region (line 185-189); no test puts in one Region and reads from another, so the gap is uncovered and un-entrenched.

## Steps to Reproduce

1. Boot the emulator and PUT /v1/lexicons/Shared with a valid PLS document, signing the request with a us-east-1 SigV4 credential scope
2. Call GET /v1/lexicons/Shared with a eu-west-1 credential scope — observe 200 and a body whose LexiconAttributes.LexiconArn is arn:aws:polly:us-east-1:...:lexicon/Shared, where real Polly answers 404 LexiconNotFoundException ("Lexicon not found")
3. Call GET /v1/lexicons with the eu-west-1 scope — observe the us-east-1 lexicon listed
4. Call DELETE /v1/lexicons/Shared with the eu-west-1 scope — observe 200 and then observe the lexicon is gone for the us-east-1 caller too
5. Store 100 lexicons across two Regions and observe MaxLexiconsNumberExceededException, where real Polly allows 100 per Region
