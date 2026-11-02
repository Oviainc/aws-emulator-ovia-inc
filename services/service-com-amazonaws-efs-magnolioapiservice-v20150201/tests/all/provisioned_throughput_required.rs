// SPDX-License-Identifier: CC0-1.0

//! The conditional-parameter rule both entry points that resolve a throughput mode
//! share: a request whose `ThroughputMode` resolves to `provisioned` while NO provisioned
//! amount is available for it — neither on the request nor already on the stored file
//! system — is refused with HTTP 400 `BadRequest` carrying EFS's FIXED sentence for that
//! rule, and the refusal stores nothing.
//!
//! The sentence is fixed text: it interpolates nothing from the request or from state, so
//! every row below — whichever entry point, whatever the file system, its id form, its
//! starting mode or the other members the request carries — must read back the SAME
//! sentence, held once as `state::PROVISIONED_THROUGHPUT_REQUIRED_MESSAGE`.
//!
//! The tables vary what the rule generalises over: the operation (`CreateFileSystem`,
//! `UpdateFileSystem`), the mode the file system starts in (`bursting`, `elastic`),
//! whether the mode is requested explicitly or already stored, the form of the
//! `FileSystemId` (bare id or ARN) and the unrelated members the request also carries.
//! Each table also carries the positive control: the same request WITH an amount
//! available is accepted.

use serde_json::{json, Value};
use service_com_amazonaws_efs_magnolioapiservice_v20150201::state;

use crate::file_system_lifecycle::{settle, Emulator, Reply};

/// `PUT /2015-02-01/file-systems/{FileSystemId}`, `UpdateFileSystem`'s modelled binding.
async fn update_file_system(efs: &Emulator, addressed_by: &str, body: Value) -> Reply {
    efs.send(
        "PUT",
        &format!("/2015-02-01/file-systems/{addressed_by}"),
        Some(body),
    )
    .await
}

/// The whole refusal, asserted the same way for every row and both operations: the
/// modelled status and error shape, the FIXED sentence, its terminating period, and that
/// nothing from the request or from state leaked into it (no quoted value, no file-system
/// id, no Smithy member name).
fn assert_provisioned_throughput_required(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected HTTP 400, got {} {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("BadRequest"),
        "case {case:?}: expected the modelled BadRequest, got {:?}",
        reply.error_type
    );
    let message = reply.string("Message");
    assert_eq!(
        message,
        state::PROVISIONED_THROUGHPUT_REQUIRED_MESSAGE,
        "case {case:?}: unexpected Message, got {message:?}"
    );
    assert!(
        message.ends_with('.'),
        "case {case:?}: the sentence is terminated with a period, got {message:?}"
    );
    assert!(
        !message.contains('\''),
        "case {case:?}: the sentence interpolates no quoted value, got {message:?}"
    );
    assert!(
        !message.contains("fs-"),
        "case {case:?}: the sentence names no file system, got {message:?}"
    );
    assert!(
        !message.contains("ProvisionedThroughputInMibps") && !message.contains("ThroughputMode"),
        "case {case:?}: the sentence names no Smithy member, got {message:?}"
    );
}

#[tokio::test]
async fn create_file_system_in_provisioned_mode_without_an_amount_is_refused_and_stores_nothing() {
    // (case, CreateFileSystem body). Every row asks for the provisioned mode and carries
    // no `ProvisionedThroughputInMibps`; the unrelated members vary, because the rule is
    // keyed to the throughput pair alone.
    let refused: Vec<(&str, Value)> = vec![
        (
            "nothing but the mode",
            json!({ "CreationToken": "needs-amount-create-1", "ThroughputMode": "provisioned" }),
        ),
        (
            "with an encrypted file system",
            json!({
                "CreationToken": "needs-amount-create-2",
                "ThroughputMode": "provisioned",
                "Encrypted": true,
            }),
        ),
        (
            "with the maxIO performance mode and tags",
            json!({
                "CreationToken": "needs-amount-create-3",
                "ThroughputMode": "provisioned",
                "PerformanceMode": "maxIO",
                "Tags": [{ "Key": "Name", "Value": "needs-amount" }],
            }),
        ),
    ];

    let efs = Emulator::new();
    for (case, body) in &refused {
        let reply = efs.create_file_system(body.clone()).await;
        assert_provisioned_throughput_required(case, &reply);
    }

    // Effect-free: no file system, and no creation token, was recorded by any refusal.
    let listed = efs.describe_file_systems("").await;
    assert_eq!(
        listed.status, 200,
        "DescribeFileSystems: expected 200, got {} {:?}",
        listed.status, listed.body
    );
    let stored = listed
        .get("FileSystems")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        stored.is_empty(),
        "a refused create stores no file system, got {stored:?}"
    );

    // The positive control: each refused request is accepted once an amount comes with
    // it, under the very token the refusal must not have consumed.
    for (case, body) in &refused {
        let mut accepted_body = body.clone();
        accepted_body["ProvisionedThroughputInMibps"] = json!(1.0);
        let accepted = efs.create_file_system(accepted_body.clone()).await;
        assert_eq!(
            accepted.status, 201,
            "case {case:?}: the same request with an amount expected 201, got {} {:?}",
            accepted.status, accepted.body
        );
        // ROUND TRIP: the created file system reads back with what it was created from.
        let file_system_id = accepted.string("FileSystemId");
        settle().await;
        let described = efs
            .describe_file_systems(&format!("FileSystemId={file_system_id}"))
            .await;
        let description = described.get("FileSystems")[0].clone();
        assert_eq!(
            description["ThroughputMode"].as_str(),
            Some("provisioned"),
            "case {case:?}: the stored file system uses the requested mode, got {description:?}"
        );
        assert_eq!(
            description["ProvisionedThroughputInMibps"].as_f64(),
            Some(1.0),
            "case {case:?}: the stored file system carries the requested amount, got \
             {description:?}"
        );
    }
}

#[tokio::test]
async fn update_file_system_to_provisioned_without_an_amount_is_refused_and_changes_nothing() {
    // (case, CreateFileSystem body, address by ARN). Each row starts from a mode that
    // stores NO provisioned amount, so switching to `provisioned` needs one in the
    // request; the modes are the modelled `ThroughputMode` closure's other two values
    // (`smithy_query(query="shape",shape="ThroughputMode")` -> bursting | elastic |
    // provisioned).
    let cases: Vec<(&str, Value, bool)> = vec![
        (
            "from the default bursting mode",
            json!({ "CreationToken": "needs-amount-update-1" }),
            false,
        ),
        (
            "from an explicit bursting mode, addressed by ARN",
            json!({ "CreationToken": "needs-amount-update-2", "ThroughputMode": "bursting" }),
            true,
        ),
        (
            "from the elastic mode",
            json!({ "CreationToken": "needs-amount-update-3", "ThroughputMode": "elastic" }),
            false,
        ),
        (
            "from the elastic mode, addressed by ARN",
            json!({ "CreationToken": "needs-amount-update-4", "ThroughputMode": "elastic" }),
            true,
        ),
    ];

    let efs = Emulator::new();
    for (case, create_body, by_arn) in cases {
        let mode_before = create_body["ThroughputMode"].as_str().unwrap_or("bursting");
        let created = efs.create_file_system(create_body.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: CreateFileSystem expected 201, got {} {:?}",
            created.status, created.body
        );
        let file_system_id = created.string("FileSystemId");
        let file_system_arn = created.string("FileSystemArn");
        settle().await;

        // An `@httpLabel` is not greedy, so an SDK percent-encodes the separators of an
        // ARN it puts in one; both forms address the same stored file system.
        let addressed_by = if by_arn {
            file_system_arn.replace(':', "%3A").replace('/', "%2F")
        } else {
            file_system_id.clone()
        };
        let refused =
            update_file_system(&efs, &addressed_by, json!({ "ThroughputMode": "provisioned" }))
                .await;
        assert_provisioned_throughput_required(case, &refused);

        // Effect-free: the next caller reads the mode and the phase the refusal found.
        let described = efs
            .describe_file_systems(&format!("FileSystemId={file_system_id}"))
            .await;
        let description = described.get("FileSystems")[0].clone();
        assert_eq!(
            description["ThroughputMode"].as_str(),
            Some(mode_before),
            "case {case:?}: a refused update leaves the stored mode standing, got {description:?}"
        );
        assert_eq!(
            description["LifeCycleState"].as_str(),
            Some("available"),
            "case {case:?}: a refused update starts no transition, got {description:?}"
        );
        assert_eq!(
            description["ProvisionedThroughputInMibps"].as_f64(),
            None,
            "case {case:?}: a refused update provisions nothing, got {description:?}"
        );

        // The positive control: the same switch WITH an amount is accepted and settles.
        let accepted = update_file_system(
            &efs,
            &addressed_by,
            json!({ "ThroughputMode": "provisioned", "ProvisionedThroughputInMibps": 1.0 }),
        )
        .await;
        assert_eq!(
            accepted.status, 202,
            "case {case:?}: the same switch with an amount expected 202, got {} {:?}",
            accepted.status, accepted.body
        );
        settle().await;
        let settled = efs
            .describe_file_systems(&format!("FileSystemId={file_system_id}"))
            .await;
        let settled = settled.get("FileSystems")[0].clone();
        assert_eq!(
            settled["ThroughputMode"].as_str(),
            Some("provisioned"),
            "case {case:?}: the accepted switch is applied, got {settled:?}"
        );
        assert_eq!(
            settled["ProvisionedThroughputInMibps"].as_f64(),
            Some(1.0),
            "case {case:?}: the accepted amount is applied, got {settled:?}"
        );
    }
}

#[tokio::test]
async fn a_file_system_that_already_stores_an_amount_may_restate_the_provisioned_mode() {
    // The rule's precondition is that NO amount is available, not that the request omits
    // one: a file system already in the provisioned mode carries its own amount, so the
    // same request body is accepted rather than refused.
    let efs = Emulator::new();
    for amount in [1.0, 100.0, state::MAX_PROVISIONED_THROUGHPUT_MIBPS] {
        let case = format!("stored amount {amount}");
        let created = efs
            .create_file_system(json!({
                "CreationToken": format!("stored-amount-{amount}"),
                "ThroughputMode": "provisioned",
                "ProvisionedThroughputInMibps": amount,
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: CreateFileSystem expected 201, got {} {:?}",
            created.status, created.body
        );
        let file_system_id = created.string("FileSystemId");
        settle().await;

        let restated = update_file_system(
            &efs,
            &file_system_id,
            json!({ "ThroughputMode": "provisioned" }),
        )
        .await;
        assert_eq!(
            restated.status, 202,
            "case {case:?}: expected HTTP 202, got {} {:?}",
            restated.status, restated.body
        );
        assert_eq!(
            restated.get("ProvisionedThroughputInMibps").as_f64(),
            Some(amount),
            "case {case:?}: the stored amount stands, got {:?}",
            restated.get("ProvisionedThroughputInMibps")
        );
        assert!(
            restated.string("Message").is_empty(),
            "case {case:?}: an accepted update carries no refusal sentence, got {:?}",
            restated.body
        );
    }
}
