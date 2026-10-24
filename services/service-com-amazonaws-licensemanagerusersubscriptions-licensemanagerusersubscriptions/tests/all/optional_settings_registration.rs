// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what `RegisterIdentityProvider` does with the OPTIONAL
//! `Settings` member, driven over the real wire path: every call goes through the
//! router's `dispatch`, so the request is deserialized, constraint-checked, handled and
//! serialized exactly as a client's would be.
//!
//! The rule under test, asserted over a SET of registrations rather than one request:
//! a request member the model does not mark `@required` is accepted when it is omitted,
//! and the record persists exactly what the caller supplied — so a registration with no
//! `Settings` is stored and reported with no subnet and no security group, and one that
//! supplies them is stored and reported with those.
//! (`smithy_query(query="shape", shape="RegisterIdentityProviderRequest")`: only
//! `IdentityProvider` and `Product` carry `required: true`.)
//!
//! A registration that provisions no activation-server VPC endpoints is what the RDS
//! license-server flow needs, so each `Settings`-less row is also driven on to
//! `CreateLicenseServerEndpoint` for the ARN it minted.

use serde_json::{json, Value};

// ONE wire harness per service: the dispatch plumbing, the signing identity and the
// provider helpers come from the sibling suites rather than a second copy.
use crate::identity_provider_subscriptions::{
    arn_prefix, as_str, harness, list_providers, provider_union, register, PRODUCTS,
};
use crate::license_server_endpoints::{create_endpoint, SERVER_TYPE};

/// One registration to exercise. The rows differ along what the rule generalises over:
/// whether `Settings` is supplied at all, how many subnets it carries when it is, the
/// directory-id form, the product, and whether the optional `Tags` member is supplied
/// too.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// `DirectoryId` matches `^(d|sd)-[0-9a-f]{10}$`; the `d-` ids are the ones the
    /// shared harness seeded into the account's directory fleet.
    directory_id: &'static str,
    product: &'static str,
    /// `None` is the omitted member under test; `Some` carries `(subnets, security
    /// group)` — `Subnets` has `@length(min: 1)` and `SecurityGroupId` matches
    /// `^sg-(([0-9a-z]{8})|([0-9a-z]{17}))$`, so both published lengths appear below.
    settings: Option<(&'static [&'static str], &'static str)>,
    tagged: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "no settings, remote desktop services, aws-managed directory",
        directory_id: "d-0123456789",
        product: PRODUCTS[4],
        settings: None,
        tagged: false,
    },
    Row {
        case: "no settings, office standard, shared directory id",
        directory_id: "sd-abcdef0123",
        product: PRODUCTS[3],
        settings: None,
        tagged: false,
    },
    Row {
        case: "no settings, visual studio enterprise, tagged",
        directory_id: "d-00000000ff",
        product: PRODUCTS[0],
        settings: None,
        tagged: true,
    },
    Row {
        case: "settings with one subnet and an 8-character security group",
        directory_id: "d-123456789a",
        product: PRODUCTS[1],
        settings: Some((&["subnet-0123456789abcdef0"], "sg-0123abcd")),
        tagged: false,
    },
    Row {
        case: "settings with two subnets and a 17-character security group",
        directory_id: "d-0123456789",
        product: PRODUCTS[2],
        settings: Some((
            &["subnet-1111111111111111a", "subnet-2222222222222222b"],
            "sg-0123456789abcdef0",
        )),
        tagged: true,
    },
];

impl Row {
    fn request(&self) -> Value {
        let mut body = json!({
            "IdentityProvider": provider_union(self.directory_id),
            "Product": self.product,
        });
        if let Some((subnets, security_group_id)) = self.settings {
            body["Settings"] = json!({
                "Subnets": subnets,
                "SecurityGroupId": security_group_id,
            });
        }
        if self.tagged {
            body["Tags"] = json!({ "team": "licensing", "case": self.case });
        }
        body
    }

    /// The settings every read of this registration must report: what the request
    /// supplied, and the empty configuration when it supplied none.
    fn expected_settings(&self) -> Value {
        match self.settings {
            Some((subnets, security_group_id)) => json!({
                "Subnets": subnets,
                "SecurityGroupId": security_group_id,
            }),
            None => json!({ "Subnets": [], "SecurityGroupId": "" }),
        }
    }
}

/// RULE: `Settings` is not `@required` on the request, so a registration that omits it is
/// accepted, and every read reports exactly the settings the caller supplied — none when
/// none were supplied.
#[tokio::test]
async fn a_registration_persists_the_settings_it_supplied_and_omitting_them_is_accepted() {
    let h = harness();
    let mut minted = Vec::new();

    for row in ROWS {
        let reply = register(&h, &row.request()).await;
        let body = reply.ok(row.case);
        let summary = body
            .get("IdentityProviderSummary")
            .unwrap_or_else(|| panic!("case {:?}: no IdentityProviderSummary in {body:?}", row.case));

        assert_eq!(
            summary.get("Settings"),
            Some(&row.expected_settings()),
            "case {:?}: expected the summary to carry {}, got {summary:?}",
            row.case,
            row.expected_settings()
        );
        assert_eq!(
            as_str(summary, "Product", row.case),
            row.product,
            "case {:?}: the summary must carry the registered product, got {summary:?}",
            row.case
        );
        let arn = as_str(summary, "IdentityProviderArn", row.case);
        assert!(
            arn.starts_with(&arn_prefix("identity-provider")),
            "case {:?}: expected an ARN under {}, got {arn:?}",
            row.case,
            arn_prefix("identity-provider")
        );
        minted.push((*row, arn.to_string()));
    }

    // The round trip: every registration is read back through the implemented reader,
    // settings and all, so what the register persisted is what a later caller sees.
    let listed = list_providers(&h, &json!({})).await;
    let body = listed.ok("list every registration");
    let summaries = body
        .get("IdentityProviderSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("IdentityProviderSummaries is required, got {body:?}"));
    assert_eq!(
        summaries.len(),
        ROWS.len(),
        "every registration must be reported once, got {summaries:?}"
    );
    for (row, arn) in &minted {
        let summary = summaries
            .iter()
            .find(|summary| summary.get("IdentityProviderArn") == Some(&json!(arn)))
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: {arn} is missing from the list, got {summaries:?}",
                    row.case
                )
            });
        assert_eq!(
            summary.get("Settings"),
            Some(&row.expected_settings()),
            "case {:?}: the list must report the stored settings {}, got {summary:?}",
            row.case,
            row.expected_settings()
        );
    }
}

/// RULE: a registration that supplied no `Settings` is a registration like any other, so
/// the license-server endpoint flow can be driven for the ARN it minted.
#[tokio::test]
async fn a_settings_less_registration_can_be_given_a_license_server_endpoint() {
    let h = harness();
    for row in ROWS.iter().filter(|row| row.settings.is_none()) {
        let registered = register(&h, &row.request()).await;
        let provider_arn = as_str(
            &registered.ok(row.case)["IdentityProviderSummary"],
            "IdentityProviderArn",
            row.case,
        )
        .to_string();

        let reply = create_endpoint(
            &h,
            &json!({
                "IdentityProviderArn": provider_arn,
                "LicenseServerSettings": {
                    "ServerType": SERVER_TYPE,
                    "ServerSettings": {
                        "RdsSalSettings": {
                            "RdsSalCredentialsProvider": {
                                "SecretsManagerCredentialsProvider": {
                                    "SecretId": "rds-sal-admin",
                                },
                            },
                        },
                    },
                },
            }),
        )
        .await;
        let body = reply.ok(row.case);
        assert_eq!(
            as_str(body, "IdentityProviderArn", row.case),
            provider_arn,
            "case {:?}: the create must echo the identity provider it created for, got {body:?}",
            row.case
        );
        let endpoint_arn = as_str(body, "LicenseServerEndpointArn", row.case);
        assert!(
            endpoint_arn.starts_with(&arn_prefix("license-server-endpoint")),
            "case {:?}: expected an ARN under {}, got {endpoint_arn:?}",
            row.case,
            arn_prefix("license-server-endpoint")
        );
    }
}
