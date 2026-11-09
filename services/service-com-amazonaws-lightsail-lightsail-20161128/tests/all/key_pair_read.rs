// SPDX-License-Identifier: CC0-1.0

//! Reading one Lightsail SSH key pair — `GetKeyPair` — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handler and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rules under test:
//!
//! * A reporting operation that names ONE resource answers that resource as the
//!   service holds it — every modelled member computed from the stored record,
//!   nothing else — and a well-formed name the account holds no such resource
//!   under is refused with the service's `DoesNotExist` envelope rather than a
//!   hollow 200. The name is matched exactly, and it is the whole of the lookup:
//!   the Region the call is made in does not select between records, and a name
//!   another kind of resource answers to is not a key pair name.
//! * A read changes nothing a later caller sees: repeating it answers the same
//!   record, leaves the store as it was and writes no operation-log entry.
//!
//! The rows below hold a key pair with a chosen id, instant and fingerprint
//! FORM, none of which a create lets a caller pick — `CreateKeyPair` mints its
//! own material, and `ImportKeyPair` and `DownloadDefaultKeyPair` are still
//! scaffold stubs with issues of their own — so seeding the store the handler
//! reads (`state::with_state`) is how a case drives the account HOLDING the key
//! pair a read names, exactly as the sibling `key_pair_delete` suite does.
//! Everything the read answers is asserted through the wire.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::ops::types::Tag;
use service_com_amazonaws_lightsail_lightsail_20161128::state::{
    self, StoredKeyPair, StoredLocation, DEFAULT_SSH_KEY_NAME,
};

use crate::instance_lifecycle::{error_code, Emulator};

/// The Regions the calls in this suite are made in.
///
/// A key pair is addressed by name alone — the store holds it under that name,
/// exactly as it holds the instance whose `sshKeyName` names one — so the read
/// must answer the same record from any Region's endpoint, including one that
/// holds no key pair of its own. `None` is a call carrying no credential scope at
/// all, served in `crate::state::DEFAULT_REGION`.
const CALL_REGIONS: &[Option<&str>] = &[None, Some("us-east-1"), Some("eu-central-1")];

/// Every member the modelled `KeyPair` carries, which is what a read of a stored
/// key pair reports: `smithy_query(query="shape",shape="KeyPair")`.
const KEY_PAIR_MEMBERS: &[&str] = &[
    "arn",
    "createdAt",
    "fingerprint",
    "location",
    "name",
    "resourceType",
    "supportCode",
    "tags",
];

/// One key pair the account holds.
///
/// The rows differ along everything the rule generalises over: the characters,
/// case and length of the name (letters, digits, hyphens, underscores, mixed
/// case, and the name the account's DEFAULT key pair is held under), the Region
/// the record was created in, the instant it was created at, how many tags it
/// carries (none, one, two) and the FORM of the fingerprint — the MD5 colon-hex
/// form `ssh-keygen -E md5` prints and the `SHA256:`-prefixed form it prints by
/// default — because this operation reports the fingerprint and never parses it.
struct Row {
    case: &'static str,
    key_pair_name: &'static str,
    region: &'static str,
    created_at_ms: i64,
    fingerprint: &'static str,
    tags: &'static [(&'static str, &'static str)],
}

const ROWS: &[Row] = &[
    Row {
        case: "a hyphenated name, no tags, an MD5 colon-hex fingerprint",
        key_pair_name: "key-pair-alpha",
        region: "us-west-2",
        created_at_ms: 1_770_000_000_000,
        fingerprint: "a6:1f:3c:d2:0b:8e:74:59:c1:3d:ef:90:12:ab:cd:34",
        tags: &[],
    },
    Row {
        case: "underscores and digits, one tag, created in the Region the calls are made in",
        key_pair_name: "kp_2026_build",
        region: "us-east-1",
        created_at_ms: 1_700_111_222_333,
        fingerprint: "b7:20:4d:e3:1c:9f:85:6a:d2:4e:f0:01:23:bc:de:45",
        tags: &[("env", "staging")],
    },
    Row {
        case: "a short upper-case name, two tags, a SHA256 fingerprint",
        key_pair_name: "KP7",
        region: "ap-northeast-1",
        created_at_ms: 1_479_816_991_349,
        fingerprint: "SHA256:hXy0pZ9Qw3kLmNoPqRsTuVwXyZaBcDeFgHiJkLmNoPq",
        tags: &[("team", "ssh"), ("cost-centre", "8080")],
    },
    Row {
        case: "a long mixed name, one tag",
        key_pair_name: "sim-ls-kp-with-a-much-longer-name-9",
        region: "eu-west-1",
        created_at_ms: 1_612_345_678_901,
        fingerprint: "c8:31:5e:f4:2d:a0:96:7b:e3:5f:01:12:34:cd:ef:56",
        tags: &[("keep", "yes")],
    },
    Row {
        case: "the account's Lightsail default key pair",
        key_pair_name: DEFAULT_SSH_KEY_NAME,
        region: "us-west-2",
        created_at_ms: 1_770_000_123_456,
        fingerprint: "d9:42:6f:05:3e:b1:a7:8c:f4:60:12:23:45:de:f0:67",
        tags: &[("purpose", "default")],
    },
];

/// The resource id the key pair of row `index` was allocated, which its ARN and
/// its support code are both built from. This suite's own, so two rows never
/// share one.
fn key_pair_id(index: usize) -> String {
    format!("05859e3d-331d-48ba-9034-1234512345{index:02}")
}

/// Put every key pair `ROWS` describes in the store, under the id, instant and
/// fingerprint form each row names.
fn hold(emulator: &Emulator) {
    let held = state::with_state(emulator.engine(), |store| {
        for (index, row) in ROWS.iter().enumerate() {
            let id = key_pair_id(index);
            store.insert_key_pair(StoredKeyPair {
                name: row.key_pair_name.to_string(),
                arn: state::key_pair_arn(row.region, &id),
                support_code: state::support_code(&id),
                created_at_ms: row.created_at_ms,
                location: StoredLocation::regional(row.region),
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
        Some(ROWS.len()),
        "case seeding {} key pairs: expected the store to hold them, got {held:?}",
        ROWS.len()
    );
}

/// The `keyPair` a served `GetKeyPair` answered for `name`, in `region`.
async fn read(emulator: &Emulator, region: Option<&str>, name: &str) -> Value {
    let (status, body) = emulator
        .call_in(region, "GetKeyPair", json!({"keyPairName": name}))
        .await;
    assert_eq!(
        status, 200,
        "case reading {name:?} in {region:?}: the read must be served, got {status} {body:?}"
    );
    body["keyPair"].clone()
}

/// The modelled projection of every key pair the store holds, rendered so two
/// moments can be compared: the modelled `KeyPair` derives `Debug` but no
/// equality, and what a case about an untouched store needs is exactly "the same
/// as before".
fn stored(emulator: &Emulator) -> Vec<String> {
    state::with_state(emulator.engine(), |store| {
        store
            .key_pairs
            .values()
            .map(|held| format!("{:?}", held.view()))
            .collect()
    })
    .expect("the shared engine holds this service's store")
}

/// Every operation-log entry the account holds, as `GetOperations` reports it,
/// walking every page.
async fn operation_log(emulator: &Emulator) -> Vec<Value> {
    let mut entries = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let (status, body) = match &token {
            Some(token) => {
                emulator
                    .call("GetOperations", json!({"pageToken": token}))
                    .await
            }
            None => emulator.call("GetOperations", json!({})).await,
        };
        assert_eq!(status, 200, "GetOperations must succeed, got {body:?}");
        entries.extend(body["operations"].as_array().cloned().unwrap_or_default());
        match body["nextPageToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    entries
}

/// Assert the `keyPair` a read answered is the key pair row `index` describes,
/// member for member.
fn assert_reported(case: &str, index: usize, key_pair: &Value) {
    let row = &ROWS[index];
    let id = key_pair_id(index);

    let reported: BTreeSet<&str> = key_pair
        .as_object()
        .unwrap_or_else(|| panic!("case {case}: expected a keyPair object, got {key_pair:?}"))
        .keys()
        .map(String::as_str)
        .collect();
    let modelled: BTreeSet<&str> = KEY_PAIR_MEMBERS.iter().copied().collect();
    assert_eq!(
        reported, modelled,
        "case {case}: expected exactly the modelled KeyPair members, got {key_pair:?}"
    );

    assert_eq!(
        key_pair["name"].as_str(),
        Some(row.key_pair_name),
        "case {case}: expected the name the account holds the key pair under, got {key_pair:?}"
    );
    let arn = state::key_pair_arn(row.region, &id);
    assert_eq!(
        key_pair["arn"].as_str(),
        Some(arn.as_str()),
        "case {case}: expected the stored ARN, got {key_pair:?}"
    );
    let support_code = state::support_code(&id);
    assert_eq!(
        key_pair["supportCode"].as_str(),
        Some(support_code.as_str()),
        "case {case}: expected the stored support code, got {key_pair:?}"
    );
    assert_eq!(
        key_pair["createdAt"].as_f64(),
        Some(row.created_at_ms as f64 / 1_000.0),
        "case {case}: expected the stored creation instant in epoch seconds, got {key_pair:?}"
    );
    assert_eq!(
        key_pair["location"]["regionName"].as_str(),
        Some(row.region),
        "case {case}: expected the key pair's OWN region, got {:?}",
        key_pair["location"]
    );
    assert!(
        key_pair["location"]["availabilityZone"].is_null(),
        "case {case}: a key pair is held by the Region as a whole, so no availability zone may \
         be reported, got {:?}",
        key_pair["location"]
    );
    assert_eq!(
        key_pair["resourceType"].as_str(),
        Some("KeyPair"),
        "case {case}: expected the KeyPair resource type, got {key_pair:?}"
    );
    assert_eq!(
        key_pair["fingerprint"].as_str(),
        Some(row.fingerprint),
        "case {case}: expected the stored fingerprint verbatim, got {key_pair:?}"
    );
    let tags: Value = row
        .tags
        .iter()
        .map(|(key, value)| json!({"key": key, "value": value}))
        .collect();
    assert_eq!(
        key_pair["tags"], tags,
        "case {case}: expected the stored tags, got {key_pair:?}"
    );
}

// ---------------------------------------------------------------------------
// The read table
// ---------------------------------------------------------------------------

/// RULE: `GetKeyPair` answers the key pair the account holds under the name it is
/// given, with every modelled member computed from that stored record — and the
/// lookup is the name alone, so the same record is answered from every Region's
/// endpoint.
#[tokio::test]
async fn a_held_key_pair_reads_back_as_the_account_holds_it() {
    let emulator = Emulator::new();
    hold(&emulator);

    for (index, row) in ROWS.iter().enumerate() {
        let mut answers: Vec<String> = Vec::new();
        for region in CALL_REGIONS {
            let case = format!("{} read in {region:?}", row.case);
            let key_pair = read(&emulator, *region, row.key_pair_name).await;
            assert_reported(&case, index, &key_pair);
            answers.push(key_pair.to_string());
        }
        assert!(
            answers.windows(2).all(|pair| pair[0] == pair[1]),
            "case {}: expected every Region's endpoint to answer the same record, got {answers:?}",
            row.case
        );
    }
}

/// RULE: a read reports and does not mutate — repeating it answers the same
/// record, leaves every stored key pair as it was and writes no operation-log
/// entry, so nothing a later caller sees depends on whether a read happened.
#[tokio::test]
async fn reading_a_key_pair_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    hold(&emulator);
    let before_store = stored(&emulator);
    let before_log = operation_log(&emulator).await;

    let mut first: Vec<String> = Vec::new();
    for row in ROWS {
        first.push(read(&emulator, None, row.key_pair_name).await.to_string());
    }
    // A refused read is a read too: it must write nothing either.
    let (refused_status, _) = emulator
        .call("GetKeyPair", json!({"keyPairName": "no-such-key-pair"}))
        .await;
    assert_eq!(
        refused_status, 400,
        "case a refused read: expected the DoesNotExist refusal, got {refused_status}"
    );

    for (index, row) in ROWS.iter().enumerate() {
        let case = format!("{} read a second time", row.case);
        let again = read(&emulator, None, row.key_pair_name).await;
        assert_reported(&case, index, &again);
        assert_eq!(
            again.to_string(),
            first[index],
            "case {case}: expected the second read to answer the first read's record"
        );
    }

    assert_eq!(
        stored(&emulator),
        before_store,
        "case reading every held key pair: expected the store to be untouched"
    );
    assert_eq!(
        operation_log(&emulator).await,
        before_log,
        "case reading every held key pair: expected a read to log no operation"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// RULE: a well-formed name the account holds no key pair under is refused with
/// the service's `DoesNotExist` envelope — HTTP 400, `NotFoundException`, body
/// code `DoesNotExist` and a message naming the resource type and the requested
/// name — never a 501 and never a hollow 200. The name is matched exactly, and a
/// name another kind of resource answers to is not a key pair name.
#[tokio::test]
async fn reading_a_key_pair_the_account_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    hold(&emulator);

    // An instance of the account, so a case can show the lookup is the key
    // pair's own rather than "any resource of this name".
    let instance_name = "key-pair-read-suite-instance";
    let (instance_status, created) = emulator
        .call(
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
        "the suite's instance must be created, got {instance_status} {created:?}"
    );

    // A key pair the account held and no longer holds: the delete is served, so
    // the read of that name must be refused from then on.
    let deleted = &ROWS[0];
    let (delete_status, deleted_body) = emulator
        .call(
            "DeleteKeyPair",
            json!({"keyPairName": deleted.key_pair_name}),
        )
        .await;
    assert_eq!(
        delete_status, 200,
        "the suite's delete must be served, got {delete_status} {deleted_body:?}"
    );

    let unknown: &[(&str, &str)] = &[
        ("a name no resource of the account answers to", "no-such-key-pair"),
        // The key pair names are store keys compared exactly, so a name that
        // differs from a held one only in case names no key pair.
        ("a held name in another case", "kp7"),
        ("a held name with a trailing digit", "key-pair-alpha2"),
        ("the name of an instance of the account", instance_name),
        ("a key pair the account no longer holds", deleted.key_pair_name),
    ];

    for (case, name) in unknown {
        for region in CALL_REGIONS {
            let case = format!("{case} read in {region:?}");
            let (status, body) = emulator
                .call_in(*region, "GetKeyPair", json!({"keyPairName": name}))
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
                body["keyPair"].is_null(),
                "case {case}: a refused read reports no key pair, got {body:?}"
            );
        }
    }

    // Every other key pair is still readable: a refused read answers about one
    // name and tells nothing else about the account.
    for (index, row) in ROWS.iter().enumerate().skip(1) {
        let case = format!("{} after the refusals", row.case);
        let key_pair = read(&emulator, None, row.key_pair_name).await;
        assert_reported(&case, index, &key_pair);
    }
}
