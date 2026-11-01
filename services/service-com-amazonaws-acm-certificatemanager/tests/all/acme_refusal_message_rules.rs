// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the refusals ACM's ACME control plane words by a FIXED rule
//! (`POST /`, `X-Amz-Target: CertificateManager.<Op>`, awsJson1_1).
//!
//! RULE 1 (a replayed idempotency token): every ACME create that indexes its
//! `@idempotencyToken` refuses a token re-sent with parameters that are not the ones it
//! already created from with `ConflictException` (HTTP 400) and ONE fixed sentence
//! ([`state::IDEMPOTENCY_TOKEN_REUSED_MESSAGE`]) — the same text for every operation and
//! every changed parameter, interpolating neither the token, nor a parameter name, nor
//! the identifier of the resource the first call created.
//!
//! RULE 2 (a domain one endpoint already validates): a second `CreateAcmeDomainValidation`
//! for such a domain is refused `ConflictException` (HTTP 400) with a sentence whose ONLY
//! variable part is the `DomainName` the request submitted — not the ARN of the
//! validation that already holds it.
//!
//! RULE 3 (an update of a validation that is not settled): the refusal says which of the
//! two reasons it is. A validation the SERVICE is still working through is a precondition
//! on the status the record carries, so it is `ValidationException` (HTTP 400) naming
//! THAT status; a validation ANOTHER caller's update is still changing is that caller's
//! modification in progress, so it is `ConflictException` (HTTP 400) naming the ARN the
//! request addressed and asking for a retry. At least one of two racing updates wins, the
//! loser writes nothing, and once the winner's work falls due the loser's update is
//! accepted — so the conflict is a window, not a wall.
//!
//! Every assertion goes through [`MultiServiceRouter::dispatch`], the same path the
//! emulator serves production traffic on, and every read goes through a DIFFERENT
//! operation than the write, so a response that is not computed from the persisted store
//! cannot pass. The wire harness and the create/describe/clock helpers are shared with
//! [`crate::certificate_lifecycle`], [`crate::acme_endpoint_lifecycle`],
//! [`crate::acme_domain_validation_lifecycle`] and
//! [`crate::acme_external_account_binding_lifecycle`].
//!
//! The tables span what the rules generalise over: WHICH create the token belongs to and
//! WHICH parameter of it changed (RULE 1), the FORM of the domain name the sentence
//! carries — apex, deep subdomain, punycode, single label and one at `DomainName`'s
//! modelled maximum length (RULE 2), the STATUS the record is in (RULE 3), and which
//! members the racing updates disagree about (RULE 3).

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_acm_certificatemanager::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::acme_domain_validation_lifecycle::{
    create_validation, describe_validation, endpoint_body, hold_prevalidations,
    settle_prevalidations_now, DOMAIN_SCOPE_OPTIONS, HOSTED_ZONE_ID,
};
use crate::acme_endpoint_lifecycle::create_endpoint;
use crate::certificate_lifecycle::{
    call, error_code, error_message, router_with_engine, ACCOUNT,
};

/// A second `HostedZoneId` satisfying the member's modelled pattern (`^Z[A-Z0-9]+$`).
const OTHER_HOSTED_ZONE_ID: &str = "Z1D633PJN98FT9";

/// The one unsettled `AcmeDomainValidationStatus` no call of this service writes, so no
/// constant in `state` names it: the value the model declares beside `VALIDATING`,
/// `VALID` and `INVALID` (`smithy_query(query="shape",
/// shape="com.amazonaws.acm#AcmeDomainValidationStatus")`, as
/// `ACME_DOMAIN_VALIDATION_STATUS_VALUES` in `src/ops/types/shared_types.rs` lists them).
const DELETING_STATUS: &str = "DELETING";

/// A `DomainScope` naming `exact` as its `ExactDomain`, as the wire carries it.
fn exact_domain_scope(exact: &str) -> Value {
    json!({ "ExactDomain": exact })
}

/// `PrevalidationOptions` carrying `options` as its `DnsPrevalidation` member — the one
/// member the union declares.
fn options(options: &Value) -> Value {
    json!({ "DnsPrevalidation": options })
}

/// Send `UpdateAcmeDomainValidation` for `arn` naming `update`'s prevalidation options,
/// and return `(status, body)`.
async fn update_validation(
    router: &MultiServiceRouter,
    arn: &str,
    update: &Value,
) -> (u16, Value) {
    call(
        router,
        "UpdateAcmeDomainValidation",
        &json!({
            "AcmeDomainValidationArn": arn,
            "PrevalidationOptions": options(update),
        }),
    )
    .await
}

/// The `DnsPrevalidationDetails` a describe reports for one validation.
async fn details(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    describe_validation(router, case, arn).await["PrevalidationDetails"]["DnsPrevalidation"]
        .clone()
}

/// RULE 1: the fixed sentence is the same for every ACME create that indexes a token and
/// for every parameter the replay changed, and it carries nothing of the request or of
/// the resource the first call created.
#[tokio::test]
async fn a_replayed_idempotency_token_is_refused_in_one_fixed_sentence() {
    let (router, _engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "token host", &endpoint_body()).await;

    // Each row: what it exercises, the operation, the first request, and the SAME token
    // on a request that differs in one parameter. The rows differ along which create the
    // token belongs to and which member of it changed.
    let rows: Vec<(&str, &str, Value, Value)> = vec![
        (
            "CreateAcmeEndpoint, a different Contact",
            "CreateAcmeEndpoint",
            json!({
                "AuthorizationBehavior": "PRE_APPROVED",
                "Contact": "REQUIRED",
                "CertificateAuthority": { "PublicCertificateAuthority": {} },
                "IdempotencyToken": "endpoint-contact",
            }),
            json!({
                "AuthorizationBehavior": "PRE_APPROVED",
                "Contact": "NOT_REQUIRED",
                "CertificateAuthority": { "PublicCertificateAuthority": {} },
                "IdempotencyToken": "endpoint-contact",
            }),
        ),
        (
            "CreateAcmeEndpoint, a different CertificateTags",
            "CreateAcmeEndpoint",
            json!({
                "AuthorizationBehavior": "PRE_APPROVED",
                "CertificateAuthority": { "PublicCertificateAuthority": {} },
                "CertificateTags": [{ "Key": "owner", "Value": "platform" }],
                "IdempotencyToken": "endpoint-tags",
            }),
            json!({
                "AuthorizationBehavior": "PRE_APPROVED",
                "CertificateAuthority": { "PublicCertificateAuthority": {} },
                "CertificateTags": [{ "Key": "owner", "Value": "security" }],
                "IdempotencyToken": "endpoint-tags",
            }),
        ),
        (
            "CreateAcmeExternalAccountBinding, a different RoleArn",
            "CreateAcmeExternalAccountBinding",
            json!({
                "AcmeEndpointArn": endpoint_arn,
                "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/AcmeIssuer"),
                "IdempotencyToken": "binding-role",
            }),
            json!({
                "AcmeEndpointArn": endpoint_arn,
                "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/OtherAcmeIssuer"),
                "IdempotencyToken": "binding-role",
            }),
        ),
        (
            "CreateAcmeExternalAccountBinding, a different Expiration",
            "CreateAcmeExternalAccountBinding",
            json!({
                "AcmeEndpointArn": endpoint_arn,
                "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/ExpiringAcmeIssuer"),
                "Expiration": { "Value": 12, "Type": "HOURS" },
                "IdempotencyToken": "binding-expiry",
            }),
            json!({
                "AcmeEndpointArn": endpoint_arn,
                "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/ExpiringAcmeIssuer"),
                "Expiration": { "Value": 13, "Type": "HOURS" },
                "IdempotencyToken": "binding-expiry",
            }),
        ),
        (
            "CreateAcmeDomainValidation, a different PrevalidationOptions",
            "CreateAcmeDomainValidation",
            json!({
                "AcmeEndpointArn": endpoint_arn,
                "DomainName": "token-options.example.com",
                "PrevalidationOptions": options(&json!({ "HostedZoneId": HOSTED_ZONE_ID })),
                "IdempotencyToken": "validation-options",
            }),
            json!({
                "AcmeEndpointArn": endpoint_arn,
                "DomainName": "token-options.example.com",
                "PrevalidationOptions": options(&json!({
                    "HostedZoneId": OTHER_HOSTED_ZONE_ID,
                })),
                "IdempotencyToken": "validation-options",
            }),
        ),
        (
            "CreateAcmeDomainValidation, a different DomainName",
            "CreateAcmeDomainValidation",
            json!({
                "AcmeEndpointArn": endpoint_arn,
                "DomainName": "token-domain.example.com",
                "PrevalidationOptions": options(&json!({})),
                "IdempotencyToken": "validation-domain",
            }),
            json!({
                "AcmeEndpointArn": endpoint_arn,
                "DomainName": "token-other-domain.example.com",
                "PrevalidationOptions": options(&json!({})),
                "IdempotencyToken": "validation-domain",
            }),
        ),
    ];

    for (case, operation, first, replayed) in rows {
        let (status, created) = call(&router, operation, &first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first call must succeed, got {status} {created}"
        );

        let (status, refusal) = call(&router, operation, &replayed).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a token re-sent with other parameters must be refused, got \
             {status} {refusal}"
        );
        assert_eq!(
            error_code(&refusal),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {refusal}"
        );
        let message = error_message(&refusal);
        assert_eq!(
            message,
            state::IDEMPOTENCY_TOKEN_REUSED_MESSAGE,
            "case {case:?}: expected the fixed idempotency sentence, got {message:?}"
        );
        // The sentence is fixed text: nothing of the request, and nothing of the
        // resource the first call created, reaches the caller through it.
        assert!(
            !message.contains("arn:")
                && !message.contains(
                    replayed["IdempotencyToken"].as_str().unwrap_or_default()
                ),
            "case {case:?}: the refusal must interpolate nothing, got {message:?}"
        );
    }
}

/// RULE 2: the duplicate-domain sentence names the domain the REQUEST submitted, whatever
/// form that name takes, and nothing else.
#[tokio::test]
async fn a_duplicate_domain_validation_is_refused_by_the_domain_the_request_named() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "duplicate host", &endpoint_body()).await;

    // The forms `DomainName`'s modelled pattern admits
    // (`^([a-z0-9]([a-z0-9-]*[a-z0-9])?\.)*[a-z0-9]([a-z0-9-]*[a-z0-9])?$`), including one
    // at its modelled maximum length (253) — the row the issue cited is only one of them.
    let apex = "dup-apex.example.com".to_string();
    let deep = "a.b.c.d.deep-dup.example.com".to_string();
    let punycode = "xn--dup-9na.example.com".to_string();
    let single_label = "duplabel".to_string();
    let longest = {
        let label = "dup-abcdefghij";
        let mut name = String::new();
        while name.len() + label.len() < 253 {
            name.push_str(label);
            name.push('.');
        }
        let apex_len = 253 - name.len();
        name.push_str(&"d".repeat(apex_len));
        name
    };
    assert_eq!(
        longest.len(),
        253,
        "case \"the longest DomainName\": the row must sit on the modelled maximum, got {}",
        longest.len()
    );

    for domain in [apex, deep, punycode, single_label, longest] {
        let case = format!("a second validation for {domain}");
        let body = json!({
            "AcmeEndpointArn": endpoint_arn,
            "DomainName": domain,
            "PrevalidationOptions": options(&json!({ "HostedZoneId": HOSTED_ZONE_ID })),
        });
        let first = create_validation(&router, &case, &body).await;
        hold_prevalidations(&engine);

        let (status, refusal) = call(&router, "CreateAcmeDomainValidation", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the duplicate must be refused, got {status} {refusal}"
        );
        assert_eq!(
            error_code(&refusal),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {refusal}"
        );
        let message = error_message(&refusal);
        assert_eq!(
            message,
            state::acme_domain_validation_exists_message(&domain),
            "case {case:?}: expected the domain-named sentence, got {message:?}"
        );
        assert!(
            !message.contains(&first),
            "case {case:?}: the refusal must not name the stored validation {first}, got \
             {message:?}"
        );

        // The refusal created nothing: the endpoint still validates that domain through
        // the ONE validation the first call minted.
        let stored = describe_validation(&router, &case, &first).await;
        assert_eq!(
            stored["DomainName"].as_str(),
            Some(domain.as_str()),
            "case {case:?}: the stored validation must still be the first one, got {stored}"
        );
    }
}

/// RULE 3, the status half: a validation the SERVICE is still working through is refused
/// `ValidationException` naming the status the RECORD carries — the status comes from the
/// resource's state, so a record in another unsettled status is named by that one.
#[tokio::test]
async fn an_update_of_a_validation_the_service_is_working_through_names_its_status() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "status host", &endpoint_body()).await;

    // Both unsettled statuses `AcmeDomainValidationStatus` declares
    // (`smithy_query(query="shape", shape="com.amazonaws.acm#AcmeDomainValidationStatus")`:
    // VALIDATING | VALID | INVALID | DELETING): the `VALIDATING` a create leaves, and
    // `DELETING`, written onto the stored record so the sentence is read off the state
    // rather than off the call that produced it.
    let rows = [
        (
            "VALIDATING, as the create left it",
            "validating.status.example.com",
            state::ACME_DOMAIN_VALIDATION_STATUS_VALIDATING,
        ),
        ("DELETING", "deleting.status.example.com", DELETING_STATUS),
    ];

    for (case, domain, status_value) in rows {
        let arn = create_validation(
            &router,
            case,
            &json!({
                "AcmeEndpointArn": endpoint_arn,
                "DomainName": domain,
                "PrevalidationOptions": options(&json!({ "HostedZoneId": HOSTED_ZONE_ID })),
            }),
        )
        .await;
        // Pin the transient status so it cannot race the wall clock, and put the record
        // in the status this row is about.
        hold_prevalidations(&engine);
        {
            let store = engine.state::<state::ComAmazonawsAcmCertificatemanagerState>();
            let mut store = store.lock();
            let validation = store
                .acme_domain_validations
                .get_mut(&arn)
                .unwrap_or_else(|| panic!("case {case:?}: {arn} must be stored"));
            validation.status = status_value.to_string();
        }
        let before = describe_validation(&router, case, &arn).await;
        assert_eq!(
            before["Status"].as_str(),
            Some(status_value),
            "case {case:?}: the record must be in {status_value}, got {before}"
        );

        let (status, refusal) = update_validation(
            &router,
            &arn,
            &json!({ "HostedZoneId": OTHER_HOSTED_ZONE_ID }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: the update must be refused, got {status} {refusal}"
        );
        assert_eq!(
            error_code(&refusal),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refusal}"
        );
        assert_eq!(
            error_message(&refusal),
            state::acme_domain_validation_status_blocks_update_message(status_value),
            "case {case:?}: expected the sentence naming {status_value}, got {refusal}"
        );
        assert_eq!(
            describe_validation(&router, case, &arn).await,
            before,
            "case {case:?}: a refused update must write nothing"
        );
    }
}

/// RULE 3, the concurrency half: an update that arrives while another caller's update of
/// the same validation is still in flight loses with `ConflictException` naming the ARN,
/// writes nothing, and is accepted once the winner's work falls due.
#[tokio::test]
async fn an_update_racing_another_update_loses_with_the_concurrency_conflict() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "race host", &endpoint_body()).await;

    // The rows differ along which members the two updates disagree about: the hosted zone
    // alone, the scope alone, both, and a loser that asks for exactly what the winner
    // wrote (so the conflict is decided by the update in flight, not by what is asked).
    let rows: Vec<(&str, &str, Value, Value)> = vec![
        (
            "the hosted zone alone",
            "zone.race.example.com",
            json!({ "HostedZoneId": OTHER_HOSTED_ZONE_ID }),
            json!({ "HostedZoneId": HOSTED_ZONE_ID }),
        ),
        (
            "the domain scope alone",
            "scope.race.example.com",
            json!({ "DomainScope": exact_domain_scope(DOMAIN_SCOPE_OPTIONS[0]) }),
            json!({ "DomainScope": exact_domain_scope(DOMAIN_SCOPE_OPTIONS[1]) }),
        ),
        (
            "both members at once",
            "both.race.example.com",
            json!({
                "HostedZoneId": OTHER_HOSTED_ZONE_ID,
                "DomainScope": exact_domain_scope(DOMAIN_SCOPE_OPTIONS[1]),
            }),
            json!({
                "HostedZoneId": HOSTED_ZONE_ID,
                "DomainScope": exact_domain_scope(DOMAIN_SCOPE_OPTIONS[0]),
            }),
        ),
        (
            "a loser asking for what the winner wrote",
            "same.race.example.com",
            json!({ "HostedZoneId": OTHER_HOSTED_ZONE_ID }),
            json!({ "HostedZoneId": OTHER_HOSTED_ZONE_ID }),
        ),
    ];

    for (case, domain, winner, loser) in rows {
        let arn = create_validation(
            &router,
            case,
            &json!({
                "AcmeEndpointArn": endpoint_arn,
                "DomainName": domain,
                "PrevalidationOptions": options(&json!({ "HostedZoneId": HOSTED_ZONE_ID })),
            }),
        )
        .await;
        // The race is between two updates, so the create's own prevalidation is settled
        // first: the record is `VALID` before either update is sent.
        settle_prevalidations_now(&engine);

        let (status, accepted) = update_validation(&router, &arn, &winner).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first update must win, got {status} {accepted}"
        );
        // Hold the winner's work open, so the second update really does arrive while it
        // is in flight rather than after the wall clock closed the window.
        hold_prevalidations(&engine);
        let mid_flight = describe_validation(&router, case, &arn).await;

        let (status, refusal) = update_validation(&router, &arn, &loser).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the racing update must lose, got {status} {refusal}"
        );
        assert_eq!(
            error_code(&refusal),
            "ConflictException",
            "case {case:?}: a racing update may only lose with ConflictException, got {refusal}"
        );
        assert_eq!(
            error_message(&refusal),
            state::acme_domain_validation_concurrent_modification_message(&arn),
            "case {case:?}: expected the concurrent-modification sentence, got {refusal}"
        );
        assert_eq!(
            describe_validation(&router, case, &arn).await,
            mid_flight,
            "case {case:?}: the losing update must write nothing"
        );
        // The winner's configuration stands in full — both members, not a mixture of the
        // two requests.
        let held = details(&router, case, &arn).await;
        assert_eq!(
            held["HostedZoneId"], winner["HostedZoneId"],
            "case {case:?}: expected the winner's HostedZoneId, got {held}"
        );
        assert_eq!(
            held["DomainScope"], winner["DomainScope"],
            "case {case:?}: expected the winner's DomainScope, got {held}"
        );

        // A window, not a wall: once the winner's work falls due the very same update is
        // accepted, and what it asks for is what the record then carries.
        settle_prevalidations_now(&engine);
        let (status, response) = update_validation(&router, &arn, &loser).await;
        assert_eq!(
            status, 200,
            "case {case:?}: once the previous update settled, the retry must succeed, got \
             {status} {response}"
        );
        hold_prevalidations(&engine);
        let after = details(&router, case, &arn).await;
        assert_eq!(
            after["HostedZoneId"], loser["HostedZoneId"],
            "case {case:?}: expected the retry's HostedZoneId, got {after}"
        );
        assert_eq!(
            after["DomainScope"], loser["DomainScope"],
            "case {case:?}: expected the retry's DomainScope, got {after}"
        );
    }
}

/// RULE 3: two updates fired CONCURRENTLY at a settled validation — exactly one is
/// accepted, the other loses with the concurrency conflict, and the record ends up
/// carrying the winner's configuration.
#[tokio::test]
async fn two_concurrent_updates_leave_one_winner() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "concurrent host", &endpoint_body()).await;
    let arn = create_validation(
        &router,
        "concurrent updates",
        &json!({
            "AcmeEndpointArn": endpoint_arn,
            "DomainName": "concurrent.example.com",
            "PrevalidationOptions": options(&json!({ "HostedZoneId": HOSTED_ZONE_ID })),
        }),
    )
    .await;
    settle_prevalidations_now(&engine);
    let settled = describe_validation(&router, "concurrent updates", &arn).await;
    assert_eq!(
        settled["Status"].as_str(),
        Some(state::ACME_DOMAIN_VALIDATION_STATUS_VALID),
        "the validation must be settled before the race, got {settled}"
    );

    // Opposite configurations, so the record cannot satisfy both.
    let first = json!({
        "HostedZoneId": HOSTED_ZONE_ID,
        "DomainScope": exact_domain_scope(DOMAIN_SCOPE_OPTIONS[0]),
    });
    let second = json!({
        "HostedZoneId": OTHER_HOSTED_ZONE_ID,
        "DomainScope": exact_domain_scope(DOMAIN_SCOPE_OPTIONS[1]),
    });
    let (left, right) = tokio::join!(
        update_validation(&router, &arn, &first),
        update_validation(&router, &arn, &second),
    );

    let accepted: Vec<&Value> = [(&left, &first), (&right, &second)]
        .into_iter()
        .filter(|((status, _), _)| *status == 200)
        .map(|(_, asked)| asked)
        .collect();
    assert_eq!(
        accepted.len(),
        1,
        "case \"two concurrent updates\": exactly one racer may be accepted, got {left:?} and \
         {right:?}"
    );
    for (status, body) in [&left, &right] {
        if *status == 200 {
            continue;
        }
        assert_eq!(
            (*status, error_code(body).as_str()),
            (400, "ConflictException"),
            "case \"two concurrent updates\": the loser may only lose with ConflictException, \
             got {status} {body}"
        );
        assert_eq!(
            error_message(body),
            state::acme_domain_validation_concurrent_modification_message(&arn),
            "case \"two concurrent updates\": expected the concurrent-modification sentence, \
             got {body}"
        );
    }

    // The winner's configuration stands in full, and the status it re-opened settles on
    // the service's own clock.
    hold_prevalidations(&engine);
    let held = details(&router, "two concurrent updates", &arn).await;
    let winner = accepted[0];
    assert_eq!(
        held["HostedZoneId"], winner["HostedZoneId"],
        "case \"two concurrent updates\": expected the winner's HostedZoneId, got {held}"
    );
    assert_eq!(
        held["DomainScope"], winner["DomainScope"],
        "case \"two concurrent updates\": expected the winner's DomainScope, got {held}"
    );
    let due = settle_prevalidations_now(&engine);
    let after = describe_validation(&router, "two concurrent updates", &arn).await;
    assert_eq!(
        after["Status"].as_str(),
        Some(state::ACME_DOMAIN_VALIDATION_STATUS_VALID),
        "case \"two concurrent updates\": expected VALID once the winner's work fell due, got \
         {after}"
    );
    let due_at = due
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    assert!(
        (after["UpdatedAt"].as_f64().unwrap_or_default() - due_at).abs() < 0.01,
        "case \"two concurrent updates\": expected UpdatedAt at the instant the transition fell \
         due ({due_at}), got {after}"
    );
    assert!(
        state::ACME_DOMAIN_VALIDATION_SETTLE_DELAY > Duration::ZERO,
        "the span an update re-opens the prevalidation for must be non-zero, got {:?}",
        state::ACME_DOMAIN_VALIDATION_SETTLE_DELAY
    );
}
