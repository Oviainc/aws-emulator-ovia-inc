// SPDX-License-Identifier: CC0-1.0

//! Clearing the remediation exceptions a Config rule holds, end to end over the wire.
//!
//! The rule pinned here: `DeleteRemediationExceptions` removes the exception the rule
//! holds for each resource key the request names, reports the keys it could not remove
//! in `FailedBatches`, and — when NO named key names an exception that exists — refuses
//! the request 400 `NoSuchRemediationExceptionException` ("You tried to delete a
//! remediation exception that does not exist"), which is the only answer that tells a
//! caller its delete removed nothing at all. An exception the clock has already taken
//! past its `ExpirationTime` is gone before the request is judged.
//!
//! Each case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds. The exceptions are
//! seeded in the store the way a handler reaches it (`service_state`, as
//! `select_resource_config` seeds the inventory its queries run over), because
//! `PutRemediationExceptions` — the operation that WRITES one — is still a scaffold with
//! a stub issue of its own.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::{
    service_state, RemediationExceptionEntry,
};

use crate::config_setup_and_rule_loop::Emulator;

/// Entries of `com.amazonaws.configservice#ResourceType`, from
/// `smithy_query(query="shape", shape="ResourceType")`; a `RemediationExceptionResourceKey`
/// carries its `ResourceType` as a free string, so these are the values a client sends.
const EC2_VOLUME: &str = "AWS::EC2::Volume";
/// See [`EC2_VOLUME`].
const S3_BUCKET: &str = "AWS::S3::Bucket";

/// The rule the seeded exceptions belong to.
const RULE: &str = "encrypted-volumes";
/// Another rule, whose exceptions no request for [`RULE`] may touch.
const OTHER_RULE: &str = "versioned-buckets";

/// One exception a case starts out with: the rule it exempts a resource from, the
/// resource, and how far from now it falls due for deletion (negative: already due).
struct Seeded {
    config_rule_name: &'static str,
    resource_type: &'static str,
    resource_id: &'static str,
    expires_in: Option<i64>,
}

/// An emulator whose store holds `seeded`, written the way a handler reaches it.
fn emulator_holding(seeded: &[Seeded]) -> Emulator {
    let emulator = Emulator::new();
    let state = service_state(&*emulator.engine).expect("the test engine is the shared engine");
    let now = SystemTime::now();
    {
        let mut store = state.lock();
        for exception in seeded {
            let entry = RemediationExceptionEntry {
                config_rule_name: exception.config_rule_name.to_string(),
                resource_type: exception.resource_type.to_string(),
                resource_id: exception.resource_id.to_string(),
                message: Some("waiting on the owning team".to_string()),
                expires_at: exception.expires_in.map(|seconds| {
                    if seconds >= 0 {
                        now + Duration::from_secs(seconds.unsigned_abs())
                    } else {
                        now - Duration::from_secs(seconds.unsigned_abs())
                    }
                }),
            };
            store.remediation_exceptions.insert(entry.key(), entry);
        }
    }
    emulator
}

/// The `(resourceType, resourceId)` pairs the store still holds an exception for under
/// `config_rule_name`.
fn held(emulator: &Emulator, config_rule_name: &str) -> Vec<(String, String)> {
    let state = service_state(&*emulator.engine).expect("the test engine is the shared engine");
    let store = state.lock();
    store
        .remediation_exceptions
        .values()
        .filter(|entry| entry.config_rule_name == config_rule_name)
        .map(|entry| (entry.resource_type.clone(), entry.resource_id.clone()))
        .collect()
}

/// The `DeleteRemediationExceptions` body naming `keys` under the rule `rule`.
fn delete(rule: &str, keys: &[Value]) -> Value {
    json!({ "ConfigRuleName": rule, "ResourceKeys": keys })
}

/// A resource key naming both members, which is what identifies a resource in Config.
fn key(resource_type: &str, resource_id: &str) -> Value {
    json!({ "ResourceType": resource_type, "ResourceId": resource_id })
}

/// The `(resourceType, resourceId)` pairs one `FailedBatches` reports, with the failure
/// message each batch carried.
fn failed_items(case: &str, body: &Value) -> Vec<(String, String, String)> {
    body["FailedBatches"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected FailedBatches, got {body}"))
        .iter()
        .flat_map(|batch| {
            let message = batch["FailureMessage"].as_str().unwrap_or_default().to_string();
            batch["FailedItems"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(move |item| {
                    (
                        item["ResourceType"].as_str().unwrap_or_default().to_string(),
                        item["ResourceId"].as_str().unwrap_or_default().to_string(),
                        message.clone(),
                    )
                })
        })
        .collect()
}

/// The sentence Config words `NoSuchRemediationExceptionException` with, which the
/// failure of one key inside a batch carries too.
const NOT_FOUND: &str = "Specified RemediationException could not be found.";

/// The `(resourceType, resourceId)` pair one row names a resource by.
type Key = (&'static str, &'static str);

/// One row of the deleting table: what it is called, the keys the request names, the
/// keys it expects to be deleted, and the keys it expects back as failed.
type DeleteCase = (&'static str, Vec<Value>, Vec<Key>, Vec<Key>);

#[tokio::test]
async fn a_delete_removes_the_exceptions_it_names_and_reports_the_rest() {
    // Rows differ along what the rule generalises over: how many keys one request names,
    // how many of them the rule holds an exception for, and which of the named keys are
    // the held ones. Each row states the keys it expects to be deleted and the keys it
    // expects back as failed.
    let cases: [DeleteCase; 5] = [
        (
            "one key the rule holds an exception for",
            vec![key(EC2_VOLUME, "vol-0001")],
            vec![(EC2_VOLUME, "vol-0001")],
            vec![],
        ),
        (
            "every key the rule holds an exception for",
            vec![
                key(EC2_VOLUME, "vol-0001"),
                key(EC2_VOLUME, "vol-0002"),
                key(S3_BUCKET, "reports"),
            ],
            vec![
                (EC2_VOLUME, "vol-0001"),
                (EC2_VOLUME, "vol-0002"),
                (S3_BUCKET, "reports"),
            ],
            vec![],
        ),
        (
            "one held key beside one the rule holds nothing for",
            vec![key(EC2_VOLUME, "vol-0001"), key(EC2_VOLUME, "vol-9999")],
            vec![(EC2_VOLUME, "vol-0001")],
            vec![(EC2_VOLUME, "vol-9999")],
        ),
        (
            "a held key beside the same resource id under another type",
            vec![key(S3_BUCKET, "reports"), key(EC2_VOLUME, "reports")],
            vec![(S3_BUCKET, "reports")],
            vec![(EC2_VOLUME, "reports")],
        ),
        (
            "a held key beside a key that names no resource at all",
            vec![key(EC2_VOLUME, "vol-0002"), json!({})],
            vec![(EC2_VOLUME, "vol-0002")],
            vec![("", "")],
        ),
    ];

    for (case, keys, expected_deleted, expected_failed) in cases {
        let emulator = emulator_holding(&[
            Seeded {
                config_rule_name: RULE,
                resource_type: EC2_VOLUME,
                resource_id: "vol-0001",
                expires_in: None,
            },
            Seeded {
                config_rule_name: RULE,
                resource_type: EC2_VOLUME,
                resource_id: "vol-0002",
                expires_in: Some(3_600),
            },
            Seeded {
                config_rule_name: RULE,
                resource_type: S3_BUCKET,
                resource_id: "reports",
                expires_in: None,
            },
            Seeded {
                config_rule_name: OTHER_RULE,
                resource_type: S3_BUCKET,
                resource_id: "reports",
                expires_in: None,
            },
        ]);
        let before = held(&emulator, RULE);

        let body = emulator
            .ok("DeleteRemediationExceptions", &delete(RULE, &keys))
            .await;

        let reported = failed_items(case, &body);
        let expected_reported: Vec<(String, String, String)> = expected_failed
            .iter()
            .map(|(resource_type, resource_id)| {
                (
                    (*resource_type).to_string(),
                    (*resource_id).to_string(),
                    NOT_FOUND.to_string(),
                )
            })
            .collect();
        assert_eq!(
            reported, expected_reported,
            "case {case:?}: expected the keys {expected_failed:?} back as failed, got {body}"
        );

        // What was deleted is GONE from the rule's exceptions, and nothing else is.
        let after = held(&emulator, RULE);
        let expected_after: Vec<(String, String)> = before
            .iter()
            .filter(|(resource_type, resource_id)| {
                !expected_deleted.iter().any(|(deleted_type, deleted_id)| {
                    deleted_type == resource_type && deleted_id == resource_id
                })
            })
            .cloned()
            .collect();
        assert_eq!(
            after, expected_after,
            "case {case:?}: expected the exceptions {expected_after:?} to survive, got {after:?}"
        );

        // Another rule's exception for the same resource is untouched: an exception
        // belongs to ONE rule.
        assert_eq!(
            held(&emulator, OTHER_RULE),
            vec![(S3_BUCKET.to_string(), "reports".to_string())],
            "case {case:?}: a delete under {RULE:?} must not touch {OTHER_RULE:?}"
        );

        // Deleting the same keys again removes nothing, which is the refusal: the first
        // delete took effect.
        let (status, code) = emulator
            .refused("DeleteRemediationExceptions", &delete(RULE, &keys))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NoSuchRemediationExceptionException"),
            "case {case:?}: expected the second delete of the same keys to be refused 400 \
             NoSuchRemediationExceptionException, got {status} {code}"
        );
    }
}

#[tokio::test]
async fn a_delete_that_names_no_existing_exception_is_refused() {
    // Rows differ along WHY no named key names an existing exception: the rule holds
    // none at all, the keys belong to another rule, only one member of the key matches,
    // the key names nothing, and the exception the key names has already expired.
    let cases: [(&str, &str, Vec<Value>); 6] = [
        (
            "a rule holding no exception at all",
            "no-exceptions-here",
            vec![key(EC2_VOLUME, "vol-0001")],
        ),
        (
            "keys held under another rule",
            OTHER_RULE,
            vec![key(EC2_VOLUME, "vol-0001"), key(EC2_VOLUME, "vol-0002")],
        ),
        (
            "a resource id the rule holds no exception for",
            RULE,
            vec![key(EC2_VOLUME, "vol-9999")],
        ),
        (
            "the right resource id under the wrong type",
            RULE,
            vec![key(S3_BUCKET, "vol-0001")],
        ),
        (
            "a key naming neither member",
            RULE,
            vec![json!({ "ResourceType": EC2_VOLUME })],
        ),
        (
            "an exception the clock has already taken past its expiration",
            RULE,
            vec![key(S3_BUCKET, "expired-bucket")],
        ),
    ];

    for (case, rule, keys) in cases {
        let emulator = emulator_holding(&[
            Seeded {
                config_rule_name: RULE,
                resource_type: EC2_VOLUME,
                resource_id: "vol-0001",
                expires_in: None,
            },
            Seeded {
                config_rule_name: RULE,
                resource_type: EC2_VOLUME,
                resource_id: "vol-0002",
                expires_in: None,
            },
            Seeded {
                config_rule_name: RULE,
                resource_type: S3_BUCKET,
                resource_id: "expired-bucket",
                expires_in: Some(-60),
            },
        ]);

        let (status, parsed) = emulator
            .call("DeleteRemediationExceptions", &delete(rule, &keys))
            .await;
        let code = parsed["__type"]
            .as_str()
            .map(|wire| wire.rsplit('#').next().unwrap_or(wire))
            .unwrap_or("<none>");
        assert_eq!(
            (status, code),
            (400, "NoSuchRemediationExceptionException"),
            "case {case:?}: expected 400 NoSuchRemediationExceptionException, got {status} \
             {parsed}"
        );
        assert_eq!(
            parsed["message"].as_str(),
            Some(NOT_FOUND),
            "case {case:?}: expected the refusal to be worded {NOT_FOUND:?}, got {parsed}"
        );

        // A refused delete deletes nothing — except the exception the CLOCK had already
        // taken past its expiration time, which was gone before the request arrived.
        assert_eq!(
            held(&emulator, RULE),
            vec![
                (EC2_VOLUME.to_string(), "vol-0001".to_string()),
                (EC2_VOLUME.to_string(), "vol-0002".to_string()),
            ],
            "case {case:?}: a refused delete leaves the rule's unexpired exceptions in place"
        );
    }
}
