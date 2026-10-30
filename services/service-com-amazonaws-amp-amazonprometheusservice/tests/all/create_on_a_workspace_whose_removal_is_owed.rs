// SPDX-License-Identifier: CC0-1.0

//! Creates sent for a child of a workspace whose OWN removal the clock already owes, end
//! to end over HTTP.
//!
//! The rule these suites pin: a create whose record the clock is already due to drop is
//! refused, never accepted — so a child of a workspace that is being deleted
//! (`WorkspaceStatusCode$DELETING`, "Workspace is being deleted") is refused, because the
//! owed removal of the parent takes that child with it and the call would otherwise be
//! accepted and leave the store holding nothing for it. This is the parent-side half of
//! the rule [`crate::create_while_a_removal_is_owed`] pins for a record's own pending
//! removal.
//!
//! Each suite spans that class with a table: the rows differ across the two single-valued
//! logging children of a workspace (`CreateLoggingConfiguration` and
//! `CreateQueryLoggingConfiguration`, both of which the workspace's removal drops), in the
//! idempotency token the refused call carries (the earlier create's, another, none), in
//! whether the workspace already held such a configuration when its deletion started, and
//! in the Region the call is signed for. The boundary suite then walks the workspace
//! statuses that are NOT on their way out, where the same create is still accepted and
//! persists.
//!
//! The shared harness ([`crate::workspace_lifecycle::Amp`]) drives the real request path —
//! `MultiServiceRouter::dispatch` over this service mounted on one shared engine — and
//! `Amp::store` reaches the SAME store the handlers read, which is how a test sees what a
//! refused create actually persisted.

use crate::create_while_a_removal_is_owed::{
    create_query_logging, describe_query_logging, query_body,
};
use crate::logging_configuration::log_group_arn;
use crate::logging_configuration_deletion::{create_logging, describe_logging};
use crate::workspace_lifecycle::{text, wait_for_due_transition, Amp, Reply};
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    LOGGING_STATUS_ACTIVE, LOGGING_STATUS_CREATING, QUERY_LOGGING_STATUS_ACTIVE,
    QUERY_LOGGING_STATUS_CREATING, STATUS_ACTIVE, STATUS_CREATING, STATUS_DELETING,
    WORKSPACE_DELETING_MESSAGE,
};

/// The `qspThreshold` every query logging row in this suite asks for: one value, because
/// nothing here generalises over the filter — what varies is the workspace's status.
const QSP_THRESHOLD: i64 = 3;

/// Which of the workspace's two single-valued logging children a row drives. Both are
/// held by the workspace record (or keyed by its id) and both are dropped when the
/// workspace's owed removal comes due, so the rule covers them alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Child {
    /// `CreateLoggingConfiguration` — the rules-and-alerting configuration.
    RulesAndAlerting,
    /// `CreateQueryLoggingConfiguration` — the query logging configuration.
    Query,
}

impl Child {
    /// The create this child is brought into existence by.
    async fn create(
        self,
        amp: &Amp,
        region: &str,
        workspace_id: &str,
        arn: &str,
        client_token: Option<&str>,
    ) -> Reply {
        match self {
            Child::RulesAndAlerting => {
                create_logging(amp, region, workspace_id, arn, client_token).await
            }
            Child::Query => {
                create_query_logging(
                    amp,
                    region,
                    workspace_id,
                    &query_body(arn, QSP_THRESHOLD, client_token),
                )
                .await
            }
        }
    }

    /// The reader that answers for this child.
    async fn describe(self, amp: &Amp, region: &str, workspace_id: &str) -> Reply {
        match self {
            Child::RulesAndAlerting => describe_logging(amp, region, workspace_id).await,
            Child::Query => describe_query_logging(amp, region, workspace_id).await,
        }
    }

    /// The status a new record of this kind starts in.
    fn creating(self) -> &'static str {
        match self {
            Child::RulesAndAlerting => LOGGING_STATUS_CREATING,
            Child::Query => QUERY_LOGGING_STATUS_CREATING,
        }
    }

    /// The status a settled record of this kind reports.
    fn active(self) -> &'static str {
        match self {
            Child::RulesAndAlerting => LOGGING_STATUS_ACTIVE,
            Child::Query => QUERY_LOGGING_STATUS_ACTIVE,
        }
    }

    /// What the STORE holds for this child of one workspace: its status and the log group
    /// it publishes to, or `None` once the store holds no such record.
    fn stored(self, amp: &Amp, region: &str, workspace_id: &str) -> Option<(String, String)> {
        let store = amp.store();
        let guard = store.lock();
        let region_store = guard.region(region);
        match self {
            Child::RulesAndAlerting => region_store
                .workspaces
                .get(workspace_id)
                .and_then(|workspace| workspace.logging_configuration.as_ref())
                .map(|logging| (logging.status_code.clone(), logging.log_group_arn.clone())),
            Child::Query => region_store.query_logging.get(workspace_id).map(|logging| {
                (
                    logging.status_code.clone(),
                    logging
                        .destinations
                        .first()
                        .map(|destination| destination.log_group_arn.clone())
                        .unwrap_or_default(),
                )
            }),
        }
    }
}

/// The token the first create of a row carries, so another row can replay exactly it.
const FIRST_TOKEN: &str = "first-create-token";

/// One row of the class: the child kind, the token the refused call carries, whether the
/// workspace already held a configuration of that kind when its deletion started, and the
/// Region the calls are signed for.
#[derive(Debug)]
struct Case {
    name: &'static str,
    region: &'static str,
    child: Child,
    token: Option<&'static str>,
    held_one_already: bool,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "rules-and-alerting, no token, nothing held yet",
            region: "us-west-2",
            child: Child::RulesAndAlerting,
            token: None,
            held_one_already: false,
        },
        Case {
            name: "rules-and-alerting, replaying the earlier create's token",
            region: "eu-central-1",
            child: Child::RulesAndAlerting,
            token: Some(FIRST_TOKEN),
            held_one_already: true,
        },
        Case {
            name: "rules-and-alerting, another token at the modelled maximum length",
            region: "ap-southeast-2",
            child: Child::RulesAndAlerting,
            // `IdempotencyToken` is modelled `len 1..64`, so a token at that maximum is
            // carried like any other.
            token: Some("tttttttttttttttttttttttttttttttttttttttttttttttttttttttttttttttt"),
            held_one_already: false,
        },
        Case {
            name: "query logging, no token, nothing held yet",
            region: "us-east-1",
            child: Child::Query,
            token: None,
            held_one_already: false,
        },
        Case {
            name: "query logging, replaying the earlier create's token",
            region: "us-gov-west-1",
            child: Child::Query,
            token: Some(FIRST_TOKEN),
            held_one_already: true,
        },
        Case {
            name: "query logging, another token",
            region: "eu-west-1",
            child: Child::Query,
            token: Some("second-create-token"),
            held_one_already: false,
        },
    ]
}

/// A create of one of the workspace's logging children, sent while the WORKSPACE's own
/// removal is owed, is refused with the modelled `ValidationException` (400) — and the
/// refused call writes nothing: the store holds whatever it held before, and once the
/// removal comes due the workspace and its children are gone, with no record the accepted
/// call would have had to leave behind.
#[tokio::test]
async fn a_logging_child_create_on_a_workspace_being_deleted_is_refused() {
    for case in cases() {
        let Case {
            name,
            region,
            child,
            token,
            held_one_already,
        } = case;
        let amp = Amp::new();
        let workspace_id = amp.active_workspace(region, "doomed").await;
        let first = log_group_arn("aws", region, "/aws/prometheus/first");
        let other = log_group_arn("aws", region, "/aws/prometheus/other");

        if held_one_already {
            let created = child
                .create(&amp, region, &workspace_id, &first, Some(FIRST_TOKEN))
                .await;
            assert_eq!(
                created.status, 202,
                "case {name:?}: the first create must be accepted, got {created:?}"
            );
            wait_for_due_transition();
            // The clock's owed move is materialised by the next request; the reader is
            // what reports it, so the store is read after that call.
            let settled_reply = child.describe(&amp, region, &workspace_id).await;
            assert_eq!(
                settled_reply.status, 200,
                "case {name:?}: the first configuration must be readable back, got \
                 {settled_reply:?}"
            );
            let settled = child.stored(&amp, region, &workspace_id);
            assert_eq!(
                settled.as_ref().map(|(status, _)| status.as_str()),
                Some(child.active()),
                "case {name:?}: the first configuration must settle before the deletion, \
                 got {settled:?}"
            );
        }

        let deleted = amp.delete(region, &workspace_id, None).await;
        assert_eq!(
            deleted.status, 202,
            "case {name:?}: deleting an {STATUS_ACTIVE} workspace must be accepted, got \
             {deleted:?}"
        );
        let deleting = amp.describe(region, &workspace_id).await;
        assert_eq!(
            text(&deleting.body["workspace"]["status"]["statusCode"]),
            STATUS_DELETING,
            "case {name:?}: the workspace must report {STATUS_DELETING} while its removal \
             is owed, got {deleting:?}"
        );

        let refused = child
            .create(&amp, region, &workspace_id, &other, token)
            .await;
        assert_eq!(
            refused.status, 400,
            "case {name:?}: a create on a workspace being deleted must be refused with \
             400, got {refused:?}"
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {name:?}: expected the modelled ValidationException, got {refused:?}"
        );
        assert_eq!(
            text(&refused.body["message"]),
            WORKSPACE_DELETING_MESSAGE,
            "case {name:?}: the refusal must say the workspace's deletion stands in the \
             way, got {refused:?}"
        );

        let after = child.stored(&amp, region, &workspace_id);
        let expected = if held_one_already {
            Some((child.active().to_string(), first.clone()))
        } else {
            None
        };
        assert_eq!(
            after, expected,
            "case {name:?}: the refused create must write nothing, expected \
             {expected:?}, got {after:?}"
        );

        wait_for_due_transition();
        let gone = amp.describe(region, &workspace_id).await;
        assert_eq!(
            gone.status, 404,
            "case {name:?}: the owed removal must leave the workspace gone, got {gone:?}"
        );
        let child_gone = child.describe(&amp, region, &workspace_id).await;
        assert_eq!(
            child_gone.status, 404,
            "case {name:?}: no accepted create may be left without a record, got \
             {child_gone:?}"
        );
        assert_eq!(
            child.stored(&amp, region, &workspace_id),
            None,
            "case {name:?}: the removed workspace holds no configuration at all"
        );
    }
}

/// The boundary of the same rule: only a workspace whose removal is owed refuses the
/// create. A workspace in any other status is not on its way out, so the create is
/// accepted and PERSISTS — a describe reads that record back.
#[tokio::test]
async fn a_workspace_that_is_not_being_deleted_still_takes_a_logging_child() {
    for child in [Child::RulesAndAlerting, Child::Query] {
        for workspace_status in [STATUS_CREATING, STATUS_ACTIVE] {
            let name = format!("{child:?} on a {workspace_status} workspace");
            let region = "us-west-2";
            let amp = Amp::new();
            let workspace_id = if workspace_status == STATUS_ACTIVE {
                amp.active_workspace(region, "settled").await
            } else {
                let created = amp
                    .call(
                        "POST",
                        "/workspaces",
                        region,
                        &serde_json::json!({ "alias": "still-creating" }).to_string(),
                    )
                    .await;
                assert_eq!(
                    created.status, 202,
                    "case {name:?}: the workspace create must be accepted, got {created:?}"
                );
                text(&created.body["workspaceId"])
            };
            let reported = amp.describe(region, &workspace_id).await;
            assert_eq!(
                text(&reported.body["workspace"]["status"]["statusCode"]),
                workspace_status,
                "case {name:?}: the workspace must be in the status under test, got \
                 {reported:?}"
            );

            let arn = log_group_arn("aws", region, "/aws/prometheus/accepted");
            let created = child.create(&amp, region, &workspace_id, &arn, None).await;
            assert_eq!(
                created.status, 202,
                "case {name:?}: the create must be accepted, got {created:?}"
            );
            assert_eq!(
                text(&created.body["status"]["statusCode"]),
                child.creating(),
                "case {name:?}: a new configuration starts {}, got {created:?}",
                child.creating()
            );
            let stored = child.stored(&amp, region, &workspace_id);
            assert_eq!(
                stored,
                Some((child.creating().to_string(), arn.clone())),
                "case {name:?}: the accepted create must persist what it names, got \
                 {stored:?}"
            );
            let described = child.describe(&amp, region, &workspace_id).await;
            assert_eq!(
                described.status, 200,
                "case {name:?}: the accepted create must be readable back, got {described:?}"
            );
        }
    }
}
