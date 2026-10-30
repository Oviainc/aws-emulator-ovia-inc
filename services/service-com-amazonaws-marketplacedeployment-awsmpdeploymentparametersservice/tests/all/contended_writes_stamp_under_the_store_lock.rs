// SPDX-License-Identifier: CC0-1.0

//! What several writes contending for one record owe it: a last-written instant that
//! only ever moves forward.
//!
//! The rule under test: a write is served at the instant it REACHES the store, so the
//! instant it stamps on the record it changed is read off the clock while it holds the
//! store lock. Two consequences a whole BATCH of writes aimed at one record must show,
//! however many of them are in flight and in whatever order the lock lets them in:
//!
//! 1. the instant the record reports afterwards is no earlier than the moment the
//!    batch could first enter the store — no write stamps a reading it took while it
//!    was still waiting its turn;
//! 2. the instant never moves BACKWARDS while the batch lands, so the content a write
//!    leaves and the instant the record reports for it are one write — a record cannot
//!    report a "last written" instant older than content written after it
//!    (`DeploymentParameterRecord::mark_written`).
//!
//! The rows span what a batch can be, along the axes the rule generalises over: which
//! operations contend (`TagResource` merging, `UntagResource` removing, the in-place
//! rewrite `PutDeploymentParameter` performs — the three writes this model has for one
//! stored record), how MANY of them do (two, three, five, eight), and how long each of
//! them has been waiting when it finally gets in (the writes of a batch are started
//! one after another, so they queue with different ages of wait). The second test
//! takes the same contention round after round against one record, which is the axis
//! the first does not cover: the instant one batch leaves is the floor for the next.
//!
//! Each write drives the real wire path through the production `MultiServiceRouter` on
//! a thread and a runtime of its own, so what is asserted is what concurrent signed
//! SDK requests leave behind. Putting a call behind the store lock is the sibling
//! `clock_sample_under_the_store_lock` suite's helper, the record's two written
//! instants are read with the one reader `tag_write_last_written` owns, and the router,
//! signed request and per-operation callers are the `deployment_parameters` harness.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::clock_sample_under_the_store_lock::hold_store_lock_until;
use crate::deployment_parameters::{
    emulator, list_tags, provision, put, tag, tag_map, tags_of, untag, CATALOG,
};
use crate::tag_write_last_written::written_instants;

/// How long a batch is kept queued behind the store lock before every write in it is
/// released at once: long enough that each write of the largest batch has been issued
/// — and, were the clock read before the wait, would have read it — while the lock is
/// still held, short enough to keep a row quick.
const HOLD: Duration = Duration::from_millis(400);
/// How long after the previous one each write of a batch is started, so the writes
/// queue with different ages of wait instead of all arriving at one instant.
const STAGGER: Duration = Duration::from_millis(25);
/// How often the record's last-written instant is read while a batch is in flight.
const OBSERVE_EVERY: Duration = Duration::from_millis(2);
/// The most observations one batch takes, so an observer whose batch never finishes
/// ends on its own instead of hanging the suite.
const MAX_OBSERVATIONS: usize = 2_000;
/// The tag every record is created with, so a removal in a batch has a key to take.
const STORED_TAG: (&str, &str) = ("env", "test");
/// The value every merge in a batch writes, so the merged keys are checkable whichever
/// order the writes landed in.
const MERGED_VALUE: &str = "platform";

/// Sleep until `at`, or not at all when it has already passed.
fn sleep_until(at: SystemTime) {
    thread::sleep(at.duration_since(SystemTime::now()).unwrap_or_default());
}

/// One row's identity: the target recorded for it out of band, and the parameter name
/// that completes it.
#[derive(Clone, Debug)]
struct Identity {
    product_id: String,
    agreement_id: String,
    name: String,
}

/// The create body for one row's record: the identity members outside the URI, a
/// secret, and the tag a removal in the batch can take.
fn create_body(identity: &Identity) -> Value {
    json!({
        "agreementId": identity.agreement_id,
        "deploymentParameter": { "name": identity.name, "secretString": "s3cr3t" },
        "tags": tag_map(&[STORED_TAG]),
    })
}

/// The body of an in-place rewrite of `identity`: the same identity, fresh content.
///
/// It carries no `tags` member, because an identity the store already holds is
/// rewritten with its tag map untouched — so a rewrite contending with the tagging
/// writes cannot put back what one of them removed.
fn rewrite_body(identity: &Identity, secret: &str) -> Value {
    json!({
        "agreementId": identity.agreement_id,
        "deploymentParameter": { "name": identity.name, "secretString": secret },
    })
}

/// Record row `index`'s target, store its deployment parameter, and answer the
/// identity and the ARN the service minted for it.
async fn store_record(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    index: usize,
) -> (Identity, String) {
    let identity = Identity {
        product_id: format!("prod-contended-{index:02}"),
        agreement_id: format!("agmt-contended-{index:02}"),
        name: format!("Parameter{index:02}"),
    };
    provision(
        engine,
        CATALOG,
        &identity.agreement_id,
        &[&identity.product_id],
    );
    let (status, created) = put(
        router,
        CATALOG,
        &identity.product_id,
        &create_body(&identity),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the setup put must succeed, got {status} {created:?}"
    );
    let resource_arn = created
        .get("resourceArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a minted resourceArn, got {created:?}"))
        .to_string();
    (identity, resource_arn)
}

/// One write of a batch, as the operation that performs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Write {
    /// `TagResource`, merging a key of this write's own onto the record's tag map.
    Merge,
    /// `UntagResource`, removing [`STORED_TAG`] from it.
    Remove,
    /// `PutDeploymentParameter` rewriting the identity already stored.
    Rewrite,
}

impl Write {
    /// The status this write answers with.
    fn status(self) -> u16 {
        match self {
            Write::Merge | Write::Remove => 204,
            Write::Rewrite => 200,
        }
    }

    /// The tag key a merge writes as the `index`th write of its batch; `None` for a
    /// write that merges nothing.
    fn merged_key(self, index: usize) -> Option<String> {
        match self {
            Write::Merge => Some(format!("owner-{index:02}")),
            _ => None,
        }
    }
}

/// Start write `index` of a batch on a thread and a runtime of its own, to be issued
/// at `start_at`, and answer the status and body it gets back.
///
/// A thread each because a write that reaches the store waits for the lock on the
/// thread it runs on: concurrency on one runtime would serialise the batch before it
/// ever reached the store, which is the opposite of what these rows need.
fn spawn_write(
    router: &MultiServiceRouter,
    identity: &Identity,
    resource_arn: &str,
    index: usize,
    write: Write,
    start_at: SystemTime,
) -> JoinHandle<(u16, Value)> {
    let router = router.clone();
    let identity = identity.clone();
    let resource_arn = resource_arn.to_string();
    let merged_key = write.merged_key(index);
    thread::spawn(move || {
        let runtime = ::tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a current-thread runtime for one write");
        sleep_until(start_at);
        runtime.block_on(async move {
            match write {
                Write::Merge => {
                    let key = merged_key.expect("a merge names the key it writes");
                    tag(
                        &router,
                        &resource_arn,
                        &tag_map(&[(key.as_str(), MERGED_VALUE)]),
                    )
                    .await
                }
                Write::Remove => {
                    untag(&router, &resource_arn, &[STORED_TAG.0.to_string()]).await
                }
                Write::Rewrite => {
                    put(
                        &router,
                        CATALOG,
                        &identity.product_id,
                        &rewrite_body(&identity, &format!("rotated-{index:02}")),
                    )
                    .await
                }
            }
        })
    })
}

/// Read the record's last-written instant under the store lock over and over until
/// `stop` is set, and answer every instant observed, in the order observed.
///
/// Every read goes through the store lock, so each one sees the record as the write
/// that landed most recently left it: the sequence is the record's last-written
/// instant as it moves while a batch lands on it.
fn observe_last_written(
    engine: &Arc<engine::InMemoryEngine>,
    resource_arn: &str,
    stop: &Arc<AtomicBool>,
) -> JoinHandle<Vec<SystemTime>> {
    let engine = Arc::clone(engine);
    let resource_arn = resource_arn.to_string();
    let stop = Arc::clone(stop);
    thread::spawn(move || {
        let mut observed = Vec::new();
        for _ in 0..MAX_OBSERVATIONS {
            if stop.load(Ordering::Acquire) {
                break;
            }
            let (_, updated_at) = written_instants(&engine, &resource_arn);
            observed.push(updated_at);
            thread::sleep(OBSERVE_EVERY);
        }
        observed
    })
}

/// What one contended batch left behind.
struct Landed {
    /// The instant the batch could first enter the store: the hold's release.
    released_at: SystemTime,
    /// The record's first- and last-written instants once every write had landed.
    instants: (SystemTime, SystemTime),
    /// The last-written instant as the observer saw it move, in the order observed.
    observed: Vec<SystemTime>,
}

/// Queue `writes` behind the store lock, release them all at once, and answer what the
/// batch left on the record — asserting on the way that every write was issued while
/// the lock was still held and that each answered its own status.
async fn land_contended_batch(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    identity: &Identity,
    resource_arn: &str,
    writes: &[Write],
) -> Landed {
    let starts_at = SystemTime::now();
    let release_at = starts_at + HOLD;
    let last_start = starts_at + STAGGER * u32::try_from(writes.len()).expect("a small batch");
    assert!(
        last_start < release_at,
        "case {case:?}: the batch must be fully issued inside the hold, last write starts \
         at {last_start:?} and the hold ends at {release_at:?}"
    );

    let stop = Arc::new(AtomicBool::new(false));
    let observer = observe_last_written(engine, resource_arn, &stop);
    let holder = hold_store_lock_until(engine, release_at);
    let running: Vec<JoinHandle<(u16, Value)>> = writes
        .iter()
        .copied()
        .enumerate()
        .map(|(index, write)| {
            spawn_write(
                router,
                identity,
                resource_arn,
                index,
                write,
                starts_at + STAGGER * u32::try_from(index).expect("a small batch"),
            )
        })
        .collect();

    let released_at = holder.join().expect("the holder thread must not panic");
    for (index, (write, running)) in writes.iter().copied().zip(running).enumerate() {
        let (status, answered) = running.join().expect("a write thread must not panic");
        assert_eq!(
            status,
            write.status(),
            "case {case:?}: write {index} ({write:?}) expected HTTP {}, got {status} {answered:?}",
            write.status()
        );
    }
    stop.store(true, Ordering::Release);
    let observed = observer.join().expect("the observer thread must not panic");

    Landed {
        released_at,
        instants: written_instants(engine, resource_arn),
        observed,
    }
}

/// Assert the record's last-written instant never moved backwards while the batch
/// landed, and that every instant observed is inside the window the batch ran in.
fn assert_moved_only_forward(case: &str, landed: &Landed, before: SystemTime, now: SystemTime) {
    let (_, updated_at) = landed.instants;
    assert!(
        updated_at >= landed.released_at && updated_at <= now,
        "case {case:?}: expected the last-written instant inside the window the batch \
         actually held the store, {:?}..={now:?}, got {updated_at:?}",
        landed.released_at
    );
    assert!(
        !landed.observed.is_empty(),
        "case {case:?}: expected the observer to read the record at least once"
    );
    for pair in landed.observed.windows(2) {
        assert!(
            pair[1] >= pair[0],
            "case {case:?}: expected the last-written instant never to move backwards, \
             observed {:?} then {:?}",
            pair[0],
            pair[1]
        );
    }
    for observed in &landed.observed {
        assert!(
            *observed >= before && *observed <= updated_at,
            "case {case:?}: expected every observed last-written instant inside \
             {before:?}..={updated_at:?}, got {observed:?}"
        );
    }
}

/// The batch table: the mixes of contending writes, and how many of each mix.
const BATCHES: &[(&str, &[Write])] = &[
    ("two merges contending", &[Write::Merge, Write::Merge]),
    (
        "a merge and a removal contending",
        &[Write::Merge, Write::Remove],
    ),
    (
        "a merge and an in-place rewrite contending",
        &[Write::Merge, Write::Rewrite],
    ),
    (
        "all three writes of this model contending",
        &[Write::Merge, Write::Remove, Write::Rewrite],
    ),
    (
        "five writes contending, the rewrite between the tagging writes",
        &[
            Write::Merge,
            Write::Rewrite,
            Write::Remove,
            Write::Merge,
            Write::Rewrite,
        ],
    ),
    (
        "eight writes contending",
        &[
            Write::Merge,
            Write::Merge,
            Write::Rewrite,
            Write::Merge,
            Write::Remove,
            Write::Merge,
            Write::Rewrite,
            Write::Merge,
        ],
    ),
];

#[tokio::test]
async fn contended_writes_stamp_the_instant_they_got_into_the_store() {
    let (router, engine) = emulator();

    for (index, (case, writes)) in BATCHES.iter().copied().enumerate() {
        let (identity, resource_arn) = store_record(&router, &engine, case, index).await;
        let (created_at, before) = written_instants(&engine, &resource_arn);

        let landed = land_contended_batch(
            &router,
            &engine,
            case,
            &identity,
            &resource_arn,
            writes,
        )
        .await;
        let now = SystemTime::now();
        assert_moved_only_forward(case, &landed, before, now);

        let (created_at_after, updated_at) = landed.instants;
        assert_eq!(
            created_at_after, created_at,
            "case {case:?}: expected the first-written instant to stay {created_at:?}, got \
             {created_at_after:?}"
        );
        assert!(
            updated_at > before,
            "case {case:?}: expected the batch to move the last-written instant past the \
             create's {before:?}, got {updated_at:?}"
        );

        // And the content is every write of the batch, so the instant above is the
        // instant of a record all of them wrote — not of a write that got lost.
        let (status, listed) = list_tags(&router, &resource_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 reading the tags back, got {status} {listed:?}"
        );
        let tags = tags_of(&listed);
        for (write_index, write) in writes.iter().copied().enumerate() {
            if let Some(key) = write.merged_key(write_index) {
                assert_eq!(
                    tags.get(&key).map(String::as_str),
                    Some(MERGED_VALUE),
                    "case {case:?}: expected the merge of write {write_index} to leave \
                     {key}={MERGED_VALUE}, got {tags:?}"
                );
            }
        }
        let removed = writes.contains(&Write::Remove);
        assert_eq!(
            tags.contains_key(STORED_TAG.0),
            !removed,
            "case {case:?}: expected the stored key {:?} {} the batch, got {tags:?}",
            STORED_TAG.0,
            if removed {
                "taken by"
            } else {
                "left alone by"
            }
        );
    }
}

#[tokio::test]
async fn one_contended_batch_is_the_floor_for_the_next() {
    // The rule is about the instant a write got into the store, so the instant one
    // batch leaves on a record is a floor no later batch may fall below, however many
    // writes each of them contended with.
    let (router, engine) = emulator();
    let case = "three contended batches, one after another, against one record";
    let (identity, resource_arn) = store_record(&router, &engine, case, 0).await;
    let (created_at, mut floor) = written_instants(&engine, &resource_arn);

    let rounds: &[&[Write]] = &[
        &[Write::Merge, Write::Merge],
        &[Write::Remove, Write::Rewrite, Write::Merge],
        &[Write::Rewrite, Write::Merge, Write::Remove, Write::Rewrite],
    ];

    for (round, writes) in rounds.iter().copied().enumerate() {
        let case = format!("{case} — round {round}");
        let landed = land_contended_batch(
            &router,
            &engine,
            &case,
            &identity,
            &resource_arn,
            writes,
        )
        .await;
        let now = SystemTime::now();
        assert_moved_only_forward(&case, &landed, floor, now);

        let (created_at_after, updated_at) = landed.instants;
        assert!(
            updated_at > floor,
            "case {case:?}: expected the round to move the last-written instant past the \
             previous round's {floor:?}, got {updated_at:?}"
        );
        assert_eq!(
            created_at_after, created_at,
            "case {case:?}: expected the first-written instant to stay {created_at:?}, got \
             {created_at_after:?}"
        );
        floor = updated_at;
    }
}
