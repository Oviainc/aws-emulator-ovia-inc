// SPDX-License-Identifier: CC0-1.0

//! Rule groups namespaces, end to end over HTTP: create one inside a workspace and read
//! it back with `DescribeRuleGroupsNamespace`, each member asserted against what the
//! service PERSISTED rather than against what the request carried.
//!
//! The rule under test is "a created rule groups namespace is persisted under its
//! workspace and reported back from that record" — so every suite here spans the inputs
//! that rule generalises over (the rules file's bytes, the namespace name across its
//! modelled length bounds and character class, the tag count, the signing region) and
//! never one recorded request's values.
//!
//! The request harness (`Amp`, `Reply`) is the one `workspace_lifecycle` already builds:
//! `MultiServiceRouter::dispatch` over this service mounted on one shared engine.

use crate::workspace_lifecycle::{tags_of, text, wait_for_due_transition, Amp, Reply};
use service_com_amazonaws_amp_amazonprometheusservice::ops::types::RULE_GROUPS_NAMESPACE_STATUS_CODE_VALUES;
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    NAMESPACE_STATUS_ACTIVE, NAMESPACE_STATUS_CREATING, NAMESPACE_STATUS_DELETING,
    NAMESPACE_STATUS_UPDATING,
};
use std::collections::BTreeMap;

/// `POST /workspaces/{workspaceId}/rulegroupsnamespaces` — CreateRuleGroupsNamespace.
pub(crate) async fn create_namespace(
    amp: &Amp,
    region: &str,
    workspace_id: &str,
    body: &serde_json::Value,
) -> Reply {
    amp.call(
        "POST",
        &format!("/workspaces/{workspace_id}/rulegroupsnamespaces"),
        region,
        &body.to_string(),
    )
    .await
}

/// `GET /workspaces/{workspaceId}/rulegroupsnamespaces/{name}` —
/// DescribeRuleGroupsNamespace.
pub(crate) async fn describe_namespace(
    amp: &Amp,
    region: &str,
    workspace_id: &str,
    name: &str,
) -> Reply {
    amp.call(
        "GET",
        &format!("/workspaces/{workspace_id}/rulegroupsnamespaces/{name}"),
        region,
        "",
    )
    .await
}

/// The create request body: `data` is a blob, so it travels base64-encoded on
/// `restJson1` — the same codec the wire serializer uses.
pub(crate) fn create_body(
    name: &str,
    data: &[u8],
    tags: &[(String, String)],
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "name": name,
        "data": aws_smithy_types::base64::encode(data),
    });
    if !tags.is_empty() {
        body["tags"] = serde_json::Value::Object(
            tags.iter()
                .map(|(key, value)| (key.clone(), serde_json::Value::String(value.clone())))
                .collect(),
        );
    }
    body
}

/// The bytes a blob member carries, decoded from the base64 the response holds.
fn blob(case: &str, value: &serde_json::Value) -> Vec<u8> {
    let encoded = value
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a blob member must be a base64 string, got {value:?}"));
    aws_smithy_types::base64::decode(encoded)
        .unwrap_or_else(|error| panic!("case {case:?}: data must be base64 ({error}): {encoded:?}"))
}

/// The ARN form `RuleGroupsNamespaceDescription$arn` documents: "For example,
/// `arn:aws:aps:<region>:123456789012:rulegroupsnamespace/ws-.../rulesfile1`".
fn expected_namespace_arn(region: &str, workspace_id: &str, name: &str) -> String {
    format!(
        "arn:aws:aps:{region}:{}:rulegroupsnamespace/{workspace_id}/{name}",
        engine::emulator_account_id()
    )
}

/// One row of the round-trip table: a namespace name and the rules file it holds.
struct Case {
    case: &'static str,
    name: String,
    data: Vec<u8>,
    tags: Vec<(String, String)>,
}

/// The table spans what the rule generalises over: the bytes of the rules file (text,
/// one byte, none at all, every byte value, kilobytes of it), the name at each modelled
/// `RuleGroupsNamespaceName` bound (length 1..128) and across its character class
/// (`[0-9A-Za-z][-.0-9A-Z_a-z]*`), and the tag count up to the `TagMap` bound of 50.
fn round_trip_cases() -> Vec<Case> {
    let fifty_tags: Vec<(String, String)> = (0..50)
        .map(|i| (format!("key-{i:02}"), format!("value-{i:02}")))
        .collect();

    vec![
        Case {
            case: "a prometheus rules file and one tag",
            name: "rulesfile1".to_string(),
            data: b"groups:\n  - name: cpu\n    rules:\n      - record: job:cpu\n        expr: sum(rate(cpu[5m]))\n"
                .to_vec(),
            tags: vec![("team".to_string(), "observability".to_string())],
        },
        Case {
            case: "name at the length minimum of 1, no tags",
            name: "a".to_string(),
            data: b"g".to_vec(),
            tags: Vec::new(),
        },
        Case {
            case: "name at the length maximum of 128, the TagMap bound of 50 tags",
            name: format!("n{}", "0".repeat(127)),
            data: (0..4096u32).map(|i| (i % 251) as u8).collect(),
            tags: fifty_tags,
        },
        Case {
            case: "name carrying every punctuation the pattern allows",
            name: "A1-b.c_D".to_string(),
            data: (0..=255u8).collect(),
            tags: vec![
                ("env".to_string(), "prod".to_string()),
                ("owner".to_string(), "sre".to_string()),
            ],
        },
        Case {
            case: "an empty rules file",
            name: "empty".to_string(),
            data: Vec::new(),
            tags: Vec::new(),
        },
    ]
}

/// Every status this crate reports for a namespace is a value the modelled
/// `RuleGroupsNamespaceStatusCode` enum declares — the catalog comes from the model, not
/// from this test.
#[test]
fn reported_statuses_are_modelled_rule_groups_namespace_status_codes() {
    for status in [
        NAMESPACE_STATUS_CREATING,
        NAMESPACE_STATUS_ACTIVE,
        NAMESPACE_STATUS_UPDATING,
        NAMESPACE_STATUS_DELETING,
    ] {
        assert!(
            RULE_GROUPS_NAMESPACE_STATUS_CODE_VALUES.contains(&status),
            "case {status:?}: not a modelled RuleGroupsNamespaceStatusCode \
             ({RULE_GROUPS_NAMESPACE_STATUS_CODE_VALUES:?})"
        );
    }
}

/// A create persists the namespace and answers from that record; a describe reads the
/// same record back, rules file included, byte for byte. One row per thing the rule
/// generalises over, in two signing regions — the region a namespace ARN must name.
#[tokio::test]
async fn create_persists_the_namespace_and_describe_reads_it_back() {
    let amp = Amp::new();
    let cases = round_trip_cases();

    for region in ["us-west-2", "eu-central-1"] {
        let workspace_id = amp.active_workspace(region, "rules-host").await;

        for case in &cases {
            let label = format!("{region} / {}", case.case);
            let created = create_namespace(
                &amp,
                region,
                &workspace_id,
                &create_body(&case.name, &case.data, &case.tags),
            )
            .await;
            assert_eq!(
                created.status, 202,
                "case {label:?}: CreateRuleGroupsNamespace must answer 202, got {created:?}"
            );
            assert_eq!(
                text(&created.body["name"]),
                case.name,
                "case {label:?}: created name must be the requested one, got {created:?}"
            );
            assert_eq!(
                text(&created.body["arn"]),
                expected_namespace_arn(region, &workspace_id, &case.name),
                "case {label:?}: arn must name this region, the emulator account, the workspace \
                 and the namespace, got {created:?}"
            );
            assert_eq!(
                text(&created.body["status"]["statusCode"]),
                NAMESPACE_STATUS_CREATING,
                "case {label:?}: a fresh namespace is CREATING, got {created:?}"
            );
            assert!(
                created.body["status"]["statusReason"].is_null(),
                "case {label:?}: nothing failed, so statusReason must be absent, got {created:?}"
            );
            assert_eq!(
                tags_of(&created.body["tags"]),
                case.tags.iter().cloned().collect::<BTreeMap<_, _>>(),
                "case {label:?}: created tags must be the stored ones, got {created:?}"
            );

            // The reader sees the same record, including the transient status a caller
            // acting straight after the create still observes.
            let described = describe_namespace(&amp, region, &workspace_id, &case.name).await;
            assert_eq!(
                described.status, 200,
                "case {label:?}: DescribeRuleGroupsNamespace must answer 200, got {described:?}"
            );
            let namespace = &described.body["ruleGroupsNamespace"];
            assert_eq!(
                text(&namespace["name"]),
                case.name,
                "case {label:?}: described name must be the stored one, got {described:?}"
            );
            assert_eq!(
                text(&namespace["arn"]),
                text(&created.body["arn"]),
                "case {label:?}: describe must report the arn the create minted, got {described:?}"
            );
            assert_eq!(
                text(&namespace["status"]["statusCode"]),
                NAMESPACE_STATUS_CREATING,
                "case {label:?}: the namespace is still CREATING, got {described:?}"
            );
            assert_eq!(
                blob(&label, &namespace["data"]),
                case.data,
                "case {label:?}: the rules file must read back byte for byte, got {} bytes",
                blob(&label, &namespace["data"]).len()
            );
            assert_eq!(
                tags_of(&namespace["tags"]),
                case.tags.iter().cloned().collect::<BTreeMap<_, _>>(),
                "case {label:?}: described tags must be the stored ones, got {described:?}"
            );
            let created_at = namespace["createdAt"].as_f64().unwrap_or_default();
            let modified_at = namespace["modifiedAt"].as_f64().unwrap_or_default();
            assert!(
                created_at > 0.0,
                "case {label:?}: createdAt must be the creation instant, got {created_at}"
            );
            assert_eq!(
                created_at, modified_at,
                "case {label:?}: an unchanged namespace was last modified when it was created, \
                 got createdAt {created_at} and modifiedAt {modified_at}"
            );
        }

        // The clock owes each namespace its move out of CREATING; nothing but time makes
        // it due, and the stored rules file is untouched by the transition.
        wait_for_due_transition();
        for case in &cases {
            let label = format!("{region} / {}", case.case);
            let described = describe_namespace(&amp, region, &workspace_id, &case.name).await;
            assert_eq!(
                text(&described.body["ruleGroupsNamespace"]["status"]["statusCode"]),
                NAMESPACE_STATUS_ACTIVE,
                "case {label:?}: the clock owes the move to ACTIVE, got {described:?}"
            );
            assert_eq!(
                blob(&label, &described.body["ruleGroupsNamespace"]["data"]),
                case.data,
                "case {label:?}: the transition must not change the stored rules file"
            );
        }
    }
}

/// `clientToken` carries `@idempotencyToken`: replaying a token answers with the
/// namespace it already created and writes nothing — not a second namespace, and not a
/// new rules file over the stored one. A DIFFERENT token with the same name is a new
/// create, which the name already in the workspace refuses.
#[tokio::test]
async fn a_replayed_client_token_answers_with_the_namespace_it_created() {
    let amp = Amp::new();
    let region = "us-east-2";
    let workspace_id = amp.active_workspace(region, "idempotent-host").await;

    // One row per form of token the modelled `IdempotencyToken` accepts (length 1..64,
    // pattern `^[!-~]+$`): the bounds of its length and its punctuation.
    let tokens = [
        ("single character, the length minimum", "t".to_string()),
        ("a uuid-shaped token", "7f1c9a60-2b4e-4a51-9f2f-8c7d6e5b4a30".to_string()),
        ("64 characters, the length maximum", "z".repeat(64)),
        ("printable punctuation", "!~#$%&*+/=?^_`{|}".to_string()),
    ];

    for (index, (case, token)) in tokens.iter().enumerate() {
        let name = format!("ns{index}");
        let first_data = format!("groups: [{{name: {name}}}]").into_bytes();
        let mut body = create_body(&name, &first_data, &[]);
        body["clientToken"] = serde_json::Value::String(token.clone());

        let first = create_namespace(&amp, region, &workspace_id, &body).await;
        assert_eq!(
            first.status, 202,
            "case {case:?}: the first create must answer 202, got {first:?}"
        );

        // The replay carries DIFFERENT data under the same token: the answer must still
        // be the namespace the first call created.
        let mut replay_body = create_body(&name, b"groups: [{name: replaced}]", &[]);
        replay_body["clientToken"] = serde_json::Value::String(token.clone());
        let replay = create_namespace(&amp, region, &workspace_id, &replay_body).await;
        assert_eq!(
            replay.status, 202,
            "case {case:?}: replaying the token must answer 202, got {replay:?}"
        );
        assert_eq!(
            text(&replay.body["arn"]),
            text(&first.body["arn"]),
            "case {case:?}: the replay must answer with the namespace the token created, \
             got {replay:?}"
        );

        let described = describe_namespace(&amp, region, &workspace_id, &name).await;
        assert_eq!(
            blob(case, &described.body["ruleGroupsNamespace"]["data"]),
            first_data,
            "case {case:?}: the replay must not overwrite the stored rules file"
        );

        // A different token naming the same namespace is a new create, and the name is
        // taken: "Use this operation only to create new rule groups namespaces."
        let mut other_body = create_body(&name, b"groups: [{name: other}]", &[]);
        other_body["clientToken"] = serde_json::Value::String(format!("other-{index}"));
        let conflict = create_namespace(&amp, region, &workspace_id, &other_body).await;
        assert_eq!(
            conflict.status, 409,
            "case {case:?}: a second create of the same name must conflict, got {conflict:?}"
        );
        assert_eq!(
            conflict.error_code(),
            "ConflictException",
            "case {case:?}: the refusal must be the modelled ConflictException, got {conflict:?}"
        );
    }
}

/// Each refusal the operations model, one row per class: an absent workspace, a
/// workspace that is not yet usable, a name the workspace already holds, a reserved tag
/// key, and a describe of a namespace that was never created.
#[tokio::test]
async fn refusals_answer_the_modelled_error_for_each_class() {
    let amp = Amp::new();
    let region = "ap-south-1";

    // A workspace id of the shape the service mints that no workspace holds.
    let absent_workspace = "ws-00000000-0000-0000-0000-000000000000";
    let absent = create_namespace(
        &amp,
        region,
        absent_workspace,
        &create_body("orphan", b"groups: []", &[]),
    )
    .await;
    assert_eq!(
        absent.status, 404,
        "case \"workspace does not exist\": expected 404, got {absent:?}"
    );
    assert_eq!(
        absent.error_code(),
        "ResourceNotFoundException",
        "case \"workspace does not exist\": expected ResourceNotFoundException, got {absent:?}"
    );

    // A workspace is usable only once it is ACTIVE, so a namespace cannot be added while
    // it is still CREATING.
    let creating = amp
        .call("POST", "/workspaces", region, r#"{"alias":"still-creating"}"#)
        .await;
    assert_eq!(
        creating.status, 202,
        "case \"workspace still CREATING\": the workspace create must answer 202, got {creating:?}"
    );
    let creating_id = text(&creating.body["workspaceId"]);
    let too_early = create_namespace(
        &amp,
        region,
        &creating_id,
        &create_body("tooearly", b"groups: []", &[]),
    )
    .await;
    assert_eq!(
        too_early.status, 409,
        "case \"workspace still CREATING\": expected 409, got {too_early:?}"
    );
    assert_eq!(
        too_early.error_code(),
        "ConflictException",
        "case \"workspace still CREATING\": expected ConflictException, got {too_early:?}"
    );

    let workspace_id = amp.active_workspace(region, "refusal-host").await;

    let first = create_namespace(
        &amp,
        region,
        &workspace_id,
        &create_body("taken", b"groups: []", &[]),
    )
    .await;
    assert_eq!(
        first.status, 202,
        "case \"name already taken\": the first create must answer 202, got {first:?}"
    );
    let duplicate = create_namespace(
        &amp,
        region,
        &workspace_id,
        &create_body("taken", b"groups: [{name: other}]", &[]),
    )
    .await;
    assert_eq!(
        duplicate.status, 409,
        "case \"name already taken\": expected 409, got {duplicate:?}"
    );
    assert_eq!(
        duplicate.error_code(),
        "ConflictException",
        "case \"name already taken\": expected ConflictException, got {duplicate:?}"
    );
    // And the refused create changed nothing: the stored rules file is the first one.
    let still_first = describe_namespace(&amp, region, &workspace_id, "taken").await;
    assert_eq!(
        blob("name already taken", &still_first.body["ruleGroupsNamespace"]["data"]),
        b"groups: []".to_vec(),
        "case \"name already taken\": a refused create must not touch the stored namespace"
    );

    // "Keys must not begin with aws:" — the service reserves that prefix for itself.
    for reserved_key in ["aws:created-by", "aws:"] {
        let refused = create_namespace(
            &amp,
            region,
            &workspace_id,
            &create_body(
                "reserved",
                b"groups: []",
                &[(reserved_key.to_string(), "no".to_string())],
            ),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {reserved_key:?}: a reserved tag key must be refused with 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {reserved_key:?}: expected ValidationException, got {refused:?}"
        );
        let absent_after = describe_namespace(&amp, region, &workspace_id, "reserved").await;
        assert_eq!(
            absent_after.status, 404,
            "case {reserved_key:?}: a refused create must persist nothing, got {absent_after:?}"
        );
    }

    // A describe of a name no workspace holds, and of a workspace that does not exist.
    let missing = describe_namespace(&amp, region, &workspace_id, "never-created").await;
    assert_eq!(
        missing.status, 404,
        "case \"namespace does not exist\": expected 404, got {missing:?}"
    );
    assert_eq!(
        missing.error_code(),
        "ResourceNotFoundException",
        "case \"namespace does not exist\": expected ResourceNotFoundException, got {missing:?}"
    );
    let missing_workspace = describe_namespace(&amp, region, absent_workspace, "never-created").await;
    assert_eq!(
        missing_workspace.status, 404,
        "case \"describe in an absent workspace\": expected 404, got {missing_workspace:?}"
    );
}

/// Namespaces belong to their workspace: the same name in two workspaces is two
/// records, each with its own ARN and its own rules file.
#[tokio::test]
async fn namespaces_are_scoped_to_their_workspace() {
    let amp = Amp::new();
    let region = "eu-west-1";
    let first_workspace = amp.active_workspace(region, "first").await;
    let second_workspace = amp.active_workspace(region, "second").await;

    for (case, workspace_id, data) in [
        ("first workspace", &first_workspace, b"groups: [{name: one}]".to_vec()),
        ("second workspace", &second_workspace, b"groups: [{name: two}]".to_vec()),
    ] {
        let created = create_namespace(
            &amp,
            region,
            workspace_id,
            &create_body("shared-name", &data, &[]),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: the create must answer 202, got {created:?}"
        );
        assert_eq!(
            text(&created.body["arn"]),
            expected_namespace_arn(region, workspace_id, "shared-name"),
            "case {case:?}: the arn must name the namespace's own workspace, got {created:?}"
        );
    }

    for (case, workspace_id, expected) in [
        ("first workspace", &first_workspace, b"groups: [{name: one}]".to_vec()),
        ("second workspace", &second_workspace, b"groups: [{name: two}]".to_vec()),
    ] {
        let described = describe_namespace(&amp, region, workspace_id, "shared-name").await;
        assert_eq!(
            blob(case, &described.body["ruleGroupsNamespace"]["data"]),
            expected,
            "case {case:?}: each workspace holds its own rules file under that name"
        );
    }
}
