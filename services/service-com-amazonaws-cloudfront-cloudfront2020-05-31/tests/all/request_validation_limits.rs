// SPDX-License-Identifier: CC0-1.0

//! What CloudFront REFUSES before it stores a resource, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test, each asserted over a SET of inputs rather than one:
//!
//! * a cache policy and an origin request policy each name at most
//!   [`MAX_HEADERS_IN_POLICY`] headers, [`MAX_COOKIES_IN_POLICY`] cookies and
//!   [`MAX_QUERY_STRINGS_IN_POLICY`] query strings, so a list past its published quota is
//!   refused with the error that policy kind models for THAT list — on the create and on
//!   the update, since the list is stored whole;
//! * a continuous deployment policy routes traffic to a staging DISTRIBUTION of this
//!   account, named by the domain name CloudFront assigned it, so a
//!   `StagingDistributionDnsNames` entry no distribution of the account is served on is an
//!   invalid argument;
//! * a field-level encryption configuration encrypts a request's fields under the profile
//!   its query argument maps to or the one a content type mapping names, so a
//!   configuration where neither names a profile is refused, while one where either does
//!   lands; and an UPDATE has to state the `QueryArgProfileConfig` at all;
//! * an invalidation path is relative to the distribution and begins with a slash, and
//!   carries no character the documentation requires URL-encoded — a batch naming a value
//!   that breaks either rule, or naming no path at all, is an invalid argument;
//! * a field-level encryption profile, a public key and an origin access control are each
//!   unique by NAME, so a second create under a name the account holds is that kind's
//!   conflict and leaves the resource holding the name exactly as it was.
//!
//! The refusal sentences are LITERALS below rather than the crate's own constants, so a
//! reworded refusal fails this suite. The harness — the router, the request helpers and
//! each resource kind's document builder — is the one the per-resource suites use, reused
//! rather than copied.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    MAX_COOKIES_IN_POLICY, MAX_HEADERS_IN_POLICY, MAX_QUERY_STRINGS_IN_POLICY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cache_policy_lifecycle::{
    create_policy as create_cache_policy, get_policy as get_cache_policy, Parameters,
    Policy as CachePolicy, DOCUMENTED_DEFAULT_TTL, DOCUMENTED_MAX_TTL,
};
use crate::cache_policy_update::update_policy as update_cache_policy;
use crate::continuous_deployment_policies::{
    create_policy as create_deployment_policy, get_policy as get_deployment_policy,
    staging_domain, Policy as DeploymentPolicy, Traffic,
};
use crate::continuous_deployment_policy_update::update_policy as update_deployment_policy;
use crate::distribution_lifecycle::{
    batch_xml, call, config as distribution_config, create_distribution, create_invalidation,
    emulator, Answer, API_VERSION,
};
use crate::field_level_encryption_config_lifecycle::{
    create_config, get_config, Config, ContentTypeMapping, QueryArgMapping, Section,
};
use crate::field_level_encryption_config_update::update_config;
use crate::field_level_encryption_profile_lifecycle::{
    create_profile, get_profile, store_profile, upload_keys, Entity, Profile,
};
use crate::origin_access_control_lifecycle::{
    create_control, get_control, table as control_table,
};
use crate::origin_request_policy_lifecycle::{
    create_policy as create_origin_request_policy, get_policy as get_origin_request_policy,
    Policy as OriginRequestPolicy,
};
use crate::public_key_lifecycle::{create_key, get_key, pem, Key};

// ---------------------------------------------------------------------------
// The per-policy list quotas
// ---------------------------------------------------------------------------

/// One of the three caller-chosen lists a cache policy and an origin request policy each
/// carry, with the quota CloudFront publishes for it and the refusal each policy kind
/// answers a list past that quota with.
struct ListCase {
    /// The wrapper the entries travel in, named in every assertion.
    list: &'static str,
    /// What one entry of this list is, used to build the names a row submits.
    entry: &'static str,
    quota: usize,
    cache_policy_code: &'static str,
    cache_policy_message: &'static str,
    origin_request_policy_code: &'static str,
    origin_request_policy_message: &'static str,
}

/// Every list under test, with the quotas the Amazon CloudFront Developer Guide's "Quotas"
/// publishes for the cache-key and origin-request settings (10 headers, 10 cookies and 10
/// query strings per policy, for each of the two policy kinds).
fn list_cases() -> Vec<ListCase> {
    vec![
        ListCase {
            list: "Headers",
            entry: "x-case-header",
            quota: MAX_HEADERS_IN_POLICY,
            cache_policy_code: "TooManyHeadersInCachePolicy",
            cache_policy_message: "Your request contains more headers in the Cache Policy than \
                                   are allowed.",
            origin_request_policy_code: "TooManyHeadersInOriginRequestPolicy",
            origin_request_policy_message: "Your request contains more headers in Origin Request \
                                            Policy than are allowed.",
        },
        ListCase {
            list: "Cookies",
            entry: "case-cookie",
            quota: MAX_COOKIES_IN_POLICY,
            cache_policy_code: "TooManyCookiesInCachePolicy",
            cache_policy_message: "Your request contains more cookies in the Cache Policy than \
                                   are allowed.",
            origin_request_policy_code: "TooManyCookiesInOriginRequestPolicy",
            origin_request_policy_message: "Your request contains more cookies in Origin Request \
                                            Policy than are allowed.",
        },
        ListCase {
            list: "QueryStrings",
            entry: "case-arg",
            quota: MAX_QUERY_STRINGS_IN_POLICY,
            cache_policy_code: "TooManyQueryStringsInCachePolicy",
            cache_policy_message: "Your request contains more query strings in the Cache Policy \
                                   than are allowed.",
            origin_request_policy_code: "TooManyQueryStringsInOriginRequestPolicy",
            origin_request_policy_message: "Your request contains more query strings in Origin \
                                            Request Policy than are allowed.",
        },
    ]
}

/// `count` distinct entries for the list `case` names.
fn entries(case: &ListCase, count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("{}-{index}", case.entry))
        .collect()
}

/// A cache policy configuration under `name` whose `case` list carries `values` and whose
/// other two lists carry nothing.
fn cache_policy(case: &ListCase, name: &str, values: Vec<String>) -> CachePolicy {
    let mut parameters = Parameters::default();
    match case.list {
        "Headers" => {
            parameters.header_behavior = "whitelist".to_string();
            parameters.headers = Some(values);
        }
        "Cookies" => {
            parameters.cookie_behavior = "whitelist".to_string();
            parameters.cookies = Some(values);
        }
        _ => {
            parameters.query_string_behavior = "whitelist".to_string();
            parameters.query_strings = Some(values);
        }
    }
    CachePolicy {
        case: "a cache policy whose list is driven to its quota",
        name: name.to_string(),
        comment: None,
        min_ttl: 1,
        default_ttl: Some(DOCUMENTED_DEFAULT_TTL),
        max_ttl: Some(DOCUMENTED_MAX_TTL),
        parameters: Some(parameters),
    }
}

/// An origin request policy configuration under `name` whose `case` list carries `values`.
fn origin_request_policy(case: &ListCase, name: &str, values: Vec<String>) -> OriginRequestPolicy {
    let mut policy = OriginRequestPolicy {
        case: "an origin request policy whose list is driven to its quota",
        name: name.to_string(),
        ..OriginRequestPolicy::default()
    };
    match case.list {
        "Headers" => {
            policy.header_behavior = "whitelist".to_string();
            policy.headers = Some(values);
        }
        "Cookies" => {
            policy.cookie_behavior = "whitelist".to_string();
            policy.cookies = Some(values);
        }
        _ => {
            policy.query_string_behavior = "whitelist".to_string();
            policy.query_strings = Some(values);
        }
    }
    policy
}

/// How many entries the `wrapper` of a serialised policy document reports.
fn carried(answer: &Answer, wrapper: &str) -> usize {
    answer
        .root()
        .descendant(wrapper)
        .and_then(|node| node.at("Items"))
        .map_or(0, |items| items.children.len())
}

/// A list ON its published quota lands and is reported back, and one PAST it is refused
/// with the error the policy kind models for that list — for both policy kinds, over all
/// three lists and over the counts a caller may submit.
#[tokio::test]
async fn a_policy_list_past_its_published_quota_is_refused() {
    let router = emulator();

    for case in list_cases() {
        // The counts span what the rule generalises over: well inside the quota, exactly on
        // it, one past it and far past it.
        for (description, count, expected) in [
            ("inside the quota", 1_usize, 201_u16),
            ("on the quota", case.quota, 201),
            ("one past the quota", case.quota + 1, 400),
            ("far past the quota", case.quota * 3, 400),
        ] {
            let values = entries(&case, count);
            let name = format!("cache-{}-{count}", case.list.to_lowercase());
            let row = cache_policy(&case, &name, values.clone());
            let label = format!("a cache policy with {count} {} ({description})", case.list);
            let answer = create_cache_policy(&router, &row.xml()).await;
            assert_eq!(
                answer.status, expected,
                "case {label}: expected HTTP {expected}, got {} with {}",
                answer.status, answer.raw
            );
            if expected == 201 {
                // The accepted policy is readable afterwards, carrying the list it
                // submitted — so the bound admits what it reports.
                let id = answer.root().text_at("Id").unwrap_or_default().to_string();
                let read = get_cache_policy(&router, &id).await;
                assert_eq!(
                    read.status, 200,
                    "case {label}: expected HTTP 200 from GetCachePolicy, got {} with {}",
                    read.status, read.raw
                );
                assert_eq!(
                    carried(&read, case.list),
                    count,
                    "case {label}: expected the stored policy to carry {count} entries, got {}",
                    read.raw
                );
            } else {
                assert_eq!(
                    answer.error_code(),
                    case.cache_policy_code,
                    "case {label}: expected {}, got {:?} with {}",
                    case.cache_policy_code,
                    answer.error_code(),
                    answer.raw
                );
                assert_eq!(
                    answer.root().find_text("Message").unwrap_or_default(),
                    case.cache_policy_message,
                    "case {label}: expected the service's sentence for that quota, got {}",
                    answer.raw
                );
            }

            let name = format!("origin-{}-{count}", case.list.to_lowercase());
            let row = origin_request_policy(&case, &name, values);
            let label = format!(
                "an origin request policy with {count} {} ({description})",
                case.list
            );
            let answer = create_origin_request_policy(&router, &row.xml()).await;
            assert_eq!(
                answer.status, expected,
                "case {label}: expected HTTP {expected}, got {} with {}",
                answer.status, answer.raw
            );
            if expected == 201 {
                let id = answer.root().text_at("Id").unwrap_or_default().to_string();
                let read = get_origin_request_policy(&router, &id).await;
                assert_eq!(
                    read.status, 200,
                    "case {label}: expected HTTP 200 from GetOriginRequestPolicy, got {} with {}",
                    read.status, read.raw
                );
                assert_eq!(
                    carried(&read, case.list),
                    count,
                    "case {label}: expected the stored policy to carry {count} entries, got {}",
                    read.raw
                );
            } else {
                assert_eq!(
                    answer.error_code(),
                    case.origin_request_policy_code,
                    "case {label}: expected {}, got {:?} with {}",
                    case.origin_request_policy_code,
                    answer.error_code(),
                    answer.raw
                );
                assert_eq!(
                    answer.root().find_text("Message").unwrap_or_default(),
                    case.origin_request_policy_message,
                    "case {label}: expected the service's sentence for that quota, got {}",
                    answer.raw
                );
            }
        }
    }
}

/// The same quota holds on an UPDATE's payload: a list past it is refused with the same
/// error, and the stored policy keeps the list it had.
#[tokio::test]
async fn an_update_whose_policy_list_passes_its_quota_is_refused() {
    let router = emulator();

    for case in list_cases() {
        let name = format!("update-{}", case.list.to_lowercase());
        let stored = cache_policy(&case, &name, entries(&case, case.quota));
        let created = create_cache_policy(&router, &stored.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {}: expected HTTP 201 seeding the policy, got {} with {}",
            case.list, created.status, created.raw
        );
        let id = created.root().text_at("Id").unwrap_or_default().to_string();
        let etag = created.header("etag").to_string();

        let past = cache_policy(&case, &name, entries(&case, case.quota + 1));
        let refused = update_cache_policy(&router, &id, &past.xml(), Some(&etag)).await;
        assert_eq!(
            refused.status, 400,
            "case {}: expected HTTP 400 from the update past the quota, got {} with {}",
            case.list, refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            case.cache_policy_code,
            "case {}: expected {}, got {:?} with {}",
            case.list,
            case.cache_policy_code,
            refused.error_code(),
            refused.raw
        );

        // The refused update wrote nothing: the stored policy still carries the list its
        // create stored, at the version token that create minted.
        let read = get_cache_policy(&router, &id).await;
        assert_eq!(
            read.header("etag"),
            etag,
            "case {}: expected the refused update to leave the version token, got {:?}",
            case.list,
            read.header("etag")
        );
        assert_eq!(
            carried(&read, case.list),
            case.quota,
            "case {}: expected the stored policy to carry {} entries, got {}",
            case.list,
            case.quota,
            read.raw
        );
    }
}

// ---------------------------------------------------------------------------
// A staging distribution's domain name
// ---------------------------------------------------------------------------

/// The sentence CloudFront refuses a staging distribution domain name of its own with.
const FOREIGN_STAGING_DOMAIN_MESSAGE: &str =
    "The parameter StagingDistributionDnsNames is not a default CloudFront domain name.";

/// The sentence CloudFront refuses a `StagingDistributionDnsNames` carrying no name with.
const EMPTY_STAGING_DOMAIN_MESSAGE: &str =
    "The parameter StagingDistributionDnsNames is null or empty.";

/// The domain names a continuous deployment policy is driven over: the form CloudFront
/// assigns a distribution (`StagingDistributionDnsNames` documents it as "the CloudFront
/// domain name of the staging distribution. For example: `d111111abcdef8.cloudfront.net`"),
/// and the ways a value can fail to be one.
/// The staging distribution domain names a continuous deployment policy is driven over.
///
/// A policy names its staging distribution by the domain name CloudFront ASSIGNED it
/// (`StagingDistributionDnsNames` documents it as "the CloudFront domain name of the staging
/// distribution. For example: `d111111abcdef8.cloudfront.net`"), so the one admitted row is
/// the domain of a distribution this account holds — created by the case itself — and the
/// rest are the ways a value can fail to name one.
fn unassigned_staging_domains() -> Vec<(&'static str, &'static str)> {
    vec![
        ("a domain of another zone", "staging.example.com"),
        (
            "a name in the assigned shape that was never assigned here",
            "d111111abcdef8.cloudfront.net",
        ),
        (
            "a label the service does not assign, under the zone",
            "staging.cloudfront.net",
        ),
        ("the zone with nothing under it", "cloudfront.net"),
        (
            "an assigned name with the zone as a prefix of a longer one",
            "d111111abcdef8.cloudfront.net.example.com",
        ),
        (
            "more than one label under the zone",
            "staging.d111111abcdef8.cloudfront.net",
        ),
        ("an empty label under the zone", ".cloudfront.net"),
        ("no domain name at all", ""),
    ]
}

/// One continuous deployment policy configuration routing traffic to `domain`.
fn deployment_policy(domain: &str) -> DeploymentPolicy {
    DeploymentPolicy {
        case: "a policy routing traffic to one staging distribution",
        dns_names: vec![domain.to_string()],
        enabled: true,
        traffic: Traffic::SingleWeight {
            weight: 0.1,
            stickiness: None,
        },
    }
}

/// A `StagingDistributionDnsNames` entry that no distribution of this account is served on
/// is refused, on the create and on the update, while the domain of a distribution this
/// account holds lands.
#[tokio::test]
async fn a_staging_distribution_domain_this_account_does_not_hold_is_refused() {
    let router = emulator();

    // The control: a distribution this account really holds, named by the domain CloudFront
    // assigned it, is the staging distribution a policy may point at.
    let assigned = staging_domain(&router, "ref-limits-staging").await;
    let row = deployment_policy(&assigned);
    let answer = create_deployment_policy(&router, &row.xml()).await;
    assert_eq!(
        answer.status, 201,
        "case the domain of a distribution this account holds ({assigned:?}): expected HTTP \
         201, got {} with {}",
        answer.status, answer.raw
    );
    let id = answer.root().text_at("Id").unwrap_or_default().to_string();
    let etag = answer.header("etag").to_string();
    let read = get_deployment_policy(&router, &id).await;
    assert_eq!(
        read.status, 200,
        "case the stored policy: expected HTTP 200 from GetContinuousDeploymentPolicy, got {} \
         with {}",
        read.status, read.raw
    );
    assert_eq!(
        read.root().text_at(
            "ContinuousDeploymentPolicyConfig/StagingDistributionDnsNames/Items/DnsName"
        ),
        Some(assigned.as_str()),
        "case the stored policy: expected it to report the submitted domain, got {}",
        read.raw
    );

    for (description, domain) in unassigned_staging_domains() {
        let refused =
            create_deployment_policy(&router, &deployment_policy(domain).xml()).await;
        assert_eq!(
            refused.status, 400,
            "case {description} ({domain:?}): expected HTTP 400, got {} with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "InvalidArgument",
            "case {description} ({domain:?}): expected InvalidArgument, got {:?} with {}",
            refused.error_code(),
            refused.raw
        );
        assert_eq!(
            refused.root().find_text("Message").unwrap_or_default(),
            FOREIGN_STAGING_DOMAIN_MESSAGE,
            "case {description} ({domain:?}): expected the service's sentence, got {}",
            refused.raw
        );

        // The same name on an UPDATE's payload is refused by the same rule, and the stored
        // policy keeps the staging distribution and the version token it had.
        let moved = update_deployment_policy(
            &router,
            &id,
            &deployment_policy(domain).xml(),
            Some(&etag),
        )
        .await;
        assert_eq!(
            moved.status, 400,
            "case {description} ({domain:?}): expected HTTP 400 from the update, got {} with {}",
            moved.status, moved.raw
        );
        assert_eq!(
            moved.root().find_text("Message").unwrap_or_default(),
            FOREIGN_STAGING_DOMAIN_MESSAGE,
            "case {description} ({domain:?}): expected the service's sentence from the update, \
             got {}",
            moved.raw
        );
        let after = get_deployment_policy(&router, &id).await;
        assert_eq!(
            after.header("etag"),
            etag,
            "case {description} ({domain:?}): expected the refused update to leave the version \
             token, got {:?}",
            after.header("etag")
        );
    }

    // A `StagingDistributionDnsNames` that carries no name at all names no staging
    // distribution either, and the service says which member it is — on the create and on
    // the update.
    let mut empty = deployment_policy(&assigned);
    empty.dns_names = Vec::new();
    for (description, answer) in [
        (
            "the create",
            create_deployment_policy(&router, &empty.xml()).await,
        ),
        (
            "the update",
            update_deployment_policy(&router, &id, &empty.xml(), Some(&etag)).await,
        ),
    ] {
        assert_eq!(
            answer.status, 400,
            "case a StagingDistributionDnsNames carrying no name ({description}): expected HTTP \
             400, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case a StagingDistributionDnsNames carrying no name ({description}): expected \
             InvalidArgument, got {:?} with {}",
            answer.error_code(),
            answer.raw
        );
        assert_eq!(
            answer.root().find_text("Message").unwrap_or_default(),
            EMPTY_STAGING_DOMAIN_MESSAGE,
            "case a StagingDistributionDnsNames carrying no name ({description}): expected the \
             service's sentence, got {}",
            answer.raw
        );
    }
}

// ---------------------------------------------------------------------------
// A field-level encryption configuration's profiles
// ---------------------------------------------------------------------------

/// The sentence CloudFront refuses a configuration that names no profile to encrypt with.
const QUERY_ARG_PROFILE_EMPTY_MESSAGE: &str =
    "The query arg profile in field level encryption config cannot be empty when content type \
     profile is empty or when no default profile id is given in any content type profile.";

/// The sentence CloudFront refuses an update that states no `QueryArgProfileConfig` with.
const QUERY_ARG_PROFILE_CONFIG_REQUIRED_MESSAGE: &str =
    "QueryArgProfileConfig is required for updates";

/// A configuration naming a profile in either section lands, and one naming none in either
/// is refused — over the forms each section takes (absent, an absent wrapper, an empty
/// list, a mapping that names a profile and one that does not).
#[tokio::test]
async fn a_configuration_naming_no_profile_to_encrypt_with_is_refused() {
    let router = emulator();
    let profile = store_profile(&router, "limits-config-profile").await;

    let rows: Vec<(Config, bool)> = vec![
        (
            Config::new(
                "a query argument mapping naming the profile",
                "limits-config-query-arg",
                None,
                Some(Section::new(
                    true,
                    Some(vec![QueryArgMapping::new("fle-profile", &profile)]),
                )),
                None,
            ),
            true,
        ),
        (
            Config::new(
                "a content type mapping naming the profile",
                "limits-config-content-type",
                None,
                None,
                Some(Section::new(
                    true,
                    Some(vec![ContentTypeMapping::new("text/html", Some(&profile))]),
                )),
            ),
            true,
        ),
        (
            Config::new(
                "an empty query argument list beside a content type mapping naming the profile",
                "limits-config-empty-beside-content-type",
                None,
                Some(Section::new(false, Some(Vec::new()))),
                Some(Section::new(
                    false,
                    Some(vec![ContentTypeMapping::new(
                        "application/json",
                        Some(&profile),
                    )]),
                )),
            ),
            true,
        ),
        (
            Config::new(
                "neither mapping section",
                "limits-config-neither",
                None,
                None,
                None,
            ),
            false,
        ),
        (
            Config::new(
                "both sections stating an empty list",
                "limits-config-both-empty",
                None,
                Some(Section::new(false, Some(Vec::new()))),
                Some(Section::new(false, Some(Vec::new()))),
            ),
            false,
        ),
        (
            Config::new(
                "a query argument section whose wrapper is left out, and no content type \
                 section",
                "limits-config-no-wrapper",
                None,
                Some(Section::new(true, None)),
                None,
            ),
            false,
        ),
        (
            Config::new(
                "an empty query argument list beside content type mappings naming no profile",
                "limits-config-content-type-nameless",
                None,
                Some(Section::new(true, Some(Vec::new()))),
                Some(Section::new(
                    true,
                    Some(vec![
                        ContentTypeMapping::new("text/plain", None),
                        ContentTypeMapping::new("text/xml", None),
                    ]),
                )),
            ),
            false,
        ),
        (
            Config::new(
                "a query argument mapping stating an empty profile id",
                "limits-config-empty-profile-id",
                None,
                Some(Section::new(
                    true,
                    Some(vec![QueryArgMapping::new("fle-profile", "")]),
                )),
                Some(Section::new(
                    true,
                    Some(vec![ContentTypeMapping::new("text/html", Some(&profile))]),
                )),
            ),
            false,
        ),
    ];

    for (row, admitted) in rows {
        let answer = create_config(&router, &row.xml()).await;
        let expected = if admitted { 201 } else { 400 };
        assert_eq!(
            answer.status, expected,
            "case {:?}: expected HTTP {expected}, got {} with {}",
            row.case, answer.status, answer.raw
        );
        if admitted {
            let id = answer.root().text_at("Id").unwrap_or_default().to_string();
            let read = get_config(&router, &id).await;
            assert_eq!(
                read.status, 200,
                "case {:?}: expected HTTP 200 from GetFieldLevelEncryption, got {} with {}",
                row.case, read.status, read.raw
            );
            continue;
        }
        assert_eq!(
            answer.error_code(),
            "QueryArgProfileEmpty",
            "case {:?}: expected QueryArgProfileEmpty, got {:?} with {}",
            row.case,
            answer.error_code(),
            answer.raw
        );
        assert_eq!(
            answer.root().find_text("Message").unwrap_or_default(),
            QUERY_ARG_PROFILE_EMPTY_MESSAGE,
            "case {:?}: expected the service's sentence, got {}",
            row.case,
            answer.raw
        );
    }
}

/// An update to a configuration must STATE the `QueryArgProfileConfig`: a payload that
/// leaves it out is refused and writes nothing, while the same payload carrying it lands.
#[tokio::test]
async fn an_update_that_drops_the_query_argument_section_is_refused() {
    let router = emulator();
    let profile = store_profile(&router, "limits-update-profile").await;

    // The rows span what the payload may carry BESIDE the dropped section: nothing, a
    // content type mapping that names a profile, and one that does not.
    let content_type_sections = vec![
        ("no content type section at all", None),
        (
            "a content type mapping naming the profile",
            Some(Section::new(
                true,
                Some(vec![ContentTypeMapping::new("text/html", Some(&profile))]),
            )),
        ),
        (
            "a content type mapping naming no profile",
            Some(Section::new(
                false,
                Some(vec![ContentTypeMapping::new("text/plain", None)]),
            )),
        ),
    ];

    for (index, (description, content_type)) in content_type_sections.into_iter().enumerate() {
        let reference = format!("limits-update-dropped-{index}");
        let stored = Config::new(
            description,
            &reference,
            None,
            Some(Section::new(
                true,
                Some(vec![QueryArgMapping::new("fle-profile", &profile)]),
            )),
            None,
        );
        let created = create_config(&router, &stored.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {description}: expected HTTP 201 seeding the configuration, got {} with {}",
            created.status, created.raw
        );
        let id = created.root().text_at("Id").unwrap_or_default().to_string();
        let etag = created.header("etag").to_string();

        let dropped = Config::new(description, &reference, None, None, content_type.clone());
        let refused = update_config(&router, &id, &dropped.xml(), Some(&etag)).await;
        assert_eq!(
            refused.status, 400,
            "case {description}: expected HTTP 400 for the dropped section, got {} with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "IllegalUpdate",
            "case {description}: expected IllegalUpdate, got {:?} with {}",
            refused.error_code(),
            refused.raw
        );
        assert_eq!(
            refused.root().find_text("Message").unwrap_or_default(),
            QUERY_ARG_PROFILE_CONFIG_REQUIRED_MESSAGE,
            "case {description}: expected the service's sentence, got {}",
            refused.raw
        );

        // The refusal wrote nothing, so the version token the create minted still stands —
        // and the same payload with the section stated lands under it.
        let kept = get_config(&router, &id).await;
        assert_eq!(
            kept.header("etag"),
            etag,
            "case {description}: expected the refused update to leave the version token, got {:?}",
            kept.header("etag")
        );
        let restated = Config::new(
            description,
            &reference,
            Some("the update that states the section"),
            Some(Section::new(
                false,
                Some(vec![QueryArgMapping::new("fle-profile", &profile)]),
            )),
            content_type,
        );
        let accepted = update_config(&router, &id, &restated.xml(), Some(&etag)).await;
        assert_eq!(
            accepted.status, 200,
            "case {description}: expected the restated section to land, got {} with {}",
            accepted.status, accepted.raw
        );
    }
}

// ---------------------------------------------------------------------------
// An invalidation's paths
// ---------------------------------------------------------------------------

/// The sentence CloudFront refuses a batch naming a value that is no invalidation path with.
const INVALID_PATHS_MESSAGE: &str = "Your request contains one or more invalid invalidation paths.";

/// `GET /2020-05-31/distribution/{DistributionId}/invalidation`.
async fn list_invalidations(router: &MultiServiceRouter, distribution_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/distribution/{distribution_id}/invalidation"),
        None,
        None,
    )
    .await
}

/// A batch naming a value that is no invalidation path is refused and records nothing,
/// while a batch whose every path names objects of the distribution lands.
#[tokio::test]
async fn an_invalidation_batch_naming_no_path_of_the_distribution_is_refused() {
    let router = emulator();
    let created = create_distribution(
        &router,
        &distribution_config("invalidation-paths", "ref-invalidation-paths").xml(),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case seeding the distribution: expected HTTP 201, got {} with {}",
        created.status, created.raw
    );
    let id = created.root().text_at("Id").unwrap_or_default().to_string();

    // The rows span what the rule generalises over: how a path starts, where a wildcard
    // sits, how many paths the batch names, and which of them breaks the rule.
    let rows: Vec<(&str, Vec<&str>, bool)> = vec![
        ("the distribution root", vec!["/"], true),
        ("one object", vec!["/index.html"], true),
        ("everything", vec!["/*"], true),
        ("a prefix wildcard", vec!["/images/*"], true),
        ("a wildcard in the middle of a path", vec!["/a/*/b.html"], true),
        ("a wildcard before an extension", vec!["/*.jpg"], true),
        (
            "several objects at several depths",
            vec!["/a.html", "/css/site.css", "/img/a/b.png"],
            true,
        ),
        ("a path carrying an escape", vec!["/my%20file.html"], true),
        ("a path carrying a query string", vec!["/a.html?v=2"], true),
        ("a path that does not start at the root", vec!["index.html"], false),
        ("a relative path", vec!["./index.html"], false),
        ("an empty path", vec![""], false),
        ("a bare wildcard", vec!["*"], false),
        ("a path carrying a space", vec!["/my file.html"], false),
        ("a path carrying an unsafe character", vec!["/a#b.html"], false),
        ("a path carrying another unsafe character", vec!["/a|b.html"], false),
        ("a path whose escape is incomplete", vec!["/a%2.html"], false),
        ("a path whose escape is no escape", vec!["/a%zz.html"], false),
        ("a batch naming no path at all", Vec::new(), false),
        (
            "one bad path among good ones",
            vec!["/a.html", "images/*", "/b.html"],
            false,
        ),
    ];

    let mut accepted = 0_usize;
    for (index, (description, paths, admitted)) in rows.into_iter().enumerate() {
        let reference = format!("inv-paths-{index}");
        let answer = create_invalidation(&router, &id, &batch_xml(&reference, &paths)).await;
        let expected = if admitted { 201 } else { 400 };
        assert_eq!(
            answer.status, expected,
            "case {description} ({paths:?}): expected HTTP {expected}, got {} with {}",
            answer.status, answer.raw
        );
        if admitted {
            accepted += 1;
        } else {
            assert_eq!(
                answer.error_code(),
                "InvalidArgument",
                "case {description} ({paths:?}): expected InvalidArgument, got {:?} with {}",
                answer.error_code(),
                answer.raw
            );
            assert_eq!(
                answer.root().find_text("Message").unwrap_or_default(),
                INVALID_PATHS_MESSAGE,
                "case {description} ({paths:?}): expected the service's sentence, got {}",
                answer.raw
            );
        }

        // Only the accepted batches were recorded: a refused one records nothing, so the
        // listing counts exactly the batches that landed.
        let listed = list_invalidations(&router, &id).await;
        let recorded = listed
            .root()
            .at("Items")
            .map_or(0, |items| items.children_named("InvalidationSummary").len());
        assert_eq!(
            recorded, accepted,
            "case {description} ({paths:?}): expected {accepted} recorded invalidation(s), got \
             {recorded} in {}",
            listed.raw
        );
    }
}

// ---------------------------------------------------------------------------
// The names a second resource cannot take
// ---------------------------------------------------------------------------

/// The names every uniqueness row below is driven over: the shortest form all three kinds
/// admit, the 64-character bound the strictest of them publishes
/// (`OriginAccessControlConfig$Name`: "You can specify up to 64 characters"), and the
/// character classes they share.
fn taken_names() -> Vec<(&'static str, String)> {
    vec![
        ("the shortest name every kind admits", "dup6".to_string()),
        (
            "a name at the 64-character bound the strictest kind publishes",
            "u".repeat(64),
        ),
        ("hyphens and digits", "orders-by-region-3".to_string()),
        ("mixed case and digits", "DupName44".to_string()),
    ]
}

/// A field-level encryption profile is unique by NAME: a second create under a name the
/// account holds is refused with the service's sentence for that code, and the profile that
/// holds the name is untouched.
#[tokio::test]
async fn a_field_level_encryption_profile_name_names_at_most_one_profile() {
    let router = emulator();
    let keys = upload_keys(&router).await;

    for (index, (description, name)) in taken_names().into_iter().enumerate() {
        let first = Profile {
            name: name.clone(),
            ..Profile::new(
                description,
                &format!("limits-profile-{index}-first"),
                None,
                vec![Entity::new(&keys[0], "provider-one", &["CreditCard"])],
            )
        };
        let created = create_profile(&router, &first.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {description} ({name}): expected HTTP 201 from the first create, got {} \
             with {}",
            created.status, created.raw
        );
        let id = created.root().text_at("Id").unwrap_or_default().to_string();
        let etag = created.header("etag").to_string();

        // A DIFFERENT configuration under the same name: a create that stored it rather
        // than refusing the name would show up in the read-back below.
        let repeat = Profile {
            name: name.clone(),
            ..Profile::new(
                description,
                &format!("limits-profile-{index}-second"),
                Some("the second profile under the taken name"),
                vec![Entity::new(&keys[1], "provider-two", &["Token", "Secret*"])],
            )
        };
        let refused = create_profile(&router, &repeat.xml()).await;
        assert_eq!(
            refused.status, 409,
            "case {description} ({name}): expected HTTP 409 under the taken name, got {} with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "FieldLevelEncryptionProfileAlreadyExists",
            "case {description} ({name}): expected FieldLevelEncryptionProfileAlreadyExists, got \
             {:?} with {}",
            refused.error_code(),
            refused.raw
        );
        assert_eq!(
            refused.root().find_text("Message").unwrap_or_default(),
            "The name that you are using to create a field level encryption profile is \
             associated with another field level encryption profile in your account.",
            "case {description} ({name}): expected the service's sentence, got {}",
            refused.raw
        );

        let read = get_profile(&router, &id).await;
        assert_eq!(
            read.header("etag"),
            etag,
            "case {description} ({name}): expected the refusal to leave the stored profile at \
             its own version, got {:?}",
            read.header("etag")
        );
        assert_eq!(
            read.root().text_at("FieldLevelEncryptionProfileConfig/CallerReference"),
            Some(format!("limits-profile-{index}-first").as_str()),
            "case {description} ({name}): expected the first create's record, got {}",
            read.raw
        );
    }
}

/// A public key is unique by NAME as well as by caller reference: a second upload under a
/// name the account holds is refused with the sentence that names the taken value.
#[tokio::test]
async fn a_public_key_name_names_at_most_one_key() {
    let router = emulator();

    for (index, (description, name)) in taken_names().into_iter().enumerate() {
        let first = Key {
            case: description.to_string(),
            caller_reference: format!("limits-key-{index}-first"),
            name: name.clone(),
            encoded_key: pem(1, 'L'),
            comment: None,
        };
        let created = create_key(&router, &first.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {description} ({name}): expected HTTP 201 from the first upload, got {} with {}",
            created.status, created.raw
        );
        let id = created.root().text_at("Id").unwrap_or_default().to_string();

        let repeat = Key {
            case: description.to_string(),
            caller_reference: format!("limits-key-{index}-second"),
            name: name.clone(),
            encoded_key: pem(3, 'M'),
            comment: Some("the second key under the taken name".to_string()),
        };
        let refused = create_key(&router, &repeat.xml()).await;
        assert_eq!(
            refused.status, 409,
            "case {description} ({name}): expected HTTP 409 under the taken name, got {} with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "PublicKeyAlreadyExists",
            "case {description} ({name}): expected PublicKeyAlreadyExists, got {:?} with {}",
            refused.error_code(),
            refused.raw
        );
        assert_eq!(
            refused.root().find_text("Message").unwrap_or_default(),
            "The name that you are using to create a CloudFront public key is associated with \
             another public key in your account.",
            "case {description} ({name}): expected the service's sentence, got {}",
            refused.raw
        );

        // The key that holds the name still reports what its own upload stored.
        let read = get_key(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {description} ({name}): expected the stored key to be readable, got {} with {}",
            read.status, read.raw
        );
        let config = read
            .root()
            .at("PublicKeyConfig")
            .unwrap_or_else(|| panic!("case {description}: no configuration in {}", read.raw));
        first.assert_matches(config, "after a refused repeat", &read.raw);
    }
}

/// An origin access control is unique by NAME: a second create under a name the account
/// holds is refused whatever else it submits, and the control that holds the name stands.
#[tokio::test]
async fn an_origin_access_control_name_names_at_most_one_control() {
    let router = emulator();
    let rows = control_table();

    for (index, (description, name)) in taken_names().into_iter().enumerate() {
        let first = rows[index % rows.len()].with_name(description, &name);
        let created = create_control(&router, &first.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {description} ({name}): expected HTTP 201 from the first create, got {} with {}",
            created.status, created.raw
        );
        let id = created.root().text_at("Id").unwrap_or_default().to_string();
        let etag = created.header("etag").to_string();

        // Another row of the kind's own table under the taken name: a different parameter
        // set, so a create that stored it would be visible in the read-back.
        let repeat = rows[(index + 1) % rows.len()].with_name(description, &name);
        let refused = create_control(&router, &repeat.xml()).await;
        assert_eq!(
            refused.status, 409,
            "case {description} ({name}): expected HTTP 409 under the taken name, got {} with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "OriginAccessControlAlreadyExists",
            "case {description} ({name}): expected OriginAccessControlAlreadyExists, got {:?} \
             with {}",
            refused.error_code(),
            refused.raw
        );
        assert_eq!(
            refused.root().find_text("Message").unwrap_or_default(),
            "An origin access control with the same name already exists.",
            "case {description} ({name}): expected the service's sentence, got {}",
            refused.raw
        );

        let read = get_control(&router, &id).await;
        assert_eq!(
            read.header("etag"),
            etag,
            "case {description} ({name}): expected the refusal to leave the stored control at \
             its own version, got {:?}",
            read.header("etag")
        );
        let config = read
            .root()
            .at("OriginAccessControlConfig")
            .unwrap_or_else(|| panic!("case {description}: no configuration in {}", read.raw));
        first.assert_matches(config, "after a refused repeat", &read.raw);
    }
}
