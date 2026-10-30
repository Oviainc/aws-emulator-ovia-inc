// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ONE resolution of a registration addressed by its
//! `(IdentityProvider, Product)` pair, driven over the real wire path: every call goes
//! through the router's `dispatch`, so the request is deserialized, constraint-checked,
//! handled and serialized exactly as a client's would be.
//!
//! The RULE under test, asserted over a TABLE of inputs rather than one recorded request:
//! an `(IdentityProvider, Product)` pair addresses exactly ONE registration and only a LIVE
//! one resolves, so every operation that addresses a registration that way agrees on
//! whether the pair resolves — and a pair that resolves to nothing is refused by all of them
//! the one way that addressing is refused: fail-closed, with `UnauthorizedException` and its
//! fixed sentence.
//!
//! `ListProductSubscriptions` (which addresses a registration by the pair when `Product` is
//! supplied) is driven beside `UpdateIdentityProviderSettings` and
//! `StartProductSubscription` (which address one the same way), so a change to what
//! "resolves" means cannot move one of them without the others: the rows below include a
//! registration that is still `REGISTERING`, one that has become `REGISTERED`, one whose
//! deregistration was accepted, a directory registered for a DIFFERENT product, and a
//! directory with no registration at all.

use serde_json::{json, Value};

use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::scaffolded::UNAUTHORIZED_EXCEPTION_CODE;

// ONE wire harness per service: the dispatch plumbing, the signing identity and the
// provider-registration helpers come from the sibling suites rather than a second copy.
use super::identity_provider_subscriptions::{
    as_str, call, deregister, harness, list_subscriptions, provider_union, register,
    start_subscription, Harness, Reply, PAST_THE_OWED_TRANSITION, PRODUCTS,
};

/// The FIXED sentence a fail-closed refusal carries
/// (`scaffolded::UNAUTHORIZED_EXCEPTION_MESSAGE`), and the status it is reported with
/// (`UNAUTHORIZED_EXCEPTION_STATUS`), spelled out here rather than read from the production
/// constants so a change to them fails a test.
const UNAUTHORIZED_MESSAGE: &str =
    "Unauthorized. Please check the request and access policy and try again.";
const UNAUTHORIZED_STATUS: u16 = 401;

async fn update_settings(h: &Harness, body: &Value) -> Reply {
    call(h, "/identity-provider/UpdateIdentityProviderSettings", body).await
}

/// What state the registration behind the addressed pair is in when the three operations
/// address it — what the rule generalises over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Registered {
    /// Registered for this pair, still inside the owed `REGISTERING` span.
    Registering,
    /// Registered for this pair and past the owed transition.
    Fully,
    /// Registered for this pair, then deregistered: REPORTED in `DEREGISTERING`, and its
    /// `(provider key, Product)` entry is still in the index, so only the LIVE filter keeps
    /// it from resolving.
    Deregistering,
    /// The directory has a live registration, but for another product — the pair the calls
    /// address is not in the index.
    AnotherProduct,
    /// The directory has no registration at all.
    NotAtAll,
}

/// One pair to address. The rows differ along what the rule generalises over: the form the
/// directory is named in (`d-` AWS-managed, `sd-` shared, and a self-managed Active
/// Directory named by domain), the product, and the state of the registration behind the
/// pair.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// `DirectoryId` matches `^(d|sd)-[0-9a-f]{10}$`; `None` is a SELF-MANAGED Active
    /// Directory, which carries `ActiveDirectorySettings.DomainName` and no `DirectoryId`.
    directory_id: Option<&'static str>,
    domain_name: Option<&'static str>,
    product: &'static str,
    registered: Registered,
}

/// The `d-` ids below are the directories the shared harness creates in the account's
/// directory fleet (`d-0123456789`, `d-00000000ff`, `d-123456789a`); an `sd-` id and a
/// self-managed directory are not resolved against that fleet at all.
const ROWS: &[Row] = &[
    Row {
        case: "aws-managed directory, still registering",
        directory_id: Some("d-0123456789"),
        domain_name: None,
        product: PRODUCTS[0],
        registered: Registered::Registering,
    },
    Row {
        case: "shared directory id, fully registered",
        directory_id: Some("sd-abcdef0123"),
        domain_name: None,
        product: PRODUCTS[1],
        registered: Registered::Fully,
    },
    Row {
        case: "self-managed active directory, fully registered",
        directory_id: None,
        domain_name: Some("corp.example.com"),
        product: PRODUCTS[2],
        registered: Registered::Fully,
    },
    Row {
        case: "aws-managed directory, deregistration accepted",
        directory_id: Some("d-00000000ff"),
        domain_name: None,
        product: PRODUCTS[3],
        registered: Registered::Deregistering,
    },
    Row {
        case: "self-managed active directory, deregistration accepted",
        directory_id: None,
        domain_name: Some("ad.example.internal"),
        product: PRODUCTS[4],
        registered: Registered::Deregistering,
    },
    Row {
        case: "directory registered for another product",
        directory_id: Some("d-123456789a"),
        domain_name: None,
        product: PRODUCTS[4],
        registered: Registered::AnotherProduct,
    },
    Row {
        case: "directory with no registration at all",
        directory_id: Some("d-0123456789"),
        domain_name: None,
        product: PRODUCTS[0],
        registered: Registered::NotAtAll,
    },
];

impl Row {
    /// The `IdentityProvider` union this row addresses: either an AWS-managed/shared
    /// directory named by `DirectoryId`, or a self-managed one named by its domain.
    fn provider(&self) -> Value {
        match (self.directory_id, self.domain_name) {
            (Some(directory_id), _) => provider_union(directory_id),
            (None, Some(domain_name)) => json!({
                "ActiveDirectoryIdentityProvider": {
                    "ActiveDirectorySettings": { "DomainName": domain_name },
                    "ActiveDirectoryType": "SELF_MANAGED",
                },
            }),
            (None, None) => unreachable!("a row names a directory one way or the other"),
        }
    }

    fn registration(&self, product: &str) -> Value {
        json!({
            "IdentityProvider": self.provider(),
            "Product": product,
            "Settings": {
                "Subnets": ["subnet-0123456789abcdef0"],
                "SecurityGroupId": "sg-0123abcd",
            },
        })
    }

    /// Whether the pair this row addresses resolves to a live registration.
    fn resolves(&self) -> bool {
        matches!(self.registered, Registered::Registering | Registered::Fully)
    }

    /// The product another registration of the same directory is made for, when the row
    /// asks for one: any published product other than the one the calls address.
    fn other_product(&self) -> &'static str {
        PRODUCTS
            .iter()
            .copied()
            .find(|product| *product != self.product)
            .expect("the published product set holds more than one product")
    }
}

/// Bring the store into the state `row` describes.
async fn arrange(h: &Harness, row: &Row) {
    match row.registered {
        Registered::NotAtAll => {}
        Registered::AnotherProduct => {
            register(h, &row.registration(row.other_product()))
                .await
                .ok(row.case);
        }
        Registered::Registering => {
            register(h, &row.registration(row.product)).await.ok(row.case);
        }
        Registered::Fully => {
            register(h, &row.registration(row.product)).await.ok(row.case);
            tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
        }
        Registered::Deregistering => {
            let reply = register(h, &row.registration(row.product)).await;
            let arn = as_str(
                &reply.ok(row.case)["IdentityProviderSummary"],
                "IdentityProviderArn",
                row.case,
            )
            .to_string();
            // The teardown is owed, not performed, so the registration is still REPORTED
            // in `DEREGISTERING` for the calls below — which is exactly the state the two
            // copies of this resolution could have disagreed about.
            deregister(h, &json!({ "IdentityProviderArn": arn }))
                .await
                .ok(row.case);
        }
    }
}

/// Assert `reply` is the fail-closed refusal every operation answers a pair that resolves to
/// no registration with.
fn refused_closed(case: &str, operation: &str, reply: &Reply) {
    assert_eq!(
        reply.status, UNAUTHORIZED_STATUS,
        "case {case:?}: {operation} refuses at HTTP {UNAUTHORIZED_STATUS}, got {reply:?}"
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some(UNAUTHORIZED_EXCEPTION_CODE),
        "case {case:?}: {operation} must fail closed, got {reply:?}"
    );
    assert_eq!(
        reply.message(),
        Some(UNAUTHORIZED_MESSAGE),
        "case {case:?}: {operation} must carry the fixed fail-closed sentence, got {reply:?}"
    );
}

/// The `ProductUserSummary` list `ListProductSubscriptions` reported, as usernames.
fn listed_usernames(case: &str, reply: &Reply) -> Vec<String> {
    let body = reply.ok(case);
    body["ProductUserSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no ProductUserSummaries in {body:?}"))
        .iter()
        .map(|summary| as_str(summary, "Username", case).to_string())
        .collect()
}

/// RULE: an `(IdentityProvider, Product)` pair addresses exactly one registration and only
/// a LIVE one resolves, so `ListProductSubscriptions` with `Product` supplied,
/// `UpdateIdentityProviderSettings` and `StartProductSubscription` agree on whether the
/// pair resolves — and all three refuse an unresolved pair the one way that addressing is
/// refused.
#[tokio::test]
async fn the_pair_resolves_the_same_registration_for_every_operation() {
    for row in ROWS {
        let h = harness();
        arrange(&h, row).await;

        let listed = list_subscriptions(
            &h,
            &json!({ "IdentityProvider": row.provider(), "Product": row.product }),
        )
        .await;
        let updated = update_settings(
            &h,
            &json!({
                "IdentityProvider": row.provider(),
                "Product": row.product,
                "UpdateSettings": {
                    "AddSubnets": ["subnet-1111111111111111a"],
                    "RemoveSubnets": [],
                },
            }),
        )
        .await;
        let started = start_subscription(
            &h,
            &json!({
                "IdentityProvider": row.provider(),
                "Product": row.product,
                "Username": "pat",
            }),
        )
        .await;

        if row.resolves() {
            assert_eq!(
                listed_usernames(row.case, &listed),
                Vec::<String>::new(),
                "case {:?}: the resolved registration has no subscriptions yet",
                row.case
            );
            updated.ok(row.case);
            started.ok(row.case);
            // The list and the subscribe resolved the SAME registration: what the one
            // persisted is what the other reports back.
            let after = list_subscriptions(
                &h,
                &json!({ "IdentityProvider": row.provider(), "Product": row.product }),
            )
            .await;
            assert_eq!(
                listed_usernames(row.case, &after),
                vec!["pat".to_string()],
                "case {:?}: the subscription must be reported through the pair that \
                 resolved it",
                row.case
            );
        } else {
            refused_closed(row.case, "ListProductSubscriptions", &listed);
            refused_closed(row.case, "UpdateIdentityProviderSettings", &updated);
            refused_closed(row.case, "StartProductSubscription", &started);
        }
    }
}
