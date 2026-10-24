// SPDX-License-Identifier: CC0-1.0

//! `UntagResource`, end to end over the wire: what deleting tags from a feed or a
//! dictionary leaves behind, and what it refuses.
//!
//! Every case drives the real request path — the router picks the service from the SigV4
//! credential scope, the framework deserializes and validates the request, the handler
//! runs, and the response is serialized as `restJson1` — so what these tests assert is
//! what a client sees. `resourceArn` is an `@httpLabel` and `tagKeys` an `@httpQuery`
//! LIST, so each request carries the ARN percent-encoded in the path and one `tagKeys`
//! parameter per key, exactly as a client sends them ([`untag_resource`]).
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * a tag delete is KEY-WISE and partial: each key the request names is removed from the
//!   tag set stored on the resource and every other key is left as it was, so the next
//!   read of the resource reports the remainder — the tags live on the one record
//!   `GetFeed` / `GetDictionary` report. A key the resource does not carry is nothing to
//!   remove, so the request still succeeds, which is also why the same request twice
//!   leaves the same set (the operation is `@idempotent`);
//! * a `resourceArn` identifies a resource WHOLE — kind, id, region and account — so any
//!   other well-formed ARN names no resource of this store and earns the modelled
//!   `ResourceNotFoundException` (HTTP 404) and no write;
//! * a resource whose deletion has been accepted takes no mutation: the delete is refused
//!   with the modelled `ConflictException` (HTTP 409) while the deletion is pending and
//!   with `ResourceNotFoundException` once it has completed;
//! * a key in the namespace AWS reserves for its own tags is refused with the modelled
//!   `ValidationException` (HTTP 400) and removes nothing, since a tag under that prefix
//!   may be neither set nor deleted by a request;
//! * a key outside the `@length` bounds `TagKeyList`'s member models earns that same
//!   `ValidationException` and removes nothing — here by the framework's boundary walk,
//!   which reaches a LIST's elements as constrained leaves.

use crate::harness::{
    call, create_tagged_resource, expected_tags, pair, past_the_transition_span, percent_encode,
    read_resource, router, tags_of, tags_path, unresolvable_arns, TAGGABLE_KINDS,
};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::{
    reserved_tag_key_message, NOT_FOUND_MESSAGE, RESERVED_TAG_KEY_PREFIX, TAG_KEY_MAX_LENGTH,
};
use smithy_server_runtime::MultiServiceRouter;

/// `DELETE /v1/tags/{resourceArn}?tagKeys=…`.
///
/// `tagKeys` is an `@httpQuery` bound to a list, so a client repeats the parameter once
/// per key, each value percent-encoded.
async fn untag_resource(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tag_keys: &[String],
) -> (u16, Value) {
    assert!(
        !tag_keys.is_empty(),
        "tagKeys is @required, so every request under test names at least one key"
    );
    let query = tag_keys
        .iter()
        .map(|key| format!("tagKeys={}", percent_encode(key)))
        .collect::<Vec<String>>()
        .join("&");
    let uri = format!("{}?{query}", tags_path(resource_arn));
    call(router, "DELETE", &uri, None).await
}

/// The keys of a tag set, for a row that deletes every key a resource holds.
fn keys_of(tags: &[(String, String)]) -> Vec<String> {
    tags.iter().map(|(key, _)| key.clone()).collect()
}

/// `count` distinct ordinary tags, for the rows about HOW MANY keys a request names.
fn many_tags(count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|index| pair(&format!("key{index}"), &format!("value{index}")))
        .collect()
}

/// One row of the delete table: what the resource carried, which keys the request names,
/// and the set the delete must leave behind.
struct DeleteCase {
    label: &'static str,
    initial: Vec<(String, String)>,
    request: Vec<String>,
    expected: Vec<(String, String)>,
}

/// Rows spanning what the delete rule generalises over: how many tags the resource held,
/// how many keys the request names, whether those keys are present or absent, whether one
/// is named twice, and the shape of the key itself.
///
/// The key lengths come from the model, not from a recorded request: `TagKeyList`'s member
/// is a `TagKey` of `@length` 1..128
/// (`smithy_query(query="shape",shape="TagKeyList")`), so a row sits on its maximum.
fn delete_cases() -> Vec<DeleteCase> {
    let longest_key = "k".repeat(TAG_KEY_MAX_LENGTH);
    let many = many_tags(10);
    vec![
        DeleteCase {
            label: "the one key a resource holds leaves it untagged",
            initial: vec![pair("env", "prod")],
            request: vec!["env".to_string()],
            expected: vec![],
        },
        DeleteCase {
            label: "one key of several goes and the rest are left alone",
            initial: vec![pair("env", "dev"), pair("team", "video"), pair("owner", "ops")],
            request: vec!["team".to_string()],
            expected: vec![pair("env", "dev"), pair("owner", "ops")],
        },
        DeleteCase {
            label: "several keys at once",
            initial: vec![pair("a", "1"), pair("b", "2"), pair("c", "3"), pair("d", "4")],
            request: vec!["a".to_string(), "c".to_string()],
            expected: vec![pair("b", "2"), pair("d", "4")],
        },
        DeleteCase {
            label: "every key the resource holds",
            initial: many.clone(),
            request: keys_of(&many),
            expected: vec![],
        },
        DeleteCase {
            label: "half the keys of a larger set",
            initial: many.clone(),
            request: keys_of(&many[..5]),
            expected: many[5..].to_vec(),
        },
        DeleteCase {
            label: "a key the resource does not carry is nothing to remove",
            initial: vec![pair("env", "dev")],
            request: vec!["absent".to_string()],
            expected: vec![pair("env", "dev")],
        },
        DeleteCase {
            label: "some keys the resource holds and some it does not",
            initial: vec![pair("env", "dev"), pair("team", "video")],
            request: vec!["env".to_string(), "nosuchkey".to_string()],
            expected: vec![pair("team", "video")],
        },
        DeleteCase {
            label: "a key named twice is removed once",
            initial: vec![pair("env", "dev"), pair("team", "video")],
            request: vec!["env".to_string(), "env".to_string()],
            expected: vec![pair("team", "video")],
        },
        DeleteCase {
            label: "only the named key goes, not one that merely begins with it",
            initial: vec![pair("env", "dev"), pair("environment", "prod")],
            request: vec!["env".to_string()],
            expected: vec![pair("environment", "prod")],
        },
        DeleteCase {
            label: "a tag key is case-sensitive, so another case names another key",
            initial: vec![pair("Env", "dev")],
            request: vec!["env".to_string()],
            expected: vec![pair("Env", "dev")],
        },
        DeleteCase {
            label: "a key whose value is empty still goes",
            initial: vec![pair("blank", ""), pair("env", "dev")],
            request: vec!["blank".to_string()],
            expected: vec![pair("env", "dev")],
        },
        DeleteCase {
            label: "the longest key the model admits",
            initial: vec![(longest_key.clone(), "v".to_string()), pair("env", "dev")],
            request: vec![longest_key],
            expected: vec![pair("env", "dev")],
        },
        DeleteCase {
            label: "a punctuated, non-ASCII key the wire has to carry encoded",
            initial: vec![pair("項目:alpha.1_x", "München"), pair("env", "dev")],
            request: vec!["項目:alpha.1_x".to_string()],
            expected: vec![pair("env", "dev")],
        },
        DeleteCase {
            label: "a key holding the reserved prefix further along, not at its start",
            initial: vec![pair("team:aws:thing", "mine"), pair("env", "dev")],
            request: vec!["team:aws:thing".to_string()],
            expected: vec![pair("env", "dev")],
        },
    ]
}

/// A tag delete removes exactly the keys it names, leaves the rest alone, shows up in the
/// next read of the resource, and is idempotent — on either kind of resource.
#[tokio::test]
async fn named_keys_are_deleted_and_the_rest_of_the_set_is_left_alone() {
    let router = router();

    for kind in TAGGABLE_KINDS {
        for (index, case) in delete_cases().into_iter().enumerate() {
            let label = format!("{} on a {kind}", case.label);
            let (id, arn) =
                create_tagged_resource(&router, kind, &format!("untag{kind}{index}"), &case.initial)
                    .await;

            let (status, body) = untag_resource(&router, &arn, &case.request).await;
            assert_eq!(
                status, 204,
                "case {label:?}: UntagResource answers its modelled 204, got {status} with {body}"
            );
            assert_eq!(
                body,
                Value::Null,
                "case {label:?}: a 204 carries no body, got {body}"
            );

            let (status, stored) = read_resource(&router, kind, &id).await;
            assert_eq!(
                status, 200,
                "case {label:?}: the untagged {kind} still reads back, got {status} with {stored}"
            );
            assert_eq!(
                tags_of(&stored),
                expected_tags(&case.expected),
                "case {label:?}: the remaining tag set is what the next read reports, got {stored}"
            );

            // `@idempotent`: the same request again leaves the same set, so a key already
            // gone is not an error the second time either.
            let (status, body) = untag_resource(&router, &arn, &case.request).await;
            assert_eq!(
                status, 204,
                "case {label:?}: repeating the delete answers 204, got {status} with {body}"
            );
            let (_, repeated) = read_resource(&router, kind, &id).await;
            assert_eq!(
                tags_of(&repeated),
                expected_tags(&case.expected),
                "case {label:?}: repeating the delete changes nothing, got {repeated}"
            );
        }
    }
}

/// A `resourceArn` that is not the ARN a stored resource carries names nothing, and no
/// tag of the real resource is touched.
#[tokio::test]
async fn an_arn_that_names_no_stored_resource_is_not_found() {
    let router = router();

    for kind in TAGGABLE_KINDS {
        let initial = vec![pair("env", "dev"), pair("team", "video")];
        let (id, arn) =
            create_tagged_resource(&router, kind, &format!("untagmiss{kind}"), &initial).await;

        for case in unresolvable_arns(kind, &arn) {
            let label = format!("{} ({kind})", case.label);
            let (status, error) = untag_resource(&router, &case.arn, &["env".to_string()]).await;
            assert_eq!(
                status, 404,
                "case {label:?}: an ARN naming no stored resource is refused 404, got {status} \
                 with {error}"
            );
            assert_eq!(
                error["__type"],
                json!("ResourceNotFoundException"),
                "case {label:?}: the refusal is the modelled ResourceNotFoundException, got \
                 {error}"
            );
            assert_eq!(
                error["message"],
                json!(NOT_FOUND_MESSAGE),
                "case {label:?}: the refusal carries this service's one not-found message, got \
                 {error}"
            );

            let (_, stored) = read_resource(&router, kind, &id).await;
            assert_eq!(
                tags_of(&stored),
                expected_tags(&initial),
                "case {label:?}: a refused delete leaves the real resource untouched, got {stored}"
            );
        }
    }
}

/// A resource whose deletion has been accepted takes no tag delete: a conflict while the
/// removal is pending, and not found once it has fallen due.
#[tokio::test]
async fn a_resource_being_deleted_takes_no_tag_delete() {
    let router = router();

    for kind in TAGGABLE_KINDS {
        let initial = vec![pair("env", "dev")];
        let (id, arn) =
            create_tagged_resource(&router, kind, &format!("untaggone{kind}"), &initial).await;
        let (status, deleted) = call(&router, "DELETE", &format!("/v1/{kind}/{id}"), None).await;
        assert_eq!(
            status, 202,
            "case {kind:?}: the deletion is accepted with 202, got {status} with {deleted}"
        );

        let (status, error) = untag_resource(&router, &arn, &["env".to_string()]).await;
        assert_eq!(
            status, 409,
            "case {kind:?}: untagging a resource whose deletion is pending conflicts, got \
             {status} with {error}"
        );
        assert_eq!(
            error["__type"],
            json!("ConflictException"),
            "case {kind:?}: the refusal is the modelled ConflictException, got {error}"
        );

        let (_, pending) = read_resource(&router, kind, &id).await;
        assert_eq!(
            tags_of(&pending),
            expected_tags(&initial),
            "case {kind:?}: the refused delete left the tag set alone, got {pending}"
        );

        // Once the removal falls due the resource is gone, so its ARN names nothing.
        past_the_transition_span();
        let (status, error) = untag_resource(&router, &arn, &["env".to_string()]).await;
        assert_eq!(
            (status, &error["__type"]),
            (404, &json!("ResourceNotFoundException")),
            "case {kind:?}: a removed resource's ARN names nothing, got {status} with {error}"
        );
    }
}

/// One row of the refused-key table: the keys a request names, the stored key none of
/// them may remove, and the message where the rule that produces it is this service's own.
struct RefusedKeyCase {
    label: &'static str,
    request: Vec<String>,
    message: Option<String>,
}

/// Rows spanning the keys this service refuses to act on: a key in the namespace AWS
/// reserves for its own tags ([`RESERVED_TAG_KEY_PREFIX`]), wherever it sits among the
/// request's keys, and a key outside the `@length` bounds `TagKeyList`'s member models
/// (1..128, `smithy_query(query="shape",shape="TagKeyList")`) — so the length rows sit on
/// the published maximum's far side and on the minimum's.
///
/// A reserved key's refusal is this service's own sentence, so those rows pin the message;
/// a key outside the modelled bounds is refused by the framework's boundary walk, whose
/// envelope is the framework's, so those rows pin only the status and the code.
fn refused_key_cases() -> Vec<RefusedKeyCase> {
    let reserved = RESERVED_TAG_KEY_PREFIX;
    vec![
        RefusedKeyCase {
            label: "a key in the reserved namespace",
            request: vec![format!("{reserved}cloudformation:stack-name")],
            message: Some(reserved_tag_key_message(&format!(
                "{reserved}cloudformation:stack-name"
            ))),
        },
        RefusedKeyCase {
            label: "the bare reserved prefix as the whole key",
            request: vec![reserved.to_string()],
            message: Some(reserved_tag_key_message(reserved)),
        },
        RefusedKeyCase {
            label: "one reserved key among ordinary ones",
            request: vec![
                "env".to_string(),
                format!("{reserved}secretsmanager:owning-service"),
                "team".to_string(),
            ],
            message: Some(reserved_tag_key_message(&format!(
                "{reserved}secretsmanager:owning-service"
            ))),
        },
        RefusedKeyCase {
            label: "a key one character past the maximum modelled length",
            request: vec!["k".repeat(TAG_KEY_MAX_LENGTH + 1)],
            message: None,
        },
        RefusedKeyCase {
            label: "a non-ASCII key past the maximum modelled length, counted in characters",
            request: vec!["東".repeat(TAG_KEY_MAX_LENGTH + 1)],
            message: None,
        },
        RefusedKeyCase {
            label: "an empty key, one character short of the minimum modelled length",
            request: vec![String::new()],
            message: None,
        },
    ]
}

/// A key this service will not act on is refused with the modelled `ValidationException`,
/// and the resource's whole tag set survives.
#[tokio::test]
async fn a_key_the_service_refuses_removes_nothing() {
    let router = router();

    for kind in TAGGABLE_KINDS {
        for (index, case) in refused_key_cases().into_iter().enumerate() {
            let label = format!("{} on a {kind}", case.label);
            // The stored set holds the ordinary keys some rows name alongside a refused
            // one, so "nothing was removed" is about keys the request really did name.
            let initial = vec![pair("env", "dev"), pair("team", "video")];
            let (id, arn) = create_tagged_resource(
                &router,
                kind,
                &format!("untagbad{kind}{index}"),
                &initial,
            )
            .await;

            let (status, error) = untag_resource(&router, &arn, &case.request).await;
            assert_eq!(
                status, 400,
                "case {label:?}: a key this service will not act on is refused 400, got {status} \
                 with {error}"
            );
            assert_eq!(
                error["__type"],
                json!("ValidationException"),
                "case {label:?}: that 400 is the modelled ValidationException, got {error}"
            );
            if let Some(message) = &case.message {
                assert_eq!(
                    error["message"],
                    json!(message),
                    "case {label:?}: expected {message:?}, got {error}"
                );
            }

            let (_, stored) = read_resource(&router, kind, &id).await;
            assert_eq!(
                tags_of(&stored),
                expected_tags(&initial),
                "case {label:?}: a refused delete removes nothing, got {stored}"
            );
        }
    }
}

/// The request's own validity is decided BEFORE any resource is looked up, so a key this
/// service will not act on is refused whatever the ARN names — the same precedence
/// `TagResource` applies to a supplied tag map.
#[tokio::test]
async fn a_refused_key_is_decided_before_the_resource_is_resolved() {
    let router = router();
    let reserved = format!("{RESERVED_TAG_KEY_PREFIX}cloudformation:stack-name");

    for kind in TAGGABLE_KINDS {
        let (_, arn) = create_tagged_resource(
            &router,
            kind,
            &format!("untagorder{kind}"),
            &[pair("env", "dev")],
        )
        .await;

        for case in unresolvable_arns(kind, &arn) {
            let label = format!("{} ({kind})", case.label);
            let (status, error) =
                untag_resource(&router, &case.arn, std::slice::from_ref(&reserved)).await;
            assert_eq!(
                (status, &error["__type"]),
                (400, &json!("ValidationException")),
                "case {label:?}: the reserved key is refused before the ARN is resolved, got \
                 {status} with {error}"
            );
            assert_eq!(
                error["message"],
                json!(reserved_tag_key_message(&reserved)),
                "case {label:?}: that refusal is the reserved-namespace one, got {error}"
            );
        }
    }
}
