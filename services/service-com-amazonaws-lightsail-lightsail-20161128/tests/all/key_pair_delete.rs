// SPDX-License-Identifier: CC0-1.0

//! Removing a Lightsail SSH key pair — `DeleteKeyPair` — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rules under test:
//!
//! * Deleting a key pair the account holds removes THAT record — the public key
//!   the service holds under that name — from the service's state and leaves
//!   every other record untouched, and logs ONE terminal operation naming the
//!   removed record's own name, resource type and location. A name the account
//!   holds no key pair under removes nothing and is refused with the service's
//!   `DoesNotExist` envelope, so a second delete of the same name fails rather
//!   than reporting a deletion that did not happen.
//! * `expectedFingerprint` is a precondition for the account's DEFAULT key pair
//!   alone — "the `expectedFingerprint` parameter is required only when
//!   specifying to delete a Lightsail default key pair" — so a delete of that
//!   pair is served only when the member is present and is the fingerprint the
//!   service holds, while for every other key pair an absent, empty or
//!   disagreeing fingerprint is ignored and the delete is served.
//!
//! The rows below hold a key pair with a chosen id, instant and fingerprint
//! FORM — the MD5 colon-hex form and the `SHA256:` one — none of which a create
//! lets a caller pick: `CreateKeyPair` mints its own material, and
//! `ImportKeyPair` and `DownloadDefaultKeyPair` are still scaffold stubs with
//! issues of their own. So seeding the store the handlers read
//! (`state::with_state`) is how a case drives the account HOLDING the key pair a
//! delete names. Every assertion about what the delete did is made through the
//! wire — the operation it answered, the operation log, `GetActiveNames` and
//! `TagResource` — plus the store itself, which is what the removal is about.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::ops::types::Tag;
use service_com_amazonaws_lightsail_lightsail_20161128::state::{
    self, StoredKeyPair, StoredLocation, DEFAULT_SSH_KEY_NAME,
};

use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The Region the key pairs in this suite were created in.
///
/// Deliberately NOT the Region the calls below are made in ([`CALL_REGION`]): a
/// key pair is addressed by name alone, exactly like the instance whose
/// `sshKeyName` names one, so the delete must find it from any Region's endpoint
/// and the entry it logs must report the KEY PAIR's own location rather than the
/// request's.
const HELD_REGION: &str = "us-west-2";

/// The Region the calls in this suite are made in: the one a request carrying no
/// credential scope is served in (`crate::state::DEFAULT_REGION`).
const CALL_REGION: &str = "us-east-1";

/// One key pair the account holds.
///
/// The rows differ along everything the rule generalises over: the characters and
/// length of the name (letters, digits, hyphens, underscores, mixed case), how
/// many tags the record carries (none, one, two), and the FORM of the
/// fingerprint — the MD5 colon-hex form `ssh-keygen -E md5` prints and the
/// `SHA256:`-prefixed form it prints by default — because this operation
/// compares the fingerprint and never parses it.
struct Row {
    case: &'static str,
    key_pair_name: &'static str,
    fingerprint: &'static str,
    tags: &'static [(&'static str, &'static str)],
}

const ROWS: &[Row] = &[
    Row {
        case: "a hyphenated name, no tags, an MD5 colon-hex fingerprint",
        key_pair_name: "key-pair-alpha",
        fingerprint: "a6:1f:3c:d2:0b:8e:74:59:c1:3d:ef:90:12:ab:cd:34",
        tags: &[],
    },
    Row {
        case: "underscores and digits, one tag",
        key_pair_name: "kp_2026_build",
        fingerprint: "b7:20:4d:e3:1c:9f:85:6a:d2:4e:f0:01:23:bc:de:45",
        tags: &[("env", "staging")],
    },
    Row {
        case: "a short upper-case name, two tags, a SHA256 fingerprint",
        key_pair_name: "KP7",
        fingerprint: "SHA256:hXy0pZ9Qw3kLmNoPqRsTuVwXyZaBcDeFgHiJkLmNoPq",
        tags: &[("team", "ssh"), ("cost-centre", "8080")],
    },
    Row {
        case: "a long mixed name, one tag",
        key_pair_name: "sim-ls-kp-with-a-much-longer-name-9",
        fingerprint: "c8:31:5e:f4:2d:a0:96:7b:e3:5f:01:12:34:cd:ef:56",
        tags: &[("keep", "yes")],
    },
];

/// What a request may carry in `expectedFingerprint`.
///
/// The cases a non-default key pair must be deleted under regardless, and the
/// ones the account's default key pair must be refused under — one table, so the
/// two halves of the rule are exercised over exactly the same inputs.
#[derive(Clone, Copy, Debug)]
enum Fingerprint {
    /// The member is absent from the request.
    Absent,
    /// The member carries the fingerprint the service holds for that key pair.
    Held,
    /// The member carries another key pair's fingerprint.
    Another,
    /// The member carries the empty string.
    Empty,
    /// The member carries a value no key pair of the account has.
    Unknown,
}

/// Every form of the member a case drives.
const FINGERPRINTS: &[Fingerprint] = &[
    Fingerprint::Absent,
    Fingerprint::Held,
    Fingerprint::Another,
    Fingerprint::Empty,
    Fingerprint::Unknown,
];

impl Fingerprint {
    /// What this form puts in the request for the key pair whose fingerprint is
    /// `held`, or `None` when the member is absent.
    fn value(self, held: &str) -> Option<String> {
        match self {
            Fingerprint::Absent => None,
            Fingerprint::Held => Some(held.to_string()),
            Fingerprint::Another => Some(ROWS[0].fingerprint.to_string()),
            Fingerprint::Empty => Some(String::new()),
            Fingerprint::Unknown => {
                Some("00:11:22:33:44:55:66:77:88:99:aa:bb:cc:dd:ee:ff".to_string())
            }
        }
    }

    /// Whether this form states the fingerprint the service holds — the one
    /// precondition the account's default key pair is deleted under.
    fn matches_held(self, held: &str) -> bool {
        self.value(held).is_some_and(|value| value == held)
    }
}

/// The `DeleteKeyPair` body for `name`, carrying `fingerprint` only when the case
/// says the member is present.
fn delete_body(name: &str, fingerprint: Option<String>) -> Value {
    match fingerprint {
        Some(value) => json!({"keyPairName": name, "expectedFingerprint": value}),
        None => json!({"keyPairName": name}),
    }
}

/// Put the key pairs `rows` describes in the store, under the id, instant and
/// fingerprint form each row names.
///
/// The ids the ARN and support code are built from are this suite's own, so two
/// rows never share one; everything else is what the row says the account holds.
fn hold(emulator: &Emulator, rows: &[&Row]) {
    let held = state::with_state(emulator.engine(), |store| {
        for (index, row) in rows.iter().enumerate() {
            let id = format!("05859e3d-331d-48ba-9034-1234512345{index:02}");
            store.insert_key_pair(StoredKeyPair {
                name: row.key_pair_name.to_string(),
                arn: state::key_pair_arn(HELD_REGION, &id),
                support_code: state::support_code(&id),
                created_at_ms: 1_770_000_000_000 + index as i64,
                location: StoredLocation::regional(HELD_REGION),
                tags: row
                    .tags
                    .iter()
                    .map(|(key, value)| Tag {
                        key: Some((*key).to_string()),
                        value: Some((*value).to_string()),
                    })
                    .collect(),
                fingerprint: row.fingerprint.to_string(),
            });
        }
        store.key_pairs.len()
    });
    assert_eq!(
        held,
        Some(rows.len()),
        "case seeding {} key pairs: expected the store to hold them, got {held:?}",
        rows.len()
    );
}

/// The modelled projection of the key pair `name`, rendered so two reads can be
/// compared: the modelled `KeyPair` derives `Debug` but no equality, and what a
/// case about an untouched record needs is exactly "the same as before".
///
/// `None` when the account holds no key pair of that name — what a removal
/// leaves behind.
fn stored(emulator: &Emulator, name: &str) -> Option<String> {
    state::with_state(emulator.engine(), |store| {
        store
            .key_pair(name)
            .map(|held| format!("{:?}", held.view()))
    })
    .expect("the shared engine holds this service's store")
}

/// Every key pair name the store holds.
fn held_names(emulator: &Emulator) -> Vec<String> {
    state::with_state(emulator.engine(), |store| {
        store.key_pairs.keys().cloned().collect()
    })
    .expect("the shared engine holds this service's store")
}

/// Every active name the account holds, as `GetActiveNames` reports it in
/// [`CALL_REGION`], walking every page.
async fn active_names(emulator: &Emulator) -> Vec<String> {
    let mut names = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let body = match &token {
            Some(token) => {
                emulator
                    .call_in(
                        Some(CALL_REGION),
                        "GetActiveNames",
                        json!({"pageToken": token}),
                    )
                    .await
            }
            None => {
                emulator
                    .call_in(Some(CALL_REGION), "GetActiveNames", json!({}))
                    .await
            }
        };
        let (status, body) = body;
        assert_eq!(status, 200, "GetActiveNames must succeed, got {body:?}");
        let page = body["activeNames"].as_array().cloned().unwrap_or_default();
        for name in page {
            names.push(name.as_str().unwrap_or_default().to_string());
        }
        match body["nextPageToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    names
}

/// Assert the operation `body` answered is the terminal `DeleteKeyPair` entry of
/// `name`, and answer its id.
fn assert_delete_operation(case: &str, name: &str, body: &Value) -> String {
    let operation = &body["operation"];
    assert_eq!(
        operation["operationType"].as_str(),
        Some("DeleteKeyPair"),
        "case {case}: expected the DeleteKeyPair operation type, got {operation:?}"
    );
    assert_eq!(
        operation["resourceType"].as_str(),
        Some("KeyPair"),
        "case {case}: expected the KeyPair resource type, got {operation:?}"
    );
    assert_eq!(
        operation["resourceName"].as_str(),
        Some(name),
        "case {case}: expected the operation to name the deleted key pair, got {operation:?}"
    );
    assert_eq!(
        operation["location"]["regionName"].as_str(),
        Some(HELD_REGION),
        "case {case}: expected the key pair's own region on the log entry, got {:?}",
        operation["location"]
    );
    assert!(
        operation["location"]["availabilityZone"].is_null(),
        "case {case}: a key pair is not zonal here, so no availability zone may be \
         reported, got {:?}",
        operation["location"]
    );
    assert_eq!(
        operation["status"].as_str(),
        Some("Succeeded"),
        "case {case}: the public key is gone as the call returns, so expected a Succeeded \
         operation, got {operation:?}"
    );
    assert_eq!(
        operation["isTerminal"].as_bool(),
        Some(true),
        "case {case}: expected a terminal operation, got {operation:?}"
    );
    assert!(
        operation["createdAt"].as_f64().unwrap_or_default() > 0.0,
        "case {case}: expected a creation timestamp, got {operation:?}"
    );
    let id = operation["id"].as_str().unwrap_or_default().to_string();
    assert!(
        is_uuid_shaped(&id),
        "case {case}: expected a uuid-shaped operation id, got {id:?}"
    );
    id
}

// ---------------------------------------------------------------------------
// The delete table
// ---------------------------------------------------------------------------

/// RULE: `DeleteKeyPair` removes the public key the account holds under the name
/// it is given and logs ONE terminal `DeleteKeyPair` operation naming that key
/// pair, its resource type and its location — and for a key pair that is not the
/// account's default, `expectedFingerprint` is not a precondition, so the delete
/// is served whether the member is absent, empty, another key pair's or a value
/// no key pair has.
#[tokio::test]
async fn deleting_a_key_pair_removes_its_public_key_whatever_fingerprint_is_stated() {
    for fingerprint in FINGERPRINTS {
        // ONE emulator per fingerprint form, holding every row: the operation
        // ids below are minted by one store's own allocator, so they are
        // comparable with each other.
        let emulator = Emulator::new();
        let rows: Vec<&Row> = ROWS.iter().collect();
        hold(&emulator, &rows);
        let mut operation_ids: BTreeSet<String> = BTreeSet::new();
        let mut remaining: Vec<&Row> = rows.clone();

        for row in &rows {
            let case = format!("{} with {fingerprint:?}", row.case);
            // The record really is there before the delete, so the assertions
            // below are about a removal.
            let before = stored(&emulator, row.key_pair_name);
            assert!(
                before.is_some(),
                "case {case}: expected the account to hold the key pair before the delete"
            );
            let untouched: Vec<(&str, Option<String>)> = remaining
                .iter()
                .filter(|other| other.key_pair_name != row.key_pair_name)
                .map(|other| (other.key_pair_name, stored(&emulator, other.key_pair_name)))
                .collect();

            let (status, body) = emulator
                .call_in(
                    Some(CALL_REGION),
                    "DeleteKeyPair",
                    delete_body(row.key_pair_name, fingerprint.value(row.fingerprint)),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case}: expectedFingerprint is not a precondition for a key pair that \
                 is not the account's default, so the delete must be served, got {status} \
                 {body:?}"
            );
            let operation_id = assert_delete_operation(&case, row.key_pair_name, &body);
            assert!(
                operation_ids.insert(operation_id.clone()),
                "case {case}: expected a fresh operation id, got the already-used \
                 {operation_id:?}"
            );

            // The entry is in the log, not just in the response.
            let (log_status, logged) = emulator
                .call_in(
                    Some(CALL_REGION),
                    "GetOperation",
                    json!({"operationId": operation_id}),
                )
                .await;
            assert_eq!(
                log_status, 200,
                "case {case}: expected the logged operation to be readable, got {logged:?}"
            );
            assert_eq!(
                logged["operation"], body["operation"],
                "case {case}: expected the log to hold the operation the delete answered, got \
                 {logged:?}"
            );
            let (for_resource_status, for_resource) = emulator
                .call_in(
                    Some(CALL_REGION),
                    "GetOperationsForResource",
                    json!({"resourceName": row.key_pair_name}),
                )
                .await;
            assert_eq!(
                for_resource_status, 200,
                "case {case}: expected the key pair's own log slice to be readable, got \
                 {for_resource:?}"
            );
            assert_eq!(
                for_resource["operations"].as_array().map(Vec::len),
                Some(1),
                "case {case}: expected exactly the delete against this key pair, got \
                 {for_resource:?}"
            );

            // The public key is gone from the store.
            assert_eq!(
                stored(&emulator, row.key_pair_name),
                None,
                "case {case}: expected the key pair to be gone from the store"
            );

            // Every other key pair is exactly as it was.
            for (name, snapshot) in untouched {
                assert_eq!(
                    stored(&emulator, name),
                    snapshot,
                    "case {case}: expected '{name}' to be untouched by the delete"
                );
            }

            remaining.retain(|other| other.key_pair_name != row.key_pair_name);
            let reported = active_names(&emulator).await;
            assert!(
                !reported.contains(&row.key_pair_name.to_string()),
                "case {case}: expected the deleted key pair to stop being an active name of \
                 the account, got {reported:?}"
            );
            // A key pair is addressed by name alone, so the ones still held are
            // the account's resources whichever Region the request is made in —
            // here a Region none of them was created in.
            for other in &remaining {
                assert!(
                    reported.contains(&other.key_pair_name.to_string()),
                    "case {case}: expected '{}' to still be an active name, got {reported:?}",
                    other.key_pair_name
                );
            }
        }

        assert_eq!(
            held_names(&emulator),
            Vec::<String>::new(),
            "case every row deleted with {fingerprint:?}: expected the store to hold no key \
             pair"
        );
    }
}

/// RULE: the delete removes the record, so the key pair stops being a resource of
/// the account: its name is free, and an operation that resolves a resource by
/// name — a tag change — no longer finds it.
#[tokio::test]
async fn a_deleted_key_pair_is_no_longer_a_resource_of_the_account() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::new();
        hold(&emulator, &[row]);

        // While the account holds it, the key pair is a tag-carrying resource of
        // the account: the tag change resolves it and the tag lands on the
        // record.
        let (tag_status, tagged) = emulator
            .call_in(
                Some(CALL_REGION),
                "TagResource",
                json!({
                    "resourceName": row.key_pair_name,
                    "tags": [{"key": "owner", "value": "ssh-team"}],
                }),
            )
            .await;
        assert_eq!(
            tag_status, 200,
            "case {case}: expected the held key pair to be taggable, got {tag_status} \
             {tagged:?}"
        );
        assert_eq!(
            tagged["operations"][0]["resourceType"].as_str(),
            Some("KeyPair"),
            "case {case}: expected the tag change to log against the key pair, got {tagged:?}"
        );
        let after_tag = stored(&emulator, row.key_pair_name).unwrap_or_default();
        assert!(
            after_tag.contains("ssh-team"),
            "case {case}: expected the new tag on the stored key pair, got {after_tag}"
        );

        let (status, body) = emulator
            .call_in(
                Some(CALL_REGION),
                "DeleteKeyPair",
                delete_body(row.key_pair_name, None),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: the delete must be served, got {status} {body:?}"
        );

        let (tag_status, refused) = emulator
            .call_in(
                Some(CALL_REGION),
                "TagResource",
                json!({
                    "resourceName": row.key_pair_name,
                    "tags": [{"key": "owner", "value": "ssh-team"}],
                }),
            )
            .await;
        assert_eq!(
            tag_status, 400,
            "case {case}: expected the deleted key pair to resolve to no resource, got \
             {tag_status} {refused:?}"
        );
        assert_eq!(
            refused["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: expected code DoesNotExist for the deleted key pair, got {refused:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The default key pair
// ---------------------------------------------------------------------------

/// The account's Lightsail default key pair, named as the modelled
/// `Instance$sshKeyName` documentation names it
/// (`crate::state::DEFAULT_SSH_KEY_NAME`).
const DEFAULT_ROW: Row = Row {
    case: "the account's Lightsail default key pair",
    key_pair_name: DEFAULT_SSH_KEY_NAME,
    fingerprint: "d9:42:6f:05:3e:b1:a7:8c:f4:60:12:23:45:de:f0:67",
    tags: &[("purpose", "default")],
};

/// RULE: `expectedFingerprint` is a precondition for the account's DEFAULT key
/// pair alone, so a delete of that pair is served only when the member is present
/// and states the fingerprint the service holds; every other form is refused with
/// the modelled `InvalidInputException` and the public key is still there
/// afterwards.
#[tokio::test]
async fn deleting_the_default_key_pair_requires_the_fingerprint_the_service_holds() {
    for fingerprint in FINGERPRINTS {
        let emulator = Emulator::new();
        // The default pair beside one of the account's own, so each case also
        // shows the refusal removes neither.
        hold(&emulator, &[&DEFAULT_ROW, &ROWS[1]]);
        let case = format!("deleting the default key pair with {fingerprint:?}");
        let before = stored(&emulator, DEFAULT_ROW.key_pair_name);
        let neighbour_before = stored(&emulator, ROWS[1].key_pair_name);

        let (status, body) = emulator
            .call_in(
                Some(CALL_REGION),
                "DeleteKeyPair",
                delete_body(
                    DEFAULT_ROW.key_pair_name,
                    fingerprint.value(DEFAULT_ROW.fingerprint),
                ),
            )
            .await;

        if fingerprint.matches_held(DEFAULT_ROW.fingerprint) {
            assert_eq!(
                status, 200,
                "case {case}: the stated fingerprint is the one the service holds, so the \
                 delete must be served, got {status} {body:?}"
            );
            assert_delete_operation(&case, DEFAULT_ROW.key_pair_name, &body);
            assert_eq!(
                stored(&emulator, DEFAULT_ROW.key_pair_name),
                None,
                "case {case}: expected the default key pair to be gone from the store"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case}: expected a 400 refusal, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidInputException",
                "case {case}: expected the modelled InvalidInputException, got {body:?}"
            );
            assert!(
                body["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("expectedFingerprint")),
                "case {case}: expected the refusal to name the member that is wrong, got \
                 {body:?}"
            );
            assert!(
                body["operation"].is_null(),
                "case {case}: a refused delete logs no operation, got {body:?}"
            );
            assert_eq!(
                stored(&emulator, DEFAULT_ROW.key_pair_name),
                before,
                "case {case}: expected the refused delete to leave the default key pair \
                 untouched"
            );
        }

        assert_eq!(
            stored(&emulator, ROWS[1].key_pair_name),
            neighbour_before,
            "case {case}: expected the account's other key pair to be untouched"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// RULE: a name the account holds no key pair under removes nothing and is
/// refused with the service's `DoesNotExist` envelope — HTTP 400,
/// `NotFoundException`, body code `DoesNotExist` and a message naming the
/// resource type and the requested name — never a 501, never a hollow 200.
#[tokio::test]
async fn deleting_a_key_pair_the_account_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    let kept = &ROWS[0];
    hold(&emulator, &[kept]);
    // An instance of the account, so a case can show the lookup is the key
    // pair's own: a name another kind of resource answers to is not a key pair
    // name.
    let instance_name = "key-pair-suite-instance";
    let (instance_status, created) = emulator
        .call_in(
            Some(CALL_REGION),
            "CreateInstances",
            json!({
                "instanceNames": [instance_name],
                "availabilityZone": "us-east-1a",
                "blueprintId": "amazon_linux_2023",
                "bundleId": "micro_3_0",
            }),
        )
        .await;
    assert_eq!(
        instance_status, 200,
        "the suite's instance must be created, got {created:?}"
    );
    let before = stored(&emulator, kept.key_pair_name);

    // Each case is a name no key pair of this account answers to: one never
    // held, one differing only in case, one that is the held name with a suffix,
    // the account's default key pair while it holds none, and the name of a
    // resource of another kind.
    for name in [
        "absent-key-pair",
        "KEY-PAIR-ALPHA",
        "key-pair-alpha-2",
        DEFAULT_SSH_KEY_NAME,
        instance_name,
    ] {
        for fingerprint in FINGERPRINTS {
            let case = format!("deleting {name:?} with {fingerprint:?}");
            let (status, body) = emulator
                .call_in(
                    Some(CALL_REGION),
                    "DeleteKeyPair",
                    delete_body(name, fingerprint.value(kept.fingerprint)),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case}: expected a 400 refusal, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "NotFoundException",
                "case {case}: expected the modelled NotFoundException, got {body:?}"
            );
            assert_eq!(
                body["code"].as_str(),
                Some("DoesNotExist"),
                "case {case}: expected code DoesNotExist, got {body:?}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(format!("The KeyPair does not exist: {name}").as_str()),
                "case {case}: expected the resource-type/name message, got {body:?}"
            );
            assert!(
                body["operation"].is_null(),
                "case {case}: a refused delete logs no operation, got {body:?}"
            );
            assert_eq!(
                stored(&emulator, kept.key_pair_name),
                before,
                "case {case}: expected the refusal to remove nothing"
            );
        }
    }

    // The instance the suite created is still the account's: a refused key pair
    // delete removes no resource of any kind.
    let (instance_status, instance) = emulator
        .call_in(
            Some(CALL_REGION),
            "GetInstance",
            json!({"instanceName": instance_name}),
        )
        .await;
    assert_eq!(
        instance_status, 200,
        "expected the instance to be untouched by the refused deletes, got {instance:?}"
    );
}

/// RULE: the delete is not idempotent — the public key is gone after the first
/// one, so a second delete of the same name has nothing to remove and fails
/// rather than reporting a deletion that did not happen.
#[tokio::test]
async fn deleting_a_key_pair_twice_fails_the_second_time() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::new();
        hold(&emulator, &[row]);
        let (first_status, first) = emulator
            .call_in(
                Some(CALL_REGION),
                "DeleteKeyPair",
                delete_body(row.key_pair_name, Some(row.fingerprint.to_string())),
            )
            .await;
        assert_eq!(
            first_status, 200,
            "case {case}: the first delete must be served, got {first_status} {first:?}"
        );

        let (status, body) = emulator
            .call_in(
                Some(CALL_REGION),
                "DeleteKeyPair",
                delete_body(row.key_pair_name, Some(row.fingerprint.to_string())),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the second delete to be refused, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case}: expected the modelled NotFoundException, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: expected code DoesNotExist, got {body:?}"
        );
        assert!(
            body["operation"].is_null(),
            "case {case}: a refused delete logs no operation, got {body:?}"
        );
    }
}
