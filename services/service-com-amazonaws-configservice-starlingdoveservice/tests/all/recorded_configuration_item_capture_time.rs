// SPDX-License-Identifier: CC0-1.0

//! What happens to a configuration item Config has ALREADY recorded for a resource when
//! the configuration recorder observes that resource again.
//!
//! The rule pinned here: a configuration item's `configurationItemCaptureTime` is the
//! instant the resource was in the state that item records, so it never moves —
//! observing a resource in the state Config already recorded records no new
//! configuration item and leaves the recorded one's capture instant where it is, and only
//! a state the resource actually went into adds an item. Everything a caller builds out
//! of those instants therefore goes on selecting the items it was built from: the
//! instants a history read reports only GROW, and the `[earlierTime, laterTime]` window
//! of `GetResourceConfigHistory` — whose "list contains details about each state of the
//! resource during the specified time interval", `earlierTime` being "the chronologically
//! earliest time in the time range" and `laterTime` "the chronologically latest" —
//! selects exactly the items captured inside it, so the two complementary windows
//! `earlierTime=T` and `laterTime=T` together cover the whole history.
//!
//! Config and S3 are mounted on ONE engine, reusing the harness
//! `config_recorded_resource_compliance` builds: every item read here was captured by a
//! RUNNING recorder observing a bucket through S3's own API, which is the writer that
//! observes one resource over and over. The suites that pin the same reader over states
//! seeded into the store (`get_resource_config_history`) or recorded one by one through
//! `PutResourceConfig` (`recorded_resource_config_history`) never observe a resource
//! twice, so neither can see an item move.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::config_recorded_resource_compliance::Emulator;

/// The operation under test.
const HISTORY: &str = "GetResourceConfigHistory";

/// The resource type Config records an Amazon S3 general purpose bucket under — an entry
/// of `com.amazonaws.configservice#ResourceType`, from `smithy_query(query="shape",
/// shape="ResourceType")`.
const BUCKET_RESOURCE_TYPE: &str = "AWS::S3::Bucket";

/// The bucket versioning statuses `PutBucketVersioning` takes
/// (`com.amazonaws.s3#BucketVersioningStatus`, whose values are `Enabled` and
/// `Suspended`).
const ENABLED: &str = "Enabled";
/// See [`ENABLED`].
const SUSPENDED: &str = "Suspended";

/// How long a poll waits for the recording pass the clock owes before giving up. A pass
/// falls due a fraction of a second after the call that made it due, so this is many
/// passes' worth of slack and not a span any assertion depends on.
const CONVERGENCE_BUDGET: Duration = Duration::from_secs(15);

/// How long a row watches a history it expects NOT to change: several passes' worth of
/// time, so a pass that was going to move an item would have run.
const QUIET_BUDGET: Duration = Duration::from_millis(2_500);

/// How long a poll waits between two reads.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// What the recorder observes between two reads of a bucket's history.
#[derive(Debug, Clone, Copy)]
enum Observed {
    /// The bucket exactly as it already was: no state for Config to record.
    TheSameState,
    /// The bucket's versioning status set to this value, which the recorder reads into a
    /// `supplementaryConfiguration` block of the configuration item.
    Versioning(&'static str),
    /// These tags put on the bucket, which the configuration item carries as its tags.
    Tags(&'static [(&'static str, &'static str)]),
}

/// One history read: the resource key plus the `extra` members (an order, a window
/// bound) the case adds.
fn request(resource_id: &str, extra: Value) -> Value {
    let mut request = json!({
        "resourceType": BUCKET_RESOURCE_TYPE,
        "resourceId": resource_id,
    });
    for (member, value) in extra.as_object().cloned().unwrap_or_default() {
        request[member] = value;
    }
    request
}

/// The instant each item of `body` was captured, as the epoch-seconds number the wire
/// carried it as — which is the only form a client holds a capture instant in, and so the
/// form a window bound is built from.
fn captured_at(case: &str, body: &Value) -> Vec<f64> {
    body["configurationItems"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected configurationItems, got {body}"))
        .iter()
        .map(|item| {
            item["configurationItemCaptureTime"]
                .as_f64()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected an item capture time, got {item}")
                })
        })
        .collect()
}

/// The capture instants of one bucket's history, OLDEST first, read the way a client
/// reads it — or `None` while the recorder has captured no configuration item for the
/// bucket yet, which the read refuses with `ResourceNotDiscoveredException` rather than
/// answering with an empty history.
async fn captured_oldest_first(
    case: &str,
    emulator: &Emulator,
    resource_id: &str,
) -> Option<Vec<f64>> {
    let (status, body) = emulator
        .call(
            "config",
            "StarlingDoveService",
            "1.1",
            HISTORY,
            &request(resource_id, json!({ "chronologicalOrder": "Forward" })),
        )
        .await;
    let not_discovered = body["__type"]
        .as_str()
        .is_some_and(|wire| wire.ends_with("ResourceNotDiscoveredException"));
    if status == 400 && not_discovered {
        return None;
    }
    assert_eq!(
        status, 200,
        "case {case:?}: a history read must answer HTTP 200, got {status} with {body}"
    );
    Some(captured_at(case, &body))
}

/// Read the bucket's history until it reports `want` items, and answer their capture
/// instants oldest first.
///
/// What is waited for is the recording pass the recorder owes, never a value: the
/// assertion that fails at the deadline names the instants the read did report.
async fn await_items(case: &str, emulator: &Emulator, resource_id: &str, want: usize) -> Vec<f64> {
    let deadline = Instant::now() + CONVERGENCE_BUDGET;
    loop {
        let captured = captured_oldest_first(case, emulator, resource_id).await;
        match &captured {
            Some(captured) if captured.len() == want => return captured.clone(),
            _ => assert!(
                Instant::now() < deadline,
                "case {case:?}: expected {want} configuration items, got {captured:?}"
            ),
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Watch the bucket's history for [`QUIET_BUDGET`], requiring every read to report
/// exactly `expected` — the same items, each at the instant it was captured at — and
/// answer the last read.
async fn stays_at(
    case: &str,
    emulator: &Emulator,
    resource_id: &str,
    expected: &[f64],
) -> Vec<f64> {
    let deadline = Instant::now() + QUIET_BUDGET;
    loop {
        let captured = captured_oldest_first(case, emulator, resource_id)
            .await
            .unwrap_or_else(|| {
                panic!("case {case:?}: a resource whose items were recorded keeps its history")
            });
        assert_eq!(
            captured, expected,
            "case {case:?}: expected the history captured at {expected:?} to stand, got \
             {captured:?}"
        );
        if Instant::now() >= deadline {
            return captured;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Put `tags` on `bucket` through S3's own API, replacing the tags it carried.
async fn tag_bucket(emulator: &Emulator, bucket: &str, tags: &[(&str, &str)]) {
    let tag_set: Vec<aws_sdk_s3::types::Tag> = tags
        .iter()
        .map(|(key, value)| {
            aws_sdk_s3::types::Tag::builder()
                .key(*key)
                .value(*value)
                .build()
                .expect("a tag with a key and a value is well formed")
        })
        .collect();
    emulator
        .s3()
        .put_bucket_tagging()
        .bucket(bucket)
        .tagging(
            aws_sdk_s3::types::Tagging::builder()
                .set_tag_set(Some(tag_set))
                .build()
                .expect("a tagging with a tag set is well formed"),
        )
        .send()
        .await
        .unwrap_or_else(|error| panic!("tagging {bucket} must succeed, got {error:?}"));
}

/// Let the recorder observe `change`, and answer the history once it has recorded what
/// there was to record: `added` further items, or none at all, in which case the history
/// is watched for several passes to show it does not change.
async fn observed(
    case: &str,
    emulator: &Emulator,
    bucket: &str,
    change: Observed,
    held: &[f64],
    added: usize,
) -> Vec<f64> {
    match change {
        Observed::TheSameState => {}
        Observed::Versioning(status) => emulator.set_versioning(bucket, status).await,
        Observed::Tags(tags) => tag_bucket(emulator, bucket, tags).await,
    }
    if added == 0 {
        stays_at(case, emulator, bucket, held).await
    } else {
        await_items(case, emulator, bucket, held.len() + added).await
    }
}

/// An emulator whose recorder is RECORDING, holding one bucket whose first configuration
/// item has been captured, plus that item's capture instant.
async fn recording_emulator(bucket: &str) -> (Emulator, Vec<f64>) {
    let emulator = Emulator::new();
    emulator.recording().await;
    emulator.create_bucket(bucket, &[]).await;
    let captured = await_items("the first recorded state", &emulator, bucket, 1).await;
    (emulator, captured)
}

#[tokio::test]
async fn an_already_recorded_configuration_item_keeps_its_capture_instant() {
    let bucket = "capture-instant-bucket";
    let (emulator, mut captured) = recording_emulator(bucket).await;

    // Rows differ along what the recorder observes between two reads of the history: the
    // state it already recorded, over and over, and each kind of change it can see — the
    // bucket's versioning status, which it records in a `supplementaryConfiguration`
    // block, and its tags, which the configuration item carries directly.
    let cases: [(&str, Observed, usize); 6] = [
        ("the same state, several passes of it", Observed::TheSameState, 0),
        ("versioning enabled", Observed::Versioning(ENABLED), 1),
        ("the same state again", Observed::TheSameState, 0),
        ("a tag put on the bucket", Observed::Tags(&[("stage", "prod")]), 1),
        ("another tag value", Observed::Tags(&[("stage", "staging")]), 1),
        ("versioning suspended", Observed::Versioning(SUSPENDED), 1),
    ];

    for (case, change, added) in cases {
        let held = captured.clone();
        captured = observed(case, &emulator, bucket, change, &held, added).await;
        // Oldest first, so the instants a read already reported are a PREFIX of what the
        // next read reports: every one of them is still reported, each still naming the
        // state it named, and only a state the bucket went into is added after them.
        assert!(
            captured.starts_with(&held),
            "case {case:?}: the items captured at {held:?} must still be reported, each at \
             its own instant, got {captured:?}"
        );
        assert_eq!(
            captured.len(),
            held.len() + added,
            "case {case:?}: expected {added} further item(s) after {held:?}, got {captured:?}"
        );
        // Each item records ONE state, so no two of them share a capture instant and
        // they are strictly ordered.
        assert!(
            captured.windows(2).all(|pair| pair[0] < pair[1]),
            "case {case:?}: the capture instants are strictly ascending, got {captured:?}"
        );
    }
}

#[tokio::test]
async fn a_window_selects_the_configuration_items_captured_inside_it() {
    let bucket = "windowed-history-bucket";
    let (emulator, captured) = recording_emulator(bucket).await;

    // Three states of the bucket, so a bound can fall on an item, between two items, or
    // outside the history at either end.
    emulator.set_versioning(bucket, ENABLED).await;
    await_items("the second recorded state", &emulator, bucket, 2).await;
    emulator.set_versioning(bucket, SUSPENDED).await;
    let at = await_items("the third recorded state", &emulator, bucket, 3).await;
    assert!(
        at.starts_with(&captured),
        "the first item must still be reported, got {at:?} after {captured:?}"
    );

    // Several further passes observe the bucket in the state it is in now. They record no
    // item and move none, so the instants read above are still the instants the bounds
    // below are about — which is what makes a window a client builds from a history it
    // read select the items it read.
    let at = stays_at("the history under further passes", &emulator, bucket, &at).await;
    let last = *at.last().expect("three items were recorded");

    // Rows differ along where the bound falls: on each item's own capture instant, in the
    // middle of each gap between two adjacent items (the widest gap included, which is
    // where a client slicing a history puts a pivot), and outside the history at either
    // end. `disjoint` says whether the two complementary windows can share no item, which
    // a bound that is not itself a capture instant cannot.
    let mut bounds: Vec<(String, f64, bool)> = Vec::new();
    for (index, instant) in at.iter().enumerate() {
        bounds.push((format!("the instant item {index} was captured"), *instant, false));
    }
    for (index, pair) in at.windows(2).enumerate() {
        bounds.push((
            format!("the midpoint of the gap after item {index}"),
            pair[0] + (pair[1] - pair[0]) / 2.0,
            true,
        ));
    }
    bounds.push(("a minute before the first item".to_string(), at[0] - 60.0, true));
    // Past the whole history but still in the past: with no `laterTime` the window's
    // later end is "current time", so a bound after that names no interval at all and is
    // refused rather than answered — which the suites pinning this operation's refusals
    // already cover.
    bounds.push(("a second past the last item".to_string(), last + 1.0, true));

    for (case, bound, disjoint) in bounds {
        // What the rule says each window selects: the items whose capture instant falls
        // inside it. Every bound is at least a recording pass away from any item it does
        // not name, so the resolution a reported instant carries cannot decide a row.
        let at_or_after: Vec<f64> = at.iter().copied().filter(|held| *held >= bound).collect();
        let at_or_before: Vec<f64> = at.iter().copied().filter(|held| *held <= bound).collect();

        let mut answered: Vec<Vec<f64>> = Vec::new();
        for (member, expected) in [
            ("earlierTime", &at_or_after),
            ("laterTime", &at_or_before),
        ] {
            let body = emulator
                .config(
                    HISTORY,
                    &request(
                        bucket,
                        json!({ member: bound, "chronologicalOrder": "Forward" }),
                    ),
                )
                .await;
            let reported = captured_at(&case, &body);
            assert_eq!(
                &reported, expected,
                "case {case:?}: {member}={bound} must select the items captured at \
                 {expected:?}, got {reported:?} in {body}"
            );
            // A truncated window would be indistinguishable from an exhausted one, so a
            // page that reported every item the window selects carries no token.
            assert_eq!(
                body["nextToken"].as_str(),
                None,
                "case {case:?}: {member}={bound} reported every item it selects, so it \
                 carries no token, got {body}"
            );
            answered.push(reported);
        }

        // What the two complementary windows ANSWERED partitions the history: together
        // they report every item, and when the bound is not itself a capture instant no
        // item is in both — so a client walking a history in slices neither loses an item
        // nor sees one twice.
        let newer = &answered[0];
        let older = &answered[1];
        let mut union: Vec<f64> = older.iter().chain(newer.iter()).copied().collect();
        union.sort_by(f64::total_cmp);
        union.dedup();
        assert_eq!(
            union, at,
            "case {case:?}: the windows bounded at {bound} must together cover the whole \
             history {at:?}, got {union:?}"
        );
        if disjoint {
            let shared: Vec<f64> = older
                .iter()
                .copied()
                .filter(|held| newer.contains(held))
                .collect();
            assert!(
                shared.is_empty(),
                "case {case:?}: a bound of {bound} is no item's capture instant, so the \
                 two windows share none, got {shared:?}"
            );
        }
    }
}
