// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the IoT SiteWise workspace slice
//! (`CreateWorkspace` -> `UpdateWorkspace` -> `DescribeWorkspace`), driven over
//! HTTP through the same `MultiServiceRouter` the emulator boots with, so every
//! assertion is about the wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * a created workspace is persisted and reads back exactly as it was created,
//!   under the ARN that carries its name, with `createdAt` and `updatedAt` equal
//!   and the encryption configuration reported as the key's ARN;
//! * an accepted update replaces only the members the request carries and
//!   preserves every member it omits, leaves `createdAt` alone and moves the
//!   workspace to `UPDATING` until the transition the clock owes falls due;
//! * only a settled workspace may be updated: one still settling a call it
//!   already accepted is a modelled conflict;
//! * an encryption configuration must pair its `EncryptionType` with a key —
//!   required for a customer managed key, omitted for the service default — and a
//!   refused write leaves the stored workspace untouched;
//! * a customer managed key configuration that is active cannot be changed, while
//!   supplying the same key again is accepted;
//! * a retried write carrying the client token of one that already succeeded
//!   performs no further action.
//!
//! The HTTP harness (`call`, `router`, `error_code`, `REGION`) is the one the
//! asset suite already has; this suite reuses it rather than growing a second.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::ACTIVATION_DELAY;

use crate::asset_lifecycle::{call, error_code, router, REGION};

/// `POST /workspaces`, as `OPERATIONS.md` reports the route.
const WORKSPACES_URI: &str = "/workspaces";

/// `GET|PUT /workspaces/{workspaceName}`.
fn workspace_uri(name: &str) -> String {
    format!("{WORKSPACES_URI}/{name}")
}

/// Long enough that a transition the clock owes has certainly fallen due.
fn settle() -> Duration {
    ACTIVATION_DELAY + Duration::from_millis(200)
}

/// The ARN form this emulator mints for a workspace: a workspace is addressed by
/// its name, so the name is what the resource section carries.
fn expected_arn(name: &str) -> String {
    format!(
        "arn:aws:iotsitewise:{REGION}:{}:workspace/{name}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The ARN of a KMS resource of this account in the region the suite signs for.
fn kms_arn(resource: &str) -> String {
    format!(
        "arn:aws:kms:{REGION}:{}:{resource}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The `state` of the `workspaceStatus` in a response body — the member every
/// workspace operation reports its status under.
fn state(body: &Value) -> &str {
    body["workspaceStatus"]["state"].as_str().unwrap_or("")
}

/// The epoch seconds a `Timestamp` member of `body` carries.
fn at(body: &Value, member: &str) -> f64 {
    body[member]
        .as_f64()
        .unwrap_or_else(|| panic!("{member} is not an epoch-seconds timestamp in {body}"))
}

/// `CreateWorkspace` with `body`.
async fn create(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", WORKSPACES_URI, Some(body)).await
}

/// `UpdateWorkspace` on `name` with `body`.
async fn update(router: &MultiServiceRouter, name: &str, body: Value) -> (u16, Value) {
    call(router, "PUT", &workspace_uri(name), Some(body)).await
}

/// `DescribeWorkspace` on `name`.
async fn describe(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    call(router, "GET", &workspace_uri(name), None).await
}

/// The service's default encryption configuration: no key of the caller's own.
fn default_encryption() -> Value {
    json!({ "encryptionType": "SITEWISE_DEFAULT_ENCRYPTION" })
}

/// A customer managed key configuration naming `kms_key_id`.
fn kms_encryption(kms_key_id: &str) -> Value {
    json!({ "encryptionType": "KMS_BASED_ENCRYPTION", "kmsKeyId": kms_key_id })
}

/// Create `name` with `encryption` and wait for the owed CREATING -> ACTIVE
/// transition, so the workspace is settled and an update may be accepted.
async fn create_settled(router: &MultiServiceRouter, name: &str, encryption: Value) {
    let (status, body) = create(
        router,
        json!({ "workspaceName": name, "encryptionConfiguration": encryption }),
    )
    .await;
    assert_eq!(status, 200, "create {name}: got {status} {body}");
    tokio::time::sleep(settle()).await;
}

/// One row of the create-and-read-back table.
#[derive(Debug)]
struct CreateCase {
    label: &'static str,
    name: String,
    description: Option<String>,
    encryption: Value,
    /// The `encryptionConfiguration` a read must report for `encryption`: the
    /// `WorkspaceEncryptionConfigurationInfo` shape, whose key member is the key's
    /// ARN rather than the id the request named.
    expected_encryption: Value,
    tags: Option<Value>,
}

/// The rows: both `EncryptionType` values (smithy_query query="shape"
/// shape="EncryptionType": SITEWISE_DEFAULT_ENCRYPTION | KMS_BASED_ENCRYPTION)
/// crossed with the three forms `kmsKeyId` admits ("Accepts a key ID, key ARN, or
/// key alias"), with `workspaceDescription` absent, at its minimum length and at
/// its maximum (`Description`, len 1..2048), with `workspaceName` at both ends of
/// `WorkspaceName`'s own 1..64, and with and without tags.
fn create_cases() -> Vec<CreateCase> {
    let key_id = "1234abcd-12ab-34cd-56ef-1234567890ab";
    let foreign_key_arn = "arn:aws:kms:us-west-2:111122223333:key/0f0f0f0f-0f0f-0f0f-0f0f-0f0f0f0f0f0f";
    vec![
        CreateCase {
            label: "service-default-encryption-bare",
            name: "plant-floor".to_string(),
            description: None,
            encryption: default_encryption(),
            expected_encryption: json!({ "encryptionType": "SITEWISE_DEFAULT_ENCRYPTION" }),
            tags: None,
        },
        CreateCase {
            label: "service-default-encryption-described-and-tagged",
            name: "plant_floor_2".to_string(),
            description: Some("The north line".to_string()),
            encryption: default_encryption(),
            expected_encryption: json!({ "encryptionType": "SITEWISE_DEFAULT_ENCRYPTION" }),
            tags: Some(json!({ "line": "north", "env": "test" })),
        },
        CreateCase {
            label: "customer-key-by-id",
            name: "KeyedById".to_string(),
            description: Some("x".to_string()),
            encryption: kms_encryption(key_id),
            expected_encryption: json!({
                "encryptionType": "KMS_BASED_ENCRYPTION",
                "kmsKeyArn": kms_arn(&format!("key/{key_id}")),
            }),
            tags: None,
        },
        CreateCase {
            label: "customer-key-by-alias",
            name: "keyed-by-alias".to_string(),
            description: Some("y".repeat(2048)),
            encryption: kms_encryption("alias/sitewise-workspaces"),
            expected_encryption: json!({
                "encryptionType": "KMS_BASED_ENCRYPTION",
                "kmsKeyArn": kms_arn("alias/sitewise-workspaces"),
            }),
            tags: Some(json!({ "owner": "ops" })),
        },
        CreateCase {
            label: "customer-key-by-arn",
            name: "k".to_string(),
            description: None,
            encryption: kms_encryption(foreign_key_arn),
            expected_encryption: json!({
                "encryptionType": "KMS_BASED_ENCRYPTION",
                "kmsKeyArn": foreign_key_arn,
            }),
            tags: None,
        },
        CreateCase {
            label: "longest-workspace-name",
            name: "w".repeat(64),
            description: Some("at the name bound".to_string()),
            encryption: default_encryption(),
            expected_encryption: json!({ "encryptionType": "SITEWISE_DEFAULT_ENCRYPTION" }),
            tags: None,
        },
    ]
}

/// The request body a row sends, carrying only the members it sets.
fn create_body(case: &CreateCase) -> Value {
    let mut body = json!({
        "workspaceName": case.name,
        "encryptionConfiguration": case.encryption,
    });
    let object = body.as_object_mut().expect("an object body");
    if let Some(description) = case.description.as_ref() {
        object.insert("workspaceDescription".to_string(), json!(description));
    }
    if let Some(tags) = case.tags.as_ref() {
        object.insert("tags".to_string(), tags.clone());
    }
    body
}

#[tokio::test]
async fn a_created_workspace_reads_back_through_describe() {
    let router = router();

    for case in create_cases() {
        let (status, created) = create(&router, create_body(&case)).await;
        assert_eq!(
            status, 200,
            "case {}: create got {status} {created}",
            case.label
        );
        assert_eq!(
            created["workspaceName"], json!(case.name),
            "case {}: the create must report the name it was given, got {created}",
            case.label
        );
        assert_eq!(
            created["workspaceArn"], json!(expected_arn(&case.name)),
            "case {}: expected the workspace ARN {}, got {created}",
            case.label,
            expected_arn(&case.name)
        );
        // "The operation returns immediately with the workspace in the CREATING
        // state": the owed transition is never zero-length, so the create's own
        // answer and a read taken straight after it both observe CREATING.
        assert_eq!(
            state(&created), "CREATING",
            "case {}: a create answers CREATING, got {created}",
            case.label
        );

        let (status, read) = describe(&router, &case.name).await;
        assert_eq!(
            status, 200,
            "case {}: describe got {status} {read}",
            case.label
        );
        assert_eq!(
            read["workspaceName"], json!(case.name),
            "case {}: expected the name back, got {read}",
            case.label
        );
        assert_eq!(
            read["workspaceArn"], json!(expected_arn(&case.name)),
            "case {}: expected the stored ARN, got {read}",
            case.label
        );
        assert_eq!(
            read["workspaceDescription"],
            case.description
                .as_ref()
                .map_or(Value::Null, |description| json!(description)),
            "case {}: expected the description it was created with, got {read}",
            case.label
        );
        assert_eq!(
            read["encryptionConfiguration"], case.expected_encryption,
            "case {}: expected the encryption configuration {}, got {read}",
            case.label, case.expected_encryption
        );
        assert_eq!(
            state(&read), "CREATING",
            "case {}: a read during the create reports CREATING, got {read}",
            case.label
        );
        // Nothing has written to the workspace since the create, so the two
        // instants it recorded are the same one.
        assert_eq!(
            at(&read, "createdAt"),
            at(&read, "updatedAt"),
            "case {}: a freshly created workspace has createdAt == updatedAt, got {read}",
            case.label
        );

        // The tags the create carried land on the workspace's own record, which is
        // what the tagging operations read it through.
        let arn = expected_arn(&case.name);
        let (status, tags) = call(&router, "GET", &format!("/tags?resourceArn={arn}"), None).await;
        assert_eq!(
            status, 200,
            "case {}: list tags got {status} {tags}",
            case.label
        );
        assert_eq!(
            tags["tags"],
            case.tags.clone().unwrap_or_else(|| json!({})),
            "case {}: expected the tags the create carried, got {tags}",
            case.label
        );
    }
}

/// One row of the update table: the configuration the workspace starts under, the
/// body the update sends, and what a read must report afterwards.
#[derive(Debug)]
struct UpdateCase {
    label: &'static str,
    name: &'static str,
    created_description: Option<&'static str>,
    created_encryption: Value,
    body: Value,
    expected_description: Value,
    expected_encryption: Value,
}

/// The rows: an update carrying each subset of the two members it may replace —
/// neither, the description alone, the encryption configuration alone, both —
/// since "Fields that you omit from the request are left unchanged", crossed with
/// a workspace that had a description and one that had none, and with the one
/// encryption change the service allows (the default key to a customer managed
/// one).
fn update_cases() -> Vec<UpdateCase> {
    let key_id = "2234abcd-12ab-34cd-56ef-1234567890ab";
    vec![
        UpdateCase {
            label: "no-members-at-all",
            name: "untouched",
            created_description: Some("as created"),
            created_encryption: default_encryption(),
            body: json!({}),
            expected_description: json!("as created"),
            expected_encryption: json!({ "encryptionType": "SITEWISE_DEFAULT_ENCRYPTION" }),
        },
        UpdateCase {
            label: "description-replaced",
            name: "described",
            created_description: Some("as created"),
            created_encryption: default_encryption(),
            body: json!({ "workspaceDescription": "as updated" }),
            expected_description: json!("as updated"),
            expected_encryption: json!({ "encryptionType": "SITEWISE_DEFAULT_ENCRYPTION" }),
        },
        UpdateCase {
            label: "description-added-to-one-that-had-none",
            name: "undescribed",
            created_description: None,
            created_encryption: default_encryption(),
            body: json!({ "workspaceDescription": "a description at last" }),
            expected_description: json!("a description at last"),
            expected_encryption: json!({ "encryptionType": "SITEWISE_DEFAULT_ENCRYPTION" }),
        },
        UpdateCase {
            label: "encryption-replaced-alone-keeps-the-description",
            name: "re-keyed",
            created_description: Some("as created"),
            created_encryption: default_encryption(),
            body: json!({ "encryptionConfiguration": kms_encryption(key_id) }),
            expected_description: json!("as created"),
            expected_encryption: json!({
                "encryptionType": "KMS_BASED_ENCRYPTION",
                "kmsKeyArn": kms_arn(&format!("key/{key_id}")),
            }),
        },
        UpdateCase {
            label: "both-members-replaced",
            name: "re-keyed-and-described",
            created_description: None,
            created_encryption: default_encryption(),
            body: json!({
                "workspaceDescription": "both at once",
                "encryptionConfiguration": kms_encryption("alias/another-key"),
            }),
            expected_description: json!("both at once"),
            expected_encryption: json!({
                "encryptionType": "KMS_BASED_ENCRYPTION",
                "kmsKeyArn": kms_arn("alias/another-key"),
            }),
        },
        UpdateCase {
            label: "the-same-customer-key-again",
            name: "recovered",
            created_description: Some("under a customer key"),
            created_encryption: kms_encryption(key_id),
            body: json!({ "encryptionConfiguration": kms_encryption(key_id) }),
            expected_description: json!("under a customer key"),
            expected_encryption: json!({
                "encryptionType": "KMS_BASED_ENCRYPTION",
                "kmsKeyArn": kms_arn(&format!("key/{key_id}")),
            }),
        },
    ]
}

#[tokio::test]
async fn an_update_replaces_only_the_members_it_carries() {
    let router = router();

    for case in update_cases() {
        let mut created = json!({
            "workspaceName": case.name,
            "encryptionConfiguration": case.created_encryption,
        });
        if let Some(description) = case.created_description {
            created
                .as_object_mut()
                .expect("an object body")
                .insert("workspaceDescription".to_string(), json!(description));
        }
        let (status, body) = create(&router, created).await;
        assert_eq!(
            status, 200,
            "case {}: create got {status} {body}",
            case.label
        );
        tokio::time::sleep(settle()).await;

        let (status, before) = describe(&router, case.name).await;
        assert_eq!(
            status, 200,
            "case {}: the read before the update got {status} {before}",
            case.label
        );
        assert_eq!(
            state(&before), "ACTIVE",
            "case {}: the owed CREATING -> ACTIVE transition must fall due, got {before}",
            case.label
        );

        let (status, updated) = update(&router, case.name, case.body.clone()).await;
        assert_eq!(
            status, 200,
            "case {}: update got {status} {updated}",
            case.label
        );
        // "The status of the workspace after the update, which is UPDATING when
        // the operation returns."
        assert_eq!(
            state(&updated), "UPDATING",
            "case {}: an accepted update answers UPDATING, got {updated}",
            case.label
        );

        let (status, during) = describe(&router, case.name).await;
        assert_eq!(
            status, 200,
            "case {}: the read during the update got {status} {during}",
            case.label
        );
        assert_eq!(
            state(&during), "UPDATING",
            "case {}: a workspace reports UPDATING until the owed transition falls due, \
             got {during}",
            case.label
        );

        tokio::time::sleep(settle()).await;
        let (status, after) = describe(&router, case.name).await;
        assert_eq!(
            status, 200,
            "case {}: the read after the update got {status} {after}",
            case.label
        );
        assert_eq!(
            state(&after), "ACTIVE",
            "case {}: the owed UPDATING -> ACTIVE transition must fall due, got {after}",
            case.label
        );
        assert_eq!(
            after["workspaceDescription"], case.expected_description,
            "case {}: expected the description {}, got {after}",
            case.label, case.expected_description
        );
        assert_eq!(
            after["encryptionConfiguration"], case.expected_encryption,
            "case {}: expected the encryption configuration {}, got {after}",
            case.label, case.expected_encryption
        );
        // The identity members an update never touches.
        assert_eq!(
            after["workspaceName"], json!(case.name),
            "case {}: an update must not rename the workspace, got {after}",
            case.label
        );
        assert_eq!(
            after["workspaceArn"], json!(expected_arn(case.name)),
            "case {}: an update must not re-mint the ARN, got {after}",
            case.label
        );
        assert_eq!(
            at(&after, "createdAt"),
            at(&before, "createdAt"),
            "case {}: an update must leave createdAt alone, got {after}",
            case.label
        );
        assert!(
            at(&after, "updatedAt") >= at(&after, "createdAt"),
            "case {}: updatedAt must not precede createdAt, got {after}",
            case.label
        );
    }
}

#[tokio::test]
async fn updated_at_moves_to_the_write_while_created_at_stays_put() {
    let router = router();
    let name = "timestamps";

    create_settled(&router, name, default_encryption()).await;
    let (status, created) = describe(&router, name).await;
    assert_eq!(status, 200, "the read after the create got {status} {created}");

    // Far enough past the create that the two instants differ by a whole second,
    // so the assertion holds whatever precision the wire carries.
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    let (status, body) = update(&router, name, json!({ "workspaceDescription": "later" })).await;
    assert_eq!(status, 200, "update got {status} {body}");
    tokio::time::sleep(settle()).await;

    let (status, after) = describe(&router, name).await;
    assert_eq!(status, 200, "the read after the update got {status} {after}");
    assert_eq!(
        at(&after, "createdAt"),
        at(&created, "createdAt"),
        "createdAt is the instant the create landed and nothing moves it, got {after}"
    );
    assert!(
        at(&after, "updatedAt") > at(&created, "updatedAt"),
        "updatedAt must move to the accepted update: created {created}, after {after}"
    );
}

#[tokio::test]
async fn only_a_settled_workspace_can_be_updated() {
    let router = router();
    let name = "settling";

    let (status, body) = create(
        &router,
        json!({
            "workspaceName": name,
            "encryptionConfiguration": default_encryption(),
        }),
    )
    .await;
    assert_eq!(status, 200, "create got {status} {body}");

    // The workspace is still CREATING, so the provisioning the create started is
    // the operation this update conflicts with.
    let (status, refused) = update(&router, name, json!({ "workspaceDescription": "too soon" })).await;
    assert_eq!(
        status, 409,
        "an update of a workspace still being created is a conflict, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ConflictingOperationException",
        "expected the modelled conflict, got {refused}"
    );
    assert_eq!(
        refused["resourceId"], json!(name),
        "the conflict names the workspace it is about, got {refused}"
    );
    assert_eq!(
        refused["resourceArn"], json!(expected_arn(name)),
        "the conflict carries the workspace's ARN, got {refused}"
    );

    tokio::time::sleep(settle()).await;
    let (status, accepted) = update(&router, name, json!({ "workspaceDescription": "in time" })).await;
    assert_eq!(status, 200, "the update of a settled workspace got {status} {accepted}");

    // And the same holds while THAT update settles.
    let (status, refused) = update(&router, name, json!({ "workspaceDescription": "too soon again" })).await;
    assert_eq!(
        status, 409,
        "an update of a workspace still updating is a conflict, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ConflictingOperationException",
        "expected the modelled conflict, got {refused}"
    );

    tokio::time::sleep(settle()).await;
    let (status, read) = describe(&router, name).await;
    assert_eq!(status, 200, "the final read got {status} {read}");
    assert_eq!(
        read["workspaceDescription"], json!("in time"),
        "only the accepted update may have changed the workspace, got {read}"
    );
}

#[tokio::test]
async fn a_workspace_no_create_made_is_a_modelled_refusal() {
    let router = router();

    create_settled(&router, "stored", default_encryption()).await;

    // Names that resolve to nothing: one never created, one differing only in
    // case from a stored name, and one that is a prefix of it.
    for name in ["absent", "Stored", "store"] {
        let (status, body) = describe(&router, name).await;
        assert_eq!(
            status, 404,
            "describe {name}: expected a not-found refusal, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "describe {name}: expected the modelled refusal, got {body}"
        );

        let (status, body) = update(&router, name, json!({ "workspaceDescription": "no such" })).await;
        assert_eq!(
            status, 404,
            "update {name}: expected a not-found refusal, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "update {name}: expected the modelled refusal, got {body}"
        );

        // A request addressing a workspace that does not exist is a not-found
        // refusal whatever else it carries, so the absent resource is reported
        // ahead of anything wrong with the rest of the body.
        let (status, body) = update(
            &router,
            name,
            json!({ "encryptionConfiguration": { "encryptionType": "KMS_BASED_ENCRYPTION" } }),
        )
        .await;
        assert_eq!(
            status, 404,
            "update {name} with a bad configuration: expected the not-found refusal \
             rather than the configuration refusal, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "update {name} with a bad configuration: expected the modelled refusal, got {body}"
        );
    }
}

/// One row of the bad-encryption table: a configuration the service refuses, and
/// why.
#[derive(Debug)]
struct BadEncryptionCase {
    label: &'static str,
    configuration: Value,
}

/// The rows: the two ways a configuration can pair its type with the wrong key
/// ("Required for KMS_BASED_ENCRYPTION; must be omitted for
/// SITEWISE_DEFAULT_ENCRYPTION"), and a type outside the `EncryptionType` value
/// set, with and without a key.
fn bad_encryption_cases() -> Vec<BadEncryptionCase> {
    vec![
        BadEncryptionCase {
            label: "customer-key-without-a-key",
            configuration: json!({ "encryptionType": "KMS_BASED_ENCRYPTION" }),
        },
        BadEncryptionCase {
            label: "service-default-with-a-key",
            configuration: json!({
                "encryptionType": "SITEWISE_DEFAULT_ENCRYPTION",
                "kmsKeyId": "1234abcd-12ab-34cd-56ef-1234567890ab",
            }),
        },
        BadEncryptionCase {
            label: "unmodelled-type",
            configuration: json!({ "encryptionType": "AES_256" }),
        },
        BadEncryptionCase {
            label: "unmodelled-type-with-a-key",
            configuration: json!({
                "encryptionType": "SSE_C",
                "kmsKeyId": "1234abcd-12ab-34cd-56ef-1234567890ab",
            }),
        },
    ]
}

#[tokio::test]
async fn an_encryption_configuration_must_pair_its_type_with_a_key() {
    let router = router();
    let stored = "well-encrypted";
    create_settled(&router, stored, default_encryption()).await;
    let (_, before) = describe(&router, stored).await;

    for case in bad_encryption_cases() {
        let (status, refused) = create(
            &router,
            json!({
                "workspaceName": "never-created",
                "encryptionConfiguration": case.configuration,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {}: the create must be refused, got {status} {refused}",
            case.label
        );
        assert_eq!(
            error_code(&refused),
            "InvalidRequestException",
            "case {}: expected the modelled refusal, got {refused}",
            case.label
        );
        // A refused create stores nothing.
        let (status, body) = describe(&router, "never-created").await;
        assert_eq!(
            status, 404,
            "case {}: a refused create must store no workspace, got {status} {body}",
            case.label
        );

        let (status, refused) = update(
            &router,
            stored,
            json!({ "encryptionConfiguration": case.configuration }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {}: the update must be refused, got {status} {refused}",
            case.label
        );
        assert_eq!(
            error_code(&refused),
            "InvalidRequestException",
            "case {}: expected the modelled refusal, got {refused}",
            case.label
        );
        // A refused update changes nothing — not the members, and not the state,
        // which an accepted one would have moved to UPDATING.
        let (status, after) = describe(&router, stored).await;
        assert_eq!(
            status, 200,
            "case {}: the read after the refused update got {status} {after}",
            case.label
        );
        assert_eq!(
            after, before,
            "case {}: a refused update must leave the workspace untouched",
            case.label
        );
    }
}

#[tokio::test]
async fn an_active_customer_managed_key_cannot_be_changed() {
    let router = router();
    let name = "key-locked";
    let key_id = "3334abcd-12ab-34cd-56ef-1234567890ab";
    create_settled(&router, name, kms_encryption(key_id)).await;
    let (_, before) = describe(&router, name).await;

    // Every configuration that names something other than the key the workspace
    // is already under: another key, the same key in a form that resolves to a
    // different ARN, and the service default.
    let changes = [
        ("another-key", kms_encryption("4444abcd-12ab-34cd-56ef-1234567890ab")),
        ("an-alias", kms_encryption("alias/key-locked")),
        ("the-service-default", default_encryption()),
    ];
    for (label, configuration) in changes {
        let (status, refused) = update(
            &router,
            name,
            json!({ "encryptionConfiguration": configuration }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label}: changing an active customer managed key must be refused, \
             got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidRequestException",
            "case {label}: expected the modelled refusal, got {refused}"
        );
        let (status, after) = describe(&router, name).await;
        assert_eq!(status, 200, "case {label}: the read got {status} {after}");
        assert_eq!(
            after, before,
            "case {label}: a refused update must leave the workspace untouched"
        );
    }

    // "supplying the same key is accepted" — which is also how a workspace
    // recovers from a failed provisioning.
    let (status, accepted) = update(
        &router,
        name,
        json!({ "encryptionConfiguration": kms_encryption(key_id) }),
    )
    .await;
    assert_eq!(status, 200, "the same key again got {status} {accepted}");
    assert_eq!(
        state(&accepted), "UPDATING",
        "an accepted update answers UPDATING, got {accepted}"
    );
}

#[tokio::test]
async fn a_retried_write_performs_no_further_action() {
    let router = router();
    let name = "idempotent";
    let token = "11111111-1111-1111-1111-111111111111";
    let other_token = "22222222-2222-2222-2222-222222222222";

    let request = json!({
        "workspaceName": name,
        "encryptionConfiguration": default_encryption(),
        "clientToken": token,
    });
    let (status, first) = create(&router, request.clone()).await;
    assert_eq!(status, 200, "create got {status} {first}");

    // The retry of a create that already succeeded reports the workspace the
    // original call made rather than colliding with it.
    let (status, retry) = create(&router, request.clone()).await;
    assert_eq!(status, 200, "the create retry got {status} {retry}");
    assert_eq!(
        retry["workspaceArn"], first["workspaceArn"],
        "the retry must report the workspace the original create made, got {retry}"
    );

    // A create of the same name under a DIFFERENT token is a fresh request for a
    // name that is taken.
    let mut collision = request.clone();
    collision["clientToken"] = json!(other_token);
    let (status, refused) = create(&router, collision).await;
    assert_eq!(status, 409, "the colliding create got {status} {refused}");
    assert_eq!(
        error_code(&refused),
        "ConflictingOperationException",
        "expected the modelled conflict, got {refused}"
    );

    tokio::time::sleep(settle()).await;
    let update_request = json!({ "workspaceDescription": "updated once", "clientToken": token });
    let (status, body) = update(&router, name, update_request.clone()).await;
    assert_eq!(status, 200, "update got {status} {body}");
    tokio::time::sleep(settle()).await;
    let (_, settled) = describe(&router, name).await;

    // The retry "succeeds without performing any further actions": it neither
    // moves the workspace back to UPDATING nor advances updatedAt.
    let (status, retry) = update(&router, name, update_request).await;
    assert_eq!(status, 200, "the update retry got {status} {retry}");
    let (status, after) = describe(&router, name).await;
    assert_eq!(status, 200, "the read after the retry got {status} {after}");
    assert_eq!(
        after, settled,
        "a retried update must perform no further action, got {after}"
    );

    // An update carrying another token is a new request and is performed.
    let (status, performed) = update(
        &router,
        name,
        json!({ "workspaceDescription": "updated twice", "clientToken": other_token }),
    )
    .await;
    assert_eq!(status, 200, "the second update got {status} {performed}");
    assert_eq!(
        state(&performed), "UPDATING",
        "a request under a fresh token is performed, got {performed}"
    );
    tokio::time::sleep(settle()).await;
    let (_, read) = describe(&router, name).await;
    assert_eq!(
        read["workspaceDescription"], json!("updated twice"),
        "the performed update must be the one the store reports, got {read}"
    );
}
