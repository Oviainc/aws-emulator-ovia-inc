// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DEREGISTRATION lifecycle of an identity-provider
//! registration, driven over the real wire path: every call goes through the router's
//! `dispatch`, so the request is deserialized, constraint-checked, handled and serialized
//! exactly as a client's would be.
//!
//! The RULE under test, asserted over a TABLE of inputs rather than one recorded request:
//! a call that answers a resource in a NON-TERMINAL status has accepted asynchronous work,
//! so the resource stays REPORTED in that status until the work the clock owes completes.
//! `DeregisterIdentityProvider` answers `IdentityProviderSummary$Status`
//! (`smithy_query(query="shape", shape="IdentityProviderSummary")`: a free-form `String`
//! documented as "The status of the identity provider") as `DEREGISTERING`, so a client
//! polling `ListIdentityProviders` keeps seeing that registration in `DEREGISTERING` until
//! the teardown completes and only then stops seeing it — the same rule the sibling
//! `license_server_endpoint_deletion` suite pins for `DELETING`/`DELETED`.
//!
//! What each suite below pins:
//!
//! * the deregister answer and every later read report the same stored status until the
//!   teardown the clock owes completes, after which the registration is gone;
//! * neither the status the registration was in when the deregistration was accepted
//!   (`REGISTERING` or `REGISTERED`), nor which of the two ways the model lets a caller
//!   address it was used, changes that lifecycle;
//! * a registration still being torn down keeps its `(IdentityProvider, Product)` pair, so
//!   a `RegisterIdentityProvider` for that pair CONFLICTS while it is deregistering and
//!   succeeds, with a new ARN, once it has completed;
//! * a registration being torn down is reported but is no longer LIVE: every operation
//!   that acts on a registration refuses it, and it is not taggable.

use serde_json::{json, Value};

use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::scaffolded::{
    BAD_REQUEST_EXCEPTION_CODE, INVALID_INPUT_RESOURCE_ARN_MESSAGE, UNAUTHORIZED_EXCEPTION_CODE,
};

// ONE wire harness per service: the dispatch plumbing, the signing identity and the
// provider-registration helpers come from the sibling suites rather than a second copy.
use super::identity_provider_subscriptions::{
    as_str, call, encoded_label, harness, list_providers, provider_union, register, send, Harness,
    Reply, PAST_THE_OWED_TRANSITION, PRODUCTS,
};

/// The sentence this service uses for an identity-provider reference that resolves to no
/// registration of the account (`state::IDENTITY_PROVIDER_NOT_VALID`), spelled out here
/// rather than read from the production constant so a change to it fails a test.
const IDENTITY_PROVIDER_NOT_VALID: &str = "Identity provider is not valid";

async fn deregister(h: &Harness, body: &Value) -> Reply {
    call(h, "/identity-provider/DeregisterIdentityProvider", body).await
}

async fn update_settings(h: &Harness, body: &Value) -> Reply {
    call(h, "/identity-provider/UpdateIdentityProviderSettings", body).await
}

async fn list_subscriptions(h: &Harness, body: &Value) -> Reply {
    call(h, "/user/ListProductSubscriptions", body).await
}

async fn start_subscription(h: &Harness, body: &Value) -> Reply {
    call(h, "/user/StartProductSubscription", body).await
}

async fn create_endpoint(h: &Harness, body: &Value) -> Reply {
    call(h, "/license-server/CreateLicenseServerEndpoint", body).await
}

/// `ListTagsForResource` binds `ResourceArn` to ONE path segment (`GET
/// /tags/{ResourceArn}`), so the `:` and `/` inside an ARN are percent-encoded with the
/// shared fixture's encoder.
async fn list_tags(h: &Harness, arn: &str) -> Reply {
    send(h, "GET", &format!("/tags/{}", encoded_label(arn)), None).await
}

async fn tag(h: &Harness, arn: &str, tags: &Value) -> Reply {
    send(
        h,
        "PUT",
        &format!("/tags/{}", encoded_label(arn)),
        Some(&json!({ "Tags": tags })),
    )
    .await
}

/// How a registration under test is addressed by the operations that let a caller name one
/// either way (`DeregisterIdentityProvider`: `IdentityProviderArn`, or
/// `(IdentityProvider, Product)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AddressedBy {
    Arn,
    ProviderAndProduct,
}

/// One registration to take through a deregistration. The rows differ along what the rule
/// generalises over: the directory form the provider names (`d-` AWS-managed, `sd-` shared,
/// and a self-managed Active Directory named by domain), the product, how the
/// deregistration addresses the registration, and whether it was still `REGISTERING` or
/// already `REGISTERED` when the deregistration was accepted.
#[derive(Debug, Clone, Copy)]
struct Deregistration {
    case: &'static str,
    /// `DirectoryId` matches `^(d|sd)-[0-9a-f]{10}$`; `None` is a SELF-MANAGED Active
    /// Directory, which carries `ActiveDirectorySettings.DomainName` and no `DirectoryId`.
    directory_id: Option<&'static str>,
    domain_name: Option<&'static str>,
    product: &'static str,
    addressed_by: AddressedBy,
    /// Whether the registration finished registering before the deregistration.
    registered_first: bool,
}

/// The directories the rows below name are the ones the shared harness creates in the
/// account's directory fleet (`d-0123456789`, `d-00000000ff`, `d-123456789a`); an `sd-` id
/// and a self-managed directory are not resolved against that fleet at all.
const DEREGISTRATIONS: &[Deregistration] = &[
    Deregistration {
        case: "aws-managed directory, addressed by ARN, still registering",
        directory_id: Some("d-0123456789"),
        domain_name: None,
        product: PRODUCTS[0],
        addressed_by: AddressedBy::Arn,
        registered_first: false,
    },
    Deregistration {
        case: "aws-managed directory, addressed by (provider, product), already registered",
        directory_id: Some("d-00000000ff"),
        domain_name: None,
        product: PRODUCTS[3],
        addressed_by: AddressedBy::ProviderAndProduct,
        registered_first: true,
    },
    Deregistration {
        case: "shared directory id, addressed by (provider, product), still registering",
        directory_id: Some("sd-abcdef0123"),
        domain_name: None,
        product: PRODUCTS[1],
        addressed_by: AddressedBy::ProviderAndProduct,
        registered_first: false,
    },
    Deregistration {
        case: "self-managed directory named by domain, addressed by ARN, already registered",
        directory_id: None,
        domain_name: Some("corp.example.com"),
        product: PRODUCTS[4],
        addressed_by: AddressedBy::Arn,
        registered_first: true,
    },
];

impl Deregistration {
    /// The `IdentityProvider` union this row registers: either an AWS-managed/shared
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

    fn registration(&self) -> Value {
        json!({
            "IdentityProvider": self.provider(),
            "Product": self.product,
            "Settings": {
                "Subnets": ["subnet-0123456789abcdef0"],
                "SecurityGroupId": "sg-0123abcd",
            },
            "Tags": { "team": "licensing" },
        })
    }

    /// The request that deregisters this row, in whichever of the two addressings it uses.
    fn deregistration(&self, arn: &str) -> Value {
        match self.addressed_by {
            AddressedBy::Arn => json!({ "IdentityProviderArn": arn }),
            AddressedBy::ProviderAndProduct => json!({
                "IdentityProvider": self.provider(),
                "Product": self.product,
            }),
        }
    }
}

/// Register one row and return the ARN the registration was minted under, waiting for the
/// `REGISTERING` -> `REGISTERED` transition first when the row asks for it.
async fn register_row(h: &Harness, row: &Deregistration) -> String {
    let reply = register(h, &row.registration()).await;
    let arn = as_str(
        &reply.ok(row.case)["IdentityProviderSummary"],
        "IdentityProviderArn",
        row.case,
    )
    .to_string();
    if row.registered_first {
        tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
        assert_eq!(
            listed_status(h, row.case, &arn).await.as_deref(),
            Some("REGISTERED"),
            "case {:?}: the precondition is a registration that finished registering",
            row.case
        );
    }
    arn
}

/// The `Status` `ListIdentityProviders` reports for `arn`, or `None` when the list does not
/// report it at all.
async fn listed_status(h: &Harness, case: &str, arn: &str) -> Option<String> {
    let reply = list_providers(h, &json!({})).await;
    let body = reply.ok(case);
    let summary = body["IdentityProviderSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no IdentityProviderSummaries in {body:?}"))
        .iter()
        .find(|summary| summary.get("IdentityProviderArn").and_then(Value::as_str) == Some(arn))?
        .clone();
    Some(as_str(&summary, "Status", case).to_string())
}

/// Assert `reply` is the modelled `ValidationException` (HTTP 400) carrying the sentence
/// this service uses for an identity provider that resolves to no registration.
fn refused_as_not_valid(case: &str, reply: &Reply) {
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

/// RULE: a deregistration is accepted, not performed — the registration is REPORTED in the
/// non-terminal `DEREGISTERING` status it answered, stays reported in it across reads, and
/// stops being reported only once the teardown the clock owes has completed. Neither the
/// status it was in nor the addressing used changes that.
#[tokio::test]
async fn a_deregistration_is_observable_in_deregistering_and_then_gone() {
    for row in DEREGISTRATIONS {
        let h = harness();
        let arn = register_row(&h, row).await;

        let reply = deregister(&h, &row.deregistration(&arn)).await;
        let summary = reply.ok(row.case)["IdentityProviderSummary"].clone();
        assert_eq!(
            as_str(&summary, "Status", row.case),
            "DEREGISTERING",
            "case {:?}: the answer carries the status the deregistration put the \
             registration in, got {summary:?}",
            row.case
        );
        assert_eq!(
            as_str(&summary, "IdentityProviderArn", row.case),
            arn.as_str(),
            "case {:?}: the answer must be the stored registration, got {summary:?}",
            row.case
        );

        // A client reading straight after the call still sees the transient status, and a
        // second read sees the same thing: a read does not move the lifecycle on.
        for read in ["the first read", "a second read"] {
            let case = format!("{} ({read})", row.case);
            assert_eq!(
                listed_status(&h, &case, &arn).await.as_deref(),
                Some("DEREGISTERING"),
                "case {case:?}: the list must still report {arn} in DEREGISTERING"
            );
        }

        tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;

        let case = format!("{} (past the owed teardown)", row.case);
        assert_eq!(
            listed_status(&h, &case, &arn).await,
            None,
            "case {case:?}: the registration must stop being reported once the teardown \
             the clock owed completed"
        );
        // The identifier now names no registration of this account, whichever way it is
        // addressed — the same refusal a never-registered identifier gets.
        refused_as_not_valid(&case, &deregister(&h, &row.deregistration(&arn)).await);
    }
}

/// RULE: a registration being torn down still holds its `(IdentityProvider, Product)` pair
/// — the pair is free again only once the teardown completes, so a register for it
/// conflicts while the teardown runs and then succeeds with a NEW ARN.
#[tokio::test]
async fn a_register_for_a_pair_still_deregistering_conflicts() {
    for row in DEREGISTRATIONS {
        let h = harness();
        let arn = register_row(&h, row).await;
        deregister(&h, &row.deregistration(&arn)).await.ok(row.case);

        let conflict = register(&h, &row.registration()).await;
        assert_eq!(
            conflict.status, 500,
            "case {:?}: ConflictException is modelled at HTTP 500 \
             (smithy_query: RegisterIdentityProvider errors), got {conflict:?}",
            row.case
        );
        assert_eq!(
            conflict.error_type.as_deref(),
            Some("ConflictException"),
            "case {:?}: a register for a pair still being torn down conflicts, got \
             {conflict:?}",
            row.case
        );
        // The refused register added nothing: the pair is still held by the one
        // registration being torn down.
        assert_eq!(
            listed_status(&h, row.case, &arn).await.as_deref(),
            Some("DEREGISTERING"),
            "case {:?}: a refused register must not disturb the deregistering \
             registration",
            row.case
        );

        tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;

        let case = format!("{} (past the owed teardown)", row.case);
        let fresh = register(&h, &row.registration()).await;
        let summary = fresh.ok(&case)["IdentityProviderSummary"].clone();
        let fresh_arn = as_str(&summary, "IdentityProviderArn", &case).to_string();
        assert_ne!(
            fresh_arn, arn,
            "case {case:?}: a register once the teardown completed mints a new \
             registration, got {summary:?}"
        );
        assert_eq!(
            as_str(&summary, "Status", &case),
            "REGISTERING",
            "case {case:?}: the new registration starts its own lifecycle, got {summary:?}"
        );
        assert_eq!(
            listed_status(&h, &case, &arn).await,
            None,
            "case {case:?}: the torn-down registration must not be resurrected"
        );
        assert_eq!(
            listed_status(&h, &case, &fresh_arn).await.as_deref(),
            Some("REGISTERING"),
            "case {case:?}: the new registration is the one reported"
        );
    }
}

/// RULE: a resource on its way out is REPORTED so a client can watch it go, not so work
/// can be built on it — every operation that acts on a registration refuses one that is
/// being torn down, exactly as it refuses one that is gone, and it is no longer taggable.
#[tokio::test]
async fn a_registration_being_deregistered_is_not_a_live_registration() {
    for row in DEREGISTRATIONS {
        let h = harness();
        let arn = register_row(&h, row).await;
        deregister(&h, &row.deregistration(&arn)).await.ok(row.case);

        // Every operation that names a registration, each addressing it the way its own
        // model lets it: all are told the identifier is not a valid identity provider.
        let refusals = [
            (
                "UpdateIdentityProviderSettings",
                update_settings(
                    &h,
                    &json!({
                        "IdentityProviderArn": arn,
                        "UpdateSettings": {
                            "AddSubnets": ["subnet-1111111111111111a"],
                            "RemoveSubnets": [],
                        },
                    }),
                )
                .await,
            ),
            (
                "CreateLicenseServerEndpoint",
                create_endpoint(
                    &h,
                    &json!({
                        "IdentityProviderArn": arn,
                        "LicenseServerSettings": {
                            "ServerType": "RDS_SAL",
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
                .await,
            ),
            (
                "ListProductSubscriptions",
                list_subscriptions(
                    &h,
                    &json!({
                        "IdentityProvider": row.provider(),
                        "Product": row.product,
                    }),
                )
                .await,
            ),
        ];
        for (surface, reply) in refusals {
            refused_as_not_valid(&format!("{}/{surface}", row.case), &reply);
        }

        // The user plane fails CLOSED instead of saying so, which is the refusal it gives
        // for a provider that is not registered at all.
        let user_plane = start_subscription(
            &h,
            &json!({
                "Username": "deregistering.user",
                "IdentityProvider": row.provider(),
                "Product": row.product,
            }),
        )
        .await;
        assert_eq!(
            user_plane.error_type.as_deref(),
            Some(UNAUTHORIZED_EXCEPTION_CODE),
            "case {:?}: the user plane must fail closed for a registration being torn \
             down, got {user_plane:?}",
            row.case
        );

        // The tagging surface resolves it exactly as it resolves a resource that is gone.
        for (surface, reply) in [
            ("ListTagsForResource", list_tags(&h, &arn).await),
            ("TagResource", tag(&h, &arn, &json!({ "team": "other" })).await),
        ] {
            let case = format!("{}/{surface}", row.case);
            assert_eq!(
                reply.status, 400,
                "case {case:?}: expected HTTP 400, got {reply:?}"
            );
            assert_eq!(
                reply.error_type.as_deref(),
                Some(BAD_REQUEST_EXCEPTION_CODE),
                "case {case:?}: expected {BAD_REQUEST_EXCEPTION_CODE}, got {reply:?}"
            );
            assert_eq!(
                reply.message(),
                Some(INVALID_INPUT_RESOURCE_ARN_MESSAGE),
                "case {case:?}: expected the tagging surface's fixed sentence, got {reply:?}"
            );
        }

        // None of those refusals is because the registration is gone: it is still reported,
        // which is the whole point of answering a non-terminal status.
        assert_eq!(
            listed_status(&h, row.case, &arn).await.as_deref(),
            Some("DEREGISTERING"),
            "case {:?}: the registration must still be reported while it is torn down",
            row.case
        );
    }
}
