// SPDX-License-Identifier: CC0-1.0

//! The INLINE POLICY of a permission set, end to end over the `awsJson1_1` wire.
//!
//! THE RULE under test: a permission set carries AT MOST ONE inline policy;
//! `PutInlinePolicyToPermissionSet` stores the `InlinePolicy` document it is given
//! VERBATIM against the permission set the request names, a later put REPLACES the stored
//! document, and `GetInlinePolicyForPermissionSet` answers from the stored one — reporting
//! `InlinePolicy` ABSENT until a put stores one, since it is an optional output member. A
//! document the modelled `PermissionSetPolicyDocument` constraints refuse (length 1..32768,
//! pattern `^[\u{0009}\u{000A}\u{000D}\u{0020}-\u{00FF}]+$`) is answered
//! `ValidationException` and stores nothing, and an `InstanceArn` or `PermissionSetArn`
//! nothing is held under is a `ResourceNotFoundException` for both operations.
//!
//! The rules are asserted over a TABLE whose rows differ along what the rule generalises
//! over: the SIZE of the document (its published length bounds 1 and 32768, and one past
//! each), the characters the document is built from (ASCII, the tab / newline /
//! carriage-return the pattern singles out, and the top of its `\u{0020}-\u{00FF}` range),
//! the number of statements it carries, and the order the puts are made in. The member's
//! constraints are the model's own
//! (`smithy_query(query="operation",operation="PutInlinePolicyToPermissionSet,GetInlinePolicyForPermissionSet",model_path="services/service-com-amazonaws-ssoadmin-swbexternalservice/smithy-model/com-amazonaws-ssoadmin-swbexternalservice.json")`).
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes, so the modelled constraints and the `awsJson1_1` envelope are exercised
//! rather than bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns, and the
//! calls run against an ORGANIZATION instance because the permission-set surface is
//! withdrawn on an account instance (asserted here too).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    account_instance_router, call, error_code, organization_router, UNKNOWN_INSTANCE_ARN,
    UNKNOWN_PERMISSION_SET_ARN,
};

/// `PermissionSetPolicyDocument`'s published length bounds (`length(min: 1, max: 32768)`).
const POLICY_DOCUMENT_MIN: usize = 1;
const POLICY_DOCUMENT_MAX: usize = 32768;

/// A document of exactly `len` characters, built from ASCII only so its character count
/// and its byte count agree, for the rows that sit on a published length bound.
fn document_of_length(len: usize) -> String {
    let prefix = "{\"Comment\":\"";
    let suffix = "\"}";
    assert!(
        len >= prefix.len() + suffix.len(),
        "case length {len}: too short to build a padded document"
    );
    format!(
        "{prefix}{}{suffix}",
        "a".repeat(len - prefix.len() - suffix.len())
    )
}

/// The documents the round-trip rule is asserted over. Rows span the published length
/// bounds, the whitespace characters the pattern singles out (`\u{0009}`, `\u{000A}`,
/// `\u{000D}`), the top of its `\u{0020}-\u{00FF}` character range, and the number of
/// statements the policy carries. No row is one a recording happened to carry.
fn policy_documents() -> Vec<(String, String)> {
    vec![
        (
            format!("a document of the minimum length {POLICY_DOCUMENT_MIN}"),
            "x".repeat(POLICY_DOCUMENT_MIN),
        ),
        (
            "a single-statement policy in canonical JSON".to_string(),
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#
                .to_string(),
        ),
        (
            "a two-statement policy indented with tabs, newlines and carriage returns"
                .to_string(),
            "{\r\n\t\"Version\": \"2012-10-17\",\r\n\t\"Statement\": [\r\n\t\t\
             {\"Effect\": \"Allow\", \"Action\": [\"ec2:Describe*\"], \"Resource\": \"*\"},\r\n\t\t\
             {\"Effect\": \"Deny\", \"Action\": \"iam:*\", \"Resource\": \"*\"}\r\n\t]\r\n}"
                .to_string(),
        ),
        (
            "a document using the top of the pattern's character range".to_string(),
            "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Sid\":\"\u{00c0}\u{00e9}\u{00ff}\
             \u{00a0}\",\"Effect\":\"Allow\",\"Action\":\"s3:*\",\"Resource\":\"*\"}]}"
                .to_string(),
        ),
        (
            format!("a document of the maximum length {POLICY_DOCUMENT_MAX}"),
            document_of_length(POLICY_DOCUMENT_MAX),
        ),
    ]
}

/// The documents the modelled constraints refuse: one character SHORT of the minimum,
/// one PAST the maximum, and characters outside the pattern's class — the one just below
/// its `\u{0020}` floor and one just above its `\u{00FF}` ceiling.
fn refused_documents() -> Vec<(String, String)> {
    vec![
        (
            "an empty document, one character below the minimum".to_string(),
            String::new(),
        ),
        (
            format!("a document one character past the maximum {POLICY_DOCUMENT_MAX}"),
            document_of_length(POLICY_DOCUMENT_MAX + 1),
        ),
        (
            "a document carrying a control character the pattern excludes".to_string(),
            "{\"Comment\":\"\u{001f}\"}".to_string(),
        ),
        (
            "a document carrying a character above the pattern's range".to_string(),
            "{\"Comment\":\"\u{20ac}\"}".to_string(),
        ),
    ]
}

/// Create a permission set named `name` on `instance_arn` and return its minted ARN.
async fn create_permission_set(
    router: &MultiServiceRouter,
    instance_arn: &str,
    name: &str,
) -> String {
    let (status, body) = call(
        router,
        "CreatePermissionSet",
        &json!({"InstanceArn": instance_arn, "Name": name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreatePermissionSet expected 200, got {status} {body}"
    );
    body["PermissionSet"]["PermissionSetArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: no minted PermissionSetArn in {body}"))
        .to_string()
}

/// `GetInlinePolicyForPermissionSet`, as `(status, InlinePolicy or Null)`.
async fn get_policy(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
) -> (u16, Value) {
    let (status, body) = call(
        router,
        "GetInlinePolicyForPermissionSet",
        &json!({
            "InstanceArn": instance_arn,
            "PermissionSetArn": permission_set_arn,
        }),
    )
    .await;
    if status != 200 {
        return (status, body);
    }
    (status, body["InlinePolicy"].clone())
}

/// A put of `document`, as `(status, body)`.
async fn put_policy(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    document: &str,
) -> (u16, Value) {
    call(
        router,
        "PutInlinePolicyToPermissionSet",
        &json!({
            "InstanceArn": instance_arn,
            "PermissionSetArn": permission_set_arn,
            "InlinePolicy": document,
        }),
    )
    .await
}

/// THE RULE: a put stores the document it names VERBATIM and the get answers from the
/// stored one — over every row of the table, each on its own permission set. Before the
/// put, the same get reports no inline policy at all.
#[tokio::test]
async fn a_put_stores_the_document_and_the_get_answers_from_it() {
    let (router, instance_arn) = organization_router();

    for (index, (label, document)) in policy_documents().into_iter().enumerate() {
        let permission_set_arn =
            create_permission_set(&router, &instance_arn, &format!("Inline{index}")).await;

        // A permission set with nothing attached: `InlinePolicy` is an OPTIONAL output
        // member, so it is absent rather than an error or an empty document.
        let (status, before) = get_policy(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a get before any put expected 200, got {status} {before}"
        );
        assert_eq!(
            before,
            Value::Null,
            "case {label:?}: a permission set with no inline policy must report none, got \
             {before}"
        );

        let (status, body) = put_policy(&router, &instance_arn, &permission_set_arn, &document).await;
        assert_eq!(
            status, 200,
            "case {label:?}: PutInlinePolicyToPermissionSet expected 200, got {status} {body}"
        );

        let (status, stored) = get_policy(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the get after the put expected 200, got {status} {stored}"
        );
        assert_eq!(
            stored.as_str(),
            Some(document.as_str()),
            "case {label:?}: expected the stored document back verbatim, got {stored:?}"
        );
    }
}

/// A permission set carries AT MOST ONE inline policy, so a later put REPLACES the stored
/// document: walking the whole table over ONE permission set, each get reports only the
/// newest row, and putting the same document twice stores it once.
#[tokio::test]
async fn a_later_put_replaces_the_stored_document() {
    let (router, instance_arn) = organization_router();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "Replaced").await;

    for (label, document) in policy_documents() {
        let (status, body) = put_policy(&router, &instance_arn, &permission_set_arn, &document).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the replacing put expected 200, got {status} {body}"
        );

        let (status, stored) = get_policy(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the get expected 200, got {status} {stored}"
        );
        assert_eq!(
            stored.as_str(),
            Some(document.as_str()),
            "case {label:?}: a later put replaces the document, got {stored:?}"
        );
    }

    let (label, document) = policy_documents().swap_remove(1);
    for round in 0..2 {
        let (status, body) = put_policy(&router, &instance_arn, &permission_set_arn, &document).await;
        assert_eq!(
            status, 200,
            "case {label:?} round {round}: a repeated put expected 200, got {status} {body}"
        );
    }
    let (_, stored) = get_policy(&router, &instance_arn, &permission_set_arn).await;
    assert_eq!(
        stored.as_str(),
        Some(document.as_str()),
        "case {label:?}: a repeated put stores one document, got {stored:?}"
    );
}

/// A read must not change what a later caller sees: repeated gets answer identically, and
/// a get never brings an inline policy into being.
#[tokio::test]
async fn a_get_changes_nothing_a_later_caller_sees() {
    let (router, instance_arn) = organization_router();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "Repeatable").await;

    for round in 0..3 {
        let (status, policy) = get_policy(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            status, 200,
            "case round {round}: expected 200, got {status} {policy}"
        );
        assert_eq!(
            policy,
            Value::Null,
            "case round {round}: a get must not attach a policy, got {policy}"
        );
    }

    let (label, document) = policy_documents().swap_remove(1);
    let (status, body) = put_policy(&router, &instance_arn, &permission_set_arn, &document).await;
    assert_eq!(
        status, 200,
        "case {label:?}: the put expected 200, got {status} {body}"
    );

    let (_, first) = get_policy(&router, &instance_arn, &permission_set_arn).await;
    let (_, second) = get_policy(&router, &instance_arn, &permission_set_arn).await;
    assert_eq!(
        first, second,
        "case {label:?}: repeated gets must agree, got {first} then {second}"
    );
    assert_eq!(
        first.as_str(),
        Some(document.as_str()),
        "case {label:?}: expected the stored document, got {first:?}"
    );
}

/// An inline policy of one permission set is NOT the inline policy of another: the
/// attachment belongs to the permission set the put named.
#[tokio::test]
async fn a_policy_belongs_to_the_permission_set_it_was_put_on() {
    let (router, instance_arn) = organization_router();
    let documents = policy_documents();
    let mut attached: Vec<(String, String, String)> = Vec::new();

    for (index, (label, document)) in documents.iter().enumerate() {
        let permission_set_arn =
            create_permission_set(&router, &instance_arn, &format!("Owned{index}")).await;
        let (status, body) = put_policy(&router, &instance_arn, &permission_set_arn, document).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the put expected 200, got {status} {body}"
        );
        attached.push((label.clone(), permission_set_arn, document.clone()));
    }

    // One permission set that never got a put: it still reports none, however many other
    // permission sets of the same instance carry one.
    let untouched = create_permission_set(&router, &instance_arn, "Untouched").await;
    let (status, policy) = get_policy(&router, &instance_arn, &untouched).await;
    assert_eq!(status, 200, "the untouched get expected 200, got {status} {policy}");
    assert_eq!(
        policy,
        Value::Null,
        "a permission set no put named must report no policy, got {policy}"
    );

    for (label, permission_set_arn, document) in &attached {
        let (status, stored) = get_policy(&router, &instance_arn, permission_set_arn).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the get expected 200, got {status} {stored}"
        );
        assert_eq!(
            stored.as_str(),
            Some(document.as_str()),
            "case {label:?}: {permission_set_arn} must carry its own document, got {stored:?}"
        );
    }
}

/// A document the modelled constraints refuse is answered `ValidationException` and stores
/// nothing: neither on a permission set that carries no policy, nor over one that does.
#[tokio::test]
async fn a_document_outside_the_modelled_constraints_is_refused_and_stores_nothing() {
    let (router, instance_arn) = organization_router();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "Refused").await;

    for (label, document) in refused_documents() {
        let (status, body) = put_policy(&router, &instance_arn, &permission_set_arn, &document).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {label:?}: expected ValidationException, got {body}"
        );

        let (_, stored) = get_policy(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            stored,
            Value::Null,
            "case {label:?}: a refused put must store nothing, got {stored}"
        );
    }

    // The same refusals over a permission set that already carries a policy leave the
    // stored one exactly as it was.
    let (accepted_label, accepted) = policy_documents().swap_remove(1);
    let (status, body) = put_policy(&router, &instance_arn, &permission_set_arn, &accepted).await;
    assert_eq!(
        status, 200,
        "case {accepted_label:?}: the accepted put expected 200, got {status} {body}"
    );
    for (label, document) in refused_documents() {
        let (status, body) = put_policy(&router, &instance_arn, &permission_set_arn, &document).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400, got {status} {body}"
        );
        let (_, stored) = get_policy(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            stored.as_str(),
            Some(accepted.as_str()),
            "case {label:?}: a refused put must leave the stored document, got {stored:?}"
        );
    }
}

/// An `InstanceArn` or `PermissionSetArn` nothing is held under is a missing resource for
/// BOTH operations — never a 200 — and a deleted permission set is as absent as one that
/// never existed.
#[tokio::test]
async fn an_unknown_instance_or_permission_set_is_not_found() {
    let (router, instance_arn) = organization_router();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "Present").await;
    let document = policy_documents().swap_remove(1).1;

    let mut rows: Vec<(String, String, String)> = vec![
        (
            "an InstanceArn no instance is held under".to_string(),
            UNKNOWN_INSTANCE_ARN.to_string(),
            permission_set_arn.clone(),
        ),
        (
            "a PermissionSetArn never minted".to_string(),
            instance_arn.clone(),
            UNKNOWN_PERMISSION_SET_ARN.to_string(),
        ),
    ];

    // A permission set that was created and then deleted.
    let deleted = create_permission_set(&router, &instance_arn, "Deleted").await;
    let (status, body) = put_policy(&router, &instance_arn, &deleted, &document).await;
    assert_eq!(status, 200, "the put on the doomed permission set expected 200, got {status} {body}");
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
        let (status, body) = put_policy(&router, &instance, &permission_set, &document).await;
        assert_eq!(
            status, 400,
            "case {label:?}: the put expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {label:?}: the put expected ResourceNotFoundException, got {body}"
        );

        let (status, body) = get_policy(&router, &instance, &permission_set).await;
        assert_eq!(
            status, 400,
            "case {label:?}: the get expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {label:?}: the get expected ResourceNotFoundException, got {body}"
        );
    }
}

/// Both operations belong to the permission-set surface, which is WITHDRAWN on an account
/// instance: there they answer the modelled `ValidationException` naming account instances,
/// before any resource lookup.
#[tokio::test]
async fn the_surface_is_withdrawn_on_an_account_instance() {
    let (router, account_instance_arn) = account_instance_router().await;
    let permission_set_arn = format!(
        "arn:aws:sso:::permissionSet/{}/ps-0123456789abcdef",
        account_instance_arn.rsplit('/').next().unwrap_or_default()
    );

    for (label, document) in policy_documents() {
        let (status, body) =
            put_policy(&router, &account_instance_arn, &permission_set_arn, &document).await;
        assert_eq!(
            status, 400,
            "case {label:?}: the put expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {label:?}: the put expected ValidationException, got {body}"
        );
        let message = body["Message"]
            .as_str()
            .or_else(|| body["message"].as_str())
            .unwrap_or_default()
            .to_string();
        assert!(
            message.contains("account instances"),
            "case {label:?}: the refusal must say the operation is unsupported for account \
             instances, got {message:?}"
        );
    }

    let (status, body) = get_policy(&router, &account_instance_arn, &permission_set_arn).await;
    assert_eq!(status, 400, "the get expected HTTP 400, got {status} {body}");
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "the get expected ValidationException, got {body}"
    );
    let message = body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_default()
        .to_string();
    assert!(
        message.contains("account instances"),
        "the get's refusal must say the operation is unsupported for account instances, got \
         {message:?}"
    );
}
