Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 7bcb53c96ea0ea6c32faf644dec1ae2940515247

# PutLexicon answers InvalidLexiconException for an unsupported PLS alphabet instead of UnsupportedPlsAlphabetException

_Filed: 2026-10-24T13:38:56Z_
_Operation: PutLexicon | Class: error-response_

> _Note: this issue was identified against the code base at commit `7bcb53c96ea0ea6c32faf644dec1ae2940515247`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A PLS document whose `<lexicon alphabet="...">` names an alphabet Amazon Polly does not support is the exact condition the modeled `UnsupportedPlsAlphabetException` describes ("The alphabet specified by the lexicon is not a supported alphabet. Valid values are ipa, x-sampa, x-amazon-pinyin, x-amazon-jyutping, x-amazon-pron-kana, and x-amazon-yomigana. Replace the unsupported alphabet with one of the valid values and resubmit your request.").

Expected: PUT /v1/lexicons/{Name} with a well-formed PLS document declaring e.g. alphabet="x-klingon" is refused with HTTP 400 and error code `UnsupportedPlsAlphabetException` (the code tells the caller exactly which attribute to fix, and that is what the model declares the error for).

Actual: the handler collapses the unsupported-alphabet case into `InvalidLexiconException` on purpose, so `UnsupportedPlsAlphabetException` is unreachable anywhere in the crate — a client branching on the error code takes the generic "not a lexicon" branch and cannot distinguish a malformed document from a valid document with a bad alphabet. The error constructor for this code does not even exist in `crate::service::errors`, so no request can ever produce it.

## Evidence

Spec: the Smithy model declares `UnsupportedPlsAlphabetException` (HTTP 400) among PutLexicon's errors, with the documentation quoted above naming the valid alphabet values — i.e. the error is specific to this failure mode. Handler: services/service-com-amazonaws-polly-parrot-v1/src/ops/put_lexicon.rs — `parse_pls` returns `PlsRejection::UnsupportedAlphabet` (lines ~331-335), and `impl From<PlsRejection> for PutLexiconError` (lines ~189-214) maps `NotALexicon | UnsupportedAlphabet` to `PutLexiconErrorModeled::InvalidLexiconException`, with a comment stating the choice. Proof the modeled code is dead: `grep -r unsupported_pls_alphabet` over the crate returns nothing, and src/service.rs's `errors` module (lines 268-366) has constructors for every other modeled Polly error but not this one. Test coverage: tests/all/lexicons_and_speech.rs ("unknown alphabet" row, ~line 327) and tests/all/error_message_wording.rs ("an unsupported alphabet", ~line 205) currently assert the InvalidLexiconException behaviour, so the divergence is locked in by tests rather than uncovered.

## Steps to Reproduce

1. Boot the emulator and call PutLexicon with Name="Bad" and Content=`<lexicon version="1.0" xmlns="http://www.w3.org/2005/01/pronunciation-lexicon" alphabet="x-klingon" xml:lang="en-US"/>`
2. Observe HTTP 400 with error code `InvalidLexiconException`
3. Real Polly refuses the same document with HTTP 400 `UnsupportedPlsAlphabetException` (the error the model declares for an unsupported alphabet)
4. Verify no other request shape can produce `UnsupportedPlsAlphabetException` either: the code is never constructed in the crate
