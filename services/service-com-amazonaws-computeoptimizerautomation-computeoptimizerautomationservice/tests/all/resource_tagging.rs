// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `TagResource`.
//!
//! The rule under test: `TagResource` merges the tags a request carries into the
//! tag list the store holds for the resource its `resourceArn` names — a key the
//! resource does not carry is added, a key it already carries takes the new
//! value and keeps its place, so the stored list holds at most one entry per key
//! — it is checked against the resource's current `ruleRevision`, it does not
//! advance that revision (the operation is modelled `@idempotent`), and it
//! writes nothing when it refuses.
//!
//! Every case drives the service over the wire through the same router the
//! emulator binary mounts (the harness lives in
//! [`super::automation_rule_lifecycle`]), so a response that is not computed
//! from persisted state fails here.

use serde_json::{json, Value};

use super::automation_rule_lifecycle::{
    assert_refused, call, call_ok, emulator, member, minimal_rule, string_member,
    unenrolled_emulator_with_engine,
};

/// Create a rule carrying `tags` and answer `(ruleArn, ruleRevision)`.
async fn rule_with_tags(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
    tags: &Value,
    case: &str,
) -> (String, i64) {
    let mut request = minimal_rule(name);
    // A row whose rule starts untagged sends no `tags` member at all, which is
    // the state a plain create leaves.
    if !tags.as_array().is_some_and(Vec::is_empty) {
        request["tags"] = tags.clone();
    }
    let created = call_ok(router, "CreateAutomationRule", &request, case).await;
    (
        string_member(&created, "ruleArn"),
        member(&created, "ruleRevision")
            .as_i64()
            .expect("the created rule carries a revision"),
    )
}

/// The tag list `ListTagsForResource` reports for `rule_arn`.
async fn listed_tags(
    router: &smithy_server_runtime::MultiServiceRouter,
    rule_arn: &str,
    case: &str,
) -> Value {
    let body = call_ok(
        router,
        "ListTagsForResource",
        &json!({"resourceArn": rule_arn}),
        case,
    )
    .await;
    member(&body, "tags").clone()
}

/// The `ruleRevision` `GetAutomationRule` reports for `rule_arn`.
async fn stored_revision(
    router: &smithy_server_runtime::MultiServiceRouter,
    rule_arn: &str,
    case: &str,
) -> i64 {
    let rule = call_ok(router, "GetAutomationRule", &json!({"ruleArn": rule_arn}), case).await;
    member(&rule, "ruleRevision")
        .as_i64()
        .expect("a stored rule carries a revision")
}

/// `count` distinct tags, all inside the modelled `Tag` constraints.
fn tag_series(count: usize, prefix: &str) -> Vec<Value> {
    (0..count)
        .map(|index| json!({"key": format!("{prefix}-{index}"), "value": format!("{index}")}))
        .collect()
}

/// What a resource's tag list is after a `TagResource`, over the span the
/// modelled `Tag`/`TagList` shapes allow.
///
/// Each row differs along what the merge rule generalises over: how many tags
/// the resource starts with, how many the request carries, whether a requested
/// key is already present, the characters and lengths of the key and the value,
/// and whether one request names the same key twice. The read back goes through
/// two independent readers, so a response invented by `TagResource` rather than
/// persisted by it fails here.
#[tokio::test]
async fn tagging_merges_the_requested_tags_into_the_stored_list() {
    // Lengths and character classes are the model's:
    // `smithy_query(query="shape",shape="Tag,TagList")` — key: length 1..128,
    // pattern ^[\w\s\.\-\:\/\=\+\@]+$; value: length ..256, pattern
    // ^[\w\s\.\-\:\/\=\+\@]*$; TagList: length ..200.
    let long_key = "k".repeat(128);
    let long_value = "v".repeat(256);

    // (case, tags the rule is created with, tags the request adds, expected list)
    let cases: Vec<(&str, Value, Value, Value)> = vec![
        (
            "onto-an-untagged-rule",
            json!([]),
            json!([{"key": "team", "value": "platform"}]),
            json!([{"key": "team", "value": "platform"}]),
        ),
        (
            "a-new-key-is-appended",
            json!([{"key": "team", "value": "platform"}]),
            json!([{"key": "env", "value": "prod"}]),
            json!([
                {"key": "team", "value": "platform"},
                {"key": "env", "value": "prod"}
            ]),
        ),
        (
            "an-existing-key-takes-the-new-value-in-place",
            json!([
                {"key": "team", "value": "platform"},
                {"key": "env", "value": "staging"}
            ]),
            json!([{"key": "team", "value": "payments"}]),
            json!([
                {"key": "team", "value": "payments"},
                {"key": "env", "value": "staging"}
            ]),
        ),
        (
            "several-at-once-mixing-new-and-existing-keys",
            json!([
                {"key": "team", "value": "platform"},
                {"key": "env", "value": "staging"}
            ]),
            json!([
                {"key": "env", "value": "prod"},
                {"key": "cost-center", "value": "4711"},
                {"key": "tier", "value": "2"}
            ]),
            json!([
                {"key": "team", "value": "platform"},
                {"key": "env", "value": "prod"},
                {"key": "cost-center", "value": "4711"},
                {"key": "tier", "value": "2"}
            ]),
        ),
        (
            "the-same-key-twice-in-one-request-takes-the-last-value",
            json!([]),
            json!([
                {"key": "env", "value": "staging"},
                {"key": "env", "value": "prod"}
            ]),
            json!([{"key": "env", "value": "prod"}]),
        ),
        (
            "every-character-class-the-patterns-admit",
            json!([]),
            json!([{"key": "a_b c.d-e:f/g=h+i@9", "value": "A_B C.D-E:F/G=H+I@0"}]),
            json!([{"key": "a_b c.d-e:f/g=h+i@9", "value": "A_B C.D-E:F/G=H+I@0"}]),
        ),
        (
            "both-members-at-their-maximum-length",
            json!([]),
            json!([{"key": long_key, "value": long_value}]),
            json!([{"key": long_key, "value": long_value}]),
        ),
        (
            "an-empty-value-is-a-value",
            json!([{"key": "unset", "value": "set"}]),
            json!([{"key": "unset", "value": ""}]),
            json!([{"key": "unset", "value": ""}]),
        ),
        (
            "a-request-naming-no-new-key-changes-nothing",
            json!([{"key": "team", "value": "platform"}]),
            json!([{"key": "team", "value": "platform"}]),
            json!([{"key": "team", "value": "platform"}]),
        ),
        (
            "filling-the-resource-to-the-tag-list-maximum-of-200",
            Value::Array(tag_series(199, "key")),
            json!([{"key": "last", "value": "200"}]),
            Value::Array(
                tag_series(199, "key")
                    .into_iter()
                    .chain(std::iter::once(json!({"key": "last", "value": "200"})))
                    .collect(),
            ),
        ),
        (
            "replacing-a-value-on-a-resource-already-at-the-maximum",
            Value::Array(tag_series(200, "key")),
            json!([{"key": "key-7", "value": "replaced"}]),
            Value::Array(
                tag_series(200, "key")
                    .into_iter()
                    .map(|tag| {
                        if string_member(&tag, "key") == "key-7" {
                            json!({"key": "key-7", "value": "replaced"})
                        } else {
                            tag
                        }
                    })
                    .collect(),
            ),
        ),
    ];

    for (case, created, added, expected) in &cases {
        let router = emulator();
        let (rule_arn, revision) = rule_with_tags(&router, "tagged", created, case).await;

        call_ok(
            &router,
            "TagResource",
            &json!({"resourceArn": &rule_arn, "ruleRevision": revision, "tags": added}),
            case,
        )
        .await;

        let listed = listed_tags(&router, &rule_arn, case).await;
        assert_eq!(
            &listed, expected,
            "case {case:?}: expected the merged tag list, got {listed}"
        );

        // The same stored list, through the other reader: one list, two readers.
        let fetched = call_ok(
            &router,
            "GetAutomationRule",
            &json!({"ruleArn": &rule_arn}),
            case,
        )
        .await;
        assert_eq!(
            member(&fetched, "tags"),
            &listed,
            "case {case:?}: ListTagsForResource and GetAutomationRule must agree, got {fetched}"
        );

        // Tagging is not a rule revision, so the same request stays valid.
        let now = stored_revision(&router, &rule_arn, case).await;
        assert_eq!(
            now, revision,
            "case {case:?}: tagging must not advance the revision, got {now}"
        );

        // Repeating the request leaves the same list: the merge is idempotent.
        call_ok(
            &router,
            "TagResource",
            &json!({"resourceArn": &rule_arn, "ruleRevision": revision, "tags": added}),
            case,
        )
        .await;
        let again = listed_tags(&router, &rule_arn, case).await;
        assert_eq!(
            &again, expected,
            "case {case:?}: a repeated tagging must leave the same list, got {again}"
        );
    }
}

/// A `ruleRevision` other than the resource's current one is refused with the
/// modelled `InvalidParameterValueException` (400) stating the current revision,
/// and adds nothing — whatever the request's tags are. After a real update moves
/// the revision, the OLD revision stops working and the new one works.
#[tokio::test]
async fn tagging_is_checked_against_the_resources_current_revision() {
    let router = emulator();
    let initial = json!([{"key": "keep", "value": "me"}]);
    let (rule_arn, revision) = rule_with_tags(&router, "tag-revision", &initial, "setup").await;

    for (case, sent) in [
        ("stale", revision - 1),
        ("ahead", revision + 1),
        ("far-ahead", revision + 1_000),
        ("zero", 0),
        ("negative", -1),
    ] {
        let (status, body) = call(
            &router,
            "TagResource",
            &json!({
                "resourceArn": &rule_arn,
                "ruleRevision": sent,
                "tags": [{"key": "team", "value": "platform"}]
            }),
        )
        .await;
        assert_refused(
            (status, body.clone()),
            400,
            "InvalidParameterValueException",
            case,
        );
        assert!(
            string_member(&body, "message").contains(&revision.to_string()),
            "case {case:?}: the rejection must state the current revision {revision}, got {body}"
        );
        let listed = listed_tags(&router, &rule_arn, case).await;
        assert_eq!(
            listed, initial,
            "case {case:?}: a refused tagging must add nothing, got {listed}"
        );
    }

    // An update DOES advance the revision, so the revision a tagging must carry
    // moves with it.
    let updated = call_ok(
        &router,
        "UpdateAutomationRule",
        &json!({"ruleArn": &rule_arn, "ruleRevision": revision, "status": "Inactive"}),
        "update",
    )
    .await;
    let next = member(&updated, "ruleRevision")
        .as_i64()
        .expect("the updated rule carries a revision");
    assert_ne!(
        next, revision,
        "case \"update\": an update must advance the revision, got {next}"
    );

    assert_refused(
        call(
            &router,
            "TagResource",
            &json!({
                "resourceArn": &rule_arn,
                "ruleRevision": revision,
                "tags": [{"key": "team", "value": "platform"}]
            }),
        )
        .await,
        400,
        "InvalidParameterValueException",
        "revision-before-the-update",
    );
    call_ok(
        &router,
        "TagResource",
        &json!({
            "resourceArn": &rule_arn,
            "ruleRevision": next,
            "tags": [{"key": "team", "value": "platform"}]
        }),
        "revision-after-the-update",
    )
    .await;
    let listed = listed_tags(&router, &rule_arn, "revision-after-the-update").await;
    assert_eq!(
        listed,
        json!([{"key": "keep", "value": "me"}, {"key": "team", "value": "platform"}]),
        "case \"revision-after-the-update\": expected the tag added, got {listed}"
    );
}

/// An addition that would leave the resource holding more tags than a `TagList`
/// may carry is refused with the modelled `InvalidParameterValueException` (400)
/// and adds nothing; one that stops at the bound succeeds. The bound is the
/// modelled `TagList` `@length(max: 200)`, which the create path's `tags` member
/// documents as "Maximum of 200 tags allowed."
#[tokio::test]
async fn tagging_past_the_per_resource_tag_limit_is_refused() {
    // (case, tags already on the rule, tags added, is the request accepted)
    let cases: Vec<(&str, usize, usize, bool)> = vec![
        ("one-below-the-bound", 198, 1, true),
        ("exactly-at-the-bound", 199, 1, true),
        ("a-full-resource-takes-no-new-key", 200, 1, false),
        ("one-past-the-bound", 199, 2, false),
        ("far-past-the-bound", 150, 100, false),
    ];

    for (case, existing, added, accepted) in &cases {
        let router = emulator();
        let created = Value::Array(tag_series(*existing, "key"));
        let (rule_arn, revision) = rule_with_tags(&router, "tag-limit", &created, case).await;
        let request = json!({
            "resourceArn": &rule_arn,
            "ruleRevision": revision,
            "tags": Value::Array(tag_series(*added, "extra"))
        });

        if *accepted {
            call_ok(&router, "TagResource", &request, case).await;
            let listed = listed_tags(&router, &rule_arn, case).await;
            let count = listed.as_array().map(Vec::len).unwrap_or_default();
            assert_eq!(
                count,
                existing + added,
                "case {case:?}: expected every tag stored, got {count}"
            );
        } else {
            assert_refused(
                call(&router, "TagResource", &request).await,
                400,
                "InvalidParameterValueException",
                case,
            );
            let listed = listed_tags(&router, &rule_arn, case).await;
            assert_eq!(
                &listed, &created,
                "case {case:?}: a refused tagging must add nothing, got {listed}"
            );
        }
    }
}

/// A tag whose key or value is outside the modelled `Tag` constraints is
/// refused with `InvalidParameterValueException` (400) and adds nothing, and a
/// request missing a required member is refused the same way — never with a 501,
/// because the operation is implemented.
#[tokio::test]
async fn a_tag_outside_the_modelled_constraints_is_refused() {
    let router = emulator();
    let initial = json!([{"key": "keep", "value": "me"}]);
    let (rule_arn, revision) = rule_with_tags(&router, "tag-invalid", &initial, "setup").await;

    // Keys and values the model's `Tag` REFUSES: a character outside
    // ^[\w\s\.\-\:\/\=\+\@]+$, and either side of the length bounds
    // (`smithy_query(query="shape",shape="Tag")` — key 1..128, value ..256).
    let cases: Vec<(&str, Value)> = vec![
        ("key-with-an-exclamation", json!([{"key": "bad!", "value": "v"}])),
        ("key-with-a-hash", json!([{"key": "bad#key", "value": "v"}])),
        ("key-with-a-comma", json!([{"key": "comma,key", "value": "v"}])),
        ("empty-key", json!([{"key": "", "value": "v"}])),
        (
            "key-past-its-maximum-length",
            json!([{"key": "k".repeat(129), "value": "v"}]),
        ),
        ("value-with-a-hash", json!([{"key": "k", "value": "bad#value"}])),
        (
            "value-past-its-maximum-length",
            json!([{"key": "k", "value": "v".repeat(257)}]),
        ),
        (
            "a-bad-key-beside-a-good-one",
            json!([{"key": "fine", "value": "v"}, {"key": "bad!", "value": "v"}]),
        ),
    ];

    for (case, tags) in &cases {
        assert_refused(
            call(
                &router,
                "TagResource",
                &json!({"resourceArn": &rule_arn, "ruleRevision": revision, "tags": tags}),
            )
            .await,
            400,
            "InvalidParameterValueException",
            case,
        );
        let listed = listed_tags(&router, &rule_arn, case).await;
        assert_eq!(
            &listed, &initial,
            "case {case:?}: a refused tagging must add nothing, got {listed}"
        );
    }

    // Each required member, absent in turn
    // (`smithy_query(query="operation",operation="TagResource")`).
    let incomplete: Vec<(&str, Value)> = vec![
        (
            "no-resource-arn",
            json!({"ruleRevision": revision, "tags": [{"key": "k", "value": "v"}]}),
        ),
        (
            "no-rule-revision",
            json!({"resourceArn": &rule_arn, "tags": [{"key": "k", "value": "v"}]}),
        ),
        (
            "no-tags",
            json!({"resourceArn": &rule_arn, "ruleRevision": revision}),
        ),
    ];
    for (case, body) in &incomplete {
        assert_refused(
            call(&router, "TagResource", body).await,
            400,
            "InvalidParameterValueException",
            case,
        );
    }
}

/// A well-formed rule ARN no rule has is the modelled
/// `ResourceNotFoundException` (404), and an account that is not opted in is
/// refused `OptInRequiredException` (403) before the ARN is resolved at all.
#[tokio::test]
async fn tagging_an_unknown_resource_or_an_unenrolled_account_is_refused() {
    let router = emulator();
    let unknown = format!(
        "arn:aws:compute-optimizer::{}:automation-rule/0000000000000000",
        engine::EMULATOR_ACCOUNT_ID
    );
    // A rule that existed and was deleted, so a well-formed ARN of this account
    // stops resolving.
    let (doomed_arn, doomed_revision) =
        rule_with_tags(&router, "doomed", &json!([]), "setup").await;
    call_ok(
        &router,
        "DeleteAutomationRule",
        &json!({"ruleArn": &doomed_arn, "ruleRevision": doomed_revision}),
        "delete",
    )
    .await;

    for (case, resource_arn) in [
        ("never-created", unknown.clone()),
        ("deleted", doomed_arn),
        (
            "another-accounts-rule",
            "arn:aws:compute-optimizer::210987654321:automation-rule/aaaaaaaaaaaaaaaa".to_string(),
        ),
    ] {
        assert_refused(
            call(
                &router,
                "TagResource",
                &json!({
                    "resourceArn": resource_arn,
                    "ruleRevision": 1,
                    "tags": [{"key": "team", "value": "platform"}]
                }),
            )
            .await,
            404,
            "ResourceNotFoundException",
            case,
        );
    }

    // The unenrolled account: the refusal is the enrollment one, and it comes
    // before resolving the resource — the same answer for a rule that exists and
    // for one that does not.
    let (unenrolled, _engine) = unenrolled_emulator_with_engine();
    for (case, resource_arn) in [
        (
            "unenrolled-known-shaped-arn",
            format!(
                "arn:aws:compute-optimizer::{}:automation-rule/abcdefghij123456",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
        ("unenrolled-unknown-arn", unknown),
    ] {
        assert_refused(
            call(
                &unenrolled,
                "TagResource",
                &json!({
                    "resourceArn": resource_arn,
                    "ruleRevision": 1,
                    "tags": [{"key": "team", "value": "platform"}]
                }),
            )
            .await,
            403,
            "OptInRequiredException",
            case,
        );
    }
}

/// The `@idempotencyToken` contract: the same token with the same parameters is
/// a replay that adds nothing a second time, the same token with different
/// parameters is `IdempotentParameterMismatchException` (409), and a token
/// already used by another operation is `IdempotencyTokenInUseException` (409).
/// A refused call does not consume its token.
#[tokio::test]
async fn the_client_token_governs_a_repeated_tagging() {
    let router = emulator();
    let (rule_arn, revision) = rule_with_tags(&router, "tag-token", &json!([]), "setup").await;

    let request = json!({
        "resourceArn": &rule_arn,
        "ruleRevision": revision,
        "tags": [{"key": "team", "value": "platform"}],
        "clientToken": "tag-token-1"
    });
    call_ok(&router, "TagResource", &request, "first").await;
    // The replay: same token, same parameters. It answers as the first call did
    // and the stored list is still the one the first call left.
    call_ok(&router, "TagResource", &request, "replay").await;
    let listed = listed_tags(&router, &rule_arn, "replay").await;
    assert_eq!(
        listed,
        json!([{"key": "team", "value": "platform"}]),
        "case \"replay\": a replay must leave the first call's list, got {listed}"
    );

    // The same token with other parameters, each differing in one member.
    let mismatches: Vec<(&str, Value)> = vec![
        (
            "other-tags",
            json!({
                "resourceArn": &rule_arn,
                "ruleRevision": revision,
                "tags": [{"key": "env", "value": "prod"}],
                "clientToken": "tag-token-1"
            }),
        ),
        (
            "other-value-for-the-same-key",
            json!({
                "resourceArn": &rule_arn,
                "ruleRevision": revision,
                "tags": [{"key": "team", "value": "payments"}],
                "clientToken": "tag-token-1"
            }),
        ),
        (
            "other-revision",
            json!({
                "resourceArn": &rule_arn,
                "ruleRevision": revision + 1,
                "tags": [{"key": "team", "value": "platform"}],
                "clientToken": "tag-token-1"
            }),
        ),
    ];
    for (case, body) in &mismatches {
        assert_refused(
            call(&router, "TagResource", body).await,
            409,
            "IdempotentParameterMismatchException",
            case,
        );
    }

    // A token another operation already used is in use, whichever operation it
    // was.
    call_ok(
        &router,
        "UntagResource",
        &json!({
            "resourceArn": &rule_arn,
            "ruleRevision": revision,
            "tagKeys": ["absent"],
            "clientToken": "untag-token-1"
        }),
        "untag-first",
    )
    .await;
    assert_refused(
        call(
            &router,
            "TagResource",
            &json!({
                "resourceArn": &rule_arn,
                "ruleRevision": revision,
                "tags": [{"key": "team", "value": "platform"}],
                "clientToken": "untag-token-1"
            }),
        )
        .await,
        409,
        "IdempotencyTokenInUseException",
        "token-of-another-operation",
    );

    // A token a REFUSED call carried is not consumed: the same token still works
    // for the request it belongs to.
    let refused_token = "tag-token-2";
    assert_refused(
        call(
            &router,
            "TagResource",
            &json!({
                "resourceArn": &rule_arn,
                "ruleRevision": revision + 5,
                "tags": [{"key": "env", "value": "prod"}],
                "clientToken": refused_token
            }),
        )
        .await,
        400,
        "InvalidParameterValueException",
        "refused-call",
    );
    call_ok(
        &router,
        "TagResource",
        &json!({
            "resourceArn": &rule_arn,
            "ruleRevision": revision,
            "tags": [{"key": "env", "value": "prod"}],
            "clientToken": refused_token
        }),
        "token-after-a-refusal",
    )
    .await;
    let listed = listed_tags(&router, &rule_arn, "token-after-a-refusal").await;
    assert_eq!(
        listed,
        json!([{"key": "team", "value": "platform"}, {"key": "env", "value": "prod"}]),
        "case \"token-after-a-refusal\": expected both tags stored, got {listed}"
    );
}

/// Tags belong to the resource the ARN names: tagging one resource leaves every
/// other resource's list alone.
#[tokio::test]
async fn tagging_one_resource_leaves_the_others_alone() {
    let router = emulator();

    let mut rules = Vec::new();
    for index in 0..3 {
        let created = json!([{"key": "owner", "value": format!("owner-{index}")}]);
        let (arn, revision) =
            rule_with_tags(&router, &format!("per-resource-{index}"), &created, "setup").await;
        rules.push((arn, revision, created));
    }

    let (target_arn, target_revision, _) = rules[1].clone();
    call_ok(
        &router,
        "TagResource",
        &json!({
            "resourceArn": &target_arn,
            "ruleRevision": target_revision,
            "tags": [{"key": "owner", "value": "new-owner"}, {"key": "tier", "value": "1"}]
        }),
        "tag-the-second",
    )
    .await;

    for (index, (arn, _, created)) in rules.iter().enumerate() {
        let case = format!("resource-{index}");
        let listed = listed_tags(&router, arn, &case).await;
        let expected = if arn == &target_arn {
            json!([{"key": "owner", "value": "new-owner"}, {"key": "tier", "value": "1"}])
        } else {
            created.clone()
        };
        assert_eq!(
            listed, expected,
            "case {case:?}: expected only the tagged resource to change, got {listed}"
        );
    }
}
