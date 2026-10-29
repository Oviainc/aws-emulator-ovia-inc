// SPDX-License-Identifier: CC0-1.0

//! `UpdateWorkspaceAlias`, end to end over HTTP.
//!
//! The rule under test: a workspace update is ACCEPTED rather than applied
//! synchronously, so the addressed workspace reports the modelled transient
//! `WorkspaceStatusCode` `UPDATING` from the moment the call is answered until the clock
//! owes its move back to `ACTIVE` — and while it is in that status every operation the
//! model allows only on an `ACTIVE` workspace is refused `ConflictException` (409),
//! except a replay of the very call that started the update, which its
//! `@idempotencyToken` makes the same request.
//!
//! The new alias is published by that settling and not before it, which
//! `workspace_alias_publication` pins over the readers and the forms the alias takes:
//! here it is asserted only as the value each read reports while the rename is in
//! flight (the one the workspace had before the call) and once it has settled.
//!
//! Each suite spans what that rule generalises over — the form of the new alias (a
//! value, blank-padded, at each modelled length bound, omitted), whether the request
//! carries a `clientToken`, which status-gated operation the second caller attempts, how
//! many renames run in succession, and the region the request is signed for — rather
//! than one recorded request's values.
//!
//! The request harness (`Amp`, `Reply`, [`text`], [`wait_for_due_transition`]) is the one
//! `workspace_lifecycle` already builds.

use crate::workspace_lifecycle::{text, wait_for_due_transition, Amp};
use service_com_amazonaws_amp_amazonprometheusservice::ops::types::WORKSPACE_STATUS_CODE_VALUES;
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    STATUS_ACTIVE, STATUS_UPDATING,
};

/// The `statusCode` a describe reports for one workspace.
async fn described_status(amp: &Amp, region: &str, workspace_id: &str) -> String {
    let described = amp.describe(region, workspace_id).await;
    text(&described.body["workspace"]["status"]["statusCode"])
}

/// The `alias` a describe reports for one workspace, absent when it carries none.
async fn described_alias(amp: &Amp, region: &str, workspace_id: &str) -> Option<String> {
    let described = amp.describe(region, workspace_id).await;
    described.body["workspace"]["alias"]
        .as_str()
        .map(str::to_string)
}

/// `UPDATING` is a value the modelled `WorkspaceStatusCode` enum declares — the catalog
/// comes from the model, not from this test.
#[test]
fn updating_is_a_modelled_workspace_status_code() {
    assert!(
        WORKSPACE_STATUS_CODE_VALUES.contains(&STATUS_UPDATING),
        "case {STATUS_UPDATING:?}: not a modelled WorkspaceStatusCode \
         ({WORKSPACE_STATUS_CODE_VALUES:?})"
    );
}

/// The rename is accepted, not applied synchronously: the workspace reads back
/// `UPDATING` straight after the call and `ACTIVE` once the owed settling is due, with
/// the new alias visible in both statuses. One row per form the new alias takes and per
/// signing region.
#[tokio::test]
async fn workspace_rename_is_accepted_asynchronously() {
    struct Case {
        name: &'static str,
        region: &'static str,
        body: serde_json::Value,
        /// The alias every later read must report — `None` when the request leaves the
        /// workspace with none.
        stored_alias: Option<String>,
    }

    let cases = vec![
        Case {
            name: "a new alias",
            region: "us-west-2",
            body: serde_json::json!({ "alias": "renamed" }),
            stored_alias: Some("renamed".to_string()),
        },
        Case {
            name: "an alias padded with blank space",
            region: "eu-central-1",
            body: serde_json::json!({ "alias": "   padded   " }),
            stored_alias: Some("padded".to_string()),
        },
        Case {
            // `WorkspaceAlias` is modelled length 1..100, so each published bound is a row.
            name: "a one-character alias (WorkspaceAlias lower bound)",
            region: "us-east-1",
            body: serde_json::json!({ "alias": "q" }),
            stored_alias: Some("q".to_string()),
        },
        Case {
            name: "a 100-character alias (WorkspaceAlias upper bound)",
            region: "ap-southeast-2",
            body: serde_json::json!({ "alias": "w".repeat(100) }),
            stored_alias: Some("w".repeat(100)),
        },
        Case {
            name: "an omitted alias leaves the workspace with none",
            region: "us-west-2",
            body: serde_json::json!({}),
            stored_alias: None,
        },
        Case {
            name: "an alias sent with a clientToken",
            region: "eu-west-1",
            body: serde_json::json!({ "alias": "tokened", "clientToken": "rename-token" }),
            stored_alias: Some("tokened".to_string()),
        },
    ];

    for case in cases {
        let name = case.name;
        let amp = Amp::new();
        let workspace_id = amp.active_workspace(case.region, "before").await;

        let renamed = amp.rename(case.region, &workspace_id, &case.body).await;
        assert_eq!(
            renamed.status, 204,
            "case {name:?}: UpdateWorkspaceAlias must answer 204, got {renamed:?}"
        );

        let in_flight = described_status(&amp, case.region, &workspace_id).await;
        assert_eq!(
            in_flight, STATUS_UPDATING,
            "case {name:?}: straight after the rename the workspace must report \
             {STATUS_UPDATING}, got {in_flight:?}"
        );
        let in_flight_alias = described_alias(&amp, case.region, &workspace_id).await;
        assert_eq!(
            in_flight_alias,
            Some("before".to_string()),
            "case {name:?}: while the rename is in flight the workspace must still report \
             the alias it had, got {in_flight_alias:?}"
        );

        wait_for_due_transition();

        let settled = described_status(&amp, case.region, &workspace_id).await;
        assert_eq!(
            settled, STATUS_ACTIVE,
            "case {name:?}: once the transition is due the workspace must report \
             {STATUS_ACTIVE}, got {settled:?}"
        );
        let settled_alias = described_alias(&amp, case.region, &workspace_id).await;
        assert_eq!(
            settled_alias, case.stored_alias,
            "case {name:?}: the settling must publish the new alias, got {settled_alias:?}"
        );

        // A listing reads the same record a describe does, so it reports the same pair.
        let listed = amp.list(case.region, "").await;
        assert_eq!(
            text(&listed.body["workspaces"][0]["status"]["statusCode"]),
            STATUS_ACTIVE,
            "case {name:?}: a listing must report the settled status, got {listed:?}"
        );
        assert_eq!(
            listed.body["workspaces"][0]["alias"]
                .as_str()
                .map(str::to_string),
            case.stored_alias,
            "case {name:?}: a listing must report the new alias, got {listed:?}"
        );
    }
}

/// While the rename is in flight the workspace is not `ACTIVE`, so every operation the
/// model allows only on an `ACTIVE` workspace is refused `ConflictException` (409) — one
/// row per such operation — and each succeeds once the clock has settled the workspace.
#[tokio::test]
async fn an_updating_workspace_refuses_the_operations_that_need_it_active() {
    /// One status-gated operation, as a request a second caller makes while the first
    /// caller's rename is still in flight.
    struct Case {
        name: &'static str,
        method: &'static str,
        /// Built from the workspace id, since every one of these is addressed by it.
        uri: fn(&str) -> String,
        body: fn() -> String,
    }

    let cases = vec![
        Case {
            name: "a second UpdateWorkspaceAlias with another clientToken",
            method: "POST",
            uri: |workspace_id| format!("/workspaces/{workspace_id}/alias"),
            body: || serde_json::json!({ "alias": "second", "clientToken": "other" }).to_string(),
        },
        Case {
            name: "a second UpdateWorkspaceAlias with no clientToken at all",
            method: "POST",
            uri: |workspace_id| format!("/workspaces/{workspace_id}/alias"),
            body: || serde_json::json!({ "alias": "third" }).to_string(),
        },
        Case {
            name: "DeleteWorkspace",
            method: "DELETE",
            uri: |workspace_id| format!("/workspaces/{workspace_id}"),
            body: String::new,
        },
        Case {
            name: "UpdateWorkspaceConfiguration",
            method: "PATCH",
            uri: |workspace_id| format!("/workspaces/{workspace_id}/configuration"),
            body: || serde_json::json!({ "retentionPeriodInDays": 30 }).to_string(),
        },
    ];

    let region = "us-west-2";
    for case in cases {
        let name = case.name;
        let amp = Amp::new();
        let workspace_id = amp.active_workspace(region, "contended").await;
        let first = amp
            .rename(
                region,
                &workspace_id,
                &serde_json::json!({ "alias": "first", "clientToken": "first-rename" }),
            )
            .await;
        assert_eq!(
            first.status, 204,
            "case {name:?}: the first rename must answer 204, got {first:?}"
        );

        let uri = (case.uri)(&workspace_id);
        let refused = amp.call(case.method, &uri, region, &(case.body)()).await;
        assert_eq!(
            refused.status, 409,
            "case {name:?}: an {STATUS_UPDATING} workspace must refuse this with 409, \
             got {refused:?}"
        );
        assert_eq!(
            refused.error_code(),
            "ConflictException",
            "case {name:?}: unexpected error code, got {refused:?}"
        );
        // The refusal changed nothing, and the first rename has not settled yet, so the
        // alias every reader reports is still the one the workspace was created with.
        let alias = described_alias(&amp, region, &workspace_id).await;
        assert_eq!(
            alias,
            Some("contended".to_string()),
            "case {name:?}: a refused call must leave the published alias alone, got {alias:?}"
        );

        wait_for_due_transition();

        // The settling published the first rename's alias, and only then.
        let published = described_alias(&amp, region, &workspace_id).await;
        assert_eq!(
            published,
            Some("first".to_string()),
            "case {name:?}: the settling must publish the first rename's alias, got \
             {published:?}"
        );

        let accepted = amp.call(case.method, &uri, region, &(case.body)()).await;
        assert!(
            (200..300).contains(&accepted.status),
            "case {name:?}: once the workspace is {STATUS_ACTIVE} again the same call must \
             be accepted, got {accepted:?}"
        );
    }
}

/// `clientToken` is `@idempotencyToken` on an `@idempotent` operation: replaying the very
/// call that started the in-flight rename is that same call, so it is accepted again and
/// neither applies a second change nor pushes the owed settling out. Once the update has
/// settled the token stands for nothing, so a later call carrying it is a new request.
#[tokio::test]
async fn replaying_the_in_flight_rename_token_is_accepted() {
    let region = "us-west-2";
    let amp = Amp::new();
    let workspace_id = amp.active_workspace(region, "idempotent").await;
    let body = serde_json::json!({ "alias": "settled-name", "clientToken": "rename-xyz" });

    let first = amp.rename(region, &workspace_id, &body).await;
    assert_eq!(
        first.status, 204,
        "case \"the first rename\": must answer 204, got {first:?}"
    );

    // Several replays in a row, so a replay that silently re-armed the owed settling
    // would show up as a workspace that never reaches ACTIVE.
    for attempt in 0..3 {
        let replay = amp.rename(region, &workspace_id, &body).await;
        assert_eq!(
            replay.status, 204,
            "case \"replay {attempt}\": replaying the token must answer 204, got {replay:?}"
        );
        let status = described_status(&amp, region, &workspace_id).await;
        assert_eq!(
            status, STATUS_UPDATING,
            "case \"replay {attempt}\": the workspace must still be {STATUS_UPDATING}, \
             got {status:?}"
        );
        // A replay applies nothing of its own, and the rename it replays has not
        // settled, so the published alias is still the pre-rename one.
        let alias = described_alias(&amp, region, &workspace_id).await;
        assert_eq!(
            alias,
            Some("idempotent".to_string()),
            "case \"replay {attempt}\": the published alias must still be the pre-rename \
             one, got {alias:?}"
        );
    }

    wait_for_due_transition();
    let settled = described_status(&amp, region, &workspace_id).await;
    assert_eq!(
        settled, STATUS_ACTIVE,
        "case \"after the owed settling\": the workspace must report {STATUS_ACTIVE}, \
         got {settled:?}"
    );
    let settled_alias = described_alias(&amp, region, &workspace_id).await;
    assert_eq!(
        settled_alias,
        Some("settled-name".to_string()),
        "case \"after the owed settling\": the rename must be published exactly once, \
         got {settled_alias:?}"
    );

    // The token no longer names an update in flight, so the SAME request is a new rename:
    // it is applied and the workspace goes back to UPDATING.
    let reused = amp
        .rename(
            region,
            &workspace_id,
            &serde_json::json!({ "alias": "reused-token", "clientToken": "rename-xyz" }),
        )
        .await;
    assert_eq!(
        reused.status, 204,
        "case \"the token reused after the update settled\": must answer 204, got {reused:?}"
    );
    let status = described_status(&amp, region, &workspace_id).await;
    assert_eq!(
        status, STATUS_UPDATING,
        "case \"the token reused after the update settled\": the workspace must be \
         {STATUS_UPDATING} again, got {status:?}"
    );
    let in_flight_alias = described_alias(&amp, region, &workspace_id).await;
    assert_eq!(
        in_flight_alias,
        Some("settled-name".to_string()),
        "case \"the token reused after the update settled\": the new rename is in flight, \
         so the previous alias must still be the published one, got {in_flight_alias:?}"
    );

    wait_for_due_transition();
    let alias = described_alias(&amp, region, &workspace_id).await;
    assert_eq!(
        alias,
        Some("reused-token".to_string()),
        "case \"the token reused after the update settled\": the settling must publish \
         the new alias, got {alias:?}"
    );
}

/// Successive renames each run the same cycle, however many there are: the record ends
/// up `ACTIVE` carrying the last alias, and no rename is lost along the way.
#[tokio::test]
async fn successive_renames_each_settle_before_the_next() {
    let region = "us-west-2";
    let amp = Amp::new();
    let workspace_id = amp.active_workspace(region, "serial-0").await;

    for round in 1..=4 {
        let previous = format!("serial-{}", round - 1);
        let alias = format!("serial-{round}");
        let renamed = amp
            .rename(region, &workspace_id, &serde_json::json!({ "alias": alias }))
            .await;
        assert_eq!(
            renamed.status, 204,
            "case \"rename {round}\": must answer 204, got {renamed:?}"
        );
        let in_flight = described_status(&amp, region, &workspace_id).await;
        assert_eq!(
            in_flight, STATUS_UPDATING,
            "case \"rename {round}\": the workspace must report {STATUS_UPDATING}, got \
             {in_flight:?}"
        );
        let in_flight_alias = described_alias(&amp, region, &workspace_id).await;
        assert_eq!(
            in_flight_alias,
            Some(previous.clone()),
            "case \"rename {round}\": the in-flight workspace must still report \
             {previous:?}, got {in_flight_alias:?}"
        );
        wait_for_due_transition();
        let settled = described_status(&amp, region, &workspace_id).await;
        assert_eq!(
            settled, STATUS_ACTIVE,
            "case \"rename {round}\": the workspace must settle {STATUS_ACTIVE}, got \
             {settled:?}"
        );
        let stored = described_alias(&amp, region, &workspace_id).await;
        assert_eq!(
            stored,
            Some(alias.clone()),
            "case \"rename {round}\": the stored alias must be the one just set, got {stored:?}"
        );
    }
}
