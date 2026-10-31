// SPDX-License-Identifier: CC0-1.0

//! What a REPEAT `CreateLicenseServerEndpoint` names, driven over the real wire path: every
//! call goes through the router's `dispatch`, so the request is deserialized,
//! constraint-checked, handled and serialized exactly as a client's would be.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! * **The `LicenseServerSettings` a create carries are part of WHICH endpoint it asks
//!   for**, not a comment on an endpoint that already exists: a repeat create for a pair
//!   state already holds an endpoint for is served — the stored ARN, HTTP 200 — only when
//!   it carries the settings that endpoint was built from, and one carrying other settings
//!   is refused with the modelled `ConflictException` instead of being told, with a 200,
//!   that nothing had to change. The refused call stores nothing: the endpoint keeps the
//!   settings the create that minted it supplied, which is what still resolves it.
//! * **The comparison is against the LIVE endpoint of the pair**: once the endpoint is
//!   deleted, the pair is free, so a create naming the settings that conflicted a moment
//!   earlier mints its own endpoint — and from then on it is those settings that resolve
//!   the endpoint and the earlier ones that conflict.

use serde_json::{json, Value};

// ONE wire harness per service: the dispatch plumbing, the registration helpers and the
// endpoint fixtures come from the sibling suites rather than a second copy.
use super::identity_provider_subscriptions::{
    as_str, harness, provider_union, register, Harness, Reply, PRODUCTS,
};
use super::license_server_endpoint_deletion::delete_endpoint;
use super::license_server_endpoints::{create_endpoint, list_endpoints, listed_arns, SERVER_TYPE};

/// An AWS-managed directory the shared harness puts in the account's directory fleet, so
/// a registration for it is accepted.
const DIRECTORY: &str = "d-0123456789";

/// A subnet and a security group the shared harness creates in the account's Amazon EC2
/// store, so the registrations below resolve their `Settings`.
const SUBNET: &str = "subnet-0123456789abcdef0";
const SECURITY_GROUP: &str = "sg-0123abcd";

/// The `LicenseServerSettings` one create carries.
///
/// The rows differ along the one value the settings tree varies by: the Secrets Manager
/// secret the RDS SAL credentials provider names. That member is
/// `SecretsManagerCredentialsProvider$SecretId` — optional, `@length(min: 1)`
/// (`smithy_query(query="shape", shape="SecretsManagerCredentialsProvider")`) — so a
/// provider naming no secret at all is one of the forms a client can send, and it is its
/// own value rather than equal to any secret id.
#[derive(Debug, Clone, Copy)]
struct ServerSettingsRow {
    case: &'static str,
    secret_id: Option<&'static str>,
}

const SERVER_SETTINGS: &[ServerSettingsRow] = &[
    ServerSettingsRow {
        case: "a secret name",
        secret_id: Some("rds-sal-admin"),
    },
    ServerSettingsRow {
        case: "a secret ARN",
        secret_id: Some("arn:aws:secretsmanager:us-west-2:000000000000:secret:rds-sal-AbCdEf"),
    },
    ServerSettingsRow {
        case: "a path-form secret name",
        secret_id: Some("rds/sal/office"),
    },
    ServerSettingsRow {
        case: "a rotated secret, one character from the first row",
        secret_id: Some("rds-sal-admin2"),
    },
    ServerSettingsRow {
        case: "no SecretId at all",
        secret_id: None,
    },
];

impl ServerSettingsRow {
    /// The `LicenseServerSettings` member on its own.
    fn license_server_settings(self) -> Value {
        let mut credentials_provider = json!({});
        if let Some(secret_id) = self.secret_id {
            credentials_provider["SecretId"] = json!(secret_id);
        }
        json!({
            "ServerType": SERVER_TYPE,
            "ServerSettings": {
                "RdsSalSettings": {
                    "RdsSalCredentialsProvider": {
                        "SecretsManagerCredentialsProvider": credentials_provider,
                    },
                },
            },
        })
    }

    /// The whole create request, for the registration `provider_arn` names.
    fn request(self, provider_arn: &str) -> Value {
        json!({
            "IdentityProviderArn": provider_arn,
            "LicenseServerSettings": self.license_server_settings(),
        })
    }
}

/// Register one identity provider for `product` and answer its ARN.
///
/// A row needs a registration of its own because one `(IdentityProviderArn, ServerType)`
/// pair carries one endpoint, and the only `ServerType` the model admits is
/// [`SERVER_TYPE`] — so the pair is varied by the registration.
async fn registered_provider(h: &Harness, product: &str) -> String {
    let reply = register(
        h,
        &json!({
            "IdentityProvider": provider_union(DIRECTORY),
            "Product": product,
            "Settings": { "Subnets": [SUBNET], "SecurityGroupId": SECURITY_GROUP },
        }),
    )
    .await;
    as_str(
        &reply.ok(product)["IdentityProviderSummary"],
        "IdentityProviderArn",
        product,
    )
    .to_string()
}

/// The `LicenseServerEndpointArn` a served create answered.
fn created_arn(reply: &Reply, case: &str) -> String {
    as_str(reply.ok(case), "LicenseServerEndpointArn", case).to_string()
}

/// Assert `reply` is the modelled conflict with the endpoint the pair already holds, and
/// that it discloses nothing out of the settings the refused request carried.
fn assert_conflicts_with(reply: &Reply, case: &str, endpoint_arn: &str, secret_id: Option<&str>) {
    // `ConflictException` carries `@error("server")` and no `@httpError` in this model, so
    // it is reported at HTTP 500 (`smithy_query(query="operation",
    // operation="CreateLicenseServerEndpoint")`).
    assert_eq!(
        reply.status, 500,
        "case {case:?}: ConflictException is modelled at HTTP 500, got {reply:?}"
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("ConflictException"),
        "case {case:?}: expected ConflictException, got {reply:?}"
    );
    let message = reply.message().unwrap_or_default();
    assert!(
        message.contains(endpoint_arn),
        "case {case:?}: the refusal must name the endpoint the pair already holds \
         ({endpoint_arn}), got {message:?}"
    );
    if let Some(secret_id) = secret_id {
        assert!(
            !message.contains(secret_id),
            "case {case:?}: a refusal must not echo the credentials reference it was \
             sent, got {message:?}"
        );
    }
}

/// RULE: the settings a create carries are part of which endpoint it asks for — a repeat
/// create is served with the stored endpoint only when it carries the settings that
/// endpoint was built from, and one naming other settings conflicts with the endpoint in
/// state, changing nothing.
#[tokio::test]
async fn a_repeat_create_naming_other_settings_conflicts_rather_than_being_served() {
    let h = harness();
    assert!(
        SERVER_SETTINGS.len() <= PRODUCTS.len(),
        "every row needs a registration of its own, so there must be a product per row: \
         {} rows against {} products",
        SERVER_SETTINGS.len(),
        PRODUCTS.len()
    );

    // One endpoint per row, each minted from that row's settings: five distinct settings
    // coexist, so nothing below refuses settings for being the wrong SHAPE.
    let mut minted: Vec<(ServerSettingsRow, String, String)> = Vec::new();
    for (index, row) in SERVER_SETTINGS.iter().enumerate() {
        let provider_arn = registered_provider(&h, PRODUCTS[index]).await;
        let reply = create_endpoint(&h, &row.request(&provider_arn)).await;
        let endpoint_arn = created_arn(&reply, row.case);
        minted.push((*row, provider_arn, endpoint_arn));
    }

    for (row, provider_arn, endpoint_arn) in &minted {
        // The repeat of the create that MINTED the endpoint is the idempotent one: the
        // stored ARN, and nothing minted beside it.
        let case = format!("{}: repeated as created", row.case);
        let repeat = create_endpoint(&h, &row.request(provider_arn)).await;
        assert_eq!(
            &created_arn(&repeat, &case),
            endpoint_arn,
            "case {case:?}: a repeat of the minting create must answer the stored endpoint"
        );

        for other in SERVER_SETTINGS.iter().filter(|other| other.case != row.case) {
            let case = format!("{} created, {} requested", row.case, other.case);
            let reply = create_endpoint(&h, &other.request(provider_arn)).await;
            assert_conflicts_with(&reply, &case, endpoint_arn, other.secret_id);
        }

        // Nothing those refusals carried was stored: the endpoint is still resolved by
        // the settings it was minted with, which it would not be had a refused request's
        // payload replaced them.
        let case = format!("{}: after every refused repeat", row.case);
        let after = create_endpoint(&h, &row.request(provider_arn)).await;
        assert_eq!(
            &created_arn(&after, &case),
            endpoint_arn,
            "case {case:?}: a refused create must leave the stored settings as they were"
        );
    }

    // And no endpoint was minted beyond the one each pair holds.
    let expected: Vec<String> = minted
        .iter()
        .map(|(_, _, endpoint_arn)| endpoint_arn.clone())
        .collect();
    let reply = list_endpoints(&h, &json!({})).await;
    assert_eq!(
        listed_arns(reply.ok("no endpoint minted twice"), "no endpoint minted twice"),
        expected,
        "neither a served repeat nor a refused one may add a record, got {:?}",
        reply.body
    );
}

/// RULE: the settings compared are the LIVE endpoint's — a pair whose endpoint was deleted
/// is free, so the settings that conflicted with it mint their own endpoint, and from then
/// on those are the settings that resolve the pair.
#[tokio::test]
async fn the_settings_compared_are_the_live_endpoints() {
    let h = harness();
    let first = SERVER_SETTINGS[0];
    let second = SERVER_SETTINGS[1];

    for product in [PRODUCTS[0], PRODUCTS[1]] {
        let provider_arn = registered_provider(&h, product).await;

        let case = format!("{product}: created from {}", first.case);
        let created = create_endpoint(&h, &first.request(&provider_arn)).await;
        let endpoint_arn = created_arn(&created, &case);

        let conflict = create_endpoint(&h, &second.request(&provider_arn)).await;
        assert_conflicts_with(
            &conflict,
            &format!("{product}: {} against the live endpoint", second.case),
            &endpoint_arn,
            second.secret_id,
        );

        let deleted = delete_endpoint(&h, &endpoint_arn).await;
        deleted.ok(&format!("{product}: delete the endpoint of the pair"));

        // The pair is free, so the settings that conflicted a moment ago are now the
        // settings an endpoint is minted from — a new record, not the deleted one.
        let case = format!("{product}: created from {} after the delete", second.case);
        let recreated = create_endpoint(&h, &second.request(&provider_arn)).await;
        let recreated_arn = created_arn(&recreated, &case);
        assert_ne!(
            recreated_arn, endpoint_arn,
            "case {case:?}: a create for a freed pair must mint its own endpoint"
        );

        let case = format!("{product}: {} repeated after the delete", second.case);
        let repeat = create_endpoint(&h, &second.request(&provider_arn)).await;
        assert_eq!(
            created_arn(&repeat, &case),
            recreated_arn,
            "case {case:?}: the new endpoint's own settings must resolve it"
        );

        // And the settings the deleted endpoint was built from now name no endpoint of
        // this pair.
        let conflict = create_endpoint(&h, &first.request(&provider_arn)).await;
        assert_conflicts_with(
            &conflict,
            &format!("{product}: {} against the new endpoint", first.case),
            &recreated_arn,
            first.secret_id,
        );
    }
}
