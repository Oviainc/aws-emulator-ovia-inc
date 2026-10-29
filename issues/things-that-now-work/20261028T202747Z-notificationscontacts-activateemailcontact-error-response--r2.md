Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 0f071a90384ece90fe46e5865745f415e3851c94

# ActivateEmailContact answers 400 for a wrong code on an active contact whose token is no longer outstanding, where its own doc states 404

_Filed: 2026-10-28T20:27:47Z_
_Operation: ActivateEmailContact | Class: error-response_

> _Note: this issue was identified against the code base at commit `0f071a90384ece90fe46e5865745f415e3851c94`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler's module doc states one rule for which failure the operation answers: the modelled 404 is for "a contact that holds no outstanding activation token — none was ever sent, or the one that was has run past the lifetime its `code` documents or spent the attempt budget it documents", and the modelled 400 is only "for a code that is not the token outstanding for this contact". `crate::state` repeats it: a token past its lifetime or with its budget spent "is no longer outstanding either: the contact again has no token for a code to be compared against until a new send mints one, so it is the same missing resource".

The `already_active` branch (src/ops/activate_email_contact.rs:205-214) returns before the `outstanding` test and never consults it. So when a contact is ACTIVE and its stored token is no longer outstanding, a submitted code that differs from it is answered with `ValidationException` (HTTP 400) even though, by the rule the same file and `crate::state` state, that contact holds NO activation token and the answer must be the `ResourceNotFoundException` (HTTP 404) naming the absent `activation-token` resource. The INACTIVE path four lines below (:221-228) applies exactly that rule to the identical token state, so the status a caller gets for "wrong code, no outstanding token" depends only on whether the contact happens to be active.

This state is reachable with no clock manipulation at all and is then PERMANENT: four wrong codes followed by the correct one spends all five of the token's attempts while flipping the contact to `active`, and `SendActivationCode` refuses an active contact with a 409 (src/ops/send_activation_code.rs:172-183), so no new token can ever supersede it. From then on every wrong-code call answers 400 instead of the documented 404. The same divergence appears once the token passes its 12-hour lifetime.

A second symptom of the same branch ignoring the token: the module doc says "Each call against an outstanding token spends one of its attempts", but a wrong-code call against an active contact whose token IS still outstanding records no attempt (`record_attempt()` lives only on the path below, :236-240).

The 200 arm of that branch (a matching code re-activating an active contact, however old the code) is deliberate and pinned by a test, and must stay as it is; only the `else` arm's refusal is at odds with the stated rule.

## Evidence

The rule, in the handler's own module doc — services/service-com-amazonaws-notificationscontacts-notificationscontacts/src/ops/activate_email_contact.rs:7-13:
```
//! through a later `GetEmailContact` or `ListEmailContacts`. The modelled 404 for an
//! ARN the store does not hold AND for a contact that holds no outstanding activation
//! token — none was ever sent, or the one that was has run past the lifetime its
//! `code` documents or spent the attempt budget it documents — and the modelled 400
//! for a code that is not the token outstanding for this contact. Each call against an
//! outstanding token spends one of its attempts.
```

The same rule in the handler body — src/ops/activate_email_contact.rs:183-186:
```
            // none has no token resource to compare with: that is the modelled 404,
            // naming the absent token. Only once a token is outstanding does a
            // submitted code that differs from it become a bad field — the modelled
            // 400.
```

and in the state helper — src/state.rs:494-497:
```
/// A token that has run past [`ACTIVATION_CODE_LIFETIME_MILLIS`] or spent its
/// [`MAX_ACTIVATION_ATTEMPTS`] budget is no longer outstanding either: the contact
/// again has no token for a code to be compared against until a new send mints one, so
/// it is the same missing resource.
```

The code that breaks it — src/ops/activate_email_contact.rs:205-214 (note `outstanding`, computed at :197, is never read on this path):
```
        if already_active {
            return if code_matches {
                Ok(ActivateEmailContactOutput {})
            } else {
                Err(ActivateEmailContactErrorModeled::ValidationException(
                    crate::state::field_validation_failure("code", WRONG_CODE),
                )
                .into())
            };
        }
```
versus the sibling path for the identical token state — src/ops/activate_email_contact.rs:221-228:
```
        if !outstanding {
            return Err(
                ActivateEmailContactErrorModeled::ResourceNotFoundException(
                    crate::state::activation_token_not_found(&input.arn),
                )
                .into(),
            );
        }
```

Why the state is permanent — src/ops/send_activation_code.rs:172-183: `if contact.is_active() { return Err(... ConflictException ...) }`, so a spent/expired token on an active contact can never be replaced.

The rule as this crate's own test suite states it — tests/all/activation_token_resource.rs:5-9:
```
//! RULE: a request whose outcome turns on a stored resource that another operation
//! mints is refused with the modelled `ResourceNotFoundException` (HTTP 404), naming
//! the ABSENT resource in `resourceId`/`resourceType`, whenever that resource does not
//! exist; the modelled `ValidationException` (HTTP 400) is only for a well-formed
//! request whose submitted value disagrees with a resource that DOES exist.
```

Test coverage: the only existing assertion on the `already_active` branch is the MATCHING-code arm — tests/all/activation_code_bounds.rs:396-411 ages the token past twice its lifetime and requires `activate` with the issued code to stay 200 ("re-activating an active contact with its own code must stay 200"). No test in tests/all/activation_code_bounds.rs, tests/all/activation_token_resource.rs or tests/all/email_contacts.rs submits a WRONG code to an ACTIVE contact, so correcting the `else` arm leaves every existing assertion unchanged.

## Steps to Reproduce

1. Read src/ops/activate_email_contact.rs:7-13 and src/state.rs:494-497: a contact whose token has spent its five-attempt budget or passed its 12-hour lifetime "holds no outstanding activation token", and that condition is the modelled 404.
2. Start the emulator and `CreateEmailContact(name="oncall", emailAddress="oncall@example.com")`; keep the returned arn.
3. `SendActivationCode(arn)` -> 200; read the issued code out of the store (as tests/all/email_contacts.rs `issued_code` does).
4. Submit four well-formed WRONG codes via `ActivateEmailContact(arn, code)` (e.g. 0000000, 0000001, 0000002, 0000003) -> four 400s, spending four of the token's five attempts.
5. Submit the issued code -> 200; the contact is now `active` and the token's fifth and last attempt is spent, so `ActivationToken::budget_is_spent()` is true and `is_outstanding()` is false.
6. `SendActivationCode(arn)` -> 409 ConflictException: no new token can ever be minted for this contact.
7. Submit a wrong code again: observed HTTP 400 `ValidationException` blaming `code`; expected, per the handler's own doc and the sibling path at :221-228, HTTP 404 `ResourceNotFoundException` with resourceType `activation-token`. The same divergence is reproducible by instead aging the token past ACTIVATION_CODE_LIFETIME_MILLIS after a successful activation.
