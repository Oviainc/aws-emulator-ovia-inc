Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: f30b839d650f592709ba7d30fd0311226ca1f682

# AssociateFeed's "output already exists" conflict echoes the stored feed's id, which this crate's own message rule forbids for a state-decided refusal

_Filed: 2026-10-29T16:30:43Z_
_Operation: AssociateFeed | Class: error-response_

> _Note: this issue was identified against the code base at commit `f30b839d650f592709ba7d30fd0311226ca1f682`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

AssociateFeed's refusal of an appended output whose name the stored feed already holds under a different definition is decided entirely from STORED STATE (`Feed::outputs_to_append` compares the request's outputs against the ones the feed holds). This crate states ONE rule for exactly that class of refusal: a refusal decided from the stored resource carries the rule's INVARIANT sentence and interpolates no value the handler read out of state, and that sentence lives in `crate::state` so every operation words it alike. This refusal breaks the rule twice over: it is built inside the handler with `format!`, and it interpolates `feed.id` — a value read off the borrowed stored record — so the message is not byte-identical across records and a client cannot match it against a constant.

The divergence is internal, not hypothetical: the crate's own test for the sibling state-decided refusal (`UpdateFeed` on a feed being deleted) asserts that the refusal must NOT echo the feed id, and the other half of the very same "an output's name identifies it within its feed" rule — a request that lists one name twice — is worded by `crate::state::duplicate_output_name_message`, which names only the value the REQUEST supplied and lives in state. So one rule about output names is answered in two different conventions by the same handler, and the state-decided half is the one that leaks a record value into the message.

The fix keeps the 409 `ConflictException`, the no-write effect and every existing assertion: word the refusal as that rule's invariant sentence in `crate::state` (naming at most the output name the request supplied, as its duplicate-name sibling does) and have the handler lift only the modelled error class.

## Evidence

The rule the crate sets for itself — `src/state.rs:205-213`: "A refusal's message follows the RULE that refused the request, not the request's or the record's values: where the violated rule is a property of the stored resource, or a relation between two members, no single value is what is at fault, so the message is the rule's invariant sentence — the same text whatever the request named and whatever the record holds." The documented EXCEPTIONS are enumerated at `src/state.rs:215-222` (`feed_not_associated_message`, `dictionary_referenced_message`) and this refusal is not one of them.

The same claim in the handler's own header — `src/ops/associate_feed.rs:17-20`: "a refusal decided from the STORED feed carries that rule's invariant sentence (`crate::state::FEED_ALREADY_ASSOCIATED_MESSAGE`, `crate::state::FEED_NOT_ASSOCIABLE_MESSAGE`): the message interpolates no value read out of state — not the feed's status and not the resource that holds it;".

The code under it — `src/ops/associate_feed.rs:403-414`: `let to_append = match feed.outputs_to_append(requested) { Ok(to_append) => to_append, Err(name) => { return Err(AssociateFeedErrorModeled::ConflictException(ConflictException { message: format!("Feed {} already has an output named {}. AssociateFeed can only append new outputs; use UpdateFeed to modify an existing one.", feed.id, name) }).into()); } };` — `feed.id` is read off the stored record borrowed at `src/ops/associate_feed.rs:307` (`let Some(feed) = state.feed_mut(&region, &input.id)`), and the refusal itself is decided from the stored list (`src/state.rs:916-923`: "An output whose NAME the feed holds under a DIFFERENT definition is not in place and is not appendable either ... That one is refused, and the `Err` carries its name for the refusal to report.").

The crate's own test of the rule, for the sibling state-decided refusal — `tests/all/error_message_rule.rs:490-493`: `assert!(!message.contains(id.as_str()), "case {label:?}: the refusal must not echo the feed id {id:?}, got {message:?}");`, under the suite header `tests/all/error_message_rule.rs:8-12`: "a refusal decided from STORED STATE ... carries that rule's INVARIANT sentence — byte-identical across requests and across records, and interpolating no value the handler read out of state".

The other half of the same output-name rule, worded in state and echoing only the request's value — `src/state.rs:327-335`: `/// A request that lists more than one output under `name`: the rule is about ONE value's own validity ... so the message names the value the REQUEST supplied` / `pub fn duplicate_output_name_message(name: &str) -> String { format!("The request lists more than one output named {name}. Output names must be unique within a feed.") }`.

Test coverage: no test pins this message's text — `grep -n 'already has an output' services/service-com-amazonaws-elementalinference-elementalinference/tests` returns no matches, and the case is exercised only for its code and effect at `tests/all/feed_association.rs:626-677` (`assert_eq!(status, expected, ...)`, `body["__type"] == "ConflictException"`, plus "the conflicting call appended nothing"), so re-wording it leaves every existing assertion green.

## Steps to Reproduce

1. POST /v1/feed with {"name":"conflict-feed","outputs":[{"name":"crop1","outputConfig":{"cropping":{}},"status":"ENABLED"}]} and note the returned feed id F
2. POST /v1/feed/F/associate with {"associatedResourceName":"source-media","outputs":[{"name":"crop1","outputConfig":{"clipping":{}},"status":"ENABLED"}],"dryRun":false}
3. Observe HTTP 409 ConflictException whose message is "Feed F already has an output named crop1. AssociateFeed can only append new outputs; use UpdateFeed to modify an existing one." — the refusal, decided from the stored feed's output list, echoes the stored record's id, so the message differs for every feed
4. Compare the sibling state-decided refusal: DELETE /v1/feed/F, then PUT /v1/feed/F — the answer is crate::state::FEED_BEING_DELETED_MESSAGE, which names neither the feed nor its status, exactly as tests/all/error_message_rule.rs:490-493 asserts it must not
5. Expected: the conflict refusal carries that rule's invariant sentence from crate::state (at most the output name the request supplied, as duplicate_output_name_message does), with no value read out of state
