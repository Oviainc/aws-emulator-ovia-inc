Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: b827135cf833dfec17047e08babbbec809d0cc25

# CreateAppAuthorization's idempotency fingerprint omits the credential, so a replay with a different API key/client secret is accepted as a retry

_Filed: 2026-10-26T08:26:18Z_
_Operation: CreateAppAuthorization | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `b827135cf833dfec17047e08babbbec809d0cc25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateAppAuthorization` is `@idempotent` and the model requires that a replayed `clientToken` carrying DIFFERENT parameters be refused with the documented `IdempotentParameterMismatch` (which this crate maps to the modelled `ConflictException`, see `crate::state::idempotent_parameter_mismatch`). The handler builds its replay fingerprint from every request member EXCEPT the credential's actual values: only the union member NAME (`credentialKind`) goes in. Consequently a second call with the same `clientToken`, the same app/tenant/authType/tags, but a different `apiKey` (or a different `clientId`/`clientSecret`) is judged a true retry: it answers HTTP 201 and replays the FIRST authorization, silently discarding the credential the caller just sent, instead of answering the documented mismatch conflict.

This also contradicts the standard the rest of the crate holds itself to: every other create operation's fingerprint covers the full value of every member the request carries — `CreateIngestionDestination` even renders the whole destination configuration down to the S3 bucket name and prefix through `render_destination_configuration` — and this operation itself treats a changed tag VALUE (a far less consequential parameter) as a mismatch. Only the credential, the one member that decides whether the authorization can actually connect, is exempt. Note the record deliberately does not persist credential material, and the fix must not change that: the defect is that the request's credential values take no part in the retry judgement at all.

## Evidence

Smithy contract — smithy_query(query="doc", shape="CreateAppAuthorizationRequest", member="clientToken"): "Passing the same value to a later call to an operation requires that you also pass the same value for all other parameters." and "If you retry the operation with the same <code>ClientToken</code>, but with different parameters, the retry fails with an <code>IdempotentParameterMismatch</code> error." `credential` is one of those parameters (OPERATIONS.md:44 lists `credential` (Credential, required) for CreateAppAuthorization).

Handler — src/ops/create_app_authorization.rs:258-275:
```
        // The PARENT and every member that identifies the authorization go into
        // the fingerprint: the same token for another bundle, application or
        // tenant is a different request rather than a retry of this one. The
        // credential's KIND is part of it; its secret is not.
        let fingerprint = crate::state::request_fingerprint(
            &[
                ("appBundleArn", app_bundle_arn.as_str()),
                ("app", input.app.as_str()),
                ("tenantIdentifier", input.tenant.tenant_identifier.as_str()),
                (
                    "tenantDisplayName",
                    input.tenant.tenant_display_name.as_str(),
                ),
                ("authType", input.auth_type.as_str()),
                ("credentialKind", credential_kind),
            ],
            input.tags.as_deref(),
        );
```
and the replay path it drives — src/ops/create_app_authorization.rs:285-299: `if spent.fingerprint != fingerprint { ... ConflictException(idempotent_parameter_mismatch(...)) }` else `return Ok(CreateAppAuthorizationOutput { app_authorization })` (the FIRST call's record).

Crate standard it diverges from — src/state.rs:1271-1290 `render_destination_configuration` pushes `bucket.bucket_name` and `bucket.prefix` (and `stream.stream_name`) into the fingerprint, i.e. every value of the member, used by src/ops/create_ingestion_destination.rs:242-257.

Test coverage: tests/all/create_app_authorization_contract.rs:226-301 is the idempotency table; its own doc says it spans "the members a create call identifies an authorization by — `app`, each `tenant` member, `authType` with its credential kind, and `tags`", and the row "another tag value" (lines 291-300) expects `Replay::Mismatch` (HTTP 409). No row varies the credential's secret, so nothing pins the current behaviour; tests/all/create_app_authorization_contract.rs:407-471 (`a_credential_must_match_the_auth_type_and_is_never_reported_back`) pins only that no response echoes credential material, which a fix must keep true.

## Steps to Reproduce

1. POST /appbundles with body {} -> 201, note appBundle.arn
2. POST /appbundles/{arn}/appauthorizations with body {"clientToken":"11111111-0000-4000-8000-000000000000","app":"SLACK","authType":"apiKey","credential":{"apiKeyCredential":{"apiKey":"key-one"}},"tenant":{"tenantIdentifier":"t-1","tenantDisplayName":"t-1"}} -> 201, note appAuthorization.appAuthorizationArn
3. Repeat the identical call with the SAME clientToken but "apiKey":"key-two"
4. Observe HTTP 201 replaying the first authorization: the differing `credential` parameter raises no ConflictException/IdempotentParameterMismatch and the new key is dropped
5. Contrast: repeat step 2's call with the same clientToken and only a changed tag value -> HTTP 409 ConflictException (the behaviour tests/all/create_app_authorization_contract.rs row "another tag value" pins), showing the credential is the one parameter exempted from the retry judgement
