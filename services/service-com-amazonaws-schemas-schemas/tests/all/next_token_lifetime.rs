// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what a `NextToken` of this service resumes, and for how
//! long.
//!
//! Two rules, both of the pagination machinery every listing of this service shares:
//!
//! 1. A token resumes its listing for the window the model documents for every
//!    `NextToken` member here and no longer: it carries the instant it was issued at,
//!    and a request that presents it once that window has passed is refused rather
//!    than served from a position of an arbitrarily old listing.
//! 2. A token names a position within ONE listing, so it resumes only the listing and
//!    the selection it was issued for. Which listing a request selects is decided by
//!    the VALUES of its selecting members, so two requests that differ in any of them
//!    are two listings — whatever characters those values hold, including the ones a
//!    rendering of the selection separates its fields with.
//!
//! Rule 1 is asserted over a table of token AGES (both sides of the published bound,
//! inside it, far past it) crossed with two different listings; rule 2 over a table of
//! selection PAIRS that differ only in which member holds which part of the value —
//! the pairs an unescaped rendering cannot tell apart.
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`, exactly as the production router serves it. The HTTP
//! harness is the crate's one harness, imported from `registry_schema_lifecycle`.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_schemas_schemas::state::{
    encode_next_token, selection_fingerprint, NEXT_TOKEN_TTL,
};

use crate::discoverer_state::{source_arn, DISCOVERERS_URI};
use crate::registry_schema_lifecycle::{
    create_registry, schema_names, schema_uri, Answer, Emulator, ACCOUNT, REGION,
};
use crate::resource_tags::percent_encode;

/// `ListDiscoverers`' route.
const LIST_DISCOVERERS_URI: &str = "/v1/discoverers";

/// The registry the schema-listing rows below page over.
const REGISTRY: &str = "token-lifetime";

/// The `DiscovererId`s one listing answer reports, in the order it lists them.
fn discoverer_ids(answer: &Answer) -> Vec<String> {
    answer.body["Discoverers"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a Discoverers array in {:?}", answer.body))
        .iter()
        .map(|item| item["DiscovererId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Append `query` to a route that may already carry one.
fn with_query(uri: &str, query: &str) -> String {
    let separator = if uri.contains('?') { '&' } else { '?' };
    format!("{uri}{separator}{query}")
}

/// Create a discoverer for one source ARN, asserting the create succeeded, and return
/// its minted `DiscovererId`. The sibling helper builds the ARN from a bus name; these
/// cases need ARNs whose resource part carries punctuation of their own.
async fn create_for_source(emulator: &Emulator, case: &str, source: &str) -> String {
    let created = emulator
        .call("POST", DISCOVERERS_URI, Some(json!({"SourceArn": source})))
        .await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: expected 201 from CreateDiscoverer for {source}, got {} {:?}",
        created.status, created.body
    );
    created.str_at("DiscovererId")
}

/// Assert `answer` is the refusal an unusable `NextToken` earns, and that it does not
/// echo the token back.
fn assert_token_refused(case: &str, answer: &Answer, token: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected 400, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "BadRequestException",
        "case {case:?}: expected BadRequestException, got {:?}",
        answer.body
    );
    let message = answer
        .body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        !message.contains(token),
        "case {case:?}: the refusal must not echo the token back, got {message:?}"
    );
}

/// One listing a token is aged against: how its handler fingerprints its selection,
/// which key a token resumes after, and what the page after that key holds.
#[derive(Debug)]
struct AgedListing {
    case: &'static str,
    /// The route, carrying the listing's selection and a page size of one.
    uri: String,
    /// The selection fingerprint the listing's handler binds into its tokens.
    selection: String,
    /// The paging key a minted token resumes strictly after.
    resume_after: String,
    /// The one item the listing serves after that key.
    expected: Vec<String>,
    /// How the items of a page of this listing are read out of the answer.
    items: fn(&Answer) -> Vec<String>,
}

/// RULE: a `NextToken` resumes its listing for as long as it lives — the window the
/// model documents, measured from the instant the listing issued it — and a request
/// that presents it afterwards is refused, not served.
#[tokio::test]
async fn a_next_token_resumes_its_listing_only_until_it_expires() {
    let emulator = Emulator::new();

    // Two discoverers, so a page of one has a position to resume after.
    let mut discoverers = [
        create_for_source(&emulator, "aged", &source_arn("aged-one")).await,
        create_for_source(&emulator, "aged", &source_arn("aged-two")).await,
    ];
    discoverers.sort();

    // And two schemas in one registry, so the same rule is asserted on a second
    // listing with a paging key of its own (the schema-name collation, not the id).
    create_registry(&emulator, REGISTRY).await;
    let schemas = ["alpha-schema", "beta-schema"];
    for schema in schemas {
        let created = emulator
            .call(
                "POST",
                &schema_uri(REGISTRY, schema),
                Some(json!({"Content": "{}", "Type": "OpenApi3"})),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {schema:?}: expected 201 from CreateSchema, got {} {:?}",
            created.status, created.body
        );
    }

    let listings = vec![
        AgedListing {
            case: "ListDiscoverers, no selecting member",
            uri: with_query(LIST_DISCOVERERS_URI, "limit=1"),
            selection: selection_fingerprint(
                "ListDiscoverers",
                &[("discovererIdPrefix", ""), ("sourceArnPrefix", "")],
            ),
            resume_after: discoverers[0].clone(),
            expected: vec![discoverers[1].clone()],
            items: discoverer_ids,
        },
        AgedListing {
            case: "ListSchemas, one registry",
            uri: with_query(&format!("/v1/registries/name/{REGISTRY}/schemas"), "limit=1"),
            selection: selection_fingerprint(
                "ListSchemas",
                &[("registryName", REGISTRY), ("schemaNamePrefix", "")],
            ),
            // A listing of schemas resumes after a schema NAME: the order a listing
            // is served in is the codepoint order of those names, so the name is the
            // key a token carries.
            resume_after: schemas[0].to_string(),
            expected: vec![schemas[1].to_string()],
            items: schema_names,
        },
    ];

    for listing in &listings {
        // The listing's OWN token resumes it: the page this request is served proves
        // the token a case mints below is the token this handler issues.
        let first = emulator.call("GET", &listing.uri, None).await;
        assert_eq!(
            first.status, 200,
            "case {:?}: expected 200, got {} {:?}",
            listing.case, first.status, first.body
        );
        let issued = first
            .body
            .get("NextToken")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {:?}: expected a NextToken", listing.case))
            .to_string();
        let resumed = emulator
            .call("GET", &with_query(&listing.uri, &format!("nextToken={issued}")), None)
            .await;
        assert_eq!(
            resumed.status, 200,
            "case {:?}: the listing's own fresh token must resume it, got {} {:?}",
            listing.case, resumed.status, resumed.body
        );
        assert_eq!(
            (listing.items)(&resumed),
            listing.expected,
            "case {:?}: a fresh token resumes after the page it was issued for",
            listing.case
        );

        // Rows: the instant of issue, inside the window, each side of the published
        // bound, and far past it.
        for (age_case, age, resumes) in [
            ("at the instant it was issued", Duration::ZERO, true),
            ("half the window old", NEXT_TOKEN_TTL / 2, true),
            (
                "a minute before the window closes",
                NEXT_TOKEN_TTL - Duration::from_secs(60),
                true,
            ),
            ("exactly the whole window old", NEXT_TOKEN_TTL, false),
            (
                "a minute past the window",
                NEXT_TOKEN_TTL + Duration::from_secs(60),
                false,
            ),
            (
                "a week past the window",
                NEXT_TOKEN_TTL + Duration::from_secs(7 * 24 * 60 * 60),
                false,
            ),
        ] {
            let case = format!("{}: {age_case}", listing.case);
            let token = encode_next_token(
                &listing.selection,
                &listing.resume_after,
                SystemTime::now() - age,
            );
            let answer = emulator
                .call(
                    "GET",
                    &with_query(&listing.uri, &format!("nextToken={token}")),
                    None,
                )
                .await;
            if resumes {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: a token inside its window must resume the listing, \
                     got {} {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    (listing.items)(&answer),
                    listing.expected,
                    "case {case:?}: it resumes after the key it names"
                );
            } else {
                assert_token_refused(&case, &answer, &token);
            }
        }
    }
}

/// One pair of selections that an unescaped rendering of the selecting members cannot
/// tell apart: the token is issued by `issuer` and presented to `impostor`.
#[derive(Debug)]
struct SelectionPair {
    case: &'static str,
    /// The query of the listing that issues the token; it selects more than one
    /// discoverer, so it has a page to resume.
    issuer: String,
    /// The query of a DIFFERENT listing, built by moving part of one member's value
    /// into the other member.
    impostor: String,
}

/// RULE: a token resumes only the selection it was issued for, so a request whose
/// selecting members hold different values is a different listing and is refused —
/// including when one member's value holds the text a rendering of the selection would
/// separate its members with, which is the one case an unescaped rendering gets wrong.
#[tokio::test]
async fn a_token_does_not_resume_a_selection_that_only_looks_like_its_own() {
    let emulator = Emulator::new();

    // Neither prefix member is constrained in any way (both are plain query strings
    // the model gives no pattern), and neither is a `SourceArn` beyond the ARN form —
    // so a stored source really can carry the text below.
    let bus = format!("arn:aws:events:{REGION}:{ACCOUNT}:event-bus/amb");
    let shared = format!("{bus};sourceArnPrefix=b");
    for suffix in ["-one", "-two"] {
        create_for_source(&emulator, "ambiguous source", &format!("{shared}{suffix}")).await;
    }
    // Plus two discoverers with ordinary sources, so the unselected listing is not
    // empty either.
    for bus in ["plain-one", "plain-two"] {
        create_for_source(&emulator, "plain source", &source_arn(bus)).await;
    }

    // Both rows move the text `;sourceArnPrefix=` across the boundary between the two
    // members: the issuing listing holds it inside `SourceArnPrefix`, the impostor
    // inside `DiscovererIdPrefix`.
    let pairs = vec![
        SelectionPair {
            case: "the separator inside the source prefix, then inside the id prefix",
            issuer: format!("sourceArnPrefix={}", percent_encode(&shared)),
            impostor: format!(
                "discovererIdPrefix={}&sourceArnPrefix=b",
                percent_encode(&format!(";sourceArnPrefix={bus}"))
            ),
        },
        SelectionPair {
            case: "a source prefix ending in the separator, against an absent member",
            issuer: format!(
                "sourceArnPrefix={}",
                percent_encode(&format!("{bus};sourceArnPrefix="))
            ),
            impostor: format!(
                "discovererIdPrefix={}",
                percent_encode(&format!(";sourceArnPrefix={bus}"))
            ),
        },
    ];

    for pair in &pairs {
        let issued = emulator
            .call(
                "GET",
                &with_query(LIST_DISCOVERERS_URI, &format!("{}&limit=1", pair.issuer)),
                None,
            )
            .await;
        assert_eq!(
            issued.status, 200,
            "case {:?}: expected 200 from the issuing listing, got {} {:?}",
            pair.case, issued.status, issued.body
        );
        let page = discoverer_ids(&issued);
        assert_eq!(
            page.len(),
            1,
            "case {:?}: the issuing listing must serve a page of one, got {:?}",
            pair.case,
            issued.body
        );
        let token = issued
            .body
            .get("NextToken")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the issuing listing must have more to give, got {:?}",
                    pair.case, issued.body
                )
            })
            .to_string();

        // Its own selection resumes it: the refusal below is about the selection, not
        // about the token.
        let own = emulator
            .call(
                "GET",
                &with_query(
                    LIST_DISCOVERERS_URI,
                    &format!("{}&limit=1&nextToken={token}", pair.issuer),
                ),
                None,
            )
            .await;
        assert_eq!(
            own.status, 200,
            "case {:?}: the token's own selection must resume, got {} {:?}",
            pair.case, own.status, own.body
        );
        let resumed = discoverer_ids(&own);
        assert_eq!(
            resumed.len(),
            1,
            "case {:?}: the resumed page holds the next match, got {:?}",
            pair.case,
            own.body
        );
        assert_ne!(
            resumed, page,
            "case {:?}: resuming must not repeat the page before it",
            pair.case
        );

        // The impostor selection is a different listing, so the token resumes nothing
        // there.
        let answer = emulator
            .call(
                "GET",
                &with_query(
                    LIST_DISCOVERERS_URI,
                    &format!("{}&limit=1&nextToken={token}", pair.impostor),
                ),
                None,
            )
            .await;
        assert_token_refused(pair.case, &answer, &token);
    }
}

/// RULE: a token names a position in one LISTING, so a token one operation issued does
/// not resume another operation's listing of the same resources.
#[tokio::test]
async fn a_token_does_not_resume_another_listing_of_the_same_resources() {
    let emulator = Emulator::new();
    create_registry(&emulator, REGISTRY).await;
    for schema in ["alpha-schema", "beta-schema"] {
        let created = emulator
            .call(
                "POST",
                &schema_uri(REGISTRY, schema),
                Some(json!({"Content": "{}", "Type": "OpenApi3"})),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {schema:?}: expected 201 from CreateSchema, got {} {:?}",
            created.status, created.body
        );
    }

    let listed = emulator
        .call("GET", &format!("/v1/registries/name/{REGISTRY}/schemas?limit=1"), None)
        .await;
    assert_eq!(
        listed.status, 200,
        "expected 200 from ListSchemas, got {} {:?}",
        listed.status, listed.body
    );
    let token = listed
        .body
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a NextToken in {:?}", listed.body))
        .to_string();

    // A search of the same registry reports the same schemas in the same order, and
    // still will not resume a listing's token.
    for (case, uri) in [
        (
            "a schema listing's token presented to a search of that registry",
            format!(
                "/v1/registries/name/{REGISTRY}/schemas/search?keywords=&limit=1&nextToken={token}"
            ),
        ),
        (
            "a schema listing's token presented to a version listing",
            format!(
                "{}?limit=1&nextToken={token}",
                crate::registry_schema_lifecycle::versions_uri(REGISTRY, "alpha-schema")
            ),
        ),
    ] {
        let answer = emulator.call("GET", &uri, None).await;
        assert_token_refused(case, &answer, &token);
    }
}
