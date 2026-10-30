// SPDX-License-Identifier: CC0-1.0

//! Writes that carry an idempotency token, sent while the record a replay would be
//! answered from — or the workspace that record hangs off — is on its way OUT, end to
//! end over HTTP.
//!
//! The rule these suites pin: a write carrying an `@idempotencyToken` is answered as a
//! replay only from a stored record that is still the one that write left, on a
//! workspace that can still take the write; a record whose removal the clock already
//! owes, and one inside a workspace whose removal is owed, is not that record, so the
//! call is a NEW request and gets the refusal its status gate gives the identical call
//! carrying no token. Carrying an idempotency token therefore never turns a refusal into
//! a success that persists nothing — the same rule
//! [`crate::create_while_a_removal_is_owed`] pins for the idempotent creates.
//!
//! Each suite spans that class with a table: the rows differ in WHOSE removal is owed
//! (the record's own, or the workspace it hangs off), in what the replay asks for (the
//! stored value or another), in the region the request is signed for, and across the
//! token-carrying writes the rule covers — `PutResourcePolicy`,
//! `UpdateLoggingConfiguration`, `PutRuleGroupsNamespace` and `PutAnomalyDetector`.
//!
//! The shared harness ([`crate::workspace_lifecycle::Amp`]) drives the real request path
//! — `MultiServiceRouter::dispatch` over this service mounted on one shared engine — so
//! every assertion is made against what the service PERSISTED and answered on the wire.

use crate::anomaly_detector_lifecycle::{create_detector, describe_detector, minimal_configuration};
use crate::anomaly_detector_update::put_detector;
use crate::logging_configuration::log_group_arn;
use crate::logging_configuration_deletion::{create_logging, describe_logging, update_logging};
use crate::resource_policy_lifecycle::{
    delete_policy, describe_policy, policy_document, put_policy,
};
use crate::rule_groups_namespace_lifecycle::{create_body, create_namespace, describe_namespace};
use crate::rule_groups_namespace_update::put_namespace;
use crate::workspace_lifecycle::{text, wait_for_due_transition, Amp, Reply};
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    POLICY_STATUS_ACTIVE, POLICY_STATUS_DELETING, STATUS_DELETING,
};

/// The token the write whose replay each row drives carries, so a row can replay exactly
/// that call.
const WRITE_TOKEN: &str = "the-write-token";

/// Assert that the call carrying the token was refused EXACTLY as the identical call
/// carrying none: same status, same modelled error code, same report. That equality is
/// the rule — an idempotency token does not buy a different answer once the record a
/// replay would come from is on its way out.
fn assert_refused_like_the_tokenless_call(case: &str, replayed: &Reply, tokenless: &Reply) {
    assert_eq!(
        replayed.status, 409,
        "case {case:?}: the call replaying the token must be refused with 409, got {replayed:?}"
    );
    assert_eq!(
        replayed.error_code(),
        "ConflictException",
        "case {case:?}: expected the modelled ConflictException, got {replayed:?}"
    );
    assert_eq!(
        replayed.status, tokenless.status,
        "case {case:?}: the token must not change the status, expected {}, got {}",
        tokenless.status, replayed.status
    );
    assert_eq!(
        replayed.error_code(),
        tokenless.error_code(),
        "case {case:?}: the token must not change the error code, expected {:?}, got {:?}",
        tokenless.error_code(),
        replayed.error_code()
    );
    assert_eq!(
        text(&replayed.body["message"]),
        text(&tokenless.body["message"]),
        "case {case:?}: the token must not change the report, expected {:?}, got {:?}",
        text(&tokenless.body["message"]),
        text(&replayed.body["message"])
    );
}

/// `PutResourcePolicy` replaying the `clientToken` of the put that produced the stored
/// revision: while the policy's own removal, or the removal of the workspace it is
/// attached to, is owed on the clock, that call is refused exactly as the same put
/// carrying no token is — and nothing of the stored policy moves. Once the owed removal
/// has landed, the policy is gone, and where the workspace is still there the put the
/// caller wanted is accepted and PERSISTS: a describe reads the new document back.
#[tokio::test]
async fn a_policy_put_replaying_a_token_while_a_removal_is_owed_is_refused() {
    struct Case {
        name: &'static str,
        region: &'static str,
        /// `true` when the row drives the WORKSPACE's removal, `false` the policy's own.
        delete_workspace: bool,
        /// `true` when the replay carries the document the first put stored.
        same_document: bool,
    }

    let cases = [
        Case {
            name: "the policy's removal is owed, the same document",
            region: "us-west-2",
            delete_workspace: false,
            same_document: true,
        },
        Case {
            name: "the policy's removal is owed, another document",
            region: "eu-west-1",
            delete_workspace: false,
            same_document: false,
        },
        Case {
            name: "the workspace's removal is owed, the same document",
            region: "us-east-1",
            delete_workspace: true,
            same_document: true,
        },
        Case {
            name: "the workspace's removal is owed, another document",
            region: "ap-southeast-2",
            delete_workspace: true,
            same_document: false,
        },
    ];

    for case in &cases {
        let Case {
            name,
            region,
            delete_workspace,
            same_document,
        } = case;
        let amp = Amp::new();
        let workspace_id = amp.active_workspace(region, "replayed-policy").await;

        let stored_document = policy_document("Stored", 1);
        let first = put_policy(
            &amp,
            region,
            &workspace_id,
            &serde_json::json!({
                "policyDocument": stored_document,
                "clientToken": WRITE_TOKEN,
            }),
        )
        .await;
        assert_eq!(
            first.status, 202,
            "case {name:?}: the put that stores the policy must be accepted, got {first:?}"
        );
        let stored_revision = text(&first.body["revisionId"]);
        wait_for_due_transition();

        // From here on a removal is owed on the clock: the policy's own, or that of the
        // workspace it is attached to.
        if *delete_workspace {
            let deleted = amp.delete(region, &workspace_id, None).await;
            assert_eq!(
                deleted.status, 202,
                "case {name:?}: DeleteWorkspace must be accepted, got {deleted:?}"
            );
            let described = amp.describe(region, &workspace_id).await;
            assert_eq!(
                text(&described.body["workspace"]["status"]["statusCode"]),
                STATUS_DELETING,
                "case {name:?}: the workspace must report {STATUS_DELETING}, got {described:?}"
            );
        } else {
            let deleted = delete_policy(&amp, region, &workspace_id, &[]).await;
            assert_eq!(
                deleted.status, 202,
                "case {name:?}: DeleteResourcePolicy must be accepted, got {deleted:?}"
            );
            let described = describe_policy(&amp, region, &workspace_id).await;
            assert_eq!(
                text(&described.body["policyStatus"]),
                POLICY_STATUS_DELETING,
                "case {name:?}: the policy must report {POLICY_STATUS_DELETING}, got {described:?}"
            );
        }

        let replayed_document = if *same_document {
            stored_document.clone()
        } else {
            policy_document("Replayed", 3)
        };
        let replayed = put_policy(
            &amp,
            region,
            &workspace_id,
            &serde_json::json!({
                "policyDocument": replayed_document,
                "clientToken": WRITE_TOKEN,
            }),
        )
        .await;
        let tokenless = put_policy(
            &amp,
            region,
            &workspace_id,
            &serde_json::json!({ "policyDocument": replayed_document }),
        )
        .await;
        assert_refused_like_the_tokenless_call(name, &replayed, &tokenless);

        // Neither refusal wrote: the stored policy is the one the first put left, at the
        // revision it minted, in the status the owed removal put it in.
        let untouched = describe_policy(&amp, region, &workspace_id).await;
        assert_eq!(
            untouched.status, 200,
            "case {name:?}: a refused put removes nothing, got {untouched:?}"
        );
        assert_eq!(
            text(&untouched.body["policyDocument"]),
            stored_document,
            "case {name:?}: the refused put must leave the stored document alone, got \
             {untouched:?}"
        );
        assert_eq!(
            text(&untouched.body["revisionId"]),
            stored_revision,
            "case {name:?}: the refused put must mint no revision, got {untouched:?}"
        );
        assert_eq!(
            text(&untouched.body["policyStatus"]),
            if *delete_workspace {
                POLICY_STATUS_ACTIVE
            } else {
                POLICY_STATUS_DELETING
            },
            "case {name:?}: the refused put must leave the status alone, got {untouched:?}"
        );

        wait_for_due_transition();
        let gone = describe_policy(&amp, region, &workspace_id).await;
        assert_eq!(
            gone.status, 404,
            "case {name:?}: the owed removal must leave the workspace holding no policy, \
             got {gone:?}"
        );
        assert_eq!(
            gone.error_code(),
            "ResourceNotFoundException",
            "case {name:?}: unexpected error code once the removal landed, got {gone:?}"
        );

        // With the removal done, the put the caller wanted is accepted on the workspace
        // that is still there, and it PERSISTS — the document the refused call carried is
        // the one a describe reads back. A workspace that was itself deleted is gone, so
        // the same put is the missing resource it names.
        let again = put_policy(
            &amp,
            region,
            &workspace_id,
            &serde_json::json!({
                "policyDocument": replayed_document,
                "clientToken": WRITE_TOKEN,
            }),
        )
        .await;
        if *delete_workspace {
            assert_eq!(
                again.status, 404,
                "case {name:?}: a put on a workspace that is gone must answer 404, got \
                 {again:?}"
            );
            continue;
        }
        assert_eq!(
            again.status, 202,
            "case {name:?}: a put once the removal is done must be accepted, got {again:?}"
        );
        wait_for_due_transition();
        let persisted = describe_policy(&amp, region, &workspace_id).await;
        assert_eq!(
            text(&persisted.body["policyDocument"]),
            replayed_document,
            "case {name:?}: the accepted put must be what reads report, got {persisted:?}"
        );
        assert_eq!(
            text(&persisted.body["policyStatus"]),
            POLICY_STATUS_ACTIVE,
            "case {name:?}: the new policy must settle {POLICY_STATUS_ACTIVE}, got \
             {persisted:?}"
        );
    }
}

/// The token-carrying writes on a workspace's other children, as one table: each starts a
/// change under a `clientToken`, the workspace is then deleted so its removal is owed, and
/// the replay of that very call is refused exactly as the identical call carrying no token
/// is. Once the owed removal lands the child is gone with the workspace, which is what
/// makes answering a replay from it a success that persists nothing.
enum ChildWrite {
    /// `UpdateLoggingConfiguration` — `PUT /workspaces/{workspaceId}/logging`.
    LoggingUpdate,
    /// `PutRuleGroupsNamespace` — `PUT /workspaces/{id}/rulegroupsnamespaces/{name}`.
    NamespacePut,
    /// `PutAnomalyDetector` — `PUT /workspaces/{id}/anomalydetectors/{detectorId}`.
    DetectorPut,
}

impl ChildWrite {
    fn name(&self) -> &'static str {
        match self {
            ChildWrite::LoggingUpdate => "UpdateLoggingConfiguration",
            ChildWrite::NamespacePut => "PutRuleGroupsNamespace",
            ChildWrite::DetectorPut => "PutAnomalyDetector",
        }
    }

    /// Bring the child this write acts on into existence, and answer with the id or name
    /// the write and the reader address it by.
    async fn create_child(&self, amp: &Amp, region: &str, workspace_id: &str) -> String {
        match self {
            ChildWrite::LoggingUpdate => {
                let created = create_logging(
                    amp,
                    region,
                    workspace_id,
                    &log_group_arn("aws", region, "/aws/prometheus/stored"),
                    None,
                )
                .await;
                assert_eq!(
                    created.status, 202,
                    "case {:?}: the create must be accepted, got {created:?}",
                    self.name()
                );
                String::new()
            }
            ChildWrite::NamespacePut => {
                let name = "replayed-namespace";
                let created = create_namespace(
                    amp,
                    region,
                    workspace_id,
                    &create_body(name, b"groups:\n  - name: stored\n", &[]),
                )
                .await;
                assert_eq!(
                    created.status, 202,
                    "case {:?}: the create must be accepted, got {created:?}",
                    self.name()
                );
                name.to_string()
            }
            ChildWrite::DetectorPut => {
                let created = create_detector(
                    amp,
                    region,
                    workspace_id,
                    &serde_json::json!({
                        "alias": "replayed-detector",
                        "configuration": minimal_configuration("sum(rate(stored_total[5m]))"),
                    }),
                )
                .await;
                assert_eq!(
                    created.status, 202,
                    "case {:?}: the create must be accepted, got {created:?}",
                    self.name()
                );
                text(&created.body["anomalyDetectorId"])
            }
        }
    }

    /// Send the write itself, carrying `client_token` when the row asks for one.
    async fn send(
        &self,
        amp: &Amp,
        region: &str,
        workspace_id: &str,
        child: &str,
        client_token: Option<&str>,
    ) -> Reply {
        match self {
            ChildWrite::LoggingUpdate => {
                update_logging(
                    amp,
                    region,
                    workspace_id,
                    &log_group_arn("aws", region, "/aws/prometheus/written"),
                    client_token,
                )
                .await
            }
            ChildWrite::NamespacePut => {
                let mut body = serde_json::json!({
                    "data": aws_smithy_types::base64::encode(b"groups:\n  - name: written\n"),
                });
                if let Some(token) = client_token {
                    body["clientToken"] = serde_json::Value::String(token.to_string());
                }
                put_namespace(amp, region, workspace_id, child, &body).await
            }
            ChildWrite::DetectorPut => {
                let mut body = serde_json::json!({
                    "configuration": minimal_configuration("sum(rate(written_total[5m]))"),
                });
                if let Some(token) = client_token {
                    body["clientToken"] = serde_json::Value::String(token.to_string());
                }
                put_detector(amp, region, workspace_id, child, &body).await
            }
        }
    }

    /// The reader every assertion about what was persisted goes through.
    async fn describe_child(
        &self,
        amp: &Amp,
        region: &str,
        workspace_id: &str,
        child: &str,
    ) -> Reply {
        match self {
            ChildWrite::LoggingUpdate => describe_logging(amp, region, workspace_id).await,
            ChildWrite::NamespacePut => {
                describe_namespace(amp, region, workspace_id, child).await
            }
            ChildWrite::DetectorPut => describe_detector(amp, region, workspace_id, child).await,
        }
    }
}

/// The same rule on the workspace's other token-carrying writes: a replay of the call
/// that started a pending change is refused once the workspace's own removal is owed,
/// exactly as the identical call carrying no token is, because the child goes with the
/// workspace — so answering from it would accept the call and leave nothing behind.
#[tokio::test]
async fn a_child_write_replaying_a_token_while_the_workspace_removal_is_owed_is_refused() {
    let region = "eu-central-1";
    for write in [
        ChildWrite::LoggingUpdate,
        ChildWrite::NamespacePut,
        ChildWrite::DetectorPut,
    ] {
        let name = write.name();
        let amp = Amp::new();
        let workspace_id = amp.active_workspace(region, "replayed-child").await;
        let child = write.create_child(&amp, region, &workspace_id).await;
        wait_for_due_transition();

        let accepted = write
            .send(&amp, region, &workspace_id, &child, Some(WRITE_TOKEN))
            .await;
        assert_eq!(
            accepted.status, 202,
            "case {name:?}: the write whose token is replayed must be accepted, got \
             {accepted:?}"
        );

        let deleted = amp.delete(region, &workspace_id, None).await;
        assert_eq!(
            deleted.status, 202,
            "case {name:?}: DeleteWorkspace must be accepted, got {deleted:?}"
        );
        let described = amp.describe(region, &workspace_id).await;
        assert_eq!(
            text(&described.body["workspace"]["status"]["statusCode"]),
            STATUS_DELETING,
            "case {name:?}: the workspace must report {STATUS_DELETING}, got {described:?}"
        );

        let replayed = write
            .send(&amp, region, &workspace_id, &child, Some(WRITE_TOKEN))
            .await;
        let tokenless = write.send(&amp, region, &workspace_id, &child, None).await;
        assert_refused_like_the_tokenless_call(name, &replayed, &tokenless);

        // What makes that 202 hollow: the owed removal takes the child with the
        // workspace, so there would have been nothing left for the call it accepted.
        wait_for_due_transition();
        let gone = write
            .describe_child(&amp, region, &workspace_id, &child)
            .await;
        assert_eq!(
            gone.status, 404,
            "case {name:?}: the owed removal must take the child with the workspace, got \
             {gone:?}"
        );
    }
}
