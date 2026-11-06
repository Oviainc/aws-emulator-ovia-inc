// SPDX-License-Identifier: CC0-1.0

//! `StopKeyUsage`, end to end over the wire, pinned as a served operation.
//!
//! Every call goes through the real `MultiServiceRouter` (`POST /` with
//! `X-Amz-Target: PaymentCryptographyControlPlane.StopKeyUsage`, awsJson1_0) and the
//! modelled-constraint walk, through the same harness `key_lifecycle` uses, so what
//! these tests assert is what a signed SDK request observes.
//!
//! The rule under test: disabling a key writes the usage switch — `Enabled` and
//! `UsageStopTimestamp` — on the ONE stored key record and nothing else about that
//! key, so the answer is the stored record, a repeat of the call lands on the same
//! record rather than being refused, and every member the key carries that this call
//! does not own comes back unchanged.
//!
//! Each case is one row of a table spanning what the rule generalises over: the key
//! families this service generates, whether the create asked for an enabled key, a
//! disabled key or neither, whether the key replicates anywhere, and how many times
//! the switch has already been thrown.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_paymentcryptography_paymentcryptographycontrolplane::ComAmazonawsPaymentcryptographyPaymentcryptographycontrolplaneState as KeyStore;

use crate::key_lifecycle::{call, now_secs, ok_call, router_with_engine, string, timestamp};

/// One row of the key-kind table: a key family this service generates, with the
/// class, usage and modes of use that describe one key of it.
///
/// Source: `smithy_query(query="shape",shape="KeyAlgorithm,KeyClass,KeyUsage")`. The
/// rule below is about a key record rather than about one algorithm, so it is driven
/// over a single-length TDES key, an AES key, a MAC key and an asymmetric pair — and
/// `TDES_3KEY` is a family the issue this test pins did not name.
struct KeyKind {
    key_algorithm: &'static str,
    key_class: &'static str,
    key_usage: &'static str,
    modes: Value,
}

fn key_kinds() -> Vec<KeyKind> {
    vec![
        KeyKind {
            key_algorithm: "TDES_3KEY",
            key_class: "SYMMETRIC_KEY",
            key_usage: "TR31_P0_PIN_ENCRYPTION_KEY",
            modes: json!({"Encrypt": true, "Decrypt": true}),
        },
        KeyKind {
            key_algorithm: "AES_128",
            key_class: "SYMMETRIC_KEY",
            key_usage: "TR31_K1_KEY_BLOCK_PROTECTION_KEY",
            modes: json!({"Wrap": true, "Unwrap": true}),
        },
        KeyKind {
            key_algorithm: "HMAC_SHA512",
            key_class: "SYMMETRIC_KEY",
            key_usage: "TR31_M7_HMAC_KEY",
            modes: json!({"Generate": true, "Verify": true}),
        },
        KeyKind {
            key_algorithm: "RSA_4096",
            key_class: "ASYMMETRIC_KEY_PAIR",
            key_usage: "TR31_S0_ASYMMETRIC_KEY_FOR_DIGITAL_SIGNATURE",
            modes: json!({"Sign": true, "Verify": true}),
        },
    ]
}

/// A `CreateKey` body for one key kind, with `extra` merged into it.
fn create_body(kind: &KeyKind, extra: Value) -> Value {
    let mut body = json!({
        "KeyAttributes": {
            "KeyUsage": kind.key_usage,
            "KeyClass": kind.key_class,
            "KeyAlgorithm": kind.key_algorithm,
            "KeyModesOfUse": kind.modes.clone(),
        },
        "Exportable": true,
    });
    if let (Value::Object(target), Value::Object(extra)) = (&mut body, extra) {
        target.extend(extra);
    }
    body
}

/// The `Key` of a response.
fn key_of<'a>(case: &str, body: &'a Value) -> &'a Value {
    body.get("Key")
        .unwrap_or_else(|| panic!("case {case}: no Key in {body}"))
}

/// The members of a `Key` that the usage switch does not own, so a stop must answer
/// them exactly as the create did.
///
/// Source: `smithy_query(query="shape",shape="Key")` minus `Enabled` and
/// `UsageStopTimestamp` (what this call writes) and minus the three members whose own
/// documentation makes them a function of a deletion or of replication progress
/// (`DeletePendingTimestamp`, `DeleteTimestamp`, `ReplicationStatus`).
const UNTOUCHED_MEMBERS: [&str; 10] = [
    "KeyArn",
    "KeyAttributes",
    "KeyCheckValue",
    "KeyCheckValueAlgorithm",
    "Exportable",
    "KeyState",
    "KeyOrigin",
    "CreateTimestamp",
    "UsageStartTimestamp",
    "UsingDefaultReplicationRegions",
];

/// `(enabled, usage_stop_timestamp in epoch seconds)` as the PERSISTED record holds
/// them, so a response can be checked against the store it was computed from rather
/// than against itself.
fn persisted_switch(engine: &engine::InMemoryEngine, case: &str, arn: &str) -> (bool, Option<f64>) {
    let state = engine.state::<KeyStore>();
    let store = state.lock();
    let key = store
        .keys
        .get(arn)
        .unwrap_or_else(|| panic!("case {case}: the store holds the key {arn}"));
    let stop = key.usage_stop_timestamp.map(|instant| {
        instant
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64())
    });
    (key.enabled, stop)
}

/// Whether two epoch-second instants are the same instant.
///
/// A `Timestamp` crosses awsJson1_0 as a number, so the instant a response carries is
/// the stored one rendered through `f64`: the two agree to far finer than the
/// microsecond the wire format can express, never bit for bit.
fn same_instant(left: f64, right: f64) -> bool {
    (left - right).abs() < 1e-6
}

/// Assert `stopped` reports a disabled key stamped around now, and that the stamp is
/// the one the store holds. Answers the stop instant the response reported.
fn assert_disabled(
    engine: &engine::InMemoryEngine,
    case: &str,
    arn: &str,
    stopped: &Value,
) -> f64 {
    let stopped_key = key_of(case, stopped);
    assert_eq!(
        stopped_key.get("Enabled").and_then(Value::as_bool),
        Some(false),
        "case {case}: expected Enabled=false, got {stopped}"
    );
    let stop_instant = timestamp(stopped_key, "UsageStopTimestamp");
    assert!(
        (stop_instant - now_secs()).abs() < 60.0,
        "case {case}: expected UsageStopTimestamp around {}, got {stop_instant}",
        now_secs()
    );
    let (persisted_enabled, persisted_stop) = persisted_switch(engine, case, arn);
    assert!(
        !persisted_enabled,
        "case {case}: expected the stored record to be disabled, got enabled={persisted_enabled}"
    );
    assert!(
        persisted_stop.is_some_and(|stored| same_instant(stored, stop_instant)),
        "case {case}: expected the answered UsageStopTimestamp to be the stored one \
         ({stop_instant}), got {persisted_stop:?}"
    );
    stop_instant
}

/// The members of `key` that the usage switch does not own.
fn untouched(case: &str, key: &Value) -> BTreeMap<&'static str, Option<Value>> {
    for member in ["KeyArn", "KeyState"] {
        assert!(
            key.get(member).is_some(),
            "case {case}: expected a {member} in {key}"
        );
    }
    UNTOUCHED_MEMBERS
        .iter()
        .map(|member| (*member, key.get(*member).cloned()))
        .collect()
}

#[tokio::test]
async fn stopping_key_usage_is_served_and_writes_only_the_usage_switch() {
    let (router, engine) = router_with_engine();

    for kind in key_kinds() {
        let algorithm = kind.key_algorithm;
        // The rows differ along whether the create named `Enabled` at all, asked for
        // an enabled key or asked for a disabled one, and along whether the key
        // replicates anywhere — none of which the usage switch depends on.
        for enabled_at_create in [None, Some(true), Some(false)] {
            for replicates in [false, true] {
                let case = format!(
                    "{algorithm} created with Enabled={enabled_at_create:?}, \
                     replicating={replicates}"
                );
                let mut extra = json!({});
                if let (Value::Object(extra), Some(enabled)) = (&mut extra, enabled_at_create) {
                    extra.insert("Enabled".to_string(), json!(enabled));
                }
                if replicates {
                    if let Value::Object(extra) = &mut extra {
                        extra.insert(
                            "ReplicationRegions".to_string(),
                            json!(["eu-central-1"]),
                        );
                    }
                }

                let created = ok_call(&router, "CreateKey", &create_body(&kind, extra)).await;
                let created_key = key_of(&case, &created).clone();
                let arn = string(&created_key, "KeyArn").to_string();
                let before = untouched(&case, &created_key);

                // The operation is SERVED: a scaffold would answer 501
                // `NotImplemented` here and persist nothing.
                let (status, stopped) =
                    call(&router, "StopKeyUsage", &json!({"KeyIdentifier": arn})).await;
                assert_eq!(
                    status, 200,
                    "case {case}: expected StopKeyUsage to answer 200, got {status} {stopped}"
                );
                let first_stop = assert_disabled(&engine, &case, &arn, &stopped);

                // Nothing the call does not own moved.
                assert_eq!(
                    untouched(&case, key_of(&case, &stopped)),
                    before,
                    "case {case}: StopKeyUsage must leave every member it does not own \
                     where the create put it: {stopped}"
                );

                // A key that replicates is still a Multi-Region primary after its
                // usage stops: disabling a key is not a replication change.
                assert_eq!(
                    key_of(&case, &stopped).get("MultiRegionKeyType").is_some(),
                    replicates,
                    "case {case}: expected MultiRegionKeyType present={replicates}: {stopped}"
                );

                // Repeating the call lands on the same record: the key is still in
                // the one state this call requires of it, so it is disabled again
                // rather than refused, and the stamp only ever moves forward.
                for repeat in 1..=2 {
                    let case = format!("{case}, repeat {repeat}");
                    let (status, again) =
                        call(&router, "StopKeyUsage", &json!({"KeyIdentifier": arn})).await;
                    assert_eq!(
                        status, 200,
                        "case {case}: expected a repeated StopKeyUsage to answer 200, \
                         got {status} {again}"
                    );
                    let repeated_stop = assert_disabled(&engine, &case, &arn, &again);
                    assert!(
                        repeated_stop >= first_stop,
                        "case {case}: expected the stop instant at or after the first \
                         ({first_stop}), got {repeated_stop}"
                    );
                    assert_eq!(
                        untouched(&case, key_of(&case, &again)),
                        before,
                        "case {case}: a repeat must still leave every member it does not \
                         own alone: {again}"
                    );
                }

                // And the reader agrees, because both answer from that one record.
                let fetched = ok_call(&router, "GetKey", &json!({"KeyIdentifier": arn})).await;
                let fetched_key = key_of(&case, &fetched);
                assert_eq!(
                    fetched_key.get("Enabled").and_then(Value::as_bool),
                    Some(false),
                    "case {case}: GetKey must report the disabled key: {fetched}"
                );
                let (_, persisted_stop) = persisted_switch(&engine, &case, &arn);
                let fetched_stop = timestamp(fetched_key, "UsageStopTimestamp");
                assert!(
                    persisted_stop.is_some_and(|stored| same_instant(stored, fetched_stop)),
                    "case {case}: GetKey must report the stored UsageStopTimestamp \
                     ({persisted_stop:?}), got {fetched_stop}: {fetched}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_usage_switch_can_be_thrown_back_and_forth() {
    let (router, engine) = router_with_engine();
    let kinds = key_kinds();

    // The switch is a stored boolean plus two instants, so a caller may throw it any
    // number of times: each throw writes that record and the next read reports it.
    for (index, kind) in kinds.iter().enumerate() {
        let case = format!("{} cycled", kind.key_algorithm);
        let created = ok_call(&router, "CreateKey", &create_body(kind, json!({}))).await;
        let arn = string(key_of(&case, &created), "KeyArn").to_string();
        // Rows differ in how many stop/start cycles precede the final read.
        let cycles = index + 1;

        for cycle in 1..=cycles {
            let case = format!("{case}, cycle {cycle} of {cycles}");
            let stopped =
                ok_call(&router, "StopKeyUsage", &json!({"KeyIdentifier": arn})).await;
            assert_disabled(&engine, &case, &arn, &stopped);

            let started =
                ok_call(&router, "StartKeyUsage", &json!({"KeyIdentifier": arn})).await;
            let started_key = key_of(&case, &started);
            assert_eq!(
                started_key.get("Enabled").and_then(Value::as_bool),
                Some(true),
                "case {case}: StartKeyUsage must enable the key again: {started}"
            );
            assert!(
                started_key.get("UsageStopTimestamp").is_none(),
                "case {case}: a key in use again has no stop instant: {started}"
            );
            let (persisted_enabled, persisted_stop) = persisted_switch(&engine, &case, &arn);
            assert!(
                persisted_enabled && persisted_stop.is_none(),
                "case {case}: expected the stored record enabled with no stop instant, \
                 got enabled={persisted_enabled}, stop={persisted_stop:?}"
            );
        }

        // The last throw is a stop, so that is what the reader must report.
        let stopped = ok_call(&router, "StopKeyUsage", &json!({"KeyIdentifier": arn})).await;
        let stop_instant = assert_disabled(&engine, &case, &arn, &stopped);
        let fetched = ok_call(&router, "GetKey", &json!({"KeyIdentifier": arn})).await;
        let fetched_key = key_of(&case, &fetched);
        assert_eq!(
            fetched_key.get("Enabled").and_then(Value::as_bool),
            Some(false),
            "case {case}: GetKey must report the last throw of the switch: {fetched}"
        );
        let fetched_stop = timestamp(fetched_key, "UsageStopTimestamp");
        assert!(
            same_instant(fetched_stop, stop_instant),
            "case {case}: GetKey must report the stop instant the last call wrote \
             ({stop_instant}), got {fetched_stop}: {fetched}"
        );
        // Usage started when the key was created and every start since, so the start
        // instant survives a stop rather than being cleared by it.
        assert!(
            fetched_key.get("UsageStartTimestamp").is_some(),
            "case {case}: a stopped key keeps the instant its usage started: {fetched}"
        );
    }
}
