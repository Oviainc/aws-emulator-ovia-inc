// SPDX-License-Identifier: CC0-1.0

//! When a workspace rename becomes observable, end to end over HTTP.
//!
//! The rule under test: an update this service accepts without reporting a status for it
//! publishes what it changed in the SAME step the resource settles, so the new value and
//! the settled status appear in one read and the pair (new value, transient status) is
//! never observable. `UpdateWorkspaceAlias` is that operation here — its modelled success
//! is 204 with a `Unit` output, so a caller has nothing in the answer to tell it that an
//! alias a reader can already see is not in effect yet; until the rename settles every
//! reader keeps reporting the alias the workspace had.
//!
//! Each suite spans what the rule generalises over — the form of the new alias (a value,
//! blank-padded, at each modelled `WorkspaceAlias` length bound, differing only in case,
//! omitted), whether the request carries a `clientToken`, the Region the request is
//! signed for, and WHICH reader asks (`DescribeWorkspace`, `ListWorkspaces`, and
//! `ListWorkspaces` filtered by the `alias` prefix) — rather than one recorded request's
//! values.
//!
//! The request harness (`Amp`, `Reply`, [`text`], [`wait_for_due_transition`]) is the one
//! `workspace_lifecycle` already builds.

use crate::workspace_lifecycle::{encode_label, text, wait_for_due_transition, Amp};
use service_com_amazonaws_amp_amazonprometheusservice::state::STATUS_ACTIVE;
use std::time::{Duration, Instant};

/// The `(statusCode, alias)` pair one `DescribeWorkspace` reports.
async fn described_pair(amp: &Amp, region: &str, workspace_id: &str) -> (String, Option<String>) {
    let described = amp.describe(region, workspace_id).await;
    (
        text(&described.body["workspace"]["status"]["statusCode"]),
        described.body["workspace"]["alias"]
            .as_str()
            .map(str::to_string),
    )
}

/// The alias `ListWorkspaces` reports for the one workspace in the Region's store.
async fn listed_alias(amp: &Amp, region: &str, workspace_id: &str) -> Option<String> {
    let listed = amp.list(region, "").await;
    let summaries = listed.body["workspaces"].as_array().cloned().unwrap_or_default();
    let summary = summaries
        .into_iter()
        .find(|summary| text(&summary["workspaceId"]) == workspace_id)
        .unwrap_or_else(|| panic!("the listing must report {workspace_id}: {listed:?}"));
    summary["alias"].as_str().map(str::to_string)
}

/// The `workspaceId`s `ListWorkspaces` reports when it is filtered by the `alias` prefix
/// — which "filters the results to only the workspaces with names that start with the
/// value that you specify here".
async fn listed_with_alias_prefix(amp: &Amp, region: &str, prefix: &str) -> Vec<String> {
    let listed = amp
        .list(region, &format!("alias={}", encode_label(prefix)))
        .await;
    listed.body["workspaces"]
        .as_array()
        .map(|summaries| {
            summaries
                .iter()
                .map(|summary| text(&summary["workspaceId"]))
                .collect()
        })
        .unwrap_or_default()
}

/// One rename, as the form of its new alias and the Region it is signed for.
struct Case {
    name: &'static str,
    region: &'static str,
    /// The alias the workspace is created with, which every reader must keep reporting
    /// until the rename settles.
    published: &'static str,
    body: serde_json::Value,
    /// The alias the settling must publish — `None` when the request leaves the
    /// workspace with none.
    renamed_to: Option<String>,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "a new alias",
            region: "us-west-2",
            published: "before",
            body: serde_json::json!({ "alias": "after" }),
            renamed_to: Some("after".to_string()),
        },
        Case {
            name: "an alias padded with blank space is trimmed before it is published",
            region: "eu-central-1",
            published: "before",
            body: serde_json::json!({ "alias": "   trimmed   " }),
            renamed_to: Some("trimmed".to_string()),
        },
        Case {
            // `WorkspaceAlias` is modelled length 1..100, so each published bound is a row.
            name: "a one-character alias (WorkspaceAlias lower bound)",
            region: "us-east-1",
            published: "before",
            body: serde_json::json!({ "alias": "q" }),
            renamed_to: Some("q".to_string()),
        },
        Case {
            name: "a 100-character alias (WorkspaceAlias upper bound)",
            region: "ap-southeast-2",
            published: "before",
            body: serde_json::json!({ "alias": "w".repeat(100) }),
            renamed_to: Some("w".repeat(100)),
        },
        Case {
            name: "an alias that differs from the published one only in case",
            region: "eu-west-1",
            published: "Mixed",
            body: serde_json::json!({ "alias": "mixed" }),
            renamed_to: Some("mixed".to_string()),
        },
        Case {
            name: "an omitted alias leaves the workspace with none",
            region: "us-west-2",
            published: "before",
            body: serde_json::json!({}),
            renamed_to: None,
        },
        Case {
            name: "a rename carrying a clientToken",
            region: "ap-northeast-1",
            published: "before",
            body: serde_json::json!({ "alias": "tokened", "clientToken": "rename-token" }),
            renamed_to: Some("tokened".to_string()),
        },
    ]
}

/// A client that polls for the new alias — the way a caller learns an accepted rename is
/// done — must never see it on a workspace that is not `ACTIVE`: on every probe the
/// reported alias is the published one while the status is transient, and the probe that
/// first reports the new alias reports `ACTIVE` with it.
#[tokio::test]
async fn a_polling_client_never_sees_the_new_alias_on_a_non_active_workspace() {
    for case in cases() {
        let name = case.name;
        let amp = Amp::new();
        let workspace_id = amp.active_workspace(case.region, case.published).await;

        let renamed = amp.rename(case.region, &workspace_id, &case.body).await;
        assert_eq!(
            renamed.status, 204,
            "case {name:?}: UpdateWorkspaceAlias must answer 204, got {renamed:?}"
        );

        // Poll exactly as a client waiting for the rename does: until the new alias shows
        // up, bounded so a workspace that never settles fails instead of hanging.
        //
        // Each probe reads the LISTING FIRST and the describe second, because the two
        // reads are two calls at two instants and the settling can fall due between them.
        // Publication is monotone — once a reader reports the new alias the workspace is
        // settled and stays settled — so a listing that reported the new alias would be
        // followed by a describe reporting `ACTIVE`. Reading in this order therefore lets
        // the probe that finds a transient status hold BOTH readers to the published
        // alias with no instant in between: the opposite order asserts about a listing
        // taken after the status was read, which the settling may legitimately have
        // overtaken.
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut probes = 0_u32;
        let mut saw_transient = false;
        loop {
            let listed_before_describe = listed_alias(&amp, case.region, &workspace_id).await;
            let (status, alias) = described_pair(&amp, case.region, &workspace_id).await;
            probes += 1;
            if status == STATUS_ACTIVE {
                assert_eq!(
                    alias, case.renamed_to,
                    "case {name:?}: a settled workspace must report the renamed alias \
                     (probe {probes}), got {alias:?}"
                );
                // The same record a listing reads, so the pair it reports agrees. Read
                // again here, AFTER the settled describe, rather than reusing the read
                // taken before it.
                let listed_after_active = listed_alias(&amp, case.region, &workspace_id).await;
                assert_eq!(
                    listed_after_active, case.renamed_to,
                    "case {name:?}: the listing must report the renamed alias, got \
                     {listed_after_active:?}"
                );
                break;
            }
            saw_transient = true;
            assert_eq!(
                alias,
                Some(case.published.to_string()),
                "case {name:?}: while the workspace reports {status:?} every reader must \
                 still report the published alias {:?} (probe {probes}), got {alias:?}",
                case.published
            );
            assert_eq!(
                listed_before_describe,
                Some(case.published.to_string()),
                "case {name:?}: while the workspace reports {status:?} the listing must \
                 still report the published alias, got {listed_before_describe:?}"
            );
            assert!(
                Instant::now() < deadline,
                "case {name:?}: the workspace never reported {STATUS_ACTIVE} \
                 ({probes} probes, last status {status:?})"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            saw_transient,
            "case {name:?}: the rename must be accepted asynchronously, so the first probe \
             must find the workspace in a transient status ({probes} probes)"
        );
    }
}

/// The rule holds for every reader of the alias, not just the one a client happens to
/// call: a describe, a listing and a listing filtered by the `alias` prefix all report
/// the published alias while the rename is in flight, and all report the new one once it
/// has settled — so the filter matches the new alias only from the moment the workspace
/// is `ACTIVE` with it.
#[tokio::test]
async fn every_reader_publishes_the_rename_in_the_same_step() {
    for case in cases() {
        let Some(renamed_to) = case.renamed_to.clone() else {
            // A rename to no alias at all has no prefix to filter on; the describe and
            // listing rows for it are covered by the polling suite above.
            continue;
        };
        let name = case.name;
        let amp = Amp::new();
        let workspace_id = amp.active_workspace(case.region, case.published).await;

        let renamed = amp.rename(case.region, &workspace_id, &case.body).await;
        assert_eq!(
            renamed.status, 204,
            "case {name:?}: UpdateWorkspaceAlias must answer 204, got {renamed:?}"
        );

        let (status, alias) = described_pair(&amp, case.region, &workspace_id).await;
        assert_ne!(
            status, STATUS_ACTIVE,
            "case {name:?}: the rename is accepted asynchronously, so the workspace must \
             report a transient status here, got {status:?}"
        );
        assert_eq!(
            alias,
            Some(case.published.to_string()),
            "case {name:?}: a describe must report the published alias while the rename is \
             in flight, got {alias:?}"
        );
        let matched = listed_with_alias_prefix(&amp, case.region, &renamed_to).await;
        assert!(
            matched.is_empty(),
            "case {name:?}: the alias filter must not match the new alias while the rename \
             is in flight, got {matched:?}"
        );
        let still_matched = listed_with_alias_prefix(&amp, case.region, case.published).await;
        assert_eq!(
            still_matched,
            vec![workspace_id.clone()],
            "case {name:?}: the alias filter must still match the published alias while the \
             rename is in flight, got {still_matched:?}"
        );

        wait_for_due_transition();

        let (status, alias) = described_pair(&amp, case.region, &workspace_id).await;
        assert_eq!(
            status, STATUS_ACTIVE,
            "case {name:?}: once the settling is due the workspace must report \
             {STATUS_ACTIVE}, got {status:?}"
        );
        assert_eq!(
            alias,
            Some(renamed_to.clone()),
            "case {name:?}: the settling must publish the new alias, got {alias:?}"
        );
        let matched = listed_with_alias_prefix(&amp, case.region, &renamed_to).await;
        assert_eq!(
            matched,
            vec![workspace_id.clone()],
            "case {name:?}: the alias filter must match the new alias once it is published, \
             got {matched:?}"
        );
        // The old alias is gone from the record, so the filter that matched it no longer
        // does — unless the new alias starts with it, which no row here does.
        let matched_old = listed_with_alias_prefix(&amp, case.region, case.published).await;
        let expected_old: Vec<String> = if renamed_to.starts_with(case.published) {
            vec![workspace_id.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            matched_old, expected_old,
            "case {name:?}: the alias filter must match the published record, got \
             {matched_old:?}"
        );
    }
}

/// Successive renames each publish in one step, however many run: no round ever exposes
/// the alias the round is applying together with the transient status, and the record
/// ends up `ACTIVE` carrying the last alias.
#[tokio::test]
async fn successive_renames_each_publish_atomically() {
    let region = "us-west-2";
    let amp = Amp::new();
    let mut published = "round-0".to_string();
    let workspace_id = amp.active_workspace(region, &published).await;

    // The rows differ in how many renames have already run and in the form each new
    // alias takes: a plain value, a padded one, one at the lower length bound and one at
    // the upper.
    let renames = [
        ("round-1".to_string(), "round-1".to_string()),
        ("  round-2  ".to_string(), "round-2".to_string()),
        ("r".to_string(), "r".to_string()),
        ("e".repeat(100), "e".repeat(100)),
    ];

    for (round, (requested, expected)) in renames.into_iter().enumerate() {
        let case = format!("rename {round} to {requested:?}");
        let renamed = amp
            .rename(region, &workspace_id, &serde_json::json!({ "alias": requested }))
            .await;
        assert_eq!(
            renamed.status, 204,
            "case {case:?}: UpdateWorkspaceAlias must answer 204, got {renamed:?}"
        );

        let (status, alias) = described_pair(&amp, region, &workspace_id).await;
        assert_ne!(
            status, STATUS_ACTIVE,
            "case {case:?}: the workspace must report a transient status here, got {status:?}"
        );
        assert_eq!(
            alias,
            Some(published.clone()),
            "case {case:?}: the previous alias must stay published until this rename \
             settles, got {alias:?}"
        );

        wait_for_due_transition();

        let (status, alias) = described_pair(&amp, region, &workspace_id).await;
        assert_eq!(
            status, STATUS_ACTIVE,
            "case {case:?}: the workspace must settle {STATUS_ACTIVE}, got {status:?}"
        );
        assert_eq!(
            alias,
            Some(expected.clone()),
            "case {case:?}: the settling must publish this rename's alias, got {alias:?}"
        );
        published = expected;
    }
}
