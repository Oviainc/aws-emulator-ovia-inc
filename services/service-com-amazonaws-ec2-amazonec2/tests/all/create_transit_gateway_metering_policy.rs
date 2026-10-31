// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateTransitGatewayMeteringPolicy`, driven over the wire this
//! service speaks (`ec2Query`), with `DescribeTransitGatewayMeteringPolicies` reading back
//! what the create stored (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test is the one every EC2 create follows: one call MINTS the resource it
//! names from the request and PERSISTS it, reporting the record it stored — the facts the
//! request chose (the transit gateway the policy meters, the set of middlebox attachments it
//! names, the tags it assigns) kept as given, and everything the request does not choose
//! derived by the service (the id under this resource's own prefix, the TRANSIENT state of a
//! create with the clock owing it the terminal one, and the instant the policy's update takes
//! effect). Every id the request names is resolved BEFORE anything is written, so a value
//! that is not an id of its kind, an id nothing holds, a tag specification naming another
//! resource type and a dry run are each refused with nothing stored.
//!
//! Each table row is a distinct input along what that rule generalises over: HOW MANY
//! middlebox attachments the request names (none, one, three, all of them), in WHICH order,
//! whether it names one TWICE, and how many tags it assigns (none, one, two) — and, for the
//! refusals, which id of the request is unusable and how.
//!
//! The middlebox attachments each case meters are seeded through the store's own writer
//! (`insert_transit_gateway_attachment`), as every transit gateway suite in this crate does,
//! because `CreateTransitGatewayVpcAttachment` is still a scaffold stub; the transit gateway
//! itself is created over the wire through `CreateTransitGateway`.

use std::time::Duration;

use ec2::ops::types::{TRANSIT_GATEWAY_METERING_POLICY_STATE_VALUES, TransitGatewayVpcAttachment};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, DRY_RUN_OPERATION, INVALID_PARAMETER_VALUE,
    INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_MALFORMED,
    INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND, INVALID_TRANSIT_GATEWAY_ID_NOT_FOUND,
    MISSING_PARAMETER, RESOURCE_TYPE_TRANSIT_GATEWAY_METERING_POLICY,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE, TRANSIT_GATEWAY_METERING_POLICY_ID_PREFIX,
    TRANSITION_DELAY, TransitGatewayAttachmentView,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, element, error_code, indexed};
use super::wire::{Param, child, child_text, document, param, set_items};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The ec2Query `Action` under test.
const ACTION: &str = "CreateTransitGatewayMeteringPolicy";

/// The describe that reads a created policy back.
const DESCRIBE: &str = "DescribeTransitGatewayMeteringPolicies";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

/// The HTTP status EC2 answers a refused request with.
const REFUSAL_STATUS: u16 = 400;

/// How many middlebox attachments every case seeds, so a row can name a subset of them in any
/// order.
const SEEDED_ATTACHMENTS: usize = 4;

/// Long enough past [`TRANSITION_DELAY`] that a move the clock owes is certainly due.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// A member of `TransitGatewayMeteringPolicyState`, from the modelled catalog
/// (`smithy_query(query="shape",shape="TransitGatewayMeteringPolicyState")`, whose values are
/// `available | deleted | deleting | modifying | pending`) rather than written out here.
fn policy_state(name: &str) -> &'static str {
    TRANSIT_GATEWAY_METERING_POLICY_STATE_VALUES
        .iter()
        .find(|value| **value == name)
        .unwrap_or_else(|| panic!("{name:?} is no TransitGatewayMeteringPolicyState member"))
}

/// One call of `action` signed for the Region above.
async fn invoke(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// A transit gateway created through the wire, as its `TransitGatewayId`.
async fn transit_gateway(router: &MultiServiceRouter) -> String {
    let (status, body) = invoke(router, "CreateTransitGateway", &[]).await;
    assert_eq!(
        status, 200,
        "CreateTransitGateway must succeed, got {status} with body {body}"
    );
    element(&body, "transitGatewayId").expect("a created transit gateway reports its own id")
}

/// Store [`SEEDED_ATTACHMENTS`] VPC attachments of `gateway_id`, in service, and answer their
/// `TransitGatewayAttachmentId`s — the attachments a request may name as middleboxes.
fn seed_attachments(engine: &engine::InMemoryEngine, gateway_id: &str) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    (0..SEEDED_ATTACHMENTS)
        .map(|index| {
            guard
                .insert_transit_gateway_attachment(TransitGatewayAttachmentView::Vpc(
                    TransitGatewayVpcAttachment {
                        transit_gateway_id: Some(gateway_id.to_string()),
                        vpc_id: Some(format!("vpc-0123456789abcde{index:0>2x}")),
                        vpc_owner_id: Some("210987654321".to_string()),
                        state: Some(TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE.to_string()),
                        ..Default::default()
                    },
                ))
                .expect("the store holds the seeded VPC attachment")
        })
        .collect()
}

/// The emulator with one transit gateway and the middlebox attachments above: the router to
/// call, the engine to read the store through, the gateway's id, and the attachment ids.
async fn seeded() -> (
    MultiServiceRouter,
    std::sync::Arc<engine::InMemoryEngine>,
    String,
    Vec<String>,
) {
    let (router, engine) = super::ec2_query::emulator();
    let gateway_id = transit_gateway(&router).await;
    let attachment_ids = seed_attachments(&engine, &gateway_id);
    (router, engine, gateway_id, attachment_ids)
}

/// `MiddleboxAttachmentId.N` form fields for `ids`.
///
/// `CreateTransitGatewayMeteringPolicyRequest$MiddleboxAttachmentIds` is bound to the
/// `MiddleboxAttachmentId` xmlName
/// (`smithy_query(query="operation",operation="CreateTransitGatewayMeteringPolicy")`), which
/// is the key ec2Query flattens the list under.
fn middlebox_params(ids: &[String]) -> Vec<Param> {
    indexed(
        "MiddleboxAttachmentId",
        &ids.iter().map(String::as_str).collect::<Vec<_>>(),
    )
}

/// The `TagSpecifications` form fields that tag the policy one call creates, naming
/// `resource_type`.
///
/// This request's `TagSpecifications` member carries NO xmlName
/// (`smithy_query(query="operation",operation="CreateTransitGatewayMeteringPolicy")`), so
/// ec2Query spells the key with the member's own name rather than the singular
/// `TagSpecification` the older EC2 creates use.
fn tag_params(resource_type: &str, tags: &[(&str, &str)]) -> Vec<Param> {
    let mut params = vec![param("TagSpecifications.1.ResourceType", resource_type)];
    for (position, (key, value)) in tags.iter().enumerate() {
        let nth = position + 1;
        params.push(param(&format!("TagSpecifications.1.Tag.{nth}.Key"), key));
        params.push(param(&format!("TagSpecifications.1.Tag.{nth}.Value"), value));
    }
    params
}

/// Read something off the one metering policy a response carries — the element that has a
/// `transitGatewayMeteringPolicyId` child.
fn with_policy<T>(case: &str, body: &str, read: impl FnOnce(roxmltree::Node<'_, '_>) -> T) -> T {
    let parsed = document(case, body);
    let policy = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "transitGatewayMeteringPolicyId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no metering policy in {body}"));
    read(policy)
}

/// A member of the one metering policy a response carries, or `None` when it carries no such
/// member.
fn member(case: &str, body: &str, name: &str) -> Option<String> {
    with_policy(case, body, |policy| child_text(policy, name))
}

/// Whether the one metering policy a response carries has a `name` child ELEMENT at all.
///
/// The absence check for a member nothing recorded: [`member`] reads an element's TEXT, which
/// a present-but-empty container (`<tagSet/>`) does not carry, so only element absence shows
/// that nothing was reported for it.
fn has_member(case: &str, body: &str, name: &str) -> bool {
    with_policy(case, body, |policy| child(policy, name).is_some())
}

/// The middlebox attachment ids of the one metering policy a response carries, in document
/// order.
fn reported_middlebox_attachments(case: &str, body: &str) -> Vec<String> {
    with_policy(case, body, |policy| {
        set_items(policy, "middleboxAttachmentIdSet")
            .into_iter()
            .map(|item| item.text().unwrap_or_default().to_string())
            .collect()
    })
}

/// The (key, value) pairs of the one metering policy a response carries.
fn reported_tags(case: &str, body: &str) -> Vec<(String, String)> {
    with_policy(case, body, |policy| {
        set_items(policy, "tagSet")
            .into_iter()
            .map(|item| {
                (
                    child_text(item, "key").unwrap_or_default(),
                    child_text(item, "value").unwrap_or_default(),
                )
            })
            .collect()
    })
}

/// How many metering policies the store holds.
fn stored_policies(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<Ec2State>()
        .lock()
        .transit_gateway_metering_policies
        .len()
}

/// The state the store holds for one metering policy, or `None` when it holds none.
fn stored_state(engine: &engine::InMemoryEngine, metering_policy_id: &str) -> Option<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .transit_gateway_metering_policies
        .get(metering_policy_id)
        .and_then(|policy| policy.state.clone())
}

/// One created policy: which of the seeded attachments the request names — by position, so a
/// row can repeat one — which it must then report, and the tags it assigns.
struct Case {
    /// What the row is, for the assertion messages.
    name: &'static str,
    /// The positions in the seeded attachment list the request names, in request order.
    names: &'static [usize],
    /// The positions the created policy must report, in order.
    reports: &'static [usize],
    /// The tags the request assigns, as (key, value) pairs.
    tags: &'static [(&'static str, &'static str)],
}

/// The creates this suite is written against: a policy metering nothing, one metering a single
/// attachment, one metering three named out of order, one naming the same attachment four
/// times over, and one metering every attachment there is — with no tags, one tag and two.
const CASES: &[Case] = &[
    Case {
        name: "no middlebox attachment and no tag",
        names: &[],
        reports: &[],
        tags: &[],
    },
    Case {
        name: "one middlebox attachment and one tag",
        names: &[0],
        reports: &[0],
        tags: &[("Name", "north")],
    },
    Case {
        name: "three middlebox attachments, named out of order, and two tags",
        names: &[2, 0, 3],
        reports: &[2, 0, 3],
        tags: &[("Name", "south"), ("Team", "network")],
    },
    Case {
        name: "a middlebox attachment named four times is metered once",
        names: &[1, 1, 0, 1],
        reports: &[1, 0],
        tags: &[("Team", "network")],
    },
    Case {
        name: "every seeded middlebox attachment, untagged",
        names: &[0, 1, 2, 3],
        reports: &[0, 1, 2, 3],
        tags: &[],
    },
];

/// The attachment ids at `positions` in the seeded list.
fn at(attachment_ids: &[String], positions: &[usize]) -> Vec<String> {
    positions
        .iter()
        .map(|position| attachment_ids[*position].clone())
        .collect()
}

/// The form fields one case's create carries.
fn create_params(attachment_ids: &[String], gateway_id: &str, case: &Case) -> Vec<Param> {
    let mut params = vec![param("TransitGatewayId", gateway_id)];
    params.extend(middlebox_params(&at(attachment_ids, case.names)));
    if !case.tags.is_empty() {
        params.extend(tag_params(
            RESOURCE_TYPE_TRANSIT_GATEWAY_METERING_POLICY,
            case.tags,
        ));
    }
    params
}

/// The tags one case assigns, as owned pairs.
fn expected_tags(case: &Case) -> Vec<(String, String)> {
    case.tags
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Every fact a created metering policy carries is the one the request named or the one the
/// service derives, it is reported by the create itself, and it reads back identically
/// through the describe. One row per case, so no single request is the specification.
#[tokio::test]
async fn a_created_metering_policy_reports_and_persists_what_the_request_named() {
    let (router, engine, gateway_id, attachment_ids) = seeded().await;
    let mut minted: Vec<String> = Vec::new();

    for case in CASES {
        let name = case.name;
        let (status, body) = invoke(
            &router,
            ACTION,
            &create_params(&attachment_ids, &gateway_id, case),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200 from {ACTION}, got {status} with body {body}"
        );

        let metering_policy_id = member(name, &body, "transitGatewayMeteringPolicyId")
            .unwrap_or_else(|| panic!("case {name:?}: the create must report the policy's id"));
        assert!(
            metering_policy_id.starts_with(&format!("{TRANSIT_GATEWAY_METERING_POLICY_ID_PREFIX}-")),
            "case {name:?}: expected an id the service minted under its own prefix, got \
             {metering_policy_id:?}"
        );
        assert!(
            !minted.contains(&metering_policy_id),
            "case {name:?}: every create mints a fresh id, got {metering_policy_id:?} again \
             after {minted:?}"
        );
        minted.push(metering_policy_id.clone());

        let reported_gateway = member(name, &body, "transitGatewayId");
        assert_eq!(
            reported_gateway.as_deref(),
            Some(gateway_id.as_str()),
            "case {name:?}: expected the policy to meter the gateway named, got \
             {reported_gateway:?} in {body}"
        );
        let reported_state = member(name, &body, "state");
        assert_eq!(
            reported_state.as_deref(),
            Some(policy_state("pending")),
            "case {name:?}: expected the transient state of a create, got {reported_state:?} \
             in {body}"
        );
        let reported_effective = member(name, &body, "updateEffectiveAt");
        assert!(
            reported_effective.as_deref().and_then(as_timestamp).is_some(),
            "case {name:?}: expected the instant the policy's update takes effect, got \
             {reported_effective:?} in {body}"
        );

        let expected_attachments = at(&attachment_ids, case.reports);
        if expected_attachments.is_empty() {
            assert!(
                !has_member(name, &body, "middleboxAttachmentIdSet"),
                "case {name:?}: a policy metering no attachment reports no \
                 <middleboxAttachmentIdSet>, got {body}"
            );
        } else {
            let actual = reported_middlebox_attachments(name, &body);
            assert_eq!(
                actual, expected_attachments,
                "case {name:?}: expected the attachments the request named, got {actual:?} in \
                 {body}"
            );
        }

        if case.tags.is_empty() {
            assert!(
                !has_member(name, &body, "tagSet"),
                "case {name:?}: an untagged policy reports no <tagSet>, got {body}"
            );
        } else {
            let actual = reported_tags(name, &body);
            assert_eq!(
                actual,
                expected_tags(case),
                "case {name:?}: expected the tags the request assigned, got {actual:?} in {body}"
            );
        }

        assert_eq!(
            stored_state(&engine, &metering_policy_id).as_deref(),
            Some(policy_state("pending")),
            "case {name:?}: the store must hold the policy in the state the create reported"
        );

        // The round trip: the describe reports the policy the create stored, member for
        // member, so the writer and every later reader cannot drift apart.
        let (status, read_back) = invoke(
            &router,
            DESCRIBE,
            &indexed(
                "TransitGatewayMeteringPolicyIds",
                &[metering_policy_id.as_str()],
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200 from {DESCRIBE}, got {status} with body {read_back}"
        );
        for element_name in ["transitGatewayMeteringPolicyId", "transitGatewayId"] {
            let expected = member(name, &body, element_name);
            let actual = member(name, &read_back, element_name);
            assert_eq!(
                actual, expected,
                "case {name:?}: expected <{element_name}> {expected:?} from {DESCRIBE}, got \
                 {actual:?} in {read_back}"
            );
        }
        let read_back_attachments = if expected_attachments.is_empty() {
            Vec::new()
        } else {
            reported_middlebox_attachments(name, &read_back)
        };
        assert_eq!(
            read_back_attachments, expected_attachments,
            "case {name:?}: expected the stored attachments from {DESCRIBE}, got \
             {read_back_attachments:?} in {read_back}"
        );
        let read_back_tags = if case.tags.is_empty() {
            Vec::new()
        } else {
            reported_tags(name, &read_back)
        };
        assert_eq!(
            read_back_tags,
            expected_tags(case),
            "case {name:?}: expected the stored tags from {DESCRIBE}, got {read_back_tags:?} in \
             {read_back}"
        );
    }

    assert_eq!(
        stored_policies(&engine),
        CASES.len(),
        "the store must hold one policy per create, and nothing else"
    );
}

/// The move the clock owes a create: a policy handed back `pending` is `available` once the
/// transition span has passed, whether or not anybody called again — and the instant its
/// update takes effect does not move when it gets there. Driven for a policy metering nothing
/// and one metering several, so the move does not depend on what the policy carries.
#[tokio::test]
async fn the_clock_owes_a_created_policy_the_state_a_policy_in_force_is_in() {
    let (router, engine, gateway_id, attachment_ids) = seeded().await;

    for case in [&CASES[0], &CASES[2]] {
        let name = case.name;
        let (status, body) = invoke(
            &router,
            ACTION,
            &create_params(&attachment_ids, &gateway_id, case),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200 from {ACTION}, got {status} with body {body}"
        );
        let metering_policy_id = member(name, &body, "transitGatewayMeteringPolicyId")
            .unwrap_or_else(|| panic!("case {name:?}: the create must report the policy's id"));
        let effective_at = member(name, &body, "updateEffectiveAt");

        tokio::time::sleep(past_the_transition()).await;
        let (status, read_back) = invoke(
            &router,
            DESCRIBE,
            &indexed(
                "TransitGatewayMeteringPolicyIds",
                &[metering_policy_id.as_str()],
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200 from {DESCRIBE}, got {status} with body {read_back}"
        );
        let actual = member(name, &read_back, "state");
        assert_eq!(
            actual.as_deref(),
            Some(policy_state("available")),
            "case {name:?}: a policy the span has passed for is in force, got {actual:?} in \
             {read_back}"
        );
        let still_effective_at = member(name, &read_back, "updateEffectiveAt");
        assert_eq!(
            still_effective_at, effective_at,
            "case {name:?}: the instant the update takes effect must not move, expected \
             {effective_at:?}, got {still_effective_at:?}"
        );
        assert_eq!(
            stored_state(&engine, &metering_policy_id).as_deref(),
            Some(policy_state("available")),
            "case {name:?}: the store must hold the state the describe reported"
        );
    }
}

/// One refused create: the fields it carries beyond the gateway id, and the refusal it must
/// answer.
struct Refusal {
    /// What the row is, for the assertion messages.
    name: &'static str,
    /// The `TransitGatewayId` the request names, or `None` for the gateway the fixture
    /// created.
    gateway_id: Option<&'static str>,
    /// The `MiddleboxAttachmentId.N` values the request names, as literals, beside the
    /// positions in the seeded list it names.
    middleboxes: &'static [&'static str],
    /// The positions in the seeded attachment list the request also names.
    seeded_middleboxes: &'static [usize],
    /// The resource type its tag specification names, with one tag, or `None` for no tag
    /// specification at all.
    tag_resource_type: Option<&'static str>,
    /// Whether the tag specification names a resource type at all — a specification with
    /// tags and no type is the one request `tag_resource_type` cannot express.
    untyped_tag_specification: bool,
    /// Whether the request asks for a dry run.
    dry_run: bool,
    /// The HTTP status it must answer.
    status: u16,
    /// The EC2 error code it must answer.
    code: &'static str,
}

/// The refusal every row below starts from: the fixture's own gateway, nothing else.
const CLEAN: Refusal = Refusal {
    name: "",
    gateway_id: None,
    middleboxes: &[],
    seeded_middleboxes: &[],
    tag_resource_type: None,
    untyped_tag_specification: false,
    dry_run: false,
    status: REFUSAL_STATUS,
    code: "",
};

/// The refusals this suite is written against: the gateway nothing holds, a middlebox id that
/// is not an attachment id at all, a well-formed middlebox id nothing holds (named beside a
/// usable one, so the refusal is not about the list's length), a tag specification naming
/// another resource type, a tag specification naming none, and a dry run — which is answered
/// about PERMISSION, so it wins even when the gateway named cannot be resolved.
fn refusals() -> Vec<Refusal> {
    vec![
        Refusal {
            name: "a transit gateway this account holds none of",
            gateway_id: Some("tgw-0123456789abcdef0"),
            code: INVALID_TRANSIT_GATEWAY_ID_NOT_FOUND,
            ..CLEAN
        },
        Refusal {
            name: "a middlebox id that is not an attachment id at all",
            middleboxes: &["middlebox-one"],
            code: INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_MALFORMED,
            ..CLEAN
        },
        Refusal {
            name: "a well-formed middlebox id this account holds no attachment for",
            seeded_middleboxes: &[0],
            middleboxes: &["tgw-attach-0123456789abcdef0"],
            code: INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND,
            ..CLEAN
        },
        Refusal {
            name: "a tag specification naming another resource type",
            tag_resource_type: Some("vpc"),
            code: INVALID_PARAMETER_VALUE,
            ..CLEAN
        },
        Refusal {
            name: "a tag specification naming no resource type",
            untyped_tag_specification: true,
            code: MISSING_PARAMETER,
            ..CLEAN
        },
        Refusal {
            name: "a dry run",
            dry_run: true,
            status: DRY_RUN_STATUS,
            code: DRY_RUN_OPERATION,
            ..CLEAN
        },
        Refusal {
            name: "a dry run naming a transit gateway this account holds none of",
            gateway_id: Some("tgw-0123456789abcdef0"),
            dry_run: true,
            status: DRY_RUN_STATUS,
            code: DRY_RUN_OPERATION,
            ..CLEAN
        },
    ]
}

/// A request this operation refuses answers that refusal's own code and stores NOTHING, so a
/// create either persists the whole policy or none of it.
#[tokio::test]
async fn an_unusable_create_is_refused_and_stores_nothing() {
    let (router, engine, gateway_id, attachment_ids) = seeded().await;

    for refusal in refusals() {
        let name = refusal.name;
        let mut params = vec![param(
            "TransitGatewayId",
            refusal.gateway_id.unwrap_or(gateway_id.as_str()),
        )];
        let mut middleboxes = at(&attachment_ids, refusal.seeded_middleboxes);
        middleboxes.extend(refusal.middleboxes.iter().map(|id| (*id).to_string()));
        params.extend(middlebox_params(&middleboxes));
        if let Some(resource_type) = refusal.tag_resource_type {
            params.extend(tag_params(resource_type, &[("Name", "north")]));
        }
        if refusal.untyped_tag_specification {
            params.push(param("TagSpecifications.1.Tag.1.Key", "Name"));
            params.push(param("TagSpecifications.1.Tag.1.Value", "north"));
        }
        if refusal.dry_run {
            params.push(param("DryRun", "true"));
        }

        let (status, body) = invoke(&router, ACTION, &params).await;
        assert_eq!(
            status, refusal.status,
            "case {name:?}: expected HTTP {expected} from {ACTION}, got {status} with body \
             {body}",
            expected = refusal.status
        );
        let code = error_code(&body);
        assert_eq!(
            code, refusal.code,
            "case {name:?}: expected the {expected:?} refusal, got {code:?} in {body}",
            expected = refusal.code
        );
        assert_eq!(
            stored_policies(&engine),
            0,
            "case {name:?}: a refused create must leave the metering policy store empty"
        );
    }
}
