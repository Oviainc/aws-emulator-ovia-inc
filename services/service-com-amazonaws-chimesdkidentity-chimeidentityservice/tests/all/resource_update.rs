// SPDX-License-Identifier: CC0-1.0

//! Updating an `AppInstance` and an `AppInstanceUser`, end to end over HTTP.
//!
//! The RULE pinned here: an update REPLACES the stored `Name` and `Metadata` and stamps
//! the resource's last-updated instant at or after its created instant, which never
//! moves; and each resource reports a last-updated instant exactly where AWS reports one
//! — an `AppInstance` only once it has actually been updated, an `AppInstanceUser` from
//! the moment it is created.
//!
//! The table spans what the rule generalises over: the two updatable resource kinds of
//! this service, the value and size of the pair being written (short, empty metadata,
//! the model's own length bounds, non-ASCII text), and HOW MANY updates a resource has
//! had (never, once, three times).

use serde_json::{json, Value};

use crate::app_instance_lifecycle::{uri_encode, Emulator};

/// The two resources of this service an update operation writes to, each with the URI
/// space, the response member and the describe path the model binds for it.
///
/// Source: `smithy_query(query="operation",operation="UpdateAppInstance,UpdateAppInstanceUser")`
/// — `PUT /app-instances/{AppInstanceArn}` and `PUT /app-instance-users/{AppInstanceUserArn}`,
/// both answering 200 with the updated resource's ARN.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    AppInstance,
    AppInstanceUser,
}

impl Kind {
    /// The collection segment both the update and the describe route hang off.
    fn space(self) -> &'static str {
        match self {
            Self::AppInstance => "app-instances",
            Self::AppInstanceUser => "app-instance-users",
        }
    }

    /// The ARN member the update's response and the describe's body carry.
    fn arn_member(self) -> &'static str {
        match self {
            Self::AppInstance => "AppInstanceArn",
            Self::AppInstanceUser => "AppInstanceUserArn",
        }
    }

    /// The member the describe response wraps the resource in.
    fn described_member(self) -> &'static str {
        match self {
            Self::AppInstance => "AppInstance",
            Self::AppInstanceUser => "AppInstanceUser",
        }
    }

    /// Whether a resource of this kind already carries a last-updated instant before
    /// anything has updated it: an `AppInstanceUser` is stamped when it is created, an
    /// `AppInstance` only when it is updated.
    fn stamped_at_creation(self) -> bool {
        match self {
            Self::AppInstance => false,
            Self::AppInstanceUser => true,
        }
    }
}

/// `PUT` the resource's update route with the required `Name`/`Metadata` pair.
async fn update(
    emulator: &Emulator,
    kind: Kind,
    arn: &str,
    name: &str,
    metadata: &str,
) -> (u16, Value) {
    emulator
        .call(
            "PUT",
            &format!("/{}/{}", kind.space(), uri_encode(arn)),
            Some(json!({ "Name": name, "Metadata": metadata })),
        )
        .await
}

/// `GET` the resource and return the resource object the response wraps.
async fn describe(emulator: &Emulator, kind: Kind, arn: &str) -> (u16, Value) {
    let (status, body) = emulator
        .call("GET", &format!("/{}/{}", kind.space(), uri_encode(arn)), None)
        .await;
    let resource = body
        .get(kind.described_member())
        .cloned()
        .unwrap_or(Value::Null);
    (status, resource)
}

/// One row: the pair written by each successive update of one resource.
struct UpdateCase {
    label: &'static str,
    /// `(Name, Metadata)` per update, in order; an empty slice is a resource nobody
    /// updates.
    writes: Vec<(String, String)>,
}

fn cases() -> Vec<UpdateCase> {
    // The bounds come from the model: `Name` is `NonEmptyResourceName` (len 1..256) on
    // an `AppInstance` and `UserName` (len 1..100, pattern `\S`) on an
    // `AppInstanceUser`, so a row shared by both kinds stays within 100; `Metadata` is
    // len 0..1024. Source: `smithy_query(query="operation",
    // operation="UpdateAppInstance,UpdateAppInstanceUser")`.
    vec![
        UpdateCase {
            label: "never updated",
            writes: Vec::new(),
        },
        UpdateCase {
            label: "one short update",
            writes: vec![("renamed".to_string(), "m1".to_string())],
        },
        UpdateCase {
            label: "metadata emptied",
            writes: vec![("still-named".to_string(), String::new())],
        },
        UpdateCase {
            label: "at the length bounds",
            writes: vec![("n".repeat(100), "d".repeat(1024))],
        },
        UpdateCase {
            label: "non-ASCII text, three times over",
            writes: vec![
                ("café-一".to_string(), "métadonnées".to_string()),
                ("café-二".to_string(), "メタデータ".to_string()),
                ("café-三".to_string(), "m3".to_string()),
            ],
        },
    ]
}

/// RULE: an update replaces the stored `Name`/`Metadata` and stamps a last-updated
/// instant at or after the created instant, which no update ever moves; before any
/// update, a last-updated instant is reported exactly for the kinds AWS stamps at
/// creation.
#[tokio::test]
async fn an_update_replaces_the_stored_pair_and_stamps_a_last_updated_instant() {
    let emulator = Emulator::new();

    for kind in [Kind::AppInstance, Kind::AppInstanceUser] {
        for (index, case) in cases().into_iter().enumerate() {
            let label = case.label;
            let token = format!("tok-upd-{kind:?}-{index}");
            let instance = emulator.app_instance("update-parent", &token).await;
            let arn = match kind {
                Kind::AppInstance => instance,
                Kind::AppInstanceUser => {
                    emulator
                        .user(&instance, &format!("u{index}"), &format!("{token}-user"))
                        .await
                }
            };

            let (status, before) = describe(&emulator, kind, &arn).await;
            assert_eq!(
                status, 200,
                "case {kind:?}/{label:?}: expected the fresh resource to describe, got \
                 {status} {before:?}"
            );
            let created = before["CreatedTimestamp"].as_f64();
            assert!(
                created.is_some(),
                "case {kind:?}/{label:?}: expected a created instant, got {before:?}"
            );
            let stamped_before = before["LastUpdatedTimestamp"].as_f64();
            assert_eq!(
                stamped_before.is_some(),
                kind.stamped_at_creation(),
                "case {kind:?}/{label:?}: expected a last-updated instant before any update \
                 = {}, got {stamped_before:?}",
                kind.stamped_at_creation()
            );

            let mut previous_update: Option<f64> = stamped_before;
            for (name, metadata) in &case.writes {
                let (status, body) = update(&emulator, kind, &arn, name, metadata).await;
                assert_eq!(
                    status, 200,
                    "case {kind:?}/{label:?}: expected the update to answer 200, got \
                     {status} {body:?}"
                );
                assert_eq!(
                    body[kind.arn_member()].as_str(),
                    Some(arn.as_str()),
                    "case {kind:?}/{label:?}: expected the update to answer with {arn:?}, \
                     got {body:?}"
                );

                let (status, after) = describe(&emulator, kind, &arn).await;
                assert_eq!(
                    status, 200,
                    "case {kind:?}/{label:?}: expected the updated resource to describe, \
                     got {status} {after:?}"
                );
                assert_eq!(
                    after["Name"].as_str(),
                    Some(name.as_str()),
                    "case {kind:?}/{label:?}: expected Name {name:?} after the update, got {:?}",
                    after["Name"]
                );
                assert_eq!(
                    after["Metadata"].as_str(),
                    Some(metadata.as_str()),
                    "case {kind:?}/{label:?}: expected Metadata {metadata:?} after the \
                     update, got {:?}",
                    after["Metadata"]
                );
                assert_eq!(
                    after["CreatedTimestamp"].as_f64(),
                    created,
                    "case {kind:?}/{label:?}: an update must not move the created instant \
                     {created:?}, got {:?}",
                    after["CreatedTimestamp"]
                );

                let updated = after["LastUpdatedTimestamp"].as_f64();
                assert!(
                    updated.is_some_and(|at| created.is_some_and(|born| at >= born)),
                    "case {kind:?}/{label:?}: expected a last-updated instant at or after \
                     the created instant {created:?}, got {updated:?}"
                );
                assert!(
                    previous_update.is_none_or(|before| {
                        updated.is_some_and(|after| after >= before)
                    }),
                    "case {kind:?}/{label:?}: expected the last-updated instant to never \
                     move backwards from {previous_update:?}, got {updated:?}"
                );
                previous_update = updated;
            }

            // A resource nobody wrote to still reports a last-updated instant only
            // where its kind is stamped at creation; one that was written to carries the
            // stamp of its last write.
            let (status, finally) = describe(&emulator, kind, &arn).await;
            let expected_stamp = kind.stamped_at_creation() || !case.writes.is_empty();
            assert_eq!(
                (status, finally["LastUpdatedTimestamp"].as_f64().is_some()),
                (200, expected_stamp),
                "case {kind:?}/{label:?}: expected a last-updated instant = \
                 {expected_stamp} after {} write(s), got {finally:?}",
                case.writes.len()
            );
        }
    }
}

/// RULE: an update keyed by an ARN this service does not hold changes nothing and is
/// refused with the modelled client error carrying the `NotFound` code.
#[tokio::test]
async fn updating_an_arn_the_service_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    let instance = emulator.app_instance("update-absent", "tok-upd-absent").await;
    let user = emulator.user(&instance, "present", "tok-upd-absent-user").await;

    // Rows: each kind addressed with an ARN of the right shape that names nothing, and
    // with the OTHER kind's live ARN — neither is a resource of the addressed space.
    let rows: Vec<(&str, Kind, String)> = vec![
        ("an app instance that never existed", Kind::AppInstance, format!("{instance}x")),
        ("a user that never existed", Kind::AppInstanceUser, format!("{user}x")),
        ("a user ARN in the app-instance space", Kind::AppInstance, user.clone()),
        (
            "an app instance ARN in the user space",
            Kind::AppInstanceUser,
            instance.clone(),
        ),
    ];

    for (label, kind, arn) in rows {
        let (status, body) = update(&emulator, kind, &arn, "rename", "md").await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400 for {arn:?}, got {status} {body:?}"
        );
        assert_eq!(
            body["Code"].as_str(),
            Some("NotFound"),
            "case {label:?}: expected the NotFound code, got {body:?}"
        );
    }

    // Nothing was renamed by the refusals: both live resources still carry what created
    // them, and neither has gained a last-updated instant it should not have.
    for (kind, arn, name) in [
        (Kind::AppInstance, instance.as_str(), "update-absent"),
        (Kind::AppInstanceUser, user.as_str(), "present"),
    ] {
        let (status, described) = describe(&emulator, kind, arn).await;
        assert_eq!(
            (status, described["Name"].as_str()),
            (200, Some(name)),
            "case {kind:?}: expected {arn:?} untouched by the refusals, got {described:?}"
        );
        assert_eq!(
            described["LastUpdatedTimestamp"].as_f64().is_some(),
            kind.stamped_at_creation(),
            "case {kind:?}: a refused update must leave the last-updated instant as \
             creation left it, got {described:?}"
        );
    }
}
