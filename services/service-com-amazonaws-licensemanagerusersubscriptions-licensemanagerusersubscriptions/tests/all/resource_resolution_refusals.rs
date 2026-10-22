// SPDX-License-Identifier: CC0-1.0

//! How this service resolves the identifier a request names, and what it answers when
//! that identifier resolves to nothing — driven over the real wire path through the
//! router's `dispatch`, so every request is deserialized, constraint-checked, handled
//! and serialized exactly as a client's would be.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! request:
//!
//! * **Resolution first.** An operation that addresses a resource resolves that
//!   identifier against persisted state BEFORE it reads or validates any other member
//!   of the request, so a payload the service would also refuse never changes the answer
//!   an unresolvable identifier gets.
//! * **The refusal follows the KIND of identifier**, and never discloses whether the
//!   identifier names anything:
//!   - an EC2 instance the account's fleet does not hold, and an ARN of one of this
//!     service's ownership-scoped resources (a user association, a license server
//!     endpoint) that state does not hold, fail CLOSED on the authorization gate:
//!     `UnauthorizedException` with its fixed sentence;
//!   - an identity-provider reference that resolves to no registration is a parameter
//!     that is not valid: `ValidationException` / "Identity provider is not valid";
//!   - the tagging surface's `ResourceArn`, which may name any of this service's
//!     resource kinds, is refused with `BadRequestException` / "Invalid input resource
//!     arn".
//! * **Identifier combinations the model cannot express are the service's own rule.**
//!   `DisassociateUser` marks no member `@required`, so a request addressing no
//!   association is refused with `ValidationException` naming the members it requires,
//!   before anything is looked up.
//! * **Every answer is computed from state**: what an associate/create/update persisted
//!   is what a later read reports, what a disassociate/delete removed is gone (and is
//!   refused the same way an identifier that never existed is), and the status
//!   transition the clock owes is materialised on a later read.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::{
    scaffolded::{
        BAD_REQUEST_EXCEPTION_CODE, INVALID_INPUT_RESOURCE_ARN_MESSAGE,
        UNAUTHORIZED_EXCEPTION_CODE, UNAUTHORIZED_EXCEPTION_MESSAGE,
    },
    state::{
        seed_account_instances, IDENTITY_PROVIDER_NOT_VALID, MAX_RESOURCES_PER_KIND,
        MAX_SUBNETS_PER_UPDATE, MAX_TAGS_PER_RESOURCE,
    },
    ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState,
};

use super::identity_provider_subscriptions::{
    arn_prefix, as_str, call, harness, list_providers, provider_union, register, send, Harness,
    Reply, ACCOUNT, PRODUCTS, REGION, SIGNING_NAME,
};

/// The sentence `DisassociateUser` answers a request that addresses no association
/// with, from the handler that owns it.
use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::ops::disassociate_user::IDENTIFIERS_REQUIRED;

/// The `LicenseServerSettings` every `CreateLicenseServerEndpoint` row carries.
///
/// `ServerType` has exactly one value in the model (`RDS_SAL`) and `ServerSettings` one
/// member, so this payload is the only shape the model admits; `SecretId` varies per
/// row where it matters.
fn license_server_settings(secret_id: &str) -> Value {
    json!({
        "ServerType": "RDS_SAL",
        "ServerSettings": {
            "RdsSalSettings": {
                "RdsSalCredentialsProvider": {
                    "SecretsManagerCredentialsProvider": { "SecretId": secret_id }
                }
            }
        }
    })
}

/// A `RegisterIdentityProvider` body for `directory_id` and `product`.
fn registration(directory_id: &str, product: &str, subnets: &[&str], tags: Option<Value>) -> Value {
    let mut body = json!({
        "IdentityProvider": provider_union(directory_id),
        "Product": product,
        "Settings": { "Subnets": subnets, "SecurityGroupId": "sg-0123abcd" },
    });
    if let Some(tags) = tags {
        body["Tags"] = tags;
    }
    body
}

async fn associate(h: &Harness, body: &Value) -> Reply {
    call(h, "/user/AssociateUser", body).await
}

async fn disassociate(h: &Harness, body: &Value) -> Reply {
    call(h, "/user/DisassociateUser", body).await
}

async fn create_endpoint(h: &Harness, body: &Value) -> Reply {
    call(h, "/license-server/CreateLicenseServerEndpoint", body).await
}

async fn delete_endpoint(h: &Harness, body: &Value) -> Reply {
    call(h, "/license-server/DeleteLicenseServerEndpoint", body).await
}

async fn update_settings(h: &Harness, body: &Value) -> Reply {
    call(h, "/identity-provider/UpdateIdentityProviderSettings", body).await
}

/// Register one identity provider and answer the `IdentityProviderArn` it minted.
async fn register_provider(h: &Harness, body: &Value, case: &str) -> String {
    let reply = register(h, body).await;
    as_str(
        &reply.ok(case)["IdentityProviderSummary"],
        "IdentityProviderArn",
        case,
    )
    .to_string()
}

/// `ListTagsForResource` binds `ResourceArn` to ONE path segment (`GET
/// /tags/{ResourceArn}`), so the `:` and `/` inside an ARN have to be percent-encoded or
/// the value would split the segment.
fn encoded(arn: &str) -> String {
    arn.replace('%', "%25").replace(':', "%3A").replace('/', "%2F")
}

async fn list_tags(h: &Harness, arn: &str) -> Reply {
    send(h, "GET", &format!("/tags/{}", encoded(arn)), None).await
}

async fn untag(h: &Harness, arn: &str, tag_keys: &[&str]) -> Reply {
    let query = tag_keys
        .iter()
        .map(|key| format!("tagKeys={key}"))
        .collect::<Vec<_>>()
        .join("&");
    send(
        h,
        "DELETE",
        &format!("/tags/{}?{query}", encoded(arn)),
        None,
    )
    .await
}

async fn tag(h: &Harness, arn: &str, tags: &Value) -> Reply {
    send(
        h,
        "PUT",
        &format!("/tags/{}", encoded(arn)),
        Some(&json!({ "Tags": tags })),
    )
    .await
}

/// Every fail-closed refusal looks the same: HTTP 400, the `UnauthorizedException` code
/// an SDK dispatches on, the fixed sentence, and a body that says nothing about the
/// identifier it was asked about.
fn assert_fails_closed(case: &str, identifier: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected HTTP 400, got {} with body {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some(UNAUTHORIZED_EXCEPTION_CODE),
        "case {case:?}: expected {UNAUTHORIZED_EXCEPTION_CODE}, got {:?} with body {:?}",
        reply.error_type,
        reply.body
    );
    assert_eq!(
        reply.body.get("message").and_then(Value::as_str),
        Some(UNAUTHORIZED_EXCEPTION_MESSAGE),
        "case {case:?}: expected the fixed refusal sentence, got {:?}",
        reply.body
    );
    let rendered = reply.body.to_string();
    assert!(
        !rendered.contains(identifier),
        "case {case:?}: the refusal must not disclose {identifier:?}, got {rendered}"
    );
}

/// Every refusal of an identity-provider reference that does not resolve.
fn assert_identity_provider_not_valid(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected HTTP 400, got {} with body {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("ValidationException"),
        "case {case:?}: expected ValidationException, got {:?} with body {:?}",
        reply.error_type,
        reply.body
    );
    assert_eq!(
        reply.body.get("message").and_then(Value::as_str),
        Some(IDENTITY_PROVIDER_NOT_VALID),
        "case {case:?}: expected {IDENTITY_PROVIDER_NOT_VALID:?}, got {:?}",
        reply.body
    );
}

/// Every refusal of a tagging `ResourceArn` that names no resource.
fn assert_invalid_resource_arn(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected HTTP 400, got {} with body {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some(BAD_REQUEST_EXCEPTION_CODE),
        "case {case:?}: expected {BAD_REQUEST_EXCEPTION_CODE}, got {:?} with body {:?}",
        reply.error_type,
        reply.body
    );
    assert_eq!(
        reply.body.get("message").and_then(Value::as_str),
        Some(INVALID_INPUT_RESOURCE_ARN_MESSAGE),
        "case {case:?}: expected {INVALID_INPUT_RESOURCE_ARN_MESSAGE:?}, got {:?}",
        reply.body
    );
}

/// An instance id the account's EC2 fleet holds, so an association on it is authorized.
fn held_instance(index: usize) -> String {
    seed_account_instances()[index].0.clone()
}

/// Instance ids the account's fleet does not hold, as `(class, instance id)`.
///
/// `InstanceId` is a plain `String` in this service's model, so the rows vary in FORM and
/// SIZE rather than against a pattern: EC2's two id lengths, a near miss on an instance
/// the fleet does hold, and a value that is not an instance id at all.
fn unheld_instances() -> Vec<(String, String)> {
    let held = held_instance(0);
    let near_miss = format!("{}0", &held[..held.len() - 1]);
    vec![
        (
            "the 17-hex-digit id form".to_string(),
            "i-0123456789abcdef0".to_string(),
        ),
        ("the 8-hex-digit id form".to_string(), "i-0abc1234".to_string()),
        ("a near miss on a held instance".to_string(), near_miss),
        (
            "an uppercase spelling of a held instance".to_string(),
            held.to_ascii_uppercase(),
        ),
        (
            "a value that is not an instance id".to_string(),
            "not-an-instance".to_string(),
        ),
    ]
}

/// RULE: the instance-authorization gate runs BEFORE any other lookup and fails closed —
/// an instance the account's fleet does not hold is refused with `UnauthorizedException`,
/// whatever the rest of the request says and whether or not the identity provider it
/// names is registered.
#[tokio::test]
async fn the_instance_gate_fails_closed_before_any_other_lookup() {
    let h = harness();
    // A registered directory and an unregistered one: the gate is reached first either
    // way, so neither the registration nor its absence changes the answer.
    register(
        &h,
        &registration("d-0123456789", PRODUCTS[0], &["subnet-0123456789abcdef0"], None),
    )
    .await
    .ok("a registration to associate through");

    // The rest of the request is varied across the rows, so what is pinned is the
    // branch and not one recorded request.
    let shapes: [(&str, &str, Option<&str>, bool); 3] = [
        ("d-0123456789", "registered.user", None, false),
        ("d-9999999999", "unregistered.user", Some("corp.example"), true),
        ("sd-abcdef0123", "shared.user", Some("example.internal"), false),
    ];

    for (row, (class, instance_id)) in unheld_instances().into_iter().enumerate() {
        let (directory_id, username, domain, tagged) = shapes[row % shapes.len()];
        let case = format!("AssociateUser/{class}/directory={directory_id}");
        let mut body = json!({
            "Username": username,
            "InstanceId": instance_id,
            "IdentityProvider": provider_union(directory_id),
        });
        if let Some(domain) = domain {
            body["Domain"] = json!(domain);
        }
        if tagged {
            body["Tags"] = json!({ "team": "licensing" });
        }
        let reply = associate(&h, &body).await;
        assert_fails_closed(&case, &instance_id, &reply);
    }
}

/// RULE: an ARN of one of this service's ownership-scoped resources that state holds
/// nothing for fails closed too — a user association addressed by `InstanceUserArn` and
/// a license server endpoint addressed by `LicenseServerEndpointArn`.
#[tokio::test]
async fn an_arn_this_service_holds_no_resource_for_fails_closed() {
    let h = harness();
    // ARNs of the shape this service mints, in other accounts and Regions, plus one that
    // is well formed but was never created.
    let rows: [(&str, String); 4] = [
        (
            "an ARN of the form this service mints",
            format!("{}iu-000000009999", arn_prefix("instance-user")),
        ),
        (
            "an ARN in another account",
            format!(
                "arn:aws:{SIGNING_NAME}:{REGION}:111111111111:instance-user/iu-000000000001"
            ),
        ),
        (
            "an ARN in another Region",
            format!(
                "arn:aws:{SIGNING_NAME}:eu-west-1:{ACCOUNT}:instance-user/iu-000000000001"
            ),
        ),
        (
            "an ARN of another resource kind",
            format!("{}ip-000000009999", arn_prefix("identity-provider")),
        ),
    ];
    for (class, arn) in &rows {
        let case = format!("DisassociateUser/{class}");
        let reply = disassociate(&h, &json!({ "InstanceUserArn": arn })).await;
        assert_fails_closed(&case, arn, &reply);
    }

    let endpoints: [(&str, String); 3] = [
        (
            "an endpoint ARN of the form this service mints",
            format!("{}lse-000000009999", arn_prefix("license-server-endpoint")),
        ),
        (
            "an endpoint ARN in another account",
            format!(
                "arn:aws:{SIGNING_NAME}:{REGION}:111111111111:license-server-endpoint/lse-000000000001"
            ),
        ),
        (
            "an endpoint ARN in another Region",
            format!(
                "arn:aws:{SIGNING_NAME}:eu-west-1:{ACCOUNT}:license-server-endpoint/lse-000000000001"
            ),
        ),
    ];
    for (class, arn) in &endpoints {
        let case = format!("DeleteLicenseServerEndpoint/{class}");
        let reply = delete_endpoint(
            &h,
            &json!({ "LicenseServerEndpointArn": arn, "ServerType": "RDS_SAL" }),
        )
        .await;
        assert_fails_closed(&case, arn, &reply);
    }
}

/// RULE: `DisassociateUser` refuses a request that addresses no association — neither an
/// `InstanceUserArn` nor the full `IdentityProvider`/`InstanceId`/`Username` triple —
/// with `ValidationException` naming the members it requires, BEFORE anything is looked
/// up; a complete identifier reaches the resolution and is refused on the authorization
/// gate instead.
#[tokio::test]
async fn disassociate_user_refuses_an_incomplete_identifier_before_any_lookup() {
    let h = harness();
    // Every strict subset of the triple, plus the empty request and a request carrying
    // only the members that are NOT identifiers.
    let incomplete: [(&str, Value); 8] = [
        ("no members at all", json!({})),
        ("Username alone", json!({ "Username": "a.user" })),
        (
            "InstanceId alone",
            json!({ "InstanceId": held_instance(0) }),
        ),
        (
            "IdentityProvider alone",
            json!({ "IdentityProvider": provider_union("d-0123456789") }),
        ),
        (
            "Username and InstanceId",
            json!({ "Username": "a.user", "InstanceId": held_instance(0) }),
        ),
        (
            "Username and IdentityProvider",
            json!({
                "Username": "a.user",
                "IdentityProvider": provider_union("d-0123456789"),
            }),
        ),
        (
            "InstanceId and IdentityProvider",
            json!({
                "InstanceId": held_instance(0),
                "IdentityProvider": provider_union("d-0123456789"),
            }),
        ),
        ("Domain alone", json!({ "Domain": "corp.example" })),
    ];

    for (case, body) in &incomplete {
        let reply = disassociate(&h, body).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?} with body {:?}",
            reply.error_type,
            reply.body
        );
        assert_eq!(
            reply.body.get("message").and_then(Value::as_str),
            Some(IDENTIFIERS_REQUIRED),
            "case {case:?}: expected {IDENTIFIERS_REQUIRED:?}, got {:?}",
            reply.body
        );
    }

    // The boundary: the SAME request with the triple completed is a different refusal —
    // the identifiers were fine, the association is not there.
    let complete = json!({
        "Username": "a.user",
        "InstanceId": held_instance(0),
        "IdentityProvider": provider_union("d-0123456789"),
    });
    let reply = disassociate(&h, &complete).await;
    assert_fails_closed("the complete triple, nothing stored", "a.user", &reply);
}

/// RULE: an identity-provider reference that resolves to no registration is refused with
/// `ValidationException` / "Identity provider is not valid" — on both surfaces that
/// resolve one, and BEFORE the rest of the request is validated, so a payload that would
/// also be refused does not change the answer.
#[tokio::test]
async fn an_unresolvable_identity_provider_reference_is_a_validation_failure() {
    let h = harness();
    let real = register_provider(
        &h,
        &registration("d-0123456789", PRODUCTS[0], &["subnet-0123456789abcdef0"], None),
        "a registration to miss",
    )
    .await;

    let unresolvable: Vec<(String, String)> = vec![
        (
            "an ARN of the form this service mints".to_string(),
            format!("{}ip-000000009999", arn_prefix("identity-provider")),
        ),
        (
            "a near miss on a registration that exists".to_string(),
            format!("{real}0"),
        ),
        (
            "an ARN in another account".to_string(),
            format!("arn:aws:{SIGNING_NAME}:{REGION}:111111111111:identity-provider/ip-000000000001"),
        ),
        (
            "an ARN in another Region".to_string(),
            format!("arn:aws:{SIGNING_NAME}:eu-west-1:{ACCOUNT}:identity-provider/ip-000000000001"),
        ),
    ];

    // The payload is varied across the rows, including the two payloads
    // `UpdateIdentityProviderSettings` would itself refuse (overlapping subnets, and one
    // that would leave no subnet): resolution short-circuits, so the answer is the same.
    let payloads: [(&str, Value); 4] = [
        (
            "overlapping add and remove",
            json!({
                "AddSubnets": ["subnet-0123456789abcdef0"],
                "RemoveSubnets": ["subnet-0123456789abcdef0"],
            }),
        ),
        (
            "disjoint add and remove",
            json!({
                "AddSubnets": ["subnet-1111111111111111a"],
                "RemoveSubnets": ["subnet-2222222222222222b"],
            }),
        ),
        (
            "nothing to add or remove",
            json!({ "AddSubnets": [], "RemoveSubnets": [] }),
        ),
        (
            "a security group only",
            json!({
                "AddSubnets": [],
                "RemoveSubnets": [],
                "SecurityGroupId": "sg-0123456789abcdef0",
            }),
        ),
    ];

    for (row, (class, arn)) in unresolvable.iter().enumerate() {
        let (payload_class, payload) = &payloads[row % payloads.len()];
        let case = format!("UpdateIdentityProviderSettings/{class}/{payload_class}");
        let reply = update_settings(
            &h,
            &json!({ "IdentityProviderArn": arn, "UpdateSettings": payload }),
        )
        .await;
        assert_identity_provider_not_valid(&case, &reply);

        let case = format!("CreateLicenseServerEndpoint/{class}");
        let reply = create_endpoint(
            &h,
            &json!({
                "IdentityProviderArn": arn,
                "LicenseServerSettings": license_server_settings("rds-sal-secret"),
            }),
        )
        .await;
        assert_identity_provider_not_valid(&case, &reply);
    }

    // Addressed by (IdentityProvider, Product) instead of by ARN: a directory with no
    // registration for the product named resolves to nothing the same way.
    for (class, product) in [
        ("a product the directory is not registered for", PRODUCTS[1]),
        ("another unregistered product", PRODUCTS[4]),
    ] {
        let reply = update_settings(
            &h,
            &json!({
                "IdentityProvider": provider_union("d-0123456789"),
                "Product": product,
                "UpdateSettings": { "AddSubnets": [], "RemoveSubnets": [] },
            }),
        )
        .await;
        assert_identity_provider_not_valid(class, &reply);
    }
}

/// RULE: the tagging surface resolves `ResourceArn` against state and refuses an ARN that
/// names no resource this service holds with `BadRequestException` / "Invalid input
/// resource arn" — the same refusal from `ListTagsForResource` and `UntagResource`,
/// because it is the same resolution.
#[tokio::test]
async fn the_tagging_surface_refuses_an_arn_that_names_no_resource() {
    let h = harness();
    let rows: Vec<(String, String)> = vec![
        (
            "an identity-provider ARN that was never registered".to_string(),
            format!("{}ip-000000009999", arn_prefix("identity-provider")),
        ),
        (
            "a product-user ARN that was never subscribed".to_string(),
            format!("{}pu-000000009999", arn_prefix("user")),
        ),
        (
            "an instance-user ARN that was never associated".to_string(),
            format!("{}iu-000000009999", arn_prefix("instance-user")),
        ),
        (
            "a license-server-endpoint ARN that was never created".to_string(),
            format!("{}lse-000000009999", arn_prefix("license-server-endpoint")),
        ),
        (
            "an ARN of another service's resource".to_string(),
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:instance/{}", held_instance(0)),
        ),
        (
            "an ARN in another account".to_string(),
            format!("arn:aws:{SIGNING_NAME}:{REGION}:111111111111:identity-provider/ip-000000000001"),
        ),
    ];

    for (class, arn) in &rows {
        assert_invalid_resource_arn(
            &format!("ListTagsForResource/{class}"),
            &list_tags(&h, arn).await,
        );
        assert_invalid_resource_arn(
            &format!("UntagResource/{class}"),
            &untag(&h, arn, &["team"]).await,
        );
        assert_invalid_resource_arn(
            &format!("TagResource/{class}"),
            &tag(&h, arn, &json!({ "team": "licensing" })).await,
        );
    }
}

/// RULE: the tag store is one store — what any operation accepting `Tags` wrote is what
/// `ListTagsForResource` reports, what `UntagResource` removes and what `TagResource`
/// adds to, for every resource kind that carries tags; and the resolution reads the
/// STORE, so a resource that was removed is refused exactly as one that never existed.
#[tokio::test]
async fn tags_round_trip_for_every_resource_kind_that_carries_them() {
    let h = harness();
    let provider_arn = register_provider(
        &h,
        &registration(
            "d-0123456789",
            PRODUCTS[0],
            &["subnet-0123456789abcdef0"],
            Some(json!({ "team": "licensing", "env": "test", "owner": "fixture" })),
        ),
        "a tagged registration",
    )
    .await;

    let association = associate(
        &h,
        &json!({
            "Username": "tagged.user",
            "InstanceId": held_instance(0),
            "IdentityProvider": provider_union("d-0123456789"),
            "Tags": { "team": "licensing", "env": "test", "owner": "fixture" },
        }),
    )
    .await;
    let association_arn = as_str(
        &association.ok("a tagged association")["InstanceUserSummary"],
        "InstanceUserArn",
        "a tagged association",
    )
    .to_string();

    let endpoint = create_endpoint(
        &h,
        &json!({
            "IdentityProviderArn": provider_arn,
            "LicenseServerSettings": license_server_settings("rds-sal-secret"),
            "Tags": { "team": "licensing", "env": "test", "owner": "fixture" },
        }),
    )
    .await;
    let endpoint_arn = as_str(
        endpoint.ok("a tagged endpoint"),
        "LicenseServerEndpointArn",
        "a tagged endpoint",
    )
    .to_string();

    // (case, the ARN, the keys to remove, what must be left)
    let rows: [(&str, &String, &[&str], &[&str]); 3] = [
        (
            "one key of three, from a registration",
            &provider_arn,
            &["env"],
            &["team", "owner"],
        ),
        (
            "two keys at once, from a user association",
            &association_arn,
            &["env", "owner"],
            &["team"],
        ),
        (
            "a key the resource does not carry, from an endpoint",
            &endpoint_arn,
            &["absent"],
            &["team", "env", "owner"],
        ),
    ];

    for (case, arn, remove, left) in rows {
        let before = list_tags(&h, arn).await;
        let tags = before.ok(case)["Tags"].clone();
        assert_eq!(
            tags.as_object().map(|map| map.len()),
            Some(3),
            "case {case:?}: the three tags written must be reported, got {tags:?}"
        );

        untag(&h, arn, remove).await.ok(case);
        let after = list_tags(&h, arn).await;
        let tags = after.ok(case)["Tags"].clone();
        let object = tags
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: Tags is not a map: {tags:?}"));
        assert_eq!(
            object.len(),
            left.len(),
            "case {case:?}: expected {} tags left, got {tags:?}",
            left.len()
        );
        for key in left {
            assert!(
                object.contains_key(*key),
                "case {case:?}: {key:?} must still be there, got {tags:?}"
            );
        }
        for key in remove {
            assert!(
                !object.contains_key(*key),
                "case {case:?}: {key:?} must be gone, got {tags:?}"
            );
        }

        // `TagResource` writes into the SAME store: it puts a removed key back and
        // overwrites one the resource already carries.
        tag(
            &h,
            arn,
            &json!({ "restored": "yes", "team": "a-new-value" }),
        )
        .await
        .ok(case);
        let written = list_tags(&h, arn).await;
        let tags = written.ok(case)["Tags"].clone();
        assert_eq!(
            tags.get("restored").and_then(Value::as_str),
            Some("yes"),
            "case {case:?}: a tag written must be reported, got {tags:?}"
        );
        assert_eq!(
            tags.get("team").and_then(Value::as_str),
            Some("a-new-value"),
            "case {case:?}: writing an existing key must overwrite it, got {tags:?}"
        );
    }

    // The resolution reads the store: deregistering the provider takes the endpoint
    // created for it with it, and both ARNs are then refused exactly as ARNs that never
    // named anything.
    call(
        &h,
        "/identity-provider/DeregisterIdentityProvider",
        &json!({ "IdentityProviderArn": provider_arn }),
    )
    .await
    .ok("deregistering the provider");
    for (case, arn) in [
        ("the deregistered provider", &provider_arn),
        ("its license server endpoint", &endpoint_arn),
    ] {
        assert_invalid_resource_arn(
            &format!("ListTagsForResource/{case}"),
            &list_tags(&h, arn).await,
        );
        assert_invalid_resource_arn(
            &format!("TagResource/{case}"),
            &tag(&h, arn, &json!({ "team": "licensing" })).await,
        );
    }
}

/// One association to exercise. The rows differ along what the rule generalises over: the
/// instance, the directory-id form, the user name, whether a `Domain` narrows it and
/// whether tags are supplied.
#[derive(Debug, Clone, Copy)]
struct Association {
    case: &'static str,
    instance: usize,
    directory_id: &'static str,
    username: &'static str,
    domain: Option<&'static str>,
    tagged: bool,
}

/// `DirectoryId` matches `^(d|sd)-[0-9a-f]{10}$`; both spellings appear below.
const ASSOCIATIONS: &[Association] = &[
    Association {
        case: "aws-managed directory, no domain, untagged",
        instance: 0,
        directory_id: "d-0123456789",
        username: "first.user",
        domain: None,
        tagged: false,
    },
    Association {
        case: "the same directory on another instance, with a domain",
        instance: 1,
        directory_id: "d-0123456789",
        username: "second.user",
        domain: Some("corp.example"),
        tagged: true,
    },
    Association {
        case: "a shared directory, a third instance",
        instance: 2,
        directory_id: "sd-abcdef0123",
        username: "third.user",
        domain: Some("example.internal"),
        tagged: false,
    },
    Association {
        case: "a second user on an instance that already has one",
        instance: 0,
        directory_id: "sd-abcdef0123",
        username: "fourth.user",
        domain: None,
        tagged: true,
    },
];

impl Association {
    fn request(&self) -> Value {
        let mut body = json!({
            "Username": self.username,
            "InstanceId": held_instance(self.instance),
            "IdentityProvider": provider_union(self.directory_id),
        });
        if let Some(domain) = self.domain {
            body["Domain"] = json!(domain);
        }
        if self.tagged {
            body["Tags"] = json!({ "team": "licensing" });
        }
        body
    }
}

/// RULE: an association is computed from, and persisted in, state — it is minted in the
/// non-terminal `ASSOCIATING` status with a minted ARN, re-associating the same user is
/// the stored association (`@idempotent`), the `ASSOCIATED` transition the clock owes is
/// materialised on a later read, and a disassociation removes it so the same identifiers
/// fail closed afterwards.
#[tokio::test]
async fn user_associations_round_trip_and_disassociation_removes_them() {
    let h = harness();
    for directory_id in ["d-0123456789", "sd-abcdef0123"] {
        register(
            &h,
            &registration(directory_id, PRODUCTS[0], &["subnet-0123456789abcdef0"], None),
        )
        .await
        .ok(directory_id);
    }

    let mut minted = Vec::new();
    for row in ASSOCIATIONS {
        let reply = associate(&h, &row.request()).await;
        let summary = reply.ok(row.case)["InstanceUserSummary"].clone();

        let arn = as_str(&summary, "InstanceUserArn", row.case);
        assert!(
            arn.starts_with(&arn_prefix("instance-user")),
            "case {:?}: expected an ARN under {}, got {arn:?}",
            row.case,
            arn_prefix("instance-user")
        );
        assert_eq!(
            as_str(&summary, "Status", row.case),
            "ASSOCIATING",
            "case {:?}: a fresh association is returned in the transient status, got {summary:?}",
            row.case
        );
        assert_eq!(
            as_str(&summary, "InstanceId", row.case),
            held_instance(row.instance),
            "case {:?}: the instance must be echoed from state, got {summary:?}",
            row.case
        );
        assert_eq!(
            as_str(&summary, "Username", row.case),
            row.username,
            "case {:?}: the user must be echoed from state, got {summary:?}",
            row.case
        );
        assert_eq!(
            summary.get("Domain").and_then(Value::as_str),
            row.domain,
            "case {:?}: Domain must be what was stored, got {summary:?}",
            row.case
        );
        let association_date = as_str(&summary, "AssociationDate", row.case);
        assert!(
            association_date.ends_with('Z') && association_date.len() == 20,
            "case {:?}: expected an ISO-8601 UTC instant, got {association_date:?}",
            row.case
        );
        assert!(
            summary.get("DisassociationDate").is_none(),
            "case {:?}: a live association has no disassociation date, got {summary:?}",
            row.case
        );

        // `@idempotent`: the same user, instance and directory is the SAME association.
        let again = associate(&h, &row.request()).await;
        assert_eq!(
            as_str(
                &again.ok(row.case)["InstanceUserSummary"],
                "InstanceUserArn",
                row.case
            ),
            arn,
            "case {:?}: re-associating must answer the stored association",
            row.case
        );

        minted.push((*row, arn.to_string()));
    }

    // The transition the clock owed is materialised on a later read — here the read a
    // disassociation performs, which reports the status the association had.
    tokio::time::sleep(Duration::from_millis(900)).await;

    for (row, arn) in &minted {
        // Half are addressed by ARN and half by the identifier triple: the model lets a
        // caller name an association either way.
        let by_arn = minted
            .iter()
            .position(|(candidate, _)| candidate.case == row.case)
            .unwrap_or_default()
            % 2
            == 0;
        let mut request = if by_arn {
            json!({ "InstanceUserArn": arn })
        } else {
            json!({
                "Username": row.username,
                "InstanceId": held_instance(row.instance),
                "IdentityProvider": provider_union(row.directory_id),
            })
        };
        if let Some(domain) = row.domain {
            request["Domain"] = json!(domain);
        }

        let reply = disassociate(&h, &request).await;
        let summary = reply.ok(row.case)["InstanceUserSummary"].clone();
        assert_eq!(
            as_str(&summary, "InstanceUserArn", row.case),
            arn,
            "case {:?}: the disassociation must answer the stored association, got {summary:?}",
            row.case
        );
        assert_eq!(
            as_str(&summary, "Status", row.case),
            "DISASSOCIATING",
            "case {:?}: the answer reports the status of the removal, got {summary:?}",
            row.case
        );
        let disassociation_date = as_str(&summary, "DisassociationDate", row.case);
        assert!(
            disassociation_date.ends_with('Z') && disassociation_date.len() == 20,
            "case {:?}: expected an ISO-8601 UTC instant, got {disassociation_date:?}",
            row.case
        );

        // Removed means removed: the same identifiers now fail closed, by either form.
        assert_fails_closed(
            &format!("{} (by ARN, after removal)", row.case),
            arn,
            &disassociate(&h, &json!({ "InstanceUserArn": arn })).await,
        );
        assert_fails_closed(
            &format!("{} (by triple, after removal)", row.case),
            row.username,
            &disassociate(
                &h,
                &json!({
                    "Username": row.username,
                    "InstanceId": held_instance(row.instance),
                    "IdentityProvider": provider_union(row.directory_id),
                }),
            )
            .await,
        );
    }
}

/// RULE: the `ASSOCIATING` status is the association's own, and the clock settles it — a
/// caller reading straight after the call still sees the transient status, and a later
/// read reports `ASSOCIATED`.
#[tokio::test]
async fn the_associating_status_settles_once_the_clock_owes_it() {
    let h = harness();
    register(
        &h,
        &registration("d-0123456789", PRODUCTS[0], &["subnet-0123456789abcdef0"], None),
    )
    .await
    .ok("a registration");
    let request = json!({
        "Username": "settling.user",
        "InstanceId": held_instance(0),
        "IdentityProvider": provider_union("d-0123456789"),
    });

    let immediate = associate(&h, &request).await;
    assert_eq!(
        as_str(
            &immediate.ok("immediate read")["InstanceUserSummary"],
            "Status",
            "immediate read"
        ),
        "ASSOCIATING",
        "a caller reading straight after the call still sees the transient status"
    );

    tokio::time::sleep(Duration::from_millis(900)).await;

    // The idempotent re-call is a READ of the stored association, so it reports the
    // transition the clock owed.
    let settled = associate(&h, &request).await;
    assert_eq!(
        as_str(
            &settled.ok("settled read")["InstanceUserSummary"],
            "Status",
            "settled read"
        ),
        "ASSOCIATED",
        "the transition the clock owed must be materialised on a later read"
    );
}

/// RULE: a user association names a DIRECTORY that must be registered for some product —
/// an unregistered directory on an authorized instance is the modelled
/// `ResourceNotFoundException`, which is a different refusal from the authorization gate.
#[tokio::test]
async fn associating_through_an_unregistered_directory_is_a_not_found() {
    let h = harness();
    register(
        &h,
        &registration("d-0123456789", PRODUCTS[2], &["subnet-0123456789abcdef0"], None),
    )
    .await
    .ok("a registration for one directory");

    for (case, directory_id) in [
        ("a directory with no registration at all", "d-9999999999"),
        ("a shared directory with no registration", "sd-abcdef0123"),
        ("a near miss on the registered directory", "d-0123456788"),
    ] {
        let reply = associate(
            &h,
            &json!({
                "Username": "a.user",
                "InstanceId": held_instance(0),
                "IdentityProvider": provider_union(directory_id),
            }),
        )
        .await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected HTTP 404, got {} with body {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?} with body {:?}",
            reply.error_type,
            reply.body
        );
    }

    // The boundary: the registered directory on the same instance is associated.
    let reply = associate(
        &h,
        &json!({
            "Username": "a.user",
            "InstanceId": held_instance(0),
            "IdentityProvider": provider_union("d-0123456789"),
        }),
    )
    .await;
    reply.ok("the registered directory");
}

/// RULE: a license server endpoint is computed from, and persisted in, state — it is
/// created against the registration the request names, reported with the members state
/// holds (never an invented address or health status), and a deletion removes it so the
/// same ARN fails closed afterwards.
#[tokio::test]
async fn license_server_endpoints_round_trip_and_deletion_removes_them() {
    let h = harness();
    // The rows differ in the registration the endpoint hangs off (two directory forms,
    // two products) and in the secret the credentials provider names.
    let rows: [(&str, &str, &str, &str); 3] = [
        (
            "aws-managed directory, remote desktop services",
            "d-0123456789",
            PRODUCTS[4],
            "rds-sal/secret-1",
        ),
        (
            "shared directory, office standard",
            "sd-abcdef0123",
            PRODUCTS[3],
            "s",
        ),
        (
            "aws-managed directory, visual studio",
            "d-00000000ff",
            PRODUCTS[0],
            "a-rather-longer-secret-identifier",
        ),
    ];

    for (case, directory_id, product, secret_id) in rows {
        let provider_arn = register_provider(
            &h,
            &registration(directory_id, product, &["subnet-0123456789abcdef0"], None),
            case,
        )
        .await;

        let created = create_endpoint(
            &h,
            &json!({
                "IdentityProviderArn": provider_arn,
                "LicenseServerSettings": license_server_settings(secret_id),
            }),
        )
        .await;
        let body = created.ok(case).clone();
        assert_eq!(
            body.get("IdentityProviderArn").and_then(Value::as_str),
            Some(provider_arn.as_str()),
            "case {case:?}: the answer must echo the registration it resolved, got {body:?}"
        );
        let endpoint_arn = as_str(&body, "LicenseServerEndpointArn", case).to_string();
        assert!(
            endpoint_arn.starts_with(&arn_prefix("license-server-endpoint")),
            "case {case:?}: expected an ARN under {}, got {endpoint_arn:?}",
            arn_prefix("license-server-endpoint")
        );

        // The deletion is the read-back: it reports the endpoint state held.
        let deleted = delete_endpoint(
            &h,
            &json!({ "LicenseServerEndpointArn": endpoint_arn, "ServerType": "RDS_SAL" }),
        )
        .await;
        let endpoint = deleted.ok(case)["LicenseServerEndpoint"].clone();
        assert_eq!(
            endpoint.get("LicenseServerEndpointArn").and_then(Value::as_str),
            Some(endpoint_arn.as_str()),
            "case {case:?}: the deletion must answer the stored endpoint, got {endpoint:?}"
        );
        assert_eq!(
            endpoint.get("IdentityProviderArn").and_then(Value::as_str),
            Some(provider_arn.as_str()),
            "case {case:?}: the endpoint must still name its registration, got {endpoint:?}"
        );
        assert_eq!(
            endpoint.get("ServerType").and_then(Value::as_str),
            Some("RDS_SAL"),
            "case {case:?}: the stored ServerType must be reported, got {endpoint:?}"
        );
        assert_eq!(
            endpoint
                .get("LicenseServerEndpointProvisioningStatus")
                .and_then(Value::as_str),
            Some("DELETING"),
            "case {case:?}: the answer reports the status of the removal, got {endpoint:?}"
        );
        assert!(
            endpoint.get("LicenseServerEndpointId").and_then(Value::as_str).is_some(),
            "case {case:?}: the minted endpoint id must be reported, got {endpoint:?}"
        );
        assert!(
            endpoint.get("CreationTime").is_some(),
            "case {case:?}: the creation instant must be reported, got {endpoint:?}"
        );
        // Nothing provisions a license server, so nothing is reported about one.
        for absent in ["ServerEndpoint", "LicenseServers"] {
            assert!(
                endpoint.get(absent).is_none(),
                "case {case:?}: {absent} must be absent rather than invented, got {endpoint:?}"
            );
        }

        // Removed means removed: the same ARN fails closed afterwards.
        assert_fails_closed(
            &format!("{case} (after deletion)"),
            &endpoint_arn,
            &delete_endpoint(
                &h,
                &json!({ "LicenseServerEndpointArn": endpoint_arn, "ServerType": "RDS_SAL" }),
            )
            .await,
        );
    }
}

/// RULE: a user association is also what brings an EC2 instance into this service's view
/// — `ListInstances` reports exactly the instances state holds, with the products the
/// association's directory is registered for, and an instance leaves the view with the
/// last association that names it.
#[tokio::test]
async fn an_association_brings_its_instance_into_the_instance_list() {
    let h = harness();
    // Two directories, registered for different products, so what the list reports about
    // an instance is read from the registrations rather than taken from the request.
    for (directory_id, product) in [("d-0123456789", PRODUCTS[0]), ("sd-abcdef0123", PRODUCTS[4])] {
        register(
            &h,
            &registration(directory_id, product, &["subnet-0123456789abcdef0"], None),
        )
        .await
        .ok(directory_id);
    }

    let listed = call(&h, "/instance/ListInstances", &json!({})).await;
    assert_eq!(
        listed.ok("before any association")["InstanceSummaries"]
            .as_array()
            .map(Vec::len),
        Some(0),
        "no instance is in view before a user is associated, got {:?}",
        listed.body
    );

    // (case, the instance, the directory, the product that directory is registered for)
    let rows: [(&str, usize, &str, &str); 3] = [
        ("the first instance, aws-managed directory", 0, "d-0123456789", PRODUCTS[0]),
        ("another instance, shared directory", 1, "sd-abcdef0123", PRODUCTS[4]),
        ("a second user on the first instance", 0, "d-0123456789", PRODUCTS[0]),
    ];
    for (row, (case, instance, directory_id, product)) in rows.iter().enumerate() {
        associate(
            &h,
            &json!({
                "Username": format!("listed.user{row}"),
                "InstanceId": held_instance(*instance),
                "IdentityProvider": provider_union(directory_id),
            }),
        )
        .await
        .ok(case);

        let listed = call(&h, "/instance/ListInstances", &json!({})).await;
        let summaries = listed.ok(case)["InstanceSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no InstanceSummaries in {:?}", listed.body))
            .clone();
        let reported: Vec<&str> = summaries
            .iter()
            .map(|summary| as_str(summary, "InstanceId", case))
            .collect();
        let expected: Vec<String> = rows[..=row]
            .iter()
            .map(|(_, instance, _, _)| held_instance(*instance))
            .fold(Vec::new(), |mut seen, id| {
                if !seen.contains(&id) {
                    seen.push(id);
                }
                seen
            });
        assert_eq!(
            reported, expected,
            "case {case:?}: the list must report exactly the instances in view, got {summaries:?}"
        );

        let summary = summaries
            .iter()
            .find(|summary| {
                summary.get("InstanceId").and_then(Value::as_str) == Some(&held_instance(*instance))
            })
            .unwrap_or_else(|| panic!("case {case:?}: the instance is not listed: {summaries:?}"));
        assert_eq!(
            as_str(summary, "Status", case),
            "ACTIVE",
            "case {case:?}: an instance with an association is in view as ACTIVE, got {summary:?}"
        );
        let products: Vec<&str> = summary["Products"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no Products in {summary:?}"))
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(
            products,
            [*product],
            "case {case:?}: the products must be the directory's registrations, got {summary:?}"
        );
        assert_eq!(
            summary["IdentityProvider"]["ActiveDirectoryIdentityProvider"]["DirectoryId"].as_str(),
            Some(*directory_id),
            "case {case:?}: the directory the association named must be reported, got {summary:?}"
        );
        assert!(
            summary.get("LastStatusCheckDate").is_none(),
            "case {case:?}: no status check was performed, so none is reported, got {summary:?}"
        );
    }

    // The instance with two associations stays in view until the second one goes.
    for (row, (_, instance, directory_id, _)) in rows.iter().enumerate() {
        disassociate(
            &h,
            &json!({
                "Username": format!("listed.user{row}"),
                "InstanceId": held_instance(*instance),
                "IdentityProvider": provider_union(directory_id),
            }),
        )
        .await
        .ok("disassociating");
    }
    let listed = call(&h, "/instance/ListInstances", &json!({})).await;
    assert_eq!(
        listed.ok("after every association was removed")["InstanceSummaries"]
            .as_array()
            .map(Vec::len),
        Some(0),
        "an instance leaves the view with its last association, got {:?}",
        listed.body
    );
}

/// RULE: what the instance view reports is a FUNCTION of the state at the read — an
/// instance associated through two directories reports both their products, and stops
/// reporting a product as soon as the association (or the registration) behind it is
/// gone.
#[tokio::test]
async fn the_instance_view_is_recomputed_from_the_associations_that_remain() {
    let h = harness();
    for (directory_id, product) in [("d-0123456789", PRODUCTS[0]), ("sd-abcdef0123", PRODUCTS[4])] {
        register(
            &h,
            &registration(directory_id, product, &["subnet-0123456789abcdef0"], None),
        )
        .await
        .ok(directory_id);
    }
    // Two users on ONE instance, through two different directories.
    for (username, directory_id) in [("user.x", "d-0123456789"), ("user.y", "sd-abcdef0123")] {
        associate(
            &h,
            &json!({
                "Username": username,
                "InstanceId": held_instance(0),
                "IdentityProvider": provider_union(directory_id),
            }),
        )
        .await
        .ok(username);
    }

    let products = |reply: &Reply, case: &str| -> Vec<String> {
        reply.ok(case)["InstanceSummaries"][0]["Products"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no Products in {:?}", reply.body))
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect()
    };

    let listed = call(&h, "/instance/ListInstances", &json!({})).await;
    assert_eq!(
        products(&listed, "both directories"),
        vec![PRODUCTS[0].to_string(), PRODUCTS[4].to_string()],
        "both directories' products are reported while both associations exist"
    );

    // Dropping one association drops the product only that directory provided.
    disassociate(
        &h,
        &json!({
            "Username": "user.y",
            "InstanceId": held_instance(0),
            "IdentityProvider": provider_union("sd-abcdef0123"),
        }),
    )
    .await
    .ok("dropping the second directory's association");
    let listed = call(&h, "/instance/ListInstances", &json!({})).await;
    assert_eq!(
        products(&listed, "one directory left"),
        vec![PRODUCTS[0].to_string()],
        "a product must stop being reported with the association behind it"
    );

    // Deregistering the remaining directory's product is the same rule by another route:
    // the view is read from the registrations, so the product goes with the registration.
    call(
        &h,
        "/identity-provider/DeregisterIdentityProvider",
        &json!({
            "IdentityProvider": provider_union("d-0123456789"),
            "Product": PRODUCTS[0],
        }),
    )
    .await
    .ok("deregistering the directory's product");
    let listed = call(&h, "/instance/ListInstances", &json!({})).await;
    assert_eq!(
        products(&listed, "no registration left"),
        Vec::<String>::new(),
        "a product must stop being reported with its registration, got {:?}",
        listed.body
    );
}

/// RULE: the work and the state one request can buy are BOUNDED, and the refusal says so
/// rather than the service doing it: an `UpdateSettings` payload may name at most
/// `MAX_SUBNETS_PER_UPDATE` subnets per list (EC2's default quota of 200 subnets per VPC
/// — the model puts no maximum on `Subnets`), and a resource kind holds at most
/// `MAX_RESOURCES_PER_KIND` resources, beyond which the modelled
/// `ServiceQuotaExceededException` is answered.
#[tokio::test]
async fn a_request_cannot_buy_unbounded_work_or_state() {
    let h = harness();
    let provider_arn = register_provider(
        &h,
        &registration(
            "d-0123456789",
            PRODUCTS[0],
            &["subnet-0123456789abcdef0"],
            None,
        ),
        "a registration to update",
    )
    .await;

    // `Subnet` matches `^subnet-[a-z0-9]{8,17}`, so an 8-hex-digit id is the shortest the
    // model accepts and makes a distinct value per index.
    let subnets = |count: usize| -> Vec<String> {
        (0..count).map(|i| format!("subnet-{i:08x}")).collect()
    };

    // Each published bound, and one past it, on both lists.
    let rows: [(&str, Value, bool); 4] = [
        (
            "AddSubnets at the limit",
            json!({ "AddSubnets": subnets(MAX_SUBNETS_PER_UPDATE), "RemoveSubnets": [] }),
            true,
        ),
        (
            "AddSubnets one past the limit",
            json!({ "AddSubnets": subnets(MAX_SUBNETS_PER_UPDATE + 1), "RemoveSubnets": [] }),
            false,
        ),
        (
            "RemoveSubnets at the limit",
            json!({ "AddSubnets": [], "RemoveSubnets": subnets(MAX_SUBNETS_PER_UPDATE) }),
            true,
        ),
        (
            "RemoveSubnets one past the limit",
            json!({ "AddSubnets": [], "RemoveSubnets": subnets(MAX_SUBNETS_PER_UPDATE + 1) }),
            false,
        ),
    ];
    for (case, payload, accepted) in &rows {
        let reply = update_settings(
            &h,
            &json!({ "IdentityProviderArn": provider_arn, "UpdateSettings": payload }),
        )
        .await;
        if *accepted {
            reply.ok(case);
        } else {
            assert_eq!(
                reply.status, 400,
                "case {case:?}: expected HTTP 400, got {} with body {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_type.as_deref(),
                Some("ValidationException"),
                "case {case:?}: expected ValidationException, got {:?} with body {:?}",
                reply.error_type,
                reply.body
            );
        }
    }

    // The store bound: fill a kind to its limit through the store the handlers read, then
    // the next create is refused with the modelled quota error.
    let association = associate(
        &h,
        &json!({
            "Username": "quota.user",
            "InstanceId": held_instance(0),
            "IdentityProvider": provider_union("d-0123456789"),
        }),
    )
    .await;
    association.ok("one association to clone");
    {
        let store = h
            .engine
            .state::<ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState>();
        let mut guard = store.lock();
        let template = guard
            .instance_users
            .values()
            .next()
            .cloned()
            .expect("the association is in the store");
        while guard.instance_users.len() < MAX_RESOURCES_PER_KIND {
            let arn = format!("{}filler{}", arn_prefix("instance-user"), guard.instance_users.len());
            let mut filler = template.clone();
            filler.arn = arn.clone();
            guard.instance_users.insert(arn, filler);
        }
    }

    let reply = associate(
        &h,
        &json!({
            "Username": "one.too.many",
            "InstanceId": held_instance(1),
            "IdentityProvider": provider_union("d-0123456789"),
        }),
    )
    .await;
    assert_eq!(
        reply.status, 400,
        "at the limit: expected HTTP 400, got {} with body {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("ServiceQuotaExceededException"),
        "at the limit: expected the modelled quota error, got {:?} with body {:?}",
        reply.error_type,
        reply.body
    );
}

/// RULE: the tag set one resource carries is bounded too — the model bounds a single
/// payload (`Tags`, length 0..50) but not the accumulation of repeated writes, so a
/// `TagResource` that would carry a resource past `MAX_TAGS_PER_RESOURCE` is refused
/// WHOLE and nothing is written; overwriting a key the resource already carries is not
/// growth and is accepted at the limit.
#[tokio::test]
async fn the_tag_set_one_resource_carries_is_bounded() {
    let h = harness();
    let provider_arn = register_provider(
        &h,
        &registration(
            "d-0123456789",
            PRODUCTS[0],
            &["subnet-0123456789abcdef0"],
            None,
        ),
        "a registration to tag",
    )
    .await;

    let tag_map = |from: usize, count: usize| -> Value {
        let mut tags = serde_json::Map::new();
        for i in from..from + count {
            tags.insert(format!("key{i}"), Value::String(format!("value{i}")));
        }
        Value::Object(tags)
    };

    // At the bound: a resource with no tags takes exactly the limit.
    tag(&h, &provider_arn, &tag_map(0, MAX_TAGS_PER_RESOURCE))
        .await
        .ok("the whole limit in one payload");
    let held = list_tags(&h, &provider_arn).await;
    assert_eq!(
        held.ok("at the limit")["Tags"].as_object().map(|m| m.len()),
        Some(MAX_TAGS_PER_RESOURCE),
        "the resource must carry the tags written, got {:?}",
        held.body
    );

    // One past it: a single fresh key is refused, and nothing is written.
    let reply = tag(&h, &provider_arn, &tag_map(MAX_TAGS_PER_RESOURCE, 1)).await;
    assert_eq!(
        reply.status, 400,
        "one past the limit: expected HTTP 400, got {} with body {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("ValidationException"),
        "one past the limit: expected ValidationException, got {:?} with body {:?}",
        reply.error_type,
        reply.body
    );
    let held = list_tags(&h, &provider_arn).await;
    let tags = held.ok("after the refusal")["Tags"].clone();
    let object = tags.as_object().expect("Tags is a map");
    assert_eq!(
        object.len(),
        MAX_TAGS_PER_RESOURCE,
        "a refused merge must leave the stored tags untouched, got {tags:?}"
    );
    assert!(
        !object.contains_key(&format!("key{MAX_TAGS_PER_RESOURCE}")),
        "a refused merge must write nothing, got {tags:?}"
    );

    // At the limit, a write that only OVERWRITES existing keys is not growth.
    tag(&h, &provider_arn, &json!({ "key0": "a-new-value" }))
        .await
        .ok("overwriting at the limit");
    let held = list_tags(&h, &provider_arn).await;
    let tags = held.ok("after the overwrite")["Tags"].clone();
    assert_eq!(
        tags.get("key0").and_then(Value::as_str),
        Some("a-new-value"),
        "an existing key must be overwritten at the limit, got {tags:?}"
    );
    assert_eq!(
        tags.as_object().map(|m| m.len()),
        Some(MAX_TAGS_PER_RESOURCE),
        "an overwrite does not grow the set, got {tags:?}"
    );

    // Removing a key makes room again: the bound is on what the resource carries.
    untag(&h, &provider_arn, &["key0"]).await.ok("making room");
    tag(&h, &provider_arn, &tag_map(MAX_TAGS_PER_RESOURCE, 1))
        .await
        .ok("a fresh key once there is room");
    let held = list_tags(&h, &provider_arn).await;
    assert_eq!(
        held.ok("back at the limit")["Tags"]
            .as_object()
            .map(|m| m.len()),
        Some(MAX_TAGS_PER_RESOURCE),
        "got {:?}",
        held.body
    );
}

/// RULE: `UpdateIdentityProviderSettings` applies its payload to the settings the
/// registration holds — the subnets it had, plus `AddSubnets`, minus `RemoveSubnets`,
/// with `SecurityGroupId` replaced when named — and persists the result, so a later read
/// reports it; a payload that cannot be applied is refused rather than half-applied.
#[tokio::test]
async fn updating_settings_applies_the_payload_and_persists_it() {
    // (case, the payload, the subnets expected after it, the security group expected)
    let rows: [(&str, Value, &[&str], &str); 5] = [
        (
            "one subnet added",
            json!({ "AddSubnets": ["subnet-1111111111111111a"], "RemoveSubnets": [] }),
            &["subnet-0123456789abcdef0", "subnet-3333333333333333c", "subnet-1111111111111111a"],
            "sg-0123abcd",
        ),
        (
            "two added and one of the originals removed",
            json!({
                "AddSubnets": ["subnet-1111111111111111a", "subnet-2222222222222222b"],
                "RemoveSubnets": ["subnet-3333333333333333c"],
            }),
            &[
                "subnet-0123456789abcdef0",
                "subnet-1111111111111111a",
                "subnet-2222222222222222b",
            ],
            "sg-0123abcd",
        ),
        (
            "a subnet already held, added again",
            json!({ "AddSubnets": ["subnet-0123456789abcdef0"], "RemoveSubnets": [] }),
            &["subnet-0123456789abcdef0", "subnet-3333333333333333c"],
            "sg-0123abcd",
        ),
        (
            "a security group only",
            json!({
                "AddSubnets": [],
                "RemoveSubnets": [],
                "SecurityGroupId": "sg-0123456789abcdef0",
            }),
            &["subnet-0123456789abcdef0", "subnet-3333333333333333c"],
            "sg-0123456789abcdef0",
        ),
        (
            "a subnet the registration does not carry, removed",
            json!({ "AddSubnets": [], "RemoveSubnets": ["subnet-9999999999999999z"] }),
            &["subnet-0123456789abcdef0", "subnet-3333333333333333c"],
            "sg-0123abcd",
        ),
    ];

    for (row, (case, payload, expected_subnets, expected_group)) in rows.iter().enumerate() {
        // A registration per row, so each row starts from the same two subnets.
        let h = harness();
        let directory_id = "d-0123456789";
        let product = PRODUCTS[row % PRODUCTS.len()];
        let provider_arn = register_provider(
            &h,
            &registration(
                directory_id,
                product,
                &["subnet-0123456789abcdef0", "subnet-3333333333333333c"],
                None,
            ),
            case,
        )
        .await;

        let updated = update_settings(
            &h,
            &json!({ "IdentityProviderArn": provider_arn, "UpdateSettings": payload }),
        )
        .await;
        let summary = updated.ok(case)["IdentityProviderSummary"].clone();
        let settings = summary["Settings"].clone();
        let subnets: Vec<&str> = settings["Subnets"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Subnets is not a list: {settings:?}"))
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(
            subnets, *expected_subnets,
            "case {case:?}: expected {expected_subnets:?}, got {subnets:?}"
        );
        assert_eq!(
            settings.get("SecurityGroupId").and_then(Value::as_str),
            Some(*expected_group),
            "case {case:?}: expected {expected_group:?}, got {settings:?}"
        );

        // Persisted, not just answered: a later read reports the same settings.
        let listed = list_providers(&h, &json!({})).await;
        let stored = listed.ok(case)["IdentityProviderSummaries"][0]["Settings"].clone();
        assert_eq!(
            stored, settings,
            "case {case:?}: the update must be persisted, got {stored:?}"
        );
    }
}

/// RULE: a payload that cannot be applied is refused as invalid — a subnet in both
/// `AddSubnets` and `RemoveSubnets`, and a payload that would leave the registration with
/// no subnet at all (`Settings$Subnets` is `@required` with `@length(min: 1)`).
#[tokio::test]
async fn a_payload_that_cannot_be_applied_is_refused() {
    let h = harness();
    let provider_arn = register_provider(
        &h,
        &registration(
            "d-0123456789",
            PRODUCTS[0],
            &["subnet-0123456789abcdef0", "subnet-3333333333333333c"],
            None,
        ),
        "a registration to update",
    )
    .await;

    let rows: [(&str, Value); 4] = [
        (
            "one subnet both added and removed",
            json!({
                "AddSubnets": ["subnet-1111111111111111a"],
                "RemoveSubnets": ["subnet-1111111111111111a"],
            }),
        ),
        (
            "a subnet both added and removed among others",
            json!({
                "AddSubnets": ["subnet-1111111111111111a", "subnet-2222222222222222b"],
                "RemoveSubnets": ["subnet-2222222222222222b"],
            }),
        ),
        (
            "every held subnet removed",
            json!({
                "AddSubnets": [],
                "RemoveSubnets": ["subnet-0123456789abcdef0", "subnet-3333333333333333c"],
            }),
        ),
        (
            "every held subnet removed and nothing usable added",
            json!({
                "AddSubnets": ["subnet-0123456789abcdef0"],
                "RemoveSubnets": ["subnet-0123456789abcdef0", "subnet-3333333333333333c"],
            }),
        ),
    ];

    for (case, payload) in &rows {
        let reply = update_settings(
            &h,
            &json!({ "IdentityProviderArn": provider_arn, "UpdateSettings": payload }),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?} with body {:?}",
            reply.error_type,
            reply.body
        );
    }

    // Nothing was half-applied: the settings are the ones the registration started with.
    let listed = list_providers(&h, &json!({})).await;
    let settings = listed.ok("after the refusals")["IdentityProviderSummaries"][0]["Settings"]
        .clone();
    let subnets: Vec<&str> = settings["Subnets"]
        .as_array()
        .expect("Subnets is a list")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(
        subnets,
        ["subnet-0123456789abcdef0", "subnet-3333333333333333c"],
        "a refused payload must leave the stored settings untouched, got {settings:?}"
    );
}
