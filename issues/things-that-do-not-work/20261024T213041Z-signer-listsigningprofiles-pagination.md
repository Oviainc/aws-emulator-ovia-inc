Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: aa34a7732176a121f53f6fa0a60cefb8de4c89bc

# ListSigningProfiles accepts a nextToken it never handed out and answers 200 with an arbitrary page

_Filed: 2026-10-24T21:30:41Z_
_Operation: ListSigningProfiles | Class: pagination_

> _Note: this issue was identified against the code base at commit `aa34a7732176a121f53f6fa0a60cefb8de4c89bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListSigningProfiles` uses the next page's `profileName` as its pagination token, and the only check it performs on an incoming `nextToken` is that the string LOOKS like a profile name (`crate::state::is_profile_name`: length 2..64, `[A-Za-z0-9_]`). Any such string is then used as a resume key into the profile map.

Expected: a pagination token is an opaque, server-issued value; a request carrying a token the service never issued is refused with a 400 (`ValidationException`, which is what this handler already reserves for the case and what the service's other paginated reads answer for a bad token) rather than being served as a page.
Actual: `GET /signing-profiles?nextToken=zzzz` answers HTTP 200 with whatever suffix of the profile ordering happens to sort at or after `zzzz` — usually an empty `profiles` list and no `nextToken`, i.e. "you have reached the end of the list". A client whose token got truncated, swapped between filters, or carried over from another listing is silently told the account holds no further signing profiles instead of being told its token is invalid, so a paginating loop terminates early and silently drops profiles.

The handler already intends this rule — its comment reads "A token this service never handed out is refused with the modelled 400" — but the shape test it implements cannot tell a token the service issued from any other profile-name-shaped string, so the refusal only fires for tokens with the wrong characters/length. Consolidating: the same looseness means a token that WAS handed out but whose profile has since been renamed away/filtered is indistinguishable from garbage, and the token is not bound to the `platformId`/`statuses`/`includeCanceled` filters it was issued under, so reusing a token against a different filter set silently serves a page from the middle of a different result set.

## Evidence

Smithy model: `ListSigningProfiles` is `@paginated(inputToken: nextToken, outputToken: nextToken)` with `nextToken` an `@httpQuery` string (`smithy_query(query="operation", operation="ListSigningProfiles")`); the handler widens its error set with `extra_errors = ["ValidationException"]` (HTTP 400) specifically so a refused token can carry the modelled `code`, which the implementation then only uses for mis-shaped strings.

Handler: `services/service-com-amazonaws-signer-wallabyservice/src/ops/list_signing_profiles.rs:177-190` — `if !crate::state::is_profile_name(token) { ValidationException }`, i.e. the whole token validation; `src/state.rs:889-894` shows `is_profile_name` is a pure shape test (`2..=64` bytes of `[A-Za-z0-9_]`). Lines 224-250 then resume by skipping names `< token` with no check that the token was ever emitted and no binding to the filters in the request.

Contrast inside the same crate: `ListProfilePermissions` does bind its token to state — `src/ops/list_profile_permissions.rs:212-216` refuses a token that is not a statement id the policy actually holds — showing the intended standard this operation falls short of.

Test coverage: `tests/all/signing_profile_lifecycle.rs::list_pages_through_profiles_and_honours_the_filters` walks real tokens only; `tests/all/profile_permissions.rs::a_next_token_this_service_never_handed_out_is_refused` asserts the rule for `ListProfilePermissions`, and there is no equivalent assertion for `ListSigningProfiles` — no test covers a profile-name-shaped token that was never issued.

## Steps to Reproduce

1. Boot the emulator and create three signing profiles with `PUT /signing-profiles/{Profile_A,Profile_B,Profile_C}` and body `{"platformId": "AWSLambda-SHA384-ECDSA"}`.
2. Call `GET /signing-profiles?maxResults=2` and observe 2 profiles plus a `nextToken` of `Profile_C` (the token is the raw profile name, not an opaque value).
3. Call `GET /signing-profiles?nextToken=zzzz` — a token the service never issued. Observe HTTP 200 with `{"profiles": []}` instead of a 400 `ValidationException` naming the invalid token.
4. Call `GET /signing-profiles?nextToken=Profile_B` — also never issued as a token in step 2's page sequence — and observe HTTP 200 serving a page from the middle of the ordering.
5. Call `GET /signing-profiles?nextToken=Profile_C&platformId=AWSIoTDeviceManagement-SHA256-ECDSA`, reusing a token issued under different filters; observe it is accepted rather than refused.
