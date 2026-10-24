// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `AcceptAttachment`, driven over HTTP through the real router (the
//! harness lives in `crate::global_network_lifecycle`).
//!
//! The rule under test: the FIRST `attachment-policies` rule of the core network's policy
//! whose conditions claim a new attachment — the rules evaluated in ascending `rule-number`
//! — decides what becomes of it, so an attachment claimed by a rule whose action carries
//! `require-attachment-acceptance: true` is created in `PENDING_ATTACHMENT_ACCEPTANCE` and
//! no span of the clock moves it out; `AcceptAttachment` is what starts it, after which it
//! is `CREATING` until the transition the clock owes falls due and `AVAILABLE` after it; and
//! an attachment that is waiting for no acceptance — one no claiming rule asked for, which
//! includes every attachment under a policy carrying no `attachment-policies` — has no
//! attachment request to accept, which is refused with HTTP 409 `ConflictException`.
//!
//! The table below spans that class: rows differing in the condition type that claims the
//! attachment (`any`, `attachment-type`, `region`, `resource-id`, `tag-exists`,
//! `tag-value`), in the operator it compares with (`equals`, `not-equals`, `contains`,
//! `begins-with`), in the `condition-logic` combining several conditions, in how the action
//! names the segment (`constant`, `tag`, none at all), in whether the claiming rule asks for
//! acceptance, in the Region, and in whether any rule claims the attachment at all. Every
//! condition type, operator and action member named here is the published Cloud WAN core
//! network policy schema's own ("Attachment policies" documentation); the states are
//! `AttachmentState`'s own values (`smithy_query shape=AttachmentState`). Nothing reaches
//! into `crate::state`: every attachment is created, accepted and read back through an
//! operation.

use serde_json::{json, Value};

use crate::attachment_lifecycle::{
    attachment_id, create_connect_attachment, create_vpc_attachment, subnet_arn, vpc_arn,
};
use crate::core_network_lifecycle::create_core_network;
use crate::global_network_lifecycle::{error_code, Emulator, PAST_SETTLE};

/// The state an attachment waiting for its acceptance is in, and the two the clock decides.
/// All three are `AttachmentState` values (`smithy_query shape=AttachmentState`).
const PENDING_ACCEPTANCE: &str = "PENDING_ATTACHMENT_ACCEPTANCE";
/// See [`PENDING_ACCEPTANCE`].
const CREATING: &str = "CREATING";
/// See [`PENDING_ACCEPTANCE`].
const AVAILABLE: &str = "AVAILABLE";

/// A core network policy document with an edge location in `region` and, where `rules` is
/// not null, those `attachment-policies`.
///
/// The key names (`version`, `core-network-configuration`, `asn-ranges`,
/// `edge-locations`, `location`, `segments`, `attachment-policies`) are the published Cloud
/// WAN core network policy schema's, the same source the other suites build documents from.
fn policy_document(region: &str, rules: &Value) -> String {
    let mut document = json!({
        "version": "2021.12",
        "core-network-configuration": {
            "asn-ranges": ["64512-64555"],
            "edge-locations": [{ "location": region }],
        },
        "segments": [{ "name": "shared" }, { "name": "prod" }],
    });
    if !rules.is_null() {
        document["attachment-policies"] = rules.clone();
    }
    document.to_string()
}

/// Create a global network and, in it, a core network whose policy is
/// [`policy_document`]. Answers the minted core network id.
async fn core_network_with(
    emulator: &Emulator,
    case: &str,
    region: &str,
    rules: &Value,
) -> String {
    let network = emulator.create_global_network(case).await;
    let (status, created) = create_core_network(
        emulator,
        json!({
            "GlobalNetworkId": network,
            "Description": case,
            "PolicyDocument": policy_document(region, rules),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateCoreNetwork expected 200, got {status} with {created}"
    );
    created["CoreNetwork"]["CoreNetworkId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no CoreNetworkId in {created}"))
        .to_string()
}

/// `AcceptAttachment`.
async fn accept(emulator: &Emulator, attachment_id: &str) -> (u16, Value) {
    emulator
        .call("POST", &format!("/attachments/{attachment_id}/accept"), None)
        .await
}

/// `GetVpcAttachment`.
async fn read_vpc(emulator: &Emulator, attachment_id: &str) -> (u16, Value) {
    emulator
        .get(&format!("/vpc-attachments/{attachment_id}"))
        .await
}

/// One row of the acceptance table: the policy its core network carries, the tags the
/// attachment is created with, and what the policy therefore decides about it.
struct AcceptanceCase {
    /// Names the case in every assertion message.
    name: &'static str,
    /// The Region of the core network's single edge location, and so of the attachment.
    region: &'static str,
    /// The `attachment-policies` the policy document carries, or `Value::Null` for a
    /// document that carries none at all.
    rules: Value,
    /// The `Tags` the attachment create carries, or `Value::Null` for none.
    tags: Value,
    /// The state the created attachment is in: [`PENDING_ACCEPTANCE`] exactly when the rule
    /// that claims it asks for acceptance.
    state: &'static str,
    /// The `AttachmentPolicyRuleNumber` the attachment reports — the claiming rule's number,
    /// or `Value::Null` where no rule claims it.
    rule_number: Value,
    /// The `SegmentName` the attachment reports, or `Value::Null` where the claiming rule
    /// names no segment (or names one by a tag the attachment does not carry).
    segment: Value,
}

/// The table. Each row names the source of what it asserts in its own name: the condition
/// type and operator that claim the attachment, the logic combining them, and the
/// association method the action uses.
fn acceptance_cases() -> Vec<AcceptanceCase> {
    vec![
        AcceptanceCase {
            name: "`any` claims every attachment, segment by constant",
            region: "us-west-2",
            rules: json!([{
                "rule-number": 100,
                "conditions": [{ "type": "any" }],
                "action": {
                    "association-method": "constant",
                    "segment": "shared",
                    "require-attachment-acceptance": true,
                },
            }]),
            tags: Value::Null,
            state: PENDING_ACCEPTANCE,
            rule_number: json!(100),
            segment: json!("shared"),
        },
        AcceptanceCase {
            name: "`attachment-type` equals VPC",
            region: "eu-west-1",
            rules: json!([{
                "rule-number": 200,
                "conditions": [{ "type": "attachment-type", "operator": "equals", "value": "VPC" }],
                "action": {
                    "association-method": "constant",
                    "segment": "prod",
                    "require-attachment-acceptance": true,
                },
            }]),
            tags: Value::Null,
            state: PENDING_ACCEPTANCE,
            rule_number: json!(200),
            segment: json!("prod"),
        },
        AcceptanceCase {
            name: "`attachment-type` not-equals CONNECT",
            region: "eu-central-1",
            rules: json!([{
                "rule-number": 7,
                "conditions": [{
                    "type": "attachment-type",
                    "operator": "not-equals",
                    "value": "CONNECT",
                }],
                "action": { "require-attachment-acceptance": true },
            }]),
            tags: Value::Null,
            state: PENDING_ACCEPTANCE,
            // The action names no segment at all, so the attachment joins none.
            rule_number: json!(7),
            segment: Value::Null,
        },
        AcceptanceCase {
            name: "`region` contains part of the edge location",
            region: "ap-south-1",
            rules: json!([{
                "rule-number": 300,
                "conditions": [{ "type": "region", "operator": "contains", "value": "south" }],
                "action": {
                    "association-method": "constant",
                    "segment": "shared",
                    "require-attachment-acceptance": true,
                },
            }]),
            tags: Value::Null,
            state: PENDING_ACCEPTANCE,
            rule_number: json!(300),
            segment: json!("shared"),
        },
        AcceptanceCase {
            name: "`resource-id` begins-with the EC2 ARN prefix",
            region: "il-central-1",
            rules: json!([{
                "rule-number": 1,
                "conditions": [{
                    "type": "resource-id",
                    "operator": "begins-with",
                    "value": "arn:aws:ec2:",
                }],
                "action": {
                    "association-method": "constant",
                    "segment": "prod",
                    "require-attachment-acceptance": true,
                },
            }]),
            tags: Value::Null,
            state: PENDING_ACCEPTANCE,
            rule_number: json!(1),
            segment: json!("prod"),
        },
        AcceptanceCase {
            name: "`tag-exists`, segment from the value of that tag",
            region: "us-east-1",
            rules: json!([{
                "rule-number": 10,
                "conditions": [{ "type": "tag-exists", "key": "env" }],
                "action": {
                    "association-method": "tag",
                    "tag-value-of-key": "env",
                    "require-attachment-acceptance": true,
                },
            }]),
            tags: json!([{ "Key": "env", "Value": "prod" }]),
            state: PENDING_ACCEPTANCE,
            rule_number: json!(10),
            segment: json!("prod"),
        },
        AcceptanceCase {
            name: "`condition-logic` or: the second condition claims it",
            region: "ca-central-1",
            rules: json!([{
                "rule-number": 42,
                "condition-logic": "or",
                "conditions": [
                    { "type": "region", "operator": "equals", "value": "us-west-2" },
                    { "type": "tag-value", "operator": "equals", "key": "tier", "value": "edge" },
                ],
                "action": {
                    "association-method": "constant",
                    "segment": "shared",
                    "require-attachment-acceptance": true,
                },
            }]),
            tags: json!([{ "Key": "tier", "Value": "edge" }]),
            state: PENDING_ACCEPTANCE,
            rule_number: json!(42),
            segment: json!("shared"),
        },
        AcceptanceCase {
            name: "`condition-logic` and (the default): one condition fails, nothing claims it",
            region: "us-west-2",
            rules: json!([{
                "rule-number": 42,
                "conditions": [
                    { "type": "region", "operator": "equals", "value": "us-west-2" },
                    { "type": "tag-value", "operator": "equals", "key": "tier", "value": "edge" },
                ],
                "action": {
                    "association-method": "constant",
                    "segment": "shared",
                    "require-attachment-acceptance": true,
                },
            }]),
            tags: Value::Null,
            state: CREATING,
            rule_number: Value::Null,
            segment: Value::Null,
        },
        AcceptanceCase {
            name: "the LOWEST matching rule number decides, and it asks for no acceptance",
            region: "eu-west-2",
            rules: json!([
                {
                    "rule-number": 100,
                    "conditions": [{ "type": "any" }],
                    "action": {
                        "association-method": "constant",
                        "segment": "prod",
                        "require-attachment-acceptance": true,
                    },
                },
                {
                    "rule-number": 50,
                    "conditions": [{ "type": "any" }],
                    "action": {
                        "association-method": "constant",
                        "segment": "shared",
                        "require-attachment-acceptance": false,
                    },
                },
            ]),
            tags: Value::Null,
            state: CREATING,
            rule_number: json!(50),
            segment: json!("shared"),
        },
        AcceptanceCase {
            name: "`tag-exists` for a tag the attachment does not carry",
            region: "ap-northeast-1",
            rules: json!([{
                "rule-number": 5,
                "conditions": [{ "type": "tag-exists", "key": "env" }],
                "action": { "require-attachment-acceptance": true },
            }]),
            tags: json!([{ "Key": "tier", "Value": "edge" }]),
            state: CREATING,
            rule_number: Value::Null,
            segment: Value::Null,
        },
        AcceptanceCase {
            name: "a policy with no attachment-policies at all",
            region: "sa-east-1",
            rules: Value::Null,
            tags: Value::Null,
            state: CREATING,
            rule_number: Value::Null,
            segment: Value::Null,
        },
    ]
}

#[tokio::test]
async fn attachment_acceptance_follows_the_rule_of_the_policy_that_claimed_the_attachment() {
    let emulator = Emulator::new();
    let cases = acceptance_cases();
    // The attachments that were accepted, so the transition the clock then owes can be
    // asserted after ONE wait rather than one per row.
    let mut accepted: Vec<(&'static str, String)> = Vec::new();

    for (index, case) in cases.iter().enumerate() {
        let name = case.name;
        let core_network = core_network_with(&emulator, name, case.region, &case.rules).await;
        let vpc = vpc_arn(case.region, index + 1);
        let mut request = json!({
            "CoreNetworkId": core_network,
            "VpcArn": vpc,
            "SubnetArns": [subnet_arn(case.region, index + 1)],
        });
        if !case.tags.is_null() {
            request["Tags"] = case.tags.clone();
        }
        let (status, created) = create_vpc_attachment(&emulator, request).await;
        assert_eq!(
            status, 200,
            "case {name:?}: CreateVpcAttachment expected 200, got {status} with {created}"
        );
        let attachment = &created["VpcAttachment"]["Attachment"];
        let id = attachment_id(name, &created["VpcAttachment"]);
        assert_eq!(
            attachment["State"],
            json!(case.state),
            "case {name:?}: expected the created attachment to be {}, got {attachment}",
            case.state
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

        // Reading the attachment back answers the same state: a read reports the stored
        // record and changes nothing about it.
        let (status, read) = read_vpc(&emulator, &id).await;
        assert_eq!(
            status, 200,
            "case {name:?}: GetVpcAttachment expected 200, got {status} with {read}"
        );
        assert_eq!(
            read["VpcAttachment"]["Attachment"]["State"],
            json!(case.state),
            "case {name:?}: the stored state must survive a read, got {read}"
        );

        let (status, answered) = accept(&emulator, &id).await;
        if case.state == PENDING_ACCEPTANCE {
            assert_eq!(
                status, 200,
                "case {name:?}: AcceptAttachment expected 200, got {status} with {answered}"
            );
            let accepted_attachment = &answered["Attachment"];
            assert_eq!(
                accepted_attachment["AttachmentId"],
                json!(id),
                "case {name:?}: AcceptAttachment must answer the attachment asked for, got {answered}"
            );
            assert_eq!(
                accepted_attachment["State"],
                json!(CREATING),
                "case {name:?}: an accepted attachment starts being created, got {answered}"
            );
            assert_eq!(
                accepted_attachment["AttachmentPolicyRuleNumber"], case.rule_number,
                "case {name:?}: accepting keeps the claiming rule, got {answered}"
            );
            assert_eq!(
                accepted_attachment["SegmentName"], case.segment,
                "case {name:?}: accepting keeps the segment the rule named, got {answered}"
            );
            let created_at = accepted_attachment["CreatedAt"].as_f64().unwrap_or_default();
            let updated_at = accepted_attachment["UpdatedAt"].as_f64().unwrap_or_default();
            assert!(
                updated_at >= created_at && created_at > 0.0,
                "case {name:?}: the acceptance is the attachment's latest change, got CreatedAt {created_at} and UpdatedAt {updated_at}"
            );

            // The acceptance was STORED: the reader answers the accepted attachment, and a
            // second acceptance has nothing left to accept.
            let (status, read) = read_vpc(&emulator, &id).await;
            assert_eq!(
                status, 200,
                "case {name:?}: GetVpcAttachment expected 200, got {status} with {read}"
            );
            assert_eq!(
                read["VpcAttachment"]["Attachment"]["State"],
                json!(CREATING),
                "case {name:?}: the acceptance must be persisted, got {read}"
            );
            let (status, refused) = accept(&emulator, &id).await;
            assert_eq!(
                status, 409,
                "case {name:?}: a second AcceptAttachment expected 409, got {status} with {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "ConflictException",
                "case {name:?}: expected ConflictException, got {refused}"
            );
            accepted.push((name, id));
        } else {
            assert_eq!(
                status, 409,
                "case {name:?}: AcceptAttachment on an attachment awaiting no acceptance expected 409, got {status} with {answered}"
            );
            assert_eq!(
                error_code(&answered),
                "ConflictException",
                "case {name:?}: expected ConflictException, got {answered}"
            );
            // The refusal stored nothing: the attachment is where it was.
            let (status, read) = read_vpc(&emulator, &id).await;
            assert_eq!(
                status, 200,
                "case {name:?}: GetVpcAttachment expected 200, got {status} with {read}"
            );
            assert_eq!(
                read["VpcAttachment"]["Attachment"]["State"],
                json!(case.state),
                "case {name:?}: a refused acceptance must change nothing, got {read}"
            );
        }
    }

    tokio::time::sleep(PAST_SETTLE).await;

    for (name, id) in &accepted {
        let (status, read) = read_vpc(&emulator, id).await;
        assert_eq!(
            status, 200,
            "case {name:?}: GetVpcAttachment expected 200, got {status} with {read}"
        );
        let attachment = &read["VpcAttachment"]["Attachment"];
        assert_eq!(
            attachment["State"],
            json!(AVAILABLE),
            "case {name:?}: the transition the acceptance owed must have fallen due, got {attachment}"
        );
        let created_at = attachment["CreatedAt"].as_f64().unwrap_or_default();
        let updated_at = attachment["UpdatedAt"].as_f64().unwrap_or_default();
        assert!(
            updated_at > created_at,
            "case {name:?}: becoming available is a later change than the create, got CreatedAt {created_at} and UpdatedAt {updated_at}"
        );
    }
}

#[tokio::test]
async fn the_clock_never_accepts_an_attachment_for_the_caller() {
    let emulator = Emulator::new();
    let case = "the clock owes nothing on an attachment awaiting acceptance";
    let region = "us-west-2";
    let rules = json!([{
        "rule-number": 100,
        "conditions": [{ "type": "any" }],
        "action": {
            "association-method": "constant",
            "segment": "shared",
            "require-attachment-acceptance": true,
        },
    }]);
    let core_network = core_network_with(&emulator, case, region, &rules).await;
    let (status, created) = create_vpc_attachment(
        &emulator,
        json!({
            "CoreNetworkId": core_network,
            "VpcArn": vpc_arn(region, 1),
            "SubnetArns": [subnet_arn(region, 1)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateVpcAttachment expected 200, got {status} with {created}"
    );
    let id = attachment_id(case, &created["VpcAttachment"]);

    // Waiting past the span a CREATING attachment would settle in leaves this one exactly
    // where it is: only an acceptance moves it.
    tokio::time::sleep(PAST_SETTLE).await;
    let (status, read) = read_vpc(&emulator, &id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetVpcAttachment expected 200, got {status} with {read}"
    );
    let attachment = &read["VpcAttachment"]["Attachment"];
    assert_eq!(
        attachment["State"],
        json!(PENDING_ACCEPTANCE),
        "case {case:?}: no span of the clock may accept an attachment, got {attachment}"
    );
    assert_eq!(
        attachment["UpdatedAt"], attachment["CreatedAt"],
        "case {case:?}: nothing has changed the attachment since the create, got {attachment}"
    );

    let (status, answered) = accept(&emulator, &id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: AcceptAttachment expected 200, got {status} with {answered}"
    );
    assert_eq!(
        answered["Attachment"]["State"],
        json!(CREATING),
        "case {case:?}: the acceptance starts the attachment, got {answered}"
    );
    tokio::time::sleep(PAST_SETTLE).await;
    let (status, read) = read_vpc(&emulator, &id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetVpcAttachment expected 200, got {status} with {read}"
    );
    assert_eq!(
        read["VpcAttachment"]["Attachment"]["State"],
        json!(AVAILABLE),
        "case {case:?}: the transition the acceptance owed must fall due, got {read}"
    );
}

#[tokio::test]
async fn accepting_an_attachment_that_does_not_exist_is_not_found() {
    let emulator = Emulator::new();
    // Ids satisfying the modelled `^attachment-([0-9a-f]{8,17})$` that name nothing: the
    // shortest and the longest the pattern allows.
    for id in ["attachment-0123abcd", "attachment-0123456789abcdef0"] {
        let (status, refused) = accept(&emulator, id).await;
        assert_eq!(
            status, 404,
            "case {id:?}: AcceptAttachment expected 404, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {id:?}: expected ResourceNotFoundException, got {refused}"
        );
        assert_eq!(
            refused["ResourceId"],
            json!(id),
            "case {id:?}: the refusal must name the id asked for, got {refused}"
        );
        assert_eq!(
            refused["ResourceType"],
            json!("attachment"),
            "case {id:?}: the refusal must name the resource type, got {refused}"
        );
    }
}

#[tokio::test]
async fn a_connect_attachment_waits_for_the_acceptance_its_policy_rule_asks_for() {
    let emulator = Emulator::new();
    let case = "only CONNECT attachments are claimed";
    let region = "us-west-2";
    // The rule claims Connect attachments only, so the VPC transport under it is created
    // without an acceptance and the Connect attachment carried over it waits for one: the
    // acceptance rule holds for every attachment create, not just one kind's.
    let rules = json!([{
        "rule-number": 20,
        "conditions": [{ "type": "attachment-type", "operator": "equals", "value": "CONNECT" }],
        "action": {
            "association-method": "constant",
            "segment": "prod",
            "require-attachment-acceptance": true,
        },
    }]);
    let core_network = core_network_with(&emulator, case, region, &rules).await;
    let (status, created) = create_vpc_attachment(
        &emulator,
        json!({
            "CoreNetworkId": core_network,
            "VpcArn": vpc_arn(region, 1),
            "SubnetArns": [subnet_arn(region, 1)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateVpcAttachment expected 200, got {status} with {created}"
    );
    let transport = attachment_id(case, &created["VpcAttachment"]);
    assert_eq!(
        created["VpcAttachment"]["Attachment"]["State"],
        json!(CREATING),
        "case {case:?}: the rule claims no VPC attachment, so the transport needs no acceptance, got {created}"
    );

    let (status, created) = create_connect_attachment(
        &emulator,
        json!({
            "CoreNetworkId": core_network,
            "EdgeLocation": region,
            "TransportAttachmentId": transport,
            "Options": { "Protocol": "GRE" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateConnectAttachment expected 200, got {status} with {created}"
    );
    let connect = attachment_id(case, &created["ConnectAttachment"]);
    let attachment = &created["ConnectAttachment"]["Attachment"];
    assert_eq!(
        attachment["State"],
        json!(PENDING_ACCEPTANCE),
        "case {case:?}: the Connect attachment is claimed by the rule, got {attachment}"
    );
    assert_eq!(
        attachment["AttachmentPolicyRuleNumber"],
        json!(20),
        "case {case:?}: expected the claiming rule's number, got {attachment}"
    );
    assert_eq!(
        attachment["SegmentName"],
        json!("prod"),
        "case {case:?}: expected the segment the rule named, got {attachment}"
    );

    let (status, answered) = accept(&emulator, &connect).await;
    assert_eq!(
        status, 200,
        "case {case:?}: AcceptAttachment expected 200, got {status} with {answered}"
    );
    assert_eq!(
        answered["Attachment"]["State"],
        json!(CREATING),
        "case {case:?}: the acceptance starts the Connect attachment, got {answered}"
    );
    let (status, read) = emulator
        .get(&format!("/connect-attachments/{connect}"))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetConnectAttachment expected 200, got {status} with {read}"
    );
    assert_eq!(
        read["ConnectAttachment"]["Attachment"]["State"],
        json!(CREATING),
        "case {case:?}: the acceptance must be persisted, got {read}"
    );
    assert_eq!(
        read["ConnectAttachment"]["TransportAttachmentId"],
        json!(transport),
        "case {case:?}: the accepted attachment keeps its transport, got {read}"
    );

    // The transport was never waiting for an acceptance, so it has none to grant.
    let (status, refused) = accept(&emulator, &transport).await;
    assert_eq!(
        status, 409,
        "case {case:?}: AcceptAttachment on the transport expected 409, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ConflictException",
        "case {case:?}: expected ConflictException, got {refused}"
    );
}
