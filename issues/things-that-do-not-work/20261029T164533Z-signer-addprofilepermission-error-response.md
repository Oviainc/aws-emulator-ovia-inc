Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 06b0d41e00354be103b9e8418d096ed156619087

# AddProfilePermission refusals echo unbounded request strings (action, statementId, revisionId) whole, skipping the echo ceiling this crate states and applies

_Filed: 2026-10-29T16:45:33Z_
_Operation: AddProfilePermission | Class: error-response_

> _Note: this issue was identified against the code base at commit `06b0d41e00354be103b9e8418d096ed156619087`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every refusal `AddProfilePermission` words itself interpolates the caller's own bytes with no ceiling: the 400 for an ungrantable `action`, the 409 for a superseded `revisionId` and the 409 for a `statementId` the policy already holds. All three members target a bare `String` in this model — no `@length`, no `@pattern` — so the boundary validator imposes no size on them (the handler's own comment says so for `action`: "modelled as a bare `String`, so the boundary validator let any text through"), and a 10 MiB `statementId` or `action` in the request body comes straight back inside the error `message`.

This crate already states the opposite rule twice and implements it once. `crate::state::stated_value` / `MAX_STATED_VALUE` exist precisely because "A value with no ceiling at all would let one oversized query parameter become an equally large error body", and `crate::state::tag_key_problem` is ordered the way it is so that "the prose for either later condition can echo a key the model's own bound already limits". The framework's own refusal renderer caps every value it echoes at 256 bytes (`MAX_ECHOED_VALUE`). The permissions slice is the one refusal path in this crate that quotes an unconstrained request value unelided, so one request parameter becomes an equally large response body — the outcome the crate says its ceiling exists to prevent.

This is narrower than, and not covered by, the open issue 20261029T123510Z-signer-getrevocationstatus-error-response: that one is about `platformId` and enumerates the other sites it knows of (`GetSigningPlatform`, `PutSigningProfile`, `TaggingArnProblem::message`); it does not mention the permissions slice or any of these three members.

## Evidence

The members carry no bound to lean on — `smithy_query(query="shape", shape="AddProfilePermissionRequest")`:

> "action": {"target": "String", "required": true, ...}, "principal": {"target": "String", "required": true, ...}, "revisionId": {"target": "String", ...}, "statementId": {"target": "String", "required": true, ...}

(no `length`, no `pattern` on any of them, unlike the sibling `profileName`/`profileVersion` which do carry them).

The handler knows the boundary let the value through — services/service-com-amazonaws-signer-wallabyservice/src/ops/add_profile_permission.rs:225-232:
```
        // `action` is modelled as a bare `String`, so the boundary validator let
        // any text through; the four actions a Signer grant may carry are the ones
        // the member documents, and nothing else is a grant AWS Signer could hold.
        if !crate::state::permissions::is_grantable_action(&input.action) {
            return Err(validation(
                crate::state::permissions::ungrantable_action_message(&input.action),
            ));
        }
```
and the message it builds echoes it whole — src/state/permissions.rs:55-61:
```
pub fn ungrantable_action_message(action: &str) -> String {
    format!(
        "action {action} is not an AWS Signer action that can be granted to another account; \
         expected one of {}",
        GRANTABLE_ACTIONS.join(", ")
    )
}
```
The two conflict refusals do the same — src/ops/add_profile_permission.rs:266-274 and :278-286:
```
        if let Some(named) = input.revision_id.as_deref() {
            if current_revision.as_deref() != Some(named) {
                return Err(conflict(format!(
                    "revision id {named} is not the current revision of the permissions of \
                     profile {}",
                    input.profile_name
                )));
```
```
            return Err(conflict(format!(
                "statement id {} already exists in the permissions of profile {}",
                input.statement_id, input.profile_name
            )));
```

The rule this crate states — src/state.rs:1523-1533:
```
/// The longest rejected value a clause states in full.
///
/// The value between the quotes is the caller's OWN bytes ... A value with no
/// ceiling at all would let one oversized query parameter become an equally
/// large error body, so the ceiling is set where the shared refusal-wording
/// module sets the largest echo it restores into a clause (16 KiB)
const MAX_STATED_VALUE: usize = 16 * 1024;
```
and states again for tag keys — src/state.rs:1114-1121:
```
/// Why `key` is not a key the modelled `TagKey` admits, or `None` when it is one.
///
/// The length is judged first so the prose for either later condition can echo a
/// key the model's own bound already limits.
pub fn tag_key_problem(key: &str) -> Option<TagKeyProblem> {
```
and which the framework applies to every echo it writes — vendor/smithy-schema-server/src/constraints.rs:387-412:
```
/// Ceiling on the rejected value as rendered INTO a message.
///
/// Separate from the count cap because one violation is enough: a single member
/// holding a multi-megabyte string produces a multi-megabyte clause.
const MAX_ECHOED_VALUE: usize = 256;
```

Test coverage, and why a ceiling changes no existing expectation: tests/all/profile_permissions.rs:940-955 asserts only the status, the `code` and `answer.message().contains("action")` for every refused action (rows at :922-929 are all short strings, longest `"signer:PutSigningProfile"`); the stale-revision rows (tests/all/profile_permissions.rs:631-745) and the duplicate-statement rows (:819-893) likewise assert on short ids. Routing these three echoes through `crate::state::stated_value` leaves every one of those rows byte-identical.

## Steps to Reproduce

1. Boot the emulator with this service mounted.
2. PUT /signing-profiles/Echo_Probe with {"platformId":"AWSLambda-SHA384-ECDSA"} to seed a profile.
3. POST /signing-profiles/Echo_Probe/permissions with {"statementId":"s1","action":"<1048576 'a' characters>","principal":"123456789012"} — observe HTTP 400 ValidationException whose message carries the whole megabyte back: the request parameter and the error body are the same size.
4. POST /signing-profiles/Echo_Probe/permissions with {"statementId":"s1","action":"signer:GetSigningProfile","principal":"123456789012"} twice — the second answers HTTP 409 whose message echoes the whole `statementId`; repeat with a 1 MiB statementId to see the same amplification on the conflict path.
5. Contrast with the ceiling the crate already applies on a sibling refusal of an equally unconstrained member: GET /revocations?...&platformId=<1 MiB of 'a'>! answers a 400 whose echoed value stops at 16 KiB and ends with `... [1048577 bytes]` (crate::state::stated_value).
