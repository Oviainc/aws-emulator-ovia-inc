// SPDX-License-Identifier: CC0-1.0

//! The CloudFront origin request policy control plane, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront's origin request policy
//! surface states:
//!
//! * a policy is PERSISTED by the call that creates it, which MINTS its identity (a UUID
//!   `Id` and a version token), so a later `GetOriginRequestPolicy` answers from that
//!   record rather than from the request being served — down to the members the create
//!   left out;
//! * `OriginRequestPolicyConfig$Name` is "A unique name to identify the origin request
//!   policy", so a repeated name is refused with `OriginRequestPolicyAlreadyExists` and
//!   leaves the policy that holds it untouched;
//! * every CloudFront list travels in a wrapper that states its own length, and a request
//!   whose `Quantity` disagrees with its `Items` is refused with `InconsistentQuantities`
//!   and stores nothing;
//! * an id no policy holds is `NoSuchOriginRequestPolicy`, whatever shape the id takes.
//!
//! The table rows differ along everything these rules generalise over: which of the three
//! optional name lists the policy carries at all, each list's length (none, one, many), the
//! enum values each behaviour member draws from (every value the model's
//! `OriginRequestPolicyHeaderBehavior`, `OriginRequestPolicyCookieBehavior` and
//! `OriginRequestPolicyQueryStringBehavior` admit, from the crate's model-derived
//! catalogs), and whether a text member is ASCII, empty, non-ASCII or carries XML
//! metacharacters.

use std::collections::BTreeSet;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::ops::types::{
    ORIGIN_REQUEST_POLICY_COOKIE_BEHAVIOR_VALUES, ORIGIN_REQUEST_POLICY_HEADER_BEHAVIOR_VALUES,
    ORIGIN_REQUEST_POLICY_QUERY_STRING_BEHAVIOR_VALUES,
};
use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::MAX_ORIGIN_REQUEST_POLICIES;
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, emulator, is_version_token, Answer, Node, API_VERSION, HOST,
};
use crate::response_headers_policy_lifecycle::{assert_absent, escape, is_policy_id, text};

// ---------------------------------------------------------------------------
// Calling the origin request policy surface
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/origin-request-policy` with the `OriginRequestPolicyConfig` document
/// `body` — the member the operation binds to the HTTP payload.
async fn create_policy(router: &MultiServiceRouter, body: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/origin-request-policy"),
        Some(body),
        None,
    )
    .await
}

/// `GET /2020-05-31/origin-request-policy/{Id}`.
async fn get_policy(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/origin-request-policy/{id}"),
        None,
        None,
    )
    .await
}

// ---------------------------------------------------------------------------
// The configurations under test
// ---------------------------------------------------------------------------

/// One `OriginRequestPolicyConfig` a client submits.
///
/// A `None` list means the wrapper is left out of the document entirely, which is what
/// makes "the record reports what was submitted" testable for absent members too;
/// `Some(vec![])` is the wrapper carrying a `Quantity` of zero and no items.
#[derive(Debug, Clone)]
struct Policy {
    case: &'static str,
    name: String,
    comment: Option<String>,
    /// A value of `OriginRequestPolicyHeaderBehavior`.
    header_behavior: String,
    headers: Option<Vec<String>>,
    /// A value of `OriginRequestPolicyCookieBehavior`.
    cookie_behavior: String,
    cookies: Option<Vec<String>>,
    /// A value of `OriginRequestPolicyQueryStringBehavior`.
    query_string_behavior: String,
    query_strings: Option<Vec<String>>,
}

impl Default for Policy {
    /// The policy that forwards nothing beyond what CloudFront always sends: `none` for
    /// each of the three behaviours — a value every one of the three model-derived
    /// catalogs below admits — and no name list at all.
    fn default() -> Self {
        Self {
            case: "",
            name: String::new(),
            comment: None,
            header_behavior: "none".to_string(),
            headers: None,
            cookie_behavior: "none".to_string(),
            cookies: None,
            query_string_behavior: "none".to_string(),
            query_strings: None,
        }
    }
}

impl Policy {
    /// The `OriginRequestPolicyConfig` XML a client sends for this configuration, in the
    /// model's member declaration order.
    fn xml(&self) -> String {
        let mut out = format!(
            "<OriginRequestPolicyConfig \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">"
        );
        if let Some(comment) = &self.comment {
            out.push_str(&format!("<Comment>{}</Comment>", escape(comment)));
        }
        out.push_str(&format!("<Name>{}</Name>", escape(&self.name)));
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
        out.push_str("</OriginRequestPolicyConfig>");
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

/// Every value `OriginRequestPolicyHeaderBehavior` admits. Taken from the crate's own
/// model-derived catalog (`ops::types::ORIGIN_REQUEST_POLICY_HEADER_BEHAVIOR_VALUES`, which
/// `smithy_query(query="shape",shape="OriginRequestPolicyHeaderBehavior")` reports the same
/// values for) rather than written out here, so the catalog cannot drift from the model.
const HEADER_BEHAVIORS: &[&str] = ORIGIN_REQUEST_POLICY_HEADER_BEHAVIOR_VALUES;

/// Every value `OriginRequestPolicyCookieBehavior` admits, from the same model-derived
/// catalog.
const COOKIE_BEHAVIORS: &[&str] = ORIGIN_REQUEST_POLICY_COOKIE_BEHAVIOR_VALUES;

/// Every value `OriginRequestPolicyQueryStringBehavior` admits, from the same model-derived
/// catalog.
const QUERY_STRING_BEHAVIORS: &[&str] = ORIGIN_REQUEST_POLICY_QUERY_STRING_BEHAVIOR_VALUES;

/// A list of owned strings from string literals, for the table below.
fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

/// The configurations every rule below is asserted over.
///
/// The rows span the whole class the rules generalise over: no optional member at all, a
/// comment that is empty, ASCII or non-ASCII, each of the three name lists absent, carrying
/// no items, one item and many, behaviours drawn from every value of the three model-derived
/// catalogs, and names carrying XML metacharacters.
fn policy_table() -> Vec<Policy> {
    vec![
        Policy {
            case: "the name alone: no comment and no name list",
            name: "name-only".to_string(),
            ..Policy::default()
        },
        Policy {
            case: "an empty comment and all three wrappers carrying no items",
            name: "empty-wrappers".to_string(),
            comment: Some(String::new()),
            headers: Some(Vec::new()),
            cookies: Some(Vec::new()),
            query_strings: Some(Vec::new()),
            ..Policy::default()
        },
        Policy {
            case: "whitelist of exactly one header, cookie and query string",
            name: "whitelist-singletons".to_string(),
            comment: Some("one of each".to_string()),
            header_behavior: "whitelist".to_string(),
            headers: Some(strings(&["authorization"])),
            cookie_behavior: "whitelist".to_string(),
            cookies: Some(strings(&["session"])),
            query_string_behavior: "whitelist".to_string(),
            query_strings: Some(strings(&["page"])),
        },
        Policy {
            case: "whitelist of many, with lists of different lengths",
            name: "whitelist-many".to_string(),
            comment: Some("プロダクション — forwards a & b < c".to_string()),
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
        },
        Policy {
            case: "allViewer headers, allExcept cookies, all query strings",
            name: "mixed-behaviors".to_string(),
            header_behavior: "allViewer".to_string(),
            headers: None,
            cookie_behavior: "allExcept".to_string(),
            cookies: Some(strings(&["tracking"])),
            query_string_behavior: "all".to_string(),
            query_strings: None,
            ..Policy::default()
        },
        Policy {
            case: "allViewerAndWhitelistCloudFront with the CloudFront headers listed",
            name: "all-viewer-and-cloudfront".to_string(),
            comment: Some("adds the CloudFront geo headers".to_string()),
            header_behavior: "allViewerAndWhitelistCloudFront".to_string(),
            headers: Some(strings(&[
                "CloudFront-Viewer-Country",
                "CloudFront-Is-Mobile-Viewer",
                "CloudFront-Forwarded-Proto",
            ])),
            cookie_behavior: "all".to_string(),
            cookies: None,
            query_string_behavior: "allExcept".to_string(),
            query_strings: Some(strings(&["utm_source", "utm_medium"])),
        },
        Policy {
            case: "allExcept on all three, each with one exclusion",
            name: "all-except-everything".to_string(),
            header_behavior: "allExcept".to_string(),
            headers: Some(strings(&["x-internal"])),
            cookie_behavior: "allExcept".to_string(),
            cookies: Some(strings(&["debug"])),
            query_string_behavior: "allExcept".to_string(),
            query_strings: Some(strings(&["debug"])),
            comment: None,
        },
    ]
}

// ---------------------------------------------------------------------------
// Reading what the service stored
// ---------------------------------------------------------------------------

/// Assert the `OriginRequestPolicyConfig` element `config` reports exactly what `policy`
/// submitted: every member it carried, and no element for a member it left out.
fn assert_config(config: &Node, policy: &Policy, context: &str, raw: &str) {
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

    let behavior = text(config, "HeadersConfig/HeaderBehavior", case, context, raw);
    assert_eq!(
        behavior, policy.header_behavior,
        "case {case} ({context}): expected HeaderBehavior {:?}, got {behavior:?}",
        policy.header_behavior
    );
    assert_list(
        config,
        "HeadersConfig",
        "Headers",
        policy.headers.as_deref(),
        case,
        context,
        raw,
    );

    let behavior = text(config, "CookiesConfig/CookieBehavior", case, context, raw);
    assert_eq!(
        behavior, policy.cookie_behavior,
        "case {case} ({context}): expected CookieBehavior {:?}, got {behavior:?}",
        policy.cookie_behavior
    );
    assert_list(
        config,
        "CookiesConfig",
        "Cookies",
        policy.cookies.as_deref(),
        case,
        context,
        raw,
    );

    let behavior = text(
        config,
        "QueryStringsConfig/QueryStringBehavior",
        case,
        context,
        raw,
    );
    assert_eq!(
        behavior, policy.query_string_behavior,
        "case {case} ({context}): expected QueryStringBehavior {:?}, got {behavior:?}",
        policy.query_string_behavior
    );
    assert_list(
        config,
        "QueryStringsConfig",
        "QueryStrings",
        policy.query_strings.as_deref(),
        case,
        context,
        raw,
    );
}

/// Assert one name list wrapper under `parent` reports `values` in order beside a
/// `Quantity` that states their number — or is absent entirely when the request left it
/// out.
fn assert_list(
    config: &Node,
    parent: &str,
    list: &str,
    values: Option<&[String]>,
    case: &str,
    context: &str,
    raw: &str,
) {
    let path = format!("{parent}/{list}");
    let Some(values) = values else {
        assert_absent(config, &path, case, context);
        return;
    };

    let quantity = text(config, &format!("{path}/Quantity"), case, context, raw);
    assert_eq!(
        quantity,
        values.len().to_string(),
        "case {case} ({context}): expected {path}.Quantity {}, got {quantity:?}",
        values.len()
    );
    let items: Vec<String> = config
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

/// A create PERSISTS the policy it mints, and the read answers from that one record: the
/// configuration exactly as submitted (down to the members the request left out), a UUID
/// id, and the version token the create handed back. Across the table, no two policies
/// share an id or a version token, and each one's `Location` names its own URI.
#[tokio::test]
async fn create_persists_the_policy_the_get_then_reports() {
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
        let config = document
            .at("OriginRequestPolicyConfig")
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: the create carries no OriginRequestPolicyConfig: {}",
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
            format!("https://{HOST}/{API_VERSION}/origin-request-policy/{id}"),
            "case {case}: expected the new policy's URI in Location, got {location:?}"
        );
        assert!(ids.insert(id.clone()), "case {case}: the id {id} repeated");
        assert!(
            etags.insert(etag.clone()),
            "case {case}: the version token {etag} repeated"
        );

        // The read answers from the stored record, not from any request.
        let fetched = get_policy(&router, &id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected HTTP 200 from the get, got {} with {}",
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
        let fetched_config = fetched_document
            .at("OriginRequestPolicyConfig")
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: the get carries no OriginRequestPolicyConfig: {}",
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
    }
}

/// `OriginRequestPolicyConfig$Name` is "A unique name to identify the origin request
/// policy": the second create under a name is refused with
/// `OriginRequestPolicyAlreadyExists` (HTTP 409), and the policy that holds the name keeps
/// the identity, the configuration and the version token its own create minted.
#[tokio::test]
async fn a_name_names_at_most_one_origin_request_policy() {
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
            header_behavior: "whitelist".to_string(),
            headers: Some(strings(&["x-second"])),
            cookie_behavior: "none".to_string(),
            cookies: None,
            query_string_behavior: "none".to_string(),
            query_strings: None,
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
            "OriginRequestPolicyAlreadyExists",
            "case {case}: expected OriginRequestPolicyAlreadyExists, got {}",
            second.raw
        );

        let fetched = get_policy(&router, &id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected the first policy to survive the refusal, got {} with {}",
            fetched.status, fetched.raw
        );
        let config = fetched
            .root()
            .at("OriginRequestPolicyConfig")
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: the get carries no OriginRequestPolicyConfig: {}",
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
/// and the get reports it back, including the values no recorded request named. The
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
        let query_string_behavior =
            QUERY_STRING_BEHAVIORS[round % QUERY_STRING_BEHAVIORS.len()];
        // A name list beside every behaviour, so a value that ignores its list and one
        // that reads it are both stored with the list the request carried.
        let policy = Policy {
            case: "one behaviour value of each enum",
            name: format!("behaviors-{header_behavior}-{cookie_behavior}-{query_string_behavior}"),
            comment: Some(format!(
                "{header_behavior} / {cookie_behavior} / {query_string_behavior}"
            )),
            header_behavior: header_behavior.to_string(),
            headers: Some(strings(&["x-one", "x-two"])),
            cookie_behavior: cookie_behavior.to_string(),
            cookies: Some(strings(&["one"])),
            query_string_behavior: query_string_behavior.to_string(),
            query_strings: Some(Vec::new()),
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
        let config = fetched
            .root()
            .at("OriginRequestPolicyConfig")
            .unwrap_or_else(|| {
                panic!(
                    "case {}: the get carries no OriginRequestPolicyConfig: {}",
                    policy.case, fetched.raw
                )
            });
        assert_config(config, &policy, "the get", &fetched.raw);
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
            "<OriginRequestPolicyConfig \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\"><Name>{name}</Name>\
             <HeadersConfig><HeaderBehavior>whitelist</HeaderBehavior>{headers}</HeadersConfig>\
             <CookiesConfig><CookieBehavior>whitelist</CookieBehavior>{cookies}</CookiesConfig>\
             <QueryStringsConfig><QueryStringBehavior>whitelist</QueryStringBehavior>\
             {query_strings}</QueryStringsConfig></OriginRequestPolicyConfig>"
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

/// An id no policy holds is `NoSuchOriginRequestPolicy` (HTTP 404), whatever shape the id
/// takes — and a create under one id leaves every other id unfound.
#[tokio::test]
async fn get_refuses_an_id_no_policy_holds() {
    let router = emulator();
    let present = Policy {
        case: "the one policy that exists",
        name: "present-policy".to_string(),
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
        let answer = get_policy(&router, &candidate).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404 for {candidate:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchOriginRequestPolicy",
            "case {case}: expected NoSuchOriginRequestPolicy for {candidate:?}, got {}",
            answer.raw
        );
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
/// one: the table's creates all land well inside [`MAX_ORIGIN_REQUEST_POLICIES`], so each of
/// them is accepted and every one of them stays readable — a create never evicts another.
#[tokio::test]
async fn creates_inside_the_store_bound_are_all_accepted() {
    let router = emulator();
    let table = policy_table();
    assert!(
        table.len() < MAX_ORIGIN_REQUEST_POLICIES,
        "case the store bound: the table must sit inside the bound \
         {MAX_ORIGIN_REQUEST_POLICIES}, got {} rows",
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
        let config = fetched
            .root()
            .at("OriginRequestPolicyConfig")
            .unwrap_or_else(|| {
                panic!(
                    "case {}: the get carries no OriginRequestPolicyConfig: {}",
                    policy.case, fetched.raw
                )
            });
        assert_config(config, policy, "the get after the whole table", &fetched.raw);
    }
}
