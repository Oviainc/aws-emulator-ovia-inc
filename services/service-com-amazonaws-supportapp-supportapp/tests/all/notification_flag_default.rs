// SPDX-License-Identifier: CC0-1.0

//! The one documented default for the three notification flags, driven through BOTH
//! operations that read them.
//!
//! `notifyOnCaseSeverity` states it on the create request — "If you don't specify these
//! parameters in your request, they default to false."
//! (`smithy_query(query="doc", shape="CreateSlackChannelConfigurationRequest", member="notifyOnCaseSeverity",
//! model_path="services/service-com-amazonaws-supportapp-supportapp/smithy-model/com-amazonaws-supportapp-supportapp.json")`)
//! — and the `none` clause of the same rule admits exactly that: a flag outside the
//! scope the clause reads is "null or false".
//!
//! So an absent flag counts as `false` wherever a flag is read:
//!
//! * `CreateSlackChannelConfiguration` persists `false` for each flag its request does
//!   not carry, which is what every later read reports.
//! * `UpdateSlackChannelConfiguration` evaluates the `none` clause over the flags the
//!   REQUEST carries, where a flag the request omits is likewise not `true`.
//!
//! The table below sweeps the ways a flag reaches a request — omitted, explicitly
//! `false`, explicitly `true` — across all three members, each row driving a create and
//! then an update over the created record, and reads every accepted write back through
//! `ListSlackChannelConfigurations`. Every assertion goes over the wire through the
//! production router (the harness in [`crate::slack_channel_configurations`]), so a
//! default that is answered but not persisted cannot pass.

use serde_json::{json, Value};

use crate::slack_channel_configurations::{role_arn, Emulator};

/// The three notification flags, in the order this API's prose lists them
/// (`smithy_query(query="operation", operation="CreateSlackChannelConfiguration")`).
const FLAG_MEMBERS: [&str; 3] = [
    "notifyOnCreateOrReopenCase",
    "notifyOnAddCorrespondenceToCase",
    "notifyOnResolveCase",
];

const TEAM_ID: &str = "T012ABCDEFG";
const CHANNEL_ID: &str = "C01234A5BCD";

/// One row: how each of the three flags reaches the request — `None` for an omitted
/// member, `Some(false)` / `Some(true)` for one that is carried.
#[derive(Debug, Clone, Copy)]
struct Row {
    label: &'static str,
    flags: [Option<bool>; 3],
}

/// The rows the sweep runs over. They differ along what the rule generalises over:
/// WHICH of the three members is carried, HOW MANY are carried, and whether a carried
/// one is `true` or `false` — including the all-absent row, where the default is the
/// only thing that decides the stored record.
const ROWS: [Row; 10] = [
    Row {
        label: "every flag absent",
        flags: [None, None, None],
    },
    Row {
        label: "only create-or-reopen, true",
        flags: [Some(true), None, None],
    },
    Row {
        label: "only add-correspondence, true",
        flags: [None, Some(true), None],
    },
    Row {
        label: "only resolve, true",
        flags: [None, None, Some(true)],
    },
    Row {
        label: "only create-or-reopen, false",
        flags: [Some(false), None, None],
    },
    Row {
        label: "only add-correspondence, false",
        flags: [None, Some(false), None],
    },
    Row {
        label: "only resolve, false",
        flags: [None, None, Some(false)],
    },
    Row {
        label: "two carried true, one absent",
        flags: [Some(true), None, Some(true)],
    },
    Row {
        label: "every flag carried false",
        flags: [Some(false), Some(false), Some(false)],
    },
    Row {
        label: "every flag carried true",
        flags: [Some(true), Some(true), Some(true)],
    },
];

impl Row {
    /// The booleans the default rule yields for this row: a carried value, else `false`.
    fn defaulted(&self) -> [bool; 3] {
        [
            self.flags[0].unwrap_or(false),
            self.flags[1].unwrap_or(false),
            self.flags[2].unwrap_or(false),
        ]
    }

    /// Whether this row asks for any notification at all.
    fn any_true(&self) -> bool {
        self.flags.contains(&Some(true))
    }

    /// The severity the row is created with: the `all`/`high` clause needs at least one
    /// flag `true`, and the `none` clause needs none of them, so the row's own flags
    /// pick the severity that admits them.
    fn create_severity(&self) -> &'static str {
        if self.any_true() {
            "all"
        } else {
            "none"
        }
    }

    /// A create request carrying exactly the members this row names.
    fn create_body(&self) -> Value {
        let mut body = json!({
            "teamId": TEAM_ID,
            "channelId": CHANNEL_ID,
            "notifyOnCaseSeverity": self.create_severity(),
            "channelRoleArn": role_arn("SupportAppRole"),
        });
        self.add_flags(&mut body);
        body
    }

    /// An update to severity `none` carrying exactly the members this row names — the
    /// scope the update's `none` clause is worded over.
    fn update_to_none_body(&self) -> Value {
        let mut body = json!({
            "teamId": TEAM_ID,
            "channelId": CHANNEL_ID,
            "notifyOnCaseSeverity": "none",
        });
        self.add_flags(&mut body);
        body
    }

    fn add_flags(&self, body: &mut Value) {
        for (member, flag) in FLAG_MEMBERS.into_iter().zip(self.flags) {
            if let Some(value) = flag {
                body[member] = json!(value);
            }
        }
    }
}

/// Assert `reported` carries `severity` and the three `flags`.
fn assert_reported(case: &str, reported: &Value, severity: &str, flags: [bool; 3]) {
    assert_eq!(
        reported.get("notifyOnCaseSeverity").and_then(Value::as_str),
        Some(severity),
        "case {case:?}: expected severity {severity:?}, got {reported:?}"
    );
    for (member, flag) in FLAG_MEMBERS.into_iter().zip(flags) {
        assert_eq!(
            reported.get(member).and_then(Value::as_bool),
            Some(flag),
            "case {case:?}: expected {member} to be {flag}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn an_absent_notification_flag_counts_as_false_on_create_and_on_update() {
    for row in ROWS {
        let case = format!(
            "{label}: flags {flags:?}",
            label = row.label,
            flags = row.flags
        );
        let emulator = Emulator::new();

        // Create: the absent members take the documented default, so the stored record
        // holds a boolean for each of the three.
        let created = emulator
            .create_in_registered_workspace(row.create_body())
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected create to answer HTTP 200, got {} / {}",
            created.status, created.body
        );
        let listed = emulator
            .listed(TEAM_ID, CHANNEL_ID)
            .await
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected the listing to report the created configuration")
            });
        assert_reported(&case, &listed, row.create_severity(), row.defaulted());

        // Update to `none`: the clause reads only the flags the REQUEST carries, where
        // an absent one is not `true` — so the row is refused exactly when it asks for
        // a notification.
        let reply = emulator.update(row.update_to_none_body()).await;
        let expected_status = if row.any_true() { 400 } else { 200 };
        assert_eq!(
            reply.status, expected_status,
            "case {case:?}: expected the update to none to answer HTTP {expected_status}, \
             got {} / {}",
            reply.status, reply.body
        );

        if row.any_true() {
            assert_eq!(
                reply.error_type, "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                reply.error_type
            );
            // A refused update writes nothing: the created record stands, defaults and
            // all.
            let after = emulator
                .listed(TEAM_ID, CHANNEL_ID)
                .await
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected the configuration to survive the refusal")
                });
            assert_reported(&case, &after, row.create_severity(), row.defaulted());
        } else {
            // Accepted: every flag the record holds is `false`, whether it was carried
            // as `false` or defaulted, so moving to `none` keeps the same booleans.
            assert_reported(&case, &reply.body, "none", row.defaulted());
            let after = emulator
                .listed(TEAM_ID, CHANNEL_ID)
                .await
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected the listing to report the updated record")
                });
            assert_reported(&case, &after, "none", row.defaulted());
        }
    }
}
