// SPDX-License-Identifier: CC0-1.0

//! The entries (rules) of a transit gateway metering policy, driven over the wire this service
//! speaks (`ec2Query`): what `CreateTransitGatewayMeteringPolicyEntry` stores on a stored
//! policy and reports back, what `GetTransitGatewayMeteringPolicyEntries` reports out of that
//! store, and what either refuses.
//!
//! The rule under test is the one every EC2 create of a numbered entry follows: one call adds
//! the entry the request describes to the parent the request names and PERSISTS it, reporting
//! the record it stored — the facts the request chose (the rule number, the account the
//! traffic is attributed to, and each traffic-matching criterion, with a CIDR block kept in
//! its canonical form) as given, and everything the request does not choose derived by the
//! service (the state its catalog admits for an entry in force, the instant the entry was
//! updated, and the instant its update takes effect). The parent and every id the request
//! names are resolved BEFORE anything is written, in that order, so a value that is not an id
//! of its kind, an id nothing holds, a rule number the policy already holds an entry at and a
//! dry run are each refused with nothing stored. What is stored is what the read reports, in
//! "lowest to highest number" order.
//!
//! Each table row is a distinct input along what that rule generalises over: WHICH rule number
//! (the lowest one, small ones, a large one, the largest an `Integer` holds), which account the
//! traffic is attributed to (every member of the modelled payer catalog), and WHICH criteria
//! the rule carries (none at all, a source end only, a destination end only, both ends, an IPv4
//! block spelled off its canonical form, an IPv6 block, a single port and a port range, a
//! protocol number and every protocol) — and, for the refusals, which member of the request is
//! unusable and how.
//!
//! The transit gateway and the metering policy each case adds entries to are created over the
//! wire through `CreateTransitGateway` and `CreateTransitGatewayMeteringPolicy`; the
//! attachments a rule matches on are seeded through the store's own writer
//! (`insert_transit_gateway_attachment`), as every transit gateway suite in this crate does,
//! because `CreateTransitGatewayVpcAttachment`'s own suite owns that path.

use std::collections::BTreeMap;

use ec2::ops::types::{
    TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_VALUES, TRANSIT_GATEWAY_METERING_PAYER_TYPE_VALUES,
    TRANSIT_GATEWAY_METERING_POLICY_ENTRY_STATE_VALUES, TransitGatewayVpcAttachment,
};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, DRY_RUN_OPERATION,
    INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_MALFORMED,
    INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND, INVALID_PARAMETER_VALUE,
    INVALID_TRANSIT_GATEWAY_METERING_POLICY_ID_MALFORMED,
    INVALID_TRANSIT_GATEWAY_METERING_POLICY_ID_NOT_FOUND,
    MIN_TRANSIT_GATEWAY_METERING_POLICY_RULE_NUMBER, RESOURCE_TYPE_TRANSIT_GATEWAY_METERING_POLICY,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
    TRANSIT_GATEWAY_METERING_POLICY_ENTRY_ALREADY_EXISTS, TransitGatewayAttachmentView,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, element, error_code, error_message, filters};
use super::wire::{Param, child, child_text, document, param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The ec2Query `Action` under test.
const ACTION: &str = "CreateTransitGatewayMeteringPolicyEntry";

/// The read that reports a policy's stored entries back.
const GET: &str = "GetTransitGatewayMeteringPolicyEntries";

/// The HTTP status EC2 answers a refused request with.
const REFUSAL_STATUS: u16 = 400;

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

/// How many attachments every case seeds, so a rule can match on one at either end.
const SEEDED_ATTACHMENTS: usize = 2;

/// A member of `TransitGatewayMeteringPolicyEntryState`, from the modelled catalog
/// (`smithy_query(query="shape",shape="TransitGatewayMeteringPolicyEntryState")`, whose values
/// are `available | deleted`) rather than written out here.
fn entry_state(name: &str) -> &'static str {
    TRANSIT_GATEWAY_METERING_POLICY_ENTRY_STATE_VALUES
        .iter()
        .find(|value| **value == name)
        .unwrap_or_else(|| panic!("{name:?} is no TransitGatewayMeteringPolicyEntryState member"))
}

/// A member of `TransitGatewayMeteringPayerType`, from the modelled catalog
/// (`smithy_query(query="shape",shape="TransitGatewayMeteringPayerType")`, whose values are
/// `source-attachment-owner | destination-attachment-owner | transit-gateway-owner`).
fn payer_type(name: &str) -> &'static str {
    TRANSIT_GATEWAY_METERING_PAYER_TYPE_VALUES
        .iter()
        .find(|value| **value == name)
        .unwrap_or_else(|| panic!("{name:?} is no TransitGatewayMeteringPayerType member"))
}

/// A member of `TransitGatewayAttachmentResourceType`, from the modelled catalog
/// (`smithy_query(query="shape",shape="TransitGatewayAttachmentResourceType")`).
fn attachment_type(name: &str) -> &'static str {
    TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_VALUES
        .iter()
        .find(|value| **value == name)
        .unwrap_or_else(|| panic!("{name:?} is no TransitGatewayAttachmentResourceType member"))
}

/// One call of `action` signed for the Region above.
async fn invoke(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// The emulator with one transit gateway, two attachments of it and one metering policy: the
/// router to call, the engine to read the store through, the policy's id, and the attachment
/// ids a rule may match on.
async fn seeded() -> (
    MultiServiceRouter,
    std::sync::Arc<engine::InMemoryEngine>,
    String,
    Vec<String>,
) {
    let (router, engine) = super::ec2_query::emulator();
    let (status, body) = invoke(&router, "CreateTransitGateway", &[]).await;
    assert_eq!(
        status, 200,
        "CreateTransitGateway must succeed, got {status} with body {body}"
    );
    let gateway_id =
        element(&body, "transitGatewayId").expect("a created transit gateway reports its own id");

    let attachment_ids: Vec<String> = {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        (0..SEEDED_ATTACHMENTS)
            .map(|index| {
                guard
                    .insert_transit_gateway_attachment(TransitGatewayAttachmentView::Vpc(
                        TransitGatewayVpcAttachment {
                            transit_gateway_id: Some(gateway_id.clone()),
                            vpc_id: Some(format!("vpc-0123456789abcde{index:0>2x}")),
                            vpc_owner_id: Some("210987654321".to_string()),
                            state: Some(TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE.to_string()),
                            ..Default::default()
                        },
                    ))
                    .expect("the store holds the seeded VPC attachment")
            })
            .collect()
    };

    let metering_policy_id = metering_policy(&router, &gateway_id).await;
    (router, engine, metering_policy_id, attachment_ids)
}

/// A metering policy of `gateway_id`, created over the wire, as its
/// `TransitGatewayMeteringPolicyId`.
async fn metering_policy(router: &MultiServiceRouter, gateway_id: &str) -> String {
    let (status, body) = invoke(
        router,
        "CreateTransitGatewayMeteringPolicy",
        &[param("TransitGatewayId", gateway_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateTransitGatewayMeteringPolicy must succeed, got {status} with body {body}"
    );
    element(&body, "transitGatewayMeteringPolicyId")
        .expect("a created metering policy reports its own id")
}

/// What a reported metering policy entry carries, read off the wire.
///
/// The whole record rather than a member at a time, so the round trip can assert that the read
/// reports exactly what the create answered — including the two instants, which must not move.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    /// `policyRuleNumber`, the entry's own number as a string.
    rule_number: Option<String>,
    /// `meteredAccount`, the account the traffic is attributed to.
    metered_account: Option<String>,
    /// `state`, the entry's state.
    state: Option<String>,
    /// `updatedAt`, when the entry was last updated.
    updated_at: Option<String>,
    /// `updateEffectiveAt`, when its update takes effect.
    update_effective_at: Option<String>,
    /// Whether the entry carries a `meteringPolicyRule` element at all.
    has_rule: bool,
    /// The children of that rule, by element name.
    rule: BTreeMap<String, String>,
}

/// Every metering policy entry a response carries, in document order: the elements that have a
/// `policyRuleNumber` child, which is the one member every reported entry carries — so the one
/// entry a create answers and the `<item>`s of a read's set are read the same way.
fn reported_entries(case: &str, body: &str) -> Vec<Reported> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && child(*node, "policyRuleNumber").is_some())
        .map(|node| {
            let rule = child(node, "meteringPolicyRule");
            Reported {
                rule_number: child_text(node, "policyRuleNumber"),
                metered_account: child_text(node, "meteredAccount"),
                state: child_text(node, "state"),
                updated_at: child_text(node, "updatedAt"),
                update_effective_at: child_text(node, "updateEffectiveAt"),
                has_rule: rule.is_some(),
                rule: rule
                    .map(|rule| {
                        rule.children()
                            .filter(roxmltree::Node::is_element)
                            .map(|member| {
                                (
                                    member.tag_name().name().to_string(),
                                    member.text().unwrap_or_default().to_string(),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            }
        })
        .collect()
}

/// The ONE entry a response carries, or a panic naming the case.
fn reported_entry(case: &str, body: &str) -> Reported {
    let entries = reported_entries(case, body);
    assert_eq!(
        entries.len(),
        1,
        "case {case:?}: expected exactly one metering policy entry, got {count} in {body}",
        count = entries.len()
    );
    entries.into_iter().next().expect("one entry")
}

/// The rule numbers the store holds for `metering_policy_id`, in stored order.
fn stored_rule_numbers(engine: &engine::InMemoryEngine, metering_policy_id: &str) -> Vec<i32> {
    engine
        .state::<Ec2State>()
        .lock()
        .transit_gateway_metering_policy_entries
        .get(metering_policy_id)
        .map(|entries| entries.keys().copied().collect())
        .unwrap_or_default()
}

/// One created entry: the rule number and metered account it names, and each traffic-matching
/// criterion it carries — the two attachments by POSITION in the seeded list, so a row names a
/// real one.
struct Case {
    /// What the row is, for the assertion messages.
    name: &'static str,
    /// The `PolicyRuleNumber` the request names.
    rule_number: i32,
    /// The `MeteredAccount` the request names, a member of the modelled payer catalog.
    metered_account: &'static str,
    /// The position in the seeded attachment list the rule's source end names.
    source_attachment: Option<usize>,
    /// The `SourceTransitGatewayAttachmentType` the request names.
    source_attachment_type: Option<&'static str>,
    /// The `SourceCidrBlock` the request names, and the canonical form it must be stored in.
    source_cidr: Option<(&'static str, &'static str)>,
    /// The `SourcePortRange` the request names.
    source_port_range: Option<&'static str>,
    /// The position in the seeded attachment list the rule's destination end names.
    destination_attachment: Option<usize>,
    /// The `DestinationTransitGatewayAttachmentType` the request names.
    destination_attachment_type: Option<&'static str>,
    /// The `DestinationCidrBlock` the request names, and its canonical form.
    destination_cidr: Option<(&'static str, &'static str)>,
    /// The `DestinationPortRange` the request names.
    destination_port_range: Option<&'static str>,
    /// The `Protocol` the request names.
    protocol: Option<&'static str>,
}

/// The entry every row below starts from: a rule number and a metered account, no criteria.
const CLEAN: Case = Case {
    name: "",
    rule_number: 0,
    metered_account: "",
    source_attachment: None,
    source_attachment_type: None,
    source_cidr: None,
    source_port_range: None,
    destination_attachment: None,
    destination_attachment_type: None,
    destination_cidr: None,
    destination_port_range: None,
    protocol: None,
};

/// The creates this suite is written against.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "the lowest rule number, with no matching criterion at all",
            rule_number: MIN_TRANSIT_GATEWAY_METERING_POLICY_RULE_NUMBER,
            metered_account: payer_type("source-attachment-owner"),
            ..CLEAN
        },
        Case {
            name: "a source attachment and type, one port and a protocol number",
            rule_number: 5,
            metered_account: payer_type("destination-attachment-owner"),
            source_attachment: Some(0),
            source_attachment_type: Some(attachment_type("vpc")),
            source_port_range: Some("443"),
            protocol: Some("6"),
            ..CLEAN
        },
        Case {
            name: "both ends, an IPv4 block named off its canonical form and a port range",
            rule_number: 100,
            metered_account: payer_type("transit-gateway-owner"),
            source_attachment: Some(0),
            source_attachment_type: Some(attachment_type("vpc")),
            source_cidr: Some(("10.0.1.5/24", "10.0.1.0/24")),
            source_port_range: Some("1024-2048"),
            destination_attachment: Some(1),
            destination_attachment_type: Some(attachment_type("peering")),
            destination_cidr: Some(("192.168.0.0/16", "192.168.0.0/16")),
            destination_port_range: Some("0-65535"),
            protocol: Some("17"),
        },
        Case {
            name: "an IPv6 block at either end and every protocol",
            rule_number: 7_000,
            metered_account: payer_type("source-attachment-owner"),
            source_cidr: Some(("2001:db8:0:0::/64", "2001:db8::/64")),
            destination_cidr: Some(("2001:db8:1::/48", "2001:db8:1::/48")),
            protocol: Some("-1"),
            ..CLEAN
        },
        Case {
            name: "a destination end only",
            rule_number: 200,
            metered_account: payer_type("destination-attachment-owner"),
            destination_attachment: Some(1),
            destination_attachment_type: Some(attachment_type("connect")),
            ..CLEAN
        },
        Case {
            name: "the largest rule number an Integer holds",
            rule_number: i32::MAX,
            metered_account: payer_type("transit-gateway-owner"),
            protocol: Some("255"),
            ..CLEAN
        },
    ]
}

/// The form fields one case's create carries, beside the policy it names.
fn create_params(metering_policy_id: &str, attachment_ids: &[String], case: &Case) -> Vec<Param> {
    let mut params = vec![
        param("TransitGatewayMeteringPolicyId", metering_policy_id),
        param("PolicyRuleNumber", &case.rule_number.to_string()),
        param("MeteredAccount", case.metered_account),
    ];
    if let Some(position) = case.source_attachment {
        params.push(param(
            "SourceTransitGatewayAttachmentId",
            &attachment_ids[position],
        ));
    }
    if let Some(kind) = case.source_attachment_type {
        params.push(param("SourceTransitGatewayAttachmentType", kind));
    }
    if let Some((named, _)) = case.source_cidr {
        params.push(param("SourceCidrBlock", named));
    }
    if let Some(range) = case.source_port_range {
        params.push(param("SourcePortRange", range));
    }
    if let Some(position) = case.destination_attachment {
        params.push(param(
            "DestinationTransitGatewayAttachmentId",
            &attachment_ids[position],
        ));
    }
    if let Some(kind) = case.destination_attachment_type {
        params.push(param("DestinationTransitGatewayAttachmentType", kind));
    }
    if let Some((named, _)) = case.destination_cidr {
        params.push(param("DestinationCidrBlock", named));
    }
    if let Some(range) = case.destination_port_range {
        params.push(param("DestinationPortRange", range));
    }
    if let Some(protocol) = case.protocol {
        params.push(param("Protocol", protocol));
    }
    params
}

/// The rule the created entry must report: every criterion the request named, by the element
/// name the model binds that member to, and nothing else.
fn expected_rule(case: &Case, attachment_ids: &[String]) -> BTreeMap<String, String> {
    let mut rule: BTreeMap<String, String> = BTreeMap::new();
    if let Some(position) = case.source_attachment {
        rule.insert(
            "sourceTransitGatewayAttachmentId".to_string(),
            attachment_ids[position].clone(),
        );
    }
    if let Some(kind) = case.source_attachment_type {
        rule.insert(
            "sourceTransitGatewayAttachmentType".to_string(),
            kind.to_string(),
        );
    }
    if let Some((_, canonical)) = case.source_cidr {
        rule.insert("sourceCidrBlock".to_string(), canonical.to_string());
    }
    if let Some(range) = case.source_port_range {
        rule.insert("sourcePortRange".to_string(), range.to_string());
    }
    if let Some(position) = case.destination_attachment {
        rule.insert(
            "destinationTransitGatewayAttachmentId".to_string(),
            attachment_ids[position].clone(),
        );
    }
    if let Some(kind) = case.destination_attachment_type {
        rule.insert(
            "destinationTransitGatewayAttachmentType".to_string(),
            kind.to_string(),
        );
    }
    if let Some((_, canonical)) = case.destination_cidr {
        rule.insert("destinationCidrBlock".to_string(), canonical.to_string());
    }
    if let Some(range) = case.destination_port_range {
        rule.insert("destinationPortRange".to_string(), range.to_string());
    }
    if let Some(protocol) = case.protocol {
        rule.insert("protocol".to_string(), protocol.to_string());
    }
    rule
}

/// Every fact a created entry carries is the one the request named or the one the service
/// derives, the create reports it, the store holds it, and it reads back identically through
/// the read — one row per case, so no single request is the specification.
#[tokio::test]
async fn a_created_entry_reports_and_persists_what_the_request_named() {
    let (router, engine, metering_policy_id, attachment_ids) = seeded().await;
    let mut created: Vec<Reported> = Vec::new();

    for case in cases() {
        let name = case.name;
        let (status, body) = invoke(
            &router,
            ACTION,
            &create_params(&metering_policy_id, &attachment_ids, &case),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200 from {ACTION}, got {status} with body {body}"
        );

        let reported = reported_entry(name, &body);
        assert_eq!(
            reported.rule_number.as_deref(),
            Some(case.rule_number.to_string().as_str()),
            "case {name:?}: expected the rule number the request named, got {actual:?} in {body}",
            actual = reported.rule_number
        );
        assert_eq!(
            reported.metered_account.as_deref(),
            Some(case.metered_account),
            "case {name:?}: expected the metered account the request named, got {actual:?} in \
             {body}",
            actual = reported.metered_account
        );
        assert_eq!(
            reported.state.as_deref(),
            Some(entry_state("available")),
            "case {name:?}: expected the state an entry in force is in, got {actual:?} in {body}",
            actual = reported.state
        );
        let updated_at = reported.updated_at.as_deref().and_then(as_timestamp);
        let effective_at = reported
            .update_effective_at
            .as_deref()
            .and_then(as_timestamp);
        assert!(
            updated_at.is_some() && effective_at.is_some(),
            "case {name:?}: expected both instants the entry carries, got {updated:?} and \
             {effective:?} in {body}",
            updated = reported.updated_at,
            effective = reported.update_effective_at
        );
        assert!(
            effective_at >= updated_at,
            "case {name:?}: the update cannot take effect before it was made, got {updated:?} \
             then {effective:?}",
            updated = reported.updated_at,
            effective = reported.update_effective_at
        );

        let expected = expected_rule(&case, &attachment_ids);
        if expected.is_empty() {
            assert!(
                !reported.has_rule,
                "case {name:?}: an entry whose request named no criterion reports no \
                 <meteringPolicyRule>, got {body}"
            );
        } else {
            assert_eq!(
                reported.rule, expected,
                "case {name:?}: expected the criteria the request named, got {actual:?} in {body}",
                actual = reported.rule
            );
        }

        assert!(
            stored_rule_numbers(&engine, &metering_policy_id).contains(&case.rule_number),
            "case {name:?}: the store must hold the entry at rule number {number}, holding \
             {held:?}",
            number = case.rule_number,
            held = stored_rule_numbers(&engine, &metering_policy_id)
        );
        created.push(reported);
    }

    // The round trip: the read reports every entry the creates stored, record for record and in
    // "lowest to highest number" order, so the writer and every later reader cannot drift apart.
    let (status, body) = invoke(
        &router,
        GET,
        &[param("TransitGatewayMeteringPolicyId", &metering_policy_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from {GET}, got {status} with body {body}"
    );
    let read_back = reported_entries("the whole policy", &body);
    created.sort_by_key(|entry| {
        entry
            .rule_number
            .as_deref()
            .and_then(|number| number.parse::<i32>().ok())
            .unwrap_or_default()
    });
    assert_eq!(
        read_back, created,
        "expected {GET} to report the entries the creates stored, got {read_back:?} in {body}"
    );
}

/// One refused create: the members it carries beyond the policy, and the refusal it must
/// answer.
struct Refusal {
    /// What the row is, for the assertion messages.
    name: &'static str,
    /// The `TransitGatewayMeteringPolicyId` it names, or `None` for the fixture's own policy.
    metering_policy_id: Option<&'static str>,
    /// The `PolicyRuleNumber` it names.
    rule_number: i32,
    /// The `SourceTransitGatewayAttachmentId` it names, as a literal.
    source_attachment_id: Option<&'static str>,
    /// The `DestinationTransitGatewayAttachmentId` it names, as a literal.
    destination_attachment_id: Option<&'static str>,
    /// The `SourceCidrBlock` it names.
    source_cidr: Option<&'static str>,
    /// The `SourcePortRange` it names.
    source_port_range: Option<&'static str>,
    /// The `Protocol` it names.
    protocol: Option<&'static str>,
    /// Whether it asks for a dry run.
    dry_run: bool,
    /// The HTTP status it must answer.
    status: u16,
    /// The EC2 error code it must answer.
    code: &'static str,
}

/// The refusal every row below starts from: the fixture's own policy at a free rule number.
const CLEAN_REFUSAL: Refusal = Refusal {
    name: "",
    metering_policy_id: None,
    rule_number: 11,
    source_attachment_id: None,
    destination_attachment_id: None,
    source_cidr: None,
    source_port_range: None,
    protocol: None,
    dry_run: false,
    status: REFUSAL_STATUS,
    code: "",
};

/// The refusals this suite is written against: four shapes of value that is not a metering
/// policy id, a well-formed id no policy of the account has, a rule number below the lowest
/// one, an attachment id that is not one at all, a well-formed attachment id nothing holds at
/// either end, a CIDR block that is no block, a port range that is no range and one whose ports
/// are out of order, a protocol outside the protocol numbers, and a dry run — which is answered
/// about PERMISSION, so it wins even when the policy named cannot be resolved.
fn refusals() -> Vec<Refusal> {
    vec![
        Refusal {
            name: "a policy id whose suffix is 16 hexadecimal digits",
            metering_policy_id: Some("tgw-mp-0123456789abcdef"),
            code: INVALID_TRANSIT_GATEWAY_METERING_POLICY_ID_MALFORMED,
            ..CLEAN_REFUSAL
        },
        Refusal {
            name: "a policy id whose suffix is not hexadecimal",
            metering_policy_id: Some("tgw-mp-not-hex"),
            code: INVALID_TRANSIT_GATEWAY_METERING_POLICY_ID_MALFORMED,
            ..CLEAN_REFUSAL
        },
        Refusal {
            name: "a value that is no resource id at all",
            metering_policy_id: Some("metering-policy-one"),
            code: INVALID_TRANSIT_GATEWAY_METERING_POLICY_ID_MALFORMED,
            ..CLEAN_REFUSAL
        },
        Refusal {
            name: "an id of another resource of the same family",
            metering_policy_id: Some("tgw-attach-0123456789abcdef0"),
            code: INVALID_TRANSIT_GATEWAY_METERING_POLICY_ID_MALFORMED,
            ..CLEAN_REFUSAL
        },
        Refusal {
            name: "a well-formed policy id this account holds no policy for",
            metering_policy_id: Some("tgw-mp-0123456789abcdef0"),
            code: INVALID_TRANSIT_GATEWAY_METERING_POLICY_ID_NOT_FOUND,
            ..CLEAN_REFUSAL
        },
        Refusal {
            name: "a rule number below the lowest one",
            rule_number: MIN_TRANSIT_GATEWAY_METERING_POLICY_RULE_NUMBER - 1,
            code: INVALID_PARAMETER_VALUE,
            ..CLEAN_REFUSAL
        },
        Refusal {
            name: "a negative rule number",
            rule_number: -12,
            code: INVALID_PARAMETER_VALUE,
            ..CLEAN_REFUSAL
        },
        Refusal {
            name: "a source attachment id that is not an attachment id at all",
            source_attachment_id: Some("middlebox-one"),
            code: INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_MALFORMED,
            ..CLEAN_REFUSAL
        },
        Refusal {
            name: "a well-formed source attachment id this account holds nothing for",
            source_attachment_id: Some("tgw-attach-0123456789abcdef0"),
            code: INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND,
            ..CLEAN_REFUSAL
        },
        Refusal {
            name: "a well-formed destination attachment id this account holds nothing for",
            destination_attachment_id: Some("tgw-attach-0123456789abcdef0"),
            code: INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND,
            ..CLEAN_REFUSAL
        },
        Refusal {
            name: "a source CIDR block that is no CIDR block",
            source_cidr: Some("10.0.0.0"),
            code: INVALID_PARAMETER_VALUE,
            ..CLEAN_REFUSAL
        },
        Refusal {
            name: "a source CIDR block whose prefix length is no number",
            source_cidr: Some("10.0.0.0/wide"),
            code: INVALID_PARAMETER_VALUE,
            ..CLEAN_REFUSAL
        },
        Refusal {
            name: "a port range that is no range of ports",
            source_port_range: Some("http"),
            code: INVALID_PARAMETER_VALUE,
            ..CLEAN_REFUSAL
        },
        Refusal {
            name: "a port range whose ports are out of order",
            source_port_range: Some("443-80"),
            code: INVALID_PARAMETER_VALUE,
            ..CLEAN_REFUSAL
        },
        Refusal {
            name: "a port above the ports a port number holds",
            source_port_range: Some("65536"),
            code: INVALID_PARAMETER_VALUE,
            ..CLEAN_REFUSAL
        },
        Refusal {
            name: "a protocol above the protocol numbers",
            protocol: Some("256"),
            code: INVALID_PARAMETER_VALUE,
            ..CLEAN_REFUSAL
        },
        Refusal {
            name: "a dry run",
            dry_run: true,
            status: DRY_RUN_STATUS,
            code: DRY_RUN_OPERATION,
            ..CLEAN_REFUSAL
        },
        Refusal {
            name: "a dry run naming a policy this account holds none of",
            metering_policy_id: Some("tgw-mp-0123456789abcdef0"),
            dry_run: true,
            status: DRY_RUN_STATUS,
            code: DRY_RUN_OPERATION,
            ..CLEAN_REFUSAL
        },
    ]
}

/// A request this operation refuses answers that refusal's own code and stores NOTHING, so a
/// create either persists the whole entry or none of it.
#[tokio::test]
async fn an_unusable_create_is_refused_and_stores_nothing() {
    let (router, engine, metering_policy_id, _attachment_ids) = seeded().await;

    for refusal in refusals() {
        let name = refusal.name;
        let mut params = vec![
            param(
                "TransitGatewayMeteringPolicyId",
                refusal
                    .metering_policy_id
                    .unwrap_or(metering_policy_id.as_str()),
            ),
            param("PolicyRuleNumber", &refusal.rule_number.to_string()),
            param("MeteredAccount", payer_type("transit-gateway-owner")),
        ];
        if let Some(attachment_id) = refusal.source_attachment_id {
            params.push(param("SourceTransitGatewayAttachmentId", attachment_id));
        }
        if let Some(attachment_id) = refusal.destination_attachment_id {
            params.push(param("DestinationTransitGatewayAttachmentId", attachment_id));
        }
        if let Some(cidr) = refusal.source_cidr {
            params.push(param("SourceCidrBlock", cidr));
        }
        if let Some(range) = refusal.source_port_range {
            params.push(param("SourcePortRange", range));
        }
        if let Some(protocol) = refusal.protocol {
            params.push(param("Protocol", protocol));
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
        assert!(
            stored_rule_numbers(&engine, &metering_policy_id).is_empty(),
            "case {name:?}: a refused create must leave the policy without entries, holding \
             {held:?}",
            held = stored_rule_numbers(&engine, &metering_policy_id)
        );
    }
}

/// A value outside the metering policy id form is refused as MALFORMED, naming the resource and
/// the value the request sent, whatever the value's shape — the answer a client tells "not an id
/// of this kind" from "no such policy" by. Asserted on the message as well as the code, because
/// the message is what a caller reads.
#[tokio::test]
async fn a_value_outside_the_policy_id_form_is_named_malformed() {
    let (router, _engine, metering_policy_id, _attachment_ids) = seeded().await;

    for named in [
        "tgw-mp-0123456789abcdef",
        "tgw-mp-not-hex",
        "tgw-mp-",
        "metering-policy-one",
        "TGW-MP-0123456789ABCDEF0",
    ] {
        for (action, params) in [
            (
                ACTION,
                vec![
                    param("TransitGatewayMeteringPolicyId", named),
                    param("PolicyRuleNumber", "1"),
                    param("MeteredAccount", payer_type("transit-gateway-owner")),
                ],
            ),
            (
                GET,
                vec![param("TransitGatewayMeteringPolicyId", named)],
            ),
        ] {
            let case = format!("{action} naming {named:?}");
            let (status, body) = invoke(&router, action, &params).await;
            assert_eq!(
                status, REFUSAL_STATUS,
                "case {case:?}: expected HTTP {REFUSAL_STATUS}, got {status} with body {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, INVALID_TRANSIT_GATEWAY_METERING_POLICY_ID_MALFORMED,
                "case {case:?}: expected the malformed-id refusal, got {code:?} in {body}"
            );
            let expected = format!(
                "The {RESOURCE_TYPE_TRANSIT_GATEWAY_METERING_POLICY} ID '{named}' is malformed"
            );
            let message = error_message(&body);
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?} in {body}"
            );
        }
    }

    // The same call with an id the account DOES hold is served, so the refusals above are about
    // the id's form and nothing else.
    let (status, body) = invoke(
        &router,
        GET,
        &[param("TransitGatewayMeteringPolicyId", &metering_policy_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "a policy the account holds must be served, got {status} with body {body}"
    );
}

/// One rule number names one entry of one policy: a second create at a number the policy
/// already holds an entry at is refused, the entry it holds is untouched, and the same number
/// is free in ANOTHER policy.
#[tokio::test]
async fn one_rule_number_names_one_entry_of_one_policy() {
    let (router, engine, metering_policy_id, _attachment_ids) = seeded().await;
    let gateway_id = element(
        &invoke(&router, "CreateTransitGateway", &[]).await.1,
        "transitGatewayId",
    )
    .expect("a created transit gateway reports its own id");
    let other_policy_id = metering_policy(&router, &gateway_id).await;

    for rule_number in [MIN_TRANSIT_GATEWAY_METERING_POLICY_RULE_NUMBER, 42] {
        let case = format!("rule number {rule_number}");
        let params = vec![
            param("TransitGatewayMeteringPolicyId", &metering_policy_id),
            param("PolicyRuleNumber", &rule_number.to_string()),
            param("MeteredAccount", payer_type("source-attachment-owner")),
            param("Protocol", "6"),
        ];
        let (status, body) = invoke(&router, ACTION, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first create must succeed, got {status} with body {body}"
        );
        let first = reported_entry(&case, &body);

        // The second create at that number is refused, whatever else it carries.
        let mut repeat = params.clone();
        repeat.push(param("SourceCidrBlock", "10.0.0.0/8"));
        let (status, body) = invoke(&router, ACTION, &repeat).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected HTTP {REFUSAL_STATUS} for a repeated rule number, got \
             {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, TRANSIT_GATEWAY_METERING_POLICY_ENTRY_ALREADY_EXISTS,
            "case {case:?}: expected the already-exists refusal, got {code:?} in {body}"
        );

        // The entry the policy holds is the one the first create stored, unchanged.
        let (status, body) = invoke(
            &router,
            GET,
            &[
                param("TransitGatewayMeteringPolicyId", &metering_policy_id),
                param("Filter.1.Name", "policy-rule-number"),
                param("Filter.1.Value.1", &rule_number.to_string()),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {GET}, got {status} with body {body}"
        );
        let held = reported_entry(&case, &body);
        assert_eq!(
            held, first,
            "case {case:?}: a refused repeat must leave the stored entry untouched, got \
             {held:?} in {body}"
        );

        // The same rule number is free in another policy: the number names an entry WITHIN its
        // policy.
        let (status, body) = invoke(
            &router,
            ACTION,
            &[
                param("TransitGatewayMeteringPolicyId", &other_policy_id),
                param("PolicyRuleNumber", &rule_number.to_string()),
                param("MeteredAccount", payer_type("transit-gateway-owner")),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the same rule number must be free in another policy, got {status} \
             with body {body}"
        );
    }

    assert_eq!(
        stored_rule_numbers(&engine, &metering_policy_id),
        vec![MIN_TRANSIT_GATEWAY_METERING_POLICY_RULE_NUMBER, 42],
        "the store must hold one entry per accepted create"
    );
}

/// The read reports a policy's entries in "lowest to highest number" order whatever order they
/// were created in, pages them at the size the request asks for, and resumes exactly where its
/// token says — and a token minted while paging another policy's entries is refused rather than
/// resumed.
#[tokio::test]
async fn the_entries_of_a_policy_are_reported_in_rule_number_order_and_paged() {
    let (router, _engine, metering_policy_id, _attachment_ids) = seeded().await;
    // Created out of order, so the order reported cannot be the order of creation.
    let created: Vec<i32> = vec![900, 1, 70, 12, 3, 1_000, 5];
    for rule_number in &created {
        let (status, body) = invoke(
            &router,
            ACTION,
            &[
                param("TransitGatewayMeteringPolicyId", &metering_policy_id),
                param("PolicyRuleNumber", &rule_number.to_string()),
                param("MeteredAccount", payer_type("source-attachment-owner")),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "the create at rule number {rule_number} must succeed, got {status} with body {body}"
        );
    }
    let mut ascending: Vec<String> = created.iter().map(i32::to_string).collect();
    ascending.sort_by_key(|number| number.parse::<i32>().unwrap_or_default());

    // Unpaged: every entry, in ascending rule-number order, with no token.
    let (status, body) = invoke(
        &router,
        GET,
        &[param("TransitGatewayMeteringPolicyId", &metering_policy_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from {GET}, got {status} with body {body}"
    );
    let reported: Vec<String> = reported_entries("unpaged", &body)
        .into_iter()
        .filter_map(|entry| entry.rule_number)
        .collect();
    assert_eq!(
        reported, ascending,
        "expected the entries in rule-number order, got {reported:?} in {body}"
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "a page nothing follows carries no token, got {body}"
    );

    // Paged at the smallest page the model publishes: the pages partition the same order.
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..4 {
        let mut params = vec![
            param("TransitGatewayMeteringPolicyId", &metering_policy_id),
            param("MaxResults", "5"),
        ];
        if let Some(token) = token.as_deref() {
            params.push(param("NextToken", token));
        }
        let (status, body) = invoke(&router, GET, &params).await;
        assert_eq!(
            status, 200,
            "page {page}: expected HTTP 200 from {GET}, got {status} with body {body}"
        );
        let numbers: Vec<String> = reported_entries(&format!("page {page}"), &body)
            .into_iter()
            .filter_map(|entry| entry.rule_number)
            .collect();
        assert!(
            numbers.len() <= 5,
            "page {page}: a page cannot carry more than the size asked for, got {numbers:?}"
        );
        walked.extend(numbers);
        token = element(&body, "nextToken");
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        walked, ascending,
        "expected the walk to report every entry once, in order, got {walked:?}"
    );

    // A token minted for ANOTHER policy's entries names no position in this one.
    let gateway_id = element(
        &invoke(&router, "CreateTransitGateway", &[]).await.1,
        "transitGatewayId",
    )
    .expect("a created transit gateway reports its own id");
    let other_policy_id = metering_policy(&router, &gateway_id).await;
    let (status, body) = invoke(
        &router,
        GET,
        &[
            param("TransitGatewayMeteringPolicyId", &metering_policy_id),
            param("MaxResults", "5"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from {GET}, got {status} with body {body}"
    );
    let borrowed = element(&body, "nextToken").expect("a short page of a longer set mints a token");
    for (case, policy_id, token) in [
        ("another policy's token", other_policy_id.as_str(), borrowed.as_str()),
        ("a token this service did not mint", metering_policy_id.as_str(), "not-a-token"),
    ] {
        let (status, body) = invoke(
            &router,
            GET,
            &[
                param("TransitGatewayMeteringPolicyId", policy_id),
                param("NextToken", token),
            ],
        )
        .await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected HTTP {REFUSAL_STATUS}, got {status} with body {body}"
        );
    }
}

/// Every filter the read answers narrows the entries to the ones that carry that value, and a
/// name it answers no filter for is refused rather than read as "no results".
#[tokio::test]
async fn a_filter_narrows_the_entries_the_read_reports() {
    let (router, _engine, metering_policy_id, attachment_ids) = seeded().await;
    let cases = cases();
    for case in &cases {
        let (status, body) = invoke(
            &router,
            ACTION,
            &create_params(&metering_policy_id, &attachment_ids, case),
        )
        .await;
        assert_eq!(
            status, 200,
            "the create for {name:?} must succeed, got {status} with body {body}",
            name = case.name
        );
    }

    // Each row: the filter, and the rule numbers of the entries that must be reported for it.
    let rows: Vec<(&str, Vec<&str>, Vec<i32>)> = vec![
        (
            "metered-account",
            vec![payer_type("source-attachment-owner")],
            vec![MIN_TRANSIT_GATEWAY_METERING_POLICY_RULE_NUMBER, 7_000],
        ),
        (
            "metered-account",
            vec![
                payer_type("destination-attachment-owner"),
                payer_type("transit-gateway-owner"),
            ],
            vec![5, 100, 200, i32::MAX],
        ),
        ("protocol", vec!["17"], vec![100]),
        ("protocol", vec!["-1", "255"], vec![7_000, i32::MAX]),
        ("source-cidr-block", vec!["10.0.1.0/24"], vec![100]),
        // The spelling the request named is NOT what the entry carries: the block is stored
        // canonical, so the off-canonical spelling selects nothing.
        ("source-cidr-block", vec!["10.0.1.5/24"], vec![]),
        ("source-port-range", vec!["443"], vec![5]),
        (
            "destination-transit-gateway-attachment-type",
            vec![attachment_type("connect")],
            vec![200],
        ),
        (
            "state",
            vec![entry_state("available")],
            vec![
                MIN_TRANSIT_GATEWAY_METERING_POLICY_RULE_NUMBER,
                5,
                100,
                200,
                7_000,
                i32::MAX,
            ],
        ),
        ("policy-rule-number", vec!["100", "200"], vec![100, 200]),
        ("state", vec![entry_state("deleted")], vec![]),
    ];

    for (name, values, expected_numbers) in rows {
        let case = format!("{name} = {values:?}");
        let mut params = vec![param("TransitGatewayMeteringPolicyId", &metering_policy_id)];
        params.extend(filters(&[(name, &values)]));
        let (status, body) = invoke(&router, GET, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {GET}, got {status} with body {body}"
        );
        let reported: Vec<String> = reported_entries(&case, &body)
            .into_iter()
            .filter_map(|entry| entry.rule_number)
            .collect();
        let mut expected: Vec<String> = expected_numbers.iter().map(i32::to_string).collect();
        expected.sort_by_key(|number| number.parse::<i32>().unwrap_or_default());
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the entries carrying that value, got {reported:?} in {body}"
        );
    }

    // A filter this operation answers no such name for is refused, whether or not an entry
    // would have matched it.
    for name in ["tag-key", "rule-number", "Protocol", ""] {
        let case = format!("the filter {name:?}");
        let mut params = vec![param("TransitGatewayMeteringPolicyId", &metering_policy_id)];
        params.extend(filters(&[(name, &["6"])]));
        let (status, body) = invoke(&router, GET, &params).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected HTTP {REFUSAL_STATUS}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, INVALID_PARAMETER_VALUE,
            "case {case:?}: expected the invalid-filter refusal, got {code:?} in {body}"
        );
    }
}
