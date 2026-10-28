Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 804429c0e5267423598a5b7dbc8d0525cc046b43

# ListLexicons honours a NextToken it can never have minted and answers a silently short list

_Filed: 2026-10-28T02:38:05Z_
_Operation: ListLexicons | Class: validation_

> _Note: this issue was identified against the code base at commit `804429c0e5267423598a5b7dbc8d0525cc046b43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ListLexicons never mints a continuation token: the handler always returns `next_token: None`, and its own module doc says so explicitly ("`NextToken` ... is therefore never returned"). Yet the handler still accepts an inbound `NextToken` and, whenever that token happens to be hex of valid UTF-8, uses the decoded string as a cursor and silently drops every lexicon whose name sorts at or before it — returning HTTP 200 with a short `Lexicons` array and NO `NextToken`, so the caller cannot tell the list was cut.

The handler's own stated rule is "a token this service could not have minted is `InvalidNextTokenException` (400) rather than a silently wrong list". Because the operation mints NO token at all, EVERY inbound token is one this service could not have minted — so under its own rule every one of them should be refused. What actually decides is `crate::state::decode_page_token`, which only asks "is this hex of UTF-8?", a test that any caller-fabricated string passes.

The sibling handler in the same crate, `DescribeVoices`, draws the opposite (and self-consistent) conclusion from the identical premise: it is not paginated, mints no token, and therefore refuses any non-empty `NextToken` with `InvalidNextTokenException` before reading any state. Two non-paginated reads in one crate disagree about what a forged token means.

## Evidence

The premise and the broken rule sit in the SAME doc comment, services/service-com-amazonaws-polly-parrot-v1/src/ops/list_lexicons.rs:7-18:
```
//! The model declares no page-size input for this operation and does not mark it
//! `@paginated` (`ListSpeechSynthesisTasks` is the service's one paginated read), so the
//! report is not truncated: a single call reports the Region's whole lexicon list and
//! `NextToken` — documented as returned "only if the response is truncated" — is
//! therefore never returned. ...
//! The modelled `NextToken` input is still checked: a token this service could not have minted is
//! `InvalidNextTokenException` (400) rather than a silently wrong list, and a
//! well-formed one names the lexicon to resume after.
```

The code under it, list_lexicons.rs:152-162 and 183-188:
```
        // A token must be one this service could have minted; anything else is refused
        // before any state is read, so a garbage token can never read as "the whole
        // list". A well-formed one names the lexicon the listing resumes after.
        let after = match input.next_token.as_deref() {
            Some(token) => Some(crate::state::decode_page_token(token).ok_or_else(|| {
                ListLexiconsErrorModeled::InvalidNextTokenException(
...
        Ok(ListLexiconsOutput {
            lexicons: Some(lexicons),
            // The response carries the Region's whole list, so it is never truncated
            // and no continuation token is owed.
            next_token: None,
        })
```
So the only token the service ever hands out is none, while the acceptance test is services/service-com-amazonaws-polly-parrot-v1/src/state.rs:473-475:
```
pub fn decode_page_token(token: &str) -> Option<String> {
    String::from_utf8(::hex::decode(token).ok()?).ok()
}
```
Any caller-fabricated hex string passes it.

The sibling that applies the rule correctly, services/service-com-amazonaws-polly-parrot-v1/src/ops/describe_voices.rs:184-198:
```
        // This operation is not paginated, so the service mints no continuation token
        // for it: a non-empty `NextToken` is one it could not have issued, and is
        // refused before any state is read rather than answered with a partial list.
        // An empty token (the model admits length 0) names no position, so it reads as
        // no token at all.
        if input
            .next_token
            .as_deref()
            .is_some_and(|token| !token.is_empty())
        {
            return Err(DescribeVoicesErrorModeled::InvalidNextTokenException(
```
and its header, describe_voices.rs:20-25: "the model declares no pagination for this operation ... The modelled `NextToken` input is therefore a token this service never issues, and a non-empty one is `InvalidNextTokenException` (400) rather than a partial list."

The model backs the sibling's reading. `smithy_query(query="doc", shape="ListLexiconsInput", member="NextToken")`: "An opaque pagination token returned from previous `ListLexicons` operation. If present, indicates where to continue the list of lexicons." `ListLexiconsOutput$NextToken` (OPERATIONS.md ListLexicons output; doc quoted in the handler header): returned "only if the response is truncated". A token that was not returned from a previous ListLexicons is not a token this input accepts — and this handler returns none, ever.

Test coverage: the current behaviour is pinned by services/service-com-amazonaws-polly-parrot-v1/tests/all/lexicon_listing.rs:160-206, whose RULE line states both halves of the contradiction — "RULE: a `NextToken` this service could not have minted is `InvalidNextTokenException` (HTTP 400), and a well-formed one resumes after the name it carries" — and then feeds `hex::encode(lexicon_name(after))`, a token the service never minted, expecting it to resume. The neighbouring test at lexicon_listing.rs:60-79 asserts the opposite invariant for the no-token case ("one call must report every lexicon in name order" / "the whole list fits in the response, so no NextToken is owed") and at :81-89 that "the listing must not contradict the reads" — which the forged-token path breaks, since GetLexicon still answers 200 for a name the short list omitted.

## Steps to Reproduce

1. Boot the emulator and PutLexicon three valid PLS lexicons named Lex01, Lex02, Lex03 in one Region
2. GET /v1/lexicons — observe all three in Lexicons and no NextToken (correct)
3. GET /v1/lexicons?NextToken=4c657830 32 (i.e. hex of the ASCII string "Lex02", no spaces: 4c65783032) — a token the service has never issued
4. Observe HTTP 200 with Lexicons containing ONLY Lex03 and no NextToken key: two stored lexicons are omitted from an apparently complete list
5. GET /v1/lexicons/Lex01 — HTTP 200, contradicting the list the previous call returned
6. Send the same fabricated token to the sibling non-paginated read: GET /v1/voices?NextToken=4c65783032 — HTTP 400 InvalidNextTokenException, which is what list_lexicons.rs:16-17 says ListLexicons does too
