Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 11fd98189f90040dc28179001c4adbbf7cfc6f77

# The PLS `pls_with_replacement` test fixture is duplicated byte-for-byte in two suites instead of living in the shared fixtures module

_Filed: 2026-10-27T04:16:53Z_
_Operation: PutLexicon | Class: maintainability_

> _Note: this issue was identified against the code base at commit `11fd98189f90040dc28179001c4adbbf7cfc6f77`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two suites of this crate's test tree each define their own private, identical copy of the PLS-document fixture `pls_with_replacement`, while the tree already has one module whose stated purpose is to hold exactly these shared fixtures so those two suites cannot drift. The copies encode the same document logic (namespace, version, alphabet, xml:lang, one lexeme, one replacement element of N characters); if one copy is adjusted — a different alphabet, a second lexeme, a changed element nesting — the suites silently stop testing the same document while both keep passing. The fix is to move the one definition into `tests/all/support.rs` (which both suites already import from) and delete the copies; no response and no assertion changes.

## Evidence

Copy 1 — services/service-com-amazonaws-polly-parrot-v1/tests/all/error_semantics.rs:71-80:
```
/// A PLS lexicon with one lexeme whose `element` replacement is `length` characters.
fn pls_with_replacement(element: &str, length: usize) -> String {
    format!(
        "<lexicon version=\"1.0\" \
         xmlns=\"http://www.w3.org/2005/01/pronunciation-lexicon\" \
         alphabet=\"ipa\" xml:lang=\"en-US\">\
         <lexeme><grapheme>G</grapheme><{element}>{}</{element}></lexeme></lexicon>",
        "x".repeat(length)
    )
}
```
Copy 2 — services/service-com-amazonaws-polly-parrot-v1/tests/all/error_document_message.rs:75-84: the same ten lines, character for character (same doc comment, same format string, same `"x".repeat(length)`).

The crate's own anti-drift rule for these two suites — tests/all/support.rs:3-11: "Fixtures the speech-synthesis-task suites share. One definition of what a started task looks like in the store, one way to seed it, one way to read a task back and one reader for the `message` slot of an error document, so `error_semantics` (the read path), `synthesis_tasks` (the list path), `task_id_validity` (the two `TaskId` refusals) and `error_document_message` (the rendering) cannot drift on any of them."

Tests that cover the duplicated helper and must stay unchanged: error_semantics.rs:303 `every_modeled_error_carries_the_fixed_message_for_its_code` (lines 308, 371, 407), error_semantics.rs:538 `the_lexicon_count_quota_refusal_carries_its_fixed_message` (line 540), error_semantics.rs:620 `put_lexicon_enforces_the_lexeme_length_quota` (lines 638, 673), and error_document_message.rs:133 `every_error_document_carries_its_message_member` (lines 161, 223, 231).

## Steps to Reproduce

1. Open services/service-com-amazonaws-polly-parrot-v1/tests/all/error_semantics.rs at line 71 and services/service-com-amazonaws-polly-parrot-v1/tests/all/error_document_message.rs at line 75: the two `pls_with_replacement` definitions are identical.
2. Open services/service-com-amazonaws-polly-parrot-v1/tests/all/support.rs line 3-11: the module exists to hold the fixtures these same two suites share so they "cannot drift on any of them", and both suites already `use crate::support::{...}`.
3. Change the alphabet in one copy only (e.g. "ipa" -> "x-sampa"): both suites still compile and pass while now exercising different documents, which is the drift the shared module was created to prevent.
