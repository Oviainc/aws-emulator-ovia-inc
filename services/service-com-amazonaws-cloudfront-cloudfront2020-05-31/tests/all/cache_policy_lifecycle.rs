// SPDX-License-Identifier: CC0-1.0

//! The CloudFront cache policy control plane, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront's cache policy surface
//! states:
//!
//! * a policy is PERSISTED by the call that creates it, which MINTS its identity (a UUID
//!   `Id` and a version token), so a later `GetCachePolicy` and `GetCachePolicyConfig`
//!   answer from that record rather than from the request being served — down to the
//!   members the create left out;
//! * the three TTLs are ORDERED (`MinTTL` <= `DefaultTTL` <= `MaxTTL`, over the members a
//!   request supplies), and a TTL is an amount of time in seconds so it is never negative:
//!   a configuration that breaks either is refused with `InvalidArgument` and stores
//!   nothing;
//! * `CachePolicyConfig$Comment` "cannot be longer than 128 characters", counted in
//!   characters rather than bytes;
//! * `CachePolicyConfig$Name` is "A unique name to identify the cache policy", so a
//!   repeated name is refused with `CachePolicyAlreadyExists` and leaves the policy that
//!   holds it untouched;
//! * every CloudFront list travels in a wrapper that states its own length, and a request
//!   whose `Quantity` disagrees with its `Items` is refused with `InconsistentQuantities`
//!   and stores nothing;
//! * an id no policy holds is `NoSuchCachePolicy`, whatever shape the id takes.
//!
//! The table rows differ along everything these rules generalise over: which optional
//! members the configuration carries at all (`Comment`, `DefaultTTL`, `MaxTTL`, the whole
//! `ParametersInCacheKeyAndForwardedToOrigin` block), each name list's length (none, one,
//! many), the enum values each behaviour member draws from (every value the model's
//! `CachePolicyHeaderBehavior`, `CachePolicyCookieBehavior` and
//! `CachePolicyQueryStringBehavior` admit, from the crate's model-derived catalogs), the
//! TTL magnitudes (zero, one, each documented default and one past it), and whether a text
//! member is ASCII, empty, non-ASCII or carries XML metacharacters.

use std::collections::BTreeSet;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::ops::types::{
    CACHE_POLICY_COOKIE_BEHAVIOR_VALUES, CACHE_POLICY_HEADER_BEHAVIOR_VALUES,
    CACHE_POLICY_QUERY_STRING_BEHAVIOR_VALUES,
};
use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    MAX_CACHE_POLICIES, MAX_CACHE_POLICY_COMMENT, NO_SUCH_CACHE_POLICY_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, emulator, is_version_token, Answer, Node, API_VERSION, HOST,
};
use crate::response_headers_policy_lifecycle::{assert_absent, escape, is_policy_id, text};

// ---------------------------------------------------------------------------
// Calling the cache policy surface
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/cache-policy` with the `CachePolicyConfig` document `body` — the
/// member the operation binds to the HTTP payload.
pub(crate) async fn create_policy(router: &MultiServiceRouter, body: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/cache-policy"),
        Some(body),
        None,
    )
    .await
}

/// `GET /2020-05-31/cache-policy/{Id}`.
pub(crate) async fn get_policy(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/cache-policy/{id}"),
        None,
        None,
    )
    .await
}

/// `GET /2020-05-31/cache-policy/{Id}/config`.
pub(crate) async fn get_policy_config(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/cache-policy/{id}/config"),
        None,
        None,
    )
    .await
}

// ---------------------------------------------------------------------------
// The configurations under test
// ---------------------------------------------------------------------------

/// The cache key parameters a configuration may carry, or may leave out entirely.
///
/// A `None` list means the wrapper is left out of the document, which is what makes "the
/// record reports what was submitted" testable for absent members too; `Some(vec![])` is
/// the wrapper carrying a `Quantity` of zero and no items.
#[derive(Debug, Clone)]
pub(crate) struct Parameters {
    /// `EnableAcceptEncodingGzip`, which the model makes required inside this block.
    pub(crate) gzip: bool,
    /// `EnableAcceptEncodingBrotli`, which the model leaves optional.
    pub(crate) brotli: Option<bool>,
    /// A value of `CachePolicyHeaderBehavior`.
    pub(crate) header_behavior: String,
    pub(crate) headers: Option<Vec<String>>,
    /// A value of `CachePolicyCookieBehavior`.
    pub(crate) cookie_behavior: String,
    pub(crate) cookies: Option<Vec<String>>,
    /// A value of `CachePolicyQueryStringBehavior`.
    pub(crate) query_string_behavior: String,
    pub(crate) query_strings: Option<Vec<String>>,
}

impl Default for Parameters {
    /// The cache key that includes nothing beyond the URL path: `none` for each of the
    /// three behaviours — a value every one of the three model-derived catalogs below
    /// admits — and no name list at all.
    fn default() -> Self {
        Self {
            gzip: false,
            brotli: None,
            header_behavior: "none".to_string(),
            headers: None,
            cookie_behavior: "none".to_string(),
            cookies: None,
            query_string_behavior: "none".to_string(),
            query_strings: None,
        }
    }
}

impl Parameters {
    /// The `ParametersInCacheKeyAndForwardedToOrigin` XML, in the model's member
    /// declaration order.
    pub(crate) fn xml(&self) -> String {
        let mut out = format!(
            "<ParametersInCacheKeyAndForwardedToOrigin>\
             <EnableAcceptEncodingGzip>{}</EnableAcceptEncodingGzip>",
            self.gzip
        );
        if let Some(brotli) = self.brotli {
            out.push_str(&format!(
                "<EnableAcceptEncodingBrotli>{brotli}</EnableAcceptEncodingBrotli>"
            ));
        }
        out.push_str(&format!(
            "<HeadersConfig><HeaderBehavior>{}</HeaderBehavior>{}</HeadersConfig>",
            self.header_behavior,
            wrapper("Headers", self.headers.as_deref())
        ));
        out.push_str(&format!(
            "<CookiesConfig><CookieBehavior>{}</CookieBehavior>{}</CookiesConfig>",
            self.cookie_behavior,
            wrapper("Cookies", self.cookies.as_deref())
        ));
        out.push_str(&format!(
            "<QueryStringsConfig><QueryStringBehavior>{}</QueryStringBehavior>{}\
             </QueryStringsConfig>",
            self.query_string_behavior,
            wrapper("QueryStrings", self.query_strings.as_deref())
        ));
        out.push_str("</ParametersInCacheKeyAndForwardedToOrigin>");
        out
    }
}

/// One `CachePolicyConfig` a client submits. `MinTTL` is the one TTL the model makes
/// required; a `None` TTL is left out of the document entirely.
#[derive(Debug, Clone, Default)]
pub(crate) struct Policy {
    pub(crate) case: &'static str,
    pub(crate) name: String,
    pub(crate) comment: Option<String>,
    pub(crate) min_ttl: i64,
    pub(crate) default_ttl: Option<i64>,
    pub(crate) max_ttl: Option<i64>,
    pub(crate) parameters: Option<Parameters>,
}

impl Policy {
    /// The `CachePolicyConfig` XML a client sends for this configuration, in the model's
    /// member declaration order.
    pub(crate) fn xml(&self) -> String {
        let mut out =
            format!("<CachePolicyConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">");
        if let Some(comment) = &self.comment {
            out.push_str(&format!("<Comment>{}</Comment>", escape(comment)));
        }
        out.push_str(&format!("<Name>{}</Name>", escape(&self.name)));
        if let Some(default_ttl) = self.default_ttl {
            out.push_str(&format!("<DefaultTTL>{default_ttl}</DefaultTTL>"));
        }
        if let Some(max_ttl) = self.max_ttl {
            out.push_str(&format!("<MaxTTL>{max_ttl}</MaxTTL>"));
        }
        out.push_str(&format!("<MinTTL>{}</MinTTL>", self.min_ttl));
        if let Some(parameters) = &self.parameters {
            out.push_str(&parameters.xml());
        }
        out.push_str("</CachePolicyConfig>");
        out
    }
}

/// One CloudFront name list wrapper: its `Quantity` and its `Items`, each item in the
/// `Name` element the model's `xmlName` gives `HeaderList`, `CookieNameList` and
/// `QueryStringNamesList`. `None` leaves the whole wrapper out.
fn wrapper(name: &str, values: Option<&[String]>) -> String {
    let Some(values) = values else {
        return String::new();
    };
    let mut out = format!("<{name}><Quantity>{}</Quantity>", values.len());
    if !values.is_empty() {
        out.push_str("<Items>");
        for value in values {
            out.push_str(&format!("<Name>{}</Name>", escape(value)));
        }
        out.push_str("</Items>");
    }
    out.push_str(&format!("</{name}>"));
    out
}

/// Every value `CachePolicyHeaderBehavior` admits. Taken from the crate's own
/// model-derived catalog (`ops::types::CACHE_POLICY_HEADER_BEHAVIOR_VALUES`, which
/// `smithy_query(query="shape",shape="CachePolicyHeaderBehavior")` reports the same values
/// for) rather than written out here, so the catalog cannot drift from the model.
const HEADER_BEHAVIORS: &[&str] = CACHE_POLICY_HEADER_BEHAVIOR_VALUES;

/// Every value `CachePolicyCookieBehavior` admits, from the same model-derived catalog.
const COOKIE_BEHAVIORS: &[&str] = CACHE_POLICY_COOKIE_BEHAVIOR_VALUES;

/// Every value `CachePolicyQueryStringBehavior` admits, from the same model-derived
/// catalog.
const QUERY_STRING_BEHAVIORS: &[&str] = CACHE_POLICY_QUERY_STRING_BEHAVIOR_VALUES;

/// The `DefaultTTL` CloudFront uses for a configuration that names none:
/// `CachePolicyConfig$DefaultTTL`, "The default value for this field is 86400 seconds (one
/// day)" (`smithy_query(query="doc",shape="CachePolicyConfig",member="DefaultTTL")`).
pub(crate) const DOCUMENTED_DEFAULT_TTL: i64 = 86_400;

/// The `MaxTTL` CloudFront uses for a configuration that names none:
/// `CachePolicyConfig$MaxTTL`, "The default value for this field is 31536000 seconds (one
/// year)" (`smithy_query(query="doc",shape="CachePolicyConfig",member="MaxTTL")`).
pub(crate) const DOCUMENTED_MAX_TTL: i64 = 31_536_000;

/// A list of owned strings from string literals, for the tables below.
pub(crate) fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

/// The configurations the persistence rules below are asserted over.
///
/// The rows span the whole class those rules generalise over: no optional member at all, a
/// comment that is empty, ASCII or non-ASCII, the cache key parameters absent entirely,
/// each of the three name lists absent, carrying no items, one item and many, behaviours
/// drawn from the three model-derived catalogs, TTLs at zero, one, the two documented
/// defaults and past them, and names carrying XML metacharacters.
pub(crate) fn policy_table() -> Vec<Policy> {
    vec![
        Policy {
            case: "the required members alone: a name and MinTTL, nothing else",
            name: "min-only".to_string(),
            min_ttl: 0,
            ..Policy::default()
        },
        Policy {
            case: "an empty comment and all three TTLs equal",
            name: "equal-ttls".to_string(),
            comment: Some(String::new()),
            min_ttl: 60,
            default_ttl: Some(60),
            max_ttl: Some(60),
            parameters: Some(Parameters::default()),
        },
        Policy {
            case: "the documented defaults stated explicitly, with empty wrappers",
            name: "documented-defaults".to_string(),
            comment: Some("the CloudFront defaults, spelled out".to_string()),
            min_ttl: 1,
            default_ttl: Some(DOCUMENTED_DEFAULT_TTL),
            max_ttl: Some(DOCUMENTED_MAX_TTL),
            parameters: Some(Parameters {
                gzip: true,
                brotli: Some(true),
                headers: Some(Vec::new()),
                cookies: Some(Vec::new()),
                query_strings: Some(Vec::new()),
                ..Parameters::default()
            }),
        },
        Policy {
            case: "whitelist of exactly one header, cookie and query string",
            name: "whitelist-singletons".to_string(),
            comment: Some("one of each".to_string()),
            min_ttl: 0,
            default_ttl: Some(300),
            max_ttl: Some(3_600),
            parameters: Some(Parameters {
                gzip: true,
                brotli: Some(false),
                header_behavior: "whitelist".to_string(),
                headers: Some(strings(&["authorization"])),
                cookie_behavior: "whitelist".to_string(),
                cookies: Some(strings(&["session"])),
                query_string_behavior: "whitelist".to_string(),
                query_strings: Some(strings(&["page"])),
            }),
        },
        Policy {
            case: "whitelist of many, with lists of different lengths",
            name: "whitelist-many".to_string(),
            comment: Some("プロダクション — keys on a & b < c".to_string()),
            min_ttl: 2,
            default_ttl: Some(86_401),
            max_ttl: Some(DOCUMENTED_MAX_TTL + 1),
            parameters: Some(Parameters {
                gzip: false,
                brotli: None,
                header_behavior: "whitelist".to_string(),
                headers: Some(strings(&[
                    "accept",
                    "accept-language",
                    "authorization",
                    "x-tenant-id",
                ])),
                cookie_behavior: "whitelist".to_string(),
                cookies: Some(strings(&["session", "cart"])),
                query_string_behavior: "whitelist".to_string(),
                query_strings: Some(strings(&["a&b", "q<uery>", "\"quoted\"", "it's", "page"])),
            }),
        },
        Policy {
            case: "allExcept cookies and all query strings, no MaxTTL at all",
            name: "mixed-behaviors".to_string(),
            min_ttl: 10,
            default_ttl: Some(DOCUMENTED_DEFAULT_TTL),
            max_ttl: None,
            comment: None,
            parameters: Some(Parameters {
                gzip: true,
                brotli: Some(true),
                header_behavior: "whitelist".to_string(),
                headers: Some(strings(&["CloudFront-Viewer-Country"])),
                cookie_behavior: "allExcept".to_string(),
                cookies: Some(strings(&["tracking"])),
                query_string_behavior: "all".to_string(),
                query_strings: None,
            }),
        },
        Policy {
            case: "a MinTTL past the documented MaxTTL default, with both others absent",
            name: "min-past-the-year".to_string(),
            min_ttl: DOCUMENTED_MAX_TTL + 1,
            ..Policy::default()
        },
    ]
}

// ---------------------------------------------------------------------------
// Reading what the service stored
// ---------------------------------------------------------------------------

/// Assert the `CachePolicyConfig` element `config` reports exactly what `policy`
/// submitted: every member it carried, and no element for a member it left out.
pub(crate) fn assert_config(config: &Node, policy: &Policy, context: &str, raw: &str) {
    let case = policy.case;

    match &policy.comment {
        Some(comment) => {
            let actual = config
                .at("Comment")
                .map(|node| node.text().to_string())
                .unwrap_or_else(|| {
                    panic!("case {case} ({context}): the answer carries no Comment: {raw}")
                });
            assert_eq!(
                &actual, comment,
                "case {case} ({context}): expected the submitted Comment {comment:?}, got \
                 {actual:?}"
            );
        }
        None => assert_absent(config, "Comment", case, context),
    }

    let name = text(config, "Name", case, context, raw);
    assert_eq!(
        name, policy.name,
        "case {case} ({context}): expected the submitted Name {:?}, got {name:?}",
        policy.name
    );

    assert_ttl(config, "MinTTL", Some(policy.min_ttl), case, context, raw);
    assert_ttl(config, "DefaultTTL", policy.default_ttl, case, context, raw);
    assert_ttl(config, "MaxTTL", policy.max_ttl, case, context, raw);

    let Some(parameters) = &policy.parameters else {
        assert_absent(
            config,
            "ParametersInCacheKeyAndForwardedToOrigin",
            case,
            context,
        );
        return;
    };
    let block = config
        .at("ParametersInCacheKeyAndForwardedToOrigin")
        .unwrap_or_else(|| {
            panic!(
                "case {case} ({context}): the answer carries no \
                 ParametersInCacheKeyAndForwardedToOrigin: {raw}"
            )
        });

    let gzip = text(block, "EnableAcceptEncodingGzip", case, context, raw);
    assert_eq!(
        gzip,
        parameters.gzip.to_string(),
        "case {case} ({context}): expected EnableAcceptEncodingGzip {}, got {gzip:?}",
        parameters.gzip
    );
    match parameters.brotli {
        Some(brotli) => {
            let actual = text(block, "EnableAcceptEncodingBrotli", case, context, raw);
            assert_eq!(
                actual,
                brotli.to_string(),
                "case {case} ({context}): expected EnableAcceptEncodingBrotli {brotli}, got \
                 {actual:?}"
            );
        }
        None => assert_absent(block, "EnableAcceptEncodingBrotli", case, context),
    }

    let behavior = text(block, "HeadersConfig/HeaderBehavior", case, context, raw);
    assert_eq!(
        behavior, parameters.header_behavior,
        "case {case} ({context}): expected HeaderBehavior {:?}, got {behavior:?}",
        parameters.header_behavior
    );
    assert_list(
        block,
        "HeadersConfig",
        "Headers",
        parameters.headers.as_deref(),
        case,
        context,
        raw,
    );

    let behavior = text(block, "CookiesConfig/CookieBehavior", case, context, raw);
    assert_eq!(
        behavior, parameters.cookie_behavior,
        "case {case} ({context}): expected CookieBehavior {:?}, got {behavior:?}",
        parameters.cookie_behavior
    );
    assert_list(
        block,
        "CookiesConfig",
        "Cookies",
        parameters.cookies.as_deref(),
        case,
        context,
        raw,
    );

    let behavior = text(
        block,
        "QueryStringsConfig/QueryStringBehavior",
        case,
        context,
        raw,
    );
    assert_eq!(
        behavior, parameters.query_string_behavior,
        "case {case} ({context}): expected QueryStringBehavior {:?}, got {behavior:?}",
        parameters.query_string_behavior
    );
    assert_list(
        block,
        "QueryStringsConfig",
        "QueryStrings",
        parameters.query_strings.as_deref(),
        case,
        context,
        raw,
    );
}

/// Assert one TTL member reports the value the request submitted, or is absent entirely
/// when the request left it out — the service invents no TTL for a member it was not given.
fn assert_ttl(
    config: &Node,
    member: &str,
    expected: Option<i64>,
    case: &str,
    context: &str,
    raw: &str,
) {
    let Some(expected) = expected else {
        assert_absent(config, member, case, context);
        return;
    };
    let actual = text(config, member, case, context, raw);
    assert_eq!(
        actual,
        expected.to_string(),
        "case {case} ({context}): expected {member} {expected}, got {actual:?}"
    );
}

/// Assert one name list wrapper under `parent` reports `values` in order beside a
/// `Quantity` that states their number — or is absent entirely when the request left it
/// out.
fn assert_list(
    block: &Node,
    parent: &str,
    list: &str,
    values: Option<&[String]>,
    case: &str,
    context: &str,
    raw: &str,
) {
    let path = format!("{parent}/{list}");
    let Some(values) = values else {
        assert_absent(block, &path, case, context);
        return;
    };

    let quantity = text(block, &format!("{path}/Quantity"), case, context, raw);
    assert_eq!(
        quantity,
        values.len().to_string(),
        "case {case} ({context}): expected {path}.Quantity {}, got {quantity:?}",
        values.len()
    );
    let items: Vec<String> = block
        .at(&format!("{path}/Items"))
        .map(|items| {
            items
                .children_named("Name")
                .into_iter()
                .map(|item| item.text().to_string())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        items, values,
        "case {case} ({context}): expected the submitted {path} items {values:?}, got {items:?}"
    );
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// A create PERSISTS the policy it mints, and both reads answer from that one record: the
/// configuration exactly as submitted (down to the members the request left out), a UUID
/// id, and the version token the create handed back. Across the table, no two policies
/// share an id or a version token, and each one's `Location` names its own URI.
#[tokio::test]
async fn create_persists_the_policy_the_reads_then_report() {
    let router = emulator();
    let mut ids = BTreeSet::new();
    let mut etags = BTreeSet::new();

    for policy in policy_table() {
        let case = policy.case;
        let created = create_policy(&router, &policy.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            created.status, created.raw
        );

        let document = created.root();
        let id = text(document, "Id", case, "the create", &created.raw);
        assert!(
            is_policy_id(&id),
            "case {case}: expected a UUID Id, got {id:?}"
        );
        let last_modified = text(
            document,
            "LastModifiedTime",
            case,
            "the create",
            &created.raw,
        );
        assert!(
            !last_modified.is_empty(),
            "case {case}: expected a LastModifiedTime, got {last_modified:?}"
        );
        let config = document.at("CachePolicyConfig").unwrap_or_else(|| {
            panic!(
                "case {case}: the create carries no CachePolicyConfig: {}",
                created.raw
            )
        });
        assert_config(config, &policy, "the create", &created.raw);

        let etag = created.header("etag").to_string();
        assert!(
            is_version_token(&etag),
            "case {case}: expected a version token in the ETag header, got {etag:?}"
        );
        let location = created.header("location").to_string();
        assert_eq!(
            location,
            format!("https://{HOST}/{API_VERSION}/cache-policy/{id}"),
            "case {case}: expected the new policy's URI in Location, got {location:?}"
        );
        assert!(ids.insert(id.clone()), "case {case}: the id {id} repeated");
        assert!(
            etags.insert(etag.clone()),
            "case {case}: the version token {etag} repeated"
        );

        // `GetCachePolicy` answers from the stored record, not from any request.
        let fetched = get_policy(&router, &id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected HTTP 200 from GetCachePolicy, got {} with {}",
            fetched.status, fetched.raw
        );
        let fetched_document = fetched.root();
        assert_eq!(
            fetched_document.text_at("Id"),
            Some(id.as_str()),
            "case {case}: expected the get to report the created Id {id}, got {}",
            fetched.raw
        );
        assert_eq!(
            fetched_document.text_at("LastModifiedTime"),
            Some(last_modified.as_str()),
            "case {case}: expected the stored LastModifiedTime {last_modified}, got {}",
            fetched.raw
        );
        let fetched_config = fetched_document.at("CachePolicyConfig").unwrap_or_else(|| {
            panic!(
                "case {case}: the get carries no CachePolicyConfig: {}",
                fetched.raw
            )
        });
        assert_config(fetched_config, &policy, "the get", &fetched.raw);
        assert_eq!(
            fetched.header("etag"),
            etag,
            "case {case}: expected the create's version token from the get, got {:?}",
            fetched.header("etag")
        );

        // `GetCachePolicyConfig` answers from the SAME record: the configuration alone,
        // beside the same version token.
        let config_answer = get_policy_config(&router, &id).await;
        assert_eq!(
            config_answer.status, 200,
            "case {case}: expected HTTP 200 from GetCachePolicyConfig, got {} with {}",
            config_answer.status, config_answer.raw
        );
        assert_config(
            config_answer.root(),
            &policy,
            "the config get",
            &config_answer.raw,
        );
        assert_eq!(
            config_answer.header("etag"),
            etag,
            "case {case}: expected the create's version token from the config get, got {:?}",
            config_answer.header("etag")
        );
    }
}

/// One row of the TTL table: the case, the three TTLs a request supplies (`None` leaves
/// the member out of the document) and whether the configuration must be refused.
type TtlRow = (&'static str, i64, Option<i64>, Option<i64>, bool);

/// The sentence CloudFront refuses a TTL chain that is not ordered with: fixed text stating
/// the RULE over all three members — which is what attributes the refusal, `InvalidArgument`
/// being this service's catch-all — and carrying none of the values submitted. Spelled out
/// here as a literal rather than read back off the crate's constant, so the WORDING is what
/// these rows are held to.
const TTL_ORDER_MESSAGE: &str = "MinTTL, MaxTTL and DefaultTTL should follow order MinTTL \
                                less than or equal to DefaultTTL less than or equal to MaxTTL";

/// The TTL rule: the values a request supplies are ordered `MinTTL` <= `DefaultTTL` <=
/// `MaxTTL`, and a TTL is an amount of time in seconds so none of them is negative. A
/// configuration that breaks the rule is refused with `InvalidArgument` (HTTP 400) and
/// stores nothing — the name it named is still free afterwards; one that satisfies it is
/// accepted and readable.
///
/// The rows span the rule: each disordered pair, in both the two- and three-member form,
/// each documented default and one past it, zero, one and equal values, and a negative
/// value of each member.
#[tokio::test]
async fn ttls_out_of_order_or_negative_are_refused() {
    let router = emulator();

    // (case, MinTTL, DefaultTTL, MaxTTL, refused)
    let rows: [TtlRow; 16] = [
        ("MaxTTL below MinTTL", 3_600, None, Some(60), true),
        ("MaxTTL below MinTTL by one", 2, None, Some(1), true),
        (
            "MaxTTL below MinTTL with a DefaultTTL between them",
            3_600,
            Some(100),
            Some(60),
            true,
        ),
        ("DefaultTTL below MinTTL", 100, Some(99), None, true),
        (
            "MaxTTL below DefaultTTL",
            0,
            Some(DOCUMENTED_DEFAULT_TTL),
            Some(60),
            true,
        ),
        (
            "MaxTTL below MinTTL at the documented year",
            DOCUMENTED_MAX_TTL + 1,
            None,
            Some(DOCUMENTED_MAX_TTL),
            true,
        ),
        ("a negative MinTTL", -1, None, None, true),
        ("a negative DefaultTTL", 0, Some(-5), None, true),
        ("a negative MaxTTL beside a zero MinTTL", 0, None, Some(-1), true),
        ("the three TTLs equal at zero", 0, Some(0), Some(0), false),
        ("the three TTLs equal at one", 1, Some(1), Some(1), false),
        (
            "the documented defaults",
            0,
            Some(DOCUMENTED_DEFAULT_TTL),
            Some(DOCUMENTED_MAX_TTL),
            false,
        ),
        (
            "one second past each documented default",
            1,
            Some(DOCUMENTED_DEFAULT_TTL + 1),
            Some(DOCUMENTED_MAX_TTL + 1),
            false,
        ),
        (
            "a MinTTL past the documented year, with the others absent",
            DOCUMENTED_MAX_TTL + 1,
            None,
            None,
            false,
        ),
        ("MinTTL alone at zero", 0, None, None, false),
        (
            "MinTTL and MaxTTL ordered, DefaultTTL absent",
            60,
            None,
            Some(DOCUMENTED_MAX_TTL),
            false,
        ),
    ];

    for (case, min_ttl, default_ttl, max_ttl, refused) in rows {
        let name = format!("ttl-{}", case.replace(' ', "-"));
        let policy = Policy {
            case,
            name: name.clone(),
            min_ttl,
            default_ttl,
            max_ttl,
            parameters: Some(Parameters::default()),
            comment: None,
        };
        let answer = create_policy(&router, &policy.xml()).await;

        if !refused {
            assert_eq!(
                answer.status, 201,
                "case {case}: expected HTTP 201 for MinTTL {min_ttl}, DefaultTTL \
                 {default_ttl:?}, MaxTTL {max_ttl:?}, got {} with {}",
                answer.status, answer.raw
            );
            let id = text(answer.root(), "Id", case, "the create", &answer.raw);
            let fetched = get_policy_config(&router, &id).await;
            assert_eq!(
                fetched.status, 200,
                "case {case}: expected the accepted policy to be readable, got {} with {}",
                fetched.status, fetched.raw
            );
            assert_config(fetched.root(), &policy, "the config get", &fetched.raw);
            continue;
        }

        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400 for MinTTL {min_ttl}, DefaultTTL {default_ttl:?}, \
             MaxTTL {max_ttl:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument, got {}",
            answer.raw
        );
        // WHICH rule the row breaks decides the sentence: a negative TTL names no duration at
        // all, while every break of the ordering chain — whichever pair exposed it — is the
        // one sentence that states the chain.
        let negative = min_ttl < 0
            || default_ttl.is_some_and(|value| value < 0)
            || max_ttl.is_some_and(|value| value < 0);
        if !negative {
            let message = answer.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                message, TTL_ORDER_MESSAGE,
                "case {case}: expected the ordering rule {TTL_ORDER_MESSAGE:?}, got {message:?}"
            );
        }

        // Nothing was stored: the name the refused create named is still free, so a
        // well-formed create under it succeeds.
        let repaired = Policy {
            case,
            name,
            min_ttl: 0,
            ..Policy::default()
        };
        let accepted = create_policy(&router, &repaired.xml()).await;
        assert_eq!(
            accepted.status, 201,
            "case {case}: the refused create must store nothing, got {} with {}",
            accepted.status, accepted.raw
        );
    }
}

/// `CachePolicyConfig$Comment`: "A comment to describe the cache policy. The comment cannot
/// be longer than 128 characters." The bound is counted in CHARACTERS, so a comment of
/// non-ASCII characters is measured the same way; a comment at the bound is accepted and
/// round-trips, one past it is refused with `InvalidArgument` and stores nothing.
#[tokio::test]
async fn a_comment_past_the_documented_bound_is_refused() {
    let router = emulator();

    // (case, the comment, refused)
    let rows: [(&str, String, bool); 5] = [
        ("an empty comment", String::new(), false),
        (
            "a comment exactly at the bound",
            "c".repeat(MAX_CACHE_POLICY_COMMENT),
            false,
        ),
        (
            "a non-ASCII comment at the bound, measured in characters",
            "é".repeat(MAX_CACHE_POLICY_COMMENT),
            false,
        ),
        (
            "a comment one character past the bound",
            "c".repeat(MAX_CACHE_POLICY_COMMENT + 1),
            true,
        ),
        (
            "a comment far past the bound",
            "c".repeat(MAX_CACHE_POLICY_COMMENT * 4),
            true,
        ),
    ];

    for (case, comment, refused) in rows {
        let name = format!("comment-{}", case.replace(' ', "-"));
        let policy = Policy {
            case,
            name: name.clone(),
            comment: Some(comment),
            min_ttl: 1,
            ..Policy::default()
        };
        let answer = create_policy(&router, &policy.xml()).await;

        if refused {
            assert_eq!(
                answer.status, 400,
                "case {case}: expected HTTP 400, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "InvalidArgument",
                "case {case}: expected InvalidArgument, got {}",
                answer.raw
            );
            continue;
        }

        assert_eq!(
            answer.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            answer.status, answer.raw
        );
        let id = text(answer.root(), "Id", case, "the create", &answer.raw);
        let fetched = get_policy_config(&router, &id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected HTTP 200, got {} with {}",
            fetched.status, fetched.raw
        );
        assert_config(fetched.root(), &policy, "the config get", &fetched.raw);
    }
}

/// `CachePolicyConfig$Name` is "A unique name to identify the cache policy": the second
/// create under a name is refused with `CachePolicyAlreadyExists` (HTTP 409), and the
/// policy that holds the name keeps the identity, the configuration and the version token
/// its own create minted.
#[tokio::test]
async fn a_name_names_at_most_one_cache_policy() {
    let router = emulator();

    for policy in policy_table() {
        let case = policy.case;
        let first = create_policy(&router, &policy.xml()).await;
        assert_eq!(
            first.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            first.status, first.raw
        );
        let id = text(first.root(), "Id", case, "the first create", &first.raw);
        let etag = first.header("etag").to_string();

        // The same name again, carrying a configuration that DIFFERS, so a policy that was
        // replaced rather than refused would be visible in the read below.
        let repeat = Policy {
            comment: Some("a second create under a name already taken".to_string()),
            min_ttl: 7,
            default_ttl: Some(7),
            max_ttl: Some(7),
            parameters: Some(Parameters {
                gzip: true,
                header_behavior: "whitelist".to_string(),
                headers: Some(strings(&["x-second"])),
                ..Parameters::default()
            }),
            ..policy.clone()
        };
        let second = create_policy(&router, &repeat.xml()).await;
        assert_eq!(
            second.status, 409,
            "case {case}: expected HTTP 409 for a repeated name, got {} with {}",
            second.status, second.raw
        );
        assert_eq!(
            second.error_code(),
            "CachePolicyAlreadyExists",
            "case {case}: expected CachePolicyAlreadyExists, got {}",
            second.raw
        );

        let fetched = get_policy(&router, &id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected the first policy to survive the refusal, got {} with {}",
            fetched.status, fetched.raw
        );
        let config = fetched.root().at("CachePolicyConfig").unwrap_or_else(|| {
            panic!(
                "case {case}: the get carries no CachePolicyConfig: {}",
                fetched.raw
            )
        });
        assert_config(config, &policy, "after a refusal", &fetched.raw);
        assert_eq!(
            fetched.header("etag"),
            etag,
            "case {case}: the refused create must not have spent a version token"
        );
    }
}

/// Every value the model's three behaviour enums admit round-trips: the create stores it
/// and both reads report it back, including the values no recorded request named. The
/// catalogs are walked to their full length, so adding a value to the model fails this
/// test rather than going untested.
#[tokio::test]
async fn every_behavior_value_round_trips() {
    let router = emulator();
    let rounds = HEADER_BEHAVIORS
        .len()
        .max(COOKIE_BEHAVIORS.len())
        .max(QUERY_STRING_BEHAVIORS.len());

    for round in 0..rounds {
        let header_behavior = HEADER_BEHAVIORS[round % HEADER_BEHAVIORS.len()];
        let cookie_behavior = COOKIE_BEHAVIORS[round % COOKIE_BEHAVIORS.len()];
        let query_string_behavior = QUERY_STRING_BEHAVIORS[round % QUERY_STRING_BEHAVIORS.len()];
        // A name list beside every behaviour, so a value that ignores its list and one
        // that reads it are both stored with the list the request carried.
        let policy = Policy {
            case: "one behaviour value of each enum",
            name: format!("behaviors-{header_behavior}-{cookie_behavior}-{query_string_behavior}"),
            comment: Some(format!(
                "{header_behavior} / {cookie_behavior} / {query_string_behavior}"
            )),
            min_ttl: 1,
            default_ttl: Some(60),
            max_ttl: Some(600),
            parameters: Some(Parameters {
                gzip: round % 2 == 0,
                brotli: Some(round % 2 == 1),
                header_behavior: header_behavior.to_string(),
                headers: Some(strings(&["x-one", "x-two"])),
                cookie_behavior: cookie_behavior.to_string(),
                cookies: Some(strings(&["one"])),
                query_string_behavior: query_string_behavior.to_string(),
                query_strings: Some(Vec::new()),
            }),
        };

        let created = create_policy(&router, &policy.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {} ({header_behavior}/{cookie_behavior}/{query_string_behavior}): expected \
             HTTP 201, got {} with {}",
            policy.case, created.status, created.raw
        );
        let id = text(created.root(), "Id", policy.case, "the create", &created.raw);

        let fetched = get_policy(&router, &id).await;
        assert_eq!(
            fetched.status, 200,
            "case {} ({header_behavior}/{cookie_behavior}/{query_string_behavior}): expected \
             HTTP 200, got {} with {}",
            policy.case, fetched.status, fetched.raw
        );
        let config = fetched.root().at("CachePolicyConfig").unwrap_or_else(|| {
            panic!(
                "case {}: the get carries no CachePolicyConfig: {}",
                policy.case, fetched.raw
            )
        });
        assert_config(config, &policy, "the get", &fetched.raw);

        let config_answer = get_policy_config(&router, &id).await;
        assert_eq!(
            config_answer.status, 200,
            "case {} ({header_behavior}/{cookie_behavior}/{query_string_behavior}): expected \
             HTTP 200 from the config get, got {} with {}",
            policy.case, config_answer.status, config_answer.raw
        );
        assert_config(
            config_answer.root(),
            &policy,
            "the config get",
            &config_answer.raw,
        );
    }
}

/// Every CloudFront list travels in a wrapper that states its own length, and a request
/// whose `Quantity` disagrees with its `Items` describes no list at all: it is refused with
/// `InconsistentQuantities` (HTTP 400) without storing anything, so the name the refused
/// create named is still free afterwards. One row per wrapper, in both directions.
#[tokio::test]
async fn a_wrapper_whose_quantity_disagrees_with_its_items_is_refused() {
    let router = emulator();

    let rows: [(&str, &str, &str, &str); 6] = [
        (
            "Headers overstated",
            "<Headers><Quantity>2</Quantity><Items><Name>authorization</Name></Items></Headers>",
            "",
            "",
        ),
        (
            "Headers understated",
            "<Headers><Quantity>1</Quantity><Items><Name>accept</Name>\
             <Name>authorization</Name></Items></Headers>",
            "",
            "",
        ),
        (
            "Cookies overstated",
            "",
            "<Cookies><Quantity>3</Quantity><Items><Name>session</Name></Items></Cookies>",
            "",
        ),
        (
            "Cookies understated",
            "",
            "<Cookies><Quantity>0</Quantity><Items><Name>session</Name></Items></Cookies>",
            "",
        ),
        (
            "QueryStrings overstated",
            "",
            "",
            "<QueryStrings><Quantity>1</Quantity></QueryStrings>",
        ),
        (
            "QueryStrings understated",
            "",
            "",
            "<QueryStrings><Quantity>1</Quantity><Items><Name>page</Name><Name>size</Name>\
             </Items></QueryStrings>",
        ),
    ];

    for (case, headers, cookies, query_strings) in rows {
        let name = format!("mismatch-{}", case.replace(' ', "-"));
        let body = format!(
            "<CachePolicyConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <Name>{name}</Name><MinTTL>1</MinTTL>\
             <ParametersInCacheKeyAndForwardedToOrigin>\
             <EnableAcceptEncodingGzip>false</EnableAcceptEncodingGzip>\
             <HeadersConfig><HeaderBehavior>whitelist</HeaderBehavior>{headers}</HeadersConfig>\
             <CookiesConfig><CookieBehavior>whitelist</CookieBehavior>{cookies}</CookiesConfig>\
             <QueryStringsConfig><QueryStringBehavior>whitelist</QueryStringBehavior>\
             {query_strings}</QueryStringsConfig>\
             </ParametersInCacheKeyAndForwardedToOrigin></CachePolicyConfig>"
        );

        let answer = create_policy(&router, &body).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InconsistentQuantities",
            "case {case}: expected InconsistentQuantities, got {}",
            answer.raw
        );

        // Nothing was stored: the name the refused create named is still free, so a
        // well-formed create under it succeeds.
        let repaired = Policy {
            case,
            name: name.clone(),
            min_ttl: 1,
            ..Policy::default()
        };
        let accepted = create_policy(&router, &repaired.xml()).await;
        assert_eq!(
            accepted.status, 201,
            "case {case}: the refused create must store nothing, got {} with {}",
            accepted.status, accepted.raw
        );
    }
}

/// An id no policy holds is `NoSuchCachePolicy` (HTTP 404) from both readers, whatever
/// shape the id takes — and a create under one id leaves every other id unfound.
#[tokio::test]
async fn the_reads_refuse_an_id_no_policy_holds() {
    let router = emulator();
    let present = Policy {
        case: "the one policy that exists",
        name: "present-policy".to_string(),
        min_ttl: 1,
        ..Policy::default()
    };
    let created = create_policy(&router, &present.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201, got {} with {}",
        present.case, created.status, created.raw
    );
    let id = text(created.root(), "Id", present.case, "the create", &created.raw);

    // Rows: a UUID never minted, the existing id with its last character changed, the
    // existing id in upper case, a non-UUID token and the policy's name.
    let mut flipped: Vec<char> = id.chars().collect();
    let last = flipped.len() - 1;
    flipped[last] = if flipped[last] == 'a' { 'b' } else { 'a' };
    let absent: [(&str, String); 5] = [
        (
            "a UUID never minted",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "the existing id with one character changed",
            flipped.into_iter().collect(),
        ),
        ("the existing id in upper case", id.to_uppercase()),
        ("a token that is no UUID", "not-a-policy-id".to_string()),
        ("a policy name rather than an id", present.name.clone()),
    ];

    for (case, candidate) in absent {
        for (reader, answer) in [
            ("GetCachePolicy", get_policy(&router, &candidate).await),
            (
                "GetCachePolicyConfig",
                get_policy_config(&router, &candidate).await,
            ),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {case} ({reader}): expected HTTP 404 for {candidate:?}, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "NoSuchCachePolicy",
                "case {case} ({reader}): expected NoSuchCachePolicy for {candidate:?}, got {}",
                answer.raw
            );
            let message = answer.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                message, NO_SUCH_CACHE_POLICY_MESSAGE,
                "case {case} ({reader}): expected the modelled message, got {message:?}"
            );
        }
    }

    // The policy that does exist is still readable, unchanged by the refusals.
    let fetched = get_policy(&router, &id).await;
    assert_eq!(
        fetched.status, 200,
        "case the existing policy: expected HTTP 200, got {} with {}",
        fetched.status, fetched.raw
    );
}

/// The store's bound on how many policies it holds is an entry count, not a per-request
/// one: the table's creates all land well inside [`MAX_CACHE_POLICIES`], so each of them is
/// accepted and every one of them stays readable — a create never evicts another.
#[tokio::test]
async fn creates_inside_the_store_bound_are_all_accepted() {
    let router = emulator();
    let table = policy_table();
    assert!(
        table.len() < MAX_CACHE_POLICIES,
        "case the store bound: the table must sit inside the bound {MAX_CACHE_POLICIES}, got {} \
         rows",
        table.len()
    );

    let mut ids = Vec::new();
    for policy in &table {
        let created = create_policy(&router, &policy.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {}: expected HTTP 201, got {} with {}",
            policy.case, created.status, created.raw
        );
        ids.push(text(
            created.root(),
            "Id",
            policy.case,
            "the create",
            &created.raw,
        ));
    }

    for (policy, id) in table.iter().zip(&ids) {
        let fetched = get_policy(&router, id).await;
        assert_eq!(
            fetched.status, 200,
            "case {}: expected HTTP 200 for {id}, got {} with {}",
            policy.case, fetched.status, fetched.raw
        );
        let config = fetched.root().at("CachePolicyConfig").unwrap_or_else(|| {
            panic!(
                "case {}: the get carries no CachePolicyConfig: {}",
                policy.case, fetched.raw
            )
        });
        assert_config(config, policy, "the get after the whole table", &fetched.raw);
    }
}
