// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a REFUSED `DeregisterIdentityProvider` leaves behind, driven
//! over the real wire path: every call goes through the router's `dispatch`, so the request
//! is deserialized, constraint-checked, handled and serialized exactly as a client's would
//! be.
//!
//! The RULE under test, asserted over a TABLE of inputs rather than one recorded request:
//! the members that identify a registration (`IdentityProvider` and `Product`) are what SAY
//! which registration a request means, so one that supplies `IdentityProviderArn` and a
//! member whose value is not the stored registration's addresses no registration of the
//! account — it is refused, and a refused teardown performs NOTHING, including every
//! cascading teardown the accepted call would have run on the resources hanging off that
//! registration (its product subscriptions, its license server endpoints, its tags and the
//! user associations of its directory).
//!
//! The sibling `arn_and_members_address_one_resource` suite pins the refusal itself, over
//! each plane of this service and every identity member of it. What this suite adds is the
//! BLAST RADIUS: an accepted deregistration cascades, so a registration torn down for a
//! request that never named it destroys state the caller cannot get back. Each row holds a
//! registration with a subscription, a license server endpoint, tags and a user association
//! hanging off it, and asserts
//!
//! * the contradicting request is refused and every one of those dependants is untouched,
//!   as is the registration the contradicting member DOES name;
//! * the same request with both members AGREEING is served and the dependants DO go — so
//!   the refusals above are about the contradiction, and the cascade they prevent is real.

use serde_json::{json, Value};

use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::state::IDENTITY_PROVIDER_NOT_VALID;

// ONE wire harness per service: the dispatch plumbing, the signing identity, the
// registration helpers and the per-plane list readers come from the sibling suites rather
// than a second copy.
use super::arn_and_members_address_one_resource::{assert_untouched, reported};
use super::identity_provider_subscriptions::{
    as_str, call, deregister, harness, list_providers, list_subscriptions, provider_union,
    register, start_subscription, stop_subscription, Harness, Reply, PRODUCTS,
};
use super::license_server_endpoints::{create_endpoint, list_endpoints, ENDPOINTS};
use super::resource_resolution_refusals::{
    assert_invalid_resource_arn, associate, held_instance, list_tags, registration,
};

/// The subnet every registration in this suite is made with — one of the account's.
const SUBNET: &str = "subnet-0123456789abcdef0";

/// The user whose subscription and association hang off the registration under test.
const USER: &str = "cascade.user";

/// The tag every registration under test carries, so a refusal that removed it is visible.
fn provider_tags() -> Value {
    json!({ "team": "licensing" })
}

/// One registration to attempt a contradicting teardown of, the second registration the
/// contradicting member may name, and the members that request supplies.
///
/// The rows differ along what the rule generalises over: WHICH identity member contradicts
/// (`Product`, `IdentityProvider`, or both at once), whether the value it names belongs to
/// another LIVE registration or to no registration at all, the directory-id form the
/// registration was made with, and the product.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// `DirectoryId` matches `^(d|sd)-[0-9a-f]{10}$`; the `d-` ids are the ones the shared
    /// harness puts in the account's directory fleet, a `sd-` id is a directory shared into
    /// the account and is resolved against no fleet.
    directory_id: &'static str,
    /// From `RegisterIdentityProvider`'s `Product` documentation, through the shared
    /// `PRODUCTS` catalog: VISUAL_STUDIO_ENTERPRISE | VISUAL_STUDIO_PROFESSIONAL |
    /// OFFICE_PROFESSIONAL_PLUS | OFFICE_STANDARD | REMOTE_DESKTOP_SERVICES.
    product: &'static str,
    /// A SECOND registration to create, as `(directory, product)`, when the row means the
    /// contradicting member to name one that is live.
    other: Option<(&'static str, &'static str)>,
    /// The `IdentityProvider` the contradicting request names, when it names one.
    named_directory: Option<&'static str>,
    /// The `Product` the contradicting request names, when it names one.
    named_product: Option<&'static str>,
}

const ROWS: &[Row] = &[
    Row {
        case: "the Product of another live registration of the same directory",
        directory_id: "d-0123456789",
        product: PRODUCTS[0],
        other: Some(("d-0123456789", PRODUCTS[3])),
        named_directory: None,
        named_product: Some(PRODUCTS[3]),
    },
    Row {
        case: "a Product the account has no registration for",
        directory_id: "d-00000000ff",
        product: PRODUCTS[1],
        other: None,
        named_directory: None,
        named_product: Some(PRODUCTS[2]),
    },
    Row {
        case: "the IdentityProvider of another live registration of the same product",
        directory_id: "d-123456789a",
        product: PRODUCTS[4],
        other: Some(("sd-abcdef0123", PRODUCTS[4])),
        named_directory: Some("sd-abcdef0123"),
        named_product: None,
    },
    Row {
        case: "an IdentityProvider the account has no registration for",
        directory_id: "sd-abcdef0123",
        product: PRODUCTS[2],
        other: None,
        named_directory: Some("d-123456789a"),
        named_product: None,
    },
    Row {
        case: "both members, naming another live registration outright",
        directory_id: "d-0123456789",
        product: PRODUCTS[3],
        other: Some(("d-00000000ff", PRODUCTS[0])),
        named_directory: Some("d-00000000ff"),
        named_product: Some(PRODUCTS[0]),
    },
];

impl Row {
    /// The `ListProductSubscriptions` body that reports this row's subscription.
    fn list_subscriptions(&self) -> Value {
        json!({
            "IdentityProvider": provider_union(self.directory_id),
            "Product": self.product,
        })
    }

    /// The `ListUserAssociations` body that reports this row's association.
    fn list_associations(&self) -> Value {
        json!({
            "IdentityProvider": provider_union(self.directory_id),
            "InstanceId": held_instance(0),
        })
    }

    /// The deregistration that addresses this row's registration by ARN and ALSO names the
    /// members the row contradicts with. A member the row leaves out names nothing.
    fn contradicting(&self, arn: &str) -> Value {
        let mut body = json!({ "IdentityProviderArn": arn });
        if let Some(directory_id) = self.named_directory {
            body["IdentityProvider"] = provider_union(directory_id);
        }
        if let Some(product) = self.named_product {
            body["Product"] = json!(product);
        }
        body
    }

    /// The deregistration that addresses the same registration by ARN and names both of its
    /// identity members with the stored registration's own values.
    fn agreeing(&self, arn: &str) -> Value {
        json!({
            "IdentityProviderArn": arn,
            "IdentityProvider": provider_union(self.directory_id),
            "Product": self.product,
        })
    }
}

/// One row's registration and everything hanging off it.
struct Fixture {
    h: Harness,
    /// The registration the contradicting request addresses by ARN.
    provider: String,
    /// The registration the contradicting member names, when the row created one.
    other_provider: Option<String>,
    subscription: String,
    endpoint: String,
    association: String,
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

/// A fresh emulator holding `row`'s registration, a product subscription and a user
/// association through its directory, a license server endpoint created for it, and the
/// second registration the row's contradicting member names when it names a live one.
async fn setup(row: &Row) -> Fixture {
    let h = harness();
    let provider = register_provider(
        &h,
        &registration(
            row.directory_id,
            row.product,
            &[SUBNET],
            Some(provider_tags()),
        ),
        row.case,
    )
    .await;

    let other_provider = match row.other {
        Some((directory_id, product)) => Some(
            register_provider(
                &h,
                &registration(directory_id, product, &[SUBNET], Some(provider_tags())),
                row.case,
            )
            .await,
        ),
        None => None,
    };

    let subscribed = start_subscription(
        &h,
        &json!({
            "Username": USER,
            "IdentityProvider": provider_union(row.directory_id),
            "Product": row.product,
        }),
    )
    .await;
    let subscription = as_str(
        &subscribed.ok(row.case)["ProductUserSummary"],
        "ProductUserArn",
        row.case,
    )
    .to_string();

    // The create body is the shared endpoint fixture's, addressed at this row's
    // registration: what it carries beyond the provider ARN is irrelevant here.
    let created = create_endpoint(&h, &ENDPOINTS[0].request(&provider)).await;
    let endpoint = as_str(created.ok(row.case), "LicenseServerEndpointArn", row.case).to_string();

    let associated = associate(
        &h,
        &json!({
            "Username": USER,
            "InstanceId": held_instance(0),
            "IdentityProvider": provider_union(row.directory_id),
        }),
    )
    .await;
    let association = as_str(
        &associated.ok(row.case)["InstanceUserSummary"],
        "InstanceUserArn",
        row.case,
    )
    .to_string();

    Fixture {
        h,
        provider,
        other_provider,
        subscription,
        endpoint,
        association,
    }
}

/// Assert `reply` is the modelled `ValidationException` (HTTP 400) carrying the sentence
/// this service uses for an identity-provider reference that resolves to no registration.
fn assert_not_valid(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: ValidationException is modelled at HTTP 400, got {reply:?}"
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("ValidationException"),
        "case {case:?}: expected ValidationException, got {reply:?}"
    );
    assert_eq!(
        reply.message(),
        Some(IDENTITY_PROVIDER_NOT_VALID),
        "case {case:?}: expected the service's sentence for an unresolvable identity \
         provider, got {reply:?}"
    );
}

/// The `LicenseServerEndpointProvisioningStatus` the list reports for `arn`.
async fn endpoint_status(h: &Harness, case: &str, arn: &str) -> String {
    let listed = list_endpoints(h, &json!({})).await;
    let summary = reported(&listed, "LicenseServerEndpoints", "LicenseServerEndpointArn", arn, case)
        .unwrap_or_else(|| panic!("case {case:?}: the list must still report the endpoint {arn}"));
    as_str(&summary, "LicenseServerEndpointProvisioningStatus", case).to_string()
}

/// RULE: a `DeregisterIdentityProvider` that names an identity member no stored value of the
/// registration its ARN names matches addresses no registration of the account, so it is
/// refused and tears NOTHING down — neither the registration the ARN names nor anything
/// hanging off it, and not the registration the contradicting member names either.
#[tokio::test]
async fn a_contradicting_deregistration_tears_nothing_down() {
    for row in ROWS {
        let fixture = setup(row).await;
        let h = &fixture.h;
        let case = row.case;

        assert_not_valid(
            case,
            &deregister(h, &row.contradicting(&fixture.provider)).await,
        );

        // The registration the ARN named is still reported, and not on its way out.
        let listed = list_providers(h, &json!({})).await;
        let summary = reported(
            &listed,
            "IdentityProviderSummaries",
            "IdentityProviderArn",
            &fixture.provider,
            case,
        );
        assert_untouched(case, "addressed", &fixture.provider, "DEREGISTERING", summary);

        // And so is the registration the contradicting member named, which the request
        // asked nothing of beyond naming one of its members.
        if let Some(other) = fixture.other_provider.as_deref() {
            let summary = reported(
                &listed,
                "IdentityProviderSummaries",
                "IdentityProviderArn",
                other,
                case,
            );
            assert_untouched(case, "contradicting", other, "DEREGISTERING", summary);
        }

        // The subscription an accepted deregistration would have dropped outright.
        let listed = list_subscriptions(h, &row.list_subscriptions()).await;
        let summary = reported(
            &listed,
            "ProductUserSummaries",
            "ProductUserArn",
            &fixture.subscription,
            case,
        );
        assert!(
            summary
                .as_ref()
                .is_some_and(|summary| summary.get("SubscriptionEndDate").is_none()),
            "case {case:?}: the subscription must carry no end date, got {summary:?}"
        );
        assert_untouched(
            case,
            "subscribed",
            &fixture.subscription,
            "UNSUBSCRIBED",
            summary,
        );

        // The license server endpoint whose deletion it would have begun.
        let status = endpoint_status(h, case, &fixture.endpoint).await;
        assert!(
            !matches!(status.as_str(), "DELETING" | "DELETED"),
            "case {case:?}: a refused deregistration must not begin deleting the endpoint \
             {}, got {status:?}",
            fixture.endpoint
        );

        // The user association it would have dropped as the directory's last registration
        // went.
        let listed = call(h, "/user/ListUserAssociations", &row.list_associations()).await;
        let summary = reported(
            &listed,
            "InstanceUserSummaries",
            "InstanceUserArn",
            &fixture.association,
            case,
        );
        assert_untouched(
            case,
            "associated",
            &fixture.association,
            "DISASSOCIATING",
            summary,
        );

        // And the tags it would have removed: a registration still carrying its tags is one
        // the tagging surface still resolves.
        let tags = list_tags(h, &fixture.provider).await;
        assert_eq!(
            tags.ok(case)["Tags"],
            provider_tags(),
            "case {case:?}: a refused deregistration must leave the registration's tags, got \
             {:?}",
            tags.body
        );
    }
}

/// RULE: the same request with both identity members AGREEING with the stored registration
/// addresses exactly that registration, so it is served and the cascade runs — which is
/// what makes the refusals above about the contradiction rather than about supplying the
/// members at all, and what the contradicting request would have destroyed.
#[tokio::test]
async fn the_agreeing_deregistration_is_the_one_that_cascades() {
    for row in ROWS {
        let fixture = setup(row).await;
        let h = &fixture.h;
        let case = row.case;

        let accepted = deregister(h, &row.agreeing(&fixture.provider)).await;
        let summary = accepted.ok(case)["IdentityProviderSummary"].clone();
        assert_eq!(
            as_str(&summary, "IdentityProviderArn", case),
            fixture.provider,
            "case {case:?}: the agreeing request must deregister the registration it names, \
             got {summary:?}"
        );
        assert_eq!(
            as_str(&summary, "Status", case),
            "DEREGISTERING",
            "case {case:?}: expected the non-terminal teardown status, got {summary:?}"
        );

        // The endpoint created for it is on its way out with it.
        let status = endpoint_status(h, case, &fixture.endpoint).await;
        assert_eq!(
            status, "DELETING",
            "case {case:?}: the endpoints of a deregistered registration are deleted the same \
             asynchronous way, got {status:?}"
        );

        // Its product users went with it: the subscription the fixture started resolves for
        // nothing, which is the fail-closed refusal a `ProductUserArn` that never existed
        // gets.
        let refused = stop_subscription(h, &json!({ "ProductUserArn": fixture.subscription })).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (401, Some("UnauthorizedException")),
            "case {case:?}: the dropped subscription must resolve for nothing, got {refused:?}"
        );

        // And its tags went too, so the tagging surface no longer resolves the registration.
        assert_invalid_resource_arn(case, &list_tags(h, &fixture.provider).await);

        // The other registration, which this request named nothing of, is untouched.
        if let Some(other) = fixture.other_provider.as_deref() {
            let listed = list_providers(h, &json!({})).await;
            let summary = reported(
                &listed,
                "IdentityProviderSummaries",
                "IdentityProviderArn",
                other,
                case,
            );
            assert_untouched(case, "contradicting", other, "DEREGISTERING", summary);
        }
    }
}
