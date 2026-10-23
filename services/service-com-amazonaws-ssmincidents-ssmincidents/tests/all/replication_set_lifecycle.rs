// SPDX-License-Identifier: CC0-1.0

//! `CreateReplicationSet` and `GetReplicationSet`, end to end over the `restJson1`
//! wire.
//!
//! Two rules are under test:
//!
//! * `CreateReplicationSet` refuses EVERY call with the error code
//!   `UnsupportedOperationException` and creates nothing — Amazon Web Services stopped
//!   accepting new Incident Manager customers on November 7, 2025 — whatever the
//!   request carries.
//! * `GetReplicationSet` resolves the requested ARN against the replication sets the
//!   account's store holds: it reports the stored set member for member, and answers an
//!   ARN it holds nothing for with the modelled 404 `ResourceNotFoundException` naming
//!   that ARN — a different class from the 400 `ValidationException` a request outside
//!   the model's own constraints gets.
//!
//! Each suite is a table spanning what the rule generalises over; the request the issue
//! quoted is ONE row. The harness ([`Emulator`]) is the one every suite in this test
//! target shares, so these assertions are about the HTTP behaviour a client sees.
//!
//! The store is seeded directly through the shared engine for the found-path rows,
//! because no operation this emulator implements creates a replication set
//! (`CreateReplicationSet` is the refusal under test here, and the remaining
//! replication-set writers are still scaffold stubs). Seeding the very `crate::state`
//! map the handler reads is what makes the assertions about persisted state rather than
//! about a hardcoded answer.

use aws_smithy_types::DateTime;
use serde_json::{json, Value};

use service_com_amazonaws_ssmincidents_ssmincidents::ops::types::{
    RegionInfo, REGION_STATUS_VALUES, REPLICATION_SET_STATUS_VALUES,
};
use service_com_amazonaws_ssmincidents_ssmincidents::state::{
    replication_set_arn, service_state, ReplicationSetEntry, MAX_REPLICATION_SETS,
};

use super::incident_lifecycle::{query_encode, Emulator, Reply};

/// The canonical placeholder account id every ARN the emulator mints carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The wording `CreateReplicationSet` refuses every call with: Amazon Web Services
/// stopped accepting new Incident Manager customers on November 7, 2025.
const DEPRECATION_MESSAGE: &str = "CreateReplicationSet API is deprecated since Nov 7, 2025.";

/// The longest `arn` the model accepts (`Arn`, `length ..1000`).
const MAX_ARN_LEN: usize = 1000;

/// The prefix the length-bound rows build an ARN from; the rest is padding.
const ARN_PREFIX: &str = "arn:aws:ssm-incidents::123456789012:";

/// `GET /getReplicationSet?arn=<arn>`.
async fn get_replication_set(emulator: &Emulator, arn: &str) -> Reply {
    emulator
        .get(&format!("/getReplicationSet?arn={}", query_encode(arn)))
        .await
}

/// How many replication sets the store holds.
fn stored_count(emulator: &Emulator) -> usize {
    service_state(&*emulator.engine)
        .expect("the ssm-incidents state store")
        .lock()
        .replication_sets
        .len()
}

/// Make `entry` the account's stored replication set, replacing any previous one.
///
/// Incident Manager holds ONE replication set per account
/// ([`MAX_REPLICATION_SETS`]), so a suite that spans several stored shapes replaces
/// rather than accumulates.
fn seed(emulator: &Emulator, entry: ReplicationSetEntry) {
    let state = service_state(&*emulator.engine).expect("the ssm-incidents state store");
    let mut state = state.lock();
    state.replication_sets.clear();
    state.replication_sets.insert(entry.arn.clone(), entry);
    assert!(
        state.replication_sets.len() <= MAX_REPLICATION_SETS,
        "the store must hold at most {MAX_REPLICATION_SETS} replication set(s), holds {}",
        state.replication_sets.len()
    );
}

/// A `RegionInfo` for one Region of a stored replication set.
fn region_info(kms_key: Option<&str>, at: DateTime) -> RegionInfo {
    RegionInfo {
        sse_kms_key_id: kms_key.map(str::to_string),
        // Source: `REGION_STATUS_VALUES` in `ops/types/shared_types.rs`, i.e. the
        // model's `RegionStatus` enum. `[0]` is `ACTIVE`.
        status: REGION_STATUS_VALUES[0].to_string(),
        status_message: None,
        status_update_date_time: at,
    }
}

/// A stored replication set over `regions` (Region name -> optional KMS key id).
fn replication_set(
    id: &str,
    regions: &[(&str, Option<&str>)],
    status: &str,
    deletion_protected: bool,
    at: DateTime,
) -> ReplicationSetEntry {
    let principal = format!("arn:aws:iam::{ACCOUNT}:user/AKIAIOSFODNN7EXAMPLE");
    ReplicationSetEntry {
        arn: replication_set_arn(id),
        region_map: regions
            .iter()
            .map(|(region, key)| ((*region).to_string(), region_info(*key, at)))
            .collect(),
        status: status.to_string(),
        deletion_protected,
        created_time: at,
        created_by: principal.clone(),
        last_modified_time: at,
        last_modified_by: principal,
    }
}

/// `CreateReplicationSet` refuses EVERY call with the error code
/// `UnsupportedOperationException` and creates nothing, whatever the request carries.
///
/// The rows differ along everything the request can vary by — how many Regions it names
/// (the model's `regions` is `length 1..3`), whether those Regions carry a KMS key,
/// whether the request carries `tags`, and whether it carries a `clientToken` (including
/// the same one twice, which on an implemented create would be an idempotent replay).
#[tokio::test]
async fn create_replication_set_refuses_every_request_as_deprecated() {
    let emulator = Emulator::new();

    let cases: Vec<(&str, Value)> = vec![
        ("one region", json!({ "regions": { "us-east-1": {} } })),
        (
            "two regions",
            json!({ "regions": { "us-east-1": {}, "us-west-2": {} } }),
        ),
        (
            "three regions, the modelled maximum",
            json!({ "regions": { "us-east-1": {}, "us-west-2": {}, "eu-west-1": {} } }),
        ),
        (
            "customer managed key",
            json!({
                "regions": {
                    "us-east-1": {
                        "sseKmsKeyId": format!("arn:aws:kms:us-east-1:{ACCOUNT}:key/abcd1234-1111-2222-3333-444455556666")
                    }
                }
            }),
        ),
        (
            "tags",
            json!({
                "regions": { "eu-central-1": {} },
                "tags": { "team": "sre", "env": "prod" }
            }),
        ),
        (
            "client token",
            json!({ "regions": { "ap-south-1": {} }, "clientToken": "replication-set-token" }),
        ),
        (
            "same client token replayed",
            json!({ "regions": { "ap-south-1": {} }, "clientToken": "replication-set-token" }),
        ),
    ];

    for (case, request) in cases {
        let reply = emulator.post("/createReplicationSet", request.clone()).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected 400, got {} body {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "UnsupportedOperationException",
            "case {case:?}: expected UnsupportedOperationException, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.text("/message"),
            DEPRECATION_MESSAGE,
            "case {case:?}: expected the deprecation wording, got {:?}",
            reply.body
        );
        assert!(
            reply.body.get("arn").is_none(),
            "case {case:?}: a refusal must carry no replication-set ARN, got {:?}",
            reply.body
        );
        assert_eq!(
            stored_count(&emulator),
            0,
            "case {case:?}: a refused create must store nothing, store holds {}",
            stored_count(&emulator)
        );
    }
}

/// `GetReplicationSet` resolves the requested ARN against the stored replication sets:
/// an ARN the store holds nothing for is a MISSING RESOURCE — 404
/// `ResourceNotFoundException` naming that ARN — never a 400 and never a 501.
///
/// The rows span the ARN forms the model's `Arn` pattern admits (each partition, an ARN
/// with no account segment, a different account, a differently shaped resource segment)
/// plus the published length bound: 1000 characters is still resolved and answered 404,
/// and one character past it is refused as invalid instead.
#[tokio::test]
async fn get_replication_set_404s_an_arn_the_store_has_no_replication_set_for() {
    let emulator = Emulator::new();

    let at_bound = format!("{ARN_PREFIX}{}", "a".repeat(MAX_ARN_LEN - ARN_PREFIX.len()));
    assert_eq!(
        at_bound.len(),
        MAX_ARN_LEN,
        "the at-bound ARN must be exactly {MAX_ARN_LEN} characters, is {}",
        at_bound.len()
    );

    let cases: Vec<(&str, String)> = vec![
        (
            "emulator account, never created",
            replication_set_arn("0ff1ce00-1111-4222-8333-444455556666"),
        ),
        (
            "another uuid",
            replication_set_arn("deadbeef-9999-4888-8777-666655554444"),
        ),
        (
            "another account",
            "arn:aws:ssm-incidents::210987654321:replication-set/11112222-3333-4444-5555-666677778888"
                .to_string(),
        ),
        (
            "no account segment",
            "arn:aws:ssm-incidents:::replication-set/abcdabcd-1234-4321-8765-0987abcdabcd".to_string(),
        ),
        (
            "china partition",
            "arn:aws-cn:ssm-incidents::123456789012:replication-set/cn-1111-4222-8333-444455556666"
                .to_string(),
        ),
        (
            "govcloud partition",
            "arn:aws-us-gov:ssm-incidents::123456789012:replication-set/gov-111-4222-8333-44445555"
                .to_string(),
        ),
        (
            "a region in the ARN",
            "arn:aws:ssm-incidents:us-east-1:123456789012:replication-set/regional-1111-4222-8333"
                .to_string(),
        ),
        (
            "a resource segment that names no replication set",
            "arn:aws:ssm-incidents::123456789012:response-plan/not-a-replication-set".to_string(),
        ),
        ("at the modelled length bound", at_bound),
    ];

    for (case, arn) in cases {
        let reply = get_replication_set(&emulator, &arn).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected 404, got {} body {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.text("/message"),
            format!("Replication set '{arn}' is not found"),
            "case {case:?}: the message must name the requested ARN, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.text("/resourceIdentifier"),
            arn,
            "case {case:?}: expected the requested ARN as resourceIdentifier, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.text("/resourceType"),
            "REPLICATION_SET",
            "case {case:?}: expected resourceType REPLICATION_SET, got {:?}",
            reply.body
        );
    }

    // One character past the published bound is a different class: the boundary
    // validator refuses it as invalid input, not as a missing resource.
    let too_long = format!(
        "{ARN_PREFIX}{}",
        "a".repeat(MAX_ARN_LEN + 1 - ARN_PREFIX.len())
    );
    let refused = get_replication_set(&emulator, &too_long).await;
    assert_eq!(
        refused.status, 400,
        "case \"past the length bound\": expected 400, got {} body {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ValidationException",
        "case \"past the length bound\": expected ValidationException, got {:?}",
        refused.body
    );
}

/// `GetReplicationSet` reports the replication set the store holds under the requested
/// ARN, member for member — it is a read of state, not a fixed answer — and still 404s
/// every OTHER ARN while one is stored.
///
/// The rows differ along the stored shape: how many Regions the set spans, whether a
/// Region carries a customer managed key, which `ReplicationSetStatus` value it carries
/// (including `CREATING` and `FAILED`, which the issue never cited: the set is reported
/// whatever its status) and whether deletion protection is on.
#[tokio::test]
async fn get_replication_set_reports_the_stored_replication_set() {
    let emulator = Emulator::new();
    let other_arn = replication_set_arn("22223333-4444-5555-6666-777788889999");

    // Status values come from `REPLICATION_SET_STATUS_VALUES` (the model's
    // `ReplicationSetStatus` enum): [0] ACTIVE, [1] CREATING, [3] FAILED.
    let cases: Vec<(&str, ReplicationSetEntry)> = vec![
        (
            "single region, no key, active, unprotected",
            replication_set(
                "aaaa1111-2222-4333-8444-555566667777",
                &[("us-east-1", None)],
                REPLICATION_SET_STATUS_VALUES[0],
                false,
                DateTime::from_secs(1_700_000_000),
            ),
        ),
        (
            "two regions, one customer managed key, creating",
            replication_set(
                "bbbb2222-3333-4444-8555-666677778888",
                &[
                    ("us-east-1", Some("arn:aws:kms:us-east-1:123456789012:key/k1")),
                    ("eu-west-1", None),
                ],
                REPLICATION_SET_STATUS_VALUES[1],
                false,
                DateTime::from_secs(1_700_000_123),
            ),
        ),
        (
            "three regions, deletion protected, failed",
            replication_set(
                "cccc3333-4444-5555-8666-777788889999",
                &[
                    ("us-east-1", Some("arn:aws:kms:us-east-1:123456789012:key/k1")),
                    ("us-west-2", Some("arn:aws:kms:us-west-2:123456789012:key/k2")),
                    ("ap-northeast-1", None),
                ],
                REPLICATION_SET_STATUS_VALUES[3],
                true,
                DateTime::from_secs(1_700_777_777),
            ),
        ),
    ];

    for (case, entry) in cases {
        seed(&emulator, entry.clone());

        // Read it twice: a reporting operation must leave the store exactly as it was,
        // so the second read answers the same thing as the first.
        for attempt in 1..=2 {
            let reply = get_replication_set(&emulator, &entry.arn).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?} attempt {attempt}: expected 200, got {} body {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.text("/replicationSet/arn"),
                entry.arn,
                "case {case:?} attempt {attempt}: expected the stored ARN, got {:?}",
                reply.body
            );
            assert_eq!(
                reply.text("/replicationSet/status"),
                entry.status,
                "case {case:?} attempt {attempt}: expected the stored status, got {:?}",
                reply.body
            );
            assert_eq!(
                reply.boolean("/replicationSet/deletionProtected"),
                Some(entry.deletion_protected),
                "case {case:?} attempt {attempt}: expected the stored deletionProtected, got {:?}",
                reply.body
            );
            assert_eq!(
                reply.number("/replicationSet/createdTime"),
                Some(entry.created_time.secs() as f64),
                "case {case:?} attempt {attempt}: expected the stored createdTime, got {:?}",
                reply.body
            );
            assert_eq!(
                reply.number("/replicationSet/lastModifiedTime"),
                Some(entry.last_modified_time.secs() as f64),
                "case {case:?} attempt {attempt}: expected the stored lastModifiedTime, got {:?}",
                reply.body
            );
            assert_eq!(
                reply.text("/replicationSet/createdBy"),
                entry.created_by,
                "case {case:?} attempt {attempt}: expected the stored createdBy, got {:?}",
                reply.body
            );
            assert_eq!(
                reply.text("/replicationSet/lastModifiedBy"),
                entry.last_modified_by,
                "case {case:?} attempt {attempt}: expected the stored lastModifiedBy, got {:?}",
                reply.body
            );

            let reported = reply
                .body
                .pointer("/replicationSet/regionMap")
                .and_then(Value::as_object)
                .unwrap_or_else(|| panic!("case {case:?}: no regionMap in {:?}", reply.body));
            assert_eq!(
                reported.len(),
                entry.region_map.len(),
                "case {case:?} attempt {attempt}: expected {} Regions, got {:?}",
                entry.region_map.len(),
                reply.body
            );
            for (region, info) in &entry.region_map {
                let pointer = format!("/replicationSet/regionMap/{region}");
                assert_eq!(
                    reply.text(&format!("{pointer}/status")),
                    info.status,
                    "case {case:?} attempt {attempt}: Region {region} status, got {:?}",
                    reply.body
                );
                assert_eq!(
                    reply.number(&format!("{pointer}/statusUpdateDateTime")),
                    Some(info.status_update_date_time.secs() as f64),
                    "case {case:?} attempt {attempt}: Region {region} statusUpdateDateTime, got {:?}",
                    reply.body
                );
                match &info.sse_kms_key_id {
                    Some(key) => assert_eq!(
                        reply.text(&format!("{pointer}/sseKmsKeyId")),
                        key,
                        "case {case:?} attempt {attempt}: Region {region} sseKmsKeyId, got {:?}",
                        reply.body
                    ),
                    None => assert!(
                        reply
                            .body
                            .pointer(&format!("{pointer}/sseKmsKeyId"))
                            .is_none(),
                        "case {case:?} attempt {attempt}: Region {region} must report no key, got {:?}",
                        reply.body
                    ),
                }
            }
        }

        // A stored replication set does not make every ARN resolve: resolution is by
        // ARN, not "the account has one".
        let other = get_replication_set(&emulator, &other_arn).await;
        assert_eq!(
            other.status, 404,
            "case {case:?}: another ARN must still be 404, got {} body {:?}",
            other.status, other.body
        );
        assert_eq!(
            other.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: another ARN must be ResourceNotFoundException, got {:?}",
            other.body
        );

        assert_eq!(
            stored_count(&emulator),
            1,
            "case {case:?}: reads must not change the store, it holds {}",
            stored_count(&emulator)
        );
    }
}

/// The two operations keep their classes apart: the deprecated create refuses with
/// `UnsupportedOperationException`, and the onboarding probe that follows it reports 404
/// `ResourceNotFoundException` — so a client can tell "Incident Manager can no longer be
/// enabled" from "this account has no replication set", and neither reads as an
/// emulator gap (501 `NotImplemented`).
#[tokio::test]
async fn the_onboarding_probe_distinguishes_deprecation_from_an_absent_replication_set() {
    let emulator = Emulator::new();
    let arn = replication_set_arn("99998888-7777-6666-5555-444433332222");

    let created = emulator
        .post(
            "/createReplicationSet",
            json!({ "regions": { "us-east-1": {} } }),
        )
        .await;
    assert_eq!(
        created.error_code(),
        "UnsupportedOperationException",
        "case \"create after deprecation\": expected UnsupportedOperationException, got {:?}",
        created.body
    );

    let probed = get_replication_set(&emulator, &arn).await;
    assert_eq!(
        probed.error_code(),
        "ResourceNotFoundException",
        "case \"probe after a refused create\": expected ResourceNotFoundException, got {:?}",
        probed.body
    );
    for (case, code) in [
        ("create after deprecation", created.error_code()),
        ("probe after a refused create", probed.error_code()),
    ] {
        assert_ne!(
            code, "NotImplemented",
            "case {case:?}: the operation must not answer as a scaffold stub, got {code:?}"
        );
    }
}
