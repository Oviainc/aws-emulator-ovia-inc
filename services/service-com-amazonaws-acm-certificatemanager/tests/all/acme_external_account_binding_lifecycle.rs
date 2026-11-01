// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for ACM's ACME external account bindings over the wire (`POST /`,
//! `X-Amz-Target: CertificateManager.<Op>`, awsJson1_1).
//!
//! RULE: `CreateAcmeExternalAccountBinding` persists ONE external account binding per
//! call under the ACME endpoint the request names — minting its ARN as a path under the
//! endpoint's own ARN and its credentials (the key id an ACME client presents and the MAC
//! key it signs its registration JWS with) from nothing the caller sent, recording the
//! IAM role the request bound, the expiry its `Expiration` names counted from the instant
//! of creation, and the resource tags it carries — and
//! `DescribeAcmeExternalAccountBinding` reports that stored record rather than echoing
//! any request of its own. A re-sent idempotency token carrying different parameters is
//! refused as a conflict, an endpoint ARN with no record behind it as not found, and one
//! of another account as a validation error; deleting the endpoint drops its bindings
//! with it.
//!
//! Every assertion goes through [`MultiServiceRouter::dispatch`], the same path the
//! emulator serves production traffic on, and every read goes through a DIFFERENT
//! operation than the write, so a response that is not computed from the persisted store
//! cannot pass. The wire harness is shared with [`crate::certificate_lifecycle`].
//!
//! The table below spans the classes the rule generalises over: every `TimeType` unit an
//! `Expiration` can be counted in and a request that carries none, `Expiration$Value` on
//! its modelled lower bound and far above it, role ARNs of several forms the member's
//! pattern admits (a plain role, a nested path, another partition, punctuation), and tag
//! sets of several sizes and scripts — not the single configuration the stub issue
//! quoted.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_acm_certificatemanager::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::acme_domain_validation_lifecycle::endpoint_body;
use crate::acme_endpoint_lifecycle::create_endpoint;
use crate::certificate_lifecycle::{
    call, error_code, error_message, router_with_engine, ACCOUNT, REGION,
};

/// Every `TimeType` the model declares
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#TimeType")`), beside the number
/// of seconds one unit of each counts.
const TIME_TYPES: &[(&str, u64)] = &[
    (state::TIME_TYPE_MINUTES, 60),
    (state::TIME_TYPE_HOURS, 60 * 60),
    (state::TIME_TYPE_DAYS, 24 * 60 * 60),
];

/// One `CreateAcmeExternalAccountBinding` case: what it exercises, the `Expiration` to
/// configure (if any), the `RoleArn` to bind, and the `Tags` to apply (if any).
pub(crate) type BindingCase = (&'static str, Option<Value>, String, Option<Value>);

/// One request body for each row of the table.
pub(crate) fn binding_cases() -> Vec<BindingCase> {
    vec![
        (
            "no expiration at all, a plain role, no tags",
            None,
            format!("arn:aws:iam::{ACCOUNT}:role/AcmeIssuer"),
            None,
        ),
        (
            "Expiration$Value on its modelled lower bound (1), counted in MINUTES",
            Some(json!({ "Value": 1, "Type": TIME_TYPES[0].0 })),
            format!("arn:aws:iam::{ACCOUNT}:role/service-role/nested/Acme-Issuer_1"),
            Some(json!([{ "Key": "owner", "Value": "platform" }])),
        ),
        (
            "an expiration counted in HOURS, a role in another partition",
            Some(json!({ "Value": 12, "Type": TIME_TYPES[1].0 })),
            format!("arn:aws-us-gov:iam::{ACCOUNT}:role/GovAcmeIssuer"),
            Some(json!([
                { "Key": "環境", "Value": "本番" },
                { "Key": "team", "Value": "" },
            ])),
        ),
        (
            "an expiration counted in DAYS, a role name carrying the punctuation IAM admits",
            Some(json!({ "Value": 365, "Type": TIME_TYPES[2].0 })),
            format!("arn:aws:iam::{ACCOUNT}:role/Acme+Issuer=One,Two@example.com"),
            // 50 is `TagList`'s modelled maximum length, so this row sits on the bound.
            Some(Value::Array(
                (0..50)
                    .map(|n| json!({ "Key": format!("key-{n}"), "Value": format!("value-{n}") }))
                    .collect(),
            )),
        ),
        (
            "a large expiration, far above the modelled lower bound",
            Some(json!({ "Value": 100_000, "Type": TIME_TYPES[2].0 })),
            format!("arn:aws:iam::{ACCOUNT}:role/LongLivedAcmeIssuer"),
            None,
        ),
    ]
}

/// The request body of one row, for `acme_endpoint_arn`.
pub(crate) fn binding_body(case: &BindingCase, acme_endpoint_arn: &str) -> Value {
    let (_what, expiration, role_arn, tags) = case;
    let mut body = json!({
        "AcmeEndpointArn": acme_endpoint_arn,
        "RoleArn": role_arn,
    });
    if let Some(expiration) = expiration.clone() {
        body["Expiration"] = expiration;
    }
    if let Some(tags) = tags.clone() {
        body["Tags"] = tags;
    }
    body
}

/// `CreateAcmeExternalAccountBinding`'s `ExternalAccountBinding` member, asserted to
/// succeed.
pub(crate) async fn create_binding(
    router: &MultiServiceRouter,
    case: &str,
    body: &Value,
) -> Value {
    let (status, response) = call(router, "CreateAcmeExternalAccountBinding", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateAcmeExternalAccountBinding must succeed, got {status} {response}"
    );
    let binding = response["ExternalAccountBinding"].clone();
    assert!(
        binding.is_object(),
        "case {case:?}: the create must report the binding it made, got {response}"
    );
    binding
}

/// `DescribeAcmeExternalAccountBinding`'s `ExternalAccountBinding` member, asserted to
/// succeed.
pub(crate) async fn describe_binding(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> Value {
    let (status, response) = call(
        router,
        "DescribeAcmeExternalAccountBinding",
        &json!({ "AcmeExternalAccountBindingArn": arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeAcmeExternalAccountBinding({arn}) must succeed, got {status} {response}"
    );
    response["ExternalAccountBinding"].clone()
}

/// The `(key, value)` tags `ListTagsForResource` reports for `arn`, in key order.
async fn resource_tags(router: &MultiServiceRouter, case: &str, arn: &str) -> Vec<(String, String)> {
    let (status, response) = call(
        router,
        "ListTagsForResource",
        &json!({ "ResourceArn": arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource({arn}) must succeed, got {status} {response}"
    );
    let mut pairs: Vec<(String, String)> = response["Tags"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ListTagsForResource reported no Tags list"))
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

/// The `(key, value)` tags a request body carries, in key order.
fn requested_tags(tags: Option<&Value>) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = tags
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .map(|tag| {
                    (
                        tag["Key"].as_str().unwrap_or_default().to_string(),
                        tag["Value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    pairs.sort();
    pairs
}

/// The credentials the service minted for the binding `arn`, read straight off the store
/// so a test can assert what the create persisted before any operation reports it.
pub(crate) fn stored_credentials(
    engine: &Arc<engine::InMemoryEngine>,
    arn: &str,
) -> (String, String) {
    let store = engine.state::<state::ComAmazonawsAcmCertificatemanagerState>();
    let store = store.lock();
    let binding = store
        .acme_external_account_bindings
        .get(arn)
        .unwrap_or_else(|| panic!("the store holds no external account binding {arn}"));
    (binding.key_id.clone(), binding.mac_key.clone())
}

/// A timestamp member as the epoch seconds the awsJson1_1 wire carries it as.
pub(crate) fn seconds(binding: &Value, member: &str) -> f64 {
    binding[member]
        .as_f64()
        .unwrap_or_else(|| panic!("{member} is not a wire timestamp in {binding}"))
}

/// Whether `arn` satisfies `AcmeExternalAccountBindingArn`'s modelled `@pattern`
/// (`^arn:aws[a-z-]*:acm:[a-z0-9-]+:[0-9]{12}:acme-endpoint/[a-zA-Z0-9-]+/acme-external-account-binding/[a-zA-Z0-9-]+$`)
/// and its modelled length (1..200), as a path under `acme_endpoint_arn`.
fn is_well_formed_binding_arn(arn: &str, acme_endpoint_arn: &str) -> bool {
    let prefix = format!("{acme_endpoint_arn}/acme-external-account-binding/");
    let Some(id) = arn.strip_prefix(&prefix) else {
        return false;
    };
    arn.len() <= 200
        && arn.starts_with(&format!("arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/"))
        && !id.is_empty()
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// The id segment of a binding ARN: everything after the last `/`.
fn binding_id(arn: &str) -> &str {
    arn.rsplit_once('/')
        .unwrap_or_else(|| panic!("{arn} is not an external account binding ARN"))
        .1
}

/// RULE: a MAC key is key material an ACME client decodes, so it must round-trip through
/// base64 — and under BOTH alphabets RFC 4648 defines, since the key is handed over as
/// text and nothing tells the client which decoder to reach for.
///
/// The decode goes through `aws_smithy_types::base64`, the standard-alphabet codec this
/// workspace already carries and the one the wire serializer itself uses for a blob. The
/// URL-safe half is asserted as the property that makes the two decoders agree: the two
/// alphabets differ only in their last two symbols (RFC 4648 §§4-5), so a key whose every
/// character is ASCII alphanumeric names neither of them and decodes identically under
/// both.
pub(crate) fn assert_mac_key_round_trips(case: &str, mac_key: &str) {
    assert_eq!(
        mac_key.chars().count(),
        state::ACME_MAC_KEY_LEN,
        "case {case:?}: expected a {}-character MAC key, got {mac_key:?}",
        state::ACME_MAC_KEY_LEN
    );
    let decoded = aws_smithy_types::base64::decode(mac_key).unwrap_or_else(|error| {
        panic!("case {case:?}: the MAC key {mac_key:?} must decode as base64, got {error}")
    });
    assert_eq!(
        aws_smithy_types::base64::encode(&decoded),
        mac_key,
        "case {case:?}: re-encoding the {} decoded key bytes must answer the MAC key itself",
        decoded.len()
    );
    assert!(
        mac_key.chars().all(|c| c.is_ascii_alphanumeric()),
        "case {case:?}: the key must name no symbol the two base64 alphabets disagree on, got {mac_key:?}"
    );
}

/// RULE: `DescribeAcmeExternalAccountBinding` reports the binding
/// `CreateAcmeExternalAccountBinding` persisted — its minted ARN under the endpoint, the
/// IAM role bound, the expiry the `Expiration` names counted from creation, and the
/// absent revocation and last-use instants — and the credentials it minted are
/// server-generated, unique per binding and decodable key material, for every row of the
/// table above.
#[tokio::test]
async fn describe_reports_what_create_acme_external_account_binding_stored() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "binding host", &endpoint_body()).await;

    let mut seen_arns: BTreeSet<String> = BTreeSet::new();
    let mut seen_mac_keys: BTreeSet<String> = BTreeSet::new();

    for case in binding_cases() {
        let (what, expiration, role_arn, tags) = case.clone();
        let body = binding_body(&case, &endpoint_arn);
        let created = create_binding(&router, what, &body).await;

        let arn = created["AcmeExternalAccountBindingArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {what:?}: the create reported no ARN in {created}"))
            .to_string();
        assert!(
            is_well_formed_binding_arn(&arn, &endpoint_arn),
            "case {what:?}: the minted ARN must be a well-formed path under {endpoint_arn}, got {arn:?}"
        );
        assert!(
            seen_arns.insert(arn.clone()),
            "case {what:?}: every create must mint its own ARN, but {arn} was minted twice"
        );

        let described = describe_binding(&router, what, &arn).await;
        assert_eq!(
            described, created,
            "case {what:?}: the describe must report the record the create stored"
        );

        assert_eq!(
            described["AcmeEndpointArn"].as_str(),
            Some(endpoint_arn.as_str()),
            "case {what:?}: expected the binding to report endpoint {endpoint_arn}, got {described}"
        );
        assert_eq!(
            described["RoleArn"].as_str(),
            Some(role_arn.as_str()),
            "case {what:?}: expected the stored RoleArn {role_arn}, got {described}"
        );
        assert!(
            described["RevokedAt"].is_null() && described["LastUsedAt"].is_null(),
            "case {what:?}: nothing revoked the binding and nothing used it, so neither instant may be reported, got {described}"
        );
        assert_eq!(
            described["CreatedAt"], described["UpdatedAt"],
            "case {what:?}: nothing has changed the fresh binding, so UpdatedAt must be CreatedAt, got {described}"
        );

        // The expiry is the span the request named, counted from the instant the binding
        // was created — not a value read back out of the request.
        match expiration.as_ref() {
            None => assert!(
                described["ExpiresAt"].is_null(),
                "case {what:?}: a create that named no Expiration must store no expiry, got {described}"
            ),
            Some(expiration) => {
                let unit = TIME_TYPES
                    .iter()
                    .find(|(name, _)| Some(*name) == expiration["Type"].as_str())
                    .unwrap_or_else(|| panic!("case {what:?}: {expiration} names no known TimeType"))
                    .1;
                let span = expiration["Value"].as_f64().unwrap_or_default() * unit as f64;
                let observed = seconds(&described, "ExpiresAt") - seconds(&described, "CreatedAt");
                assert!(
                    (observed - span).abs() < 1e-3,
                    "case {what:?}: expected ExpiresAt {span} seconds after CreatedAt, got {observed}"
                );
            }
        }

        // The tags the create carried, read back through a third operation.
        assert_eq!(
            resource_tags(&router, what, &arn).await,
            requested_tags(tags.as_ref()),
            "case {what:?}: ListTagsForResource must report the tags the create applied"
        );

        // The credentials: server-minted, never a member of any request, and the key
        // material an ACME client decodes.
        let (key_id, mac_key) = stored_credentials(&engine, &arn);
        assert_eq!(
            key_id,
            binding_id(&arn),
            "case {what:?}: the key id must identify the binding it belongs to"
        );
        assert_mac_key_round_trips(what, &mac_key);
        assert!(
            seen_mac_keys.insert(mac_key.clone()),
            "case {what:?}: every binding must get its own MAC key, but {mac_key} was minted twice"
        );
        assert!(
            !body.to_string().contains(&mac_key) && !body.to_string().contains(&key_id),
            "case {what:?}: neither credential may come from the request {body}"
        );

        // Reading the binding again leaves the credentials alone: they are immutable.
        describe_binding(&router, what, &arn).await;
        assert_eq!(
            stored_credentials(&engine, &arn),
            (key_id, mac_key),
            "case {what:?}: a read must not re-mint the binding's credentials"
        );
    }
}

/// RULE: an `IdempotencyToken` names ONE create against one endpoint: re-sent with the
/// same parameters it answers with the binding it already created, re-sent with different
/// parameters it is a conflict, and two different tokens create two bindings.
#[tokio::test]
async fn an_idempotency_token_names_one_create() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "token host", &endpoint_body()).await;

    for case in binding_cases() {
        let (what, _expiration, _role_arn, _tags) = case.clone();
        let token = format!("token-for-{what}");
        let mut body = binding_body(&case, &endpoint_arn);
        body["IdempotencyToken"] = json!(token);

        let first = create_binding(&router, what, &body).await;
        let replayed = create_binding(&router, what, &body).await;
        assert_eq!(
            first, replayed,
            "case {what:?}: re-sending one token must answer with the binding it already created"
        );

        // The same token on a CHANGED request is a different request wearing an
        // identifier that is already taken.
        let mut changed = body.clone();
        changed["RoleArn"] = json!(format!("arn:aws:iam::{ACCOUNT}:role/AnotherRole"));
        let (status, response) = call(&router, "CreateAcmeExternalAccountBinding", &changed).await;
        assert_eq!(
            status, 400,
            "case {what:?}: a re-sent token with different parameters must be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ConflictException",
            "case {what:?}: expected ConflictException, got {response}"
        );
        let arn = first["AcmeExternalAccountBindingArn"]
            .as_str()
            .unwrap_or_default();
        assert_eq!(
            error_message(&response),
            state::IDEMPOTENCY_TOKEN_REUSED_MESSAGE,
            "case {what:?}: expected the fixed idempotency-conflict sentence, got {:?}",
            error_message(&response)
        );
        assert!(
            !error_message(&response).contains(arn),
            "case {what:?}: the conflict must not name the binding the token created ({arn}), got {:?}",
            error_message(&response)
        );

        // A second token creates a second binding, so the token is what identifies the
        // create and nothing else about the request does.
        let mut second = body.clone();
        second["IdempotencyToken"] = json!(format!("other-{token}"));
        let other = create_binding(&router, what, &second).await;
        assert_ne!(
            other["AcmeExternalAccountBindingArn"], first["AcmeExternalAccountBindingArn"],
            "case {what:?}: a different token must create a different binding"
        );
    }

    // Two creates per row reached the store, and no replay added a third.
    let store = engine.state::<state::ComAmazonawsAcmCertificatemanagerState>();
    let held = store.lock().acme_external_account_bindings.len();
    assert_eq!(
        held,
        binding_cases().len() * 2,
        "expected one binding per token, got {held}"
    );
}

/// RULE: a binding exists only under an endpoint the caller's own account holds, so an
/// endpoint ARN of another account is a validation error, one with no record behind it is
/// not found, and an expiry no instant can hold is refused — and a binding ARN nothing
/// answers to is likewise not found.
#[tokio::test]
async fn a_binding_needs_an_endpoint_of_the_callers_own_account() {
    let (router, _engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "refusal host", &endpoint_body()).await;
    let role_arn = format!("arn:aws:iam::{ACCOUNT}:role/AcmeIssuer");

    // An endpoint ARN of this account that names no stored endpoint.
    let absent = format!("arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/00000000-0000-4000-8000-000000000000");
    let (status, response) = call(
        &router,
        "CreateAcmeExternalAccountBinding",
        &json!({ "AcmeEndpointArn": absent, "RoleArn": role_arn }),
    )
    .await;
    assert_eq!(
        status, 400,
        "an endpoint with no record behind it must be refused, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException for {absent}, got {response}"
    );
    assert_eq!(
        error_message(&response),
        state::acme_endpoint_not_found_message(&absent),
        "the refusal must name the endpoint the request asked for"
    );

    // An endpoint ARN of ANOTHER account: a fact about the request, not about the store.
    let other_account = format!(
        "arn:aws:acm:{REGION}:{}:acme-endpoint/other-endpoint",
        "210987654321"
    );
    let (status, response) = call(
        &router,
        "CreateAcmeExternalAccountBinding",
        &json!({ "AcmeEndpointArn": other_account, "RoleArn": role_arn }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a cross-account endpoint ARN must be refused, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ValidationException",
        "expected ValidationException for {other_account}, got {response}"
    );

    // An expiration no instant can hold: the modelled `@range(min: 1)` has a lower bound
    // and no upper one, so the bound left to refuse is what a clock can represent.
    for (unit, _seconds) in TIME_TYPES {
        let (status, response) = call(
            &router,
            "CreateAcmeExternalAccountBinding",
            &json!({
                "AcmeEndpointArn": endpoint_arn,
                "RoleArn": role_arn,
                "Expiration": { "Value": i64::MAX, "Type": unit },
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {unit:?}: an unrepresentable expiry must be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {unit:?}: expected ValidationException, got {response}"
        );
    }

    // A binding ARN nothing answers to, on an endpoint that does exist.
    let absent_binding = state::acme_external_account_binding_arn(
        &endpoint_arn,
        "00000000-0000-4000-8000-000000000000",
    );
    let (status, response) = call(
        &router,
        "DescribeAcmeExternalAccountBinding",
        &json!({ "AcmeExternalAccountBindingArn": absent_binding }),
    )
    .await;
    assert_eq!(
        status, 400,
        "describing a binding that does not exist must be refused, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException for {absent_binding}, got {response}"
    );
    assert_eq!(
        error_message(&response),
        state::acme_external_account_binding_not_found_message(&absent_binding),
        "the refusal must name the binding the request asked for"
    );
}

/// RULE: a binding's ARN is a path under its endpoint's, so it cannot outlive the
/// endpoint: deleting the endpoint drops every binding under it.
#[tokio::test]
async fn deleting_the_endpoint_drops_its_bindings() {
    let (router, engine) = router_with_engine();
    let kept_endpoint = create_endpoint(&router, "kept host", &endpoint_body()).await;
    let doomed_endpoint = create_endpoint(&router, "doomed host", &endpoint_body()).await;

    let mut kept = Vec::new();
    let mut doomed = Vec::new();
    for case in binding_cases() {
        let (what, _expiration, _role_arn, _tags) = case.clone();
        for (endpoint_arn, arns) in [
            (&kept_endpoint, &mut kept),
            (&doomed_endpoint, &mut doomed),
        ] {
            let body = binding_body(&case, endpoint_arn);
            let created = create_binding(&router, what, &body).await;
            arns.push(
                created["AcmeExternalAccountBindingArn"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        }
    }

    let (status, response) = call(
        &router,
        "DeleteAcmeEndpoint",
        &json!({ "AcmeEndpointArn": doomed_endpoint }),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteAcmeEndpoint must succeed, got {status} {response}"
    );

    for arn in &doomed {
        let (status, response) = call(
            &router,
            "DescribeAcmeExternalAccountBinding",
            &json!({ "AcmeExternalAccountBindingArn": arn }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {arn:?}: a binding of a deleted endpoint must be gone, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {arn:?}: expected ResourceNotFoundException, got {response}"
        );
    }
    for arn in &kept {
        describe_binding(&router, arn, arn).await;
    }

    // The deletion took the endpoint's bindings and nothing else with them.
    let store = engine.state::<state::ComAmazonawsAcmCertificatemanagerState>();
    let held = store.lock().acme_external_account_bindings.len();
    assert_eq!(
        held,
        kept.len(),
        "expected only the kept endpoint's {} bindings to remain, got {held}",
        kept.len()
    );
}
