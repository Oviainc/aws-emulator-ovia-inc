// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateAcmeDomainValidation` over the wire (`POST /`,
//! `X-Amz-Target: CertificateManager.UpdateAcmeDomainValidation`, awsJson1_1).
//!
//! RULE: `UpdateAcmeDomainValidation` REPLACES the prevalidation configuration of the
//! stored ACME domain validation its `AcmeDomainValidationArn` identifies with the
//! `PrevalidationOptions` the request names — leaving every member of the record the
//! operation models no input for (the ARN, the endpoint, the domain, the DNS record the
//! service minted, the resource tags and `CreatedAt`) exactly as it stands — and, because
//! domain ownership was proved against the configuration just replaced, puts the
//! validation back into `VALIDATING` and lets the SERVICE's own clock move it to `VALID`
//! a non-zero span later. A request that asks for the configuration the validation
//! already carries, or that names no `PrevalidationOptions` at all, changes nothing (the
//! operation is `@idempotent`); an ARN with no record behind it is refused
//! `ResourceNotFoundException`, and a validation ACM is itself still working through (a
//! status other than the settled `VALID`/`INVALID`) is refused `ValidationException`.
//!
//! Every write goes through `UpdateAcmeDomainValidation` and every read through
//! `DescribeAcmeDomainValidation` or `ListTagsForResource`, so a response that is not
//! computed from the persisted store cannot pass. The wire harness and the
//! create/describe/clock helpers are shared with
//! [`crate::acme_domain_validation_lifecycle`], [`crate::acme_endpoint_lifecycle`] and
//! [`crate::certificate_lifecycle`].
//!
//! The table spans what the rule generalises over: WHICH members the new configuration
//! names (neither, the hosted zone alone, the scope alone, both, and none at all), the
//! DIRECTION of the change (adding a member, dropping one, flipping one across both
//! `DomainScopeOption` values, emptying the whole configuration), and the two forms of
//! "no change" (the same configuration re-sent, and an omitted `PrevalidationOptions`) —
//! not the single configuration the stub issue quoted.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_acm_certificatemanager::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::acme_domain_validation_lifecycle::{
    create_validation, describe_validation, endpoint_body, hold_prevalidations,
    settle_prevalidations_now, DOMAIN_SCOPE_OPTIONS, HOSTED_ZONE_ID,
};
use crate::acme_endpoint_lifecycle::create_endpoint;
use crate::certificate_lifecycle::{
    call, error_code, error_message, router_with_engine, ACCOUNT, REGION,
};

/// A second `HostedZoneId` satisfying the member's modelled pattern (`^Z[A-Z0-9]+$`), so
/// a row can change the zone rather than only add or drop one.
const OTHER_HOSTED_ZONE_ID: &str = "Z1D633PJN98FT9";

/// One `UpdateAcmeDomainValidation` case: what it exercises, the domain to validate, the
/// `PrevalidationOptions` the validation is created with, the `PrevalidationOptions` the
/// update names (`None` for a request that omits the member entirely), the
/// `DnsPrevalidationOptions` the describe must report afterwards, and whether the update
/// asks for a change at all.
struct UpdateCase {
    case: &'static str,
    domain: &'static str,
    create: Value,
    update: Option<Value>,
    expect: Value,
    changed: bool,
}

/// Every member of `AcmeDomainValidation` this operation models no input member for, so
/// every one an update must leave exactly as it found it
/// (`smithy_query(query="operation", operation="UpdateAcmeDomainValidation")` beside
/// `smithy_query(query="shape", shape="com.amazonaws.acm#AcmeDomainValidation")`).
const UNTOUCHED_MEMBERS: &[&str] = &[
    "AcmeDomainValidationArn",
    "AcmeEndpointArn",
    "DomainName",
    "PrevalidationType",
    "FailureDetails",
    "CreatedAt",
];

/// A `DomainScope` naming the members given, as the wire carries it.
fn scope(exact: Option<&str>, subdomains: Option<&str>, wildcards: Option<&str>) -> Value {
    let mut members = serde_json::Map::new();
    if let Some(exact) = exact {
        members.insert("ExactDomain".into(), json!(exact));
    }
    if let Some(subdomains) = subdomains {
        members.insert("Subdomains".into(), json!(subdomains));
    }
    if let Some(wildcards) = wildcards {
        members.insert("Wildcards".into(), json!(wildcards));
    }
    Value::Object(members)
}

/// A `PrevalidationOptions` union value carrying `options` as its `DnsPrevalidation`
/// member — the one member the union declares.
fn options(options: &Value) -> Value {
    json!({ "DnsPrevalidation": options })
}

/// Send `UpdateAcmeDomainValidation` for `arn`, naming `update`'s prevalidation options
/// (or none at all), and return `(status, body)`.
async fn update_validation(
    router: &MultiServiceRouter,
    arn: &str,
    update: Option<&Value>,
) -> (u16, Value) {
    let mut body = json!({ "AcmeDomainValidationArn": arn });
    if let Some(update) = update {
        body["PrevalidationOptions"] = options(update);
    }
    call(router, "UpdateAcmeDomainValidation", &body).await
}

/// The resource tags `ListTagsForResource` reports for `arn`.
async fn resource_tags(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let (status, response) =
        call(router, "ListTagsForResource", &json!({ "ResourceArn": arn })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource({arn}) must succeed, got {status} {response}"
    );
    response["Tags"].clone()
}

/// One row per class of update the rule covers.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "no configuration at all gains a hosted zone",
            domain: "apex.example.com",
            create: json!({}),
            update: Some(json!({ "HostedZoneId": HOSTED_ZONE_ID })),
            expect: json!({ "HostedZoneId": HOSTED_ZONE_ID }),
            changed: true,
        },
        UpdateCase {
            case: "the hosted zone is replaced by another one",
            domain: "zone.example.com",
            create: json!({ "HostedZoneId": HOSTED_ZONE_ID }),
            update: Some(json!({ "HostedZoneId": OTHER_HOSTED_ZONE_ID })),
            expect: json!({ "HostedZoneId": OTHER_HOSTED_ZONE_ID }),
            changed: true,
        },
        UpdateCase {
            // The member carries the configuration as a WHOLE, so a new configuration
            // that names only a scope drops the hosted zone the old one carried.
            case: "the hosted zone gives way to a fully ENABLED scope",
            domain: "scoped.example.com",
            create: json!({ "HostedZoneId": HOSTED_ZONE_ID }),
            update: Some(json!({
                "DomainScope": scope(
                    Some(DOMAIN_SCOPE_OPTIONS[0]),
                    Some(DOMAIN_SCOPE_OPTIONS[0]),
                    Some(DOMAIN_SCOPE_OPTIONS[0]),
                ),
            })),
            expect: json!({
                "DomainScope": scope(
                    Some(DOMAIN_SCOPE_OPTIONS[0]),
                    Some(DOMAIN_SCOPE_OPTIONS[0]),
                    Some(DOMAIN_SCOPE_OPTIONS[0]),
                ),
            }),
            changed: true,
        },
        UpdateCase {
            // Both `DomainScopeOption` values the model declares, one flipped to the
            // other, with the hosted zone carried across unchanged.
            case: "a scope member flips from ENABLED to DISABLED beside an unchanged zone",
            domain: "flip.example.com",
            create: json!({
                "DomainScope": scope(Some(DOMAIN_SCOPE_OPTIONS[0]), None, None),
                "HostedZoneId": HOSTED_ZONE_ID,
            }),
            update: Some(json!({
                "DomainScope": scope(Some(DOMAIN_SCOPE_OPTIONS[1]), None, None),
                "HostedZoneId": HOSTED_ZONE_ID,
            })),
            expect: json!({
                "DomainScope": scope(Some(DOMAIN_SCOPE_OPTIONS[1]), None, None),
                "HostedZoneId": HOSTED_ZONE_ID,
            }),
            changed: true,
        },
        UpdateCase {
            // A scope member a request does not name is absent, not defaulted, so adding
            // one to a scope is a change even when the one already there is unchanged.
            case: "a scope gains a second member",
            domain: "widened.example.com",
            create: json!({
                "DomainScope": scope(Some(DOMAIN_SCOPE_OPTIONS[0]), None, None),
            }),
            update: Some(json!({
                "DomainScope": scope(
                    Some(DOMAIN_SCOPE_OPTIONS[0]),
                    None,
                    Some(DOMAIN_SCOPE_OPTIONS[1]),
                ),
            })),
            expect: json!({
                "DomainScope": scope(
                    Some(DOMAIN_SCOPE_OPTIONS[0]),
                    None,
                    Some(DOMAIN_SCOPE_OPTIONS[1]),
                ),
            }),
            changed: true,
        },
        UpdateCase {
            case: "the whole configuration is emptied",
            domain: "emptied.example.com",
            create: json!({
                "DomainScope": scope(Some(DOMAIN_SCOPE_OPTIONS[1]), Some(DOMAIN_SCOPE_OPTIONS[1]), None),
                "HostedZoneId": HOSTED_ZONE_ID,
            }),
            update: Some(json!({})),
            expect: json!({}),
            changed: true,
        },
        UpdateCase {
            // An EMPTY scope is a scope: a configuration that carries one is not the
            // same as one that carries none, so this update changes something.
            case: "an absent scope becomes an empty one",
            domain: "present.example.com",
            create: json!({ "HostedZoneId": HOSTED_ZONE_ID }),
            update: Some(json!({ "DomainScope": {}, "HostedZoneId": HOSTED_ZONE_ID })),
            expect: json!({ "DomainScope": {}, "HostedZoneId": HOSTED_ZONE_ID }),
            changed: true,
        },
        UpdateCase {
            case: "the configuration the validation already carries, re-sent",
            domain: "same.example.com",
            create: json!({
                "DomainScope": scope(Some(DOMAIN_SCOPE_OPTIONS[0]), Some(DOMAIN_SCOPE_OPTIONS[1]), None),
                "HostedZoneId": HOSTED_ZONE_ID,
            }),
            update: Some(json!({
                "DomainScope": scope(Some(DOMAIN_SCOPE_OPTIONS[0]), Some(DOMAIN_SCOPE_OPTIONS[1]), None),
                "HostedZoneId": HOSTED_ZONE_ID,
            })),
            expect: json!({
                "DomainScope": scope(Some(DOMAIN_SCOPE_OPTIONS[0]), Some(DOMAIN_SCOPE_OPTIONS[1]), None),
                "HostedZoneId": HOSTED_ZONE_ID,
            }),
            changed: false,
        },
        UpdateCase {
            case: "no PrevalidationOptions named at all",
            domain: "untouched.example.com",
            create: json!({ "HostedZoneId": HOSTED_ZONE_ID }),
            update: None,
            expect: json!({ "HostedZoneId": HOSTED_ZONE_ID }),
            changed: false,
        },
    ]
}

/// The `DnsPrevalidation` details of a described validation.
fn details(validation: &Value) -> Value {
    validation["PrevalidationDetails"]["DnsPrevalidation"].clone()
}

/// RULE: for every row of the table, `UpdateAcmeDomainValidation` replaces the stored
/// prevalidation configuration with the one the request names, leaves the rest of the
/// record alone, re-opens the prevalidation (`VALIDATING`) exactly when the
/// configuration actually changed, and `DescribeAcmeDomainValidation` reports the result.
#[tokio::test]
async fn update_replaces_the_prevalidation_configuration_the_request_names() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "update host", &endpoint_body()).await;

    for case in update_cases() {
        let label = case.case;
        let arn = create_validation(
            &router,
            label,
            &json!({
                "AcmeEndpointArn": endpoint_arn,
                "DomainName": case.domain,
                "PrevalidationOptions": options(&case.create),
                "Tags": [{ "Key": "owner", "Value": "équipe-sécurité" }],
            }),
        )
        .await;

        // This operation applies to a SETTLED validation, so the prevalidation the create
        // owes this one is brought due first: the instant moves, the transition itself is
        // still applied by the emulator on the next request.
        settle_prevalidations_now(&engine);
        let before = describe_validation(&router, label, &arn).await;
        assert_eq!(
            before["Status"].as_str(),
            Some(state::ACME_DOMAIN_VALIDATION_STATUS_VALID),
            "case {label:?}: the validation must have settled before the update, got {before}"
        );
        let tags_before = resource_tags(&router, label, &arn).await;

        let (status, response) = update_validation(&router, &arn, case.update.as_ref()).await;
        assert_eq!(
            status, 200,
            "case {label:?}: UpdateAcmeDomainValidation must succeed, got {status} {response}"
        );
        // The operation's output shape is `Unit`: a success carries no member.
        assert!(
            response
                .as_object()
                .is_none_or(|members| members.is_empty()),
            "case {label:?}: UpdateAcmeDomainValidation models no output member, got {response}"
        );

        // Pin the transient status the update owes this validation, so `VALIDATING`
        // cannot race the wall clock while the record is read back.
        hold_prevalidations(&engine);
        let after = describe_validation(&router, label, &arn).await;
        assert_eq!(
            details(&after)["HostedZoneId"],
            case.expect["HostedZoneId"],
            "case {label:?}: expected HostedZoneId {}, got {after}",
            case.expect["HostedZoneId"]
        );
        assert_eq!(
            details(&after)["DomainScope"],
            case.expect["DomainScope"],
            "case {label:?}: expected DomainScope {}, got {after}",
            case.expect["DomainScope"]
        );

        for member in UNTOUCHED_MEMBERS {
            assert_eq!(
                after[member], before[member],
                "case {label:?}: {member} is not this operation's to change, expected {}, got {}",
                before[member], after[member]
            );
        }
        // The DNS record the caller publishes is minted from the validation's own
        // identity and its domain, neither of which this operation can change.
        assert_eq!(
            details(&after)["ResourceRecord"],
            details(&before)["ResourceRecord"],
            "case {label:?}: the server-managed resource record must stand, got {after}"
        );
        assert_eq!(
            resource_tags(&router, label, &arn).await,
            tags_before,
            "case {label:?}: the update must leave the validation's resource tags alone"
        );

        let updated_before = before["UpdatedAt"].as_f64().unwrap_or_default();
        let updated_after = after["UpdatedAt"].as_f64().unwrap_or_default();
        if case.changed {
            // The configuration the domain was proved against is gone, so ownership is
            // unproven under the new one and the SERVICE re-opens the prevalidation.
            assert_eq!(
                after["Status"].as_str(),
                Some(state::ACME_DOMAIN_VALIDATION_STATUS_VALIDATING),
                "case {label:?}: a changed configuration must re-open the prevalidation, got {after}"
            );
            assert!(
                updated_after > updated_before,
                "case {label:?}: a configuration change must move UpdatedAt past {updated_before}, got {updated_after}"
            );

            // Nothing the caller does moves the status on: the clock the update fixed
            // does, and the record is dated at the instant the transition fell due.
            let due = settle_prevalidations_now(&engine);
            let settled = describe_validation(&router, label, &arn).await;
            assert_eq!(
                settled["Status"].as_str(),
                Some(state::ACME_DOMAIN_VALIDATION_STATUS_VALID),
                "case {label:?}: expected {} once the prevalidation fell due, got {settled}",
                state::ACME_DOMAIN_VALIDATION_STATUS_VALID
            );
            let due_at = due
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64();
            assert!(
                (settled["UpdatedAt"].as_f64().unwrap_or_default() - due_at).abs() < 0.01,
                "case {label:?}: expected UpdatedAt to be the instant the transition fell due ({due_at}), got {settled}"
            );
        } else {
            // A request that asks for nothing new is a request a client may repeat: the
            // record, its status and its `UpdatedAt` stand exactly as they were.
            assert_eq!(
                after, before,
                "case {label:?}: an update that asks for no change must leave the record alone, got {after}"
            );
        }

        // `@idempotent`: the same update sent again writes the configuration the
        // validation now carries, so nothing moves — the status included.
        let settled = describe_validation(&router, label, &arn).await;
        let (status, response) = update_validation(&router, &arn, case.update.as_ref()).await;
        assert_eq!(
            status, 200,
            "case {label:?}: re-sending the update must succeed, got {status} {response}"
        );
        let again = describe_validation(&router, label, &arn).await;
        assert_eq!(
            again, settled,
            "case {label:?}: re-sending the same update must change nothing, got {again}"
        );
    }

    // Each row's validation holds its OWN configuration: one update reached one record.
    for case in update_cases() {
        let store = engine.state::<state::ComAmazonawsAcmCertificatemanagerState>();
        let arn = {
            let held = store.lock();
            held.acme_domain_validation_for(&endpoint_arn, case.domain)
                .map(|validation| validation.arn.clone())
        }
        .unwrap_or_else(|| panic!("case {:?}: the validation must still be stored", case.case));
        let validation = describe_validation(&router, case.case, &arn).await;
        assert_eq!(
            details(&validation)["HostedZoneId"],
            case.expect["HostedZoneId"],
            "case {:?}: every other validation's update must have left this one alone, got {validation}",
            case.case
        );
        assert_eq!(
            details(&validation)["DomainScope"],
            case.expect["DomainScope"],
            "case {:?}: every other validation's update must have left this one alone, got {validation}",
            case.case
        );
    }
}

/// RULE: an `AcmeDomainValidationArn` with no record behind it is refused
/// `ResourceNotFoundException` (HTTP 400) and nothing is created for it.
#[tokio::test]
async fn update_of_an_unknown_validation_is_not_found() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "missing host", &endpoint_body()).await;
    // One validation really does exist, so each refusal below is about the ARN the
    // request named rather than about an empty store.
    let existing = create_validation(
        &router,
        "a stored validation",
        &json!({
            "AcmeEndpointArn": endpoint_arn,
            "DomainName": "stored.example.com",
            "PrevalidationOptions": options(&json!({ "HostedZoneId": HOSTED_ZONE_ID })),
        }),
    )
    .await;
    hold_prevalidations(&engine);

    // Well-formed `AcmeDomainValidationArn` values (the modelled `@pattern` is
    // `^arn:aws[a-z-]*:acm:[a-z0-9-]+:[0-9]{12}:acme-endpoint/[a-zA-Z0-9-]+/acme-domain-validation/[a-zA-Z0-9-]+$`)
    // that name no stored validation: an id nothing minted, the stored id under another
    // endpoint, in another Region and under another account, and the longest id the
    // member's 200-character length allows.
    let (endpoint_path, validation_id) = existing
        .rsplit_once("/acme-domain-validation/")
        .expect("the created ARN is an ACME domain validation ARN");
    let endpoint_id = endpoint_path
        .rsplit_once("acme-endpoint/")
        .expect("the endpoint ARN carries an endpoint id")
        .1;
    let prefix = format!("arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/{endpoint_id}/acme-domain-validation/");
    let long_id = "a".repeat(200 - prefix.len());
    let missing = [
        (
            "an id nothing minted",
            format!("{prefix}0000-no-such-validation"),
        ),
        (
            "the stored id under another endpoint",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/no-such-endpoint/acme-domain-validation/{validation_id}"),
        ),
        (
            "the stored id in another Region",
            format!("arn:aws:acm:eu-west-1:{ACCOUNT}:acme-endpoint/{endpoint_id}/acme-domain-validation/{validation_id}"),
        ),
        (
            "the stored id under another account",
            format!("arn:aws:acm:{REGION}:123456789012:acme-endpoint/{endpoint_id}/acme-domain-validation/{validation_id}"),
        ),
        (
            "an id at AcmeDomainValidationArn's maximum length",
            format!("{prefix}{long_id}"),
        ),
    ];

    let stored_before = describe_validation(&router, "a stored validation", &existing).await;
    for (case, arn) in missing {
        let (status, response) = update_validation(
            &router,
            &arn,
            Some(&json!({ "HostedZoneId": OTHER_HOSTED_ZONE_ID })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: updating {arn} must be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {response}"
        );
        assert_eq!(
            error_message(&response),
            state::acme_domain_validation_not_found_message(&arn),
            "case {case:?}: expected the service's not-found sentence, got {response}"
        );

        // The refusal created nothing, and left the validation that does exist alone.
        let (status, response) = call(
            &router,
            "DescribeAcmeDomainValidation",
            &json!({ "AcmeDomainValidationArn": arn }),
        )
        .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: a refused update must create no validation for {arn}, got {status} {response}"
        );
        hold_prevalidations(&engine);
        let stored = describe_validation(&router, case, &existing).await;
        assert_eq!(
            stored, stored_before,
            "case {case:?}: a refused update must leave the stored validation alone, got {stored}"
        );
    }
}

/// RULE: a validation ACM is itself still working through — a status other than the
/// settled `VALID` and `INVALID`, which for a freshly created validation is the
/// `VALIDATING` its prevalidation has not yet left — is refused `ValidationException`
/// (HTTP 400) and keeps the configuration it carries; once the prevalidation falls due
/// the very same update is accepted.
#[tokio::test]
async fn update_of_an_unsettled_validation_is_refused() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "conflict host", &endpoint_body()).await;

    // Both forms of update, so the refusal is about the validation's state and not about
    // which member the request named.
    let rows = [
        (
            "a hosted zone while VALIDATING",
            "zone.conflict.example.com",
            json!({ "HostedZoneId": OTHER_HOSTED_ZONE_ID }),
        ),
        (
            "a domain scope while VALIDATING",
            "scope.conflict.example.com",
            json!({
                "DomainScope": scope(Some(DOMAIN_SCOPE_OPTIONS[1]), None, None),
            }),
        ),
    ];

    for (case, domain, update) in rows {
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
        // Pin the transient status: the prevalidation this create owes the validation is
        // pushed out of the test's way, so `VALIDATING` cannot race the wall clock.
        hold_prevalidations(&engine);
        let validating = describe_validation(&router, case, &arn).await;
        assert_eq!(
            validating["Status"].as_str(),
            Some(state::ACME_DOMAIN_VALIDATION_STATUS_VALIDATING),
            "case {case:?}: the validation must still be VALIDATING, got {validating}"
        );

        let (status, response) = update_validation(&router, &arn, Some(&update)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: updating an unsettled validation must be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response}"
        );
        assert!(
            error_message(&response).contains(state::ACME_DOMAIN_VALIDATION_STATUS_VALIDATING),
            "case {case:?}: the refusal must name the status that blocks the update, got {response}"
        );

        // The refusal wrote nothing: the record still carries what the create stored.
        let unchanged = describe_validation(&router, case, &arn).await;
        assert_eq!(
            unchanged, validating,
            "case {case:?}: a refused update must leave the record alone, got {unchanged}"
        );

        // Bring the prevalidation due and the very same update is accepted.
        settle_prevalidations_now(&engine);
        let (status, response) = update_validation(&router, &arn, Some(&update)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: once the validation is VALID the same update must succeed, got {status} {response}"
        );
        hold_prevalidations(&engine);
        let updated = describe_validation(&router, case, &arn).await;
        assert_eq!(
            details(&updated)["HostedZoneId"],
            update["HostedZoneId"],
            "case {case:?}: expected the accepted update's HostedZoneId to be stored, got {updated}"
        );
        assert_eq!(
            details(&updated)["DomainScope"],
            update["DomainScope"],
            "case {case:?}: expected the accepted update's DomainScope to be stored, got {updated}"
        );
    }
}

/// RULE: the span the re-opened prevalidation takes is a non-zero one a caller can
/// observe, and nothing sleeps for it — so an update answers immediately, a caller that
/// reads straight afterwards sees `VALIDATING`, and the `VALID` transition arrives on the
/// clock the update fixed.
#[tokio::test]
async fn the_reopened_prevalidation_settles_on_the_clock_the_update_fixed() {
    assert!(
        state::ACME_DOMAIN_VALIDATION_SETTLE_DELAY >= Duration::from_millis(250)
            && state::ACME_DOMAIN_VALIDATION_SETTLE_DELAY <= Duration::from_millis(750),
        "the prevalidation span must be a non-zero span a caller can observe, got {:?}",
        state::ACME_DOMAIN_VALIDATION_SETTLE_DELAY
    );

    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "settling host", &endpoint_body()).await;
    let arn = create_validation(
        &router,
        "re-opened prevalidation",
        &json!({
            "AcmeEndpointArn": endpoint_arn,
            "DomainName": "reopened.example.com",
            "PrevalidationOptions": options(&json!({ "HostedZoneId": HOSTED_ZONE_ID })),
        }),
    )
    .await;
    settle_prevalidations_now(&engine);

    // The update returns without waiting the span out, and the status it leaves behind is
    // the transient one.
    let started = SystemTime::now();
    let (status, response) = update_validation(
        &router,
        &arn,
        Some(&json!({ "HostedZoneId": OTHER_HOSTED_ZONE_ID })),
    )
    .await;
    let elapsed = started.elapsed().unwrap_or_default();
    assert_eq!(
        status, 200,
        "the update must succeed, got {status} {response}"
    );
    assert!(
        elapsed < state::ACME_DOMAIN_VALIDATION_SETTLE_DELAY,
        "the update must not sleep for the prevalidation span, it took {elapsed:?}"
    );

    let pending = describe_validation(&router, "re-opened prevalidation", &arn).await;
    assert_eq!(
        pending["Status"].as_str(),
        Some(state::ACME_DOMAIN_VALIDATION_STATUS_VALIDATING),
        "expected the transient status before the span passes, got {pending}"
    );

    // Waiting the real span out — the emulator stores the due instant and never sleeps
    // for it — is all the caller does, and the service has moved the status on.
    std::thread::sleep(state::ACME_DOMAIN_VALIDATION_SETTLE_DELAY + Duration::from_millis(100));
    let settled = describe_validation(&router, "re-opened prevalidation", &arn).await;
    assert_eq!(
        settled["Status"].as_str(),
        Some(state::ACME_DOMAIN_VALIDATION_STATUS_VALID),
        "expected VALID once the span the update fixed has passed, got {settled}"
    );
    assert_eq!(
        details(&settled)["HostedZoneId"].as_str(),
        Some(OTHER_HOSTED_ZONE_ID),
        "the settled validation must still carry the updated configuration, got {settled}"
    );
}
