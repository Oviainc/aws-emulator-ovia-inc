// SPDX-License-Identifier: CC0-1.0

//! The provisioning step that brings a connector into service: the UPDATE that pins the
//! host key a probe reported.
//!
//! The RULE under test: `UpdateConnector` MODIFIES the members its `SftpConfig` names
//! and leaves the others as they stand. That is what the documented sequence needs —
//! `TrustedHostKeys` is "optional for `CreateConnector`. If not provided, you can use
//! `TestConnection` to retrieve the server host key during the initial connection
//! attempt, and subsequently update the connector with the observed host key" — because
//! the update that pins that key names the key and nothing else: a connector whose
//! credential that update dropped could never pass the test the sequence ends with.
//!
//! The suite asserts the rule over a TABLE of updates that each name ONE member, and
//! then drives the thing the sequence is for: a connector in service carries work out
//! against the server. The target and the connector helpers are the shared
//! [`crate::connector_gated_workflow::Target`] and
//! [`crate::connector_remote_operations`] ones, so nothing here stands a second
//! emulator up.

use serde_json::{json, Value};

use crate::connector_gated_workflow::{
    results, Target, COMPLETED, FILE_BODY, PAST_TRANSFER_SPAN, STATUS_OK,
};
use crate::connector_remote_operations::accept;
use crate::harness::member_of;

/// RULE: `UpdateConnector` modifies the members its `SftpConfig` names and leaves the
/// others as they stand — so the documented provisioning sequence works as documented:
/// the update that pins the observed host key, naming that key and nothing else, leaves
/// the credential the connector was created with, and the next test succeeds.
///
/// The rows differ along which member the update names, and therefore which of them the
/// connector must still hold afterwards: the host key alone, the secret alone, and the
/// concurrency alone.
#[tokio::test]
async fn pinning_the_observed_host_key_leaves_the_credential_the_connector_had() {
    let target = Target::with_home_at_bucket_root().await;
    let secret = target
        .store_secret(
            "aws/transfer/provisioning",
            &target.user_name,
            &target.user_key.private,
        )
        .await;

    // The connector starts with the secret and NO trusted host key, which is what the
    // documented sequence starts from.
    let case = "the documented provisioning sequence";
    let connector_id = target
        .create_connector(case, json!({"UserSecretId": &secret}), None)
        .await;
    let probe = target.test_connection(&connector_id).await;
    let host_key = member_of(probe.get("SftpConnectionDetails"), "HostKey")
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a host key to pin, got {}", probe.body))
        .to_string();

    // (case, the SftpConfig the update names, the members the connector must hold after)
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "the observed host key alone",
            json!({"TrustedHostKeys": [&host_key]}),
            json!({"UserSecretId": &secret, "TrustedHostKeys": [&host_key]}),
        ),
        (
            "the secret alone, which leaves the key pinned",
            json!({"UserSecretId": &secret}),
            json!({"UserSecretId": &secret, "TrustedHostKeys": [&host_key]}),
        ),
        (
            "the concurrency alone, which leaves both",
            json!({"MaxConcurrentConnections": 3}),
            json!({
                "UserSecretId": &secret,
                "TrustedHostKeys": [&host_key],
                "MaxConcurrentConnections": 3,
            }),
        ),
    ];

    for (row, update, expected) in rows {
        let answer = target
            .emulator
            .call(
                "UpdateConnector",
                json!({"ConnectorId": &connector_id, "SftpConfig": update}),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {row:?}: expected UpdateConnector 200, got {} with body {}",
            answer.status, answer.body
        );

        let described = target
            .emulator
            .call("DescribeConnector", json!({"ConnectorId": &connector_id}))
            .await;
        let sftp_config = member_of(described.get("Connector"), "SftpConfig").clone();
        for (member, want) in expected.as_object().expect("the expectation is an object") {
            assert_eq!(
                member_of(&sftp_config, member),
                want,
                "case {row:?}: expected {member} to be {want}, got {sftp_config}"
            );
        }

        // The connection the connector now configures is the one the sequence was for.
        let settled = target.test_connection(&connector_id).await;
        assert_eq!(
            settled.get("Status").as_str(),
            Some(STATUS_OK),
            "case {row:?}: expected the pinned connector's test to succeed, got {}",
            settled.body
        );
    }

    // What the sequence is FOR: the connector now carries work out against the server.
    target.put_object("outbox/landing.txt", FILE_BODY).await;
    let bucket = target.bucket.clone();
    let move_id = accept(
        &target,
        case,
        "StartRemoteMove",
        json!({
            "ConnectorId": &connector_id,
            "SourcePath": format!("/{bucket}/outbox/landing.txt"),
            "TargetPath": format!("/{bucket}/archive/landing.txt"),
        }),
        "MoveId",
    )
    .await;
    tokio::time::sleep(PAST_TRANSFER_SPAN).await;
    let reported = results(&target, &connector_id, &move_id).await;
    assert_eq!(
        member_of(&reported[0], "StatusCode").as_str(),
        Some(COMPLETED),
        "case {case:?}: a connector in service carries its work out, got {}",
        reported[0]
    );
    assert_eq!(
        target.object("archive/landing.txt").await.as_deref(),
        Some(FILE_BODY),
        "case {case:?}: the moved object must be at the target"
    );
}
