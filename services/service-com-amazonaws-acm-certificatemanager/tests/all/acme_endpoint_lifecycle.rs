// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for ACM's managed ACME endpoint over the wire (`POST /`,
//! `X-Amz-Target: CertificateManager.<Op>`, awsJson1_1).
//!
//! RULE: `CreateAcmeEndpoint` persists one ACME endpoint per call — minting its ARN
//! and its server-managed directory URL, recording the authorization behavior, the
//! contact requirement, the certificate authority and the certificate tags the request
//! configured, and starting it in `CREATING` — and `DescribeAcmeEndpoint` reports that
//! stored record rather than echoing any request of its own.
//!
//! Every assertion goes through [`MultiServiceRouter::dispatch`], the same path the
//! emulator serves production traffic on, and every read goes through a DIFFERENT
//! operation than the write, so a response that is not computed from the persisted
//! store cannot pass. The wire harness is shared with [`crate::certificate_lifecycle`].
//!
//! The table below spans the classes the rule generalises over: both `AcmeContact`
//! values and the member left absent so the model's default applies, a certificate
//! authority with no allowed algorithm, with one, and with every published one, and tag
//! sets of several sizes and scripts — not the single configuration the stub issue
//! quoted.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_acm_certificatemanager::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_lifecycle::{
    call, call_in_region, error_code, error_message, router, router_with_engine, ACCOUNT, REGION,
};

/// Every `PublicKeyAlgorithm` the model declares
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#PublicKeyAlgorithm")`).
pub(crate) const PUBLIC_KEY_ALGORITHMS: &[&str] = &["RSA_2048", "EC_prime256v1", "EC_secp384r1"];

/// Every `AcmeContact` the model declares
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#AcmeContact")`); `REQUIRED`
/// is also the member's modelled default.
pub(crate) const CONTACTS: &[&str] = &["REQUIRED", "NOT_REQUIRED"];

/// Every `AcmeAuthorizationBehavior` the model declares
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#AcmeAuthorizationBehavior")`).
pub(crate) const AUTHORIZATION_BEHAVIORS: &[&str] = &["PRE_APPROVED"];

/// One `CreateAcmeEndpoint` case: what it exercises, the request body, and the
/// `Contact` the stored endpoint must report (the request's, or the modelled default
/// when the request named none).
pub(crate) type EndpointCase<'a> = (&'a str, Value, &'a str);

/// `CreateAcmeEndpoint`, asserted to succeed, returning the ARN it minted.
pub(crate) async fn create_endpoint(
    router: &MultiServiceRouter,
    case: &str,
    body: &Value,
) -> String {
    let (status, response) = call(router, "CreateAcmeEndpoint", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateAcmeEndpoint must succeed, got {status} {response}"
    );
    response["AcmeEndpointArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateAcmeEndpoint returned no AcmeEndpointArn"))
        .to_string()
}

/// `DescribeAcmeEndpoint`'s `AcmeEndpoint` member, asserted to succeed.
pub(crate) async fn describe_endpoint(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> Value {
    let (status, response) = call(
        router,
        "DescribeAcmeEndpoint",
        &json!({ "AcmeEndpointArn": arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeAcmeEndpoint({arn}) must succeed, got {status} {response}"
    );
    response["AcmeEndpoint"].clone()
}

/// The endpoint id of an ACME endpoint ARN: the part after `acme-endpoint/`.
fn endpoint_id(arn: &str) -> &str {
    arn.rsplit_once("acme-endpoint/")
        .unwrap_or_else(|| panic!("{arn} is not an ACME endpoint ARN"))
        .1
}

/// Whether `arn` satisfies `AcmeEndpointArn`'s modelled `@pattern`
/// (`^arn:aws[a-z-]*:acm:[a-z0-9-]+:[0-9]{12}:acme-endpoint/[a-zA-Z0-9-]+$`) and its
/// modelled length (1..200) for this test's Region and the emulator's account.
fn is_well_formed_endpoint_arn(arn: &str) -> bool {
    let prefix = format!("arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/");
    let Some(id) = arn.strip_prefix(&prefix) else {
        return false;
    };
    !arn.is_empty()
        && arn.len() <= 200
        && !id.is_empty()
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// A `TagList` on the wire as `(key, value)` pairs in key order.
fn tag_pairs(tags: &Value) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = tags
        .as_array()
        .unwrap_or_else(|| panic!("{tags} is not a TagList"))
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    pairs.sort();
    pairs
}

/// Hold every stored ACME endpoint in `CREATING` for the rest of the test by moving the
/// instant its activation falls due an hour out.
///
/// This writes only the due instant the emulator itself stores; the transition is still
/// materialised by `materialize_due` on the next request, exactly as in production. A
/// test about the transient state therefore cannot be made flaky by the
/// [`state::ACME_ENDPOINT_ACTIVATION_DELAY`] span elapsing mid-test.
pub(crate) fn hold_activations(engine: &Arc<engine::InMemoryEngine>) {
    let store = engine.state::<state::ComAmazonawsAcmCertificatemanagerState>();
    let mut store = store.lock();
    let held = SystemTime::now() + Duration::from_secs(3600);
    for endpoint in store.acme_endpoints.values_mut() {
        if let Some(pending) = endpoint.pending_status.as_mut() {
            pending.due_at = held;
        }
    }
}

/// Put every stored ACME endpoint's activation back where the create fixed it:
/// [`state::ACME_ENDPOINT_ACTIVATION_DELAY`] after the endpoint was created, which is
/// the instant the handler itself stored. Answers the latest instant one of them falls
/// due, so a caller can wait the real span out.
pub(crate) fn restore_activations(engine: &Arc<engine::InMemoryEngine>) -> SystemTime {
    let store = engine.state::<state::ComAmazonawsAcmCertificatemanagerState>();
    let mut store = store.lock();
    let mut latest = SystemTime::UNIX_EPOCH;
    for endpoint in store.acme_endpoints.values_mut() {
        let created_at = endpoint.created_at;
        if let Some(pending) = endpoint.pending_status.as_mut() {
            pending.due_at = created_at + state::ACME_ENDPOINT_ACTIVATION_DELAY;
            latest = latest.max(pending.due_at);
        }
    }
    latest
}

/// Wait until `due` has passed on the real clock. The emulator never sleeps for a due
/// instant — this is the test standing in for the time a caller would let pass before
/// polling again.
pub(crate) fn wait_until(due: SystemTime) {
    if let Ok(remaining) = due.duration_since(SystemTime::now()) {
        std::thread::sleep(remaining + Duration::from_millis(50));
    }
}

/// One request body for each row of the table.
pub(crate) fn endpoint_cases() -> Vec<EndpointCase<'static>> {
    // 50 is `TagList`'s modelled maximum length, so this row sits exactly on the bound.
    let many_tags: Vec<Value> = (0..50)
        .map(|index| json!({ "Key": format!("key-{index}"), "Value": format!("value-{index}") }))
        .collect();
    vec![
        (
            "no allowed algorithms, contact required",
            json!({
                "AuthorizationBehavior": AUTHORIZATION_BEHAVIORS[0],
                "Contact": "REQUIRED",
                "CertificateAuthority": { "PublicCertificateAuthority": {} },
            }),
            "REQUIRED",
        ),
        (
            "one allowed algorithm, contact not required",
            json!({
                "AuthorizationBehavior": AUTHORIZATION_BEHAVIORS[0],
                "Contact": "NOT_REQUIRED",
                "CertificateAuthority": {
                    "PublicCertificateAuthority": { "AllowedKeyAlgorithms": ["EC_secp384r1"] },
                },
            }),
            "NOT_REQUIRED",
        ),
        (
            "every published allowed algorithm",
            json!({
                "AuthorizationBehavior": AUTHORIZATION_BEHAVIORS[0],
                "Contact": "REQUIRED",
                "CertificateAuthority": {
                    "PublicCertificateAuthority": { "AllowedKeyAlgorithms": PUBLIC_KEY_ALGORITHMS },
                },
            }),
            "REQUIRED",
        ),
        (
            "Contact absent, so the modelled default applies",
            json!({
                "AuthorizationBehavior": AUTHORIZATION_BEHAVIORS[0],
                "CertificateAuthority": { "PublicCertificateAuthority": {} },
            }),
            "REQUIRED",
        ),
        (
            "resource tags and certificate tags, non-ASCII values",
            json!({
                "AuthorizationBehavior": AUTHORIZATION_BEHAVIORS[0],
                "Contact": "NOT_REQUIRED",
                "CertificateAuthority": {
                    "PublicCertificateAuthority": { "AllowedKeyAlgorithms": ["RSA_2048"] },
                },
                "Tags": [{ "Key": "owner", "Value": "équipe-sécurité" }],
                "CertificateTags": [
                    { "Key": "環境", "Value": "本番" },
                    { "Key": "team", "Value": "" },
                ],
            }),
            "NOT_REQUIRED",
        ),
        (
            "certificate tags at TagList's maximum length",
            json!({
                "AuthorizationBehavior": AUTHORIZATION_BEHAVIORS[0],
                "Contact": "REQUIRED",
                "CertificateAuthority": { "PublicCertificateAuthority": {} },
                "CertificateTags": many_tags,
            }),
            "REQUIRED",
        ),
    ]
}

/// RULE: `DescribeAcmeEndpoint` reports the endpoint `CreateAcmeEndpoint` persisted —
/// its minted ARN, its server-managed URL, the `CREATING` status it starts in and every
/// configuration member the request carried — for every row of the table above.
#[tokio::test]
async fn describe_reports_what_create_acme_endpoint_stored() {
    let (router, engine) = router_with_engine();
    let mut seen_arns: BTreeSet<String> = BTreeSet::new();
    let mut seen_urls: BTreeSet<String> = BTreeSet::new();

    for (case, body, expected_contact) in endpoint_cases() {
        let arn = create_endpoint(&router, case, &body).await;
        // Pin the transient status: the activation the create owes this endpoint is
        // pushed out of the test's way, so `CREATING` cannot race the wall clock.
        hold_activations(&engine);

        assert!(
            is_well_formed_endpoint_arn(&arn),
            "case {case:?}: the minted ARN must match AcmeEndpointArn's pattern, got {arn:?}"
        );
        assert!(
            seen_arns.insert(arn.clone()),
            "case {case:?}: every create must mint its own ARN, but {arn} was minted twice"
        );

        let endpoint = describe_endpoint(&router, case, &arn).await;
        assert_eq!(
            endpoint["AcmeEndpointArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected the described ARN to be {arn}, got {endpoint}"
        );
        assert_eq!(
            endpoint["Status"].as_str(),
            Some(state::ACME_ENDPOINT_STATUS_CREATING),
            "case {case:?}: expected a freshly created endpoint to be {}, got {endpoint}",
            state::ACME_ENDPOINT_STATUS_CREATING
        );
        assert!(
            endpoint["FailureReason"].is_null(),
            "case {case:?}: nothing failed, so FailureReason must be absent, got {endpoint}"
        );
        assert_eq!(
            endpoint["AuthorizationBehavior"].as_str(),
            body["AuthorizationBehavior"].as_str(),
            "case {case:?}: expected the stored AuthorizationBehavior to be reported, got {endpoint}"
        );
        assert_eq!(
            endpoint["Contact"].as_str(),
            Some(expected_contact),
            "case {case:?}: expected Contact {expected_contact}, got {endpoint}"
        );
        assert_eq!(
            endpoint["CertificateAuthority"]["PublicCertificateAuthority"]["AllowedKeyAlgorithms"],
            body["CertificateAuthority"]["PublicCertificateAuthority"]["AllowedKeyAlgorithms"],
            "case {case:?}: expected the stored certificate authority to be reported, got {endpoint}"
        );

        // `CertificateTags` is reported exactly when the create named some: `TagList`'s
        // minimum length is 1, so an absent member cannot come back as an empty list.
        match body.get("CertificateTags") {
            Some(requested) => assert_eq!(
                tag_pairs(&endpoint["CertificateTags"]),
                tag_pairs(requested),
                "case {case:?}: expected the stored CertificateTags to be reported, got {endpoint}"
            ),
            None => assert!(
                endpoint["CertificateTags"].is_null(),
                "case {case:?}: a create that named no CertificateTags must report none, got {endpoint}"
            ),
        }

        // The endpoint URL is the ACME directory a client starts from, derived from the
        // endpoint's own identity: unique per endpoint and carrying nothing the caller
        // sent.
        let url = endpoint["EndpointUrl"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the endpoint carried no EndpointUrl"))
            .to_string();
        assert!(
            url.starts_with("https://") && url.ends_with("/directory"),
            "case {case:?}: expected an https ACME directory URL, got {url:?}"
        );
        assert!(
            url.contains(endpoint_id(&arn)),
            "case {case:?}: expected {url:?} to be derived from the endpoint id {}",
            endpoint_id(&arn)
        );
        assert!(
            seen_urls.insert(url.clone()),
            "case {case:?}: every endpoint must get its own URL, but {url} was served twice"
        );

        let created_at = endpoint["CreatedAt"].as_f64().unwrap_or_default();
        let updated_at = endpoint["UpdatedAt"].as_f64().unwrap_or_default();
        assert!(
            created_at > 0.0,
            "case {case:?}: expected a CreatedAt timestamp, got {endpoint}"
        );
        assert_eq!(
            created_at, updated_at,
            "case {case:?}: an endpoint nothing has changed yet must report CreatedAt as its UpdatedAt, got {endpoint}"
        );

        // Reading must not change what a later caller sees.
        let reread = describe_endpoint(&router, case, &arn).await;
        assert_eq!(
            reread, endpoint,
            "case {case:?}: a second describe must report the same record, got {reread}"
        );
    }
}

/// RULE: the endpoint URL is server-managed — two identical requests get two different
/// endpoints with two different URLs, and no value the caller sent appears in either.
#[tokio::test]
async fn endpoint_url_is_server_managed() {
    let router = router();
    // One request body, sent twice: nothing in it distinguishes the two endpoints, so
    // anything shared between their URLs would have to come from the request.
    let body = json!({
        "AuthorizationBehavior": AUTHORIZATION_BEHAVIORS[0],
        "Contact": CONTACTS[0],
        "CertificateAuthority": { "PublicCertificateAuthority": {} },
        "Tags": [{ "Key": "marker", "Value": "client-supplied-marker" }],
        "CertificateTags": [{ "Key": "cert-marker", "Value": "client-supplied-cert-marker" }],
    });

    let mut urls = Vec::new();
    for attempt in 0..2 {
        let case = format!("identical request #{attempt}");
        let arn = create_endpoint(&router, &case, &body).await;
        let endpoint = describe_endpoint(&router, &case, &arn).await;
        let url = endpoint["EndpointUrl"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the endpoint carried no EndpointUrl"))
            .to_string();
        for marker in ["client-supplied-marker", "client-supplied-cert-marker", "marker"] {
            assert!(
                !url.contains(marker),
                "case {case:?}: a server-managed URL must carry nothing the caller sent, but {url:?} contains {marker:?}"
            );
        }
        assert!(
            url.contains(endpoint_id(&arn)) && url.contains(REGION),
            "case {case:?}: expected {url:?} to be derived from the endpoint id and Region"
        );
        urls.push(url);
    }
    assert_ne!(
        urls[0], urls[1],
        "case identical requests: two endpoints must not share one directory URL, got {urls:?}"
    );
}

/// RULE: an endpoint starts in `CREATING` and reaches `ACTIVE` when the activation the
/// create owed it falls due — a transition the clock makes, which the next read
/// materialises, and which no read undoes.
#[tokio::test]
async fn creating_endpoint_becomes_active_when_the_clock_owes_it() {
    // The span is chosen, but it must be observable: a caller that describes the
    // endpoint straight after creating it has to be able to see `CREATING`.
    let observable = Duration::from_millis(250)..=Duration::from_millis(750);
    assert!(
        observable.contains(&state::ACME_ENDPOINT_ACTIVATION_DELAY),
        "case activation span: expected a span in {observable:?}, got {:?}",
        state::ACME_ENDPOINT_ACTIVATION_DELAY
    );

    let (router, engine) = router_with_engine();
    let case = "creating -> active";
    let body = json!({
        "AuthorizationBehavior": AUTHORIZATION_BEHAVIORS[0],
        "Contact": CONTACTS[1],
        "CertificateAuthority": {
            "PublicCertificateAuthority": { "AllowedKeyAlgorithms": [PUBLIC_KEY_ALGORITHMS[0]] },
        },
    });
    let arn = create_endpoint(&router, case, &body).await;

    hold_activations(&engine);
    let creating = describe_endpoint(&router, case, &arn).await;
    assert_eq!(
        creating["Status"].as_str(),
        Some(state::ACME_ENDPOINT_STATUS_CREATING),
        "case {case:?}: before the activation falls due the endpoint must still be CREATING, got {creating}"
    );

    // Put the activation back where the create fixed it and let the real span pass:
    // nothing in the emulator sleeps for it, so the transition has to be materialised
    // by the next request alone.
    wait_until(restore_activations(&engine));
    let active = describe_endpoint(&router, case, &arn).await;
    assert_eq!(
        active["Status"].as_str(),
        Some(state::ACME_ENDPOINT_STATUS_ACTIVE),
        "case {case:?}: once the activation is due the endpoint must be ACTIVE, got {active}"
    );
    assert!(
        active["UpdatedAt"].as_f64().unwrap_or_default()
            > active["CreatedAt"].as_f64().unwrap_or_default(),
        "case {case:?}: the activation must move UpdatedAt past CreatedAt, got {active}"
    );
    assert_eq!(
        active["AcmeEndpointArn"].as_str(),
        Some(arn.as_str()),
        "case {case:?}: the transition must not change the endpoint's identity, got {active}"
    );

    // Terminal for the clock: a later read reports the same status, not a further one.
    let again = describe_endpoint(&router, case, &arn).await;
    assert_eq!(
        again, active,
        "case {case:?}: an ACTIVE endpoint must stay as it is across reads, got {again}"
    );
}

/// RULE: an `IdempotencyToken` identifies ONE create — re-sent with the same parameters
/// it answers with the endpoint it already created, re-sent with changed parameters it
/// is refused as a conflict, and a different token creates a different endpoint.
#[tokio::test]
async fn idempotency_token_resolves_to_one_endpoint() {
    let router = router();
    let body = |token: &str, contact: &str| {
        json!({
            "IdempotencyToken": token,
            "AuthorizationBehavior": AUTHORIZATION_BEHAVIORS[0],
            "Contact": contact,
            "CertificateAuthority": { "PublicCertificateAuthority": {} },
        })
    };

    // The rule holds for every token form and both contact values, so the table varies
    // the token's shape as well as the parameters it is re-sent with.
    for (token, contact) in [
        ("token-1", CONTACTS[0]),
        ("TOKEN_2", CONTACTS[1]),
        ("0123456789abcdef", CONTACTS[0]),
    ] {
        let case = format!("token {token:?}");
        let first = create_endpoint(&router, &case, &body(token, contact)).await;
        let replayed = create_endpoint(&router, &case, &body(token, contact)).await;
        assert_eq!(
            first, replayed,
            "case {case:?}: a re-sent token must answer with the endpoint it already created"
        );

        // The replay created nothing: the endpoint behind the ARN is the one the first
        // call made, and a changed request wearing the same token is refused.
        let endpoint = describe_endpoint(&router, &case, &first).await;
        assert_eq!(
            endpoint["Contact"].as_str(),
            Some(contact),
            "case {case:?}: the replay must not have overwritten the stored configuration, got {endpoint}"
        );

        let changed = CONTACTS.iter().find(|value| **value != contact).expect(
            "AcmeContact models more than one value, so there is always a changed request",
        );
        let (status, response) =
            call(&router, "CreateAcmeEndpoint", &body(token, changed)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the same token with different parameters must be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {response}"
        );

        let other = create_endpoint(&router, &case, &body(&format!("{token}-other"), contact)).await;
        assert_ne!(
            first, other,
            "case {case:?}: a different token must create a different endpoint"
        );

        // An ACME endpoint is regional, so the SAME token signed for another Region
        // names another create rather than replaying this one.
        let (status, response) = call_in_region(
            &router,
            "eu-west-1",
            "CreateAcmeEndpoint",
            &body(token, contact),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the same token in another Region must create that Region's endpoint, got {status} {response}"
        );
        let elsewhere = response["AcmeEndpointArn"].as_str().unwrap_or_default();
        assert!(
            elsewhere.starts_with("arn:aws:acm:eu-west-1:") && elsewhere != first,
            "case {case:?}: expected a distinct eu-west-1 ARN, got {elsewhere:?} beside {first}"
        );
    }
}

/// RULE: the idempotency fingerprint tells two DIFFERENT requests apart even when one
/// request's tag text is the other's structure — a changed request carrying a live
/// token is refused, never answered as a replay with its parameters discarded.
#[tokio::test]
async fn idempotency_fingerprint_is_unambiguous() {
    let router = router();
    let token = "ambiguous-groups";
    // Two requests whose tags hold the same STRINGS in different members. A rendering
    // that marked the group boundaries in-band (pushing a literal member name beside
    // caller text) would render these identically; the fingerprint must not.
    let first = json!({
        "IdempotencyToken": token,
        "AuthorizationBehavior": AUTHORIZATION_BEHAVIORS[0],
        "Contact": CONTACTS[0],
        "CertificateAuthority": { "PublicCertificateAuthority": {} },
        "Tags": [{ "Key": "CertificateTags", "Value": "k" }],
        "CertificateTags": [{ "Key": "v", "Value": "w" }],
    });
    let second = json!({
        "IdempotencyToken": token,
        "AuthorizationBehavior": AUTHORIZATION_BEHAVIORS[0],
        "Contact": CONTACTS[0],
        "CertificateAuthority": { "PublicCertificateAuthority": {} },
        "CertificateTags": [
            { "Key": "k", "Value": "CertificateTags" },
            { "Key": "v", "Value": "w" },
        ],
    });

    let arn = create_endpoint(&router, "tags hold the member name", &first).await;
    let (status, response) = call(&router, "CreateAcmeEndpoint", &second).await;
    assert_eq!(
        status, 400,
        "case regrouped tags: a different request on a live token must be refused, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ConflictException",
        "case regrouped tags: expected ConflictException, got {response}"
    );

    // And the first endpoint's own configuration is untouched by the refused call.
    let endpoint = describe_endpoint(&router, "tags hold the member name", &arn).await;
    assert_eq!(
        tag_pairs(&endpoint["CertificateTags"]),
        vec![("v".to_string(), "w".to_string())],
        "case regrouped tags: the refused call must change nothing, got {endpoint}"
    );
}

/// RULE: a tag whose key or value begins with the reserved `aws:` prefix is refused
/// with HTTP 400 `ValidationException`, on the endpoint's own tags as on the tags it
/// applies to the certificates it issues.
#[tokio::test]
async fn reserved_tag_prefix_is_refused() {
    let router = router();
    // One row per member the rule covers, times the two places the prefix can appear.
    let cases: &[(&str, Value)] = &[
        ("reserved key in Tags", json!({ "Tags": [{ "Key": "aws:owner", "Value": "x" }] })),
        (
            "reserved value in Tags",
            json!({ "Tags": [{ "Key": "owner", "Value": "aws:internal" }] }),
        ),
        (
            "reserved key in CertificateTags",
            json!({ "CertificateTags": [{ "Key": "aws:stage", "Value": "x" }] }),
        ),
        (
            "reserved value in CertificateTags",
            json!({ "CertificateTags": [{ "Key": "stage", "Value": "aws:internal" }] }),
        ),
    ];

    for (case, extra) in cases {
        let mut body = json!({
            "AuthorizationBehavior": AUTHORIZATION_BEHAVIORS[0],
            "Contact": CONTACTS[0],
            "CertificateAuthority": { "PublicCertificateAuthority": {} },
        });
        for (member, value) in extra.as_object().expect("the extra members are an object") {
            body[member] = value.clone();
        }

        let (status, response) = call(&router, "CreateAcmeEndpoint", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a reserved tag must be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response}"
        );
        assert!(
            error_message(&response).contains("aws:"),
            "case {case:?}: the refusal must say which rule the tag broke, got {response}"
        );
    }
}

/// RULE: a well-formed ACME endpoint ARN with no stored record behind it draws HTTP 400
/// `ResourceNotFoundException` naming the ARN the request asked for; an ARN that is not
/// an ACME endpoint ARN at all is refused as invalid input.
#[tokio::test]
async fn describe_unknown_endpoint_is_not_found() {
    let router = router();
    // One created endpoint, so the store is not empty and a miss is a real miss.
    let created = create_endpoint(
        &router,
        "a stored endpoint",
        &json!({
            "AuthorizationBehavior": AUTHORIZATION_BEHAVIORS[0],
            "Contact": CONTACTS[0],
            "CertificateAuthority": { "PublicCertificateAuthority": {} },
        }),
    )
    .await;

    // The ways a well-formed ACME endpoint ARN can have no record behind it: never
    // created, another Region, another account, and several id forms.
    let missing = [
        format!("arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/00000000-0000-0000-0000-000000000000"),
        format!("arn:aws:acm:eu-central-1:{ACCOUNT}:acme-endpoint/{}", endpoint_id(&created)),
        format!("arn:aws:acm:{REGION}:210987654321:acme-endpoint/{}", endpoint_id(&created)),
        format!("arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/a"),
        format!("arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/{}", "e".repeat(64)),
    ];
    for arn in &missing {
        let (status, response) = call(
            &router,
            "DescribeAcmeEndpoint",
            &json!({ "AcmeEndpointArn": arn }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {arn:?}: an unknown endpoint must draw HTTP 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {arn:?}: expected ResourceNotFoundException, got {response}"
        );
        assert_eq!(
            error_message(&response),
            format!("Could not find ACME endpoint {arn}."),
            "case {arn:?}: the refusal must name the ARN the request asked for, got {response}"
        );
    }

    // An ARN of another resource type does not satisfy `AcmeEndpointArn`'s pattern, so
    // it is bad input rather than a missing endpoint.
    let certificate_arn =
        format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/{}", endpoint_id(&created));
    let (status, response) = call(
        &router,
        "DescribeAcmeEndpoint",
        &json!({ "AcmeEndpointArn": certificate_arn }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {certificate_arn:?}: an ARN of another resource must be refused, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ValidationException",
        "case {certificate_arn:?}: expected ValidationException, got {response}"
    );
}

/// RULE: the account's ACME endpoints are a bounded resource — past the quota
/// `CreateAcmeEndpoint` answers `ServiceQuotaExceededException` and stores nothing
/// more, however often it is called.
#[tokio::test]
async fn endpoint_quota_is_enforced() {
    let (router, engine) = router_with_engine();
    let body = json!({
        "AuthorizationBehavior": AUTHORIZATION_BEHAVIORS[0],
        "Contact": CONTACTS[0],
        "CertificateAuthority": { "PublicCertificateAuthority": {} },
    });

    let mut held = 0usize;
    loop {
        let (status, response) = call(&router, "CreateAcmeEndpoint", &body).await;
        if status == 200 {
            held += 1;
            assert!(
                held <= 5_000,
                "case quota: CreateAcmeEndpoint never refused, after {held} endpoints"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case quota: expected the quota refusal to be HTTP 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ServiceQuotaExceededException",
            "case quota: expected ServiceQuotaExceededException after {held} endpoints, got {response}"
        );
        break;
    }

    // Refused BEFORE anything is inserted: repeating the call past the quota leaves the
    // store exactly as it was.
    let stored = || {
        engine
            .state::<state::ComAmazonawsAcmCertificatemanagerState>()
            .lock()
            .acme_endpoints
            .len()
    };
    let before = stored();
    assert_eq!(
        before, held,
        "case quota: expected the {held} created endpoints to be stored, got {before}"
    );
    for attempt in 0..3 {
        let (status, response) = call(&router, "CreateAcmeEndpoint", &body).await;
        assert_eq!(
            status, 400,
            "case quota retry #{attempt}: expected the refusal to repeat, got {status} {response}"
        );
        assert_eq!(
            stored(),
            before,
            "case quota retry #{attempt}: a refused create must store nothing"
        );
    }
}

/// RULE: the stored endpoints are bounded by a count nothing in the request can
/// partition — a caller that signs each create for ANOTHER Region still cannot grow the
/// store past [`state::ACME_ENDPOINT_STORE_LIMIT`], and every call past it is refused.
#[tokio::test]
async fn store_stays_bounded_however_the_region_varies() {
    let (router, engine) = router_with_engine();
    let body = json!({
        "AuthorizationBehavior": AUTHORIZATION_BEHAVIORS[0],
        "Contact": CONTACTS[0],
        "CertificateAuthority": { "PublicCertificateAuthority": {} },
    });
    // Distinct Region strings, cycled so no single one reaches ACM's own per-Region
    // quota first: only the cap on the whole store can stop this loop.
    let regions = [
        "us-east-1",
        "us-east-2",
        "us-west-1",
        "us-west-2",
        "eu-west-1",
        "eu-west-2",
        "eu-central-1",
        "eu-north-1",
        "ap-south-1",
        "ap-northeast-3",
        "sa-east-1",
        "af-south-1",
    ];
    let stored = || {
        engine
            .state::<state::ComAmazonawsAcmCertificatemanagerState>()
            .lock()
            .acme_endpoints
            .len()
    };

    let mut refused = 0usize;
    for attempt in 0..(state::ACME_ENDPOINT_STORE_LIMIT + regions.len() * 2) {
        let region = regions[attempt % regions.len()];
        let (status, response) = call_in_region(&router, region, "CreateAcmeEndpoint", &body).await;
        if status != 200 {
            refused += 1;
            assert_eq!(
                error_code(&response),
                "ServiceQuotaExceededException",
                "case {region}, attempt {attempt}: expected the store cap to answer the modelled quota error, got {status} {response}"
            );
        }
        assert!(
            stored() <= state::ACME_ENDPOINT_STORE_LIMIT,
            "case {region}, attempt {attempt}: the store must stay within {}, got {}",
            state::ACME_ENDPOINT_STORE_LIMIT,
            stored()
        );
    }
    assert!(
        refused >= regions.len(),
        "case varied Regions: creates must stop being accepted once the store is full, but only {refused} were refused"
    );
    assert_eq!(
        stored(),
        state::ACME_ENDPOINT_STORE_LIMIT,
        "case varied Regions: expected the store to fill to its cap and stop, got {}",
        stored()
    );

    // A Region never seen before is refused too: the cap is not keyed on the Region.
    let (status, response) =
        call_in_region(&router, "ap-southeast-4", "CreateAcmeEndpoint", &body).await;
    assert_eq!(
        status, 400,
        "case a fresh Region: the cap must hold for a Region with no endpoints, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ServiceQuotaExceededException",
        "case a fresh Region: expected ServiceQuotaExceededException, got {response}"
    );
    assert_eq!(
        stored(),
        state::ACME_ENDPOINT_STORE_LIMIT,
        "case a fresh Region: a refused create must store nothing"
    );
}
