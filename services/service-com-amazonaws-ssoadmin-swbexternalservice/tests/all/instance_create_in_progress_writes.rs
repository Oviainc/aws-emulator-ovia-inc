// SPDX-License-Identifier: CC0-1.0

//! What an instance whose CREATION has not finished serves, end to end over the
//! `awsJson1_1` wire.
//!
//! The rule under test, from `DescribeInstance`'s own documentation of the status
//! (`smithy_query(query="doc",shape="DescribeInstance")`): "CREATE_IN_PROGRESS - The
//! instance is in the process of being created. … While the instance is in the
//! CREATE_IN_PROGRESS state, you can call only DescribeInstance and DeleteInstance
//! operations." So while a `CreateInstance` is still in flight, every operation that
//! MUTATES that instance — or any resource held under it — is refused with the modelled
//! `ConflictException` and writes nothing, while the instance's own describe and delete
//! are served; once the clock makes the activation due the same request is served (or
//! refused for a reason of its OWN, which is what shows the refusal in the window came
//! from the status and from nothing else).
//!
//! The table spans the class the rule generalises over rather than one operation: each
//! member `UpdateInstance` can write, both creates that hold a resource under an instance,
//! both tagging writes, and both Region workflows — including two rows whose request is
//! refused for another reason once the instance is active, so the status refusal cannot be
//! confused with theirs. `instance_create.rs` owns the create's own rules and the window
//! itself; this suite is the one that pins what the window WITHHOLDS.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints,
//! the `awsJson1_1` envelope and the `X-Amz-Target` routing are exercised rather than
//! bypassed. Where a phase has to be on the far side of the activation instant it moves
//! the STORED instant rather than sleeping — the transition itself is still applied by the
//! service's own `materialize_due`, exactly as in production. The harness is the one
//! `ssoadmin_permission_sets.rs` owns.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state::STATUS_CREATE_IN_PROGRESS;
use smithy_server_runtime::MultiServiceRouter;

use crate::instance_create::{empty_account_router, hold_activation};
use crate::ssoadmin_permission_sets::{call, error_code, TEST_REGION};

/// The provider a customer managed application is created under: the `custom` entry of the
/// published catalog (`state::BUILT_IN_APPLICATION_PROVIDERS`), the one `CreateApplication`
/// serves.
const PROVIDER_CUSTOM: &str = "arn:aws:sso::aws:applicationProvider/custom";

/// The only value the model's `TrustedTokenIssuerType` enum admits
/// (`smithy_query(query="shape",shape="TrustedTokenIssuerType")`).
const ISSUER_TYPE: &str = "OIDC_JWT";

/// The only value the model's `JwksRetrievalOption` enum admits
/// (`smithy_query(query="shape",shape="OidcJwtConfiguration")`).
const JWKS_RETRIEVAL_OPTION: &str = "OPEN_ID_DISCOVERY";

/// A customer managed key ARN in the hyphenated-UUID spelling the model's `KmsKeyArn`
/// pattern admits (`smithy_query(query="shape",shape="EncryptionConfiguration")`).
const KEY_ARN_UUID: &str =
    "arn:aws:kms:us-east-1:123456789012:key/0a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d";

/// A Region the created instance is NOT enabled in, for the Region rows: `RegionName`'s
/// pattern `^([a-z]+-){2,3}\d$`, and not the [`TEST_REGION`] every call is signed for.
const OTHER_REGION: &str = "eu-west-1";

/// The tag key the created instance carries, so the untag row names a key that is really
/// there and the tag row re-values one.
const SEED_TAG_KEY: &str = "Owner";

/// One row of the table: a request made against an instance whose creation has not
/// finished.
struct Row {
    label: &'static str,
    operation: &'static str,
    /// The request body, built from the instance's ARN.
    body: fn(&str) -> Value,
    /// What the SAME request must answer once the activation has fallen due: `""` for a
    /// served call, otherwise the wire error code of the refusal this request earns for a
    /// reason of its own.
    once_active: &'static str,
}

/// The rows the rule is asserted over.
const ROWS: &[Row] = &[
    Row {
        label: "UpdateInstance renaming the instance",
        operation: "UpdateInstance",
        body: |arn| json!({"InstanceArn": arn, "Name": "RenamedWhileCreating"}),
        once_active: "",
    },
    Row {
        label: "UpdateInstance enabling permission sets",
        operation: "UpdateInstance",
        body: |arn| json!({"InstanceArn": arn, "PermissionSetsEnabled": true}),
        once_active: "",
    },
    Row {
        label: "UpdateInstance re-keying the encryption configuration",
        operation: "UpdateInstance",
        body: |arn| {
            json!({
                "InstanceArn": arn,
                "EncryptionConfiguration": {
                    "KeyType": "CUSTOMER_MANAGED_KEY",
                    "KmsKeyArn": KEY_ARN_UUID,
                },
            })
        },
        once_active: "",
    },
    Row {
        label: "CreateApplication under the instance",
        operation: "CreateApplication",
        body: |arn| {
            json!({
                "InstanceArn": arn,
                "ApplicationProviderArn": PROVIDER_CUSTOM,
                "Name": "app-while-creating",
            })
        },
        once_active: "",
    },
    Row {
        label: "CreateTrustedTokenIssuer under the instance",
        operation: "CreateTrustedTokenIssuer",
        body: |arn| {
            json!({
                "InstanceArn": arn,
                "Name": "issuer-while-creating",
                "TrustedTokenIssuerType": ISSUER_TYPE,
                "TrustedTokenIssuerConfiguration": {
                    "OidcJwtConfiguration": {
                        "IssuerUrl": "https://issuer.example.com/oidc",
                        "ClaimAttributePath": "email",
                        "IdentityStoreAttributePath": "emails.primary.value",
                        "JwksRetrievalOption": JWKS_RETRIEVAL_OPTION,
                    }
                },
            })
        },
        once_active: "",
    },
    Row {
        label: "TagResource on the instance itself",
        operation: "TagResource",
        body: |arn| {
            json!({
                "ResourceArn": arn,
                "Tags": [{"Key": SEED_TAG_KEY, "Value": "identity"}],
            })
        },
        once_active: "",
    },
    Row {
        label: "TagResource scoped to the holding instance",
        operation: "TagResource",
        body: |arn| {
            json!({
                "InstanceArn": arn,
                "ResourceArn": arn,
                "Tags": [{"Key": "Tier", "Value": "test"}],
            })
        },
        once_active: "",
    },
    Row {
        label: "UntagResource on the instance itself",
        operation: "UntagResource",
        body: |arn| json!({"ResourceArn": arn, "TagKeys": [SEED_TAG_KEY]}),
        once_active: "",
    },
    Row {
        label: "AddRegion enabling the instance in one more Region",
        operation: "AddRegion",
        body: |arn| json!({"InstanceArn": arn, "RegionName": OTHER_REGION}),
        once_active: "",
    },
    Row {
        label: "RemoveRegion naming the primary Region",
        operation: "RemoveRegion",
        body: |arn| json!({"InstanceArn": arn, "RegionName": TEST_REGION}),
        // Once the instance is active the remove is refused for its OWN reason: "The
        // primary Region cannot be removed."
        once_active: "ValidationException",
    },
    Row {
        label: "RemoveRegion naming a Region the instance is not enabled in",
        operation: "RemoveRegion",
        body: |arn| json!({"InstanceArn": arn, "RegionName": OTHER_REGION}),
        // Once the instance is active that Region names no resource of it.
        once_active: "ResourceNotFoundException",
    },
];

/// A router over an account that holds no instance, the engine behind it, and an instance
/// `CreateInstance` just minted whose activation instant is held open: the
/// `CREATE_IN_PROGRESS` window the model declares.
async fn creating_instance() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>, String) {
    let (router, engine) = empty_account_router();
    let (status, created) = call(
        &router,
        "CreateInstance",
        &json!({
            "Name": "WhileCreating",
            "Tags": [{"Key": SEED_TAG_KEY, "Value": "platform"}],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateInstance expected 200, got {status} {created}"
    );
    let instance_arn = created["InstanceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateInstance minted no InstanceArn: {created}"))
        .to_string();
    hold_activation(&engine, SystemTime::now() + Duration::from_secs(3_600));
    (router, engine, instance_arn)
}

/// `DescribeInstance` for `arn`, as a 200 body — the read that is served throughout.
async fn describe(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let (status, body) = call(router, "DescribeInstance", &json!({"InstanceArn": arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInstance expected 200 for {arn}, got {status} {body}"
    );
    body
}

/// The tags `ListTagsForResource` reports for `arn`, as a 200 body.
async fn tags_of(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let (status, body) = call(router, "ListTagsForResource", &json!({"ResourceArn": arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource expected 200 for {arn}, got {status} {body}"
    );
    body["Tags"].clone()
}

/// THE RULE: every mutating request made while the instance's creation is in flight is
/// refused 400 `ConflictException`, changes nothing a later caller sees, and is answered on
/// its own terms once the activation has fallen due.
#[tokio::test]
async fn a_write_is_refused_while_the_instance_is_being_created() {
    for row in ROWS {
        let case = row.label;
        // Each row gets its own account: an account holds ONE instance, and a row must be
        // judged against an instance nothing else has written to.
        let (router, engine, instance_arn) = creating_instance().await;
        let request = (row.body)(&instance_arn);

        // The window is real: the instance reports the transient status throughout.
        let before = describe(&router, case, &instance_arn).await;
        assert_eq!(
            before["Status"].as_str(),
            Some(STATUS_CREATE_IN_PROGRESS),
            "case {case:?}: the fixture must be in the creation window, got {:?}",
            before["Status"]
        );
        let tags_before = tags_of(&router, case, &instance_arn).await;

        let (status, refused) = call(&router, row.operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: {} must be refused while the creation is in flight, got \
             {status} {refused}",
            row.operation
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {refused}"
        );
        let message = refused["Message"].as_str().unwrap_or_default().to_string();
        assert!(
            message.contains(&instance_arn) && message.contains(STATUS_CREATE_IN_PROGRESS),
            "case {case:?}: the refusal must name the instance and the status it is in, \
             got {message:?}"
        );

        // And it wrote NOTHING: both readers answer exactly what they answered before.
        let after = describe(&router, case, &instance_arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused write must change nothing a later caller sees, got \
             {after} vs {before}"
        );
        let tags_after = tags_of(&router, case, &instance_arn).await;
        assert_eq!(
            tags_after, tags_before,
            "case {case:?}: a refused write must leave the tag set alone, got \
             {tags_after} vs {tags_before}"
        );

        // Once the clock makes the activation due, the SAME request is answered on its own
        // terms: served, or refused for a reason that is not the instance's status.
        hold_activation(&engine, SystemTime::now() - Duration::from_secs(60));
        let active = describe(&router, case, &instance_arn).await;
        assert_eq!(
            active["Status"].as_str(),
            Some("ACTIVE"),
            "case {case:?}: the activation was due, got {:?}",
            active["Status"]
        );
        let (status, answer) = call(&router, row.operation, &request).await;
        if row.once_active.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: {} is served once the instance is active, got {status} \
                 {answer}",
                row.operation
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: {} is refused for its own reason once the instance is \
                 active, got {status} {answer}",
                row.operation
            );
            assert_eq!(
                error_code(&answer),
                row.once_active,
                "case {case:?}: expected {} once the instance is active, got {answer}",
                row.once_active
            );
        }
    }
}

/// The other half of the same sentence: the two operations it names ARE served inside the
/// window, so the refusals above are about the operation and not about the instance being
/// unreachable.
#[tokio::test]
async fn the_describe_and_the_delete_are_served_while_the_instance_is_being_created() {
    // The describe, repeatedly: it reports the transient status and, being a read, leaves
    // the record where it was.
    let (router, _engine, instance_arn) = creating_instance().await;
    let first = describe(&router, "the describe", &instance_arn).await;
    assert_eq!(
        first["Status"].as_str(),
        Some(STATUS_CREATE_IN_PROGRESS),
        "the describe must report the transient status, got {first}"
    );
    let second = describe(&router, "the describe again", &instance_arn).await;
    assert_eq!(
        second, first,
        "a repeated describe must answer identically, got {second} vs {first}"
    );

    // The delete, on an instance of its own: it is served inside the window and moves the
    // instance into the deleting status.
    let (router, _engine, instance_arn) = creating_instance().await;
    let (status, deleted) = call(
        &router,
        "DeleteInstance",
        &json!({"InstanceArn": instance_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteInstance must be served inside the creation window, got {status} {deleted}"
    );
    let deleting = describe(&router, "after the delete", &instance_arn).await;
    assert_eq!(
        deleting["Status"].as_str(),
        Some("DELETE_IN_PROGRESS"),
        "the delete must have started, got {deleting}"
    );
}
