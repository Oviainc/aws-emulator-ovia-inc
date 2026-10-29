Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 07e288609021e6571eb22a2173d293968d6da2ab

# The TaggingArnProblem -> modelled-error-shape mapping is written out three times, once per tagging operation

_Filed: 2026-10-29T13:37:41Z_
_Operation: TagResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `07e288609021e6571eb22a2173d293968d6da2ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::TaggingArnProblem` tells the two ways a tagging `resourceArn` can fail to reach a profile apart precisely so each can answer its own modelled shape and status, and it already owns the `code` and the prose for both. The BRANCH that turns a problem into the shape — `Malformed` -> `BadRequestException` (400), `NoSuchProfile` -> `NotFoundException` (404) — is then re-implemented, body for body, in all three tagging handlers: `tag_resource.rs`, `untag_resource.rs` and `list_tags_for_resource.rs`. The three copies differ only in the generated error enum they name.

That is one rule in four places (the state doc plus three handlers), so a later change to which condition answers which shape has to be made three times and the copies can drift: one handler answering 404 where another answers 400 for the same ARN would be invisible at the call sites, which all just write `.map_err(|problem| refused(problem, &input.resource_arn))?`.

The crate already has the seam for collapsing it: `src/service.rs` uses a local declarative macro (`frame_empty_answers!`) to instantiate one body across several per-operation types, so the mapping can be stated once and expanded per operation with every response byte-identical.

## Evidence

The rule, stated once in state — services/service-com-amazonaws-signer-wallabyservice/src/state.rs:1026-1050:
```
/// The two cases answer DIFFERENT modelled errors, which is why they are told
/// apart: a label that is not a signing-profile ARN at all is a bad request
/// (`BadRequestException`, "The request contains invalid parameters for the ARN or
/// tags"), while a well-formed ARN naming no stored profile is a missing resource
/// (`NotFoundException`, "The signing profile was not found").
pub enum TaggingArnProblem { Malformed, NoSuchProfile }
```
Copy 1 — src/ops/tag_resource.rs:126-138:
```
fn refused(problem: crate::state::TaggingArnProblem, resource_arn: &str) -> TagResourceError {
    let message = Some(problem.message(resource_arn));
    let code = Some(problem.error_code().to_string());
    match problem {
        crate::state::TaggingArnProblem::Malformed => {
            TagResourceErrorModeled::BadRequestException(BadRequestException { message, code })
        }
        crate::state::TaggingArnProblem::NoSuchProfile => {
            TagResourceErrorModeled::NotFoundException(NotFoundException { message, code })
        }
    }
    .into()
}
```
Copy 2 — src/ops/untag_resource.rs:125-137: identical body, `UntagResourceErrorModeled::BadRequestException` / `::NotFoundException`.
Copy 3 — src/ops/list_tags_for_resource.rs:119-137: identical body, `ListTagsForResourceErrorModeled::BadRequestException` / `::NotFoundException`.

The existing test that pins the behaviour the fix must not change — tests/all/resource_tags.rs:553 `an_unresolvable_resource_arn_is_refused_the_same_way_by_every_tagging_operation`, whose rows assert `shape: "BadRequestException"` (lines 570, 577, 584, 593, 600) and `shape: "NotFoundException"` (lines 609, 618, 627) for all three operations; the file header states the contract at tests/all/resource_tags.rs:16-17: "`BadRequestException`, and a well-formed one naming no stored profile 404 `NotFoundException`, for every one of the three operations".

The seam already used for one-body-per-operation expansion in this crate — src/service.rs:411-417 (`frame_empty_answers!(empty_answers, engine, cancel_signing_profile::CancelSigningProfile, ...)`).

## Steps to Reproduce

1. Open services/service-com-amazonaws-signer-wallabyservice/src/ops/tag_resource.rs:126, src/ops/untag_resource.rs:125 and src/ops/list_tags_for_resource.rs:119 side by side: the three `refused` bodies are the same match on `TaggingArnProblem`, differing only in the error enum named.
2. Change which shape one condition answers in ONE of the three (e.g. make `NoSuchProfile` answer `BadRequestException` in tag_resource.rs) and observe that the other two operations keep the old mapping — the rule is not stated in one place.
3. Run tests/all/resource_tags.rs `an_unresolvable_resource_arn_is_refused_the_same_way_by_every_tagging_operation`: it is the test that must stay byte-identical after the three copies are collapsed into one expansion.
