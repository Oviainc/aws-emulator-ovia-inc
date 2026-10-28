// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeTransitGatewayConnects`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=DescribeTransitGatewayConnects`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that this describe REPORTS WHAT THE STORE HOLDS: every Connect
//! attachment of the account, each reported with the members it was stored with, narrowed by
//! the ids and the documented filters the request names and paged in
//! `TransitGatewayAttachmentId` order — and nothing else, because every kind of transit
//! gateway attachment shares one id space and one store, so an attachment of another kind is
//! neither reported nor resolvable by id here. Each table row is a distinct input along what
//! that generalises over: which filter, how many values, how many filters at once, whether
//! the attribute a filter names is set at all, a wildcard value, the id selector alone and
//! combined with a filter, and a page size at the published bounds.
//!
//! `CreateTransitGatewayConnect` is still a scaffold, so the Connect attachments are seeded
//! straight into the store the way the Connect-peer and attachment-acceptance suites seed the
//! attachments their operations act on; the transit gateways themselves are created over the
//! wire, so the gateway ids the attachments name are ones that operation really stored.

use std::sync::Arc;

use ec2::ops::types::{
    Tag, TransitGatewayConnect, TransitGatewayConnectOptions, TransitGatewayVpcAttachment,
};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, PROTOCOL_VALUE_GRE,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE, TRANSIT_GATEWAY_ATTACHMENT_STATE_DELETING,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_FAILED, TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING_ACCEPTANCE, TransitGatewayAttachmentView,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, error_message, filters, indexed};
use super::wire::{Param, child, child_text, document, is_long_id, param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

/// The id prefix every transit gateway attachment is minted with.
const ATTACHMENT_ID_PREFIX: &str = "tgw-attach";

/// The transport attachment two of the seeded Connect attachments were made from.
const TRANSPORT_A: &str = "tgw-attach-0a1b2c3d4e5f60718";

/// The transport attachment another two were made from.
const TRANSPORT_B: &str = "tgw-attach-0b2c3d4e5f6071829";

/// The transport attachment the last three were made from.
const TRANSPORT_C: &str = "tgw-attach-0c3d4e5f607182939";

/// The instant the first seeded attachment was created at; each later one is a second after
/// the one before, so `creationTime` is a member that differs per attachment and cannot be
/// read back from a single shared value.
const BASE_SECS: i64 = 1_760_000_000;

/// One Connect attachment to seed: the gateway it is on, the attachment it was made from, the
/// state it is in, the tunnel protocol its options name — `None` for options never set, which
/// no documented filter value can then match — and its tags.
struct Spec {
    label: &'static str,
    gateway: usize,
    transport_id: &'static str,
    state: &'static str,
    protocol: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
}

/// The Connect attachments every test of this suite reads.
///
/// The rows differ along what the rule generalises over: which transit gateway, which
/// transport attachment (shared by more than one, so a transport filter can select a set),
/// which state (several of the members
/// `smithy_query(query="shape",shape="TransitGatewayAttachmentState")` publishes, including
/// two that share the `pending` prefix a wildcard spans), whether the options are set, and
/// how many tags the attachment carries.
fn specs() -> Vec<Spec> {
    vec![
        Spec {
            label: "available, gateway A, transport A, one tag",
            gateway: 0,
            transport_id: TRANSPORT_A,
            state: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            protocol: Some(PROTOCOL_VALUE_GRE),
            tags: &[("Name", "appliance-a")],
        },
        Spec {
            label: "pendingAcceptance, gateway A, transport A",
            gateway: 0,
            transport_id: TRANSPORT_A,
            state: TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING_ACCEPTANCE,
            protocol: Some(PROTOCOL_VALUE_GRE),
            tags: &[],
        },
        Spec {
            label: "deleting, gateway A, transport B",
            gateway: 0,
            transport_id: TRANSPORT_B,
            state: TRANSIT_GATEWAY_ATTACHMENT_STATE_DELETING,
            protocol: Some(PROTOCOL_VALUE_GRE),
            tags: &[],
        },
        Spec {
            label: "failed, gateway B, transport B",
            gateway: 1,
            transport_id: TRANSPORT_B,
            state: TRANSIT_GATEWAY_ATTACHMENT_STATE_FAILED,
            protocol: Some(PROTOCOL_VALUE_GRE),
            tags: &[],
        },
        Spec {
            label: "available, gateway B, transport C, two tags",
            gateway: 1,
            transport_id: TRANSPORT_C,
            state: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            protocol: Some(PROTOCOL_VALUE_GRE),
            tags: &[("Name", "appliance-b"), ("Stage", "test")],
        },
        Spec {
            label: "pending, gateway B, transport C",
            gateway: 1,
            transport_id: TRANSPORT_C,
            state: TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING,
            protocol: Some(PROTOCOL_VALUE_GRE),
            tags: &[],
        },
        Spec {
            label: "available, gateway B, transport C, no options",
            gateway: 1,
            transport_id: TRANSPORT_C,
            state: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            protocol: None,
            tags: &[],
        },
    ]
}

/// One seeded Connect attachment: the id the store minted for it, beside what it was stored
/// with, so a report of it is compared against the record rather than against a literal.
#[derive(Debug, Clone)]
struct Seeded {
    label: &'static str,
    id: String,
    transit_gateway_id: String,
    transport_id: String,
    state: String,
    creation_time: String,
    protocol: String,
    tags: Vec<(String, String)>,
}

/// The emulator, two transit gateways created over the wire, the Connect attachments
/// [`specs`] describes seeded on them, and one VPC attachment beside them — the attachment of
/// another kind that shares the id space this operation must not report.
struct Fixture {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    seeded: Vec<Seeded>,
    vpc_attachment_id: String,
}

impl Fixture {
    /// The fixture, with every seeded attachment's minted id resolved.
    async fn new() -> Self {
        let (router, engine) = super::ec2_query::emulator();
        let mut gateways = Vec::new();
        for index in 0..2 {
            let (status, body) = call(&router, REGION, "CreateTransitGateway", &[]).await;
            assert_eq!(
                status, 200,
                "the fixture's transit gateway {index} must be created, got {status} with \
                 body {body}"
            );
            gateways.push(
                element(&body, "transitGatewayId")
                    .unwrap_or_else(|| panic!("no transitGatewayId in {body}")),
            );
        }

        let mut seeded = Vec::new();
        for (index, spec) in specs().into_iter().enumerate() {
            let transit_gateway_id = gateways[spec.gateway].clone();
            let created_at = BASE_SECS + index as i64;
            let attachment = TransitGatewayConnect {
                transport_transit_gateway_attachment_id: Some(spec.transport_id.to_string()),
                transit_gateway_id: Some(transit_gateway_id.clone()),
                state: Some(spec.state.to_string()),
                creation_time: Some(::aws_smithy_types::DateTime::from_secs(created_at)),
                options: spec.protocol.map(|protocol| TransitGatewayConnectOptions {
                    protocol: Some(protocol.to_string()),
                }),
                tags: Some(
                    spec.tags
                        .iter()
                        .map(|(key, value)| Tag {
                            key: Some((*key).to_string()),
                            value: Some((*value).to_string()),
                        })
                        .collect(),
                ),
                ..Default::default()
            };
            let id = engine
                .state::<Ec2State>()
                .lock()
                .insert_transit_gateway_attachment(TransitGatewayAttachmentView::Connect(
                    attachment,
                ))
                .unwrap_or_else(|| {
                    panic!("the attachment store has room for the seed {:?}", spec.label)
                });
            seeded.push(Seeded {
                label: spec.label,
                id,
                transit_gateway_id,
                transport_id: spec.transport_id.to_string(),
                state: spec.state.to_string(),
                creation_time: timestamp(created_at),
                protocol: spec.protocol.unwrap_or_default().to_string(),
                tags: spec
                    .tags
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                    .collect(),
            });
        }

        let vpc_attachment_id = engine
            .state::<Ec2State>()
            .lock()
            .insert_transit_gateway_attachment(TransitGatewayAttachmentView::Vpc(
                TransitGatewayVpcAttachment {
                    transit_gateway_id: Some(gateways[0].clone()),
                    vpc_id: Some("vpc-0123456789abcdef0".to_string()),
                    state: Some(TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE.to_string()),
                    ..Default::default()
                },
            ))
            .expect("the attachment store has room for one VPC attachment");

        Self {
            router,
            engine,
            seeded,
            vpc_attachment_id,
        }
    }

    /// The seeded attachment `label` names.
    fn seed(&self, label: &str) -> &Seeded {
        self.seeded
            .iter()
            .find(|seed| seed.label == label)
            .unwrap_or_else(|| panic!("no seeded attachment labelled {label:?}"))
    }

    /// The ids of the seeded attachments `labels` names, sorted the way a reported set is.
    fn ids(&self, labels: &[&str]) -> Vec<String> {
        let mut ids: Vec<String> = labels.iter().map(|label| self.seed(label).id.clone()).collect();
        ids.sort();
        ids
    }

    /// Every seeded Connect attachment's id, sorted.
    fn all_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.seeded.iter().map(|seed| seed.id.clone()).collect();
        ids.sort();
        ids
    }
}

/// `secs` as the ISO 8601 UTC timestamp the wire carries a `DateTime` member as.
fn timestamp(secs: i64) -> String {
    ::aws_smithy_types::DateTime::from_secs(secs)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("an ISO 8601 rendering of the seeded creation time")
}

/// One `DescribeTransitGatewayConnects` call carrying `params`.
async fn describe(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    call(router, REGION, "DescribeTransitGatewayConnects", params).await
}

/// One reported `TransitGatewayConnect`, read out of the XML by the document rather than by
/// string surgery — `state` appears on an attachment and on nothing else of this response,
/// but `tagSet` items carry their own children.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    attachment_id: String,
    transport_id: String,
    transit_gateway_id: String,
    state: String,
    creation_time: String,
    protocol: String,
    tags: Vec<(String, String)>,
}

/// Every Connect attachment a response reports, in document order.
fn reported(case: &str, body: &str) -> Vec<Reported> {
    let parsed = document(case, body);
    let Some(set) = parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "transitGatewayConnectSet")
    else {
        return Vec::new();
    };
    set.children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .map(|item| Reported {
            attachment_id: child_text(item, "transitGatewayAttachmentId").unwrap_or_default(),
            transport_id: child_text(item, "transportTransitGatewayAttachmentId")
                .unwrap_or_default(),
            transit_gateway_id: child_text(item, "transitGatewayId").unwrap_or_default(),
            state: child_text(item, "state").unwrap_or_default(),
            creation_time: child_text(item, "creationTime").unwrap_or_default(),
            protocol: child(item, "options")
                .and_then(|options| child_text(options, "protocol"))
                .unwrap_or_default(),
            tags: child(item, "tagSet")
                .map(|tag_set| {
                    tag_set
                        .children()
                        .filter(|node| node.is_element() && node.tag_name().name() == "item")
                        .map(|tag| {
                            (
                                child_text(tag, "key").unwrap_or_default(),
                                child_text(tag, "value").unwrap_or_default(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect()
}

/// The ids a response reports, sorted.
fn reported_ids(case: &str, body: &str) -> Vec<String> {
    let mut ids: Vec<String> = reported(case, body)
        .into_iter()
        .map(|attachment| attachment.attachment_id)
        .collect();
    ids.sort();
    ids
}

/// Every stored Connect attachment is reported with the members it was stored with — the
/// transport attachment it was made from, the gateway it is on, its state, its creation time,
/// its tunnel protocol and its tags — and the attachment of another kind in the same store is
/// not reported at all.
#[tokio::test]
async fn the_describe_reports_every_stored_connect_attachment() {
    let case = "no selector";
    let fixture = Fixture::new().await;

    let (status, body) = describe(&fixture.router, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let got = reported_ids(case, &body);
    assert_eq!(
        got,
        fixture.all_ids(),
        "case {case:?}: expected every seeded Connect attachment, got {got:?}"
    );
    assert!(
        !got.contains(&fixture.vpc_attachment_id),
        "case {case:?}: the VPC attachment {id:?} must not be reported, got {got:?}",
        id = fixture.vpc_attachment_id
    );

    let reports = reported(case, &body);
    for seed in &fixture.seeded {
        let label = seed.label;
        let report = reports
            .iter()
            .find(|report| report.attachment_id == seed.id)
            .unwrap_or_else(|| panic!("case {label:?}: no report of {id:?}", id = seed.id));
        let expected = Reported {
            attachment_id: seed.id.clone(),
            transport_id: seed.transport_id.clone(),
            transit_gateway_id: seed.transit_gateway_id.clone(),
            state: seed.state.clone(),
            creation_time: seed.creation_time.clone(),
            protocol: seed.protocol.clone(),
            tags: seed.tags.clone(),
        };
        assert_eq!(
            *report, expected,
            "case {label:?}: expected the stored attachment reported as {expected:?}, \
             got {report:?}"
        );
        assert!(
            is_long_id(ATTACHMENT_ID_PREFIX, &seed.id),
            "case {label:?}: the store must mint a long-form attachment id, got {id:?}",
            id = seed.id
        );
    }
}

/// One narrowing of the describe and the attachments it must then report.
struct Narrowing {
    label: &'static str,
    params: Vec<Param>,
    expected: Vec<String>,
}

/// The ids and the documented filters narrow the report: each documented filter on its own,
/// a filter whose values are joined with an OR, two filters joined with an AND, a wildcard
/// value, a filter on an attribute an attachment does not carry, the id selector alone, and
/// the id selector together with a filter that excludes one of the ids it named.
#[tokio::test]
async fn the_ids_and_the_filters_narrow_what_is_reported() {
    let fixture = Fixture::new().await;
    let gateway_b = fixture
        .seed("failed, gateway B, transport B")
        .transit_gateway_id
        .clone();
    let deleting = fixture.seed("deleting, gateway A, transport B").id.clone();
    let available_a = fixture
        .seed("available, gateway A, transport A, one tag")
        .id
        .clone();
    let available_b = fixture
        .seed("available, gateway B, transport C, two tags")
        .id
        .clone();

    let narrowings = vec![
        Narrowing {
            label: "no selector at all",
            params: Vec::new(),
            expected: fixture.all_ids(),
        },
        Narrowing {
            label: "the state filter",
            params: filters(&[("state", &[TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE])]),
            expected: fixture.ids(&[
                "available, gateway A, transport A, one tag",
                "available, gateway B, transport C, two tags",
                "available, gateway B, transport C, no options",
            ]),
        },
        Narrowing {
            label: "the state filter with two values, joined with an OR",
            params: filters(&[(
                "state",
                &[
                    TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
                    TRANSIT_GATEWAY_ATTACHMENT_STATE_FAILED,
                ],
            )]),
            expected: fixture.ids(&[
                "available, gateway A, transport A, one tag",
                "available, gateway B, transport C, two tags",
                "available, gateway B, transport C, no options",
                "failed, gateway B, transport B",
            ]),
        },
        Narrowing {
            label: "the state filter with a wildcard spanning two states",
            params: filters(&[("state", &["pending*"])]),
            expected: fixture.ids(&[
                "pendingAcceptance, gateway A, transport A",
                "pending, gateway B, transport C",
            ]),
        },
        Narrowing {
            label: "the state filter on a state no attachment is in",
            params: filters(&[("state", &["rejected"])]),
            expected: Vec::new(),
        },
        Narrowing {
            label: "the options.protocol filter, which an attachment with no options cannot match",
            params: filters(&[("options.protocol", &[PROTOCOL_VALUE_GRE])]),
            expected: fixture.ids(&[
                "available, gateway A, transport A, one tag",
                "pendingAcceptance, gateway A, transport A",
                "deleting, gateway A, transport B",
                "failed, gateway B, transport B",
                "available, gateway B, transport C, two tags",
                "pending, gateway B, transport C",
            ]),
        },
        Narrowing {
            label: "the options.protocol filter with a single-character wildcard",
            params: filters(&[("options.protocol", &["gr?"])]),
            expected: fixture.ids(&[
                "available, gateway A, transport A, one tag",
                "pendingAcceptance, gateway A, transport A",
                "deleting, gateway A, transport B",
                "failed, gateway B, transport B",
                "available, gateway B, transport C, two tags",
                "pending, gateway B, transport C",
            ]),
        },
        Narrowing {
            label: "the transit-gateway-id filter",
            params: filters(&[("transit-gateway-id", &[gateway_b.as_str()])]),
            expected: fixture.ids(&[
                "failed, gateway B, transport B",
                "available, gateway B, transport C, two tags",
                "pending, gateway B, transport C",
                "available, gateway B, transport C, no options",
            ]),
        },
        Narrowing {
            label: "the transport-transit-gateway-attachment-id filter",
            params: filters(&[("transport-transit-gateway-attachment-id", &[TRANSPORT_C])]),
            expected: fixture.ids(&[
                "available, gateway B, transport C, two tags",
                "pending, gateway B, transport C",
                "available, gateway B, transport C, no options",
            ]),
        },
        Narrowing {
            label: "the transit-gateway-attachment-id filter",
            params: filters(&[("transit-gateway-attachment-id", &[deleting.as_str()])]),
            expected: vec![deleting.clone()],
        },
        Narrowing {
            label: "two filters, joined with an AND",
            params: filters(&[
                ("transit-gateway-id", &[gateway_b.as_str()]),
                ("state", &[TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE]),
            ]),
            expected: fixture.ids(&[
                "available, gateway B, transport C, two tags",
                "available, gateway B, transport C, no options",
            ]),
        },
        Narrowing {
            label: "the id selector naming two attachments",
            params: indexed(
                "TransitGatewayAttachmentIds",
                &[available_a.as_str(), available_b.as_str()],
            ),
            expected: fixture.ids(&[
                "available, gateway A, transport A, one tag",
                "available, gateway B, transport C, two tags",
            ]),
        },
        Narrowing {
            label: "the id selector and a filter that excludes one of its ids",
            params: {
                let mut params = indexed(
                    "TransitGatewayAttachmentIds",
                    &[available_a.as_str(), available_b.as_str()],
                );
                params.extend(filters(&[(
                    "transit-gateway-id",
                    &[gateway_b.as_str()],
                )]));
                params
            },
            expected: vec![available_b.clone()],
        },
    ];

    for narrowing in narrowings {
        let label = narrowing.label;
        let (status, body) = describe(&fixture.router, &narrowing.params).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200, got {status} with body {body}"
        );
        let got = reported_ids(label, &body);
        let mut want = narrowing.expected.clone();
        want.sort();
        assert_eq!(
            got, want,
            "case {label:?}: expected the attachments {want:?}, got {got:?}"
        );
    }
}

/// The report is paged in `TransitGatewayAttachmentId` order, a token is minted only when an
/// attachment follows the page, and the token resumes at the attachment after the last one
/// reported.
#[tokio::test]
async fn the_describe_pages_in_id_order() {
    let case = "seven attachments, five to a page";
    let fixture = Fixture::new().await;
    let all = fixture.all_ids();

    let (status, body) = describe(&fixture.router, &[param("MaxResults", "5")]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let first_page = elements(&body, "transitGatewayAttachmentId");
    assert_eq!(
        first_page,
        all[..5].to_vec(),
        "case {case:?}: expected the first five ids in id order, got {first_page:?}"
    );
    let token = element(&body, "nextToken")
        .unwrap_or_else(|| panic!("case {case:?}: expected a nextToken in {body}"));

    let (status, body) = describe(
        &fixture.router,
        &[param("MaxResults", "5"), param("NextToken", &token)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 for the second page, got {status} with body {body}"
    );
    let second_page = elements(&body, "transitGatewayAttachmentId");
    assert_eq!(
        second_page,
        all[5..].to_vec(),
        "case {case:?}: expected the remaining ids, got {second_page:?}"
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "case {case:?}: the last page must carry no token, got body {body}"
    );
}

/// A `MaxResults` outside the range the model publishes (`TransitGatewayMaxResults`,
/// `range 5..1000`) is refused one past each published bound, while each bound itself is
/// served.
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let fixture = Fixture::new().await;

    for (case, page_size, expected_status, expected_code) in [
        ("one below the published minimum", 4, 400, "InvalidMaxResults"),
        ("the published minimum", 5, 200, ""),
        ("the published maximum", 1000, 200, ""),
        ("one above the published maximum", 1001, 400, "ValidationError"),
    ] {
        let (status, body) =
            describe(&fixture.router, &[param("MaxResults", &page_size.to_string())]).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?}: expected {expected_code}, got {body}"
            );
        }
    }
}

/// A filter name this operation does not document is refused with the wording the layer that
/// checks this request's members gives every value it rejects — `Value (<name>) for parameter
/// Filter is invalid. `, naming the member and the name the REQUEST sent — under EC2's
/// `InvalidParameterValue`. Each name the operation DOES document is then served, so what is
/// refused is membership of the catalog rather than filtering as such.
#[tokio::test]
async fn an_undocumented_filter_is_refused_with_the_parameter_wording() {
    let fixture = Fixture::new().await;

    for name in [
        "transit-gateway-connect-peer-id",
        "options.Protocol",
        "State",
        "state ",
        "tag:Name",
        "tag-key",
        "",
    ] {
        let case = format!("the filter {name:?}");
        let (status, body) =
            describe(&fixture.router, &filters(&[(name, &["anything"])])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got body {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message,
            format!("Value ({name}) for parameter Filter is invalid. "),
            "case {case:?}: expected the refused value named in the parameter wording, \
             got {message:?}"
        );
    }

    // The names the operation documents, from
    // `DescribeTransitGatewayConnectsRequest$Filters`:
    //
    // smithy_query(query="doc",shape="DescribeTransitGatewayConnectsRequest",member="Filters",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
    for name in [
        "options.protocol",
        "state",
        "transit-gateway-attachment-id",
        "transit-gateway-id",
        "transport-transit-gateway-attachment-id",
    ] {
        let case = format!("the documented filter {name:?}");
        let (status, body) =
            describe(&fixture.router, &filters(&[(name, &["anything"])])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a documented filter must be served, got {status} with body {body}"
        );
    }
}

/// What the describe cannot answer it refuses with EC2's own code: an id that is not a
/// transit gateway attachment id at all, a well-formed id the account holds no attachment
/// for, an id naming an attachment of ANOTHER kind — which is no Connect attachment, so it is
/// as unknown here as an id naming nothing — a pagination token this service did not mint,
/// and a dry run.
#[tokio::test]
async fn the_describe_refuses_what_it_cannot_answer() {
    struct Case<'a> {
        label: &'a str,
        params: Vec<Param>,
        status: u16,
        code: &'a str,
    }
    let fixture = Fixture::new().await;
    let cases = vec![
        Case {
            label: "an id that is not a transit gateway attachment id",
            params: indexed("TransitGatewayAttachmentIds", &["tgw-attach-nope"]),
            status: 400,
            code: "InvalidTransitGatewayAttachmentID.Malformed",
        },
        Case {
            label: "a well-formed id the account holds no attachment for",
            params: indexed(
                "TransitGatewayAttachmentIds",
                &["tgw-attach-0fedcba9876543210"],
            ),
            status: 400,
            code: "InvalidTransitGatewayAttachmentID.NotFound",
        },
        Case {
            label: "an id naming an attachment of another kind",
            params: indexed(
                "TransitGatewayAttachmentIds",
                &[fixture.vpc_attachment_id.as_str()],
            ),
            status: 400,
            code: "InvalidTransitGatewayAttachmentID.NotFound",
        },
        Case {
            label: "a filter this operation does not document",
            params: filters(&[("tag:Name", &["appliance-a"])]),
            status: 400,
            code: "InvalidParameterValue",
        },
        Case {
            label: "a pagination token this service did not mint",
            params: vec![param("NextToken", "not-a-token")],
            status: 400,
            code: "InvalidPaginationToken",
        },
        Case {
            label: "a dry run",
            params: vec![param("DryRun", "true")],
            status: DRY_RUN_STATUS,
            code: "DryRunOperation",
        },
    ];
    for case in cases {
        let label = case.label;
        let (status, body) = describe(&fixture.router, &case.params).await;
        assert_eq!(
            status, case.status,
            "case {label:?}: expected HTTP {expected}, got {status} with body {body}",
            expected = case.status,
        );
        assert_eq!(
            error_code(&body),
            case.code,
            "case {label:?}: expected the error code {expected:?}, got body {body}",
            expected = case.code,
        );
    }
}

/// The describe is a REPORT: two identical calls around a refused one answer the same
/// attachments in the same states, and the store still holds every attachment it held —
/// including the one of another kind — so nothing a later caller sees was changed by reading.
#[tokio::test]
async fn reading_changes_nothing_a_later_caller_sees() {
    let case = "two reads around a refusal";
    let fixture = Fixture::new().await;

    let (_, first) = describe(&fixture.router, &[]).await;
    let (refused, _) = describe(&fixture.router, &[param("NextToken", "not-a-token")]).await;
    assert_eq!(
        refused, 400,
        "case {case:?}: the middle call must be refused, got {refused}"
    );
    let (_, second) = describe(&fixture.router, &[]).await;
    assert_eq!(
        reported(case, &first),
        reported(case, &second),
        "case {case:?}: two reads must report the same attachments"
    );

    let store = fixture.engine.state::<Ec2State>();
    let store = store.lock();
    for seed in &fixture.seeded {
        let label = seed.label;
        let stored = store
            .transit_gateway_attachments
            .get(&seed.id)
            .unwrap_or_else(|| panic!("case {label:?}: {id:?} must still be stored", id = seed.id));
        assert_eq!(
            stored.view.state(),
            Some(seed.state.as_str()),
            "case {label:?}: expected the stored state {expected:?}, got {got:?}",
            expected = seed.state,
            got = stored.view.state(),
        );
    }
    assert!(
        store
            .transit_gateway_attachments
            .contains_key(&fixture.vpc_attachment_id),
        "case {case:?}: the VPC attachment must still be stored"
    );
}
