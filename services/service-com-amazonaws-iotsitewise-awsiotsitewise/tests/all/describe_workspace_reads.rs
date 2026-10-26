// SPDX-License-Identifier: CC0-1.0

//! What `DescribeWorkspace` reports, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rules this suite pins:
//!
//! * a read answers from the stored workspace — every member it reports is the one
//!   the accepted write left, under the ARN that carries the workspace's own name —
//!   whatever form that name takes across the length and the character range the
//!   model admits;
//! * the read is REPORTING: reading a workspace any number of times neither settles
//!   the transition the clock owes nor touches a member, so the only thing that
//!   differs between two reads of one workspace is the state the clock has made due;
//! * a name that addresses no stored workspace is the modelled not-found refusal,
//!   worded the way this service words the absence of a resource, rather than an
//!   answer about a workspace nothing created.
//!
//! The sibling `workspace_lifecycle` suite covers the WRITES (what a create and an
//! update persist); this one is about the read itself, so it reuses that slice's
//! HTTP harness rather than growing a second.

use serde_json::{json, Value};

use crate::asset_lifecycle::{error_code, router};
use crate::workspace_lifecycle::{
    at, create, create_settled, default_encryption, describe, expected_arn, kms_arn,
    kms_encryption, settle, state,
};

/// A customer managed key named by its id, as `kmsKeyId` documents it ("Accepts a
/// key ID, key ARN, or key alias").
const KEY_ID: &str = "7c9f4b1e-2d3a-4f58-9b61-0e2d4c6a8b10";

/// The same key named by an alias — the other form that member admits.
const KEY_ALIAS: &str = "alias/factory-floor";

/// One row of the read table: a workspace to create, and what a read of it has to
/// report.
#[derive(Debug)]
struct ReadCase {
    label: &'static str,
    name: String,
    description: Option<&'static str>,
    encryption: Value,
    /// The `encryptionConfiguration` the read must carry: the
    /// `WorkspaceEncryptionConfigurationInfo` shape, whose key member is the ARN of
    /// the key the create named rather than the id or alias it was named by.
    expected_encryption: Value,
}

/// The rows: the name forms the model admits — at both published length bounds and
/// across every character class of its pattern — each paired with a different
/// combination of the optional members, so the read is checked against a different
/// stored workspace every time rather than against a renamed copy of one.
///
/// Source: `smithy_query(query="operation", operation="DescribeWorkspace")` —
/// `workspaceName` targets `WorkspaceName`, len 1..64, pattern `^[a-zA-Z0-9_-]+$`
/// — and `smithy_query(query="shape", shape="WorkspaceEncryptionConfiguration")`
/// for the two encryption schemes and the forms `kmsKeyId` accepts.
fn read_cases() -> Vec<ReadCase> {
    vec![
        ReadCase {
            label: "the shortest name the model admits, and no description",
            name: "w".to_string(),
            description: None,
            encryption: default_encryption(),
            expected_encryption: json!({ "encryptionType": "SITEWISE_DEFAULT_ENCRYPTION" }),
        },
        ReadCase {
            label: "the longest name the model admits",
            name: "w".repeat(64),
            description: Some("sixty-four characters of workspace name"),
            encryption: default_encryption(),
            expected_encryption: json!({ "encryptionType": "SITEWISE_DEFAULT_ENCRYPTION" }),
        },
        ReadCase {
            label: "a name spanning every character class the pattern admits",
            name: "Plant_7-Line_02".to_string(),
            description: Some("upper and lower case, digits, underscore and hyphen"),
            encryption: kms_encryption(KEY_ID),
            expected_encryption: json!({
                "encryptionType": "KMS_BASED_ENCRYPTION",
                "kmsKeyArn": kms_arn(&format!("key/{KEY_ID}")),
            }),
        },
        ReadCase {
            label: "a name of digits alone, under a key named by its alias",
            name: "0123456789".to_string(),
            description: None,
            encryption: kms_encryption(KEY_ALIAS),
            expected_encryption: json!({
                "encryptionType": "KMS_BASED_ENCRYPTION",
                "kmsKeyArn": kms_arn(KEY_ALIAS),
            }),
        },
    ]
}

/// The `CreateWorkspace` body for `case`: the optional description is carried only
/// when the row has one, so a row without it exercises an omitted member rather
/// than an explicit null.
fn create_body(case: &ReadCase) -> Value {
    let mut body = json!({
        "workspaceName": case.name,
        "encryptionConfiguration": case.encryption,
    });
    if let Some(description) = case.description {
        body["workspaceDescription"] = json!(description);
    }
    body
}

/// `body` without its `workspaceStatus`: the members a read reports that the clock
/// cannot change, so two reads taken at different instants are comparable.
fn without_status(body: &Value) -> Value {
    let mut stripped = body.clone();
    if let Some(object) = stripped.as_object_mut() {
        object.remove("workspaceStatus");
    }
    stripped
}

#[tokio::test]
async fn a_read_reports_the_stored_workspace_whatever_form_its_name_takes() {
    let router = router();

    for case in read_cases() {
        let (status, created) = create(&router, create_body(&case)).await;
        assert_eq!(
            status, 200,
            "case {}: create got {status} {created}",
            case.label
        );

        // Taken straight after the create, the read reports the transient state the
        // transition the create owes has not settled yet.
        let (status, settling) = describe(&router, &case.name).await;
        assert_eq!(
            status, 200,
            "case {}: the read during the create got {status} {settling}",
            case.label
        );
        assert_eq!(
            state(&settling),
            "CREATING",
            "case {}: expected CREATING while the create settles, got {settling}",
            case.label
        );

        tokio::time::sleep(settle()).await;
        let (status, read) = describe(&router, &case.name).await;
        assert_eq!(status, 200, "case {}: the read got {status} {read}", case.label);

        assert_eq!(
            read["workspaceName"],
            json!(case.name),
            "case {}: the read must name the workspace it was asked for, got {read}",
            case.label
        );
        assert_eq!(
            read["workspaceArn"],
            json!(expected_arn(&case.name)),
            "case {}: the ARN must carry the workspace's name, got {read}",
            case.label
        );
        assert_eq!(
            read["workspaceArn"], created["workspaceArn"],
            "case {}: the read must report the identity the create minted, got {read}",
            case.label
        );

        let reported_description = read
            .get("workspaceDescription")
            .cloned()
            .unwrap_or(Value::Null);
        let expected_description = case.description.map_or(Value::Null, |text| json!(text));
        assert_eq!(
            reported_description, expected_description,
            "case {}: the read must report the description the create carried, got {read}",
            case.label
        );
        assert_eq!(
            read["encryptionConfiguration"], case.expected_encryption,
            "case {}: the read must report the encryption the create was accepted with, got {read}",
            case.label
        );
        assert_eq!(
            state(&read),
            "ACTIVE",
            "case {}: expected ACTIVE once the owed transition fell due, got {read}",
            case.label
        );
        assert_eq!(
            at(&read, "createdAt"),
            at(&read, "updatedAt"),
            "case {}: a workspace no update has touched reports one instant for both, got {read}",
            case.label
        );
    }
}

#[tokio::test]
async fn reading_a_workspace_changes_nothing_a_later_caller_sees() {
    let router = router();
    let name = "polled-hard";

    let (status, created) = create(
        &router,
        json!({
            "workspaceName": name,
            "workspaceDescription": "read over and over",
            "encryptionConfiguration": default_encryption(),
        }),
    )
    .await;
    assert_eq!(status, 200, "create got {status} {created}");

    // Poll the workspace while the transition the create owes is still pending: no
    // number of reads may settle it early, and every read must report the same
    // members as the first. The poll count is kept well inside the owed span so
    // the window, not the test runner's scheduling, is what the reads land in.
    let (status, first) = describe(&router, name).await;
    assert_eq!(status, 200, "the first read got {status} {first}");
    for poll in 0..4 {
        let (status, read) = describe(&router, name).await;
        assert_eq!(status, 200, "poll {poll}: got {status} {read}");
        assert_eq!(
            state(&read),
            "CREATING",
            "poll {poll}: a read must not settle the owed transition, got {read}"
        );
        assert_eq!(
            read, first,
            "poll {poll}: a read must report the workspace the read before it did, got {read}"
        );
    }

    tokio::time::sleep(settle()).await;
    let (status, settled) = describe(&router, name).await;
    assert_eq!(status, 200, "the read after the owed transition got {status} {settled}");
    assert_eq!(
        state(&settled),
        "ACTIVE",
        "the owed transition must have fallen due by itself, got {settled}"
    );
    assert_eq!(
        without_status(&settled),
        without_status(&first),
        "the state is the only member the clock may move, got {settled}"
    );
    assert_eq!(
        at(&settled, "createdAt"),
        at(&first, "createdAt"),
        "createdAt must not move because the workspace was read, got {settled}"
    );
    assert_eq!(
        at(&settled, "updatedAt"),
        at(&first, "updatedAt"),
        "updatedAt must not move because the workspace was read, got {settled}"
    );

    // And a settled workspace stays settled, however often it is read.
    for poll in 0..4 {
        let (status, read) = describe(&router, name).await;
        assert_eq!(status, 200, "settled poll {poll}: got {status} {read}");
        assert_eq!(
            read, settled,
            "settled poll {poll}: a read must report the same workspace, got {read}"
        );
    }
}

#[tokio::test]
async fn a_name_nothing_stores_is_the_modelled_not_found_refusal() {
    let router = router();
    create_settled(&router, "stored-one", default_encryption()).await;

    // Names that address no stored workspace: one never created, the stored name in
    // another case, a prefix of it, an extension of it, and one at the longest form
    // the model admits.
    let longest = "z".repeat(64);
    for name in [
        "absent",
        "STORED-ONE",
        "stored-on",
        "stored-one-2",
        longest.as_str(),
    ] {
        let (status, body) = describe(&router, name).await;
        assert_eq!(
            status, 404,
            "describe {name}: expected the not-found refusal, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "describe {name}: expected the modelled refusal, got {body}"
        );
        assert_eq!(
            body["message"],
            json!(format!("Workspace with ID {name} not found")),
            "describe {name}: the refusal must name the workspace the request addressed, got {body}"
        );
    }
}

#[tokio::test]
async fn a_name_past_the_modelled_bound_is_refused_rather_than_answered() {
    let router = router();

    // Sixty-five characters: one past the published maximum of `workspaceName`
    // (`smithy_query(query="operation", operation="DescribeWorkspace")`, len 1..64),
    // so no accepted create can ever have stored it and no read may answer for it.
    let past_the_bound = "w".repeat(65);

    let (status, refused) = create(
        &router,
        json!({
            "workspaceName": past_the_bound,
            "encryptionConfiguration": default_encryption(),
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a create naming a workspace past the bound must be refused, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidRequestException",
        "the create must be refused as a bad request rather than for another reason, got {refused}"
    );

    let (status, body) = describe(&router, &past_the_bound).await;
    assert_eq!(
        status, 400,
        "a read naming a workspace past the bound must be refused as invalid rather than \
         answered or reported absent, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidRequestException",
        "a read past the bound must be the modelled bad-request refusal, got {body}"
    );
    let message = body["message"].as_str().unwrap_or("");
    assert!(
        message.contains("workspaceName") && message.contains("64"),
        "the refusal must name the member and the bound it broke, got {body}"
    );
}
