// SPDX-License-Identifier: CC0-1.0

//! The CloudFront continuous deployment policy surface, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path
//! a client takes. The rules under test are the ones CloudFront states for this surface:
//!
//! * a policy is PERSISTED by the call that creates it, which MINTS its identity (a UUID
//!   `Id` and a version token), so a later `GetContinuousDeploymentPolicy` answers from
//!   that record rather than from the request being served — down to a `TrafficConfig`
//!   the create left out;
//! * `ListContinuousDeploymentPolicies` reports the policies the store holds in the one
//!   order its `Marker` indexes into, `MaxItems` cuts the page, `NextMarker` is present
//!   exactly when policies remain and names the id the page ended at, and `Quantity` is
//!   "The total number of continuous deployment policies in your Amazon Web Services
//!   account, regardless of the `MaxItems` value" — so walking the pages at ANY
//!   admissible page size enumerates the collection exactly once;
//! * a page size that names no page and a `Marker` that is no resume point this service
//!   handed out are refused with the errors the operation models (`InvalidArgument` 400,
//!   `NoSuchContinuousDeploymentPolicy` 404);
//! * a configuration outside what the model and the documentation allow is refused
//!   before anything is stored: a `Quantity` that disagrees with its `Items`
//!   (`InconsistentQuantities`), a `TrafficConfig` missing the configuration its `Type`
//!   selects, a `Weight` outside "between 0 and 0.15", a session stickiness TTL outside
//!   "300–3600 seconds" or with `IdleTTL` past `MaximumTTL`, and a single-header name
//!   without the documented `aws-cf-cd-` prefix (`InvalidArgument`);
//! * "A continuous deployment policy for this staging distribution already exists": a
//!   staging distribution takes part in at most one policy, and a repeated configuration
//!   is the policy that already holds it.
//!
//! The rows differ along everything these rules generalise over: how many staging
//! distribution domain names a policy names, whether it is enabled, which of the two
//! traffic configurations it carries (and whether it carries one at all), whether a
//! single-weight configuration names session stickiness, the weight and the TTLs at each
//! published bound and one past them, how many policies the account holds, and the page
//! size requested.
//!
//! The harness (the router, the request helper and the element scanner) is the lifecycle
//! suite's, reused rather than copied.

use std::collections::BTreeSet;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    MAX_CONTINUOUS_DEPLOYMENT_POLICIES, MAX_ITEMS, MAX_SESSION_STICKINESS_TTL,
    MAX_STAGING_DISTRIBUTION_DNS_NAMES, MAX_TRAFFIC_WEIGHT, MIN_SESSION_STICKINESS_TTL,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, config as distribution_config, create_distribution, emulator, is_minted_uuid,
    is_version_token, Answer, Node, API_VERSION, HOST,
};

// ---------------------------------------------------------------------------
// Calling the continuous deployment policy surface
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/continuous-deployment-policy` with the `ContinuousDeploymentPolicyConfig`
/// document `body` (the operation's `@httpPayload` member).
pub(crate) async fn create_policy(router: &MultiServiceRouter, body: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/continuous-deployment-policy"),
        Some(body),
        None,
    )
    .await
}

/// `GET /2020-05-31/continuous-deployment-policy/{Id}`.
pub(crate) async fn get_policy(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/continuous-deployment-policy/{id}"),
        None,
        None,
    )
    .await
}

/// `GET /2020-05-31/continuous-deployment-policy`, with `query` appended verbatim
/// (`?MaxItems=2&Marker=…`), from the operation's `http` binding.
async fn list_policies(router: &MultiServiceRouter, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/continuous-deployment-policy{query}"),
        None,
        None,
    )
    .await
}

// ---------------------------------------------------------------------------
// The configurations under test
// ---------------------------------------------------------------------------

/// The traffic configuration a policy carries: `TrafficConfig` is optional, and its
/// `Type` selects which of the two inner configurations applies.
///
/// `pub(crate)` for the reason [`create_policy`] is: a sibling suite that submits a
/// continuous deployment policy configuration writes it with this one writer rather than a
/// copy of it.
#[derive(Debug, Clone)]
pub(crate) enum Traffic {
    /// No `TrafficConfig` element at all.
    None,
    /// `Type` `SingleWeight`, with `(IdleTTL, MaximumTTL)` when the row names session
    /// stickiness.
    SingleWeight {
        weight: f32,
        stickiness: Option<(i32, i32)>,
    },
    /// `Type` `SingleHeader`.
    SingleHeader { header: String, value: String },
}

/// One `ContinuousDeploymentPolicyConfig` a client submits.
#[derive(Debug, Clone)]
pub(crate) struct Policy {
    pub(crate) case: &'static str,
    /// `StagingDistributionDnsNames.Items`, in order.
    pub(crate) dns_names: Vec<String>,
    pub(crate) enabled: bool,
    pub(crate) traffic: Traffic,
}

impl Policy {
    /// The `ContinuousDeploymentPolicyConfig` XML a client sends for this policy.
    pub(crate) fn xml(&self) -> String {
        self.xml_with_quantity(i32::try_from(self.dns_names.len()).expect("a small table"))
    }

    /// The same document with `StagingDistributionDnsNames.Quantity` stated as
    /// `quantity`, so a row can state a quantity that disagrees with its `Items`.
    fn xml_with_quantity(&self, quantity: i32) -> String {
        let mut out = format!(
            "<ContinuousDeploymentPolicyConfig \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">"
        );
        // `StagingDistributionDnsNameList`'s member is named `DnsName` in XML.
        out.push_str(&format!(
            "<StagingDistributionDnsNames><Quantity>{quantity}</Quantity><Items>"
        ));
        for name in &self.dns_names {
            out.push_str(&format!("<DnsName>{name}</DnsName>"));
        }
        out.push_str("</Items></StagingDistributionDnsNames>");
        out.push_str(&format!("<Enabled>{}</Enabled>", self.enabled));
        match &self.traffic {
            Traffic::None => {}
            Traffic::SingleWeight { weight, stickiness } => {
                out.push_str(&format!(
                    "<TrafficConfig><SingleWeightConfig><Weight>{weight}</Weight>"
                ));
                if let Some((idle, maximum)) = stickiness {
                    out.push_str(&format!(
                        "<SessionStickinessConfig><IdleTTL>{idle}</IdleTTL>\
                         <MaximumTTL>{maximum}</MaximumTTL></SessionStickinessConfig>"
                    ));
                }
                out.push_str("</SingleWeightConfig><Type>SingleWeight</Type></TrafficConfig>");
            }
            Traffic::SingleHeader { header, value } => {
                out.push_str(&format!(
                    "<TrafficConfig><SingleHeaderConfig><Header>{header}</Header>\
                     <Value>{value}</Value></SingleHeaderConfig>\
                     <Type>SingleHeader</Type></TrafficConfig>"
                ));
            }
        }
        out.push_str("</ContinuousDeploymentPolicyConfig>");
        out
    }

    /// Assert that `config` — a `ContinuousDeploymentPolicyConfig` element the emulator
    /// answered with — is the configuration this row submitted.
    pub(crate) fn assert_matches(&self, config: &Node, context: &str) {
        let case = self.case;
        let names = config
            .at("StagingDistributionDnsNames")
            .unwrap_or_else(|| panic!("case {case} ({context}): no StagingDistributionDnsNames"));
        assert_eq!(
            names.text_at("Quantity"),
            Some(self.dns_names.len().to_string().as_str()),
            "case {case} ({context}): expected Quantity {}, got {:?}",
            self.dns_names.len(),
            names.text_at("Quantity")
        );
        let listed: Vec<String> = names
            .child("Items")
            .map(|items| {
                items
                    .children_named("DnsName")
                    .iter()
                    .map(|node| node.text().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            listed, self.dns_names,
            "case {case} ({context}): expected the submitted domain names {:?}, got {listed:?}",
            self.dns_names
        );
        assert_eq!(
            config.text_at("Enabled"),
            Some(if self.enabled { "true" } else { "false" }),
            "case {case} ({context}): expected Enabled {}, got {:?}",
            self.enabled,
            config.text_at("Enabled")
        );
        match &self.traffic {
            Traffic::None => assert!(
                config.at("TrafficConfig").is_none(),
                "case {case} ({context}): a config that named no TrafficConfig must report none"
            ),
            Traffic::SingleWeight { weight, stickiness } => {
                assert_eq!(
                    config.text_at("TrafficConfig/Type"),
                    Some("SingleWeight"),
                    "case {case} ({context}): expected TrafficConfig.Type SingleWeight, got {:?}",
                    config.text_at("TrafficConfig/Type")
                );
                let reported: f32 = config
                    .text_at("TrafficConfig/SingleWeightConfig/Weight")
                    .unwrap_or_default()
                    .parse()
                    .unwrap_or(f32::NAN);
                assert!(
                    (reported - weight).abs() < f32::EPSILON,
                    "case {case} ({context}): expected Weight {weight}, got {reported}"
                );
                match stickiness {
                    Some((idle, maximum)) => {
                        assert_eq!(
                            config.text_at(
                                "TrafficConfig/SingleWeightConfig/SessionStickinessConfig/IdleTTL"
                            ),
                            Some(idle.to_string().as_str()),
                            "case {case} ({context}): expected IdleTTL {idle}, got {:?}",
                            config.text_at(
                                "TrafficConfig/SingleWeightConfig/SessionStickinessConfig/IdleTTL"
                            )
                        );
                        assert_eq!(
                            config.text_at(
                                "TrafficConfig/SingleWeightConfig/SessionStickinessConfig/MaximumTTL"
                            ),
                            Some(maximum.to_string().as_str()),
                            "case {case} ({context}): expected MaximumTTL {maximum}, got {:?}",
                            config.text_at(
                                "TrafficConfig/SingleWeightConfig/SessionStickinessConfig/MaximumTTL"
                            )
                        );
                    }
                    None => assert!(
                        config
                            .at("TrafficConfig/SingleWeightConfig/SessionStickinessConfig")
                            .is_none(),
                        "case {case} ({context}): a weight config that named no session \
                         stickiness must report none"
                    ),
                }
            }
            Traffic::SingleHeader { header, value } => {
                assert_eq!(
                    config.text_at("TrafficConfig/Type"),
                    Some("SingleHeader"),
                    "case {case} ({context}): expected TrafficConfig.Type SingleHeader, got {:?}",
                    config.text_at("TrafficConfig/Type")
                );
                assert_eq!(
                    config.text_at("TrafficConfig/SingleHeaderConfig/Header"),
                    Some(header.as_str()),
                    "case {case} ({context}): expected Header {header:?}, got {:?}",
                    config.text_at("TrafficConfig/SingleHeaderConfig/Header")
                );
                assert_eq!(
                    config.text_at("TrafficConfig/SingleHeaderConfig/Value"),
                    Some(value.as_str()),
                    "case {case} ({context}): expected Value {value:?}, got {:?}",
                    config.text_at("TrafficConfig/SingleHeaderConfig/Value")
                );
            }
        }
    }
}

/// The policies every rule below is asserted over.
///
/// The rows differ along what the rules generalise over: one or several staging
/// distribution domain names, enabled and disabled, no traffic configuration at all,
/// each of the two the `Type` enum admits (`ContinuousDeploymentPolicyType` is
/// `SingleHeader | SingleWeight`, from `smithy_query`), a weight at each published bound
/// of "between 0 and 0.15", and session stickiness at the bounds of "300–3600 seconds".
///
/// The domain names here are PLACEHOLDERS: a policy names its staging distribution by the
/// domain name CloudFront assigned it, so a row that is to be STORED goes through
/// [`staged`] first, which replaces them with the domain names of distributions this
/// service really holds. A row that a refusal is asserted on keeps them, since a request
/// refused before the names are resolved never reaches that lookup.
pub(crate) fn policy_table() -> Vec<Policy> {
    vec![
        Policy {
            case: "one staging domain, no traffic configuration",
            dns_names: vec!["d111111abcdef8.cloudfront.net".to_string()],
            enabled: true,
            traffic: Traffic::None,
        },
        Policy {
            case: "disabled, a single-weight configuration at the lower bound",
            dns_names: vec!["d222222abcdef8.cloudfront.net".to_string()],
            enabled: false,
            traffic: Traffic::SingleWeight {
                weight: 0.0,
                stickiness: None,
            },
        },
        Policy {
            case: "a single-weight configuration at the published upper bound",
            dns_names: vec!["d333333abcdef8.cloudfront.net".to_string()],
            enabled: true,
            traffic: Traffic::SingleWeight {
                weight: MAX_TRAFFIC_WEIGHT,
                stickiness: Some((MIN_SESSION_STICKINESS_TTL, MAX_SESSION_STICKINESS_TTL)),
            },
        },
        Policy {
            case: "two staging domains and a single-header configuration",
            dns_names: vec![
                "d444444abcdef8.cloudfront.net".to_string(),
                "d555555abcdef8.cloudfront.net".to_string(),
            ],
            enabled: true,
            traffic: Traffic::SingleHeader {
                header: "aws-cf-cd-staging".to_string(),
                value: "true".to_string(),
            },
        },
        Policy {
            case: "a weight between the bounds, with equal session TTLs",
            dns_names: vec!["d666666abcdef8.cloudfront.net".to_string()],
            enabled: true,
            traffic: Traffic::SingleWeight {
                weight: 0.1,
                stickiness: Some((600, 600)),
            },
        },
    ]
}

// ---------------------------------------------------------------------------
// The staging distributions a policy points at
// ---------------------------------------------------------------------------

/// Create one distribution and report the domain name CloudFront ASSIGNED it.
///
/// `StagingDistributionDnsNames` is "the CloudFront domain name of the staging
/// distribution", so a policy this service stores names a distribution it really holds:
/// every domain name a stored policy carries below comes from a create like this one rather
/// than from a literal written here.
pub(crate) async fn staging_domain(router: &MultiServiceRouter, reference: &str) -> String {
    let created =
        create_distribution(router, &distribution_config("staging", reference).xml()).await;
    assert_eq!(
        created.status, 201,
        "case the staging distribution {reference}: expected HTTP 201 from CreateDistribution, \
         got {} with {}",
        created.status, created.raw
    );
    let domain = created
        .root()
        .text_at("DomainName")
        .unwrap_or_default()
        .to_string();
    assert!(
        !domain.is_empty(),
        "case the staging distribution {reference}: expected an assigned domain name, got {}",
        created.raw
    );
    domain
}

/// `count` staging distributions, in creation order, stamped with `prefix` so two callers
/// cannot ask for the same caller reference.
pub(crate) async fn staging_domains(
    router: &MultiServiceRouter,
    prefix: &str,
    count: usize,
) -> Vec<String> {
    let mut domains = Vec::with_capacity(count);
    for index in 0..count {
        domains.push(staging_domain(router, &format!("ref-{prefix}-{index}")).await);
    }
    domains
}

/// Every row with its placeholder staging distribution domain names replaced by the domain
/// names of distributions this call creates — one distribution per name, so no two rows
/// claim the same staging distribution.
pub(crate) async fn staged(
    router: &MultiServiceRouter,
    prefix: &str,
    rows: Vec<Policy>,
) -> Vec<Policy> {
    let mut staged = Vec::with_capacity(rows.len());
    for (index, mut row) in rows.into_iter().enumerate() {
        let count = row.dns_names.len();
        row.dns_names = staging_domains(router, &format!("{prefix}-{index}"), count).await;
        staged.push(row);
    }
    staged
}

// ---------------------------------------------------------------------------
// Reading what the service minted
// ---------------------------------------------------------------------------

/// The `ContinuousDeploymentPolicySummary` elements of a `ContinuousDeploymentPolicyList`.
fn summaries(list: &Node) -> Vec<&Node> {
    list.child("Items")
        .map(|items| items.children_named("ContinuousDeploymentPolicySummary"))
        .unwrap_or_default()
}

/// The policy ids a page carries, in the order it serves them.
fn summary_ids(list: &Node) -> Vec<String> {
    summaries(list)
        .iter()
        .map(|summary| {
            summary
                .text_at("ContinuousDeploymentPolicy/Id")
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Create every row of `policies` and report `(case, id)` in creation order.
async fn create_all(router: &MultiServiceRouter, policies: &[Policy]) -> Vec<(&'static str, String)> {
    let mut created = Vec::new();
    for policy in policies {
        let case = policy.case;
        let answer = create_policy(router, &policy.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            answer.status, answer.raw
        );
        let id = answer
            .root()
            .text_at("Id")
            .unwrap_or_default()
            .to_string();
        created.push((case, id));
    }
    created
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// A create PERSISTS the policy it mints, and the read answers from that one record: the
/// configuration as submitted, a UUID id, a `LastModifiedTime` and the version token the
/// create handed back. Across the table, no two policies share an id or a token.
#[tokio::test]
async fn create_persists_the_policy_the_read_then_reports() {
    let router = emulator();
    let mut ids = BTreeSet::new();
    let mut etags = BTreeSet::new();

    for policy in staged(&router, "persisted", policy_table()).await {
        let case = policy.case;
        let created = create_policy(&router, &policy.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            created.status, created.raw
        );

        let id = created
            .root()
            .text_at("Id")
            .unwrap_or_default()
            .to_string();
        assert!(
            is_minted_uuid(&id),
            "case {case}: expected a UUID Id, got {id:?}"
        );
        let etag = created.header("etag").to_string();
        assert!(
            is_version_token(&etag),
            "case {case}: expected a version token in the ETag header, got {etag:?}"
        );
        let location = created.header("location").to_string();
        assert_eq!(
            location,
            format!("https://{HOST}/{API_VERSION}/continuous-deployment-policy/{id}"),
            "case {case}: expected the new policy's URI in Location, got {location:?}"
        );
        assert!(ids.insert(id.clone()), "case {case}: the id {id} repeated");
        assert!(
            etags.insert(etag.clone()),
            "case {case}: the version token {etag} repeated"
        );
        policy.assert_matches(
            created
                .root()
                .at("ContinuousDeploymentPolicyConfig")
                .unwrap_or_else(|| {
                    panic!("case {case}: the create carries no config: {}", created.raw)
                }),
            "the create",
        );

        // The read answers from the stored record, not from any request.
        let read = get_policy(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected HTTP 200 from the read, got {} with {}",
            read.status, read.raw
        );
        assert_eq!(
            read.root().text_at("Id"),
            Some(id.as_str()),
            "case {case}: expected the read to report Id {id}, got {:?}",
            read.root().text_at("Id")
        );
        assert!(
            read.root()
                .text_at("LastModifiedTime")
                .is_some_and(|time| !time.is_empty()),
            "case {case}: expected a LastModifiedTime, got {:?}",
            read.root().text_at("LastModifiedTime")
        );
        assert_eq!(
            read.header("etag"),
            etag,
            "case {case}: expected the create's version token, got {:?}",
            read.header("etag")
        );
        policy.assert_matches(
            read.root()
                .at("ContinuousDeploymentPolicyConfig")
                .unwrap_or_else(|| {
                    panic!("case {case}: the read carries no config: {}", read.raw)
                }),
            "the read",
        );
    }
}

/// An id no policy holds is not found, whatever shape it takes.
#[tokio::test]
async fn a_read_refuses_an_id_no_policy_holds() {
    let router = emulator();
    let table = staged(&router, "readrefusal", policy_table()).await;
    create_all(&router, &table[..1]).await;

    for (case, id) in [
        ("a UUID never minted", "00000000-0000-0000-0000-000000000000"),
        ("an id of another shape", "E1111111111111"),
        ("not an id at all", "nope"),
    ] {
        let answer = get_policy(&router, id).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404 for {id:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchContinuousDeploymentPolicy",
            "case {case}: expected NoSuchContinuousDeploymentPolicy, got {}",
            answer.raw
        );
    }
}

/// An account holding no policy lists none: HTTP 200, a `Quantity` of zero, and no
/// `Items` element at all.
#[tokio::test]
async fn an_empty_account_lists_no_policies() {
    let router = emulator();
    let answer = list_policies(&router, "").await;
    assert_eq!(
        answer.status, 200,
        "case empty: expected HTTP 200, got {} with {}",
        answer.status, answer.raw
    );
    assert!(
        answer.raw.contains("<ContinuousDeploymentPolicyList"),
        "case empty: expected a <ContinuousDeploymentPolicyList> payload, got {}",
        answer.raw
    );
    let list = answer.root();
    assert_eq!(
        list.text_at("Quantity"),
        Some("0"),
        "case empty: expected Quantity 0, got {:?}",
        list.text_at("Quantity")
    );
    assert_eq!(
        list.text_at("MaxItems"),
        Some(MAX_ITEMS.to_string().as_str()),
        "case empty: expected MaxItems {MAX_ITEMS}, got {:?}",
        list.text_at("MaxItems")
    );
    assert_eq!(
        list.text_at("NextMarker"),
        None,
        "case empty: expected no NextMarker, got {:?}",
        list.text_at("NextMarker")
    );
    assert!(
        summaries(list).is_empty(),
        "case empty: expected no summaries, got {}",
        answer.raw
    );
}

/// A list reports exactly the policies the store holds, in the id order its `Marker`
/// indexes into, each summary carrying what the same record answers a read with — and
/// listing twice reports the same thing, because a list changes nothing.
#[tokio::test]
async fn a_list_reports_the_stored_policies_in_id_order() {
    let router = emulator();
    let table = staged(&router, "listed", policy_table()).await;
    let created = create_all(&router, &table).await;
    let expected: Vec<String> = {
        let mut ids: Vec<String> = created.iter().map(|(_, id)| id.clone()).collect();
        ids.sort();
        ids
    };

    let answer = list_policies(&router, "").await;
    assert_eq!(
        answer.status, 200,
        "case every-policy: expected HTTP 200, got {} with {}",
        answer.status, answer.raw
    );
    let list = answer.root();
    let listed = summary_ids(list);
    assert_eq!(
        listed, expected,
        "case every-policy: expected every policy in id order ({expected:?}), got {listed:?}"
    );
    // "The total number of continuous deployment policies in your Amazon Web Services
    // account, regardless of the MaxItems value."
    assert_eq!(
        list.text_at("Quantity"),
        Some(expected.len().to_string().as_str()),
        "case every-policy: expected Quantity {} (the whole account), got {:?}",
        expected.len(),
        list.text_at("Quantity")
    );
    assert_eq!(
        list.text_at("NextMarker"),
        None,
        "case every-policy: a page holding the whole collection has no NextMarker, got {:?}",
        list.text_at("NextMarker")
    );

    // Every summary is a projection of the record the read answers from.
    for (summary, id) in summaries(list).iter().zip(&expected) {
        let read = get_policy(&router, id).await;
        assert_eq!(
            read.status, 200,
            "case every-policy: expected HTTP 200 reading {id}, got {} with {}",
            read.status, read.raw
        );
        let summarised = summary
            .at("ContinuousDeploymentPolicy")
            .unwrap_or_else(|| panic!("case every-policy: summary of {id} carries no policy"));
        assert_eq!(
            summarised.text_at("LastModifiedTime"),
            read.root().text_at("LastModifiedTime"),
            "case every-policy: summary {id} must report the record's LastModifiedTime ({:?}), \
             got {:?}",
            read.root().text_at("LastModifiedTime"),
            summarised.text_at("LastModifiedTime")
        );
        let case = created
            .iter()
            .find(|(_, created_id)| created_id == id)
            .map(|(case, _)| *case)
            .unwrap_or("unknown");
        let policy = table
            .iter()
            .find(|policy| policy.case == case)
            .expect("every created policy comes from the table");
        policy.assert_matches(
            summarised
                .at("ContinuousDeploymentPolicyConfig")
                .unwrap_or_else(|| panic!("case {case}: summary carries no config")),
            "a list summary",
        );
    }

    // A list is a READ: it changed nothing a later caller sees.
    let again = summary_ids(list_policies(&router, "").await.root());
    assert_eq!(
        again, listed,
        "case repeated: listing twice must report the same policies in the same order \
         ({listed:?}), got {again:?}"
    );
}

/// Walking the pages at any admissible page size enumerates every policy exactly once, in
/// id order, with `NextMarker` present exactly when policies remain and `Quantity`
/// reporting the whole account on every page.
#[tokio::test]
async fn walking_the_pages_enumerates_every_policy_once() {
    let router = emulator();
    // More policies than the smallest page sizes below, each with its own staging
    // distribution.
    let domains = staging_domains(&router, "page", 7).await;
    let policies: Vec<Policy> = (0..7)
        .map(|index| Policy {
            case: "pagination",
            dns_names: vec![domains[index].clone()],
            enabled: index % 2 == 0,
            traffic: match index % 3 {
                0 => Traffic::None,
                1 => Traffic::SingleWeight {
                    weight: 0.05,
                    stickiness: Some((300, 900)),
                },
                _ => Traffic::SingleHeader {
                    header: "aws-cf-cd-wave".to_string(),
                    value: format!("wave-{index}"),
                },
            },
        })
        .collect();
    let created = create_all(&router, &policies).await;
    let total = created.len();
    let expected: Vec<String> = {
        let mut ids: Vec<String> = created.iter().map(|(_, id)| id.clone()).collect();
        ids.sort();
        ids
    };

    // Page sizes spanning what the rule generalises over: none at all, one at a time,
    // two, exactly the collection size, one past it, and the service's own ceiling.
    for page_size in [None, Some(1), Some(2), Some(3), Some(7), Some(8), Some(MAX_ITEMS)] {
        let label = page_size
            .map(|size| format!("MaxItems={size}"))
            .unwrap_or_else(|| "no-MaxItems".to_string());
        let mut marker: Option<String> = None;
        let mut collected: Vec<String> = Vec::new();

        for page in 0.. {
            let mut query = String::new();
            if let Some(size) = page_size {
                query.push_str(&format!("?MaxItems={size}"));
            }
            if let Some(resume) = marker.as_deref() {
                query.push_str(if query.is_empty() { "?" } else { "&" });
                query.push_str(&format!("Marker={resume}"));
            }
            let answer = list_policies(&router, &query).await;
            assert_eq!(
                answer.status, 200,
                "case {label} page {page}: expected HTTP 200, got {} with {}",
                answer.status, answer.raw
            );
            let list = answer.root();
            assert_eq!(
                list.text_at("MaxItems"),
                Some(page_size.unwrap_or(MAX_ITEMS).to_string().as_str()),
                "case {label} page {page}: expected MaxItems to echo the page size, got {:?}",
                list.text_at("MaxItems")
            );
            assert_eq!(
                list.text_at("Quantity"),
                Some(total.to_string().as_str()),
                "case {label} page {page}: expected Quantity {total} (the whole account), \
                 got {:?}",
                list.text_at("Quantity")
            );
            let ids = summary_ids(list);
            if let Some(size) = page_size {
                assert!(
                    i32::try_from(ids.len()).unwrap_or(i32::MAX) <= size,
                    "case {label} page {page}: a page holds at most {size} summaries, got {}",
                    ids.len()
                );
            }
            collected.extend(ids);

            match list.text_at("NextMarker").map(str::to_string) {
                Some(next) => {
                    assert_eq!(
                        Some(next.as_str()),
                        collected.last().map(String::as_str),
                        "case {label} page {page}: NextMarker must name the id this page ended \
                         at ({:?}), got {next:?}",
                        collected.last()
                    );
                    marker = Some(next);
                }
                None => break,
            }
        }

        assert_eq!(
            collected, expected,
            "case {label}: walking the pages must enumerate every policy exactly once, in id \
             order ({expected:?}), got {collected:?}"
        );
    }
}

/// The refusals the listing models: a page size that names no page, and a `Marker` that
/// is no resume point this service handed out.
#[tokio::test]
async fn unusable_list_arguments_are_refused() {
    let router = emulator();
    let table = staged(&router, "listargs", policy_table()).await;
    create_all(&router, &table[..2]).await;

    // Rows: a page size that is no page, one that is no integer, and markers of every
    // shape that is not a policy id this listing issued.
    for (case, query) in [
        ("a negative page size", "?MaxItems=-3"),
        ("a page size that is no integer", "?MaxItems=not-a-number"),
        ("a marker that is no policy id", "?Marker=not-an-id"),
        (
            "a marker of another resource's id shape",
            "?Marker=E1111111111111",
        ),
        (
            "an unusable marker beside a usable page size",
            "?MaxItems=2&Marker=not-an-id",
        ),
    ] {
        let answer = list_policies(&router, query).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400 for {query}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument, got {}",
            answer.raw
        );
    }

    // A marker shaped like a policy id that no policy holds names a policy that does not
    // exist — the one resource this listing can be about.
    for (case, marker) in [
        ("a UUID never minted", "00000000-0000-0000-0000-000000000000"),
        ("another UUID never minted", "ffffffff-ffff-ffff-ffff-ffffffffffff"),
    ] {
        let answer = list_policies(&router, &format!("?Marker={marker}")).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404 for {marker}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchContinuousDeploymentPolicy",
            "case {case}: expected NoSuchContinuousDeploymentPolicy, got {}",
            answer.raw
        );
    }
}

/// A configuration outside what the model and the documentation allow is refused with the
/// error the operation models for it, and nothing is stored for it.
#[tokio::test]
async fn configurations_outside_the_documented_constraints_are_refused() {
    let router = emulator();

    // Each row: what it is, the document, the status and the wire code. The quantity row
    // states a `Quantity` that disagrees with its `Items`; the rest sit one step outside
    // a published bound, or name a `Type` whose configuration is missing. Every one of them
    // is refused on the member it breaks, before the staging distribution domain names are
    // resolved, so their domain names stay placeholders.
    let base = Policy {
        case: "refusals",
        dns_names: vec!["d777777abcdef8.cloudfront.net".to_string()],
        enabled: true,
        traffic: Traffic::None,
    };
    let weight_only_type = format!(
        "<ContinuousDeploymentPolicyConfig \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <StagingDistributionDnsNames><Quantity>1</Quantity><Items>\
         <DnsName>d888888abcdef8.cloudfront.net</DnsName></Items>\
         </StagingDistributionDnsNames><Enabled>true</Enabled>\
         <TrafficConfig><Type>SingleWeight</Type></TrafficConfig>\
         </ContinuousDeploymentPolicyConfig>"
    );
    let header_only_type = weight_only_type.replace("SingleWeight", "SingleHeader");
    let unknown_type = weight_only_type.replace("SingleWeight", "SingleStripe");

    let rows: Vec<(&str, String, u16, &str)> = vec![
        (
            "a Quantity above the domain names carried",
            base.xml_with_quantity(2),
            400,
            "InconsistentQuantities",
        ),
        (
            "a Quantity below the domain names carried",
            base.xml_with_quantity(0),
            400,
            "InconsistentQuantities",
        ),
        (
            "Type SingleWeight without its configuration",
            weight_only_type.clone(),
            400,
            "InvalidArgument",
        ),
        (
            "Type SingleHeader without its configuration",
            header_only_type,
            400,
            "InvalidArgument",
        ),
        (
            // The request boundary refuses a value outside `ContinuousDeploymentPolicyType`
            // before the handler runs, so this row pins the MODELLED constraint rather
            // than the handler's own fallback for it — under the code this service answers
            // a request whose published schema it cannot read, which the value set an enum
            // declares is part of.
            "a Type the enum does not admit",
            unknown_type,
            400,
            "MalformedXML",
        ),
        (
            "a weight one step past the published maximum",
            Policy {
                traffic: Traffic::SingleWeight {
                    weight: 0.16,
                    stickiness: None,
                },
                ..base.clone()
            }
            .xml(),
            400,
            "InvalidArgument",
        ),
        (
            "a negative weight",
            Policy {
                traffic: Traffic::SingleWeight {
                    weight: -0.01,
                    stickiness: None,
                },
                ..base.clone()
            }
            .xml(),
            400,
            "InvalidArgument",
        ),
        (
            "an IdleTTL below the published minimum",
            Policy {
                traffic: Traffic::SingleWeight {
                    weight: 0.1,
                    stickiness: Some((MIN_SESSION_STICKINESS_TTL - 1, 900)),
                },
                ..base.clone()
            }
            .xml(),
            400,
            "InvalidArgument",
        ),
        (
            "a MaximumTTL above the published maximum",
            Policy {
                traffic: Traffic::SingleWeight {
                    weight: 0.1,
                    stickiness: Some((900, MAX_SESSION_STICKINESS_TTL + 1)),
                },
                ..base.clone()
            }
            .xml(),
            400,
            "InvalidArgument",
        ),
        (
            "an IdleTTL past its MaximumTTL",
            Policy {
                traffic: Traffic::SingleWeight {
                    weight: 0.1,
                    stickiness: Some((1_200, 600)),
                },
                ..base.clone()
            }
            .xml(),
            400,
            "InvalidArgument",
        ),
        (
            "a single-header name without the documented prefix",
            Policy {
                traffic: Traffic::SingleHeader {
                    header: "x-staging".to_string(),
                    value: "true".to_string(),
                },
                ..base.clone()
            }
            .xml(),
            400,
            "InvalidArgument",
        ),
        (
            // One name past this emulator's bound on the only caller-chosen LENGTH the
            // configuration carries, so the list it stores and later scans cannot grow
            // with a number the request picks. (An overstated `Quantity` is refused by
            // the `InconsistentQuantities` rows above, which run first, so a request
            // cannot walk around the bound by understating its list.)
            "one staging domain name past the server's bound",
            Policy {
                dns_names: (0..=MAX_STAGING_DISTRIBUTION_DNS_NAMES)
                    .map(|index| format!("dpast{index:02}abcdef8.cloudfront.net"))
                    .collect(),
                ..base.clone()
            }
            .xml(),
            400,
            "InvalidArgument",
        ),
    ];

    for (case, body, status, code) in rows {
        let answer = create_policy(&router, &body).await;
        assert_eq!(
            answer.status, status,
            "case {case}: expected HTTP {status}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {case}: expected {code}, got {}",
            answer.raw
        );
        // A refused create stores nothing.
        let listed = list_policies(&router, "").await;
        assert_eq!(
            listed.root().text_at("Quantity"),
            Some("0"),
            "case {case}: the refused create must store nothing, got {}",
            listed.raw
        );
    }

    // The same configurations AT the published bounds are accepted, so each refusal above
    // is about the bound rather than about the member. Each one names staging distributions
    // this service really holds, which is what a stored policy's domain names are.
    let bound_domains = staging_domains(&router, "bounds", 2).await;
    let at_the_bound_domains =
        staging_domains(&router, "atbound", MAX_STAGING_DISTRIBUTION_DNS_NAMES).await;
    for (case, policy) in [
        (
            "the lower weight bound",
            Policy {
                dns_names: vec![bound_domains[0].clone()],
                traffic: Traffic::SingleWeight {
                    weight: 0.0,
                    stickiness: Some((MIN_SESSION_STICKINESS_TTL, MAX_SESSION_STICKINESS_TTL)),
                },
                ..base.clone()
            },
        ),
        (
            "the upper weight bound",
            Policy {
                dns_names: vec![bound_domains[1].clone()],
                traffic: Traffic::SingleWeight {
                    weight: MAX_TRAFFIC_WEIGHT,
                    stickiness: Some((MAX_SESSION_STICKINESS_TTL, MAX_SESSION_STICKINESS_TTL)),
                },
                ..base.clone()
            },
        ),
        (
            "exactly as many staging domain names as the server's bound allows",
            Policy {
                dns_names: at_the_bound_domains.clone(),
                traffic: Traffic::None,
                ..base.clone()
            },
        ),
    ] {
        let answer = create_policy(&router, &policy.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected HTTP 201 at the bound, got {} with {}",
            answer.status, answer.raw
        );
    }
}

/// "A continuous deployment policy for this staging distribution already exists": a
/// staging distribution takes part in at most ONE policy, and a repeat of a stored
/// configuration is the policy that already holds it. Either refusal leaves the stored
/// policy exactly as it was.
#[tokio::test]
async fn a_staging_distribution_takes_part_in_at_most_one_policy() {
    let router = emulator();
    // Three staging distributions: the two the stored policy holds, and one no policy does.
    let domains = staging_domains(&router, "inuse", 3).await;
    let first = Policy {
        case: "the policy that holds the staging distribution",
        dns_names: vec![domains[0].clone(), domains[1].clone()],
        enabled: true,
        traffic: Traffic::SingleWeight {
            weight: 0.1,
            stickiness: None,
        },
    };
    let created = create_policy(&router, &first.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201, got {} with {}",
        first.case, created.status, created.raw
    );
    let id = created
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();
    let etag = created.header("etag").to_string();

    // Rows: the very same configuration again, and configurations that differ but still
    // name one of the staging distribution domain names the stored policy holds.
    let conflicts: Vec<(&str, Policy, &str)> = vec![
        (
            "the same configuration again",
            first.clone(),
            "ContinuousDeploymentPolicyAlreadyExists",
        ),
        (
            "a different configuration over the same staging domains",
            Policy {
                enabled: false,
                traffic: Traffic::None,
                ..first.clone()
            },
            "StagingDistributionInUse",
        ),
        (
            "one of the staging domains, beside a new one",
            Policy {
                dns_names: vec![domains[1].clone(), domains[2].clone()],
                ..first.clone()
            },
            "StagingDistributionInUse",
        ),
    ];

    for (case, policy, code) in conflicts {
        let answer = create_policy(&router, &policy.xml()).await;
        assert_eq!(
            answer.status, 409,
            "case {case}: expected HTTP 409, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {case}: expected {code}, got {}",
            answer.raw
        );

        // The store still holds exactly the policy the first create landed.
        let listed = list_policies(&router, "").await;
        assert_eq!(
            listed.root().text_at("Quantity"),
            Some("1"),
            "case {case}: the refused create must store nothing, got {}",
            listed.raw
        );
        let read = get_policy(&router, &id).await;
        assert_eq!(
            read.header("etag"),
            etag,
            "case {case}: the refused create must not spend a version token, got {:?}",
            read.header("etag")
        );
        first.assert_matches(
            read.root()
                .at("ContinuousDeploymentPolicyConfig")
                .unwrap_or_else(|| panic!("case {case}: the read carries no config")),
            "after a refusal",
        );
    }

    // A policy over a staging distribution no policy holds still lands.
    let other = Policy {
        case: "another staging distribution",
        dns_names: vec![staging_domain(&router, "ref-inuse-free").await],
        enabled: true,
        traffic: Traffic::SingleWeight {
            weight: 0.1,
            stickiness: None,
        },
    };
    let answer = create_policy(&router, &other.xml()).await;
    assert_eq!(
        answer.status, 201,
        "case {}: expected HTTP 201, got {} with {}",
        other.case, answer.status, answer.raw
    );
}

/// The account holds at most `MAX_CONTINUOUS_DEPLOYMENT_POLICIES` policies: the create at
/// the bound is refused with `TooManyContinuousDeploymentPolicies` (HTTP 400), nothing is
/// stored for it, and every policy that did land is still listed.
#[tokio::test]
async fn an_account_holds_at_most_the_bound_many_policies() {
    let router = emulator();

    // One policy per slot the bound allows, each over its own staging distribution.
    let domains = staging_domains(&router, "fill", MAX_CONTINUOUS_DEPLOYMENT_POLICIES).await;
    for (index, domain) in domains.iter().enumerate() {
        let policy = Policy {
            case: "filling the account",
            dns_names: vec![domain.clone()],
            enabled: index % 2 == 0,
            traffic: Traffic::None,
        };
        let answer = create_policy(&router, &policy.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case policy {index} of {MAX_CONTINUOUS_DEPLOYMENT_POLICIES}: expected HTTP 201, \
             got {} with {}",
            answer.status, answer.raw
        );
    }

    let listed = list_policies(&router, "").await;
    assert_eq!(
        listed.root().text_at("Quantity"),
        Some(MAX_CONTINUOUS_DEPLOYMENT_POLICIES.to_string().as_str()),
        "case at the bound: expected the account to hold {MAX_CONTINUOUS_DEPLOYMENT_POLICIES} \
         policies, got {:?}",
        listed.root().text_at("Quantity")
    );

    // One more, over a staging distribution no policy holds, so only the bound can refuse
    // it.
    let over = Policy {
        case: "one policy past the bound",
        dns_names: vec![staging_domain(&router, "ref-fill-over").await],
        enabled: true,
        traffic: Traffic::None,
    };
    let answer = create_policy(&router, &over.xml()).await;
    assert_eq!(
        answer.status, 400,
        "case {}: expected HTTP 400, got {} with {}",
        over.case, answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        "TooManyContinuousDeploymentPolicies",
        "case {}: expected TooManyContinuousDeploymentPolicies, got {}",
        over.case,
        answer.raw
    );

    // The refused create stored nothing, and every policy that landed is still there.
    let after = list_policies(&router, &format!("?MaxItems={MAX_CONTINUOUS_DEPLOYMENT_POLICIES}"))
        .await;
    assert_eq!(
        after.root().text_at("Quantity"),
        Some(MAX_CONTINUOUS_DEPLOYMENT_POLICIES.to_string().as_str()),
        "case {}: the refused create must store nothing, got {}",
        over.case,
        after.raw
    );
    assert_eq!(
        summary_ids(after.root()).len(),
        MAX_CONTINUOUS_DEPLOYMENT_POLICIES,
        "case {}: expected every stored policy to still be listed, got {}",
        over.case,
        after.raw
    );
}
