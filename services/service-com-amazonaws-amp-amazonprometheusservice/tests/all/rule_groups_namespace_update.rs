// SPDX-License-Identifier: CC0-1.0

//! `PutRuleGroupsNamespace`, end to end over HTTP.
//!
//! The rule under test: an update REPLACES the rules file the named, existing namespace
//! holds in the persisted record, leaves every other member of that record alone (ARN,
//! name, creation instant, tags), moves only `modifiedAt`, and reports the namespace
//! `UPDATING` until the clock owes its move back to `ACTIVE` — and the operation only ever
//! updates: a name the workspace does not hold is `ResourceNotFoundException` and a
//! namespace or workspace that is not `ACTIVE` is `ConflictException`.
//!
//! Every suite spans what that rule generalises over — the bytes of the replacement rules
//! file (text, one byte, none at all, every byte value, kilobytes of it), the namespace
//! name across its modelled length bounds and character class, the number of successive
//! updates, the tags the namespace already carries, and the signing region — rather than
//! one recorded request's values.
//!
//! The request harness (`Amp`, `Reply`) and the create/describe calls are the ones
//! `workspace_lifecycle` and `rule_groups_namespace_lifecycle` already build.

use crate::rule_groups_namespace_lifecycle::{create_body, create_namespace, describe_namespace};
use crate::workspace_lifecycle::{tags_of, text, wait_for_due_transition, Amp, Reply};
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    NAMESPACE_STATUS_ACTIVE, NAMESPACE_STATUS_CREATING, NAMESPACE_STATUS_UPDATING,
    RULE_GROUPS_NAMESPACE_NOT_FOUND_MESSAGE, WORKSPACE_NOT_FOUND_MESSAGE,
};
use std::collections::BTreeMap;

/// `PUT /workspaces/{workspaceId}/rulegroupsnamespaces/{name}` —
/// PutRuleGroupsNamespace. `data` is a blob, so it travels base64-encoded on `restJson1`.
pub(crate) async fn put_namespace(
    amp: &Amp,
    region: &str,
    workspace_id: &str,
    name: &str,
    body: &serde_json::Value,
) -> Reply {
    amp.call(
        "PUT",
        &format!("/workspaces/{workspace_id}/rulegroupsnamespaces/{name}"),
        region,
        &body.to_string(),
    )
    .await
}

/// The update request body: just the replacement rules file, base64-encoded.
pub(crate) fn put_body(data: &[u8]) -> serde_json::Value {
    serde_json::json!({ "data": aws_smithy_types::base64::encode(data) })
}

/// The bytes a blob member carries, decoded from the base64 the response holds.
pub(crate) fn blob(case: &str, value: &serde_json::Value) -> Vec<u8> {
    let encoded = value.as_str().unwrap_or_else(|| {
        panic!("case {case:?}: a blob member must be a base64 string, got {value:?}")
    });
    aws_smithy_types::base64::decode(encoded)
        .unwrap_or_else(|error| panic!("case {case:?}: data must be base64 ({error}): {encoded:?}"))
}

/// The stored namespace as `(data, arn, createdAt, modifiedAt, tags, statusCode)` — what
/// an update must change exactly one part of, and a refused update none of.
struct Stored {
    data: Vec<u8>,
    arn: String,
    created_at: f64,
    modified_at: f64,
    tags: BTreeMap<String, String>,
    status_code: String,
}

async fn stored(case: &str, amp: &Amp, region: &str, workspace_id: &str, name: &str) -> Stored {
    let reply = describe_namespace(amp, region, workspace_id, name).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: DescribeRuleGroupsNamespace must answer 200, got {reply:?}"
    );
    let namespace = &reply.body["ruleGroupsNamespace"];
    Stored {
        data: blob(case, &namespace["data"]),
        arn: text(&namespace["arn"]),
        created_at: namespace["createdAt"].as_f64().unwrap_or_default(),
        modified_at: namespace["modifiedAt"].as_f64().unwrap_or_default(),
        tags: tags_of(&namespace["tags"]),
        status_code: text(&namespace["status"]["statusCode"]),
    }
}

/// One row of the update table: the namespace it acts on and the successive rules files
/// written over it.
struct Case {
    case: &'static str,
    name: String,
    /// The rules file the namespace is created with, then each replacement in turn.
    files: Vec<Vec<u8>>,
    tags: Vec<(String, String)>,
}

/// The table spans the bytes of the replacement rules file (text, one byte, none at all,
/// every byte value, kilobytes of it), the namespace name at each modelled
/// `RuleGroupsNamespaceName` bound (length 1..128) and across its character class
/// (`[0-9A-Za-z][-.0-9A-Z_a-z]*`), the number of successive updates, and whether the
/// namespace carries tags an update must leave alone.
fn update_cases() -> Vec<Case> {
    vec![
        Case {
            case: "a prometheus rules file replaced by a longer one",
            name: "rulesfile1".to_string(),
            files: vec![
                b"groups:\n  - name: cpu\n    rules:\n      - record: job:cpu\n        expr: sum(rate(cpu[5m]))\n".to_vec(),
                b"groups:\n  - name: cpu\n    rules:\n      - record: job:cpu\n        expr: sum(rate(cpu[5m]))\n  - name: mem\n    rules:\n      - alert: HighMem\n        expr: mem > 0.9\n".to_vec(),
            ],
            tags: vec![("team".to_string(), "observability".to_string())],
        },
        Case {
            case: "name at the length minimum of 1, three successive updates",
            name: "a".to_string(),
            files: vec![
                b"g".to_vec(),
                b"groups: []".to_vec(),
                b"h".to_vec(),
                Vec::new(),
            ],
            tags: Vec::new(),
        },
        Case {
            case: "name at the length maximum of 128, kilobytes of rules replaced",
            name: format!("n{}", "0".repeat(127)),
            files: vec![
                (0..4096u32).map(|i| (i % 251) as u8).collect(),
                (0..8192u32).map(|i| (i % 253) as u8).collect(),
            ],
            tags: (0..3)
                .map(|i| (format!("key-{i}"), format!("value-{i}")))
                .collect(),
        },
        Case {
            case: "name carrying every punctuation the pattern allows, every byte value",
            name: "A1-b.c_D".to_string(),
            files: vec![b"groups: []".to_vec(), (0..=255u8).collect()],
            tags: vec![("env".to_string(), "prod".to_string())],
        },
        Case {
            case: "an empty rules file replaced by one byte",
            name: "empty".to_string(),
            files: vec![Vec::new(), b"g".to_vec()],
            tags: Vec::new(),
        },
    ]
}

/// A put replaces the stored rules file and nothing else, and the reader sees the new
/// file byte for byte. One row per thing the rule generalises over, in two signing
/// regions — the region a namespace ARN names, which an update must not change.
#[tokio::test]
async fn a_put_replaces_the_stored_rules_file_and_leaves_the_rest_of_the_record() {
    let amp = Amp::new();

    for region in ["us-west-2", "eu-central-1"] {
        let workspace_id = amp.active_workspace(region, "rules-update").await;

        for case in update_cases() {
            let label = format!("{region} / {}", case.case);
            let created = create_namespace(
                &amp,
                region,
                &workspace_id,
                &create_body(&case.name, &case.files[0], &case.tags),
            )
            .await;
            assert_eq!(
                created.status, 202,
                "case {label:?}: seeding the namespace must answer 202, got {created:?}"
            );
            wait_for_due_transition();
            let before = stored(&label, &amp, region, &workspace_id, &case.name).await;
            assert_eq!(
                before.status_code, NAMESPACE_STATUS_ACTIVE,
                "case {label:?}: the seeded namespace must be ACTIVE before an update"
            );

            for (index, file) in case.files.iter().enumerate().skip(1) {
                let label = format!("{label} / update {index}");
                let previous = stored(&label, &amp, region, &workspace_id, &case.name).await;

                let put =
                    put_namespace(&amp, region, &workspace_id, &case.name, &put_body(file)).await;
                assert_eq!(
                    put.status, 202,
                    "case {label:?}: PutRuleGroupsNamespace must answer 202, got {put:?}"
                );
                assert_eq!(
                    text(&put.body["name"]),
                    case.name,
                    "case {label:?}: the put must report the namespace's name, got {put:?}"
                );
                assert_eq!(
                    text(&put.body["arn"]),
                    before.arn,
                    "case {label:?}: an update must not change the ARN, got {put:?}"
                );
                assert_eq!(
                    text(&put.body["status"]["statusCode"]),
                    NAMESPACE_STATUS_UPDATING,
                    "case {label:?}: an accepted update reports UPDATING, got {put:?}"
                );
                assert!(
                    put.body["status"]["statusReason"].is_null(),
                    "case {label:?}: nothing failed, so statusReason must be absent, got {put:?}"
                );
                assert_eq!(
                    tags_of(&put.body["tags"]),
                    case.tags.iter().cloned().collect::<BTreeMap<_, _>>(),
                    "case {label:?}: the put must report the namespace's stored tags, got {put:?}"
                );

                // The reader sees the replacement file, the transient status, and every
                // other member exactly as it was.
                let after = stored(&label, &amp, region, &workspace_id, &case.name).await;
                assert_eq!(
                    after.data,
                    *file,
                    "case {label:?}: the replacement rules file must read back byte for byte, \
                     got {} bytes against {} written",
                    after.data.len(),
                    file.len()
                );
                assert_eq!(
                    after.status_code, NAMESPACE_STATUS_UPDATING,
                    "case {label:?}: a caller reading straight after the put still sees UPDATING"
                );
                assert_eq!(
                    after.arn, before.arn,
                    "case {label:?}: an update must not change the ARN, got {:?}",
                    after.arn
                );
                assert_eq!(
                    after.created_at, before.created_at,
                    "case {label:?}: an update must not change createdAt, got {} against {}",
                    after.created_at, before.created_at
                );
                assert!(
                    after.modified_at >= previous.modified_at,
                    "case {label:?}: modifiedAt must move to the update's instant, got {} \
                     against the previous {}",
                    after.modified_at,
                    previous.modified_at
                );
                assert!(
                    after.modified_at >= after.created_at,
                    "case {label:?}: an updated namespace was modified no earlier than it was \
                     created, got modifiedAt {} and createdAt {}",
                    after.modified_at,
                    after.created_at
                );
                assert_eq!(
                    after.tags,
                    case.tags.iter().cloned().collect::<BTreeMap<_, _>>(),
                    "case {label:?}: an update must leave the stored tags alone, got {:?}",
                    after.tags
                );

                // Nothing but time makes the move back to ACTIVE due, and it leaves the
                // rules file the update stored untouched.
                wait_for_due_transition();
                let settled = stored(&label, &amp, region, &workspace_id, &case.name).await;
                assert_eq!(
                    settled.status_code, NAMESPACE_STATUS_ACTIVE,
                    "case {label:?}: the clock owes the move back to ACTIVE, got {:?}",
                    settled.status_code
                );
                assert_eq!(
                    settled.data, *file,
                    "case {label:?}: the transition must not change the stored rules file"
                );
                assert_eq!(
                    settled.modified_at, after.modified_at,
                    "case {label:?}: the transition is not a modification, got modifiedAt {} \
                     against {}",
                    settled.modified_at, after.modified_at
                );
            }
        }
    }
}

/// `tags` is not an input of this operation — "You can't use this operation to add tags to
/// an existing rule groups namespace. Instead, use TagResource." — so a body carrying tags
/// leaves the stored tags exactly as they are, whatever those tags are.
#[tokio::test]
async fn a_put_never_changes_the_stored_tags() {
    let amp = Amp::new();
    let region = "us-east-1";
    let workspace_id = amp.active_workspace(region, "tags-untouched").await;

    // One row per tag set the namespace already holds: none, one, and the `TagMap` bound
    // of 50 — and each is put with a DIFFERENT tag map in the body.
    let fifty: Vec<(String, String)> = (0..50)
        .map(|i| (format!("key-{i:02}"), format!("value-{i:02}")))
        .collect();
    let cases: Vec<(&str, Vec<(String, String)>)> = vec![
        ("no tags", Vec::new()),
        (
            "one tag",
            vec![("team".to_string(), "sre".to_string())],
        ),
        ("the TagMap bound of 50 tags", fifty),
    ];

    for (index, (case, tags)) in cases.iter().enumerate() {
        let name = format!("tagged{index}");
        let created =
            create_namespace(&amp, region, &workspace_id, &create_body(&name, b"groups: []", tags))
                .await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: seeding the namespace must answer 202, got {created:?}"
        );
        wait_for_due_transition();

        let mut body = put_body(b"groups: [{name: updated}]");
        body["tags"] = serde_json::json!({ "added-by": "the-put" });
        let put = put_namespace(&amp, region, &workspace_id, &name, &body).await;
        assert_eq!(
            put.status, 202,
            "case {case:?}: the put must answer 202, got {put:?}"
        );
        let expected: BTreeMap<String, String> = tags.iter().cloned().collect();
        assert_eq!(
            tags_of(&put.body["tags"]),
            expected,
            "case {case:?}: the put must report the stored tags, not the body's, got {put:?}"
        );
        let after = stored(case, &amp, region, &workspace_id, &name).await;
        assert_eq!(
            after.tags, expected,
            "case {case:?}: a put must add no tag to the stored namespace, got {:?}",
            after.tags
        );
        assert_eq!(
            after.data,
            b"groups: [{name: updated}]".to_vec(),
            "case {case:?}: the put must still have replaced the rules file"
        );
    }
}

/// `clientToken` carries `@idempotencyToken` on an `@idempotent` operation: replaying the
/// token that started the update still in flight answers with THAT update and writes
/// nothing. Once the update has settled the token stands for nothing, so a call reusing it
/// is a new request and is applied.
#[tokio::test]
async fn a_replayed_client_token_answers_with_the_update_it_started() {
    let amp = Amp::new();
    let region = "ap-northeast-1";
    let workspace_id = amp.active_workspace(region, "idempotent-put").await;

    // One row per form of token the modelled `IdempotencyToken` accepts (length 1..64,
    // pattern `^[!-~]+$`): the bounds of its length and its punctuation.
    let tokens = [
        ("single character, the length minimum", "t".to_string()),
        (
            "a uuid-shaped token",
            "3b9c1d20-5f6a-4e81-9c2b-7a8d6e5f4c30".to_string(),
        ),
        ("64 characters, the length maximum", "z".repeat(64)),
        ("printable punctuation", "!~#$%&*+/=?^_`{|}".to_string()),
    ];

    for (index, (case, token)) in tokens.iter().enumerate() {
        let name = format!("ns{index}");
        create_namespace(
            &amp,
            region,
            &workspace_id,
            &create_body(&name, b"groups: [{name: seed}]", &[]),
        )
        .await;
        wait_for_due_transition();

        let first_file = format!("groups: [{{name: {name}-first}}]").into_bytes();
        let mut body = put_body(&first_file);
        body["clientToken"] = serde_json::Value::String(token.clone());
        let first = put_namespace(&amp, region, &workspace_id, &name, &body).await;
        assert_eq!(
            first.status, 202,
            "case {case:?}: the first put must answer 202, got {first:?}"
        );
        let after_first = stored(case, &amp, region, &workspace_id, &name).await;

        // The replay carries DIFFERENT data under the same token, while the update is
        // still in flight: the answer is the update already started, and nothing is
        // written.
        let mut replay_body = put_body(b"groups: [{name: replaced}]");
        replay_body["clientToken"] = serde_json::Value::String(token.clone());
        let replay = put_namespace(&amp, region, &workspace_id, &name, &replay_body).await;
        assert_eq!(
            replay.status, 202,
            "case {case:?}: replaying the token must answer 202, got {replay:?}"
        );
        assert_eq!(
            text(&replay.body["arn"]),
            text(&first.body["arn"]),
            "case {case:?}: the replay must answer with the namespace the token updated, \
             got {replay:?}"
        );
        assert_eq!(
            text(&replay.body["status"]["statusCode"]),
            NAMESPACE_STATUS_UPDATING,
            "case {case:?}: the replay reports the in-flight update's status, got {replay:?}"
        );
        let after_replay = stored(case, &amp, region, &workspace_id, &name).await;
        assert_eq!(
            after_replay.data, first_file,
            "case {case:?}: the replay must not overwrite the stored rules file"
        );
        assert_eq!(
            after_replay.modified_at, after_first.modified_at,
            "case {case:?}: the replay wrote nothing, so modifiedAt must not move, got {} \
             against {}",
            after_replay.modified_at, after_first.modified_at
        );

        // Once the update has settled the token identifies nothing in flight, so the same
        // token is a NEW request and is applied.
        wait_for_due_transition();
        let mut reuse_body = put_body(b"groups: [{name: reused-token}]");
        reuse_body["clientToken"] = serde_json::Value::String(token.clone());
        let reuse = put_namespace(&amp, region, &workspace_id, &name, &reuse_body).await;
        assert_eq!(
            reuse.status, 202,
            "case {case:?}: reusing a settled token is a new request, got {reuse:?}"
        );
        let after_reuse = stored(case, &amp, region, &workspace_id, &name).await;
        assert_eq!(
            after_reuse.data,
            b"groups: [{name: reused-token}]".to_vec(),
            "case {case:?}: the new request must have replaced the stored rules file"
        );
        wait_for_due_transition();
    }
}

/// Each refusal the operation models, one row per class: a workspace that does not exist,
/// a name the workspace does not hold (the operation updates only — it never creates), a
/// namespace that is not `ACTIVE` (still being created, or still settling an update under
/// another token), and a workspace that is not `ACTIVE`. Every refusal leaves the stored
/// namespace exactly as it was.
#[tokio::test]
async fn refusals_answer_the_modelled_error_for_each_class() {
    let amp = Amp::new();
    let region = "ap-south-1";

    // A workspace id of the shape the service mints that no workspace holds.
    let absent_workspace = "ws-00000000-0000-0000-0000-000000000000";
    let absent = put_namespace(
        &amp,
        region,
        absent_workspace,
        "orphan",
        &put_body(b"groups: []"),
    )
    .await;
    assert_eq!(
        absent.status, 404,
        "case \"workspace does not exist\": expected 404, got {absent:?}"
    );
    assert_eq!(
        absent.error_code(),
        "ResourceNotFoundException",
        "case \"workspace does not exist\": expected the modelled not-found code, got {absent:?}"
    );
    assert_eq!(
        text(&absent.body["message"]),
        WORKSPACE_NOT_FOUND_MESSAGE,
        "case \"workspace does not exist\": expected the constant workspace message, \
         got {absent:?}"
    );

    let workspace_id = amp.active_workspace(region, "put-refusals").await;

    // "Use this operation only to update existing rule groups namespaces." — a name the
    // workspace does not hold is a missing resource, and no namespace appears.
    for name in ["never-created", "a", "A1-b.c_D"] {
        let missing = put_namespace(&amp, region, &workspace_id, name, &put_body(b"groups: []")).await;
        assert_eq!(
            missing.status, 404,
            "case {name:?}: a name the workspace does not hold must be refused 404, got {missing:?}"
        );
        assert_eq!(
            missing.error_code(),
            "ResourceNotFoundException",
            "case {name:?}: expected the modelled not-found code, got {missing:?}"
        );
        assert_eq!(
            text(&missing.body["message"]),
            RULE_GROUPS_NAMESPACE_NOT_FOUND_MESSAGE,
            "case {name:?}: expected the constant namespace message, got {missing:?}"
        );
        assert_eq!(
            text(&missing.body["resourceId"]),
            name,
            "case {name:?}: resourceId must name the namespace asked for, got {missing:?}"
        );
        let after = describe_namespace(&amp, region, &workspace_id, name).await;
        assert_eq!(
            after.status, 404,
            "case {name:?}: a refused put must create nothing, got {after:?}"
        );
    }

    // "Update/Deletion is disallowed until namespace is ACTIVE and workspace status is
    // ACTIVE": a namespace still CREATING is a conflict, and the put writes nothing.
    let created = create_namespace(
        &amp,
        region,
        &workspace_id,
        &create_body("tooearly", b"groups: [{name: seed}]", &[]),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case \"namespace still CREATING\": seeding must answer 202, got {created:?}"
    );
    let creating = stored(
        "namespace still CREATING",
        &amp,
        region,
        &workspace_id,
        "tooearly",
    )
    .await;
    assert_eq!(
        creating.status_code, NAMESPACE_STATUS_CREATING,
        "case \"namespace still CREATING\": the seeded namespace must still be CREATING"
    );
    let too_early = put_namespace(
        &amp,
        region,
        &workspace_id,
        "tooearly",
        &put_body(b"groups: [{name: tooearly}]"),
    )
    .await;
    assert_eq!(
        too_early.status, 409,
        "case \"namespace still CREATING\": expected 409, got {too_early:?}"
    );
    assert_eq!(
        too_early.error_code(),
        "ConflictException",
        "case \"namespace still CREATING\": expected ConflictException, got {too_early:?}"
    );
    let untouched = stored(
        "namespace still CREATING",
        &amp,
        region,
        &workspace_id,
        "tooearly",
    )
    .await;
    assert_eq!(
        untouched.data,
        b"groups: [{name: seed}]".to_vec(),
        "case \"namespace still CREATING\": a refused put must not touch the stored rules file"
    );
    assert_eq!(
        untouched.modified_at, creating.modified_at,
        "case \"namespace still CREATING\": a refused put must not move modifiedAt"
    );

    // A namespace still settling an update started under ANOTHER token is the same
    // conflict — only the token that started it replays.
    wait_for_due_transition();
    let mut first_body = put_body(b"groups: [{name: in-flight}]");
    first_body["clientToken"] = serde_json::Value::String("token-one".to_string());
    let accepted = put_namespace(&amp, region, &workspace_id, "tooearly", &first_body).await;
    assert_eq!(
        accepted.status, 202,
        "case \"namespace UPDATING\": the first update must be accepted, got {accepted:?}"
    );
    let in_flight = stored("namespace UPDATING", &amp, region, &workspace_id, "tooearly").await;
    let mut other_body = put_body(b"groups: [{name: other-token}]");
    other_body["clientToken"] = serde_json::Value::String("token-two".to_string());
    let conflict = put_namespace(&amp, region, &workspace_id, "tooearly", &other_body).await;
    assert_eq!(
        conflict.status, 409,
        "case \"namespace UPDATING\": expected 409, got {conflict:?}"
    );
    assert_eq!(
        conflict.error_code(),
        "ConflictException",
        "case \"namespace UPDATING\": expected ConflictException, got {conflict:?}"
    );
    let still_in_flight = stored("namespace UPDATING", &amp, region, &workspace_id, "tooearly").await;
    assert_eq!(
        still_in_flight.data, in_flight.data,
        "case \"namespace UPDATING\": a refused put must not touch the stored rules file"
    );

    // A workspace that is not ACTIVE forbids the call whatever the namespace's own status:
    // a workspace being deleted still holds its namespaces for the span of the deletion.
    wait_for_due_transition();
    let doomed = amp.active_workspace(region, "doomed").await;
    let seeded = create_namespace(
        &amp,
        region,
        &doomed,
        &create_body("rules", b"groups: [{name: seed}]", &[]),
    )
    .await;
    assert_eq!(
        seeded.status, 202,
        "case \"workspace DELETING\": seeding must answer 202, got {seeded:?}"
    );
    wait_for_due_transition();
    let deleting = amp.delete(region, &doomed, None).await;
    assert_eq!(
        deleting.status, 202,
        "case \"workspace DELETING\": the delete must answer 202, got {deleting:?}"
    );
    let refused = put_namespace(
        &amp,
        region,
        &doomed,
        "rules",
        &put_body(b"groups: [{name: too-late}]"),
    )
    .await;
    assert_eq!(
        refused.status, 409,
        "case \"workspace DELETING\": expected 409, got {refused:?}"
    );
    assert_eq!(
        refused.error_code(),
        "ConflictException",
        "case \"workspace DELETING\": expected ConflictException, got {refused:?}"
    );
}

/// Namespaces belong to their workspace: a put names one workspace's namespace and leaves
/// the same name in another workspace alone.
#[tokio::test]
async fn a_put_touches_only_the_named_workspace_s_namespace() {
    let amp = Amp::new();
    let region = "eu-west-1";
    let first = amp.active_workspace(region, "first").await;
    let second = amp.active_workspace(region, "second").await;

    for (case, workspace_id) in [("first workspace", &first), ("second workspace", &second)] {
        let created = create_namespace(
            &amp,
            region,
            workspace_id,
            &create_body("shared-name", b"groups: [{name: seed}]", &[]),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: seeding must answer 202, got {created:?}"
        );
    }
    wait_for_due_transition();

    let put = put_namespace(
        &amp,
        region,
        &first,
        "shared-name",
        &put_body(b"groups: [{name: only-the-first}]"),
    )
    .await;
    assert_eq!(
        put.status, 202,
        "case \"the named workspace\": the put must answer 202, got {put:?}"
    );

    let updated = stored("the named workspace", &amp, region, &first, "shared-name").await;
    assert_eq!(
        updated.data,
        b"groups: [{name: only-the-first}]".to_vec(),
        "case \"the named workspace\": the named namespace must hold the replacement file"
    );
    let untouched = stored("the other workspace", &amp, region, &second, "shared-name").await;
    assert_eq!(
        untouched.data,
        b"groups: [{name: seed}]".to_vec(),
        "case \"the other workspace\": the same name in another workspace must be untouched"
    );
    assert_eq!(
        untouched.status_code, NAMESPACE_STATUS_ACTIVE,
        "case \"the other workspace\": its status must be untouched too, got {:?}",
        untouched.status_code
    );
}
