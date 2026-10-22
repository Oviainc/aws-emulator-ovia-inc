// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of Amazon Connect's flow loop — `CreateContactFlow` and the
//! `DescribeContactFlow` that reads it back — driven over HTTP through the same
//! `MultiServiceRouter` the emulator serves with.
//!
//! Every suite asserts a RULE over a table of inputs rather than one recorded call:
//! what a create persists is exactly what every later read reports, a flow's identity
//! (a fresh UUID id, the ARN built from its instance's ARN, and the SHA-256 of its
//! content) is derived from the stored flow, a flow name is unique within its instance,
//! and the content validation Connect documents for `PUBLISHED` runs for `PUBLISHED`
//! alone.

use std::collections::BTreeSet;

use engine::EMULATOR_ACCOUNT_ID;
use hex::encode as hex_encode;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::harness::{connect_managed, looks_like_a_uuid, Answer, Connect};

/// Every `ContactFlowType` member, from `com.amazonaws.connect#ContactFlowType`
/// (smithy_query(query="shape",shape="ContactFlowType")). A flow's `Type` is stored and
/// reported verbatim, so the rule has to hold for each one — including the members this
/// issue never cited.
const CONTACT_FLOW_TYPES: [&str; 10] = [
    "CONTACT_FLOW",
    "CUSTOMER_QUEUE",
    "CUSTOMER_HOLD",
    "CUSTOMER_WHISPER",
    "AGENT_HOLD",
    "AGENT_WHISPER",
    "OUTBOUND_WHISPER",
    "AGENT_TRANSFER",
    "QUEUE_TRANSFER",
    "CAMPAIGN",
];

/// A flow content of the documented form: the JSON string of a flow definition.
const FLOW_CONTENT: &str =
    r#"{"Version":"2019-10-30","StartAction":"a1","Actions":[{"Identifier":"a1","Type":"DisconnectParticipant","Parameters":{}}]}"#;

impl Connect {
    async fn create_contact_flow(&self, instance_id: &str, body: Value) -> Answer {
        self.send_answer("PUT", &format!("/contact-flows/{instance_id}"), Some(body))
            .await
    }

    async fn describe_contact_flow(&self, instance_id: &str, label: &str) -> Answer {
        self.send_answer("GET", &format!("/contact-flows/{instance_id}/{label}"), None)
            .await
    }
}

/// The SHA-256 of a flow content, lowercase hex — computed here from the content the
/// test sent, so the assertion never reads the value back out of the handler.
fn sha256_hex(content: &str) -> String {
    hex_encode(Sha256::digest(content.as_bytes()))
}

/// A minimal valid `CreateContactFlow` body.
fn flow_named(name: &str, flow_type: &str) -> Value {
    json!({ "Name": name, "Type": flow_type, "Content": FLOW_CONTENT })
}

/// The `ContactFlowId` label that reads a flow back: a flow left in the `SAVED` status
/// is read through the `$SAVED` alias the model documents, a published one directly.
fn label_for(id: &str, status: &str) -> String {
    if status == "SAVED" {
        format!("{id}:$SAVED")
    } else {
        id.to_string()
    }
}

/// RULE: every member `CreateContactFlow` accepts is persisted and reported verbatim by
/// `DescribeContactFlow`, and the flow's derived identity — a fresh UUID id, the ARN
/// `<instance ARN>/contact-flow/<id>`, and `FlowContentSha256` as the SHA-256 of the
/// stored content — is computed from the stored flow.
///
/// The table spans every `ContactFlowType` member, an absent and a present description,
/// tag counts 0/1/2, both `Status` members plus an absent one, the `ContactFlowName`
/// lower bound (1 character) and a long name, and contents of three different sizes.
#[tokio::test]
async fn created_flow_members_are_what_every_later_read_reports() {
    let connect = Connect::new();
    let (instance_id, instance_arn) = connect.create_instance(connect_managed("flows")).await;

    let long_name = "n".repeat(256);
    let big_content = json!({
        "Version": "2019-10-30",
        "StartAction": "a1",
        "Actions": (0..64)
            .map(|index| json!({ "Identifier": format!("a{index}"), "Type": "MessageParticipant" }))
            .collect::<Vec<Value>>(),
    })
    .to_string();

    let mut cases: Vec<(String, Value)> = Vec::new();
    // One case per modelled flow type, cycling the optional members so the table spans
    // them too rather than repeating one shape ten times.
    for (index, flow_type) in CONTACT_FLOW_TYPES.iter().enumerate() {
        let mut body = flow_named(&format!("flow-{flow_type}"), flow_type);
        match index % 5 {
            0 => {} // no description, no tags, no Status: the plainest request there is
            1 => {
                body["Description"] = json!("one tag, published explicitly");
                body["Status"] = json!("PUBLISHED");
                body["Tags"] = json!({ "env": "prod" });
            }
            2 => {
                body["Description"] = json!("saved, not published");
                body["Status"] = json!("SAVED");
                body["Tags"] = json!({ "env": "dev", "team": "cx" });
            }
            3 => {
                // ContactFlowName lower bound: 1 character. Each pass needs its own
                // name, since a name is unique within the instance.
                body["Name"] = json!(if index == 3 { "f" } else { "g" });
                body["Content"] = json!("{}");
            }
            _ => {
                body["Name"] = json!(format!("{long_name}-{flow_type}"));
                body["Content"] = json!(big_content);
                body["Description"] = json!("a large content");
            }
        }
        cases.push((format!("{flow_type} (variant {})", index % 5), body));
    }

    let mut minted_ids: BTreeSet<String> = BTreeSet::new();
    let mut minted_arns: BTreeSet<String> = BTreeSet::new();

    for (case, body) in &cases {
        let created = connect.create_contact_flow(&instance_id, body.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateContactFlow got {} {}",
            created.status, created.body
        );

        let id = created.body["ContactFlowId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no ContactFlowId in {}", created.body))
            .to_string();
        let arn = created.body["ContactFlowArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no ContactFlowArn in {}", created.body))
            .to_string();
        let checksum = created.body["FlowContentSha256"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no FlowContentSha256 in {}", created.body))
            .to_string();

        assert!(
            looks_like_a_uuid(&id),
            "case {case:?}: expected a v4-UUID flow id, got {id:?}"
        );
        assert!(
            minted_ids.insert(id.clone()),
            "case {case:?}: expected a fresh id, got the already-minted {id:?}"
        );
        assert_eq!(
            arn,
            format!("{instance_arn}/contact-flow/{id}"),
            "case {case:?}: the ARN must be the instance's ARN plus the minted id"
        );
        assert!(
            minted_arns.insert(arn.clone()),
            "case {case:?}: expected a fresh ARN, got the already-minted {arn:?}"
        );

        let content = body["Content"].as_str().expect("a Content string");
        assert_eq!(
            checksum,
            sha256_hex(content),
            "case {case:?}: FlowContentSha256 must be the SHA-256 of the stored content"
        );
        assert!(
            checksum.len() == 64 && checksum.chars().all(|c| c.is_ascii_alphanumeric()),
            "case {case:?}: FlowContentSha256 must match ^[a-zA-Z0-9]{{64}}$, got {checksum:?}"
        );

        let expected_status = body["Status"].as_str().unwrap_or("PUBLISHED").to_string();
        let label = label_for(&id, &expected_status);
        let read = connect.describe_contact_flow(&instance_id, &label).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: DescribeContactFlow {label} got {} {}",
            read.status, read.body
        );
        let flow = &read.body["ContactFlow"];

        assert_eq!(flow["Id"], json!(id), "case {case:?}: Id");
        assert_eq!(flow["Arn"], json!(arn), "case {case:?}: Arn");
        assert_eq!(flow["Name"], body["Name"], "case {case:?}: Name");
        assert_eq!(flow["Type"], body["Type"], "case {case:?}: Type");
        assert_eq!(flow["Content"], body["Content"], "case {case:?}: Content");
        assert_eq!(
            flow["Status"],
            json!(expected_status),
            "case {case:?}: Status (an absent request Status is a published flow)"
        );
        assert_eq!(
            flow["State"],
            json!("ACTIVE"),
            "case {case:?}: a created flow is ACTIVE"
        );
        assert_eq!(
            flow["Description"],
            body.get("Description").cloned().unwrap_or(Value::Null),
            "case {case:?}: Description"
        );
        assert_eq!(
            flow["Tags"],
            body.get("Tags").cloned().unwrap_or(Value::Null),
            "case {case:?}: Tags"
        );
        assert_eq!(
            flow["FlowContentSha256"],
            json!(checksum),
            "case {case:?}: the read must report the checksum the create answered with"
        );
        assert_eq!(
            flow["LastModifiedRegion"],
            json!("us-east-1"),
            "case {case:?}: LastModifiedRegion"
        );
        assert!(
            flow["LastModifiedTime"].as_f64().is_some_and(|at| at > 0.0),
            "case {case:?}: expected a LastModifiedTime timestamp, got {}",
            flow["LastModifiedTime"]
        );
        assert!(
            flow["Version"].is_null(),
            "case {case:?}: no flow version has been created, so Version stays absent, got {}",
            flow["Version"]
        );

        // The second read answers from storage too: identical, not re-derived.
        let again = connect.describe_contact_flow(&instance_id, &label).await;
        assert_eq!(
            again.body["ContactFlow"], *flow,
            "case {case:?}: every read of a flow reports the same stored flow"
        );
    }
}

/// RULE: a flow name is unique within its instance — a second create under a name the
/// instance already holds is refused with `DuplicateResourceException` (409) and leaves
/// the stored flow untouched — but the same name in another instance is a new flow.
#[tokio::test]
async fn flow_names_are_unique_within_an_instance_only() {
    let connect = Connect::new();
    let (first_instance, _) = connect.create_instance(connect_managed("one")).await;
    let (second_instance, _) = connect.create_instance(connect_managed("two")).await;

    // Names differing in length and character class: the rule is about the name being
    // taken, not about what the name looks like.
    let long_name = "n".repeat(256);
    for name in ["support", "f", long_name.as_str(), "flow with spaces"] {
        let created = connect
            .create_contact_flow(&first_instance, flow_named(name, "CONTACT_FLOW"))
            .await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: the first create must succeed, got {} {}",
            created.status, created.body
        );
        let id = created.body["ContactFlowId"]
            .as_str()
            .expect("ContactFlowId")
            .to_string();

        let mut repeat = flow_named(name, "CAMPAIGN");
        repeat["Content"] = json!("{\"Version\":\"2019-10-30\"}");
        let refused = connect.create_contact_flow(&first_instance, repeat).await;
        assert_eq!(
            refused.status, 409,
            "case {name:?}: a duplicate name must be a 409, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code, "DuplicateResourceException",
            "case {name:?}: expected DuplicateResourceException, got {:?} {}",
            refused.error_code, refused.body
        );

        // The refused create changed nothing: the stored flow is still the first one.
        let read = connect.describe_contact_flow(&first_instance, &id).await;
        assert_eq!(
            read.body["ContactFlow"]["Type"],
            json!("CONTACT_FLOW"),
            "case {name:?}: a refused duplicate must not overwrite the stored flow, got {}",
            read.body
        );
        assert_eq!(
            read.body["ContactFlow"]["Content"],
            json!(FLOW_CONTENT),
            "case {name:?}: a refused duplicate must not overwrite the stored content, got {}",
            read.body
        );

        // Another instance has its own namespace.
        let elsewhere = connect
            .create_contact_flow(&second_instance, flow_named(name, "CONTACT_FLOW"))
            .await;
        assert_eq!(
            elsewhere.status, 200,
            "case {name:?}: the same name in another instance is a new flow, got {} {}",
            elsewhere.status, elsewhere.body
        );
        assert_ne!(
            elsewhere.body["ContactFlowId"], created.body["ContactFlowId"],
            "case {name:?}: the two instances' flows are distinct resources"
        );
    }
}

/// RULE: the `PUBLISHED` status initiates validation of the content and the `SAVED`
/// status does not, so a content that is not a flow definition is refused with
/// `InvalidContactFlowException` (400) when the flow is published — whether `Status`
/// says `PUBLISHED` or is absent, which is the same thing — and stored as given when it
/// is only saved.
#[tokio::test]
async fn only_a_published_flow_has_its_content_validated() {
    let connect = Connect::new();
    let (instance_id, _) = connect.create_instance(connect_managed("validation")).await;

    // (case, content, whether the content is a flow definition: a JSON object)
    let contents: Vec<(&str, &str, bool)> = vec![
        ("a flow definition", FLOW_CONTENT, true),
        ("an empty JSON object", "{}", true),
        ("a nested JSON object", r#"{"a":{"b":[1,2,{"c":null}]}}"#, true),
        ("not JSON at all", "this is not a flow", false),
        ("a JSON array", "[]", false),
        ("a JSON string", "\"flow\"", false),
        ("a JSON number", "42", false),
        ("a truncated object", "{\"Version\":", false),
        ("a single space", " ", false),
    ];

    for (index, (case, content, is_definition)) in contents.iter().enumerate() {
        for status in [Some("PUBLISHED"), None, Some("SAVED")] {
            let published = status.unwrap_or("PUBLISHED") == "PUBLISHED";
            let name = format!("flow-{index}-{}", status.unwrap_or("default"));
            let mut body = flow_named(&name, "CONTACT_FLOW");
            body["Content"] = json!(content);
            if let Some(status) = status {
                body["Status"] = json!(status);
            }

            let answer = connect.create_contact_flow(&instance_id, body).await;
            let case = format!("{case} with Status {status:?}");

            if published && !is_definition {
                assert_eq!(
                    answer.status, 400,
                    "case {case:?}: a published flow's content is validated, got {} {}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code, "InvalidContactFlowException",
                    "case {case:?}: expected InvalidContactFlowException, got {:?} {}",
                    answer.error_code, answer.body
                );
                let problems = answer.body["problems"]
                    .as_array()
                    .unwrap_or_else(|| panic!("case {case:?}: no problems in {}", answer.body));
                assert!(
                    problems
                        .iter()
                        .all(|problem| problem["message"].as_str().is_some_and(|m| !m.is_empty())),
                    "case {case:?}: every problem must carry a message, got {}",
                    answer.body
                );
                continue;
            }

            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the create to succeed, got {} {}",
                answer.status, answer.body
            );
            let id = answer.body["ContactFlowId"].as_str().expect("ContactFlowId");
            let label = label_for(id, status.unwrap_or("PUBLISHED"));
            let read = connect.describe_contact_flow(&instance_id, &label).await;
            assert_eq!(
                read.body["ContactFlow"]["Content"],
                json!(content),
                "case {case:?}: the content must be stored exactly as given, got {}",
                read.body
            );
        }
    }
}

/// RULE: a create or a read naming an instance or a flow the store does not hold is
/// `ResourceNotFoundException` (404); an unqualified read of a flow that was never
/// published is `ContactFlowNotPublishedException` (404); and a flow is gone with its
/// instance.
#[tokio::test]
async fn unknown_instances_flows_and_unpublished_reads_are_refused() {
    let connect = Connect::new();
    let (instance_id, _) = connect.create_instance(connect_managed("missing")).await;

    let published = connect
        .create_contact_flow(&instance_id, flow_named("published", "CONTACT_FLOW"))
        .await;
    let published_id = published.body["ContactFlowId"]
        .as_str()
        .expect("ContactFlowId")
        .to_string();

    let mut saved = flow_named("saved", "CONTACT_FLOW");
    saved["Status"] = json!("SAVED");
    let saved = connect.create_contact_flow(&instance_id, saved).await;
    let saved_id = saved.body["ContactFlowId"]
        .as_str()
        .expect("ContactFlowId")
        .to_string();

    // An identifier of the right modelled form that names nothing.
    let absent = "11111111-2222-3333-4444-555555555555";

    let not_found: Vec<(&str, Answer)> = vec![
        (
            "CreateContactFlow in an absent instance",
            connect
                .create_contact_flow(absent, flow_named("orphan", "CONTACT_FLOW"))
                .await,
        ),
        (
            "DescribeContactFlow of an absent flow",
            connect.describe_contact_flow(&instance_id, absent).await,
        ),
        (
            "DescribeContactFlow inside an absent instance",
            connect.describe_contact_flow(absent, &published_id).await,
        ),
        (
            "DescribeContactFlow of a flow version that was never created",
            connect
                .describe_contact_flow(&instance_id, &format!("{published_id}:3"))
                .await,
        ),
    ];
    for (case, answer) in not_found {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?} {}",
            answer.error_code, answer.body
        );
        assert!(
            answer.body["message"].is_string() || answer.body["Message"].is_string(),
            "case {case:?}: the error must carry a message, got {}",
            answer.body
        );
    }

    // A flow that only ever reached SAVED has no published content to report.
    let unpublished = connect.describe_contact_flow(&instance_id, &saved_id).await;
    assert_eq!(
        unpublished.status, 404,
        "case {:?}: got {} {}",
        "unqualified read of a SAVED flow", unpublished.status, unpublished.body
    );
    assert_eq!(
        unpublished.error_code, "ContactFlowNotPublishedException",
        "case {:?}: expected ContactFlowNotPublishedException, got {:?} {}",
        "unqualified read of a SAVED flow", unpublished.error_code, unpublished.body
    );
    let through_alias = connect
        .describe_contact_flow(&instance_id, &format!("{saved_id}:$SAVED"))
        .await;
    assert_eq!(
        through_alias.status, 200,
        "case {:?}: the $SAVED alias reads the saved content, got {} {}",
        "aliased read of a SAVED flow", through_alias.status, through_alias.body
    );

    // Deleting the instance takes its flows with it.
    assert_eq!(connect.delete_instance(&instance_id).await, 200);
    let orphaned = connect
        .describe_contact_flow(&instance_id, &published_id)
        .await;
    assert_eq!(
        orphaned.status, 404,
        "case {:?}: a deleted instance's flows are gone with it, got {} {}",
        "read after DeleteInstance", orphaned.status, orphaned.body
    );
}

/// RULE: a flow's ARN is built from its instance's ARN, so it carries the region the
/// request was signed for.
#[tokio::test]
async fn flow_arns_carry_the_requests_region() {
    // Any AWS region token: the ARN's region segment comes from the request, never from
    // a constant in the handler.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let connect = Connect::in_region(region);
        let (instance_id, _) = connect.create_instance(connect_managed("regional")).await;
        let created = connect
            .create_contact_flow(&instance_id, flow_named("inbound", "CONTACT_FLOW"))
            .await;
        let id = created.body["ContactFlowId"].as_str().expect("ContactFlowId");

        assert_eq!(
            created.body["ContactFlowArn"],
            json!(format!(
                "arn:aws:connect:{region}:{EMULATOR_ACCOUNT_ID}:instance/{instance_id}/contact-flow/{id}"
            )),
            "case {region:?}: the flow ARN must carry the request's region"
        );
        let read = connect.describe_contact_flow(&instance_id, id).await;
        assert_eq!(
            read.body["ContactFlow"]["LastModifiedRegion"],
            json!(region),
            "case {region:?}: LastModifiedRegion must be the request's region, got {}",
            read.body
        );
    }
}
