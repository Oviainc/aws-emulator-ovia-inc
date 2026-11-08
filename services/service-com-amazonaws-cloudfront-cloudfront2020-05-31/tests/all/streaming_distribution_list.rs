// SPDX-License-Identifier: CC0-1.0

//! The CloudFront streaming (RTMP) distribution surface, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront states for this surface:
//!
//! * a streaming distribution is PERSISTED by the call that creates it, which MINTS its
//!   identity (an `Id`, an ARN, a domain name and a version token), so every later read
//!   answers from that record: `ListStreamingDistributions` reports one
//!   `StreamingDistributionSummary` per stored distribution carrying exactly what
//!   `GetStreamingDistribution` answers from the same record;
//! * `MaxItems` ("the value that you provided for the MaxItems request parameter") cuts the
//!   page and is echoed as asked for, `IsTruncated` and `NextMarker` say exactly when
//!   distributions remain and `NextMarker` names the id the page ended at, and a request
//!   resumes AFTER the distribution its `Marker` names — so walking the pages at ANY
//!   admissible page size enumerates every distribution exactly once, in one order;
//! * a page size that names no page, and a `Marker` that is no resume point this service
//!   handed out, are refused with the error the operation models (`InvalidArgument`, 400)
//!   and change nothing;
//! * a distribution CloudFront has just accepted is `InProgress`, and the CLOCK — not a
//!   later call — owes it the terminal `Deployed`, which a listing reports;
//! * a listing is a READ: it answers the same records again and mints nothing.
//!
//! The table rows differ along everything these rules generalise over: the number of
//! distributions stored (none, one, several), the page size (one, several, the collection
//! size, past it and past the server's own ceiling), the resume point, how many alternate
//! domain names and trusted signers a configuration states (none, one, several, the
//! documented signer bound), which optional members it carries at all (`Aliases`,
//! `Logging`, `PriceClass`), every value `PriceClass` admits (from `smithy_query` on
//! `PriceClass`), both booleans of `Enabled`, and the form of the comment each summary
//! carries (absent-as-empty, ASCII, XML-significant characters, non-ASCII).
//!
//! The harness (the router, the request helpers and the element scanner) is the
//! distribution lifecycle suite's, reused rather than copied; the XML escaper is the
//! response headers policy suite's, for the same reason.

use std::collections::BTreeSet;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    DEFAULT_PRICE_CLASS, INVALID_STREAMING_DISTRIBUTION_MARKER_MESSAGE, MAX_ITEMS,
    MAX_ITEMS_INVALID_MESSAGE, MAX_TRUSTED_SIGNERS, STATUS_DEPLOYED, STATUS_IN_PROGRESS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, emulator, past_propagation, Answer, Node, ACCOUNT_ID, API_VERSION,
};
use crate::response_headers_policy_lifecycle::escape;

// ---------------------------------------------------------------------------
// Calling the surface
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/streaming-distribution` with the configuration document `body` — the
/// member the create binds to the HTTP payload.
async fn create_streaming_distribution(router: &MultiServiceRouter, body: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/streaming-distribution"),
        Some(body),
        None,
    )
    .await
}

/// `GET /2020-05-31/streaming-distribution/{Id}`.
async fn get_streaming_distribution(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/streaming-distribution/{id}"),
        None,
        None,
    )
    .await
}

/// `GET /2020-05-31/streaming-distribution`, with `query` appended verbatim
/// (`?MaxItems=2&Marker=E…`), from the operation's `http` binding.
async fn list_streaming_distributions(router: &MultiServiceRouter, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/streaming-distribution{query}"),
        None,
        None,
    )
    .await
}

// ---------------------------------------------------------------------------
// The distributions under test
// ---------------------------------------------------------------------------

/// One `StreamingDistributionConfig` a client submits.
#[derive(Debug, Clone)]
struct Row {
    case: String,
    caller_reference: String,
    /// `S3Origin$DomainName` — the bucket the media files are fetched from.
    bucket: String,
    /// `S3Origin$OriginAccessIdentity`, which "If you don't want to require signed URLs …
    /// specify an empty `OriginAccessIdentity` element".
    origin_access_identity: String,
    /// The alternate domain names (`Aliases`) the configuration states, or `None` when it
    /// carries no `Aliases` element at all.
    aliases: Option<Vec<String>>,
    comment: String,
    /// `Logging`, or `None` when the configuration carries no `Logging` element at all.
    logging: Option<(bool, String, String)>,
    /// The Amazon Web Services accounts `TrustedSigners` names.
    signers: Vec<String>,
    /// `PriceClass`, or `None` when the configuration states none.
    price_class: Option<&'static str>,
    enabled: bool,
}

impl Row {
    /// The configuration document, which IS the create's request body (its `httpPayload`
    /// member), with the members in the order the model declares them.
    fn xml(&self) -> String {
        let mut document = format!(
            "<StreamingDistributionConfig \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <CallerReference>{}</CallerReference>\
             <S3Origin><DomainName>{}</DomainName>\
             <OriginAccessIdentity>{}</OriginAccessIdentity></S3Origin>",
            escape(&self.caller_reference),
            escape(&self.bucket),
            escape(&self.origin_access_identity)
        );
        if let Some(aliases) = &self.aliases {
            document.push_str(&wrapper("Aliases", "CNAME", aliases, None));
        }
        document.push_str(&format!("<Comment>{}</Comment>", escape(&self.comment)));
        if let Some((enabled, bucket, prefix)) = &self.logging {
            document.push_str(&format!(
                "<Logging><Enabled>{enabled}</Enabled><Bucket>{}</Bucket>\
                 <Prefix>{}</Prefix></Logging>",
                escape(bucket),
                escape(prefix)
            ));
        }
        document.push_str(&wrapper(
            "TrustedSigners",
            "AwsAccountNumber",
            &self.signers,
            Some(!self.signers.is_empty()),
        ));
        if let Some(price_class) = self.price_class {
            document.push_str(&format!("<PriceClass>{price_class}</PriceClass>"));
        }
        document.push_str(&format!(
            "<Enabled>{}</Enabled></StreamingDistributionConfig>",
            self.enabled
        ));
        document
    }

    /// The `PriceClass` the stored configuration runs under: the one submitted, or the
    /// service's own default where the configuration states none.
    fn expected_price_class(&self) -> String {
        self.price_class
            .unwrap_or(DEFAULT_PRICE_CLASS)
            .to_string()
    }

    /// The alternate domain names the stored configuration runs under: naming no `Aliases`
    /// element at all is naming none.
    fn expected_aliases(&self) -> Vec<String> {
        self.aliases.clone().unwrap_or_default()
    }
}

/// One CloudFront list wrapper: its own length (`Quantity`) beside the list itself
/// (`Items`), with an `Enabled` flag in front where the wrapper carries one.
fn wrapper(name: &str, item: &str, values: &[String], enabled: Option<bool>) -> String {
    let mut out = format!("<{name}>");
    if let Some(enabled) = enabled {
        out.push_str(&format!("<Enabled>{enabled}</Enabled>"));
    }
    out.push_str(&format!("<Quantity>{}</Quantity>", values.len()));
    if !values.is_empty() {
        out.push_str("<Items>");
        for value in values {
            out.push_str(&format!("<{item}>{}</{item}>", escape(value)));
        }
        out.push_str("</Items>");
    }
    out.push_str(&format!("</{name}>"));
    out
}

/// The distributions every rule below is asserted over.
///
/// `PriceClass` admits `PriceClass_100`, `PriceClass_200`, `PriceClass_All` and `None`
/// (from `smithy_query(query="shape", shape="PriceClass")`), and a configuration may state
/// none at all — all five cases appear below. `Aliases`, `Logging` and `PriceClass` are the
/// optional members of a `StreamingDistributionConfig`, so a row carries each and a row
/// carries none; `TrustedSigners` sits at the documented bound of 5 Amazon Web Services
/// accounts per distribution on the last row.
fn table() -> Vec<Row> {
    vec![
        Row {
            case: "the smallest configuration a client can send".to_string(),
            caller_reference: "rtmp-minimal".to_string(),
            bucket: "media-minimal.s3.amazonaws.com".to_string(),
            origin_access_identity: String::new(),
            aliases: None,
            comment: String::new(),
            logging: None,
            signers: Vec::new(),
            price_class: None,
            enabled: false,
        },
        Row {
            case: "one alias, logging on, the cheapest price class".to_string(),
            caller_reference: "rtmp-one-alias".to_string(),
            bucket: "media-one.s3.amazonaws.com".to_string(),
            origin_access_identity: "origin-access-identity/cloudfront/E1AAAAAAAAAAAA"
                .to_string(),
            aliases: Some(vec!["rtmp-one.example.com".to_string()]),
            comment: "serves the archive".to_string(),
            logging: Some((true, "logs-one.s3.amazonaws.com".to_string(), "rtmp/".to_string())),
            signers: vec!["000000000001".to_string()],
            price_class: Some("PriceClass_100"),
            enabled: true,
        },
        Row {
            case: "three aliases and a comment carrying XML metacharacters".to_string(),
            caller_reference: "rtmp-three-aliases".to_string(),
            bucket: "media-three.s3.amazonaws.com".to_string(),
            origin_access_identity: String::new(),
            aliases: Some(vec![
                "a.rtmp.example.com".to_string(),
                "b.rtmp.example.com".to_string(),
                "c.rtmp.example.com".to_string(),
            ]),
            comment: "comment with \"quotes\" & <angles>".to_string(),
            logging: Some((false, String::new(), String::new())),
            signers: vec!["000000000002".to_string(), "000000000003".to_string()],
            price_class: Some("PriceClass_200"),
            enabled: true,
        },
        Row {
            case: "an empty alias wrapper and a non-ASCII comment".to_string(),
            caller_reference: "rtmp-no-aliases".to_string(),
            bucket: "media-none.s3.amazonaws.com".to_string(),
            origin_access_identity: String::new(),
            aliases: Some(Vec::new()),
            comment: "プロダクション — 配信元".to_string(),
            logging: None,
            signers: Vec::new(),
            price_class: Some("PriceClass_All"),
            enabled: false,
        },
        Row {
            case: "the documented trusted-signer bound and the None price class".to_string(),
            caller_reference: "rtmp-signer-bound".to_string(),
            bucket: "media-signers.s3.amazonaws.com".to_string(),
            origin_access_identity: String::new(),
            aliases: Some(vec!["signed.rtmp.example.com".to_string()]),
            comment: "signed urls only".to_string(),
            logging: Some((
                true,
                "logs-signers.s3.amazonaws.com".to_string(),
                String::new(),
            )),
            signers: (0..MAX_TRUSTED_SIGNERS)
                .map(|index| format!("00000000001{index}"))
                .collect(),
            price_class: Some("None"),
            enabled: true,
        },
    ]
}

// ---------------------------------------------------------------------------
// Reading an answer
// ---------------------------------------------------------------------------

/// Every `StreamingDistributionSummary` a page reports, in page order.
fn page_summaries(answer: &Answer) -> Vec<&Node> {
    answer
        .root()
        .at("Items")
        .map(|items| items.children_named("StreamingDistributionSummary"))
        .unwrap_or_default()
}

/// The ids a page reports, in the order the page carries them.
fn page_ids(answer: &Answer) -> Vec<String> {
    page_summaries(answer)
        .into_iter()
        .map(|summary| summary.text_at("Id").unwrap_or_default().to_string())
        .collect()
}

/// Whether `value` has the shape of the id CloudFront mints for a streaming distribution:
/// `E` followed by 13 uppercase alphanumerics, as `StreamingDistribution$Id`'s own example
/// (`EGTXBD79EXAMPLE`) takes.
///
/// Spelled out here rather than taken from the crate, so a changed mint fails this suite.
fn is_streaming_distribution_id(value: &str) -> bool {
    match value.strip_prefix('E') {
        Some(body) => {
            body.len() == 13
                && body
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase())
        }
        None => false,
    }
}

/// The text of one member of a document, or a panic naming the case.
fn member(root: &Node, path: &str, case: &str, raw: &str) -> String {
    root.text_at(path)
        .unwrap_or_else(|| panic!("case {case}: the answer carries no {path}: {raw}"))
        .to_string()
}

/// The values of one list wrapper's `Items`, each element called `item`.
fn wrapper_items(root: &Node, path: &str, item: &str) -> Vec<String> {
    root.at(path)
        .and_then(|wrapper| wrapper.at("Items"))
        .map(|items| {
            items
                .children_named(item)
                .into_iter()
                .map(|node| node.text().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Create every row through the API and report `(row, id)` in creation order, asserting
/// what the create itself answers: HTTP 201, a minted id, an ARN naming the RTMP resource
/// kind, a `Location` under this service's API version and a version token.
async fn create_all(router: &MultiServiceRouter, rows: &[Row]) -> Vec<(Row, String)> {
    let mut created: Vec<(Row, String)> = Vec::new();
    let mut ids: BTreeSet<String> = BTreeSet::new();
    for row in rows {
        let answer = create_streaming_distribution(router, &row.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case {}: expected HTTP 201 from CreateStreamingDistribution, got {} with {}",
            row.case, answer.status, answer.raw
        );
        let id = member(answer.root(), "Id", &row.case, &answer.raw);
        assert!(
            is_streaming_distribution_id(&id),
            "case {}: expected a minted streaming distribution Id, got {id:?}",
            row.case
        );
        assert!(
            ids.insert(id.clone()),
            "case {}: the id {id} repeated across creates",
            row.case
        );
        let arn = member(answer.root(), "ARN", &row.case, &answer.raw);
        assert_eq!(
            arn,
            format!("arn:aws:cloudfront::{ACCOUNT_ID}:streaming-distribution/{id}"),
            "case {}: expected the streaming distribution ARN of {id}, got {arn:?}",
            row.case
        );
        assert_eq!(
            answer.header("location"),
            format!("https://cloudfront.amazonaws.com/{API_VERSION}/streaming-distribution/{id}"),
            "case {}: expected the Location of the created resource, got {:?}",
            row.case,
            answer.header("location")
        );
        assert!(
            !answer.header("etag").is_empty(),
            "case {}: expected a version token on the create, got {}",
            row.case,
            answer.raw
        );
        created.push((row.clone(), id));
    }
    created
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// An account holding no streaming distributions lists none: a page with nothing in it,
/// carrying no `Items` element at all, the default page size, and nothing that says more
/// remain.
#[tokio::test]
async fn an_account_using_no_streaming_distributions_lists_no_items() {
    let router = emulator();
    let case = "empty account";

    let listed = list_streaming_distributions(&router, "").await;
    assert_eq!(
        listed.status, 200,
        "case {case}: expected HTTP 200 from ListStreamingDistributions, got {} with {}",
        listed.status, listed.raw
    );
    assert_eq!(
        listed.root().text_at("Quantity"),
        Some("0"),
        "case {case}: expected Quantity 0, got {}",
        listed.raw
    );
    assert_eq!(
        listed.root().text_at("MaxItems"),
        Some(MAX_ITEMS.to_string().as_str()),
        "case {case}: expected the default MaxItems {MAX_ITEMS}, got {}",
        listed.raw
    );
    assert_eq!(
        listed.root().text_at("IsTruncated"),
        Some("false"),
        "case {case}: expected IsTruncated false, got {}",
        listed.raw
    );
    assert!(
        listed.root().at("Items").is_none(),
        "case {case}: an account holding none carries no Items element, got {}",
        listed.raw
    );
    assert!(
        listed.root().text_at("NextMarker").is_none(),
        "case {case}: expected no NextMarker when nothing remains, got {}",
        listed.raw
    );
}

/// The listing reports every stored streaming distribution — each summary carrying what
/// the same record answers a get with — and walking the pages at any admissible page size
/// enumerates every distribution exactly once, in one order.
#[tokio::test]
async fn a_listing_reports_every_stored_streaming_distribution_and_pages_through_them() {
    let router = emulator();
    let created = create_all(&router, &table()).await;

    // One stable total order over the stored distributions, which every page indexes into.
    let mut expected = created.clone();
    expected.sort_by(|left, right| left.1.cmp(&right.1));
    let order: Vec<String> = expected.iter().map(|(_, id)| id.clone()).collect();

    // A listing that asks for no page size reports the whole (sub-ceiling) collection.
    let listed = list_streaming_distributions(&router, "").await;
    assert_eq!(
        listed.status, 200,
        "case whole collection: expected HTTP 200, got {} with {}",
        listed.status, listed.raw
    );
    assert_eq!(
        page_ids(&listed),
        order,
        "case whole collection: expected every stored id once, in order, got {}",
        listed.raw
    );
    assert_eq!(
        listed.root().text_at("Quantity"),
        Some(order.len().to_string().as_str()),
        "case whole collection: expected Quantity {}, got {}",
        order.len(),
        listed.raw
    );
    assert_eq!(
        listed.root().text_at("IsTruncated"),
        Some("false"),
        "case whole collection: nothing remains, got {}",
        listed.raw
    );
    assert_eq!(
        listed.root().text_at("Marker"),
        Some(""),
        "case whole collection: a first page names no marker, got {}",
        listed.raw
    );

    // Each summary reports the record the create persisted, which is what the get answers
    // from too: one record, two projections of it.
    let summaries = page_summaries(&listed);
    for ((row, id), summary) in expected.iter().zip(summaries.iter()) {
        let case = &row.case;
        let read = get_streaming_distribution(&router, id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected HTTP 200 from GetStreamingDistribution, got {} with {}",
            read.status, read.raw
        );
        let config = read
            .root()
            .at("StreamingDistributionConfig")
            .unwrap_or_else(|| {
                panic!("case {case}: the get carries no StreamingDistributionConfig: {}", read.raw)
            });

        // The members the summary and the distribution share, each read from the one
        // stored record, with the values the submitted configuration asked for.
        for (member_name, listed_value, read_value, submitted) in [
            (
                "Id",
                summary.text_at("Id").unwrap_or_default().to_string(),
                member(read.root(), "Id", case, &read.raw),
                id.clone(),
            ),
            (
                "ARN",
                summary.text_at("ARN").unwrap_or_default().to_string(),
                member(read.root(), "ARN", case, &read.raw),
                format!("arn:aws:cloudfront::{ACCOUNT_ID}:streaming-distribution/{id}"),
            ),
            (
                "Comment",
                summary.text_at("Comment").unwrap_or_default().to_string(),
                config.text_at("Comment").unwrap_or_default().to_string(),
                row.comment.clone(),
            ),
            (
                "PriceClass",
                summary.text_at("PriceClass").unwrap_or_default().to_string(),
                config.text_at("PriceClass").unwrap_or_default().to_string(),
                row.expected_price_class(),
            ),
            (
                "Enabled",
                summary.text_at("Enabled").unwrap_or_default().to_string(),
                config.text_at("Enabled").unwrap_or_default().to_string(),
                row.enabled.to_string(),
            ),
            (
                "S3Origin/DomainName",
                summary
                    .text_at("S3Origin/DomainName")
                    .unwrap_or_default()
                    .to_string(),
                config
                    .text_at("S3Origin/DomainName")
                    .unwrap_or_default()
                    .to_string(),
                row.bucket.clone(),
            ),
            (
                "TrustedSigners/Quantity",
                summary
                    .text_at("TrustedSigners/Quantity")
                    .unwrap_or_default()
                    .to_string(),
                config
                    .text_at("TrustedSigners/Quantity")
                    .unwrap_or_default()
                    .to_string(),
                row.signers.len().to_string(),
            ),
            (
                "Aliases/Quantity",
                summary
                    .text_at("Aliases/Quantity")
                    .unwrap_or_default()
                    .to_string(),
                config
                    .text_at("Aliases/Quantity")
                    .unwrap_or_default()
                    .to_string(),
                row.expected_aliases().len().to_string(),
            ),
        ] {
            assert_eq!(
                listed_value, submitted,
                "case {case}: expected the summary's {member_name} to be {submitted:?}, got \
                 {listed_value:?}"
            );
            assert_eq!(
                read_value, submitted,
                "case {case}: expected the get's {member_name} to be {submitted:?}, got \
                 {read_value:?}"
            );
        }

        // The lists themselves, not just their stated lengths.
        let listed_aliases = wrapper_items(summary, "Aliases", "CNAME");
        assert_eq!(
            listed_aliases,
            row.expected_aliases(),
            "case {case}: expected the summary's aliases to be the submitted ones, got \
             {listed_aliases:?}"
        );
        let listed_signers = wrapper_items(summary, "TrustedSigners", "AwsAccountNumber");
        assert_eq!(
            listed_signers, row.signers,
            "case {case}: expected the summary's trusted signers to be the submitted ones, \
             got {listed_signers:?}"
        );
        let read_signers = wrapper_items(config, "TrustedSigners", "AwsAccountNumber");
        assert_eq!(
            read_signers, row.signers,
            "case {case}: expected the get's trusted signers to be the submitted ones, got \
             {read_signers:?}"
        );
        // "ActiveTrustedSigners: a complex type that lists the accounts, if any, that you
        // included in the TrustedSigners complex type for this distribution" — the same
        // accounts, read from the same record.
        let active_signers: Vec<String> = read
            .root()
            .at("ActiveTrustedSigners")
            .and_then(|node| node.at("Items"))
            .map(|items| {
                items
                    .children_named("Signer")
                    .into_iter()
                    .map(|signer| {
                        signer
                            .text_at("AwsAccountNumber")
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            active_signers, row.signers,
            "case {case}: expected the active trusted signers to be the configured accounts, \
             got {active_signers:?}"
        );
        // The domain name is minted, stable, and the same in both projections.
        let listed_domain = summary.text_at("DomainName").unwrap_or_default().to_string();
        let read_domain = member(read.root(), "DomainName", case, &read.raw);
        assert_eq!(
            listed_domain, read_domain,
            "case {case}: the summary and the get must report one domain name, got \
             {listed_domain:?} and {read_domain:?}"
        );
        assert!(
            listed_domain.ends_with(".cloudfront.net"),
            "case {case}: expected a cloudfront.net domain name, got {listed_domain:?}"
        );
    }

    // Walking the pages at ANY admissible page size enumerates every distribution exactly
    // once, in that same order: sizes below, at and past the collection size, and one past
    // the server's own ceiling.
    let ceiling = usize::try_from(MAX_ITEMS).expect("the ceiling fits a usize");
    let sizes = [
        1usize,
        2,
        3,
        order.len() - 1,
        order.len(),
        order.len() + 1,
        ceiling + 1,
    ];
    for size in sizes {
        let mut walked: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        loop {
            let query = match &marker {
                Some(value) => format!("?MaxItems={size}&Marker={value}"),
                None => format!("?MaxItems={size}"),
            };
            let page = list_streaming_distributions(&router, &query).await;
            assert_eq!(
                page.status, 200,
                "case page size {size}: expected HTTP 200 for {query}, got {} with {}",
                page.status, page.raw
            );
            assert_eq!(
                page.root().text_at("MaxItems"),
                Some(size.to_string().as_str()),
                "case page size {size}: expected the requested MaxItems echoed, got {}",
                page.raw
            );
            assert_eq!(
                page.root().text_at("Marker"),
                Some(marker.as_deref().unwrap_or("")),
                "case page size {size}: expected the request's Marker echoed, got {}",
                page.raw
            );
            let ids = page_ids(&page);
            // The page served is the requested size bounded above by the server's own
            // ceiling, and never longer than what remains.
            assert!(
                ids.len() <= size.min(ceiling),
                "case page size {size}: a page served {} items, got {}",
                ids.len(),
                page.raw
            );
            assert_eq!(
                page.root().text_at("Quantity"),
                Some(ids.len().to_string().as_str()),
                "case page size {size}: expected Quantity to state the page's length, got {}",
                page.raw
            );
            walked.extend(ids.iter().cloned());
            let truncated = walked.len() < order.len();
            assert_eq!(
                page.root().text_at("IsTruncated"),
                Some(if truncated { "true" } else { "false" }),
                "case page size {size}: expected IsTruncated {truncated} after {} of {} \
                 items, got {}",
                walked.len(),
                order.len(),
                page.raw
            );
            let next = page.root().text_at("NextMarker").map(str::to_string);
            assert_eq!(
                next.is_some(),
                truncated,
                "case page size {size}: expected NextMarker present {truncated} after {} of \
                 {} items, got {}",
                walked.len(),
                order.len(),
                page.raw
            );
            match next {
                Some(value) => {
                    assert_eq!(
                        Some(value.as_str()),
                        ids.last().map(String::as_str),
                        "case page size {size}: expected NextMarker to name the id the page \
                         ended at, got {}",
                        page.raw
                    );
                    marker = Some(value);
                }
                None => break,
            }
        }
        assert_eq!(
            walked, order,
            "case page size {size}: walking the pages must enumerate every distribution \
             exactly once, in order"
        );
    }

    // A listing is a READ: the same request answers the same records again.
    let again = list_streaming_distributions(&router, "").await;
    assert_eq!(
        page_ids(&again),
        order,
        "case repeated listing: a read must not change what a later caller sees, got {}",
        again.raw
    );
}

/// A page size that names no page, and a `Marker` that is no resume point this service
/// handed out, are refused with the modelled `InvalidArgument` (HTTP 400) and store
/// nothing; a well-shaped marker is a resume POSITION rather than a refusal.
#[tokio::test]
async fn a_page_size_or_marker_that_names_no_page_is_refused() {
    let router = emulator();
    let created = create_all(&router, &table()[..3]).await;
    let before: Vec<String> = page_ids(&list_streaming_distributions(&router, "").await);

    // Rows: every form a page size can be unusable in (negative, no integer at all,
    // past what the member holds), and every form a marker can fail the id shape in (no id
    // at all, too short, too long, the wrong case, another namespace's token, characters
    // outside the id alphabet) — plus an unusable size beside a usable marker, where the
    // size is what is refused.
    let rows = [
        ("a negative page size", "?MaxItems=-1", false),
        (
            "a page size that is no integer",
            "?MaxItems=not-a-number",
            false,
        ),
        (
            "a page size past what the member can hold",
            "?MaxItems=99999999999",
            false,
        ),
        ("a marker that is no id at all", "?Marker=nope", true),
        ("a marker too short to be an id", "?Marker=E123", true),
        (
            "a marker too long to be an id",
            "?Marker=E1AAAAAAAAAAAAAAAA",
            true,
        ),
        ("a marker in the wrong case", "?Marker=e1aaaaaaaaaaaa", true),
        (
            "a marker from another resource's namespace",
            "?Marker=K1AAAAAAAAAAAA",
            true,
        ),
        (
            "a marker carrying characters outside the id alphabet",
            "?Marker=E1AAAA%20AAAAAA%21",
            true,
        ),
        (
            "an unusable page size beside a usable marker",
            "?MaxItems=-1&Marker=E1AAAAAAAAAAAA",
            false,
        ),
    ];
    for (case, query, over_marker) in rows {
        let answer = list_streaming_distributions(&router, query).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400 for {query}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument for {query}, got {}",
            answer.raw
        );
        // What each refusal states is the CONSTRAINT on the parameter — a marker is a
        // streaming distribution id, a page size is an integer that bounds a page — so every
        // unusable value is refused with that one sentence rather than with text quoted out
        // of the request.
        let expected_message = if over_marker {
            INVALID_STREAMING_DISTRIBUTION_MARKER_MESSAGE
        } else {
            MAX_ITEMS_INVALID_MESSAGE
        };
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, expected_message,
            "case {case}: expected the constraint stated for {query}, got {}",
            answer.raw
        );
    }

    // A well-shaped marker naming the last stored distribution is a resume POSITION: the
    // page it names holds the distributions ordered after it, of which there are none.
    let last = before
        .last()
        .cloned()
        .expect("the fixture created some distributions");
    let after_last = list_streaming_distributions(&router, &format!("?Marker={last}")).await;
    assert_eq!(
        after_last.status, 200,
        "case resume at the last id: expected HTTP 200, got {} with {}",
        after_last.status, after_last.raw
    );
    assert!(
        page_ids(&after_last).is_empty(),
        "case resume at the last id: nothing is ordered after it, got {}",
        after_last.raw
    );
    assert_eq!(
        after_last.root().text_at("Marker"),
        Some(last.as_str()),
        "case resume at the last id: expected the request's Marker echoed, got {}",
        after_last.raw
    );

    // Every refusal above left the store exactly as it was.
    let listed = list_streaming_distributions(&router, "").await;
    assert_eq!(
        page_ids(&listed),
        before,
        "case refusals: a refused listing must store nothing, got {}",
        listed.raw
    );
    assert_eq!(
        created.len(),
        before.len(),
        "case refusals: expected the created distributions to be the listed ones"
    );
}

/// A streaming distribution CloudFront has just accepted is `InProgress`, and the CLOCK —
/// not a later call — owes it the terminal `Deployed`: a listing and a get report the
/// status the record has at the instant they answer.
#[tokio::test]
async fn a_listing_reports_the_status_the_clock_owes() {
    let router = emulator();
    let rows = table();
    let created = create_all(&router, &rows[..2]).await;

    for (row, id) in &created {
        let case = &row.case;
        let listed = list_streaming_distributions(&router, "").await;
        let status = page_summaries(&listed)
            .into_iter()
            .find(|summary| summary.text_at("Id") == Some(id.as_str()))
            .and_then(|summary| summary.text_at("Status"))
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            status, STATUS_IN_PROGRESS,
            "case {case}: a distribution just created has not propagated yet, got \
             {status:?} in {}",
            listed.raw
        );
        let read = get_streaming_distribution(&router, id).await;
        assert_eq!(
            member(read.root(), "Status", case, &read.raw),
            STATUS_IN_PROGRESS,
            "case {case}: the get must report the same status as the listing, got {}",
            read.raw
        );
    }

    // The transition is owed by the clock at the same instant whether or not anybody calls.
    tokio::time::sleep(past_propagation()).await;

    for (row, id) in &created {
        let case = &row.case;
        let listed = list_streaming_distributions(&router, "").await;
        let status = page_summaries(&listed)
            .into_iter()
            .find(|summary| summary.text_at("Id") == Some(id.as_str()))
            .and_then(|summary| summary.text_at("Status"))
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            status, STATUS_DEPLOYED,
            "case {case}: the clock owes this distribution its deployment, got {status:?} \
             in {}",
            listed.raw
        );
        let read = get_streaming_distribution(&router, id).await;
        assert_eq!(
            member(read.root(), "Status", case, &read.raw),
            STATUS_DEPLOYED,
            "case {case}: the get must report the same status as the listing, got {}",
            read.raw
        );
    }
}

/// A caller reference identifies ONE streaming distribution for its whole life: the same
/// configuration again is answered with the distribution that reference already names and
/// creates nothing, and a configuration that differs under a taken reference is refused.
#[tokio::test]
async fn a_repeated_caller_reference_creates_nothing() {
    let router = emulator();
    let rows = table();
    let created = create_all(&router, &rows[..2]).await;
    let before = page_ids(&list_streaming_distributions(&router, "").await);

    for (row, id) in &created {
        let case = format!("{} repeated verbatim", row.case);
        let again = create_streaming_distribution(&router, &row.xml()).await;
        assert_eq!(
            again.status, 201,
            "case {case}: expected the same answer again, got {} with {}",
            again.status, again.raw
        );
        assert_eq!(
            again.root().text_at("Id"),
            Some(id.as_str()),
            "case {case}: expected the distribution the reference already names, got {}",
            again.raw
        );

        // The same reference describing a DIFFERENT distribution is a second distribution
        // under a reference that is taken.
        let mut changed = row.clone();
        changed.comment = format!("{} (changed)", row.comment);
        let refused = create_streaming_distribution(&router, &changed.xml()).await;
        assert_eq!(
            refused.status, 409,
            "case {case}: expected HTTP 409 for a taken caller reference, got {} with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "StreamingDistributionAlreadyExists",
            "case {case}: expected StreamingDistributionAlreadyExists, got {}",
            refused.raw
        );
        assert!(
            refused
                .root()
                .find_text("Message")
                .unwrap_or_default()
                .contains(id.as_str()),
            "case {case}: expected the refusal to name the distribution the reference \
             already holds, got {}",
            refused.raw
        );
    }

    let listed = list_streaming_distributions(&router, "").await;
    assert_eq!(
        page_ids(&listed),
        before,
        "case repeats: neither a repeat nor a refusal may create a distribution, got {}",
        listed.raw
    );
}
