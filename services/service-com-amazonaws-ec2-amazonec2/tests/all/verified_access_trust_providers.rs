// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the Verified Access trust provider slice:
//! `CreateVerifiedAccessTrustProvider` and `DescribeVerifiedAccessTrustProviders`, driven
//! over the wire the service actually speaks — `ec2Query`, through the crate's shared
//! [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. `CreateVerifiedAccessTrustProvider` MINTS A TRUST PROVIDER FROM THE REQUEST and
//!    `DescribeVerifiedAccessTrustProviders` REPORTS THE STORE: every member the request
//!    sets round-trips, and the members the service owns (the id, the creation and last
//!    updated timestamps) are derived rather than taken;
//! 2. each KIND of trust provider takes its own options: the members the modelled docs
//!    call required for that kind are required, and the other kind's members are refused;
//! 3. `DescribeVerifiedAccessTrustProviders` narrows by `VerifiedAccessTrustProviderIds`
//!    and by every filter it answers, and pages by `MaxResults`/`NextToken`;
//! 4. one `ClientToken` names one trust provider.
//!
//! Each table row is a distinct input along what the rule generalises over — the trust
//! provider kind and every value of its sub-type enum, which filter, how many values a
//! filter carries, which page size — so no row is the specification. The kind catalogs are
//! the model's own `TRUST_PROVIDER_TYPE_VALUES`, `USER_TRUST_PROVIDER_TYPE_VALUES` and
//! `DEVICE_TRUST_PROVIDER_TYPE_VALUES`, so every published kind is covered.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use ec2::ops::types::{
    DEVICE_TRUST_PROVIDER_TYPE_VALUES, TRUST_PROVIDER_TYPE_VALUES, USER_TRUST_PROVIDER_TYPE_VALUES,
};
use ec2::state::{
    MAX_TAGS_PER_RESOURCE, MAX_VERIFIED_ACCESS_TRUST_PROVIDERS_PAGE_SIZE,
    TRUST_PROVIDER_TYPE_USER, USER_TRUST_PROVIDER_TYPE_OIDC,
};
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, filters, indexed, unescaped};

/// The Region every call here is signed for. A trust provider is an account-level
/// resource — its modelled shape names no Region — so the Region only selects the
/// envelope.
const REGION: &str = "us-west-2";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// One row of a table that expects a status and, when it is a refusal, an error code:
/// `(case, the parameters under test, the HTTP status, the error code)`.
type StatusRow = (&'static str, Params, u16, &'static str);

/// One row of the filter table: `(case, the filter parameters, the ids it must select)`.
type FilterRow = (String, Params, Vec<String>);

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(
        router,
        REGION,
        "CreateVerifiedAccessTrustProvider",
        params,
    )
    .await
}

async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(
        router,
        REGION,
        "DescribeVerifiedAccessTrustProviders",
        params,
    )
    .await
}

/// A `VerifiedAccessTrustProviderId` is `vatp-` followed by 8 or 17 lowercase hex digits,
/// the short and the long EC2 id form.
fn is_trust_provider_id(candidate: &str) -> bool {
    let Some(suffix) = candidate.strip_prefix("vatp-") else {
        return false;
    };
    matches!(suffix.len(), 8 | 17)
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// An RFC 3339 UTC timestamp, as the service renders `CreationTime` and `LastUpdatedTime`:
/// `YYYY-MM-DDThh:mm:ssZ`.
fn is_iso8601_utc(candidate: &str) -> bool {
    candidate.len() >= 20
        && candidate.ends_with('Z')
        && candidate.as_bytes()[4] == b'-'
        && candidate.as_bytes()[7] == b'-'
        && candidate.as_bytes()[10] == b'T'
        && candidate.as_bytes()[13] == b':'
}

/// The sub-type catalog the model publishes for `trust_provider_type`.
fn sub_types(trust_provider_type: &str) -> &'static [&'static str] {
    if trust_provider_type == TRUST_PROVIDER_TYPE_USER {
        USER_TRUST_PROVIDER_TYPE_VALUES
    } else {
        DEVICE_TRUST_PROVIDER_TYPE_VALUES
    }
}

/// The parameters one kind of trust provider needs: its sub-type, plus the options the
/// modelled docs call required for that kind.
fn kind_params(trust_provider_type: &str, sub_type: &str) -> Params {
    let mut params = vec![p("TrustProviderType", trust_provider_type)];
    if trust_provider_type == TRUST_PROVIDER_TYPE_USER {
        params.push(p("UserTrustProviderType", sub_type));
        if sub_type == USER_TRUST_PROVIDER_TYPE_OIDC {
            params.extend([
                p("OidcOptions.Issuer", "https://login.example.test/"),
                p(
                    "OidcOptions.AuthorizationEndpoint",
                    "https://login.example.test/authorize",
                ),
                p("OidcOptions.TokenEndpoint", "https://login.example.test/token"),
                p(
                    "OidcOptions.UserInfoEndpoint",
                    "https://login.example.test/userinfo",
                ),
                p("OidcOptions.ClientId", "client-17"),
                p("OidcOptions.ClientSecret", "secret-17"),
                p("OidcOptions.Scope", "openid"),
            ]);
        }
    } else {
        params.push(p("DeviceTrustProviderType", sub_type));
        params.extend([
            p("DeviceOptions.TenantId", format!("tenant-{sub_type}")),
            p(
                "DeviceOptions.PublicSigningKeyUrl",
                "https://device.example.test/keys",
            ),
        ]);
    }
    params
}

/// Create one trust provider of `trust_provider_type`/`sub_type` with `policy_reference`
/// and whatever extra parameters the case needs.
async fn create_provider(
    router: &MultiServiceRouter,
    trust_provider_type: &str,
    sub_type: &str,
    policy_reference: &str,
    extra: Params,
) -> (u16, String) {
    let mut params = kind_params(trust_provider_type, sub_type);
    params.push(p("PolicyReferenceName", policy_reference));
    params.extend(extra);
    create(router, &params).await
}

/// Create one trust provider that must succeed and answer the id it was minted.
async fn seed(
    router: &MultiServiceRouter,
    case: &str,
    trust_provider_type: &str,
    sub_type: &str,
    policy_reference: &str,
    extra: Params,
) -> String {
    let (status, body) = create_provider(
        router,
        trust_provider_type,
        sub_type,
        policy_reference,
        extra,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateVerifiedAccessTrustProvider, \
         got {status} with body {body}"
    );
    let id = element(&body, "verifiedAccessTrustProviderId").unwrap_or_default();
    assert!(
        is_trust_provider_id(&id),
        "case {case:?}: expected a minted trust provider id, got {id:?}"
    );
    id
}

/// The ids `DescribeVerifiedAccessTrustProviders` reports for `params`, in the order it
/// reported them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeVerifiedAccessTrustProviders, \
         got {status} with body {body}"
    );
    elements(&body, "verifiedAccessTrustProviderId")
}

/// RULE 1 over the kind catalog: every published kind of trust provider is created from
/// the request, is given an id and timestamps of the service's own, and reads back out of
/// the store through `DescribeVerifiedAccessTrustProviders` with every member it was
/// created with.
#[tokio::test]
async fn every_published_kind_round_trips_through_the_describe() {
    for trust_provider_type in TRUST_PROVIDER_TYPE_VALUES {
        for sub_type in sub_types(trust_provider_type) {
            let router = emulator();
            let case = format!("{trust_provider_type}/{sub_type}");
            let policy_reference = format!("ref{}", sub_type.replace('-', ""));
            let (status, body) = create_provider(
                &router,
                trust_provider_type,
                sub_type,
                &policy_reference,
                vec![
                    p("Description", format!("the {sub_type} provider")),
                    p(
                        "TagSpecification.1.ResourceType",
                        "verified-access-trust-provider",
                    ),
                    p("TagSpecification.1.Tag.1.Key", "Team"),
                    p("TagSpecification.1.Tag.1.Value", "platform"),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );

            let id = element(&body, "verifiedAccessTrustProviderId").unwrap_or_default();
            assert!(
                is_trust_provider_id(&id),
                "case {case:?}: expected a minted trust provider id, got {id:?}"
            );
            let creation_time = element(&body, "creationTime").unwrap_or_default();
            let last_updated_time = element(&body, "lastUpdatedTime").unwrap_or_default();
            assert!(
                is_iso8601_utc(&creation_time),
                "case {case:?}: expected an ISO 8601 creationTime, got {creation_time:?}"
            );
            assert_eq!(
                last_updated_time, creation_time,
                "case {case:?}: a freshly created trust provider was last updated when it \
                 was created, got {last_updated_time:?}"
            );

            // Every member the request set, as the response reports it.
            let mut expected = vec![
                ("trustProviderType", (*trust_provider_type).to_string()),
                ("policyReferenceName", policy_reference.clone()),
                ("description", format!("the {sub_type} provider")),
                // "The options for server side encryption": no specification was named, so
                // no customer managed key is in use.
                ("customerManagedKeyEnabled", "false".to_string()),
                ("key", "Team".to_string()),
                ("value", "platform".to_string()),
            ];
            if *trust_provider_type == TRUST_PROVIDER_TYPE_USER {
                expected.push(("userTrustProviderType", (*sub_type).to_string()));
                if *sub_type == USER_TRUST_PROVIDER_TYPE_OIDC {
                    expected.extend([
                        ("issuer", "https://login.example.test/".to_string()),
                        ("clientId", "client-17".to_string()),
                        ("clientSecret", "secret-17".to_string()),
                        ("scope", "openid".to_string()),
                    ]);
                }
            } else {
                expected.extend([
                    ("deviceTrustProviderType", (*sub_type).to_string()),
                    ("tenantId", format!("tenant-{sub_type}")),
                    (
                        "publicSigningKeyUrl",
                        "https://device.example.test/keys".to_string(),
                    ),
                ]);
            }
            for (member, value) in &expected {
                assert_eq!(
                    element(&body, member).as_deref(),
                    Some(value.as_str()),
                    "case {case:?}: expected {member}={value:?} from the create, got {:?}",
                    element(&body, member)
                );
            }

            // The kind's own members, and only those: a user provider reports no device
            // sub-type and a device provider no user sub-type.
            let absent = if *trust_provider_type == TRUST_PROVIDER_TYPE_USER {
                ["deviceTrustProviderType", "tenantId"]
            } else {
                ["userTrustProviderType", "issuer"]
            };
            for member in absent {
                assert!(
                    element(&body, member).is_none(),
                    "case {case:?}: a {trust_provider_type} provider reports no {member}, \
                     got {body}"
                );
            }

            // ROUND TRIP: the trust provider is in the store, not just echoed.
            let (status, described) =
                describe(&router, &indexed("VerifiedAccessTrustProviderId", &[&id])).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from the describe, got {status} / {described}"
            );
            assert_eq!(
                elements(&described, "verifiedAccessTrustProviderId"),
                vec![id.clone()],
                "case {case:?}: expected the describe to report the created trust provider, \
                 got {described}"
            );
            for (member, value) in &expected {
                assert_eq!(
                    element(&described, member).as_deref(),
                    Some(value.as_str()),
                    "case {case:?}: expected the stored {member}={value:?}, got {:?}",
                    element(&described, member)
                );
            }
            assert_eq!(
                element(&described, "creationTime").as_deref(),
                Some(creation_time.as_str()),
                "case {case:?}: the creation time is stored, not re-derived per read, got {:?}",
                element(&described, "creationTime")
            );
        }
    }
}

/// RULE 2: each kind takes its OWN options — the members the model calls required for a
/// kind are required, and the other kind's members are refused — and a refused create
/// stores nothing.
#[tokio::test]
async fn each_kind_requires_its_own_options_and_refuses_the_others() {
    let oidc_options = vec![
        p("OidcOptions.Issuer", "https://login.example.test/"),
        p("OidcOptions.ClientId", "client-17"),
    ];
    let device_options = vec![p("DeviceOptions.TenantId", "tenant-7")];

    let rows: Vec<StatusRow> = vec![
        (
            "a user/oidc provider with its OIDC options",
            [
                vec![
                    p("TrustProviderType", "user"),
                    p("UserTrustProviderType", "oidc"),
                ],
                oidc_options.clone(),
            ]
            .concat(),
            200,
            "",
        ),
        (
            "a user/iam-identity-center provider, which carries no OIDC endpoints",
            vec![
                p("TrustProviderType", "user"),
                p("UserTrustProviderType", "iam-identity-center"),
            ],
            200,
            "",
        ),
        (
            "a device provider with its device options",
            [
                vec![
                    p("TrustProviderType", "device"),
                    p("DeviceTrustProviderType", "jamf"),
                ],
                device_options.clone(),
            ]
            .concat(),
            200,
            "",
        ),
        (
            "a user provider with no user sub-type",
            vec![p("TrustProviderType", "user")],
            400,
            "MissingParameter",
        ),
        (
            "a device provider with no device sub-type",
            vec![p("TrustProviderType", "device")],
            400,
            "MissingParameter",
        ),
        (
            "a user/oidc provider with no OIDC options",
            vec![
                p("TrustProviderType", "user"),
                p("UserTrustProviderType", "oidc"),
            ],
            400,
            "MissingParameter",
        ),
        (
            "a device provider with no device options",
            vec![
                p("TrustProviderType", "device"),
                p("DeviceTrustProviderType", "crowdstrike"),
            ],
            400,
            "MissingParameter",
        ),
        (
            "a user provider carrying a device sub-type",
            [
                vec![
                    p("TrustProviderType", "user"),
                    p("UserTrustProviderType", "oidc"),
                    p("DeviceTrustProviderType", "jumpcloud"),
                ],
                oidc_options.clone(),
            ]
            .concat(),
            400,
            "InvalidParameterCombination",
        ),
        (
            "a device provider carrying a user sub-type",
            [
                vec![
                    p("TrustProviderType", "device"),
                    p("DeviceTrustProviderType", "jamf"),
                    p("UserTrustProviderType", "oidc"),
                ],
                device_options.clone(),
            ]
            .concat(),
            400,
            "InvalidParameterCombination",
        ),
        (
            "a user provider carrying device options",
            [
                vec![
                    p("TrustProviderType", "user"),
                    p("UserTrustProviderType", "oidc"),
                ],
                oidc_options.clone(),
                device_options.clone(),
            ]
            .concat(),
            400,
            "InvalidParameterCombination",
        ),
        (
            "a device provider carrying OIDC options",
            [
                vec![
                    p("TrustProviderType", "device"),
                    p("DeviceTrustProviderType", "jumpcloud"),
                ],
                device_options.clone(),
                oidc_options.clone(),
            ]
            .concat(),
            400,
            "InvalidParameterCombination",
        ),
        (
            "an iam-identity-center provider carrying OIDC options, which the model makes a \
             user-provider member without qualifying the kind",
            [
                vec![
                    p("TrustProviderType", "user"),
                    p("UserTrustProviderType", "iam-identity-center"),
                ],
                oidc_options.clone(),
            ]
            .concat(),
            200,
            "",
        ),
    ];

    for (case, kind, expected_status, expected_code) in rows {
        let router = emulator();
        let mut params = kind;
        params.push(p("PolicyReferenceName", "refone"));
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 200 {
            assert!(
                is_trust_provider_id(
                    &element(&body, "verifiedAccessTrustProviderId").unwrap_or_default()
                ),
                "case {case:?}: expected a minted trust provider, got {body}"
            );
            continue;
        }
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        assert!(
            described_ids(&router, case, &[]).await.is_empty(),
            "case {case:?}: a refused CreateVerifiedAccessTrustProvider must store nothing"
        );
    }
}

/// RULE 2: `TagSpecifications` tags the trust provider this call creates and nothing else,
/// and the documented tag quota holds.
#[tokio::test]
async fn tag_specifications_tag_the_trust_provider_and_nothing_else() {
    let router = emulator();
    let tags = [("Name", "sales"), ("Team", "platform"), ("Tier", "web")];
    let mut extra = vec![p(
        "TagSpecification.1.ResourceType",
        "verified-access-trust-provider",
    )];
    for (index, (key, value)) in tags.iter().enumerate() {
        extra.push(p(&format!("TagSpecification.1.Tag.{}.Key", index + 1), *key));
        extra.push(p(
            &format!("TagSpecification.1.Tag.{}.Value", index + 1),
            *value,
        ));
    }
    let id = seed(&router, "three tags", "user", "oidc", "reftags", extra).await;

    let (status, body) =
        describe(&router, &indexed("VerifiedAccessTrustProviderId", &[&id])).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status} with body {body}");
    for (key, value) in tags {
        assert!(
            body.contains(&format!("<key>{key}</key><value>{value}</value>")),
            "case {key:?}: expected the stored tag {key}={value}, got {body}"
        );
    }

    // A specification naming another resource type is refused: this call creates one
    // resource, and tags for anything else would be accepted and dropped.
    for resource_type in ["instance", "volume", "verified-access-instance"] {
        let (status, body) = create_provider(
            &router,
            "user",
            "iam-identity-center",
            "refother",
            vec![
                p("TagSpecification.1.ResourceType", resource_type),
                p("TagSpecification.1.Tag.1.Key", "Name"),
                p("TagSpecification.1.Tag.1.Value", "nope"),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {resource_type:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {resource_type:?}: expected InvalidParameterValue, got {body}"
        );
    }

    // One tag past the documented quota.
    let mut too_many = vec![p(
        "TagSpecification.1.ResourceType",
        "verified-access-trust-provider",
    )];
    for nth in 0..=MAX_TAGS_PER_RESOURCE {
        too_many.push(p(
            &format!("TagSpecification.1.Tag.{}.Key", nth + 1),
            format!("k{nth}"),
        ));
        too_many.push(p(&format!("TagSpecification.1.Tag.{}.Value", nth + 1), "v"));
    }
    let (status, body) = create_provider(
        &router,
        "user",
        "iam-identity-center",
        "refquota",
        too_many,
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"one tag past the quota\": expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "TagLimitExceeded",
        "case \"one tag past the quota\": expected TagLimitExceeded, got {body}"
    );
}

/// RULE 3: the describe reports the store narrowed by `VerifiedAccessTrustProviderIds` and
/// by every filter it answers, with EC2's AND across filters, OR within one filter, and
/// `*` wildcards.
#[tokio::test]
async fn the_describe_narrows_by_id_and_by_every_answered_filter() {
    let router = emulator();

    // Trust providers differing along every attribute the filters select on.
    struct Seed {
        trust_provider_type: &'static str,
        sub_type: &'static str,
        policy_reference: &'static str,
        description: &'static str,
        team: &'static str,
    }
    let seeds = [
        Seed {
            trust_provider_type: "user",
            sub_type: "oidc",
            policy_reference: "refsales",
            description: "sales-login",
            team: "platform",
        },
        Seed {
            trust_provider_type: "user",
            sub_type: "iam-identity-center",
            policy_reference: "refstaff",
            description: "staff-login",
            team: "platform",
        },
        Seed {
            trust_provider_type: "device",
            sub_type: "jamf",
            policy_reference: "refmacs",
            description: "sales-macs",
            team: "endpoint",
        },
        Seed {
            trust_provider_type: "device",
            sub_type: "crowdstrike",
            policy_reference: "reffalcon",
            description: "falcon-agents",
            team: "endpoint",
        },
    ];

    let mut created: Vec<String> = Vec::new();
    for entry in &seeds {
        let id = seed(
            &router,
            entry.policy_reference,
            entry.trust_provider_type,
            entry.sub_type,
            entry.policy_reference,
            vec![
                p("Description", entry.description),
                p(
                    "TagSpecification.1.ResourceType",
                    "verified-access-trust-provider",
                ),
                p("TagSpecification.1.Tag.1.Key", "Team"),
                p("TagSpecification.1.Tag.1.Value", entry.team),
            ],
        )
        .await;
        created.push(id);
    }
    // The describe reports and pages in id order.
    let mut in_id_order = created.clone();
    in_id_order.sort();
    assert_eq!(
        described_ids(&router, "every stored trust provider", &[]).await,
        in_id_order,
        "a describe with no narrowing reports every stored trust provider, in id order"
    );

    let only = |indexes: &[usize]| -> Vec<String> {
        let mut ids: Vec<String> = indexes.iter().map(|index| created[*index].clone()).collect();
        ids.sort();
        ids
    };

    let rows: Vec<FilterRow> = vec![
        (
            "VerifiedAccessTrustProviderIds names two of the four".to_string(),
            indexed(
                "VerifiedAccessTrustProviderId",
                &[&created[0], &created[3]],
            ),
            only(&[0, 3]),
        ),
        (
            "trust-provider-type=user".to_string(),
            filters(&[("trust-provider-type", &["user"])]),
            only(&[0, 1]),
        ),
        (
            "trust-provider-type=device".to_string(),
            filters(&[("trust-provider-type", &["device"])]),
            only(&[2, 3]),
        ),
        (
            "user-trust-provider-type=oidc".to_string(),
            filters(&[("user-trust-provider-type", &["oidc"])]),
            only(&[0]),
        ),
        (
            "device-trust-provider-type=jamf".to_string(),
            filters(&[("device-trust-provider-type", &["jamf"])]),
            only(&[2]),
        ),
        (
            "two values of one filter are joined with OR".to_string(),
            filters(&[("device-trust-provider-type", &["jamf", "crowdstrike"])]),
            only(&[2, 3]),
        ),
        (
            "description, exactly".to_string(),
            filters(&[("description", &["staff-login"])]),
            only(&[1]),
        ),
        (
            "description, with a wildcard".to_string(),
            filters(&[("description", &["sales-*"])]),
            only(&[0, 2]),
        ),
        (
            "policy-reference-name".to_string(),
            filters(&[("policy-reference-name", &["reffalcon"])]),
            only(&[3]),
        ),
        (
            "verified-access-trust-provider-id".to_string(),
            filters(&[(
                "verified-access-trust-provider-id",
                &[created[1].as_str()],
            )]),
            only(&[1]),
        ),
        (
            "tag:Team".to_string(),
            filters(&[("tag:Team", &["endpoint"])]),
            only(&[2, 3]),
        ),
        (
            "tag-key".to_string(),
            filters(&[("tag-key", &["Team"])]),
            only(&[0, 1, 2, 3]),
        ),
        (
            "two filters are joined with AND".to_string(),
            filters(&[
                ("trust-provider-type", &["device"]),
                ("tag:Team", &["endpoint"]),
                ("description", &["sales-*"]),
            ]),
            only(&[2]),
        ),
        (
            "a filter AND an id narrowing".to_string(),
            [
                indexed("VerifiedAccessTrustProviderId", &[&created[0], &created[1]]),
                filters(&[("user-trust-provider-type", &["iam-identity-center"])]),
            ]
            .concat(),
            only(&[1]),
        ),
        (
            "a value no trust provider carries".to_string(),
            filters(&[("policy-reference-name", &["refnothing"])]),
            Vec::new(),
        ),
        (
            "a filter naming an attribute the kind does not carry".to_string(),
            filters(&[("user-trust-provider-type", &["jamf"])]),
            Vec::new(),
        ),
        (
            "a filter with no value at all matches nothing".to_string(),
            filters(&[("description", &[])]),
            Vec::new(),
        ),
    ];

    for (case, params, expected) in rows {
        assert_eq!(
            described_ids(&router, &case, &params).await,
            expected,
            "case {case:?}: expected exactly those trust providers"
        );
    }

    // `creation-time` and `last-updated-time` are the two answered filters whose values the
    // SERVICE derives (from its own clock), so their rows are computed from what the
    // describe reports rather than written out: filtering on a timestamp selects exactly
    // the trust providers that carry it, and a wildcard over the date part selects every
    // one created on that date.
    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "case \"clock filters\": expected HTTP 200, got {status} with body {body}"
    );
    let reported = elements(&body, "verifiedAccessTrustProviderId");
    for (name, member) in [
        ("creation-time", "creationTime"),
        ("last-updated-time", "lastUpdatedTime"),
    ] {
        let times = elements(&body, member);
        assert_eq!(
            times.len(),
            reported.len(),
            "case {name:?}: expected one {member} per reported trust provider, got {times:?}"
        );
        let probe = times.first().cloned().unwrap_or_default();
        let mut carrying_probe: Vec<String> = reported
            .iter()
            .zip(&times)
            .filter(|(_, time)| *time == &probe)
            .map(|(id, _)| id.clone())
            .collect();
        carrying_probe.sort();
        let exact = described_ids(
            &router,
            &format!("{name}={probe}"),
            &filters(&[(name, &[probe.as_str()])]),
        )
        .await;
        assert_eq!(
            exact, carrying_probe,
            "case {name:?}={probe:?}: expected exactly the trust providers carrying that \
             timestamp, got {exact:?}"
        );
        // The date part of an RFC 3339 timestamp is its first 10 characters; every trust
        // provider here was created by the same call sequence, so all of them carry it.
        let date = probe.get(..10).unwrap_or_default();
        let pattern = format!("{date}*");
        let wildcarded = described_ids(
            &router,
            &format!("{name}={pattern}"),
            &filters(&[(name, &[pattern.as_str()])]),
        )
        .await;
        assert_eq!(
            wildcarded, in_id_order,
            "case {name:?}={pattern:?}: expected every trust provider created on that date, \
             got {wildcarded:?}"
        );
    }

    // An id that names no trust provider of the account is an error, not an absent row.
    for id in [
        "vatp-00000000000000999",
        "vatp-0000ffff",
        "not-an-id",
        &format!("{}x", created[0]),
    ] {
        let (status, body) =
            describe(&router, &indexed("VerifiedAccessTrustProviderId", &[id])).await;
        assert_eq!(
            status, 400,
            "case {id:?}: expected HTTP 400 for an unknown id, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidVerifiedAccessTrustProviderId.NotFound",
            "case {id:?}: expected InvalidVerifiedAccessTrustProviderId.NotFound, got {body}"
        );
        // The message is built from the request: it names the id the caller gave, so the
        // caller can tell WHICH of the ids it named is unknown.
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert!(
            message.contains(id),
            "case {id:?}: expected the refusal to name the id the request gave, \
             got {message:?}"
        );
    }

    // A filter name this operation does not answer is refused, rather than read as "no
    // results".
    for name in ["trustProviderType", "tag:", "trust-provider", "TAG-KEY"] {
        let (status, body) = describe(&router, &filters(&[(name, &["user"])])).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400 for an unanswered filter, got {status} / {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: expected InvalidParameterValue, got {body}"
        );
    }

    // DryRun reports what the request WOULD have done and changes nothing.
    let (status, body) = describe(&router, &[p("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "case \"DryRun\": expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case \"DryRun\": expected DryRunOperation, got {body}"
    );
    assert_eq!(
        described_ids(&router, "after a DryRun", &[]).await,
        in_id_order,
        "case \"DryRun\": a dry run changes nothing a later caller sees"
    );
}

/// RULE 3: `MaxResults` and `NextToken` walk the collection exactly once, whatever page
/// size the caller asks for, and a token this service did not mint is refused.
#[tokio::test]
async fn the_describe_pages_by_max_results_and_next_token() {
    let router = emulator();

    // More trust providers than the smallest page the model admits (`range 5..200`), so
    // more than one page is needed at every size under test.
    let mut created: Vec<String> = Vec::new();
    for nth in 0..7 {
        let id = seed(
            &router,
            &format!("page seed {nth}"),
            "user",
            "iam-identity-center",
            &format!("refpage{nth}"),
            Vec::new(),
        )
        .await;
        created.push(id);
    }
    created.sort();

    // The smallest page the model admits, one in between, exactly the collection, and one
    // more than it.
    for page_size in [5usize, 6, created.len(), created.len() + 1] {
        let case = format!("MaxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut params = vec![p("MaxResults", page_size.to_string())];
            if let Some(token) = &next_token {
                params.push(p("NextToken", token.clone()));
            }
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = elements(&body, "verifiedAccessTrustProviderId");
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most MaxResults trust providers, got {}",
                page.len()
            );
            walked.extend(page);
            next_token = element(&body, "nextToken");
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, created,
            "case {case:?}: paging must walk every trust provider exactly once, got {walked:?}"
        );
    }

    // No page size named: one page, up to the largest the operation can be asked for.
    let (status, body) = describe(&router, &[]).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status} with body {body}");
    assert!(
        created.len() <= MAX_VERIFIED_ACCESS_TRUST_PROVIDERS_PAGE_SIZE,
        "this suite seeds fewer trust providers than one page holds"
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "case \"no MaxResults\": a page that holds everything carries no token, got {body}"
    );

    // A token this service did not issue is refused rather than read as a cursor.
    for token in ["not-a-token", "00", ""] {
        let (status, body) = describe(&router, &[p("NextToken", token)]).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400 for a foreign NextToken, got {status} / {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: expected InvalidPaginationToken, got {body}"
        );
    }

    // A page size outside the modelled `range 5..200` is refused at the boundary, before
    // the handler runs, so only the status is this operation's to promise.
    for (case, max_results) in [
        ("below the modelled minimum", "4"),
        ("zero", "0"),
        ("negative", "-1"),
        ("above the modelled maximum", "201"),
    ] {
        let (status, body) = describe(&router, &[p("MaxResults", max_results)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
    }
}

/// RULE 4: "A unique, case-sensitive token that you provide to ensure idempotency"
/// (`CreateVerifiedAccessTrustProviderRequest$ClientToken`) — one token names one trust
/// provider, and a different token mints another.
#[tokio::test]
async fn one_client_token_names_one_trust_provider() {
    let router = emulator();

    let first = seed(
        &router,
        "the first create",
        "user",
        "iam-identity-center",
        "reftoken",
        vec![p("ClientToken", "token-a")],
    )
    .await;
    // The same token replayed, with the same parameters: the trust provider it already
    // minted, and no second one.
    let replayed = seed(
        &router,
        "the same token replayed",
        "user",
        "iam-identity-center",
        "reftoken",
        vec![p("ClientToken", "token-a")],
    )
    .await;
    assert_eq!(
        replayed, first,
        "case \"the same token replayed\": one token names one trust provider"
    );
    // Case-sensitive: a token differing only in case is another token.
    let other = seed(
        &router,
        "a token differing only in case",
        "user",
        "iam-identity-center",
        "reftoken",
        vec![p("ClientToken", "TOKEN-A")],
    )
    .await;
    assert_ne!(
        other, first,
        "case \"a token differing only in case\": the token is case-sensitive"
    );

    let mut expected = vec![first, other];
    expected.sort();
    assert_eq!(
        described_ids(&router, "after the replay", &[]).await,
        expected,
        "a replayed create stores nothing new"
    );
}
