// SPDX-License-Identifier: CC0-1.0

//! The instant a call is served at: read from the clock once the call holds the
//! store, not before it starts waiting for it.
//!
//! The rule under test: every operation that resolves a deployment parameter is
//! served at the instant it REACHES the store, so the clock sample it judges and
//! stamps state with is taken after it holds the store lock. Two consequences a call
//! that had to wait behind another one must show:
//!
//! 1. a record whose `expirationDate` fell due while the call was waiting is already
//!    gone when the call resolves anything — `expirationDate` is "the date when
//!    deployment parameters expire and are scheduled for deletion"
//!    (`smithy_query(query="doc",shape="PutDeploymentParameterRequest",member="expirationDate")`),
//!    so the three tagging operations answer this service's not-found failure
//!    (HTTP 404 `NotFoundException`) for it and `PutDeploymentParameter` mints a
//!    FRESH resource for the identity, exactly as they would had the date passed
//!    before the call arrived;
//! 2. the "last written" instant a write stamps on the record it changed is never
//!    older than the moment that write got into the store, so writes the lock
//!    serialises stamp the record in the order they actually wrote it.
//!
//! A call that waited while the clock took NOTHING is unaffected: the third test
//! spans the same operations against a record whose date is still ahead, and they
//! answer exactly as they do uncontended.
//!
//! Each row drives the real wire path through the production `MultiServiceRouter`
//! while a thread of its own holds the store lock across the instant under test, so
//! what is asserted is what a signed SDK request observes when it is serialised
//! behind another call. The rows span the operations the rule generalises over — the
//! reporting read, the two tagging writes, and the create-or-update — because the
//! rule is about reaching the store, not about which operation does it. The harness
//! (router, signed request, per-operation callers) is the one the
//! `deployment_parameters` suite owns, and the record's two written instants are read
//! with the one reader `tag_write_last_written` owns.

use std::sync::mpsc;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::scaffolded::{
    NOT_FOUND_CODE, NOT_FOUND_TEXT,
};
use service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::state::service_state;
use smithy_server_runtime::MultiServiceRouter;

use crate::deployment_parameters::{
    emulator, error_code, is_parameter_id, list_tags, message, provision, put, tag, tag_map, untag,
    CATALOG,
};
use crate::tag_write_last_written::written_instants;

/// How far ahead of the lock-holder taking the store a row's `expirationDate` is
/// set: far enough that the call under test has started — and, were the clock read
/// too early, would have read it — while the date is still ahead, short enough to
/// keep the row quick.
const DUE_IN: Duration = Duration::from_millis(500);
/// How long past a row's due instant the holder keeps the store locked, so the date
/// has demonstrably passed by the time the waiting call gets in.
const PAST_DUE: Duration = Duration::from_millis(250);
/// How long the holder keeps the store locked in the rows that have no due instant
/// of their own.
const HOLD: Duration = Duration::from_millis(300);
/// The tag every row's record is created with, so a removal has a key to take.
const STORED_TAG: (&str, &str) = ("env", "test");

/// `at` as the RFC 3339 date-time string `expirationDate` travels as.
///
/// The member is `@timestampFormat("date-time")`
/// (`smithy_query(query="operation",operation="PutDeploymentParameter")`), so on
/// restJson1 it is a string in this form — rendered here at millisecond precision,
/// because these rows mean an instant inside a second rather than a whole one.
/// `expiration_date_wire_form` owns the wire-form rule itself.
fn rfc3339_at(at: SystemTime) -> String {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("the test clock is after the Unix epoch");
    let millis = i64::try_from(since_epoch.as_millis()).expect("an epoch millisecond inside i64");
    ::aws_smithy_types::DateTime::from_millis(millis)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("an epoch millisecond renders as an RFC 3339 date-time")
}

/// Hold this service's store lock until `until`, on a thread of its own, and answer
/// the instant the hold ended at.
///
/// Returns once the lock is HELD, so the call the test makes next is one that has to
/// wait for it. The instant is sampled just before the guard is dropped, so anything
/// the waiting call does to the store happens strictly after it — which is what the
/// assertions compare against.
///
/// `pub(crate)` because the sibling `contended_writes_stamp_under_the_store_lock`
/// suite queues a whole BATCH of calls behind the same hold: ONE way of putting a
/// call behind the store lock, so the two suites cannot come to contend differently.
pub(crate) fn hold_store_lock_until(
    engine: &Arc<engine::InMemoryEngine>,
    until: SystemTime,
) -> JoinHandle<SystemTime> {
    let engine = Arc::clone(engine);
    let (took_it, taken) = mpsc::channel();
    let holder = thread::spawn(move || {
        let state =
            service_state(&*engine).expect("the shared engine must hold this service's store");
        let store = state.lock();
        took_it
            .send(())
            .expect("the test must still be waiting for the store lock to be taken");
        thread::sleep(until.duration_since(SystemTime::now()).unwrap_or_default());
        let released_at = SystemTime::now();
        drop(store);
        released_at
    });
    taken
        .recv()
        .expect("the holder thread must take the store lock");
    holder
}

/// One `PutDeploymentParameter` body: the identity members outside the URI, the
/// secret, the stored tag and the optional `expirationDate`.
fn put_body(
    agreement_id: &str,
    name: &str,
    secret: &str,
    expiration_date: Option<SystemTime>,
) -> Value {
    let mut body = json!({
        "agreementId": agreement_id,
        "deploymentParameter": { "name": name, "secretString": secret },
        "tags": tag_map(&[STORED_TAG]),
    });
    if let Some(at) = expiration_date {
        body.as_object_mut()
            .expect("an object body")
            .insert("expirationDate".to_string(), json!(rfc3339_at(at)));
    }
    body
}

/// One row's identity: the target it is written against, recorded out of band, and
/// the parameter name that completes it.
struct Identity {
    product_id: String,
    agreement_id: String,
    name: String,
}

/// Record row `index`'s target and name its identity. One identity per row, so a
/// row's expiry decides only its own resource.
fn row_identity(engine: &Arc<engine::InMemoryEngine>, index: usize) -> Identity {
    let product_id = format!("prod-under-lock-{index:02}");
    let agreement_id = format!("agmt-under-lock-{index:02}");
    provision(engine, CATALOG, &agreement_id, &[&product_id]);
    Identity {
        product_id,
        agreement_id,
        name: format!("Parameter{index:02}"),
    }
}

/// One stored deployment parameter: the ARN the service minted and the id it
/// reported.
struct Stored {
    resource_arn: String,
    parameter_id: String,
}

/// Store the parameter `identity` names, expiring at `expiration_date`.
async fn store_parameter(
    router: &MultiServiceRouter,
    case: &str,
    identity: &Identity,
    expiration_date: Option<SystemTime>,
) -> Stored {
    let (status, created) = put(
        router,
        CATALOG,
        &identity.product_id,
        &put_body(
            &identity.agreement_id,
            &identity.name,
            "s3cr3t",
            expiration_date,
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the setup put must succeed, got {status} {created:?}"
    );
    Stored {
        resource_arn: reported(case, &created, "resourceArn"),
        parameter_id: reported(case, &created, "deploymentParameterId"),
    }
}

/// The string `member` of an answered body.
fn reported(case: &str, body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} member, got {body:?}"))
        .to_string()
}

/// One call that resolves a deployment parameter, as the operation that makes it.
#[derive(Debug, Clone, Copy)]
enum Resolve {
    /// `ListTagsForResource` — the reporting read.
    List,
    /// `TagResource` — a merge onto the record's tag map.
    Tag,
    /// `UntagResource` — a removal from it.
    Untag,
    /// `PutDeploymentParameter` for the identity already stored.
    Put,
}

/// Every operation of this service that resolves a deployment parameter.
const EVERY_RESOLVE: &[Resolve] = &[Resolve::List, Resolve::Tag, Resolve::Untag, Resolve::Put];

/// Make `call` against the row's record, and report its status and body.
async fn perform(
    router: &MultiServiceRouter,
    identity: &Identity,
    stored: &Stored,
    call: Resolve,
) -> (u16, Value) {
    match call {
        Resolve::List => list_tags(router, &stored.resource_arn).await,
        Resolve::Tag => {
            tag(
                router,
                &stored.resource_arn,
                &tag_map(&[("owner", "platform")]),
            )
            .await
        }
        Resolve::Untag => untag(router, &stored.resource_arn, &[STORED_TAG.0.to_string()]).await,
        Resolve::Put => {
            put(
                router,
                CATALOG,
                &identity.product_id,
                &put_body(&identity.agreement_id, &identity.name, "rotated", None),
            )
            .await
        }
    }
}

#[tokio::test]
async fn an_expiry_that_falls_due_while_a_call_waits_for_the_store_is_already_applied() {
    let (router, engine) = emulator();

    for (index, call) in EVERY_RESOLVE.iter().copied().enumerate() {
        let case = format!("{call:?} waiting while the record's expirationDate falls due");
        let identity = row_identity(&engine, index);
        let due = SystemTime::now() + DUE_IN;
        let stored = store_parameter(&router, &case, &identity, Some(due)).await;

        // The record is live as the row starts: its date is still ahead, so nothing
        // this row asserts can come from a date that had already passed.
        let (status, listed) = list_tags(&router, &stored.resource_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the record must be addressable before its date falls due, \
             got {status} {listed:?}"
        );

        let holder = hold_store_lock_until(&engine, due + PAST_DUE);
        let (status, answered) = perform(&router, &identity, &stored, call).await;
        let released_at = holder.join().expect("the holder thread must not panic");
        assert!(
            released_at > due,
            "case {case:?}: the hold must span the due instant {due:?}, released at \
             {released_at:?}"
        );

        match call {
            // The expired record is gone, so the identity is free: this put mints a
            // fresh resource rather than updating the one the clock took.
            Resolve::Put => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} {answered:?}"
                );
                let minted = reported(&case, &answered, "deploymentParameterId");
                assert!(
                    is_parameter_id(&minted),
                    "case {case:?}: expected a minted ^dp-[a-zA-Z0-9]+$ id, got {minted:?}"
                );
                assert_ne!(
                    minted, stored.parameter_id,
                    "case {case:?}: the expired resource is gone, so the put must mint a NEW \
                     deploymentParameterId, got {minted:?}"
                );
            }
            // The tagging operations answer this service's not-found failure for a
            // resource the clock has taken.
            _ => {
                assert_eq!(
                    status, 404,
                    "case {case:?}: expected HTTP 404, got {status} {answered:?}"
                );
                assert_eq!(
                    error_code(&answered),
                    NOT_FOUND_CODE,
                    "case {case:?}: expected the code {NOT_FOUND_CODE}, got {answered:?}"
                );
                assert_eq!(
                    message(&answered),
                    NOT_FOUND_TEXT,
                    "case {case:?}: expected the text {NOT_FOUND_TEXT:?}, got {answered:?}"
                );
            }
        }
    }
}

/// One write under test, as the operation that performs it.
#[derive(Debug, Clone, Copy)]
enum Write {
    /// `TagResource` onto a stored record.
    Tag,
    /// `UntagResource` on a stored record.
    Untag,
    /// `PutDeploymentParameter` rewriting an identity already stored.
    Rewrite,
    /// `PutDeploymentParameter` minting an identity the store does not hold.
    Mint,
}

/// Every write of this service, and the status each answers with.
const EVERY_WRITE: &[(Write, u16)] = &[
    (Write::Tag, 204),
    (Write::Untag, 204),
    (Write::Rewrite, 200),
    (Write::Mint, 200),
];

#[tokio::test]
async fn a_write_that_waited_for_the_store_stamps_the_instant_it_got_in() {
    let (router, engine) = emulator();

    for (index, (write, expected_status)) in EVERY_WRITE.iter().copied().enumerate() {
        let case = format!("{write:?} waiting for the store lock");
        let identity = row_identity(&engine, index);
        // Every row's record stays live: this rule is about the instant the write
        // stamps, not about the expiry. `Mint` is the one row whose identity is NOT
        // stored yet, so its write creates the record instead of changing one.
        let stored = match write {
            Write::Mint => None,
            _ => Some(store_parameter(&router, &case, &identity, None).await),
        };
        let first_written = stored
            .as_ref()
            .map(|stored| written_instants(&engine, &stored.resource_arn));

        let holder = hold_store_lock_until(&engine, SystemTime::now() + HOLD);
        let (status, answered) = match (write, stored.as_ref()) {
            (Write::Tag, Some(stored)) => {
                tag(
                    &router,
                    &stored.resource_arn,
                    &tag_map(&[("owner", "platform")]),
                )
                .await
            }
            (Write::Untag, Some(stored)) => {
                untag(&router, &stored.resource_arn, &[STORED_TAG.0.to_string()]).await
            }
            _ => {
                put(
                    &router,
                    CATALOG,
                    &identity.product_id,
                    &put_body(&identity.agreement_id, &identity.name, "rotated", None),
                )
                .await
            }
        };
        let released_at = holder.join().expect("the holder thread must not panic");
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {answered:?}"
        );

        let resource_arn = match stored.as_ref() {
            Some(stored) => stored.resource_arn.clone(),
            None => reported(&case, &answered, "resourceArn"),
        };
        let (created_at, updated_at) = written_instants(&engine, &resource_arn);
        let now = SystemTime::now();
        assert!(
            updated_at >= released_at && updated_at <= now,
            "case {case:?}: expected the last-written instant inside the window the write \
             actually held the store, {released_at:?}..={now:?}, got {updated_at:?}"
        );
        match first_written {
            // A write onto a stored record leaves the first-written instant where the
            // create put it.
            Some((created_before, _)) => assert_eq!(
                created_at, created_before,
                "case {case:?}: expected the first-written instant to stay {created_before:?}, \
                 got {created_at:?}"
            ),
            // A create stamps both instants from the sample its own call was served
            // at, which is the one it took under the lock.
            None => assert!(
                created_at >= released_at && created_at == updated_at,
                "case {case:?}: expected the create to stamp both instants at one sample no \
                 earlier than {released_at:?}, got {created_at:?} and {updated_at:?}"
            ),
        }
    }
}

#[tokio::test]
async fn a_call_that_waited_is_served_normally_when_the_clock_took_nothing() {
    let (router, engine) = emulator();

    for (index, call) in EVERY_RESOLVE.iter().copied().enumerate() {
        let case = format!("{call:?} waiting while the record's expirationDate stays ahead");
        let identity = row_identity(&engine, index);
        // A date far enough ahead that it cannot fall due inside the hold: the clock
        // owes nothing, so the wait must change no answer.
        let due = SystemTime::now() + Duration::from_secs(3_600);
        let stored = store_parameter(&router, &case, &identity, Some(due)).await;

        let holder = hold_store_lock_until(&engine, SystemTime::now() + HOLD);
        let (status, answered) = perform(&router, &identity, &stored, call).await;
        let released_at = holder.join().expect("the holder thread must not panic");
        assert!(
            released_at < due,
            "case {case:?}: the hold must end before the due instant {due:?}, released at \
             {released_at:?}"
        );

        let expected_status = match call {
            Resolve::List | Resolve::Put => 200,
            Resolve::Tag | Resolve::Untag => 204,
        };
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {answered:?}"
        );
        if matches!(call, Resolve::Put) {
            let reported_id = reported(&case, &answered, "deploymentParameterId");
            assert_eq!(
                reported_id, stored.parameter_id,
                "case {case:?}: the live record is updated in place, so the put must report \
                 {:?}, got {reported_id:?}",
                stored.parameter_id
            );
        }
        // And the record is still there afterwards, whichever call waited.
        let (status, listed) = list_tags(&router, &stored.resource_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the record must still be addressable, got {status} {listed:?}"
        );
    }
}
