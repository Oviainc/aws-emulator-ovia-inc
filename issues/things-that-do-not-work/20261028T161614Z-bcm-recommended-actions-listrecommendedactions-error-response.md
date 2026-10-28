Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 804fb9e39527c7d8ad6b71682b2c9d408f11768a

# A nextToken issued for a different filter is refused with reason cannotParse, the code the handler reserves for a value it could not read at all

_Filed: 2026-10-28T16:16:14Z_
_Operation: ListRecommendedActions | Class: error-response_

> _Note: this issue was identified against the code base at commit `804fb9e39527c7d8ad6b71682b2c9d408f11768a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`parse_token` has two distinct refusal branches and the handler's own comments treat them as different classes of failure:

1. the token is not something this service minted (wrong length, or a non-lowercase-hex character) — "reported as unreadable";
2. the token IS read successfully (correct length, all lowercase hex, fingerprint and offset split out) but its 128-bit selection fingerprint names a different filter — "a token for another query".

Both branches return the `cannot_parse` helper, so both answers carry the modelled required member `ValidationException$reason = "cannotParse"`. That contradicts the rule the crate states for that reason code in the two doc comments that define it: `cannotParse` is documented as the reason for "a value this service could not read at all (here: a malformed `nextToken`)" and the helper is documented as being "for a value the service could not read at all … naming the member it could not read". In branch 2 the service demonstrably DID read the value — it parsed the token into a fingerprint and an offset and compared the fingerprint — and its own message says so ("The nextToken was issued for a different request", fieldList message "issued for a different filter"), which is not a statement that the value was unreadable.

The result is that a client branching on the modelled `reason` member cannot distinguish "you sent me garbage" from "you sent me a valid token for another listing", even though this handler deliberately distinguishes them in every other part of the response it builds. The two sides of the contradiction are in the same file, a few hundred lines apart, and no test pins the reason for either token branch.

## Evidence

Rule side (this crate's own definition of the reason code):
- services/service-com-amazonaws-bcmrecommendedactions-awsbillingandcostmanagementrecommendedactions/src/ops/list_recommended_actions.rs:172-174 —
  "/// `ValidationExceptionReason$cannotParse` — the request carries a value this
   /// service could not read at all (here: a malformed `nextToken`).
   const REASON_CANNOT_PARSE: &str = \"cannotParse\";"
- same file:234-236 —
  "/// A 400 `ValidationException` for a value the service could not read at all —
   /// `ValidationExceptionReason$cannotParse` — naming the member it could not read.
   fn cannot_parse("
  whose body hardcodes "reason: REASON_CANNOT_PARSE.to_string()," (same file:242).

Code side (the branch that applies that reason to a value it DID read):
- same file:502-517 —
  "// A minted token is lower-case hex throughout, so anything else — including an
   // upper-cased copy of a real token — is a token this service did not mint, and is
   // reported as unreadable rather than as a token for another query.
   if token.len() != TOKEN_LEN || !token.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')) {
       return Err(malformed());
   }
   let (token_fingerprint, offset) = token.split_at(TOKEN_FINGERPRINT_LEN);
   if token_fingerprint != fingerprint {
       return Err(cannot_parse(
           \"The nextToken was issued for a different request. Repeat the original filter when paginating.\",
           (
               \"nextToken\".to_string(),
               \"issued for a different filter\".to_string(),
           ),
       ));
   }"
  The comment itself names the distinction ("reported as unreadable rather than as a token for another query") that the reason member then erases.

Model context: `ValidationException$reason` is REQUIRED and typed by `ValidationExceptionReason`, whose values are ["unknownOperation", "cannotParse", "fieldValidationFailed", "other"] (smithy_query query="shape" shape="ValidationException,ValidationExceptionReason"); its doc is "Provides a single, overarching explanation for the validation failure."

Test coverage: tests/all/list_recommended_actions.rs:631-688 (`a_next_token_this_service_did_not_mint_is_refused`) drives both branches — including "a token issued for a different filter" (lines 663-666) — but asserts only `status == 400` (line 675) and `error_code(..) == "ValidationException"` (line 679); it never asserts `reason`, so no test pins the current value for either branch. By contrast the member-level refusal path IS pinned: tests/all/list_recommended_actions.rs:863-867 asserts `reason == "fieldValidationFailed"` with the stated reason "a member-level refusal is `fieldValidationFailed`" (lines 859-862).

## Steps to Reproduce

1. Read src/ops/list_recommended_actions.rs:172-174 and 234-236: this crate defines `cannotParse` as the reason for a value the service "could not read at all", e.g. a malformed nextToken.
2. Read src/ops/list_recommended_actions.rs:505-517: the first branch (bad length / non-hex) returns `malformed()`; the second branch is reached only AFTER the token was read — split into a 32-hex-character fingerprint and an 8-hex-character offset — and still returns `cannot_parse`, i.e. reason "cannotParse".
3. Start the emulator, call ListRecommendedActions with maxResults=1 and filter {FEATURE EQUALS [BUDGETS]} to mint a nextToken.
4. Re-send that token on an unfiltered request (or with a different filter): the response is 400 ValidationException with message "The nextToken was issued for a different request." and fieldList [{name: nextToken, message: "issued for a different filter"}] — but reason "cannotParse", the code the file reserves for a value it could not read at all.
5. Compare with step 1: the only two places in the crate that state what `cannotParse` means disagree with the branch that emits it.
