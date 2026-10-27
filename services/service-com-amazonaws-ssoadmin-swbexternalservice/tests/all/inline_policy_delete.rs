// SPDX-License-Identifier: CC0-1.0

//! Deleting the INLINE POLICY of a permission set, end to end over the `awsJson1_1` wire.
//!
//! THE RULE under test: `DeleteInlinePolicyFromPermissionSet` drops the inline-policy
//! document held against the (instance, permission set) pair the request names, so the
//! reader of it answers with `InlinePolicy` absent afterwards and a later put stores a
//! document again; nothing else of the permission set's record moves with it, no OTHER
//! permission set is touched, and the call is IDEMPOTENT — a permission set holding no
//! document is left holding none and answered the same empty 200, because the inline
//! policy is an optional property of the permission set rather than a resource of its own
//! (`GetInlinePolicyForPermissionSet` reports its absence with a 200). An `InstanceArn` or
//! `PermissionSetArn` nothing is held under is a `ResourceNotFoundException`, and the
//! permission-set surface is withdrawn on an account instance.
//!
//! The rules are asserted over the TABLE of documents `inline_policy.rs` owns, whose rows
//! differ along what the rule generalises over: the SIZE of the deleted document (the
//! published length bounds of `PermissionSetPolicyDocument`, `length(min: 1, max: 32768)`),
//! the characters it is built from (ASCII, the tab / newline / carriage return its pattern
//! singles out, and the top of its `\u{0020}-\u{00FF}` range) and the number of statements
//! it carries — plus rows of this suite's own that differ along the ORDER of the calls (a
//! delete with no put before it, a delete after a put, repeated deletes, a put after a
//! delete). The member's constraints and this operation's errors are the model's own
//! (`smithy_query(query="operation",operation="DeleteInlinePolicyFromPermissionSet,GetInlinePolicyForPermissionSet,PutInlinePolicyToPermissionSet",model_path="services/service-com-amazonaws-ssoadmin-swbexternalservice/smithy-model/com-amazonaws-ssoadmin-swbexternalservice.json")`).
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes, so the modelled constraints and the `awsJson1_1` envelope are exercised
//! rather than bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns, and the
//! calls run against an ORGANIZATION instance because the permission-set surface is
//! withdrawn on an account instance (asserted here too).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The document TABLE and the three request helpers are the ones `inline_policy.rs` owns —
// this suite deletes what that one puts, so it drives the same documents and the same
// requests rather than keeping a second copy of either.
use crate::inline_policy::{create_permission_set, get_policy, policy_documents, put_policy};
use crate::ssoadmin_permission_sets::{
    account_instance_router, assert_refused, call, error_code, organization_router,
    UNKNOWN_INSTANCE_ARN, UNKNOWN_PERMISSION_SET_ARN,
};

/// A delete, as `(status, body)`.
async fn delete_policy(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
) -> (u16, Value) {
    call(
        router,
        "DeleteInlinePolicyFromPermissionSet",
        &json!({
            "InstanceArn": instance_arn,
            "PermissionSetArn": permission_set_arn,
        }),
    )
    .await
}

/// THE RULE: a delete drops the stored document, so the reader answers with no inline
/// policy afterwards — over every row of the table, each on its own permission set. The
/// permission set itself survives, and a put after the delete stores a document again.
#[tokio::test]
async fn a_delete_drops_the_stored_document() {
    let (router, instance_arn) = organization_router();

    for (index, (label, document)) in policy_documents().into_iter().enumerate() {
        let permission_set_arn =
            create_permission_set(&router, &instance_arn, &format!("Deleted{index}")).await;
        let (status, body) =
            put_policy(&router, &instance_arn, &permission_set_arn, &document).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the put expected 200, got {status} {body}"
        );
        let (_, stored) = get_policy(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            stored.as_str(),
            Some(document.as_str()),
            "case {label:?}: the put must store the document first, got {stored:?}"
        );

        let (status, body) = delete_policy(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            status, 200,
            "case {label:?}: DeleteInlinePolicyFromPermissionSet expected 200, got {status} {body}"
        );

        let (status, after) = get_policy(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the get after the delete expected 200, got {status} {after}"
        );
        assert_eq!(
            after,
            Value::Null,
            "case {label:?}: the deleted document must be gone, got {after}"
        );

        // The permission set itself is NOT deleted with its policy, and it can carry a
        // document again.
        let (status, described) = call(
            &router,
            "DescribePermissionSet",
            &json!({"InstanceArn": instance_arn, "PermissionSetArn": permission_set_arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: the permission set must survive the delete, got {status} {described}"
        );

        let (status, body) =
            put_policy(&router, &instance_arn, &permission_set_arn, &document).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a put after the delete expected 200, got {status} {body}"
        );
        let (_, restored) = get_policy(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            restored.as_str(),
            Some(document.as_str()),
            "case {label:?}: a put after the delete must store the document, got {restored:?}"
        );
    }
}

/// The delete is IDEMPOTENT: a permission set holding no document — never given one, or
/// already deleted — is answered the same empty 200 and still holds none, and a put after
/// any number of deletes is reported.
#[tokio::test]
async fn repeated_deletes_answer_the_same_and_store_nothing() {
    let (router, instance_arn) = organization_router();

    // Rows differ along the ORDER of the calls: how many deletes run, and whether a put ran
    // before them at all.
    let documents = policy_documents();
    let mut rows: Vec<(String, Option<String>, usize)> = vec![
        ("no put at all, one delete".to_string(), None, 1),
        ("no put at all, three deletes".to_string(), None, 3),
    ];
    for (label, document) in &documents {
        rows.push((
            format!("{label}, deleted twice"),
            Some(document.clone()),
            2,
        ));
    }

    for (index, (label, document, deletes)) in rows.into_iter().enumerate() {
        let permission_set_arn =
            create_permission_set(&router, &instance_arn, &format!("Idempotent{index}")).await;
        if let Some(document) = &document {
            let (status, body) =
                put_policy(&router, &instance_arn, &permission_set_arn, document).await;
            assert_eq!(
                status, 200,
                "case {label:?}: the put expected 200, got {status} {body}"
            );
        }

        for round in 0..deletes {
            let (status, body) = delete_policy(&router, &instance_arn, &permission_set_arn).await;
            assert_eq!(
                status, 200,
                "case {label:?} round {round}: the delete expected 200, got {status} {body}"
            );
            assert_eq!(
                body,
                json!({}),
                "case {label:?} round {round}: the response models no members, got {body}"
            );

            let (status, after) = get_policy(&router, &instance_arn, &permission_set_arn).await;
            assert_eq!(
                status, 200,
                "case {label:?} round {round}: the get expected 200, got {status} {after}"
            );
            assert_eq!(
                after,
                Value::Null,
                "case {label:?} round {round}: no document may be held, got {after}"
            );
        }

        // Still a usable permission set after the repeated deletes.
        let restored = documents[index % documents.len()].1.clone();
        let (status, body) =
            put_policy(&router, &instance_arn, &permission_set_arn, &restored).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a put after the deletes expected 200, got {status} {body}"
        );
        let (_, stored) = get_policy(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            stored.as_str(),
            Some(restored.as_str()),
            "case {label:?}: the put after the deletes must be reported, got {stored:?}"
        );
    }
}

/// The delete touches the inline policy of ONE permission set and nothing else: the rest of
/// that permission set's record is reported unchanged, and a NEIGHBOUR of the same instance
/// keeps its own document.
#[tokio::test]
async fn a_delete_touches_only_the_inline_policy_it_names() {
    let (router, instance_arn) = organization_router();
    let documents = policy_documents();
    let (subject_label, subject_document) = documents[2].clone();
    let (neighbour_label, neighbour_document) = documents[1].clone();

    let subject = create_permission_set(&router, &instance_arn, "Subject").await;
    let neighbour = create_permission_set(&router, &instance_arn, "Neighbour").await;

    for (label, permission_set, document) in [
        (&subject_label, &subject, &subject_document),
        (&neighbour_label, &neighbour, &neighbour_document),
    ] {
        let (status, body) = put_policy(&router, &instance_arn, permission_set, document).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the put expected 200, got {status} {body}"
        );
    }

    // The rest of the subject's record, so the delete can be shown to leave it alone. The
    // members are the model's own for each operation.
    let neighbouring_state: Vec<(&str, &str, Value)> = vec![
        (
            "its mutable members",
            "UpdatePermissionSet",
            json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": subject,
                "Description": "the description the delete must not touch",
                "SessionDuration": "PT3H",
                "RelayState": "https://example.com/portal",
            }),
        ),
        (
            "a tag",
            "TagResource",
            json!({
                "InstanceArn": instance_arn,
                "ResourceArn": subject,
                "Tags": [{"Key": "Owner", "Value": "Platform"}],
            }),
        ),
        (
            "an attached managed policy",
            "AttachManagedPolicyToPermissionSet",
            json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": subject,
                "ManagedPolicyArn": "arn:aws:iam::aws:policy/ReadOnlyAccess",
            }),
        ),
        (
            "a permissions boundary",
            "PutPermissionsBoundaryToPermissionSet",
            json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": subject,
                "PermissionsBoundary": {"ManagedPolicyArn": "arn:aws:iam::aws:policy/PowerUserAccess"},
            }),
        ),
    ];
    for (label, operation, body) in &neighbouring_state {
        let (status, answer) = call(&router, operation, body).await;
        assert_eq!(
            status, 200,
            "case {label:?}: {operation} expected 200, got {status} {answer}"
        );
    }

    let (status, body) = delete_policy(&router, &instance_arn, &subject).await;
    assert_eq!(
        status, 200,
        "case {subject_label:?}: the delete expected 200, got {status} {body}"
    );

    let (_, gone) = get_policy(&router, &instance_arn, &subject).await;
    assert_eq!(
        gone,
        Value::Null,
        "case {subject_label:?}: the subject's document must be gone, got {gone}"
    );

    // The neighbour's own document is untouched by the subject's delete.
    let (status, kept) = get_policy(&router, &instance_arn, &neighbour).await;
    assert_eq!(
        status, 200,
        "case {neighbour_label:?}: the neighbour's get expected 200, got {status} {kept}"
    );
    assert_eq!(
        kept.as_str(),
        Some(neighbour_document.as_str()),
        "case {neighbour_label:?}: the neighbour must keep its own document, got {kept:?}"
    );

    // And every other part of the subject's record is still reported by its own reader.
    let (status, described) = call(
        &router,
        "DescribePermissionSet",
        &json!({"InstanceArn": instance_arn, "PermissionSetArn": subject}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribePermissionSet expected 200, got {status} {described}"
    );
    assert_eq!(
        described["PermissionSet"]["Description"].as_str(),
        Some("the description the delete must not touch"),
        "the delete must not touch the permission set's members, got {described}"
    );
    assert_eq!(
        described["PermissionSet"]["SessionDuration"].as_str(),
        Some("PT3H"),
        "the delete must not touch the permission set's members, got {described}"
    );

    let (status, tags) = call(
        &router,
        "ListTagsForResource",
        &json!({"InstanceArn": instance_arn, "ResourceArn": subject}),
    )
    .await;
    assert_eq!(status, 200, "ListTagsForResource expected 200, got {status} {tags}");
    assert!(
        tags["Tags"].as_array().is_some_and(|written| written
            .iter()
            .any(|tag| tag["Key"].as_str() == Some("Owner"))),
        "the delete must not touch the permission set's tags, got {tags}"
    );

    let (status, attached) = call(
        &router,
        "ListManagedPoliciesInPermissionSet",
        &json!({"InstanceArn": instance_arn, "PermissionSetArn": subject}),
    )
    .await;
    assert_eq!(
        status, 200,
        "ListManagedPoliciesInPermissionSet expected 200, got {status} {attached}"
    );
    assert!(
        attached["AttachedManagedPolicies"]
            .as_array()
            .is_some_and(|policies| policies.iter().any(|policy| policy["Arn"].as_str()
                == Some("arn:aws:iam::aws:policy/ReadOnlyAccess"))),
        "the delete must not detach a managed policy, got {attached}"
    );

    let (status, boundary) = call(
        &router,
        "GetPermissionsBoundaryForPermissionSet",
        &json!({"InstanceArn": instance_arn, "PermissionSetArn": subject}),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetPermissionsBoundaryForPermissionSet expected 200, got {status} {boundary}"
    );
    assert_eq!(
        boundary["PermissionsBoundary"]["ManagedPolicyArn"].as_str(),
        Some("arn:aws:iam::aws:policy/PowerUserAccess"),
        "the delete must not touch the permissions boundary, got {boundary}"
    );
}

/// An `InstanceArn` or `PermissionSetArn` nothing is held under is a missing resource —
/// never a 200 — and a deleted permission set is as absent as one that never existed.
#[tokio::test]
async fn an_unknown_instance_or_permission_set_is_not_found() {
    let (router, instance_arn) = organization_router();
    let present = create_permission_set(&router, &instance_arn, "Present").await;
    let document = policy_documents().swap_remove(1).1;

    let mut rows: Vec<(String, String, String)> = vec![
        (
            "an InstanceArn no instance is held under".to_string(),
            UNKNOWN_INSTANCE_ARN.to_string(),
            present.clone(),
        ),
        (
            "a PermissionSetArn never minted".to_string(),
            instance_arn.clone(),
            UNKNOWN_PERMISSION_SET_ARN.to_string(),
        ),
    ];

    // A permission set that carried a document and was then deleted outright.
    let deleted = create_permission_set(&router, &instance_arn, "Deleted").await;
    let (status, body) = put_policy(&router, &instance_arn, &deleted, &document).await;
    assert_eq!(
        status, 200,
        "the put on the doomed permission set expected 200, got {status} {body}"
    );
    let (status, body) = call(
        &router,
        "DeletePermissionSet",
        &json!({"InstanceArn": instance_arn, "PermissionSetArn": deleted}),
    )
    .await;
    assert_eq!(status, 200, "DeletePermissionSet expected 200, got {status} {body}");
    rows.push((
        "a deleted PermissionSetArn".to_string(),
        instance_arn.clone(),
        deleted,
    ));

    for (label, instance, permission_set) in rows {
        let (status, body) = delete_policy(&router, &instance, &permission_set).await;
        assert_refused(
            &label,
            "DeleteInlinePolicyFromPermissionSet",
            "ResourceNotFoundException",
            status,
            &body,
        );
    }

    // The refused calls left the permission set that DOES exist alone.
    let (status, body) = put_policy(&router, &instance_arn, &present, &document).await;
    assert_eq!(status, 200, "the put expected 200, got {status} {body}");
    let (_, stored) = get_policy(&router, &instance_arn, &present).await;
    assert_eq!(
        stored.as_str(),
        Some(document.as_str()),
        "a refused delete must not touch another permission set, got {stored:?}"
    );
}

/// The operation belongs to the permission-set surface, which is WITHDRAWN on an account
/// instance: there it answers the modelled `ValidationException` naming account instances,
/// before any resource lookup.
#[tokio::test]
async fn the_surface_is_withdrawn_on_an_account_instance() {
    let (router, account_instance_arn) = account_instance_router().await;
    let permission_set_arn = format!(
        "arn:aws:sso:::permissionSet/{}/ps-0123456789abcdef",
        account_instance_arn.rsplit('/').next().unwrap_or_default()
    );

    let (status, body) = delete_policy(&router, &account_instance_arn, &permission_set_arn).await;
    assert_eq!(status, 400, "expected HTTP 400, got {status} {body}");
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "expected ValidationException, got {body}"
    );
    let message = body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_default()
        .to_string();
    assert!(
        message.contains("account instances"),
        "the refusal must say the operation is unsupported for account instances, got {message:?}"
    );
}
