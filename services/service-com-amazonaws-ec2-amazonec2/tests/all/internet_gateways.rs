// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for an account's internet gateways: `CreateInternetGateway` mints one and
//! `DescribeInternetGateways` reports it back. Driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=…`, answered with XML (README.md / OPERATIONS.md "Wire protocol"). ec2Query is
//! selected by the `ec2` signing name in the request's SigV4 credential scope, so every call
//! here carries one.
//!
//! The rules under test:
//!
//! 1. A create PERSISTS the resource it answered: the very next read of that gateway reports
//!    the SAME record — its `igw-` id, the account that owns it, the tags the request
//!    assigned, and an attachment set that is present and EMPTY, because "after creating the
//!    internet gateway, you attach it to a VPC using AttachInternetGateway"
//!    (`CreateInternetGateway`).
//! 2. The operation takes no `ClientToken`, so each call mints a DISTINCT gateway, and the
//!    report lists every one of them in `InternetGatewayId` order.
//! 3. `InternetGatewayIds` narrows the report to the ids named, and an id the account holds
//!    no gateway for is refused with `InvalidInternetGatewayID.NotFound` rather than reported
//!    as an absent row.
//! 4. Each filter the operation documents selects on the member it names, an undocumented
//!    filter name is refused rather than silently reported as an empty set, and an
//!    `attachment.…` filter selects nothing while no VPC is attached ("Present only if a VPC
//!    is attached", `DescribeInternetGatewaysRequest$Filters`).
//! 5. `MaxResults` pages the report in that same id order, with a `NextToken` only while a
//!    further gateway follows the page; a token this service did not issue is refused.
//! 6. A `DryRun` of either operation answers `DryRunOperation` and changes nothing.
//! 7. A tag specification naming any other resource type is refused, with nothing stored.
//!
//! Every table row is a distinct input along what those rules generalise over — how many tags
//! a gateway carries and what they spell, how many gateways the account holds, which selector
//! narrows the report and how large a page is asked for — so no single recorded request is
//! the specification.

use ec2::state::{
    INTERNET_GATEWAY_ID_PREFIX, INTERNET_GATEWAY_TAGGABLE_RESOURCE, MAX_TAGS_PER_RESOURCE,
    MAX_TAG_KEY_CHARS, MAX_TAG_VALUE_CHARS,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{error_code, error_message, filters, indexed, member};
use super::wire::{child, child_text, document, is_long_id, param, set_items};

/// The Region every call here is signed for. An internet gateway carries no Region member,
/// so the Region only picks which account's ambient network the store already holds.
const REGION: &str = "us-east-1";

/// One internet gateway as a response reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedGateway {
    internet_gateway_id: Option<String>,
    owner_id: Option<String>,
    /// Whether the record carried the `attachmentSet` container at all — told apart from a
    /// container that is present and empty, which is what an unattached gateway reports.
    attachment_set_present: bool,
    /// Every attachment of the gateway, as `(state, vpcId)`.
    attachments: Vec<(Option<String>, Option<String>)>,
    tags: Vec<(String, String)>,
}

/// The gateway one `<internetGateway>` or `<internetGatewaySet>` item reports.
///
/// Read off the TREE: a tag's `<value>` and an attachment's `<state>` are children of their
/// own `<item>`, so scanning the body for an element name would mix two records together.
fn gateway_from(node: roxmltree::Node<'_, '_>) -> ReportedGateway {
    ReportedGateway {
        internet_gateway_id: child_text(node, "internetGatewayId"),
        owner_id: child_text(node, "ownerId"),
        attachment_set_present: child(node, "attachmentSet").is_some(),
        attachments: set_items(node, "attachmentSet")
            .into_iter()
            .map(|item| (child_text(item, "state"), child_text(item, "vpcId")))
            .collect(),
        tags: set_items(node, "tagSet")
            .into_iter()
            .map(|item| {
                (
                    child_text(item, "key").unwrap_or_default(),
                    child_text(item, "value").unwrap_or_default(),
                )
            })
            .collect(),
    }
}

/// The one gateway a `CreateInternetGateway` response carries.
fn created_gateway(case: &str, body: &str) -> ReportedGateway {
    let parsed = document(case, body);
    let node = child(parsed.root_element(), "internetGateway")
        .unwrap_or_else(|| panic!("case {case:?}: expected an <internetGateway> in {body}"));
    gateway_from(node)
}

/// The gateways a `DescribeInternetGateways` response carries, in the order it reported them.
fn described_gateways(case: &str, body: &str) -> Vec<ReportedGateway> {
    let parsed = document(case, body);
    set_items(parsed.root_element(), "internetGatewaySet")
        .into_iter()
        .map(gateway_from)
        .collect()
}

/// The ids of the gateways a `DescribeInternetGateways` response carries, in report order.
fn described_ids(case: &str, body: &str) -> Vec<String> {
    described_gateways(case, body)
        .into_iter()
        .map(|gateway| gateway.internet_gateway_id.unwrap_or_default())
        .collect()
}

/// The `TagSpecifications` form fields that assign `tags` to the gateway, as ec2Query
/// flattens them — and nothing at all for a request that assigns none.
fn tag_params(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    if tags.is_empty() {
        return Vec::new();
    }
    let mut params = vec![param(
        "TagSpecification.1.ResourceType",
        INTERNET_GATEWAY_TAGGABLE_RESOURCE,
    )];
    for (position, (key, value)) in tags.iter().enumerate() {
        params.push(param(
            &format!("TagSpecification.1.Tag.{}.Key", position + 1),
            key,
        ));
        params.push(param(
            &format!("TagSpecification.1.Tag.{}.Value", position + 1),
            value,
        ));
    }
    params
}

/// One `CreateInternetGateway` call carrying `params`.
async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "CreateInternetGateway", params).await
}

/// One `DescribeInternetGateways` call carrying `params`.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeInternetGateways", params).await
}

/// A gateway carrying `tags`, created over the wire, as the record the create answered.
async fn create_gateway(
    router: &MultiServiceRouter,
    case: &str,
    tags: &[(&str, &str)],
) -> ReportedGateway {
    let (status, body) = create(router, &tag_params(tags)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the create, got {status} with body {body}"
    );
    created_gateway(case, &body)
}

/// The id of a gateway carrying `tags`, created over the wire.
async fn create_gateway_id(
    router: &MultiServiceRouter,
    case: &str,
    tags: &[(&str, &str)],
) -> String {
    create_gateway(router, case, tags)
        .await
        .internet_gateway_id
        .unwrap_or_else(|| panic!("case {case:?}: the create answered no internetGatewayId"))
}

/// The one gateway `internet_gateway_id` names, read back through the describe.
async fn read_back(
    router: &MultiServiceRouter,
    case: &str,
    internet_gateway_id: &str,
) -> ReportedGateway {
    let (status, body) = describe(router, &indexed("InternetGatewayId", &[internet_gateway_id])).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 reading {internet_gateway_id} back, got {status} \
         with body {body}"
    );
    let reported = described_gateways(case, &body);
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: expected exactly the gateway asked for, got {reported:?}"
    );
    reported.into_iter().next().unwrap_or_else(|| {
        panic!("case {case:?}: a one-element report has a first element: {body}")
    })
}

/// Every gateway the account holds, in report order.
async fn all_ids(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, body) = describe(router, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the unfiltered report, got {status} with body \
         {body}"
    );
    described_ids(case, &body)
}

/// One row of the create table: the tags a request assigns to the gateway it creates.
#[derive(Debug, Clone)]
struct GatewayCase {
    case: &'static str,
    tags: Vec<(String, String)>,
}

/// The create table: one row per kind of tag payload a caller can assign, spanning the count
/// and the size of what a gateway holds. The published bounds are this crate's shared tag
/// bounds (`MAX_TAGS_PER_RESOURCE`, `MAX_TAG_KEY_CHARS`, `MAX_TAG_VALUE_CHARS`), so the last
/// two rows sit exactly on them.
fn gateway_cases() -> Vec<GatewayCase> {
    let pair = |key: &str, value: &str| (key.to_string(), value.to_string());
    vec![
        GatewayCase {
            case: "no tag specification at all",
            tags: Vec::new(),
        },
        GatewayCase {
            case: "one tag",
            tags: vec![pair("Name", "edge")],
        },
        GatewayCase {
            case: "three tags, one of them with an empty value",
            tags: vec![
                pair("Name", "alpha"),
                pair("Tier", "web"),
                pair("Note", ""),
            ],
        },
        GatewayCase {
            case: "a value carrying spaces and punctuation",
            tags: vec![pair("Project.Name", "edge gateway (eu-west)")],
        },
        GatewayCase {
            case: "a key and a value at their published lengths",
            tags: vec![pair(
                &"k".repeat(MAX_TAG_KEY_CHARS),
                &"v".repeat(MAX_TAG_VALUE_CHARS),
            )],
        },
        GatewayCase {
            case: "as many tags as one resource holds",
            tags: (0..MAX_TAGS_PER_RESOURCE)
                .map(|index| pair(&format!("key-{index}"), &format!("value-{index}")))
                .collect(),
        },
    ]
}

/// `row`'s tags as the borrowed pairs the request builder takes.
fn borrowed(row: &GatewayCase) -> Vec<(&str, &str)> {
    row.tags
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect()
}

/// RULE 1 and RULE 2: every row of the create table is stored as the create answered it, and
/// each create mints a gateway of its own.
#[tokio::test]
async fn a_created_gateway_is_read_back_exactly_as_it_was_created() {
    let (router, _engine) = super::ec2_query::emulator();
    let account = engine::emulator_account_id().to_string();

    let mut created: Vec<String> = Vec::new();
    for row in gateway_cases() {
        let case = row.case;
        let answered = create_gateway(&router, case, &borrowed(&row)).await;

        let internet_gateway_id = answered
            .internet_gateway_id
            .clone()
            .unwrap_or_else(|| panic!("case {case:?}: the create answered no internetGatewayId"));
        assert!(
            is_long_id(INTERNET_GATEWAY_ID_PREFIX, &internet_gateway_id),
            "case {case:?}: expected an {INTERNET_GATEWAY_ID_PREFIX}-<17 hex digits> id, got \
             {internet_gateway_id:?}"
        );
        assert_eq!(
            answered.owner_id.as_deref(),
            Some(account.as_str()),
            "case {case:?}: expected the gateway owned by {account}, got {:?}",
            answered.owner_id
        );
        assert!(
            answered.attachment_set_present && answered.attachments.is_empty(),
            "case {case:?}: expected a present and empty attachment set on a gateway attached \
             to nothing, got {answered:?}"
        );
        assert_eq!(
            answered.tags, row.tags,
            "case {case:?}: expected the tags the request assigned, got {:?}",
            answered.tags
        );

        // The create persisted the record, so the very next read reports the same one.
        let stored = read_back(&router, case, &internet_gateway_id).await;
        assert_eq!(
            stored, answered,
            "case {case:?}: expected the describe to report the record the create answered \
             ({answered:?}), got {stored:?}"
        );

        assert!(
            !created.contains(&internet_gateway_id),
            "case {case:?}: expected a gateway of its own, got the id {internet_gateway_id} a \
             previous create already minted"
        );
        created.push(internet_gateway_id);
    }

    // RULE 2: every one of them is reported, in `InternetGatewayId` order — which the ids
    // this store mints are also created in, so the table's order is the report's order.
    let mut expected = created.clone();
    expected.sort();
    assert_eq!(
        all_ids(&router, "every created gateway").await,
        expected,
        "expected every created gateway reported in id order, from {created:?}"
    );
}

/// RULE 2 on the one input the operation has nothing to tell apart by: two identical requests.
/// `CreateInternetGateway` takes no `ClientToken`, so the second is a second gateway.
#[tokio::test]
async fn two_identical_creates_mint_two_gateways() {
    let (router, _engine) = super::ec2_query::emulator();

    let first = create_gateway_id(&router, "the first create", &[("Name", "twin")]).await;
    let second = create_gateway_id(&router, "the repeat", &[("Name", "twin")]).await;
    assert_ne!(
        first, second,
        "a repeat of the same request must mint a second gateway, not replay the first"
    );

    let reported = all_ids(&router, "both gateways").await;
    let mut expected = vec![first, second];
    expected.sort();
    assert_eq!(
        reported, expected,
        "expected both gateways reported in id order, got {reported:?}"
    );
}

/// RULE 3: the id list narrows the report, and an id the account holds no gateway for is
/// refused — whatever its form, since `InternetGatewayId` publishes no pattern to tell a
/// malformed id from an unknown one.
#[tokio::test]
async fn an_id_list_narrows_the_report_and_an_unknown_id_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();

    let first = create_gateway_id(&router, "the first gateway", &[("Name", "first")]).await;
    let second = create_gateway_id(&router, "the second gateway", &[("Name", "second")]).await;
    let third = create_gateway_id(&router, "the third gateway", &[]).await;

    // (case, the ids asked for, the ids expected back)
    let selections: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        ("no id at all reports the whole store", Vec::new(), vec![
            first.as_str(),
            second.as_str(),
            third.as_str(),
        ]),
        ("one id", vec![second.as_str()], vec![second.as_str()]),
        (
            "two of the three, asked for in the other order",
            vec![third.as_str(), first.as_str()],
            vec![first.as_str(), third.as_str()],
        ),
        (
            "the same id twice is still one row",
            vec![first.as_str(), first.as_str()],
            vec![first.as_str()],
        ),
    ];
    for (case, asked, expected) in selections {
        let (status, body) = describe(&router, &indexed("InternetGatewayId", &asked)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            described_ids(case, &body),
            super::ec2_query::owned(&expected),
            "case {case:?}: expected exactly {expected:?}, got body {body}"
        );
    }

    // An id that names no gateway of the account, in each form a client can send one.
    for unknown in [
        "igw-0123456789abcdef0",
        "igw-00000000000000999",
        "igw-12345678",
        "not-an-internet-gateway",
    ] {
        let (status, body) = describe(&router, &indexed("InternetGatewayId", &[unknown])).await;
        assert_eq!(
            status, 400,
            "case {unknown:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInternetGatewayID.NotFound",
            "case {unknown:?}: expected the not-found code, got body {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The internetGateway ID '{unknown}' does not exist"),
            "case {unknown:?}: expected EC2's wording for an unresolvable id, got body {body}"
        );
    }
}

/// One row of the filter table: the case, the `Filters` entries it sends, and the gateway ids
/// it must select — named, because the row is the shape clippy asks for a name for.
type FilterCase<'a> = (&'a str, Vec<(&'a str, Vec<&'a str>)>, Vec<&'a str>);

/// RULE 4: each documented filter selects on the member it names, and a name the operation
/// does not document is refused rather than reported as an empty set.
#[tokio::test]
async fn every_documented_filter_selects_on_its_member() {
    let (router, _engine) = super::ec2_query::emulator();
    let account = engine::emulator_account_id().to_string();

    let alpha = create_gateway_id(
        &router,
        "the tagged gateway",
        &[("Name", "alpha"), ("Tier", "web")],
    )
    .await;
    let beta = create_gateway_id(&router, "the other tagged gateway", &[("Name", "beta")]).await;
    let untagged = create_gateway_id(&router, "the untagged gateway", &[]).await;

    // (case, the filters, the ids expected back)
    let cases: Vec<FilterCase> = vec![
        (
            "internet-gateway-id names one gateway",
            vec![("internet-gateway-id", vec![beta.as_str()])],
            vec![beta.as_str()],
        ),
        (
            "internet-gateway-id over two values is their OR",
            vec![(
                "internet-gateway-id",
                vec![untagged.as_str(), alpha.as_str()],
            )],
            vec![alpha.as_str(), untagged.as_str()],
        ),
        (
            "owner-id names the account every gateway belongs to",
            vec![("owner-id", vec![account.as_str()])],
            vec![alpha.as_str(), beta.as_str(), untagged.as_str()],
        ),
        (
            "owner-id of another account selects nothing",
            vec![("owner-id", vec!["999999999999"])],
            Vec::new(),
        ),
        (
            "tag:<key> selects by the value under that key",
            vec![("tag:Name", vec!["alpha"])],
            vec![alpha.as_str()],
        ),
        (
            "tag:<key> with a value nothing carries selects nothing",
            vec![("tag:Name", vec!["gamma"])],
            Vec::new(),
        ),
        (
            "tag-key selects whatever carries the key, whatever its value",
            vec![("tag-key", vec!["Name"])],
            vec![alpha.as_str(), beta.as_str()],
        ),
        (
            "two filters are joined with an AND",
            vec![
                ("tag-key", vec!["Name"]),
                ("internet-gateway-id", vec![beta.as_str()]),
            ],
            vec![beta.as_str()],
        ),
        (
            "two filters whose AND nothing satisfies",
            vec![
                ("tag:Name", vec!["alpha"]),
                ("internet-gateway-id", vec![beta.as_str()]),
            ],
            Vec::new(),
        ),
        (
            "a filter carrying no value at all selects nothing",
            vec![("tag-key", Vec::new())],
            Vec::new(),
        ),
        // "Present only if a VPC is attached": nothing here is attached, so an
        // `attachment.…` filter selects nothing whatever value it names.
        (
            "attachment.state while no VPC is attached",
            vec![("attachment.state", vec!["available"])],
            Vec::new(),
        ),
        (
            "attachment.vpc-id while no VPC is attached",
            vec![("attachment.vpc-id", vec!["vpc-00000000000000001"])],
            Vec::new(),
        ),
    ];
    for (case, entries, expected) in cases {
        let as_slices: Vec<(&str, &[&str])> = entries
            .iter()
            .map(|(name, values)| (*name, values.as_slice()))
            .collect();
        let (status, body) = describe(&router, &filters(&as_slices)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            described_ids(case, &body),
            super::ec2_query::owned(&expected),
            "case {case:?}: expected exactly {expected:?}, got body {body}"
        );
    }

    // A filter name the operation does not document is refused — including a documented name
    // spelled in another case, since "filter names are case-sensitive", and a `tag:` naming
    // no key.
    for name in ["state", "vpc-id", "Internet-Gateway-Id", "tag:"] {
        let (status, body) = describe(&router, &filters(&[(name, &["anything"])])).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: expected an invalid-filter refusal, got body {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The filter '{name}' is invalid"),
            "case {name:?}: expected the invalid-filter wording, got body {body}"
        );
    }
}

/// RULE 5: the report pages in id order, a token comes back only while a further gateway
/// follows the page, and a token this service did not issue is refused.
#[tokio::test]
async fn the_report_pages_in_id_order() {
    let (router, _engine) = super::ec2_query::emulator();

    let mut ids: Vec<String> = Vec::new();
    for index in 0..7 {
        ids.push(
            create_gateway_id(
                &router,
                "a gateway to page over",
                &[("Index", &index.to_string())],
            )
            .await,
        );
    }
    ids.sort();

    // The smallest page the model admits (`DescribeInternetGatewaysMaxResults`, range
    // 5..1000), walked to the end: the last page carries no token.
    let page_size = 5usize;
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut params = vec![param("MaxResults", &page_size.to_string())];
        if let Some(cursor) = token.as_deref() {
            params.push(param("NextToken", cursor));
        }
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 200,
            "page {pages}: expected HTTP 200, got {status} with body {body}"
        );
        let page = described_ids("a page", &body);
        assert!(
            page.len() <= page_size,
            "page {pages}: expected at most {page_size} gateways, got {page:?}"
        );
        seen.extend(page);
        token = member(&body, "nextToken").filter(|value| !value.is_empty());
        pages += 1;
        assert!(
            pages <= ids.len(),
            "paging must terminate: {pages} pages for {} gateways, last body {body}",
            ids.len()
        );
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        pages, 2,
        "expected {} gateways to take two pages of {page_size}, took {pages}",
        ids.len()
    );
    assert_eq!(
        seen, ids,
        "expected the walk to report every gateway once, in id order, got {seen:?}"
    );

    // A page as large as the store needs no token at all.
    let (status, body) = describe(&router, &[param("MaxResults", "1000")]).await;
    assert_eq!(
        status, 200,
        "one page: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        described_ids("one page", &body),
        ids,
        "one page: expected every gateway, got body {body}"
    );
    assert_eq!(
        member(&body, "nextToken"),
        None,
        "one page: nothing follows the last gateway, so no token is due, got body {body}"
    );

    // A token this service never issued is refused rather than read as an id.
    for bogus in ["not-a-token", "aWdjLTA=", ""] {
        let (status, body) = describe(&router, &[param("NextToken", bogus)]).await;
        assert_eq!(
            status, 400,
            "case {bogus:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {bogus:?}: expected a pagination-token refusal, got body {body}"
        );
    }
}

/// RULE 6: a dry run of either operation answers `DryRunOperation` and leaves the store as it
/// was.
#[tokio::test]
async fn a_dry_run_changes_nothing() {
    let (router, _engine) = super::ec2_query::emulator();
    let before = create_gateway_id(&router, "the stored gateway", &[("Name", "kept")]).await;

    // (case, the extra request fields of the create)
    let dry_runs: Vec<(&str, Vec<(String, String)>)> = vec![
        ("a dry run with no tags", vec![param("DryRun", "true")]),
        (
            "a dry run that would have assigned tags",
            {
                let mut params = tag_params(&[("Name", "never-stored")]);
                params.push(param("DryRun", "true"));
                params
            },
        ),
    ];
    for (case, params) in dry_runs {
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected the dry-run outcome, got body {body}"
        );
        assert_eq!(
            all_ids(&router, case).await,
            vec![before.clone()],
            "case {case:?}: a dry run must store nothing"
        );
    }

    // And the read's own dry run reports the outcome rather than the store.
    let (status, body) = describe(&router, &[param("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "a dry-run describe: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "a dry-run describe: expected the dry-run outcome, got body {body}"
    );
    assert_eq!(
        all_ids(&router, "after the dry-run describe").await,
        vec![before],
        "a dry-run describe must leave the store as it was"
    );
}

/// RULE 7: "you must specify the resource type to tag, otherwise the request will fail"
/// (`TagSpecification`), and the only type this action may name is the resource it creates.
#[tokio::test]
async fn a_tag_specification_for_another_resource_type_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();

    for other in ["vpc", "network-acl", "subnet"] {
        let params = vec![
            param("TagSpecification.1.ResourceType", other),
            param("TagSpecification.1.Tag.1.Key", "Name"),
            param("TagSpecification.1.Tag.1.Value", "misdirected"),
        ];
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {other:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {other:?}: expected the invalid-value refusal, got body {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("'{other}' is not a valid resource type for tagging on CreateInternetGateway"),
            "case {other:?}: expected the wording naming the type and the action, got body {body}"
        );
        assert!(
            all_ids(&router, other).await.is_empty(),
            "case {other:?}: a refused create must store nothing"
        );
    }
}
