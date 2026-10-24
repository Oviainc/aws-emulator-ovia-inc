// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ListAttachmentRoutingPolicyAssociations`, driven over HTTP
//! through the real router (the harness lives in `crate::global_network_lifecycle`).
//!
//! The rule under test: the listing reports ONE summary per attachment of the addressed
//! core network that carries a routing policy label — whatever kind of attachment it is and
//! whatever the label says — an attachment carrying no label has no routing policy
//! association and is absent, an attachment of another core network is never reported, and
//! the summaries are paged by `MaxResults`/`NextToken` in the stable attachment-id order.
//! Reading changes nothing a later caller sees.
//!
//! The table below spans that class: rows differing in attachment kind (VPC, Connect,
//! site-to-site VPN), in edge location and in the label carried — none, the empty label the
//! modelled `length 0..256` allows, a plain one, the 256-character label at that upper
//! bound, and one carrying spaces and non-ASCII, which the modelled `^[\s\S]*$` allows.
//! Nothing reaches into `crate::state`: every attachment is created and read back through
//! an operation.

use serde_json::{json, Value};

use crate::attachment_lifecycle::{
    attachment_id, core_network_at, create_connect_attachment, create_vpc_attachment, subnet_arn,
    vpc_arn,
};
use crate::global_network_lifecycle::{error_code, Emulator};

/// The `ListAttachmentRoutingPolicyAssociations` route
/// (`GET /routing-policy-label/core-network/{CoreNetworkId}`, `smithy_query
/// query="operation" operation="ListAttachmentRoutingPolicyAssociations"`).
const LIST_PREFIX: &str = "/routing-policy-label/core-network";

/// The upper bound the model puts on a routing policy label: `ConstrainedString`, which
/// `AttachmentRoutingPolicyAssociationSummary$RoutingPolicyLabel` targets, is `length
/// 0..256` (`smithy_query shape=ConstrainedString`).
const MAX_LABEL_CHARS: usize = 256;

/// The largest page a caller may ask for: `MaxResults` is modelled `range 1..500`
/// (`smithy_query query="operation" operation="ListAttachmentRoutingPolicyAssociations"`).
const MAX_PAGE: usize = 500;

/// The `ListAttachmentRoutingPolicyAssociations` URI of `core_network`, carrying `query`
/// (already URL-encoded) when it is not empty.
fn list_uri(core_network: &str, query: &str) -> String {
    if query.is_empty() {
        format!("{LIST_PREFIX}/{core_network}")
    } else {
        format!("{LIST_PREFIX}/{core_network}?{query}")
    }
}

/// `ListAttachmentRoutingPolicyAssociations`, asserting the call succeeded.
async fn list(emulator: &Emulator, case: &str, core_network: &str, query: &str) -> Value {
    let (status, body) = emulator.get(&list_uri(core_network, query)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListAttachmentRoutingPolicyAssociations expected 200, got {status} with {body}"
    );
    body
}

/// The `(AttachmentId, RoutingPolicyLabel)` pairs one page reports, in the order returned.
fn reported(case: &str, body: &Value) -> Vec<(String, String)> {
    body["AttachmentRoutingPolicyAssociations"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected an AttachmentRoutingPolicyAssociations array in {body}")
        })
        .iter()
        .map(|summary| {
            let id = summary["AttachmentId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no AttachmentId in {summary}"));
            let label = summary["RoutingPolicyLabel"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no RoutingPolicyLabel in {summary}"));
            (id.to_string(), label.to_string())
        })
        .collect()
}

/// A VPN connection ARN in `region`, as the modelled `VpnConnectionArn` pattern spells one
/// (`^arn:[^:]{1,63}:ec2:[^:]{0,63}:[^:]{0,63}:vpn-connection\/vpn-[0-9a-f]{8,17}$`,
/// `smithy_query shape=VpnConnectionArn`).
fn vpn_arn(region: &str, index: usize) -> String {
    format!(
        "arn:aws:ec2:{region}:{}:vpn-connection/vpn-{index:08x}",
        engine::emulator_account_id()
    )
}

/// Which kind of attachment a row of the table creates. The three kinds share the stored
/// attachment envelope and each carries an optional `RoutingPolicyLabel` on its create
/// (`smithy_query query="operation"
/// operation="CreateVpcAttachment,CreateConnectAttachment,CreateSiteToSiteVpnAttachment"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Vpc,
    Connect,
    Vpn,
}

/// One row of the table: an attachment of one kind, at one edge location, carrying one
/// routing policy label or none.
struct LabelCase {
    /// Names the case in every assertion message.
    name: &'static str,
    kind: Kind,
    /// The edge location the attachment sits at, which the core network under test
    /// configures an edge for.
    region: &'static str,
    /// The `RoutingPolicyLabel` the create carries, or `None` for a create that carries
    /// none.
    label: Option<String>,
}

/// The edge locations the core network under test configures, so every row below sits at
/// one of them.
const REGIONS: [&str; 3] = ["us-west-2", "eu-central-1", "ap-southeast-4"];

/// The table: rows differing along everything the rule generalises over — the attachment
/// kind, the edge location, and the label (absent, empty, plain, at the published length
/// bound, and carrying characters beyond ASCII letters).
fn label_cases() -> Vec<LabelCase> {
    vec![
        LabelCase {
            name: "a VPC attachment carrying no label",
            kind: Kind::Vpc,
            region: "us-west-2",
            label: None,
        },
        LabelCase {
            name: "a VPC attachment carrying a plain label",
            kind: Kind::Vpc,
            region: "us-west-2",
            label: Some("north".to_string()),
        },
        LabelCase {
            name: "a VPC attachment carrying the empty label the modelled length 0..256 allows",
            kind: Kind::Vpc,
            region: "eu-central-1",
            label: Some(String::new()),
        },
        LabelCase {
            name: "a VPC attachment carrying the 256-character label at the modelled bound",
            kind: Kind::Vpc,
            region: "ap-southeast-4",
            label: Some("l".repeat(MAX_LABEL_CHARS)),
        },
        LabelCase {
            name: "a VPC attachment carrying a label with spaces and non-ASCII",
            kind: Kind::Vpc,
            region: "eu-central-1",
            label: Some("ingress policy \u{2013} \u{3c0}".to_string()),
        },
        LabelCase {
            name: "a Connect attachment carrying a label",
            kind: Kind::Connect,
            region: "us-west-2",
            label: Some("connect-edge".to_string()),
        },
        LabelCase {
            name: "a Connect attachment carrying no label",
            kind: Kind::Connect,
            region: "us-west-2",
            label: None,
        },
        LabelCase {
            name: "a site-to-site VPN attachment carrying a label",
            kind: Kind::Vpn,
            region: "ap-southeast-4",
            label: Some("vpn-south".to_string()),
        },
        LabelCase {
            name: "a site-to-site VPN attachment carrying no label",
            kind: Kind::Vpn,
            region: "eu-central-1",
            label: None,
        },
    ]
}

/// `body` with `RoutingPolicyLabel` added when the row carries one.
fn with_label(mut body: Value, case: &LabelCase) -> Value {
    if let Some(label) = case.label.as_deref() {
        body.as_object_mut()
            .unwrap_or_else(|| panic!("case {:?}: the request body must be an object", case.name))
            .insert("RoutingPolicyLabel".to_string(), json!(label));
    }
    body
}

/// Create the attachment a row describes on `core_network`, answering its minted id.
///
/// `index` keeps each row's VPC / subnet / VPN ARNs distinct, and `transport` is the VPC
/// attachment a Connect attachment is carried over.
async fn create_case(
    emulator: &Emulator,
    core_network: &str,
    index: usize,
    transport: &str,
    case: &LabelCase,
) -> String {
    let (wrapper, (status, created)) = match case.kind {
        Kind::Vpc => (
            "VpcAttachment",
            create_vpc_attachment(
                emulator,
                with_label(
                    json!({
                        "CoreNetworkId": core_network,
                        "VpcArn": vpc_arn(case.region, index),
                        "SubnetArns": [subnet_arn(case.region, index)],
                    }),
                    case,
                ),
            )
            .await,
        ),
        Kind::Connect => (
            "ConnectAttachment",
            create_connect_attachment(
                emulator,
                with_label(
                    json!({
                        "CoreNetworkId": core_network,
                        "EdgeLocation": case.region,
                        "TransportAttachmentId": transport,
                        "Options": { "Protocol": "GRE" },
                    }),
                    case,
                ),
            )
            .await,
        ),
        Kind::Vpn => (
            "SiteToSiteVpnAttachment",
            emulator
                .post(
                    "/site-to-site-vpn-attachments",
                    with_label(
                        json!({
                            "CoreNetworkId": core_network,
                            "VpnConnectionArn": vpn_arn(case.region, index),
                        }),
                        case,
                    ),
                )
                .await,
        ),
    };
    assert_eq!(
        status, 200,
        "case {:?}: the create expected 200, got {status} with {created}",
        case.name
    );
    attachment_id(case.name, &created[wrapper])
}

/// Build the core network of the table and every attachment in it, answering
/// `(core network id, one (id, label) pair per row)` in creation order.
///
/// A Connect attachment is carried over a VPC attachment of the same core network, so one
/// unlabelled VPC attachment is created first to serve as that transport. It is itself an
/// attachment the listing must leave out.
/// `base` offsets the ARNs this subject's attachments attach, so two subjects of one
/// emulator attach different VPCs and VPN connections: a resource is attached by at most
/// one attachment across every core network.
async fn seeded(
    emulator: &Emulator,
    name: &str,
    base: usize,
) -> (String, Vec<(String, Option<String>)>) {
    let (_, core_network) = core_network_at(emulator, name, &REGIONS).await;
    let transport = create_case(
        emulator,
        &core_network,
        base,
        "",
        &LabelCase {
            name: "the Connect transport",
            kind: Kind::Vpc,
            region: "us-west-2",
            label: None,
        },
    )
    .await;

    let mut created = vec![(transport.clone(), None)];
    for (index, case) in label_cases().iter().enumerate() {
        let id = create_case(emulator, &core_network, base + index + 1, &transport, case).await;
        created.push((id, case.label.clone()));
    }
    (core_network, created)
}

/// The pairs the listing must report for `created`: the labelled attachments only, in the
/// attachment-id order the store holds them in.
fn expected(created: &[(String, Option<String>)]) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = created
        .iter()
        .filter_map(|(id, label)| label.clone().map(|label| (id.clone(), label)))
        .collect();
    pairs.sort_by(|left, right| left.0.cmp(&right.0));
    pairs
}

#[tokio::test]
async fn the_listing_reports_every_labelled_attachment_of_the_core_network() {
    let emulator = Emulator::new();
    let (core_network, created) = seeded(&emulator, "labels", 0).await;
    let wanted = expected(&created);

    let body = list(&emulator, "the whole listing", &core_network, "").await;
    assert_eq!(
        reported("the whole listing", &body),
        wanted,
        "case {:?}: expected exactly the labelled attachments {wanted:?}, got {body}",
        "the whole listing"
    );
    assert!(
        body["NextToken"].is_null(),
        "case {:?}: a listing that fits one page carries no NextToken, got {body}",
        "the whole listing"
    );

    // Each row on its own: `AttachmentId` narrows the listing to that attachment, which
    // reports its own label and nothing else — and reports nothing at all for an attachment
    // that carries no label.
    for (id, label) in &created {
        let body = list(
            &emulator,
            id,
            &core_network,
            &format!("attachmentId={id}"),
        )
        .await;
        let wanted = match label {
            Some(label) => vec![(id.clone(), label.clone())],
            None => Vec::new(),
        };
        assert_eq!(
            reported(id, &body),
            wanted,
            "case {id:?}: expected {wanted:?} for the attachment filter, got {body}"
        );
    }
}

#[tokio::test]
async fn a_read_reports_the_same_listing_to_a_later_caller() {
    let emulator = Emulator::new();
    let (core_network, created) = seeded(&emulator, "repeat", 0).await;
    let wanted = expected(&created);

    for pass in ["the first read", "the second read", "the third read"] {
        let body = list(&emulator, pass, &core_network, "").await;
        assert_eq!(
            reported(pass, &body),
            wanted,
            "case {pass:?}: a read must not change what a later caller sees, got {body}"
        );
    }
}

#[tokio::test]
async fn every_page_size_walks_the_listing_exactly_once() {
    let emulator = Emulator::new();
    let (core_network, created) = seeded(&emulator, "pages", 0).await;
    let wanted = expected(&created);
    assert!(
        wanted.len() > 2,
        "the table must hold enough labelled attachments to page over, got {wanted:?}"
    );

    // Each row: a page size. 1 is the modelled lower bound, 500 the upper one, and the
    // sizes between them straddle the number of entries there are to report.
    for size in [1, 2, 3, wanted.len(), wanted.len() + 1, MAX_PAGE] {
        let case = format!("maxResults={size}");
        let mut walked: Vec<(String, String)> = Vec::new();
        let mut query = format!("maxResults={size}");
        let mut pages = 0usize;
        loop {
            let body = list(&emulator, &case, &core_network, &query).await;
            let page = reported(&case, &body);
            assert!(
                !page.is_empty() && page.len() <= size,
                "case {case:?}: a page holds at least one and at most {size} entries, got {body}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= wanted.len(),
                "case {case:?}: paging must terminate within {} pages, got {body}",
                wanted.len()
            );
            match body["NextToken"].as_str() {
                Some(token) => query = format!("maxResults={size}&nextToken={token}"),
                None => break,
            }
        }
        assert_eq!(
            walked, wanted,
            "case {case:?}: following the tokens must report every labelled attachment exactly once"
        );
        let expected_pages = wanted.len().div_ceil(size);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: {} entries at {size} per page is {expected_pages} pages, walked {pages}",
            wanted.len()
        );
    }
}

#[tokio::test]
async fn one_core_networks_labels_are_not_reported_by_another() {
    let emulator = Emulator::new();
    // A core network lives in its own global network (one core network per global network),
    // so the second subject is built the same way as the first.
    let (first, first_created) = seeded(&emulator, "first", 0).await;
    let (second, second_created) = seeded(&emulator, "second", 100).await;

    for (case, core_network, created) in [
        ("the first core network", &first, &first_created),
        ("the second core network", &second, &second_created),
    ] {
        let body = list(&emulator, case, core_network, "").await;
        assert_eq!(
            reported(case, &body),
            expected(created),
            "case {case:?}: a core network reports only its own attachments, got {body}"
        );
    }

    // An attachment of the OTHER core network is a filter that matches nothing here, not a
    // refusal and not a leak.
    let foreign = expected(&second_created)
        .first()
        .map(|(id, _)| id.clone())
        .expect("the second core network must hold a labelled attachment");
    let case = "a filter naming an attachment of another core network";
    let body = list(
        &emulator,
        case,
        &first,
        &format!("attachmentId={foreign}"),
    )
    .await;
    assert_eq!(
        reported(case, &body),
        Vec::new(),
        "case {case:?}: expected no entry, got {body}"
    );
}

#[tokio::test]
async fn the_listing_refuses_an_unknown_core_network_and_an_unissued_token() {
    let emulator = Emulator::new();
    let (core_network, _) = seeded(&emulator, "refusals", 0).await;

    // Each row: a request that cannot be served, and the modelled refusal for it. Both
    // errors are declared by this operation (`smithy_query query="operation"
    // operation="ListAttachmentRoutingPolicyAssociations"`).
    for (case, uri, status_wanted, code_wanted) in [
        (
            "a core network id this account holds nothing for",
            list_uri("core-network-00000000000000099", ""),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a NextToken this service never issued",
            list_uri(&core_network, "nextToken=not-a-token"),
            400,
            "ValidationException",
        ),
        (
            "a NextToken that is hex but names no listing of this service",
            list_uri(&core_network, "nextToken=6162636465"),
            400,
            "ValidationException",
        ),
    ] {
        let (status, body) = emulator.get(&uri).await;
        assert_eq!(
            status, status_wanted,
            "case {case:?}: expected {status_wanted}, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            code_wanted,
            "case {case:?}: expected {code_wanted}, got {body}"
        );
    }

    // An attachment id that names nothing stored is a filter matching nothing, not a
    // refusal: the core network addressed does exist.
    let case = "a filter naming an attachment nothing was stored for";
    let body = list(
        &emulator,
        case,
        &core_network,
        "attachmentId=attachment-00000000000000099",
    )
    .await;
    assert_eq!(
        reported(case, &body),
        Vec::new(),
        "case {case:?}: expected no entry, got {body}"
    );
}
