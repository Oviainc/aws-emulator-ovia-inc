// SPDX-License-Identifier: CC0-1.0

//! WHICH SHARD a rule's Kinesis data stream target sends an event to, end to end
//! over the wire through the real `MultiServiceRouter` with Kinesis mounted beside
//! EventBridge on one engine — the half of the `KinesisParameters` contract that
//! makes a stream target worth pointing a rule at: "so that you can control the
//! shard to which the event goes".
//!
//! The RULE asserted here: a Kinesis data stream target's record is placed on the
//! shard whose `HashKeyRange` contains the hash of the partition key the record
//! carries ("an MD5 hash function is used to map partition keys to 128-bit integer
//! values and to map associated data records to shards. As a result of this hashing
//! mechanism, all data records with the same partition key map to the same shard
//! within the stream" — `PutRecordInput$PartitionKey`), and that key is resolved
//! PER EVENT out of the event being delivered: the JSON text of the value the
//! target's `PartitionKeyPath` selects out of it, or the event's own id when the
//! target carries no `KinesisParameters` at all
//! (`KinesisParameters$PartitionKeyPath`). So two events whose selected values
//! differ are placed wherever their OWN values hash to — over a map of more than
//! one shard, on more than one of them — while two events selecting the same value
//! are always placed alike.
//!
//! `kinesis_partition_key_json_text` owns the rendering table (which JSON text a
//! selected value becomes) and asserts that placement for ONE event on a map of
//! four shards; this suite is the per-event half of the same rule, so its rows
//! differ along the stream's SHARD MAP (one shard, and maps of two, three and
//! five) and along the selected value the key is hashed from (two values that
//! differ, the same value published twice, a one-character value, a long one, one
//! carrying non-ASCII text and one whose text is a number). Two targets of ONE
//! rule name ONE stream, differing only in whether they carry `KinesisParameters`,
//! so the eventId default's placement is graded beside the path's.
//!
//! The harness — the router with EventBridge, Kinesis, SQS, IAM and CloudWatch on
//! one engine, the queue and role fixtures, and the stream, shard-map and
//! shard-reading helpers — is the one `target_dispatch` owns.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::SERVICE_PRINCIPAL;

use crate::target_dispatch::{
    covering_shard, drain, emulator, given_queue, given_role, given_rule, given_stream,
    publish_details, read_shards_until, shard_map, Held,
};

/// The stream every case below points its targets at, and the rule that matches.
const STREAM_NAME: &str = "routed-telemetry";
const RULE_NAME: &str = "to-the-stream";

/// The action a Kinesis data stream target's invocation performs, and so the one
/// the target's role has to be allowed to make.
const TARGET_ACTION: &str = "kinesis:PutRecord";

/// The path the keyed target selects its partition key with.
const SELECTING_PATH: &str = "$.detail.tenant";

/// The `$.detail.tenant` values the published events differ along: two that differ,
/// one repeated (two events selecting the same value), one a single character, one
/// long, one carrying non-ASCII text, and one whose text is a number.
const TENANTS: [&str; 7] = [
    "tenant-alpha",
    "tenant-omega",
    "tenant-alpha",
    "x",
    "tenant-with-a-rather-longer-name-0123456789-0123456789",
    "tenant-ωμέγα",
    "42",
];

/// The `Detail` an event carrying `tenant` is published with.
fn detail(tenant: &str) -> String {
    json!({ "tenant": tenant, "orderId": "A-1" }).to_string()
}

/// The `id` of the event a record's data carries, with the `detail.tenant` that
/// event was published with.
fn delivered(case: &str, data: &str) -> (String, Value) {
    let parsed: Value = serde_json::from_str(data)
        .unwrap_or_else(|error| panic!("case {case}: a record's data is not JSON ({error})"));
    (
        parsed["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: a record's data carries no event id: {parsed}"))
            .to_string(),
        parsed["detail"]["tenant"].clone(),
    )
}

/// Whether `text` has the shape of a uuid: 8-4-4-4-12 hex digits.
fn is_uuid_shaped(text: &str) -> bool {
    let groups: Vec<&str> = text.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .map(|group| group.len())
            .eq([8usize, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|group| group.chars().all(|digit| digit.is_ascii_hexdigit()))
}

/// Whether `key` is the delivered event's own id, as a target carrying no
/// `KinesisParameters` keys its record: the id itself, or the id followed by an
/// underscore and a second uuid.
fn is_event_id_key(key: &str, event_id: &str) -> bool {
    key == event_id
        || key
            .strip_prefix(event_id)
            .and_then(|rest| rest.strip_prefix('_'))
            .is_some_and(is_uuid_shaped)
}

/// Each record sits on the shard its OWN key hashes into, whatever map the stream
/// has: two events selecting one value are placed alike, events selecting values
/// that hash into different ranges are placed apart, and the target carrying no
/// `KinesisParameters` is placed by the delivered event's id.
#[tokio::test]
async fn a_kinesis_targets_record_sits_on_the_shard_its_own_partition_key_hashes_into() {
    // A map of one shard holds every record whatever key it carries, which is the
    // degenerate end of the rule; the others are maps a key can choose within.
    for shard_count in [1_i64, 2, 3, 5] {
        let case = format!("a stream of {shard_count} shard(s)");
        let case = case.as_str();
        let emulator = emulator();
        let stream_arn = given_stream(&emulator, case, STREAM_NAME, shard_count).await;
        let (dead_letter_url, dead_letter_arn) =
            given_queue(&emulator, "stream-dead-letters").await;
        let role_arn = given_role(
            &emulator,
            "stream-writer",
            SERVICE_PRINCIPAL,
            Some(TARGET_ACTION),
        )
        .await;
        given_rule(
            &emulator,
            RULE_NAME,
            json!([
                {
                    "Id": "sel",
                    "Arn": stream_arn,
                    "RoleArn": role_arn,
                    "KinesisParameters": { "PartitionKeyPath": SELECTING_PATH },
                    "DeadLetterConfig": { "Arn": dead_letter_arn },
                },
                {
                    "Id": "dflt",
                    "Arn": stream_arn,
                    "RoleArn": role_arn,
                    "DeadLetterConfig": { "Arn": dead_letter_arn },
                },
            ]),
        )
        .await;

        let details: Vec<String> = TENANTS.iter().map(|tenant| detail(tenant)).collect();
        let event_ids = publish_details(&emulator, &details).await;
        assert_eq!(
            event_ids.len(),
            TENANTS.len(),
            "case {case}: expected one EventId per published entry, got {event_ids:?}"
        );

        let shards = shard_map(&emulator, case, STREAM_NAME, shard_count).await;
        let want = TENANTS.len() * 2;
        let held = read_shards_until(&emulator, case, STREAM_NAME, &shards, want).await;
        assert_eq!(
            held.len(),
            want,
            "case {case}: expected one record per target per event, got {held:?}"
        );

        // Every record sits where its own key hashes to, which is the whole of
        // what a partition key decides.
        for record in &held {
            let belongs = covering_shard(&shards, &record.key).unwrap_or_else(|| {
                panic!(
                    "case {case}: the key {:?} hashes into no shard of the map",
                    record.key
                )
            });
            assert_eq!(
                record.shard, belongs,
                "case {case}: expected the record keyed {:?} on shard {belongs}, got it on {}",
                record.key, record.shard
            );
        }

        // What each event left behind: one record under the JSON text of the value
        // ITS OWN envelope carries, and one under its own id.
        let mut by_event: BTreeMap<String, Vec<Held>> = BTreeMap::new();
        for record in &held {
            let (event_id, tenant) = delivered(case, &record.data);
            assert!(
                TENANTS.iter().any(|published| json!(published) == tenant),
                "case {case}: a record carries a detail.tenant nothing published: {tenant}"
            );
            by_event.entry(event_id).or_default().push(record.clone());
        }
        for (tenant, event_id) in TENANTS.iter().zip(&event_ids) {
            let records = by_event.get(event_id).cloned().unwrap_or_default();
            assert_eq!(
                records.len(),
                2,
                "case {case} ({tenant}): expected both targets to write a record of event \
                 {event_id}, got {records:?}"
            );
            // The path selects a string, and a key carries the selected value's
            // JSON text, so the key is that tenant quoted.
            let selected = json!(tenant).to_string();
            let keyed = records
                .iter()
                .filter(|record| record.key == selected)
                .count();
            assert_eq!(
                keyed, 1,
                "case {case} ({tenant}): expected the path-keyed record of event {event_id} \
                 under {selected:?}, got {records:?}"
            );
            let defaulted = records
                .iter()
                .filter(|record| is_event_id_key(&record.key, event_id))
                .count();
            assert_eq!(
                defaulted, 1,
                "case {case} ({tenant}): expected the record of the target carrying no \
                 KinesisParameters under the event's own id {event_id}, got {records:?}"
            );
        }

        // One value chooses ONE shard: the two events publishing the same tenant
        // are keyed alike and placed alike.
        let mut shards_by_key: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for record in &held {
            shards_by_key
                .entry(record.key.clone())
                .or_default()
                .insert(record.shard.clone());
        }
        for (key, placed) in &shards_by_key {
            assert_eq!(
                placed.len(),
                1,
                "case {case}: every record keyed {key:?} sits on one shard, got {placed:?}"
            );
        }

        // And the VALUE chooses the shard rather than the stream choosing one for
        // it: over a map of more than one shard the published values are placed on
        // more than one of them, which no key resolved once per target, fixed for
        // the whole stream, or drawn at random could do.
        let selected_keys: BTreeSet<String> = TENANTS
            .iter()
            .map(|tenant| json!(tenant).to_string())
            .collect();
        let placed: BTreeSet<String> = held
            .iter()
            .filter(|record| selected_keys.contains(&record.key))
            .map(|record| record.shard.clone())
            .collect();
        let expected: BTreeSet<String> = selected_keys
            .iter()
            .filter_map(|key| covering_shard(&shards, key))
            .collect();
        assert_eq!(
            placed, expected,
            "case {case}: expected the path-keyed records on the shards their keys hash into \
             ({expected:?}), got {placed:?}"
        );
        if shard_count > 1 {
            assert!(
                placed.len() > 1,
                "case {case}: the published values hash across more than one shard of this map, \
                 got them all on {placed:?}"
            );
        }

        let dead_letters = drain(&emulator, &dead_letter_url).await;
        assert!(
            dead_letters.is_empty(),
            "case {case}: a delivery that succeeded dead-letters nothing, got {dead_letters:?}"
        );
    }
}
