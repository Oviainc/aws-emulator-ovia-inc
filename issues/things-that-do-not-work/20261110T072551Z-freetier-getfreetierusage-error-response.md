Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: db7313a8939a25ce751a06463082be3acfaf223f

# GetFreeTierUsage filter refusals echo the request into the `[Message = …]` envelope this crate documents as carrying nothing of the request

_Filed: 2026-11-10T07:25:51Z_
_Operation: GetFreeTierUsage | Class: error-response_

> _Note: this issue was identified against the code base at commit `db7313a8939a25ce751a06463082be3acfaf223f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states ONE rule for the `message` a handler-raised `ValidationException` of this service carries: the sentence states the CONDITION and nothing of the request, and it is wrapped by the single front door `crate::state::stated_validation`. `GetFreeTierUsage` routes every one of its own refusals through that function (its local `validation` helper), and the refusals of the filter's ARITY conditions obey the rule: `ONE_ROOT_OPERATOR` and `ONE_MATCH_OPTION` are fixed constants, each documented with exactly that reasoning, and `tests/all/match_option_cardinality.rs` pins the match-option one down to 'carries no count' and 'does not name MatchOptions / Dimensions / filter / EQUALS'.

But the sibling refusals inside the very same function (`check_dimension`) interpolate the request into the sentence they hand to the same envelope: the submitted `Key`, the submitted `MatchOptions` entry and the submitted `Values` entry, plus the dotted member path the arity sentence is forbidden from naming. So one member (`DimensionValues$MatchOptions`), refused by one operation, inside one envelope, is worded by two opposite rules depending on WHICH way it is wrong — and the rule the crate documents and tests is the one the echoing branches break.

These branches are reachable: every value involved satisfies the model, so the framework's constraint walk does not pre-empt them. `GREATER_THAN_OR_EQUAL` is a declared `MatchOption`, `SERVICE` a declared `Dimension`, and `"abc"` satisfies `Value`'s `@length(1,20)` and `^[ a-zA-Z0-9\-\:\.\_\/\,\$\(\)]*$`. A client therefore reads, for one logical failure class and in one envelope, sometimes a fixed reason it can branch on and sometimes a reason that varies with its own input — which is precisely what the envelope's doc says must not happen.

## Evidence

The rule, stated once on the envelope every refusal of this operation passes through — `services/service-com-amazonaws-freetier-awsfreetierservice/src/state.rs:702-707`:
```
/// `sentence` states the condition and nothing of the request: a reason string
/// that varied with the input would make a client's branch on it depend on
/// something the rule does not distinguish.
pub fn stated_validation(sentence: &str) -> String {
    format!("[Message = {sentence}]")
}
```
That this operation's refusals all go through it — `src/ops/get_free_tier_usage.rs:573-581`:
```
/// Refuse the request with this operation's own modelled `ValidationException`
/// (HTTP 400), stating `message` in the envelope this service's validation
/// front door states a refusal in ([`crate::state::stated_validation`]).
fn validation(message: impl AsRef<str>) -> GetFreeTierUsageError {
    GetFreeTierUsageErrorModeled::ValidationException(ValidationException {
        message: crate::state::stated_validation(message.as_ref()),
    })
```
The branches that OBEY the rule, in the same file — `src/ops/get_free_tier_usage.rs:245-249`:
```
/// What the service states when `MatchOptions` does not carry exactly one option
/// (`DimensionValues$MatchOptions`: "You can specify only one of these values in
/// the array"). As fixed as [`ONE_ROOT_OPERATOR`], and for the same reason: the
/// count submitted is not part of the rule.
const ONE_MATCH_OPTION: &str = "Exactly one match option must be provided";
```
and `:234-243` for `ONE_ROOT_OPERATOR` ("ONE sentence for the whole condition, with nothing taken from the request").

The branches that BREAK it, all inside `check_dimension` and all handed to the same `validation` envelope:
* `src/ops/get_free_tier_usage.rs:450-456`:
```
if option == MATCH_GREATER_THAN_OR_EQUAL {
    return Err(validation(format!(
        "filter.Dimensions.MatchOptions: {MATCH_GREATER_THAN_OR_EQUAL} compares \
         magnitudes, so it applies only to {DIMENSION_USAGE_PERCENTAGE}, not to \
         {key}"
    )));
```
* `src/ops/get_free_tier_usage.rs:460-465`: `"filter.Dimensions.MatchOptions: {key} is a number, so it compares with {MATCH_EQUALS} or {MATCH_GREATER_THAN_OR_EQUAL}, not with {option}"`
* `src/ops/get_free_tier_usage.rs:484-488`: `"filter.Dimensions.Values: {wanted} is not a number, and {key} compares numbers"`
* `src/ops/get_free_tier_usage.rs:418-423` and `:435-440` do the same for `filter.Dimensions.Key` / `filter.Dimensions.MatchOptions` (`"{key} is not a dimension; expected one of …"`, `"{option} is not a match option; expected one of …"`).

The test that pins the rule for the arity refusal of the SAME member — `tests/all/match_option_cardinality.rs:194-222`:
```
/// The sentence states NOTHING of the request: every offending request reads identically,
/// and the text carries neither a count nor the member path the comparison was reached at.
...
            for borrowed in ["MatchOptions", "Dimensions", "filter", "EQUALS"] {
                assert!(
                    !actual.contains(borrowed),
                    "case {case} ({options:?}): expected a sentence carrying nothing of the \
                     request, but it names {borrowed:?}: {actual:?}"
```
No test asserts the wording of the echoing branches (`tests/all/dimension_decode.rs` asserts only that such a comparison is REFUSED with a `ValidationException`, at every nesting position), so the two halves of one rule can keep drifting unnoticed.

Reachability of the echoing branches, from the model: `smithy_query(query="shape", shape="MatchOption")` -> `["EQUALS","STARTS_WITH","ENDS_WITH","CONTAINS","GREATER_THAN_OR_EQUAL"]`; `Dimension` -> includes `SERVICE`; `Values` member `Value` -> `length {min:1,max:20}`, pattern `^[ a-zA-Z0-9\-\:\.\_\/\,\$\(\)]*$` (so `"abc"` is admitted by the framework walk and reaches `handle`).

## Steps to Reproduce

1. Boot the emulator with the freetier service mounted.
2. Call GetFreeTierUsage with filter={"Dimensions":{"Key":"SERVICE","Values":["AWSLambda"],"MatchOptions":["GREATER_THAN_OR_EQUAL"]}} and read the ValidationException message: it is `[Message = filter.Dimensions.MatchOptions: GREATER_THAN_OR_EQUAL compares magnitudes, so it applies only to USAGE_PERCENTAGE, not to SERVICE]` (src/ops/get_free_tier_usage.rs:450-456) — it names the member path, the submitted option and the submitted key.
3. Call GetFreeTierUsage with the same Dimensions but MatchOptions=["EQUALS","STARTS_WITH"] and read the message: it is `[Message = Exactly one match option must be provided]` — one fixed sentence for the whole condition, as src/ops/get_free_tier_usage.rs:245-249 documents and tests/all/match_option_cardinality.rs:194-222 pins (the test even forbids the words `MatchOptions`, `Dimensions`, `filter`).
4. Call GetFreeTierUsage with filter={"Dimensions":{"Key":"USAGE_PERCENTAGE","Values":["abc"],"MatchOptions":["EQUALS"]}} (every value satisfies the model, so the framework walk passes it to the handler) and read `[Message = filter.Dimensions.Values: abc is not a number, and USAGE_PERCENTAGE compares numbers]` (src/ops/get_free_tier_usage.rs:484-488) — again varying with the request.
5. Compare all three against src/state.rs:702-707, the one envelope all three are stated in, whose doc says the sentence "states the condition and nothing of the request": two of this operation's reachable refusal branches contradict the rule the third obeys.
