// SPDX-License-Identifier: CC0-1.0

//! The vended CloudWatch Logs log group an app monitor with `CwLogEnabled` advertises.
//!
//! The rule under test: when a call turns CloudWatch Logs delivery on, RUM provisions the
//! log group it then reports as `DataStorage.CwLog.CwLogGroup`, so the name a client reads
//! out of `GetAppMonitor` names a REAL CloudWatch Logs log group —
//! `logs:DescribeLogGroups` with that name as the prefix finds a `logGroupName` equal to
//! it, without the client ever calling `logs:CreateLogGroup`. A monitor with delivery off
//! advertises no group and provisions none.
//!
//! The rows span what the rule generalises over: which call turns delivery on (the create
//! or a later update), the app monitor's name (which the group's name embeds), and whether
//! delivery is on at all. The NAME the group carries is pinned by
//! `vended_cw_log_group.rs`.
//!
//! The second rule, over the same side effect: when the caller may NOT have RUM install
//! that group's delivery policy, RUM refuses the call in its OWN words — the operation it
//! could not carry out and the app monitor the request named — rather than in the IAM
//! projection of an action the caller never asked for, whichever handler was asked to turn
//! delivery on and however the permission is withheld.

use serde_json::{json, Value};

use crate::authorization::{identity_policy, statement, Emulator, ACCOUNT_KEY};

/// The prefix a RUM-managed log group's name carries: RUM is a vended-logs service.
const CW_LOG_GROUP_PREFIX: &str = "/aws/vendedlogs/RUMService_";

/// The `CwLog` structure `GetAppMonitor` reports for `name`.
async fn cw_log(emulator: &Emulator, case: &str, name: &str) -> Value {
    let reply = emulator
        .rum(ACCOUNT_KEY, "GET", &format!("/appmonitor/{name}"), None)
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected GetAppMonitor to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    reply.json(case)["AppMonitor"]["DataStorage"]["CwLog"].clone()
}

/// The `logGroupName`s `logs:DescribeLogGroups` reports for a prefix.
async fn log_groups_under(emulator: &Emulator, case: &str, prefix: &str) -> Vec<String> {
    let (status, body) = emulator
        .logs(
            "DescribeLogGroups",
            &json!({"logGroupNamePrefix": prefix}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeLogGroups to answer 200, got {status} {body}"
    );
    body["logGroups"]
        .as_array()
        .map(|groups| {
            groups
                .iter()
                .filter_map(|group| group["logGroupName"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// How delivery gets turned on for a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TurnedOnBy {
    /// `CreateAppMonitor` already carries `CwLogEnabled: true`.
    TheCreate,
    /// `CreateAppMonitor` leaves it off and a later `UpdateAppMonitor` turns it on.
    AnUpdate,
    /// It is never turned on.
    Nothing,
}

#[tokio::test]
async fn turning_cw_log_delivery_on_provisions_the_group_the_app_monitor_advertises() {
    let rows: [(&str, &str, TurnedOnBy); 4] = [
        (
            "the create carries CwLogEnabled",
            "vended-at-create",
            TurnedOnBy::TheCreate,
        ),
        (
            "a later update turns delivery on",
            "vended-by-update",
            TurnedOnBy::AnUpdate,
        ),
        (
            // The group's name embeds the monitor's, so a name carrying the other
            // characters `AppMonitorName` admits must still resolve.
            "a monitor whose name carries the other characters the model admits",
            "vended_by-update.2",
            TurnedOnBy::AnUpdate,
        ),
        (
            "delivery is never turned on",
            "no-delivery",
            TurnedOnBy::Nothing,
        ),
    ];

    for (case, monitor, turned_on_by) in rows {
        let emulator = Emulator::new();
        let created = emulator
            .rum(
                ACCOUNT_KEY,
                "POST",
                "/appmonitor",
                Some(json!({
                    "Name": monitor,
                    "Domain": "example.com",
                    "CwLogEnabled": turned_on_by == TurnedOnBy::TheCreate,
                })),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateAppMonitor to answer 200, got {} {:?}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        if turned_on_by == TurnedOnBy::AnUpdate {
            let updated = emulator
                .rum(
                    ACCOUNT_KEY,
                    "PATCH",
                    &format!("/appmonitor/{monitor}"),
                    Some(json!({"CwLogEnabled": true})),
                )
                .await;
            assert_eq!(
                updated.status, 200,
                "case {case:?}: expected UpdateAppMonitor to answer 200, got {} {:?}",
                updated.status,
                String::from_utf8_lossy(&updated.body)
            );
        }

        let reported = cw_log(&emulator, case, monitor).await;
        if turned_on_by == TurnedOnBy::Nothing {
            assert_eq!(
                reported["CwLogEnabled"].as_bool(),
                Some(false),
                "case {case:?}: expected CwLogEnabled false, got {reported}"
            );
            assert!(
                reported["CwLogGroup"].is_null(),
                "case {case:?}: a monitor that stores no copy advertises no group, got {reported}"
            );
            let groups = log_groups_under(&emulator, case, CW_LOG_GROUP_PREFIX).await;
            assert!(
                groups.is_empty(),
                "case {case:?}: no vended group may be provisioned for a monitor with \
                 delivery off, got {groups:?}"
            );
            continue;
        }

        assert_eq!(
            reported["CwLogEnabled"].as_bool(),
            Some(true),
            "case {case:?}: expected CwLogEnabled true, got {reported}"
        );
        let advertised = reported["CwLogGroup"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: a monitor with delivery on must advertise a group, got {reported}")
            })
            .to_string();
        assert!(
            advertised.starts_with(CW_LOG_GROUP_PREFIX),
            "case {case:?}: expected a vended-logs group name, got {advertised:?}"
        );

        // The claim the issue's check makes: the advertised name resolves in CloudWatch
        // Logs, found by prefix, with the `logGroupName` equal to it exactly.
        let groups = log_groups_under(&emulator, case, &advertised).await;
        assert!(
            groups.contains(&advertised),
            "case {case:?}: DescribeLogGroups must report a log group named {advertised:?}, \
             got {groups:?}"
        );

        // Reading the monitor again reports the same name, and provisioning is
        // idempotent: a second call that turns delivery on leaves exactly one group.
        let again = cw_log(&emulator, case, monitor).await;
        assert_eq!(
            again["CwLogGroup"].as_str(),
            Some(advertised.as_str()),
            "case {case:?}: the advertised group name must be stable across reads, got {again}"
        );
        let repeated = emulator
            .rum(
                ACCOUNT_KEY,
                "PATCH",
                &format!("/appmonitor/{monitor}"),
                Some(json!({"CwLogEnabled": true})),
            )
            .await;
        assert_eq!(
            repeated.status, 200,
            "case {case:?}: turning delivery on again must succeed, got {} {:?}",
            repeated.status,
            String::from_utf8_lossy(&repeated.body)
        );
        let groups = log_groups_under(&emulator, case, &advertised).await;
        assert_eq!(
            groups,
            vec![advertised.clone()],
            "case {case:?}: exactly one group may back the advertised name, got {groups:?}"
        );
    }
}

/// A create this service refuses provisions nothing: the group is created only once the
/// create is going to be stored, so a refused request leaves no group behind owning
/// nothing.
#[tokio::test]
async fn a_refused_create_provisions_no_log_group() {
    let emulator = Emulator::new();
    let monitor = "refused-create";
    // The name is taken by a monitor created with delivery OFF, so the second create is
    // refused with `ConflictException` while asking for delivery ON.
    let created = emulator
        .rum(
            ACCOUNT_KEY,
            "POST",
            "/appmonitor",
            Some(json!({"Name": monitor, "Domain": "example.com"})),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the first create must succeed, got {} {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );

    let case = "a second create under a name already in use, asking for delivery on";
    let refused = emulator
        .rum(
            ACCOUNT_KEY,
            "POST",
            "/appmonitor",
            Some(json!({
                "Name": monitor,
                "Domain": "example.com",
                "CwLogEnabled": true,
            })),
        )
        .await;
    assert_eq!(
        refused.status, 409,
        "case {case:?}: expected HTTP 409, got {} {:?}",
        refused.status,
        String::from_utf8_lossy(&refused.body)
    );
    assert_eq!(
        refused.error_code(case),
        "ConflictException",
        "case {case:?}: expected ConflictException, got {:?}",
        String::from_utf8_lossy(&refused.body)
    );
    let groups = log_groups_under(&emulator, case, CW_LOG_GROUP_PREFIX).await;
    assert!(
        groups.is_empty(),
        "case {case:?}: a refused create must provision no log group, got {groups:?}"
    );
    // And the monitor the name belongs to still has delivery off.
    let reported = cw_log(&emulator, case, monitor).await;
    assert_eq!(
        reported["CwLogEnabled"].as_bool(),
        Some(false),
        "case {case:?}: the stored monitor must be untouched, got {reported}"
    );
}

/// Which handler was asked to turn delivery on, and the word RUM's refusal names it by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AskedBy {
    /// `CreateAppMonitor` carrying `CwLogEnabled: true`.
    TheCreate,
    /// `UpdateAppMonitor` turning it on for a monitor that already exists.
    AnUpdate,
}

impl AskedBy {
    /// What the refused call was doing to the application, as RUM's sentence words it.
    ///
    /// Spelled out here rather than taken from the handler's own renderer: an expectation
    /// built by calling the code under test would hold whatever that code said.
    fn doing(self) -> &'static str {
        match self {
            Self::TheCreate => "creating",
            Self::AnUpdate => "updating",
        }
    }
}

/// How the caller's `logs:PutResourcePolicy` stands for a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogsPermission {
    /// Every RUM action, and nothing in the `logs` namespace.
    NotGranted,
    /// Every RUM action, with the CloudWatch Logs action explicitly denied.
    ExplicitlyDenied,
    /// Both held.
    Held,
}

impl LogsPermission {
    /// The caller's inline identity policy for this row.
    fn policy(self) -> String {
        match self {
            Self::NotGranted => identity_policy(&statement("Allow", "rum:*", "*")),
            Self::ExplicitlyDenied => identity_policy(&format!(
                "{},{}",
                statement("Allow", "rum:*", "*"),
                statement("Deny", "logs:PutResourcePolicy", "*")
            )),
            Self::Held => identity_policy(&format!(
                "{},{}",
                statement("Allow", "rum:*", "*"),
                statement("Allow", "logs:PutResourcePolicy", "*")
            )),
        }
    }
}

/// One condition, one vocabulary: the refusal a caller sees when it may not have RUM
/// install the vended log group's delivery policy is RUM's own sentence, naming what the
/// call was doing and the application it named — whichever of the two handlers that can
/// turn delivery on was asked, and however the permission is withheld. A refused call
/// changes nothing and provisions nothing.
#[tokio::test]
async fn refusing_cw_log_delivery_is_worded_the_same_by_both_handlers() {
    // (case, the monitor's name, which handler is asked, the caller's logs permission)
    let rows: [(&str, &str, AskedBy, LogsPermission); 6] = [
        (
            "a create asking for delivery from a caller granted no logs action",
            "worded-at-create",
            AskedBy::TheCreate,
            LogsPermission::NotGranted,
        ),
        (
            "an update asking for delivery from a caller granted no logs action",
            "worded-by-update",
            AskedBy::AnUpdate,
            LogsPermission::NotGranted,
        ),
        (
            // The name is what the sentence carries, so a name using the other
            // characters `AppMonitorName` admits must appear in it unchanged.
            "a create from a caller explicitly denied the logs action",
            "worded_at-create.2",
            AskedBy::TheCreate,
            LogsPermission::ExplicitlyDenied,
        ),
        (
            "an update from a caller explicitly denied the logs action",
            "worded_by-update.2",
            AskedBy::AnUpdate,
            LogsPermission::ExplicitlyDenied,
        ),
        (
            "a create from a caller holding both",
            "worded-allowed-create",
            AskedBy::TheCreate,
            LogsPermission::Held,
        ),
        (
            "an update from a caller holding both",
            "worded-allowed-update",
            AskedBy::AnUpdate,
            LogsPermission::Held,
        ),
    ];

    for (index, (case, monitor, asked_by, permission)) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        let key = emulator
            .assumed_role_key(&format!("cw-log-wording-{index}"), Some(&permission.policy()))
            .await;
        // An update needs a monitor to turn delivery on for; the create makes its own.
        if asked_by == AskedBy::AnUpdate {
            emulator.create_monitor(monitor).await;
        }
        let reply = match asked_by {
            AskedBy::TheCreate => {
                emulator
                    .rum(
                        &key,
                        "POST",
                        "/appmonitor",
                        Some(json!({
                            "Name": monitor,
                            "Domain": "example.com",
                            "CwLogEnabled": true,
                        })),
                    )
                    .await
            }
            AskedBy::AnUpdate => {
                emulator
                    .rum(
                        &key,
                        "PATCH",
                        &format!("/appmonitor/{monitor}"),
                        Some(json!({"CwLogEnabled": true})),
                    )
                    .await
            }
        };

        if permission == LogsPermission::Held {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: a caller holding the permission must be served, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            let reported = cw_log(&emulator, case, monitor).await;
            assert_eq!(
                reported["CwLogEnabled"].as_bool(),
                Some(true),
                "case {case:?}: expected delivery to be on, got {reported}"
            );
            continue;
        }

        assert_eq!(
            reply.status, 403,
            "case {case:?}: expected HTTP 403, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(case),
            "AccessDeniedException",
            "case {case:?}: expected AccessDeniedException, got {:?}",
            String::from_utf8_lossy(&reply.body)
        );
        let expected = format!("Error {} the application {monitor}", asked_by.doing());
        let message = reply.message(case);
        assert_eq!(
            message, expected,
            "case {case:?}: expected the refusal {expected:?}, got {message:?}"
        );
        // The same refusal, never the IAM projection of the CloudWatch Logs action the
        // caller never asked for.
        assert!(
            !message.contains("is not authorized to perform"),
            "case {case:?}: expected RUM's own wording, got {message:?}"
        );

        // Nothing was written and nothing was provisioned: a refused create left no
        // monitor at all, and a refused update left delivery off.
        let groups = log_groups_under(&emulator, case, CW_LOG_GROUP_PREFIX).await;
        assert!(
            groups.is_empty(),
            "case {case:?}: a refused call must provision no log group, got {groups:?}"
        );
        match asked_by {
            AskedBy::TheCreate => {
                let read = emulator
                    .rum(ACCOUNT_KEY, "GET", &format!("/appmonitor/{monitor}"), None)
                    .await;
                assert_eq!(
                    read.status, 404,
                    "case {case:?}: a refused create must store no monitor, got {} {:?}",
                    read.status,
                    String::from_utf8_lossy(&read.body)
                );
            }
            AskedBy::AnUpdate => {
                let reported = cw_log(&emulator, case, monitor).await;
                assert_eq!(
                    reported["CwLogEnabled"].as_bool(),
                    Some(false),
                    "case {case:?}: a refused update must leave delivery off, got {reported}"
                );
            }
        }
    }
}
