Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f9dfc7a2b81347ee1edf7acfb92674e6dbacd756

# Service catalog breaks its own namespace rule: `ssm` is published while `systems-manager` — the console home the comment itself names — is absent

_Filed: 2026-10-29T06:46:45Z_
_Operation: ListServices | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f9dfc7a2b81347ee1edf7acfb92674e6dbacd756`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`src/state.rs` states, as the rule the seeded catalog is built to, that the catalog holds ONE namespace throughout: the console service identifiers, never a service's SigV4 signing name / endpoint prefix. It then names the entries it deliberately omits and, for each omission that is really filed under another console home, the console identifier it lives under — among them "`appconfig` (under `systems-manager`)".

The seeded list contradicts that in two directions at once:

1. `systems-manager` — the console home the comment itself names, and therefore by the comment's own rule an entry "a console service home is published under" — is NOT in `SEED_SERVICE_CATALOG`. So the omission rationale for `appconfig` redirects callers to an identifier the catalog does not publish.
2. `ssm` IS in the catalog (src/state.rs:380), i.e. the signing-name/endpoint-prefix spelling, which is exactly the namespace the same comment says the catalog must not be keyed on (it excludes `cognito-idp` in favour of `cognito`, `logs` in favour of `cloudwatch`, and `sts` outright).

This is not cosmetic, because the same comment makes the catalog the ADMISSION list as well as the discovery list: `UpdateAccountCustomizations` refuses any `visibleServices` member the catalog does not name. So `{"visibleServices": ["systems-manager"]}` — an identifier this crate's own documentation presents as a console service home — is answered HTTP 400 `ValidationException` "Invalid services provided", and `ListServices` never reports it, while the endpoint-prefix spelling `ssm` is both discoverable and settable. The crate cannot be right both ways: either `systems-manager` is a console identifier (then it belongs in the catalog and must be settable) or the stated reason for omitting `appconfig` is wrong.

## Evidence

The stated rule — services/service-com-amazonaws-uxc-awsaccountuxsetting/src/state.rs:167-170:

    /// Entries no console service home is published under are deliberately absent —
    /// `access-analyzer` (IAM Access Analyzer lives under `iam`), `account`,
    /// `appconfig` (under `systems-manager`), `cognito-idp`, `logs` and `sts` — so the
    /// catalog is one namespace throughout.

and the namespace it claims to be, src/state.rs:136-139:

    /// the same set: the console names Amazon Cognito `cognito` where the API endpoint
    /// prefix is `cognito-idp`, names CloudWatch Logs under `cloudwatch` where the
    /// prefix is `logs`, and publishes console-only entries (`activate`, `artifact`,
    /// `aws-marketplace`, `ce`) for services with no endpoint prefix of that name at

The other redirect target IS published — `iam` appears at src/state.rs:288 (`    "iam",`) — but `systems-manager` appears nowhere in `SEED_SERVICE_CATALOG` (src/state.rs:178-405); what the list carries instead is the endpoint-prefix spelling, src/state.rs:380:

        "ssm",

And the catalog is the admission list, so the absence is a refusal — src/state.rs:170-175:

    /// It is the ADMISSION list as well as the discovery list: `ListServices`
    /// "returns a paginated list of ... service identifiers that you can use as
    /// values for the `visibleServices` setting", and the setting's own member doc
    /// sends a caller here for "valid service identifiers", so an identifier this
    /// catalog does not name is not a valid value of the setting and
    /// `UpdateAccountCustomizations` refuses it

enforced at src/ops/update_account_customizations.rs:238-254:

            if let Some(supplied) = input.visible_services.as_deref() {
                if supplied
                    .iter()
                    .any(|service| !guard.is_catalog_service(service))
                {
                    return Err(UpdateAccountCustomizationsErrorModeled::ValidationException(
                        ValidationException {
                            message: INVALID_SERVICES.to_string(),

Test coverage: tests/all/service_catalog.rs:91-102 pins the namespace rule over nine rows (`("CloudWatch Logs", Some("cloudwatch"), "logs")`, `("Amazon Cognito", Some("cognito"), "cognito-idp")`, `("AWS STS", None, "sts")`, …) — asserting each console identifier present and each endpoint prefix absent — but no row covers Systems Manager, so neither `ssm`'s presence nor `systems-manager`'s absence is pinned. tests/all/visible_service_catalog_membership.rs:58-68 (`ENDPOINT_PREFIXES`) likewise omits `ssm`. Adding `systems-manager` and dropping the prefix spelling therefore breaks no existing assertion: tests/all/account_customizations.rs:346-372 (`every_catalog_entry_is_accepted_as_a_visible_service`) and tests/all/service_catalog.rs:163-182 only require every entry to be model-legal, settable and strictly increasing.

## Steps to Reproduce

1. Read services/service-com-amazonaws-uxc-awsaccountuxsetting/src/state.rs:167-170 — the catalog deliberately omits `appconfig` because it is "under `systems-manager`", i.e. `systems-manager` is a console service home.
2. Search `SEED_SERVICE_CATALOG` (src/state.rs:178-405) for `systems-manager`: it is not there. Search for `ssm`: it is there, at src/state.rs:380.
3. Start the emulator and call `GET /v1/services`, walking the pages: `ssm` is reported; `systems-manager` never is.
4. Call `PATCH /v1/account-customizations` with `{"visibleServices": ["systems-manager"]}` (a value satisfying `Service` `@length(1,64)` and `@pattern ^[a-z0-9]+(-[a-z0-9]+)*$`): HTTP 400 `ValidationException` with message "Invalid services provided", because the catalog is also the admission list (src/state.rs:170-175, src/ops/update_account_customizations.rs:238-254).
5. Call the same request with `{"visibleServices": ["ssm"]}`: HTTP 200 — the endpoint-prefix spelling is accepted, which is the namespace src/state.rs:136-139 says the catalog must exclude.
