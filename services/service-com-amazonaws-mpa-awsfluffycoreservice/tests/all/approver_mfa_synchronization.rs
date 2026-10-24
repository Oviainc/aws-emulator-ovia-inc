// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateApprovalTeam$UpdateActions` and the approver MFA state
//! `GetApprovalTeam` reports in `GetApprovalTeamResponseApprover$MfaMethods`.
//!
//! The rule under test: an update action the request names is APPLIED to the team the
//! update produces, and its effect is observable on a later read — the one action
//! `UpdateAction` declares synchronizes the MFA devices of every approver on the team,
//! so each device is reported `IN_SYNC` afterwards, where a device that has never been
//! synchronized is `OUT_OF_SYNC`. An update that names no action changes no device, and
//! a read never synchronizes one.
//!
//! The tables span that class along what the rule generalises over: the number of
//! approvers on the team (1, 2 and the `Approvers` `length(1..20)` maximum), the form of
//! the `UpdateActions` list (absent, empty, the token once, the token repeated, the token
//! beside other members), which approvers are on the team when the action runs (kept,
//! added by the same request, added by a later one), and tokens the enum does not declare.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the routes, the URI label, the JSON envelope
//! and the HTTP status are all exercised.

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::ops::types::shared_types as model;
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_team_lifecycle::{error_code, text, Harness, REGION};

/// `Approvers` is `length(min: 1, max: 20)` on both `CreateApprovalTeam` and
/// `UpdateApprovalTeam`, so this is the largest team either call accepts.
const MAX_APPROVERS: usize = 20;

/// One approver: `(PrimaryIdentityId, PrimaryIdentitySourceArn)`.
fn identity(n: usize) -> (String, String) {
    (
        format!("identity-{n:04}"),
        format!(
            "arn:aws:mpa:{REGION}:{}:identity-source/ssoins-{n:04}",
            engine::emulator_account_id()
        ),
    )
}

fn approver_member(n: usize) -> Value {
    let (id, source) = identity(n);
    json!({ "PrimaryIdentityId": id, "PrimaryIdentitySourceArn": source })
}

fn create_body(name: &str, approvers: usize) -> Value {
    json!({
        "Name": name,
        "Description": "mfa synchronization",
        "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 1 } },
        "Approvers": (1..=approvers).map(approver_member).collect::<Vec<_>>(),
        "Policies": [{
            "PolicyArn": "arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy/$DEFAULT"
        }],
    })
}

/// Create a team with `approvers` approvers and answer `(Arn, VersionId)`.
async fn create_team(harness: &Harness, name: &str, approvers: usize) -> (String, String) {
    let (status, created) = harness.create(&create_body(name, approvers)).await;
    assert_eq!(
        status, 200,
        "case {name} with {approvers} approvers: create returned {created:?}"
    );
    (text(&created, "Arn"), text(&created, "VersionId"))
}

/// The `MfaMethods` of every approver `GetApprovalTeam` reports, as
/// `(Type, SyncStatus)` pairs in the order the response lists them.
async fn reported_mfa_methods(harness: &Harness, arn: &str) -> Vec<Vec<(String, String)>> {
    let (status, team) = harness.get(arn).await;
    assert_eq!(status, 200, "get returned {team:?}");
    team["Approvers"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|approver| {
            approver["MfaMethods"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|method| (text(method, "Type"), text(method, "SyncStatus")))
                .collect()
        })
        .collect()
}

/// Every token this service writes for an approver's MFA configuration, and the action
/// token it reads, is one the model declares. The catalogs come from the generated
/// `shared_types` value lists, rendered from the model's `MfaType`, `MfaSyncStatus` and
/// `UpdateAction` enums.
#[test]
fn the_mfa_tokens_this_service_uses_are_ones_the_model_declares() {
    let mfa_type = state::MfaType::EmailOtp;
    assert!(
        model::MFA_TYPE_VALUES.contains(&mfa_type.as_wire()),
        "case {mfa_type:?}: expected a token MfaType declares, got {:?}",
        mfa_type.as_wire()
    );

    for sync in [state::MfaSyncStatus::InSync, state::MfaSyncStatus::OutOfSync] {
        assert!(
            model::MFA_SYNC_STATUS_VALUES.contains(&sync.as_wire()),
            "case {sync:?}: expected a token MfaSyncStatus declares, got {:?}",
            sync.as_wire()
        );
    }

    assert_eq!(
        model::UPDATE_ACTION_VALUES,
        [state::SYNCHRONIZE_MFA_DEVICES_ACTION],
        "case UpdateAction catalog: this service applies every action the enum declares, so a \
         new token must be implemented rather than accepted and ignored; got {:?}",
        model::UPDATE_ACTION_VALUES
    );
}

/// An approver whose devices have never been synchronized is reported `OUT_OF_SYNC`,
/// for every team size — and reading the team does not synchronize them.
#[tokio::test]
async fn an_unsynchronized_approver_is_reported_out_of_sync_and_a_read_changes_nothing() {
    let harness = Harness::new();

    for approvers in [1, 2, MAX_APPROVERS] {
        let label = format!("{approvers} approvers");
        let (arn, _) = create_team(&harness, &format!("mfa-fresh-{approvers}"), approvers).await;

        let first = reported_mfa_methods(&harness, &arn).await;
        assert_eq!(
            first.len(),
            approvers,
            "case {label}: expected one approver entry per approver, got {first:?}"
        );
        for (index, methods) in first.iter().enumerate() {
            assert_eq!(
                methods.as_slice(),
                [(
                    state::MfaType::EmailOtp.as_wire().to_string(),
                    state::MfaSyncStatus::OutOfSync.as_wire().to_string(),
                )],
                "case {label} approver {index}: expected one unsynchronized EMAIL_OTP device, \
                 got {methods:?}"
            );
        }

        // A read reports the stored state; it never synchronizes a device.
        let second = reported_mfa_methods(&harness, &arn).await;
        assert_eq!(
            second, first,
            "case {label}: a second read must report the same devices, got {second:?}"
        );
    }
}

/// The modelled update action is applied: every approver on the team the update produces
/// is synchronized, and the team records a new version.
#[tokio::test]
async fn the_synchronize_action_puts_every_approver_in_sync_and_records_a_new_version() {
    /// One row: a patch body that names the action, and how many approvers the team has
    /// after it is applied.
    struct ActionCase {
        label: &'static str,
        patch: Value,
        approvers_after: usize,
    }

    let in_sync = state::MfaSyncStatus::InSync.as_wire().to_string();
    let device_type = state::MfaType::EmailOtp.as_wire().to_string();

    for approvers in [1, 2, MAX_APPROVERS] {
        let mut cases = vec![
            ActionCase {
                label: "the action alone",
                patch: json!({ "UpdateActions": [state::SYNCHRONIZE_MFA_DEVICES_ACTION] }),
                approvers_after: approvers,
            },
            ActionCase {
                label: "the action named twice",
                patch: json!({
                    "UpdateActions": [
                        state::SYNCHRONIZE_MFA_DEVICES_ACTION,
                        state::SYNCHRONIZE_MFA_DEVICES_ACTION,
                    ],
                }),
                approvers_after: approvers,
            },
            ActionCase {
                label: "the action beside a Description",
                patch: json!({
                    "Description": "synchronized",
                    "UpdateActions": [state::SYNCHRONIZE_MFA_DEVICES_ACTION],
                }),
                approvers_after: approvers,
            },
        ];
        if approvers < MAX_APPROVERS {
            // The action applies to the team the update PRODUCES, so an approver the
            // same request adds is synchronized too. Only rows below `Approvers`'
            // modelled maximum can add one.
            cases.push(ActionCase {
                label: "the action beside an Approvers that adds one",
                patch: json!({
                    "Approvers": (1..=approvers + 1).map(approver_member).collect::<Vec<_>>(),
                    "UpdateActions": [state::SYNCHRONIZE_MFA_DEVICES_ACTION],
                }),
                approvers_after: approvers + 1,
            });
        }

        for (index, case) in cases.into_iter().enumerate() {
            let label = format!("{approvers} approvers, {}", case.label);
            // A fresh team per row, so each row starts from unsynchronized devices.
            let harness = Harness::new();
            let (arn, created_version) =
                create_team(&harness, &format!("mfa-sync-{approvers}-{index}"), approvers).await;

            let (status, updated) = harness.update(&arn, &case.patch).await;
            assert_eq!(status, 200, "case {label}: update returned {updated:?}");
            let version = text(&updated, "VersionId");
            assert_ne!(
                version, created_version,
                "case {label}: an applied update action must record a NEW VersionId"
            );

            let methods = reported_mfa_methods(&harness, &arn).await;
            assert_eq!(
                methods.len(),
                case.approvers_after,
                "case {label}: expected {} approver entries, got {methods:?}",
                case.approvers_after
            );
            for (approver_index, devices) in methods.iter().enumerate() {
                assert_eq!(
                    devices.as_slice(),
                    [(device_type.clone(), in_sync.clone())],
                    "case {label} approver {approver_index}: expected a synchronized device, \
                     got {devices:?}"
                );
            }

            let (status, team) = harness.get(&arn).await;
            assert_eq!(status, 200, "case {label}: get returned {team:?}");
            assert_eq!(
                text(&team, "VersionId"),
                version,
                "case {label}: get must report the VersionId the update returned"
            );
        }
    }
}

/// An update that names no action leaves every device as it was, in both directions: it
/// neither synchronizes an unsynchronized device nor unsynchronizes a synchronized one.
/// A later update that ADDS an approver brings on one whose devices are not synchronized,
/// while the approvers it keeps stay synchronized — the state is per approver, not per team.
#[tokio::test]
async fn an_update_that_names_no_action_leaves_every_device_as_it_was() {
    let out_of_sync = state::MfaSyncStatus::OutOfSync.as_wire().to_string();
    let in_sync = state::MfaSyncStatus::InSync.as_wire().to_string();
    let device_type = state::MfaType::EmailOtp.as_wire().to_string();

    // `UpdateActions` is `length(min: 0, max: 100)`, so an empty list is a legal request
    // that names no action; a member absent from the body is the other form.
    let no_action_patches: Vec<(&str, Value)> = vec![
        ("UpdateActions absent", json!({ "Description": "unchanged devices" })),
        ("UpdateActions empty", json!({ "UpdateActions": [] })),
        (
            "an empty UpdateActions beside other members",
            json!({
                "UpdateActions": [],
                "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 1 } },
            }),
        ),
    ];

    for (index, (label, patch)) in no_action_patches.iter().enumerate() {
        let harness = Harness::new();
        let (arn, _) = create_team(&harness, &format!("mfa-noop-{index}"), 2).await;

        let (status, updated) = harness.update(&arn, patch).await;
        assert_eq!(status, 200, "case {label}: update returned {updated:?}");
        let methods = reported_mfa_methods(&harness, &arn).await;
        for (approver_index, devices) in methods.iter().enumerate() {
            assert_eq!(
                devices.as_slice(),
                [(device_type.clone(), out_of_sync.clone())],
                "case {label} approver {approver_index}: an update naming no action must leave \
                 the device unsynchronized, got {devices:?}"
            );
        }
    }

    // Synchronize, then update again without the action: the devices stay synchronized.
    let harness = Harness::new();
    let (arn, _) = create_team(&harness, "mfa-sticky", 2).await;
    let (status, updated) = harness
        .update(
            &arn,
            &json!({ "UpdateActions": [state::SYNCHRONIZE_MFA_DEVICES_ACTION] }),
        )
        .await;
    assert_eq!(status, 200, "case sticky: synchronize returned {updated:?}");
    let (status, updated) = harness
        .update(&arn, &json!({ "Description": "after the synchronization" }))
        .await;
    assert_eq!(status, 200, "case sticky: update returned {updated:?}");
    let methods = reported_mfa_methods(&harness, &arn).await;
    for (approver_index, devices) in methods.iter().enumerate() {
        assert_eq!(
            devices.as_slice(),
            [(device_type.clone(), in_sync.clone())],
            "case sticky approver {approver_index}: a synchronized device stays synchronized, \
             got {devices:?}"
        );
    }

    // An approver a later update ADDS has not been synchronized; the kept one has.
    let (status, updated) = harness
        .update(
            &arn,
            &json!({ "Approvers": [approver_member(1), approver_member(9)] }),
        )
        .await;
    assert_eq!(status, 200, "case added approver: update returned {updated:?}");
    let methods = reported_mfa_methods(&harness, &arn).await;
    assert_eq!(
        methods.len(),
        2,
        "case added approver: expected two approver entries, got {methods:?}"
    );
    assert_eq!(
        methods[0].as_slice(),
        [(device_type.clone(), in_sync.clone())],
        "case added approver: the kept approver stays synchronized, got {:?}",
        methods[0]
    );
    assert_eq!(
        methods[1].as_slice(),
        [(device_type.clone(), out_of_sync.clone())],
        "case added approver: the added approver is not synchronized, got {:?}",
        methods[1]
    );
}

/// An `UpdateActions` entry that is not a token `UpdateAction` declares is refused with
/// `ValidationException` (400), and the team is left alone.
#[tokio::test]
async fn an_update_action_the_model_does_not_declare_is_refused() {
    let harness = Harness::new();
    let (arn, created_version) = create_team(&harness, "mfa-unknown-action", 2).await;

    // Each row is a token `UpdateAction` does not declare: a lowercase spelling, a
    // near-miss, the empty string, and a token from no enum at all.
    for token in [
        "synchronize_mfa_devices",
        "SYNCHRONIZE_MFA_DEVICE",
        "",
        "ROTATE_MFA_DEVICES",
    ] {
        assert!(
            !model::UPDATE_ACTION_VALUES.contains(&token),
            "case {token:?}: this row must name a token the model does NOT declare"
        );
        let (status, body) = harness
            .update(&arn, &json!({ "UpdateActions": [token] }))
            .await;
        assert_eq!(
            status, 400,
            "case {token:?}: an unmodelled update action must be refused, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {token:?}: wrong error code in {body:?}"
        );
    }

    let (status, team) = harness.get(&arn).await;
    assert_eq!(status, 200, "get returned {team:?}");
    assert_eq!(
        text(&team, "VersionId"),
        created_version,
        "a refused update must leave the version alone: {team:?}"
    );
    let methods = reported_mfa_methods(&harness, &arn).await;
    for (approver_index, devices) in methods.iter().enumerate() {
        assert_eq!(
            devices.as_slice(),
            [(
                state::MfaType::EmailOtp.as_wire().to_string(),
                state::MfaSyncStatus::OutOfSync.as_wire().to_string(),
            )],
            "approver {approver_index}: a refused update synchronizes nothing, got {devices:?}"
        );
    }
}
