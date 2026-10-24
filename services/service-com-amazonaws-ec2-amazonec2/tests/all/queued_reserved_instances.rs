// SPDX-License-Identifier: CC0-1.0

//! What `DeleteQueuedReservedInstances` deletes, what it reports per id, and the values it
//! refuses — end to end over the wire the service actually speaks: `ec2Query`, an
//! `application/x-www-form-urlencoded` POST whose body carries
//! `Action=DeleteQueuedReservedInstances`, answered with XML (README.md / OPERATIONS.md
//! "Wire protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rules under test:
//!
//! * A Reserved Instance in the `queued` state has its queued purchase deleted and moves
//!   to `queued-deleted` — a member of the modelled `ReservedInstanceState` enum, which is
//!   why the record is kept rather than dropped — and the id comes back in
//!   `SuccessfulQueuedPurchaseDeletions`. Every other id the request names is reported in
//!   `FailedQueuedPurchaseDeletions` with the modelled reason
//!   (`smithy_query(query="shape",shape="DeleteQueuedReservedInstancesErrorCode")`):
//!   `reserved-instances-id-invalid` for an id the account holds no Reserved Instance for,
//!   `reserved-instances-not-in-queued-state` for one whose Reserved Instance is in any
//!   other state. That per-id channel is the whole point of the operation's output, so
//!   neither case fails the call.
//! * A value that is not a Reserved Instances id at all — an id is the hyphenated form of
//!   a UUID, not EC2's `<prefix>-<hex>` form — is a parameter EC2 cannot read, so the
//!   whole call is refused with `InvalidParameterValue` and nothing in the batch is
//!   deleted.
//! * `DryRun` answers `DryRunOperation` without deleting anything.

use std::time::{Duration, SystemTime};

use ec2::ops::delete_queued_reserved_instances::{
    MAX_STORED_RESERVED_INSTANCES, RESERVED_INSTANCE_STATE_ACTIVE, RESERVED_INSTANCE_STATE_QUEUED,
    RESERVED_INSTANCE_STATE_QUEUED_DELETED, RESERVED_INSTANCES_ID_INVALID,
    RESERVED_INSTANCES_NOT_IN_QUEUED_STATE, ReservedInstancesSpec,
};
use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{error_code, indexed};
use super::wire::{Param, child, child_text, document, param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The modelled length ceiling on `DeleteQueuedReservedInstancesIdList`: "len 1..100"
/// (`smithy_query(query="operation",operation="DeleteQueuedReservedInstances")`).
const MAX_IDS_PER_REQUEST: usize = 100;

/// How far ahead of now a queued purchase is scheduled: a queued purchase is one AWS has
/// not made yet, so its `Start` is in the future. Any future instant will do — nothing
/// here depends on how far.
const QUEUED_PURCHASE_LEAD: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// A one-year Reserved Instance term, in seconds — the units `ReservedInstances$Duration`
/// is reported in ("the duration of the Reserved Instance, in seconds").
const ONE_YEAR_SECONDS: i64 = 365 * 24 * 60 * 60;

/// The results one `DeleteQueuedReservedInstances` response carries, in document order.
#[derive(Debug, Default, PartialEq, Eq)]
struct Deletions {
    /// The `ReservedInstancesId` of each `SuccessfulQueuedPurchaseDeletions` entry.
    deleted: Vec<String>,
    /// The `ReservedInstancesId` and error `Code` of each
    /// `FailedQueuedPurchaseDeletions` entry.
    failed: Vec<(String, String)>,
}

/// One purchase to seed in a table row: its instance type, its Availability Zone (`None`
/// for a Regional-scope purchase) and its term in seconds.
type PurchaseRow = (&'static str, Option<&'static str>, i64);

/// One row of the delete table: the case, the purchases to seed, and how many times each
/// seeded id is named in the one request.
type DeleteCase = (&'static str, &'static [PurchaseRow], usize);

/// One `DeleteQueuedReservedInstances` call naming `ids`, as its HTTP status and XML body.
async fn delete_queued(
    router: &MultiServiceRouter,
    ids: &[&str],
    extra: &[Param],
) -> (u16, String) {
    let mut params = indexed("ReservedInstancesId", ids);
    params.extend_from_slice(extra);
    super::ec2_query::call(router, REGION, "DeleteQueuedReservedInstances", &params).await
}

/// The two result sets of a response body, read through the document rather than by string
/// surgery so a per-item `<code>` is never confused with an error envelope's `<Code>`.
fn deletions(case: &str, body: &str) -> Deletions {
    let doc = document(case, body);
    let items = |set_name: &str| -> Vec<roxmltree::Node<'_, '_>> {
        doc.descendants()
            .find(|node| node.is_element() && node.tag_name().name() == set_name)
            .map(|set| {
                set.children()
                    .filter(|item| item.is_element() && item.tag_name().name() == "item")
                    .collect()
            })
            .unwrap_or_default()
    };
    let deleted = items("successfulQueuedPurchaseDeletionSet")
        .into_iter()
        .map(|item| child_text(item, "reservedInstancesId").unwrap_or_default())
        .collect();
    let failed = items("failedQueuedPurchaseDeletionSet")
        .into_iter()
        .map(|item| {
            let id = child_text(item, "reservedInstancesId").unwrap_or_default();
            let code = child(item, "error")
                .and_then(|error| child_text(error, "code"))
                .unwrap_or_default();
            (id, code)
        })
        .collect();
    Deletions { deleted, failed }
}

/// A Reserved Instance purchase to seed, in `state`.
///
/// The enum members are the modelled catalogs' own: `standard` of
/// `OFFERING_CLASS_TYPE_VALUES`, `All Upfront` of `OFFERING_TYPE_VALUES_VALUES`,
/// `Linux/UNIX` of `R_I_PRODUCT_DESCRIPTION_VALUES`, and an `InstanceType` of
/// `INSTANCE_TYPE_VALUES`.
fn purchase(
    state: &str,
    instance_type: &str,
    availability_zone: Option<&str>,
    duration_seconds: i64,
) -> ReservedInstancesSpec {
    ReservedInstancesSpec {
        instance_type: instance_type.to_string(),
        availability_zone: availability_zone.map(str::to_string),
        region: REGION.to_string(),
        instance_count: 2,
        duration_seconds,
        offering_class: "standard".to_string(),
        offering_type: "All Upfront".to_string(),
        product_description: "Linux/UNIX".to_string(),
        state: state.to_string(),
        start: SystemTime::now() + QUEUED_PURCHASE_LEAD,
        fixed_price: 500.0,
        usage_price: 0.0,
    }
}

/// Seed one Reserved Instance and answer the id the store minted for it.
fn seed(engine: &engine::InMemoryEngine, spec: ReservedInstancesSpec) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_reserved_instances(spec)
        .expect("the store has room for the few purchases this suite seeds")
}

/// The `State` the store holds for `reserved_instances_id`, or `None` when it holds no such
/// Reserved Instance.
fn stored_state(engine: &engine::InMemoryEngine, reserved_instances_id: &str) -> Option<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .reserved_instances
        .get(reserved_instances_id)
        .and_then(|reserved_instances| reserved_instances.state.clone())
}

/// How many Reserved Instances the store holds.
fn stored_count(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().reserved_instances.len()
}

/// A well-formed Reserved Instances id — the hyphenated UUID form — that names nothing,
/// built from `n` so each row of a table gets its own.
fn unknown_id(n: usize) -> String {
    format!("00000000-0000-4000-8000-{n:012x}")
}

// ---------------------------------------------------------------------------
// The purchases a delete takes, and what it reports per id
// ---------------------------------------------------------------------------

/// The rule: every id the request names that holds a `queued` purchase has that purchase
/// deleted — the Reserved Instance moves to `queued-deleted` and the id is reported in
/// `SuccessfulQueuedPurchaseDeletions` — and a repeat of the same call reports the same
/// ids as no longer queued, so the delete is observable on the wire.
///
/// The rows differ along what the rule generalises over: how many purchases one call
/// deletes, the shape of each purchase (instance type, term, zonal vs Regional scope), and
/// whether the batch names an id more than once.
#[tokio::test]
async fn a_queued_purchase_is_deleted_and_stops_being_queued() {
    // (case, the purchases to seed, how many times each seeded id is named)
    let cases: &[DeleteCase] = &[
        (
            "one zonal purchase",
            &[("t3.micro", Some("us-east-1a"), ONE_YEAR_SECONDS)],
            1,
        ),
        (
            "one Regional purchase",
            &[("m5.large", None, ONE_YEAR_SECONDS)],
            1,
        ),
        (
            "three purchases of different types, terms and scopes",
            &[
                ("t3.micro", Some("us-east-1a"), ONE_YEAR_SECONDS),
                ("m5.large", None, 3 * ONE_YEAR_SECONDS),
                ("c5.xlarge", Some("us-east-1c"), ONE_YEAR_SECONDS),
            ],
            1,
        ),
        (
            "a batch that names each purchase three times",
            &[
                ("t3.micro", Some("us-east-1b"), ONE_YEAR_SECONDS),
                ("r5.2xlarge", None, 3 * ONE_YEAR_SECONDS),
            ],
            3,
        ),
    ];

    for (case, specs, repeats) in cases {
        let (router, engine) = super::ec2_query::emulator();
        let seeded: Vec<String> = specs
            .iter()
            .map(|(instance_type, zone, duration)| {
                seed(
                    &engine,
                    purchase(
                        RESERVED_INSTANCE_STATE_QUEUED,
                        instance_type,
                        *zone,
                        *duration,
                    ),
                )
            })
            .collect();
        let named: Vec<&str> = seeded
            .iter()
            .flat_map(|id| std::iter::repeat_n(id.as_str(), *repeats))
            .collect();

        let (status, body) = delete_queued(&router, &named, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let results = deletions(case, &body);
        assert_eq!(
            results.deleted, seeded,
            "case {case:?}: every queued purchase must be reported deleted once, in request \
             order, got {:?} for {seeded:?}",
            results.deleted
        );
        assert!(
            results.failed.is_empty(),
            "case {case:?}: no id must fail, got {:?}",
            results.failed
        );
        for id in &seeded {
            assert_eq!(
                stored_state(&engine, id).as_deref(),
                Some(RESERVED_INSTANCE_STATE_QUEUED_DELETED),
                "case {case:?}: {id} must be left in '{RESERVED_INSTANCE_STATE_QUEUED_DELETED}', \
                 got {:?}",
                stored_state(&engine, id)
            );
        }
        assert_eq!(
            stored_count(&engine),
            seeded.len(),
            "case {case:?}: a deleted queued purchase is kept, not dropped: the store holds \
             {} of {} Reserved Instances",
            stored_count(&engine),
            seeded.len()
        );

        // The same call again: the purchases are no longer queued, so the delete that
        // happened is visible through this operation alone.
        let (status, body) = delete_queued(&router, &named, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the repeat, got {status} with body {body}"
        );
        let repeat = deletions(case, &body);
        let expected: Vec<(String, String)> = seeded
            .iter()
            .map(|id| {
                (
                    id.clone(),
                    RESERVED_INSTANCES_NOT_IN_QUEUED_STATE.to_string(),
                )
            })
            .collect();
        assert_eq!(
            repeat.failed, expected,
            "case {case:?}: a purchase already deleted must come back as \
             '{RESERVED_INSTANCES_NOT_IN_QUEUED_STATE}', got {:?}",
            repeat.failed
        );
        assert!(
            repeat.deleted.is_empty(),
            "case {case:?}: the repeat must delete nothing, got {:?}",
            repeat.deleted
        );
    }
}

/// The rule: an id the account holds no Reserved Instance for is reported per id as
/// `reserved-instances-id-invalid` on a successful call, and nothing is stored for it.
///
/// The rows differ in the form of the (well-formed) id: its value, its digit case and the
/// UUID version nibble it carries, plus how many such ids one call names.
#[tokio::test]
async fn an_id_that_names_no_reserved_instance_is_reported_as_a_failed_deletion() {
    // (case, the ids to name)
    let cases: &[(&str, &[&str])] = &[
        ("one unknown id", &["b847fa93-e282-4f55-b59a-1342f5bd7c02"]),
        (
            "the same id in upper case",
            &["B847FA93-E282-4F55-B59A-1342F5BD7C02"],
        ),
        (
            "ids carrying other UUID version nibbles",
            &[
                "00000000-0000-1000-8000-000000000001",
                "0f0e0d0c-0b0a-5908-8706-050403020100",
                "ffffffff-ffff-ffff-ffff-ffffffffffff",
            ],
        ),
        (
            "an all-zero id",
            &["00000000-0000-0000-0000-000000000000"],
        ),
    ];

    for (case, ids) in cases {
        let (router, engine) = super::ec2_query::emulator();
        let before = stored_count(&engine);

        let (status, body) = delete_queued(&router, ids, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an unknown id is a per-id result, not a request failure: got \
             {status} with body {body}"
        );
        let results = deletions(case, &body);
        let expected: Vec<(String, String)> = ids
            .iter()
            .map(|id| ((*id).to_string(), RESERVED_INSTANCES_ID_INVALID.to_string()))
            .collect();
        assert_eq!(
            results.failed, expected,
            "case {case:?}: every unknown id must fail with \
             '{RESERVED_INSTANCES_ID_INVALID}', got {:?}",
            results.failed
        );
        assert!(
            results.deleted.is_empty(),
            "case {case:?}: nothing must be reported deleted, got {:?}",
            results.deleted
        );
        assert_eq!(
            stored_count(&engine),
            before,
            "case {case:?}: an unknown id must store nothing: the store holds {} of {before} \
             Reserved Instances",
            stored_count(&engine)
        );
    }
}

/// The rule: only a Reserved Instance in the `queued` state carries a queued purchase, so
/// one in any other state is reported as `reserved-instances-not-in-queued-state` and is
/// left in the state it held.
///
/// The rows are EVERY member of the modelled `ReservedInstanceState` enum
/// (`ec2::ops::types::RESERVED_INSTANCE_STATE_VALUES`), so the table covers the whole
/// catalog rather than the one state an example happens to carry: `queued` is deleted and
/// each of the others is refused.
#[tokio::test]
async fn only_a_queued_reserved_instance_has_a_purchase_to_delete() {
    for state in ec2::ops::types::RESERVED_INSTANCE_STATE_VALUES {
        let case = format!("a Reserved Instance in state {state:?}");
        let (router, engine) = super::ec2_query::emulator();
        let id = seed(
            &engine,
            purchase(state, "t3.micro", Some("us-east-1a"), ONE_YEAR_SECONDS),
        );

        let (status, body) = delete_queued(&router, &[id.as_str()], &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let results = deletions(&case, &body);
        if *state == RESERVED_INSTANCE_STATE_QUEUED {
            assert_eq!(
                results.deleted,
                vec![id.clone()],
                "case {case:?}: a queued purchase must be deleted, got {:?} / {:?}",
                results.deleted,
                results.failed
            );
            assert_eq!(
                stored_state(&engine, &id).as_deref(),
                Some(RESERVED_INSTANCE_STATE_QUEUED_DELETED),
                "case {case:?}: the deleted purchase must be left in \
                 '{RESERVED_INSTANCE_STATE_QUEUED_DELETED}', got {:?}",
                stored_state(&engine, &id)
            );
            continue;
        }
        assert_eq!(
            results.failed,
            vec![(
                id.clone(),
                RESERVED_INSTANCES_NOT_IN_QUEUED_STATE.to_string()
            )],
            "case {case:?}: expected '{RESERVED_INSTANCES_NOT_IN_QUEUED_STATE}', got {:?} / {:?}",
            results.failed,
            results.deleted
        );
        assert!(
            results.deleted.is_empty(),
            "case {case:?}: nothing must be reported deleted, got {:?}",
            results.deleted
        );
        assert_eq!(
            stored_state(&engine, &id).as_deref(),
            Some(*state),
            "case {case:?}: the Reserved Instance must be left in the state it held, got {:?}",
            stored_state(&engine, &id)
        );
    }
}

// ---------------------------------------------------------------------------
// The values the call itself refuses
// ---------------------------------------------------------------------------

/// The rule: a value that is not a Reserved Instances id — an id is the hyphenated form of
/// a UUID — is a parameter the service cannot read, so the whole call is refused with
/// `InvalidParameterValue` and no purchase the batch names is deleted.
///
/// The rows differ in HOW the value misses that form: another resource's `<prefix>-<hex>`
/// id form, free text, the other three forms a UUID can be written in (simple, braced,
/// URN), a hexadecimal digit too few, and a non-hexadecimal digit in the right shape.
#[tokio::test]
async fn a_value_that_is_not_a_reserved_instances_id_is_refused() {
    // (case, the value named beside a real queued purchase)
    let cases: &[(&str, &str)] = &[
        ("an EC2 reservation id", "r-00000000000000000"),
        ("an instance id", "i-0123456789abcdef0"),
        ("free text", "not-a-reserved-instance"),
        (
            "the simple (unhyphenated) UUID form",
            "b847fa93e2824f55b59a1342f5bd7c02",
        ),
        (
            "the braced UUID form",
            "{b847fa93-e282-4f55-b59a-1342f5bd7c02}",
        ),
        (
            "the URN UUID form",
            "urn:uuid:b847fa93-e282-4f55-b59a-1342f5bd7c02",
        ),
        (
            "one hexadecimal digit short",
            "b847fa93-e282-4f55-b59a-1342f5bd7c0",
        ),
        (
            "a non-hexadecimal digit",
            "b847fa93-e282-4f55-b59a-1342f5bd7c0z",
        ),
    ];

    for (case, malformed) in cases {
        let (router, engine) = super::ec2_query::emulator();
        let queued = seed(
            &engine,
            purchase(
                RESERVED_INSTANCE_STATE_QUEUED,
                "t3.micro",
                Some("us-east-1a"),
                ONE_YEAR_SECONDS,
            ),
        );

        let (status, body) = delete_queued(&router, &[queued.as_str(), malformed], &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {:?} with body {body}",
            error_code(&body)
        );
        assert_eq!(
            stored_state(&engine, &queued).as_deref(),
            Some(RESERVED_INSTANCE_STATE_QUEUED),
            "case {case:?}: a refused call must delete nothing, got {:?}",
            stored_state(&engine, &queued)
        );
    }
}

/// The rule: `DryRun` asks whether the call would have succeeded — so it is answered
/// `DryRunOperation` (HTTP 412) with nothing deleted, while a value the service cannot read
/// is still refused with that reason instead.
#[tokio::test]
async fn dry_run_deletes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let queued = seed(
        &engine,
        purchase(
            RESERVED_INSTANCE_STATE_QUEUED,
            "t3.micro",
            Some("us-east-1a"),
            ONE_YEAR_SECONDS,
        ),
    );
    let active = seed(
        &engine,
        purchase(
            RESERVED_INSTANCE_STATE_ACTIVE,
            "m5.large",
            None,
            ONE_YEAR_SECONDS,
        ),
    );
    let dry_run = [param("DryRun", "true")];

    // Over a queued purchase, and over an id that names nothing: both are requests that
    // would have succeeded, because an unknown id is a per-id result rather than a refusal.
    for (case, ids) in [
        ("a queued purchase", vec![queued.clone()]),
        ("an id that names nothing", vec![unknown_id(7)]),
        (
            "a queued purchase, an unknown id and a Reserved Instance that is not queued",
            vec![queued.clone(), unknown_id(8), active.clone()],
        ),
    ] {
        let named: Vec<&str> = ids.iter().map(String::as_str).collect();
        let (status, body) = delete_queued(&router, &named, &dry_run).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412 for a DryRun that would have succeeded, got \
             {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {:?} with body {body}",
            error_code(&body)
        );
        assert_eq!(
            stored_state(&engine, &queued).as_deref(),
            Some(RESERVED_INSTANCE_STATE_QUEUED),
            "case {case:?}: a DryRun must delete nothing, got {:?}",
            stored_state(&engine, &queued)
        );
        assert_eq!(
            stored_state(&engine, &active).as_deref(),
            Some(RESERVED_INSTANCE_STATE_ACTIVE),
            "case {case:?}: a DryRun must move no Reserved Instance, got {:?}",
            stored_state(&engine, &active)
        );
    }

    // A DryRun over a value that is not an id is refused for THAT reason: the request would
    // not have succeeded.
    let (status, body) = delete_queued(&router, &["r-00000000000000000"], &dry_run).await;
    assert_eq!(
        status, 400,
        "a DryRun naming an unreadable id must be refused with that reason: got {status} \
         with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "a DryRun naming an unreadable id must answer InvalidParameterValue, got {:?}",
        error_code(&body)
    );

    // And the delete itself still works afterwards, so nothing above consumed it.
    let (status, body) = delete_queued(&router, &[queued.as_str()], &[]).await;
    assert_eq!(
        status, 200,
        "the delete after the dry runs must succeed: got {status} with body {body}"
    );
    assert_eq!(
        deletions("after the dry runs", &body).deleted,
        vec![queued.clone()],
        "the delete after the dry runs must report the purchase it deleted"
    );
}

/// The rule: the batch is bounded by the modelled `@length(min: 1, max: 100)` on
/// `DeleteQueuedReservedInstancesIdList` — a request naming exactly the maximum is answered
/// per id, and one naming more than the maximum, or none at all, is refused.
#[tokio::test]
async fn the_batch_size_is_bounded_by_the_modelled_length() {
    let (router, _engine) = super::ec2_query::emulator();

    // Exactly the published maximum: every id is answered, none is dropped.
    let at_the_bound: Vec<String> = (0..MAX_IDS_PER_REQUEST).map(unknown_id).collect();
    let named: Vec<&str> = at_the_bound.iter().map(String::as_str).collect();
    let (status, body) = delete_queued(&router, &named, &[]).await;
    assert_eq!(
        status, 200,
        "case \"{MAX_IDS_PER_REQUEST} ids\": expected HTTP 200, got {status} with body {body}"
    );
    let results = deletions("at the bound", &body);
    assert_eq!(
        results.failed.len(),
        MAX_IDS_PER_REQUEST,
        "case \"{MAX_IDS_PER_REQUEST} ids\": every id must be answered, got {} entries",
        results.failed.len()
    );

    // One past it, and none at all: both are outside the modelled length, and the boundary
    // names which rule each one broke — the list's own `@length` for a batch that is too
    // long, the `@required` member for a request that carries no list at all.
    for (case, count, expected) in [
        (
            "one id past the maximum",
            MAX_IDS_PER_REQUEST + 1,
            "ValidationError",
        ),
        ("no ids", 0, "MalformedInput"),
    ] {
        let ids: Vec<String> = (0..count).map(unknown_id).collect();
        let named: Vec<&str> = ids.iter().map(String::as_str).collect();
        let (status, body) = delete_queued(&router, &named, &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a batch outside the modelled length must be refused, got {status} \
             with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {:?} with body {body}",
            error_code(&body)
        );
    }
}

/// The rule: the Reserved Instances store is bounded by its own ceiling
/// ([`MAX_STORED_RESERVED_INSTANCES`]), so repeated purchases cannot grow the emulator's
/// memory without bound — and a record a delete moved to `queued-deleted` still occupies
/// that ceiling, because nothing reclaims it.
///
/// The bound is asserted AT the ceiling and one past it, through the same gate a purchase
/// path has to ask (`has_reserved_instances_capacity`) and through the insert itself.
#[test]
fn the_reserved_instances_store_is_bounded_by_its_ceiling() {
    let (_router, engine) = super::ec2_query::emulator();
    let store = engine.state::<Ec2State>();
    let mut store = store.lock();

    for n in 0..MAX_STORED_RESERVED_INSTANCES {
        assert!(
            store.has_reserved_instances_capacity(1),
            "case \"purchase {n} of {MAX_STORED_RESERVED_INSTANCES}\": the store must still \
             have room, it holds {}",
            store.reserved_instances.len()
        );
        let stored = store.insert_reserved_instances(purchase(
            RESERVED_INSTANCE_STATE_QUEUED,
            "t3.micro",
            Some("us-east-1a"),
            ONE_YEAR_SECONDS,
        ));
        assert!(
            stored.is_some(),
            "case \"purchase {n} of {MAX_STORED_RESERVED_INSTANCES}\": the store must accept a \
             purchase below its ceiling, it holds {}",
            store.reserved_instances.len()
        );
    }

    assert!(
        !store.has_reserved_instances_capacity(1),
        "case \"at the ceiling\": the store must report no room, it holds {} of \
         {MAX_STORED_RESERVED_INSTANCES}",
        store.reserved_instances.len()
    );
    let past_the_ceiling = store.insert_reserved_instances(purchase(
        RESERVED_INSTANCE_STATE_QUEUED,
        "m5.large",
        None,
        ONE_YEAR_SECONDS,
    ));
    assert!(
        past_the_ceiling.is_none(),
        "case \"one past the ceiling\": the store must refuse the purchase, it answered \
         {past_the_ceiling:?}"
    );
    assert_eq!(
        store.reserved_instances.len(),
        MAX_STORED_RESERVED_INSTANCES,
        "case \"one past the ceiling\": the store must hold exactly its ceiling, it holds {}",
        store.reserved_instances.len()
    );
}
