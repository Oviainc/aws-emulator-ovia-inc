// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `GetVpcAttachment`, driven over HTTP through the real router (the
//! harness lives in `crate::global_network_lifecycle`, the core-network helpers in
//! `crate::attachment_lifecycle` and `crate::attachment_acceptance`).
//!
//! The rule under test: `GetVpcAttachment` REPORTS the one stored attachment record its
//! `AttachmentId` names — the shared `Attachment` envelope as of the clock, the subnet ARNs
//! the attachment reaches its VPC through, in the order they were stored, and its VPC
//! options — and writes nothing: the state it answers is the one the create (and any
//! acceptance) stored, the transition the clock owes shows up without any call having made
//! it, and two reads in a row answer the same thing. An id that names no stored VPC
//! attachment — never minted, or an attachment of another kind in the one id space the
//! attachment readers share — is the modelled `ResourceNotFoundException` (HTTP 404) naming
//! the resource type and the id asked for.
//!
//! The tables below span that class: VPC attachments differing in how many subnets they hold
//! (one, several, and the bound on how many one attachment may hold), in which VPC options
//! they carry (none at all, every flag off, every flag on, a mixture), in Region and so in
//! edge location, in tags, in routing policy label, and in what the core network's
//! `attachment-policies` decide about them (nothing, a claim without acceptance, a claim
//! requiring one); several VPC attachments of ONE core network, at different edges of it;
//! and refusals differing in how the id fails to name a VPC attachment. Nothing reaches into
//! `crate::state`: every attachment is created, accepted and read back through an operation.

use serde_json::{json, Value};

use crate::attachment_acceptance::{accept, core_network_with, read_vpc};
use crate::attachment_lifecycle::{
    attachment_id, core_network_at, create_connect_attachment, create_vpc_attachment, subnet_arn,
    vpc_arn, ATTACHMENT,
};
// The state an attachment claimed by a rule asking for acceptance waits in, owned by
// `crate::attachment_routing_policy_label`.
use crate::attachment_routing_policy_label::PENDING_ACCEPTANCE;
use crate::global_network_lifecycle::{
    error_code, expected_arn, is_minted_id, Emulator, PAST_SETTLE,
};
use crate::resource_tagging::list_tags;
// The `VpcOptions` and `SubnetArns` accessors, the `Options` request body and the bound on
// how many subnets one attachment holds, all owned by `crate::vpc_attachment_lifecycle`, so
// the two suites cannot drift on what a VPC attachment reports.
use crate::vpc_attachment_lifecycle::{options_body, options_of, subnets_of, MAX_SUBNETS};

/// The `AttachmentType` a VPC attachment reports, one of the values the enum carries
/// (`smithy_query shape=AttachmentType`).
const VPC: &str = "VPC";

/// The two `AttachmentState` values the clock decides for an attachment, beside the
/// [`PENDING_ACCEPTANCE`] an acceptance decides (`smithy_query shape=AttachmentState`).
const CREATING: &str = "CREATING";
/// See [`CREATING`].
const AVAILABLE: &str = "AVAILABLE";

/// One row of the read table: the VPC attachment to create, and what the policy of its core
/// network therefore decides about it.
struct ReadCase {
    /// Names the case in every assertion message.
    name: &'static str,
    /// The Region of the core network's single edge location, and so of the VPC, its subnets
    /// and the attachment.
    region: &'static str,
    /// How many subnets the attachment reaches its VPC through.
    subnets: usize,
    /// The four `VpcOptions` flags the create carries, or `None` for a create naming no
    /// `Options` member at all.
    options: Option<[bool; 4]>,
    /// The `Tags` the create carries, or `Value::Null` for no `Tags` member.
    tags: Value,
    /// The `RoutingPolicyLabel` the create carries, or `None` for none. `Attachment` carries
    /// no label member (`smithy_query shape=Attachment`), so a row that names one must read
    /// back exactly like one that does not.
    routing_policy_label: Option<&'static str>,
    /// The `attachment-policies` the core network's policy document carries, or `Value::Null`
    /// for a document carrying none.
    rules: Value,
    /// The state the created VPC attachment is in: [`PENDING_ACCEPTANCE`] exactly when the
    /// rule that claims it asks for acceptance.
    state: &'static str,
    /// The `AttachmentPolicyRuleNumber` the attachment reports — the claiming rule's number,
    /// or `Value::Null` where no rule claims it.
    rule_number: Value,
    /// The `SegmentName` the attachment reports, or `Value::Null` where the claiming rule
    /// names none.
    segment: Value,
}

/// The table: rows differing along everything the rule generalises over — the subnet count
/// (one, several, and exactly [`MAX_SUBNETS`]), the VPC options (absent, all off, all on,
/// mixed), the Region (and so the edge location), the tags, the routing policy label, and
/// what the `attachment-policies` of the core network decide (nothing at all, a claim
/// without acceptance, a claim requiring it).
///
/// Every policy key named here (`rule-number`, `conditions`, `type`, `operator`, `value`,
/// `action`, `association-method`, `segment`, `require-attachment-acceptance`) is the
/// published Cloud WAN core network policy schema's own, the same source
/// `crate::attachment_acceptance` builds its documents from.
fn read_cases() -> Vec<ReadCase> {
    vec![
        ReadCase {
            name: "one subnet, no Options member, no tags, no attachment-policies",
            region: "us-west-2",
            subnets: 1,
            options: None,
            tags: Value::Null,
            routing_policy_label: None,
            rules: Value::Null,
            state: CREATING,
            rule_number: Value::Null,
            segment: Value::Null,
        },
        ReadCase {
            name: "two subnets, every option on, two tags, a rule asking for no acceptance",
            region: "eu-west-1",
            subnets: 2,
            options: Some([true, true, true, true]),
            tags: json!([{ "Key": "team", "Value": "net" }, { "Key": "tier", "Value": "edge" }]),
            routing_policy_label: Some("north"),
            rules: json!([{
                "rule-number": 50,
                "conditions": [{ "type": "attachment-type", "operator": "equals", "value": "VPC" }],
                "action": {
                    "association-method": "constant",
                    "segment": "shared",
                    "require-attachment-acceptance": false,
                },
            }]),
            state: CREATING,
            rule_number: json!(50),
            segment: json!("shared"),
        },
        ReadCase {
            name: "three subnets, mixed options, a rule requiring acceptance",
            region: "ap-south-1",
            subnets: 3,
            options: Some([true, false, true, false]),
            tags: json!([{ "Key": "env", "Value": "prod" }]),
            routing_policy_label: None,
            rules: json!([{
                "rule-number": 200,
                "conditions": [{ "type": "region", "operator": "equals", "value": "ap-south-1" }],
                "action": {
                    "association-method": "constant",
                    "segment": "prod",
                    "require-attachment-acceptance": true,
                },
            }]),
            state: PENDING_ACCEPTANCE,
            rule_number: json!(200),
            segment: json!("prod"),
        },
        ReadCase {
            name: "every option spelled out and off, a claim naming no segment",
            region: "il-central-1",
            subnets: 2,
            options: Some([false, false, false, false]),
            tags: json!([{ "Key": "only", "Value": "" }]),
            routing_policy_label: Some("south"),
            rules: json!([{
                "rule-number": 1,
                "conditions": [{
                    "type": "resource-id",
                    "operator": "begins-with",
                    "value": "arn:aws:ec2:",
                }],
                "action": { "require-attachment-acceptance": false },
            }]),
            state: CREATING,
            rule_number: json!(1),
            segment: Value::Null,
        },
        ReadCase {
            name: "the bound on how many subnets one attachment holds",
            region: "us-east-1",
            subnets: MAX_SUBNETS,
            options: Some([false, true, false, true]),
            tags: Value::Null,
            routing_policy_label: None,
            rules: Value::Null,
            state: CREATING,
            rule_number: Value::Null,
            segment: Value::Null,
        },
        ReadCase {
            name: "a rule nothing matches, so no rule claims the attachment",
            region: "ca-central-1",
            subnets: 1,
            options: None,
            tags: json!([{ "Key": "tier", "Value": "edge" }]),
            routing_policy_label: Some("west"),
            rules: json!([{
                "rule-number": 7,
                "conditions": [{
                    "type": "attachment-type",
                    "operator": "equals",
                    "value": "SITE_TO_SITE_VPN",
                }],
                "action": {
                    "association-method": "constant",
                    "segment": "shared",
                    "require-attachment-acceptance": true,
                },
            }]),
            state: CREATING,
            rule_number: Value::Null,
            segment: Value::Null,
        },
    ]
}

/// The tags an attachment of this row must report: the ones the create carried, or the empty
/// list where it carried no `Tags` member.
fn expected_tags(case: &ReadCase) -> Value {
    if case.tags.is_null() {
        json!([])
    } else {
        case.tags.clone()
    }
}

/// The subnet ARNs this row's attachment is created with: `case.subnets` of them, from a
/// block no other row names, so each attachment holds its own.
fn subnets_for(case: &ReadCase, row: usize) -> Vec<String> {
    (0..case.subnets)
        .map(|subnet| subnet_arn(case.region, (row + 1) * 100 + subnet))
        .collect()
}

/// `body` with the row's optional `Options` / `Tags` / `RoutingPolicyLabel` members added.
fn with_optional(mut body: Value, case: &ReadCase) -> Value {
    let object = body
        .as_object_mut()
        .unwrap_or_else(|| panic!("case {:?}: the request body must be an object", case.name));
    if let Some(flags) = case.options {
        object.insert("Options".to_string(), options_body(flags));
    }
    if !case.tags.is_null() {
        object.insert("Tags".to_string(), case.tags.clone());
    }
    if let Some(label) = case.routing_policy_label {
        object.insert("RoutingPolicyLabel".to_string(), json!(label));
    }
    body
}

/// `shape` without the two members the CLOCK decides — `Attachment.State` and
/// `Attachment.UpdatedAt` — so two answers taken at different instants can be compared on
/// everything the clock does not move. The state itself is asserted separately, against the
/// states the clock may have reached by then.
fn stable_members(shape: &Value) -> Value {
    let mut stripped = shape.clone();
    if let Some(attachment) = stripped["Attachment"].as_object_mut() {
        attachment.remove("State");
        attachment.remove("UpdatedAt");
    }
    stripped
}

/// Assert the `VpcAttachment` in `shape` is the one `case` describes, holding `subnets`, in
/// one of `states`.
fn assert_vpc_attachment(
    case: &ReadCase,
    shape: &Value,
    core_network: &str,
    subnets: &[String],
    states: &[&str],
) {
    let name = case.name;
    let attachment = &shape["Attachment"];
    let id = attachment_id(name, shape);
    assert!(
        is_minted_id(&id, ATTACHMENT),
        "case {name:?}: expected an attachment-<17 hex> id, got {id:?}"
    );
    assert_eq!(
        subnets_of(shape),
        subnets,
        "case {name:?}: expected the stored subnets, in the order the create gave them, got {shape}"
    );
    assert_eq!(
        options_of(shape),
        case.options.unwrap_or([false, false, false, false]),
        "case {name:?}: expected the stored VPC options, got {shape}"
    );
    // A row that carried an `Options` must report the member itself, not only flags that
    // read as the modelled default because the member is gone.
    assert_eq!(
        shape["Options"].is_object(),
        case.options.is_some(),
        "case {name:?}: the Options member is reported exactly when the create carried one, got {shape}"
    );
    assert_eq!(
        attachment["CoreNetworkId"],
        json!(core_network),
        "case {name:?}: expected the core network the attachment hangs off, got {attachment}"
    );
    assert_eq!(
        attachment["CoreNetworkArn"],
        json!(expected_arn("core-network", core_network)),
        "case {name:?}: expected the core network's ARN, got {attachment}"
    );
    assert_eq!(
        attachment["AttachmentType"],
        json!(VPC),
        "case {name:?}: expected a {VPC} attachment, got {attachment}"
    );
    assert_eq!(
        attachment["EdgeLocation"],
        json!(case.region),
        "case {name:?}: expected the Region the VPC is in as the edge location, got {attachment}"
    );
    // "This is returned only for Direct Connect gateway attachments. All other attachment
    // types retrun EdgeLocation" (`smithy_query query="doc" shape="Attachment"
    // member="EdgeLocations"`), so a VPC attachment reports no plural member at all.
    assert_eq!(
        attachment["EdgeLocations"],
        Value::Null,
        "case {name:?}: a VPC attachment reports EdgeLocation and not EdgeLocations, got {attachment}"
    );
    // The resource a VPC attachment attaches IS the VPC its create named.
    assert_eq!(
        attachment["ResourceArn"],
        json!(vpc_arn(case.region, 0)),
        "case {name:?}: expected the attached VPC's ARN, got {attachment}"
    );
    assert_eq!(
        attachment["OwnerAccountId"],
        json!(engine::emulator_account_id()),
        "case {name:?}: expected the emulator's account id, got {attachment}"
    );
    assert_eq!(
        attachment["Tags"],
        expected_tags(case),
        "case {name:?}: expected the tags the create carried, got {attachment}"
    );
    assert_eq!(
        attachment["LastModificationErrors"],
        json!([]),
        "case {name:?}: no modification of the attachment has failed, got {attachment}"
    );
    assert_eq!(
        attachment["AttachmentPolicyRuleNumber"], case.rule_number,
        "case {name:?}: expected the claiming rule's number {}, got {attachment}",
        case.rule_number
    );
    assert_eq!(
        attachment["SegmentName"], case.segment,
        "case {name:?}: expected segment {}, got {attachment}",
        case.segment
    );
    let reported_state = attachment["State"].as_str().unwrap_or_default();
    assert!(
        states.contains(&reported_state),
        "case {name:?}: expected one of the states {states:?}, got {attachment}"
    );
    assert!(
        attachment["CreatedAt"].as_f64().is_some_and(|at| at > 0.0),
        "case {name:?}: CreatedAt must be a timestamp, got {attachment}"
    );
}

#[tokio::test]
async fn get_vpc_attachment_reports_the_stored_record_of_every_kind_of_vpc_attachment() {
    let emulator = Emulator::new();
    let cases = read_cases();
    // What each row created, kept so the transition the clock owes can be asserted after ONE
    // wait: the row, its attachment id, the subnets it holds and the shape the create
    // answered.
    let mut created: Vec<(&ReadCase, String, Vec<String>, Value)> = Vec::new();

    for (row, case) in cases.iter().enumerate() {
        let name = case.name;
        let core_network = core_network_with(&emulator, name, case.region, &case.rules).await;
        let subnets = subnets_for(case, row);

        // Each row attaches a VPC of its OWN Region, so one VPC is never attached twice —
        // the index may repeat because the Region does not.
        let request = json!({
            "CoreNetworkId": core_network,
            "VpcArn": vpc_arn(case.region, 0),
            "SubnetArns": subnets,
        });
        let (status, created_vpc) =
            create_vpc_attachment(&emulator, with_optional(request, case)).await;
        assert_eq!(
            status, 200,
            "case {name:?}: CreateVpcAttachment expected 200, got {status} with {created_vpc}"
        );
        let at_create = created_vpc["VpcAttachment"].clone();
        // The create's own answer is taken at the instant the record was stored, so its
        // state is exactly the one the policy decided — no span has elapsed yet.
        assert_vpc_attachment(case, &at_create, &core_network, &subnets, &[case.state]);
        let id = attachment_id(name, &at_create);

        // The read answers the record the create stored, member for member. Everything but
        // the two members the clock decides must be identical to what the create answered;
        // the state must be the stored one or the one the clock has since made due — never
        // one the READ moved it to, and an attachment waiting for an acceptance is moved by
        // no span at all.
        let (status, read) = read_vpc(&emulator, &id).await;
        assert_eq!(
            status, 200,
            "case {name:?}: GetVpcAttachment expected 200, got {status} with {read}"
        );
        assert_eq!(
            stable_members(&read["VpcAttachment"]),
            stable_members(&at_create),
            "case {name:?}: the reader must answer the stored attachment, got {read}"
        );
        let reachable: &[&str] = if case.state == PENDING_ACCEPTANCE {
            &[PENDING_ACCEPTANCE]
        } else {
            &[CREATING, AVAILABLE]
        };
        assert_vpc_attachment(case, &read["VpcAttachment"], &core_network, &subnets, reachable);
        // Where the span has not elapsed yet, the read moved NOTHING: it reports the very
        // instant the create stored as `UpdatedAt`, not the clock of the read.
        if read["VpcAttachment"]["Attachment"]["State"] == at_create["Attachment"]["State"] {
            assert_eq!(
                read["VpcAttachment"]["Attachment"]["UpdatedAt"],
                at_create["Attachment"]["UpdatedAt"],
                "case {name:?}: a read that found the attachment in the state the create left it must report the create's UpdatedAt, got {read}"
            );
        }

        // A read never changes what the next caller sees: the next one answers the same
        // record, and a state no read moved — only the span the create started can have
        // advanced it.
        let (_, again) = read_vpc(&emulator, &id).await;
        assert_eq!(
            stable_members(&again["VpcAttachment"]),
            stable_members(&read["VpcAttachment"]),
            "case {name:?}: GetVpcAttachment must be a pure read, got {read} then {again}"
        );
        assert_vpc_attachment(case, &again["VpcAttachment"], &core_network, &subnets, reachable);

        // The attachment's own ARN is not a member of the shape, so it is read back the one
        // way a client can: through the tagging slice, which resolves that ARN to this
        // stored attachment and the tags it was created with.
        let (status, listed) = list_tags(&emulator, &expected_arn(ATTACHMENT, &id)).await;
        assert_eq!(
            status, 200,
            "case {name:?}: ListTagsForResource expected 200, got {status} with {listed}"
        );
        assert_eq!(
            listed,
            expected_tags(case),
            "case {name:?}: the attachment ARN must resolve to the stored attachment's tags, got {listed}"
        );

        // An attachment waiting for the acceptance its policy asked for is moved by nothing
        // but that acceptance: the clock owes it no transition, and `AcceptAttachment` is
        // what starts the one it then owes.
        if case.state == PENDING_ACCEPTANCE {
            let (status, accepted) = accept(&emulator, &id).await;
            assert_eq!(
                status, 200,
                "case {name:?}: AcceptAttachment expected 200, got {status} with {accepted}"
            );
            let (status, read) = read_vpc(&emulator, &id).await;
            assert_eq!(
                status, 200,
                "case {name:?}: GetVpcAttachment after the acceptance expected 200, got {status} with {read}"
            );
            // The acceptance started the span the clock owes, so the attachment is being
            // created and is AVAILABLE once that span has elapsed — it is no longer waiting
            // for anything.
            assert_vpc_attachment(
                case,
                &read["VpcAttachment"],
                &core_network,
                &subnets,
                &[CREATING, AVAILABLE],
            );
        }

        created.push((case, id, subnets, at_create));
    }

    // Past the settle span every row's `CREATING` -> `AVAILABLE` transition has fallen due,
    // and the read reports it although no call made it.
    tokio::time::sleep(PAST_SETTLE).await;
    for (case, id, subnets, at_create) in &created {
        let name = case.name;
        let (status, read) = read_vpc(&emulator, id).await;
        assert_eq!(
            status, 200,
            "case {name:?}: GetVpcAttachment past the settle span expected 200, got {status} with {read}"
        );
        let shape = &read["VpcAttachment"];
        let attachment = &shape["Attachment"];
        assert_eq!(
            attachment["State"],
            json!(AVAILABLE),
            "case {name:?}: past the settle span the attachment is {AVAILABLE}, got {attachment}"
        );
        let created_at = attachment["CreatedAt"].as_f64().unwrap_or_default();
        let updated_at = attachment["UpdatedAt"].as_f64().unwrap_or_default();
        assert!(
            updated_at > created_at,
            "case {name:?}: the settle transition is a change, so UpdatedAt ({updated_at}) must be past CreatedAt ({created_at}), got {attachment}"
        );
        assert_eq!(
            subnets_of(shape),
            *subnets,
            "case {name:?}: the subnets must not have moved with the state, got {shape}"
        );
        assert_eq!(
            shape["Options"], at_create["Options"],
            "case {name:?}: the VPC options must not have moved with the state, got {shape}"
        );
        for member in [
            "AttachmentId",
            "CoreNetworkId",
            "CoreNetworkArn",
            "AttachmentType",
            "OwnerAccountId",
            "EdgeLocation",
            "ResourceArn",
            "CreatedAt",
            "Tags",
            "AttachmentPolicyRuleNumber",
            "SegmentName",
        ] {
            assert_eq!(
                attachment[member], at_create["Attachment"][member],
                "case {name:?}: {member} must not have moved with the state, got {attachment}"
            );
        }

        // The settled state is a function of the stored record and the clock, not of the
        // read: once the transition has fallen due, `UpdatedAt` stays where it fell rather
        // than following the clock of each read.
        let (_, again) = read_vpc(&emulator, id).await;
        assert_eq!(
            again, read,
            "case {name:?}: a settled attachment reads the same twice, got {read} then {again}"
        );
    }
}

#[tokio::test]
async fn every_vpc_attachment_of_one_core_network_reads_back_its_own_record() {
    let emulator = Emulator::new();
    // One core network with an edge in each of these Regions, so the edge location every
    // attachment reports is the Region of ITS VPC rather than the first edge of the core
    // network they share.
    let regions = ["us-west-2", "eu-west-1", "ap-south-1"];
    let (_global_network, core_network) =
        core_network_at(&emulator, "one core network, many VPCs", &regions).await;

    // One row per attachment: its Region, how many subnets it holds and its options, so no
    // two attachments of this core network hold the same thing.
    let mut attachments: Vec<(&str, String, Vec<String>, [bool; 4])> = Vec::new();
    for (index, region) in regions.iter().enumerate() {
        let subnets: Vec<String> = (0..=index)
            .map(|subnet| subnet_arn(region, (index + 1) * 10 + subnet))
            .collect();
        let flags = [index == 0, index == 1, index == 2, index % 2 == 0];
        let (status, created) = create_vpc_attachment(
            &emulator,
            json!({
                "CoreNetworkId": core_network,
                "VpcArn": vpc_arn(region, index),
                "SubnetArns": subnets,
                "Options": options_body(flags),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: CreateVpcAttachment expected 200, got {status} with {created}"
        );
        let id = attachment_id(region, &created["VpcAttachment"]);
        assert!(
            attachments.iter().all(|(_, minted, _, _)| *minted != id),
            "case {region:?}: each attachment gets its own id, got {id:?} twice"
        );
        attachments.push((region, id, subnets, flags));
    }

    for (region, id, subnets, flags) in &attachments {
        let (status, read) = read_vpc(&emulator, id).await;
        assert_eq!(
            status, 200,
            "case {region:?}: GetVpcAttachment expected 200, got {status} with {read}"
        );
        let shape = &read["VpcAttachment"];
        assert_eq!(
            shape["Attachment"]["AttachmentId"],
            json!(id),
            "case {region:?}: the reader must answer the attachment asked for, got {shape}"
        );
        assert_eq!(
            shape["Attachment"]["EdgeLocation"],
            json!(region),
            "case {region:?}: each attachment sits at the edge of its own VPC's Region, got {shape}"
        );
        assert_eq!(
            subnets_of(shape),
            *subnets,
            "case {region:?}: each attachment reports its OWN subnets, got {shape}"
        );
        assert_eq!(
            options_of(shape),
            *flags,
            "case {region:?}: each attachment reports its OWN options, got {shape}"
        );
    }
}

#[tokio::test]
async fn an_id_that_names_no_stored_vpc_attachment_is_the_modelled_not_found() {
    let emulator = Emulator::new();
    let (global_network, core_network) =
        core_network_at(&emulator, "not found", &["us-east-1"]).await;

    let (status, created_vpc) = create_vpc_attachment(
        &emulator,
        json!({
            "CoreNetworkId": core_network,
            "VpcArn": vpc_arn("us-east-1", 1),
            "SubnetArns": [subnet_arn("us-east-1", 11)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateVpcAttachment expected 200, got {status} with {created_vpc}"
    );
    let vpc = attachment_id("the VPC attachment", &created_vpc["VpcAttachment"]);

    let (status, created_vpn) = emulator
        .post(
            "/site-to-site-vpn-attachments",
            json!({
                "CoreNetworkId": core_network,
                "VpnConnectionArn": format!(
                    "arn:aws:ec2:us-east-1:{}:vpn-connection/vpn-0000000a",
                    engine::emulator_account_id()
                ),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateSiteToSiteVpnAttachment expected 200, got {status} with {created_vpn}"
    );
    let vpn = attachment_id("the VPN attachment", &created_vpn["SiteToSiteVpnAttachment"]);

    let (status, created_connect) = create_connect_attachment(
        &emulator,
        json!({
            "CoreNetworkId": core_network,
            "EdgeLocation": "us-east-1",
            "TransportAttachmentId": vpc,
            "Options": { "Protocol": "GRE" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateConnectAttachment expected 200, got {status} with {created_connect}"
    );
    let connect = attachment_id("the Connect attachment", &created_connect["ConnectAttachment"]);

    // Each row is a different way of naming no stored VPC attachment, every one satisfying
    // the modelled `AttachmentId` pattern so it reaches the handler: a never-minted id at
    // each width the pattern allows, the counter value a DIFFERENT resource kind of this
    // service was minted under spelled with the attachment prefix, and the ids of the two
    // attachments of OTHER kinds the setup stored in the one id space the attachment readers
    // share.
    let borrowed_suffix = format!(
        "attachment-{}",
        core_network.strip_prefix("core-network-").unwrap_or_else(|| panic!(
            "a minted core network id carries the core-network- prefix, got {core_network:?}"
        ))
    );
    for (case, asked_for) in [
        ("never minted, 17 hex digits", "attachment-00000000000000099"),
        ("never minted, the 8-hex lower bound", "attachment-0000beef"),
        ("another counter value under this prefix", &borrowed_suffix),
        ("a site-to-site VPN attachment's id", &vpn),
        ("a Connect attachment's id", &connect),
    ] {
        let (status, body) = read_vpc(&emulator, asked_for).await;
        assert_eq!(
            status, 404,
            "case {case} ({asked_for:?}): expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case} ({asked_for:?}): wrong error code in {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!(ATTACHMENT),
            "case {case} ({asked_for:?}): the refusal must name the resource type, got {body}"
        );
        assert_eq!(
            body["ResourceId"],
            json!(asked_for),
            "case {case} ({asked_for:?}): the refusal must name the id asked for, got {body}"
        );
    }

    // The refusals are the lookup and nothing wider: the stored VPC attachment still reads,
    // and the reads and refusals stored nothing — the global network still holds exactly the
    // three attachments the setup created in it.
    let (status, read) = read_vpc(&emulator, &vpc).await;
    assert_eq!(
        status, 200,
        "GetVpcAttachment on the live attachment expected 200, got {status} with {read}"
    );
    let (status, counts) = emulator
        .get(&format!(
            "/global-networks/{global_network}/network-resource-count?resourceType=attachment"
        ))
        .await;
    assert_eq!(
        status, 200,
        "GetNetworkResourceCounts expected 200, got {status} with {counts}"
    );
    // Pick the entry by its `ResourceType` rather than by position, so the assertion names
    // the count it means even if the list ever carries more than one entry.
    let attachment_count = counts["NetworkResourceCounts"]
        .as_array()
        .unwrap_or_else(|| panic!("a count list is an array, got {counts}"))
        .iter()
        .find(|entry| entry["ResourceType"] == json!(ATTACHMENT))
        .unwrap_or_else(|| panic!("the counts must carry an {ATTACHMENT} entry, got {counts}"))
        .clone();
    assert_eq!(
        attachment_count["Count"],
        json!(3),
        "the reads stored nothing, so the global network still holds its three attachments, got {counts}"
    );
}
