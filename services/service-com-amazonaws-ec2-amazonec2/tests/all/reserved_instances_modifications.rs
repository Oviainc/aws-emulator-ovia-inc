// SPDX-License-Identifier: CC0-1.0

//! What `ModifyReservedInstances` records, what the clock's fulfilment of it does to the
//! account's Reserved Instances, and the values it refuses — end to end over the wire the
//! service actually speaks: `ec2Query`, an `application/x-www-form-urlencoded` POST whose
//! body carries `Action=ModifyReservedInstances`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing name in the
//! request's SigV4 credential scope, so every call here carries one.
//!
//! The rules under test:
//!
//! * A request naming Reserved Instances the account holds, in effect, whose target
//!   configurations account for exactly the reservations those purchases hold, is RECORDED:
//!   it answers one `ReservedInstancesModificationId` and stores the request in
//!   `processing` — the first member of the status catalog the `status` filter documents
//!   (`smithy_query(query="doc",shape="DescribeReservedInstancesModificationsRequest",member="Filters")`:
//!   `processing | fulfilled | failed`) — with the configurations it was given and no new
//!   Reserved Instances id yet.
//! * The clock owes that request its `fulfilled`: one Reserved Instance per target
//!   configuration is minted then — "the Reserved Instances created as part of the
//!   modification request", whose id "is only available when the status of the modification
//!   is `fulfilled`" — carrying what the configuration asked for (instance type,
//!   Availability Zone and so scope, count) and keeping the term of the purchase it
//!   replaces, which is left `retired`. Both halves are visible to a LATER caller:
//!   modifying the same Reserved Instance again is refused because it is retired, and
//!   `DeleteQueuedReservedInstances` resolves the new ids.
//! * A `reservedInstancesIds` entry the account holds no Reserved Instance for, one whose
//!   Reserved Instance is in any state other than `active`, a batch whose purchases are not
//!   interchangeable ("identical, except for Availability Zone, network platform, and
//!   instance type"), and target configurations whose total instance count is not the one
//!   being modified, are all refused with HTTP 400 `InvalidInput` naming the member and what
//!   was wrong with it; a configuration this service cannot read at all is refused with the
//!   code EC2 answers for that parameter, and nothing is stored either way.
//! * A repeated `ClientToken` answers with the modification the first call recorded rather
//!   than recording a second, and the same token carrying a different request is refused as
//!   an idempotency mismatch.

use std::time::{Duration, SystemTime};

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, IDEMPOTENT_PARAMETER_MISMATCH, INVALID_INPUT,
    INVALID_PARAMETER_COMBINATION, INVALID_PARAMETER_VALUE, MAX_CLIENT_TOKEN_CHARS,
    MISSING_PARAMETER, RESERVED_INSTANCE_SCOPE_REGION, RESERVED_INSTANCE_SCOPE_ZONE,
    RESERVED_INSTANCE_STATE_ACTIVE, RESERVED_INSTANCE_STATE_RETIRED,
    RESERVED_INSTANCES_MODIFICATION_ID_PREFIX, RESERVED_INSTANCES_MODIFICATION_STATUS_FULFILLED,
    RESERVED_INSTANCES_MODIFICATION_STATUS_PROCESSING, ReservedInstancesSpec, TRANSITION_DELAY,
    availability_zones_in_region, reserved_instances_id_is_well_formed,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{error_code, error_message, indexed};
use super::wire::{Param, param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The form-field prefix ec2Query flattens `TargetConfigurations` into: the member's
/// `@xmlName` (`smithy_query(query="operation",operation="ModifyReservedInstances")` gives
/// `ReservedInstancesConfigurationSetItemType`), which is the parameter name EC2's own API
/// reference documents for this list.
const TARGETS: &str = "ReservedInstancesConfigurationSetItemType";

/// A one-year Reserved Instance term, in seconds — the units `ReservedInstances$Duration`
/// is reported in ("the duration of the Reserved Instance, in seconds").
const ONE_YEAR_SECONDS: i64 = 365 * 24 * 60 * 60;

/// Long enough past [`TRANSITION_DELAY`] that the fulfilment the clock owes a recorded
/// modification is certainly due.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// The error code the boundary validator answers BEFORE a handler runs, for a value
/// outside a modelled enum this service states no wording of its own for: the `scope`
/// catalog `ReservedInstancesConfiguration$Scope` names is therefore enforced by the model
/// itself, not by this operation.
const BOUNDARY_VALIDATION_ERROR: &str = "ValidationError";

/// The code EC2 answers for a value outside the `InstanceType` catalog, which
/// `$InstanceType` names: that refusal is the boundary's too, but the service states EC2's own
/// code and sentence for that parameter type rather than the validation clause.
const INSTANCE_TYPE_REFUSAL: &str = "InvalidParameterValue";

/// One Reserved Instance to seed: its instance type, its Availability Zone (`None` for a
/// Regional-scope purchase) and how many reservations it holds.
type PurchaseRow = (&'static str, Option<&'static str>, i32);

/// One target configuration a table row asks for: how many reservations it accounts for,
/// the instance type it moves them to (`None` keeps the purchase's own) and the
/// Availability Zone it moves them to (`None` for a configuration of Regional scope).
type TargetRow = (i32, Option<&'static str>, Option<&'static str>);

/// One row of the recording table: the case, the purchases to seed, how many times each is
/// named in the one request, and the target configurations to ask for.
type RecordCase = (
    &'static str,
    &'static [PurchaseRow],
    usize,
    &'static [TargetRow],
);

/// One row of the fulfilment table: the case, the purchases to seed and the target
/// configurations to ask for.
type FulfilCase = (&'static str, &'static [PurchaseRow], &'static [TargetRow]);

/// One row of the instance-count table: the case, the purchases to seed, the target
/// configurations to ask for, and the two totals the rule compares (what the
/// configurations ask for, and what the purchases hold).
type CountCase = (
    &'static str,
    &'static [PurchaseRow],
    &'static [TargetRow],
    i64,
    i64,
);

/// One row of the unreadable-configuration table: the case, the fields of the one target
/// configuration it sends, and the error code the refusal must carry.
type ConfigurationCase = (
    &'static str,
    &'static [(&'static str, &'static str)],
    &'static str,
);

/// The form fields for a whole `TargetConfigurations` list, as ec2Query flattens it:
/// `<prefix>.<N>.<Member>`, each member under its own `@ec2QueryName` (`InstanceCount`,
/// `InstanceType`, `AvailabilityZone`).
fn target_params(targets: &[TargetRow]) -> Vec<Param> {
    targets
        .iter()
        .enumerate()
        .flat_map(|(index, (instance_count, instance_type, zone))| {
            let position = index + 1;
            let mut fields = vec![param(
                &format!("{TARGETS}.{position}.InstanceCount"),
                &instance_count.to_string(),
            )];
            if let Some(instance_type) = instance_type {
                fields.push(param(
                    &format!("{TARGETS}.{position}.InstanceType"),
                    instance_type,
                ));
            }
            if let Some(zone) = zone {
                fields.push(param(
                    &format!("{TARGETS}.{position}.AvailabilityZone"),
                    zone,
                ));
            }
            fields
        })
        .collect()
}

/// One `ModifyReservedInstances` call naming `ids` and asking for `targets`, as its HTTP
/// status and XML body.
async fn modify(
    router: &MultiServiceRouter,
    ids: &[&str],
    targets: &[TargetRow],
    extra: &[Param],
) -> (u16, String) {
    let mut params = indexed("ReservedInstancesId", ids);
    params.extend(target_params(targets));
    params.extend_from_slice(extra);
    super::ec2_query::call(router, REGION, "ModifyReservedInstances", &params).await
}

/// The `reservedInstancesModificationId` a successful call reported, having first pinned
/// the status.
fn modification_id(case: &str, status: u16, body: &str) -> String {
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    super::ec2_query::member(body, "reservedInstancesModificationId").unwrap_or_else(|| {
        panic!("case {case:?}: no <reservedInstancesModificationId> in {body}")
    })
}

/// A Reserved Instance purchase to seed, in `state`.
///
/// The enum members are the modelled catalogs' own: `standard` of
/// `OFFERING_CLASS_TYPE_VALUES`, `All Upfront` of `OFFERING_TYPE_VALUES_VALUES`,
/// `Linux/UNIX` of `R_I_PRODUCT_DESCRIPTION_VALUES`, and an `InstanceType` of
/// `INSTANCE_TYPE_VALUES`.
fn purchase(state: &str, row: PurchaseRow) -> ReservedInstancesSpec {
    let (instance_type, availability_zone, instance_count) = row;
    ReservedInstancesSpec {
        instance_type: instance_type.to_string(),
        availability_zone: availability_zone.map(str::to_string),
        region: REGION.to_string(),
        instance_count,
        duration_seconds: ONE_YEAR_SECONDS,
        offering_class: "standard".to_string(),
        offering_type: "All Upfront".to_string(),
        product_description: "Linux/UNIX".to_string(),
        state: state.to_string(),
        start: SystemTime::now(),
        fixed_price: 500.0,
        usage_price: 0.0,
    }
}

/// Seed one Reserved Instance in `state` and answer the id the store minted for it.
fn seed(engine: &engine::InMemoryEngine, state: &str, row: PurchaseRow) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_reserved_instances(purchase(state, row))
        .expect("the store has room for the few purchases this suite seeds")
}

/// Seed one `active` Reserved Instance whose offering differs from [`purchase`]'s in the
/// one member `differing` names, and answer the id the store minted for it.
///
/// The members are the ones a modification may NOT change
/// (`ec2::state::reserved_instances_identity`), each taken from the modelled catalog it
/// belongs to: `OFFERING_CLASS_TYPE_VALUES`, `OFFERING_TYPE_VALUES_VALUES`,
/// `R_I_PRODUCT_DESCRIPTION_VALUES`, and the term in seconds.
fn seed_differing(engine: &engine::InMemoryEngine, differing: &str, row: PurchaseRow) -> String {
    let mut spec = purchase(RESERVED_INSTANCE_STATE_ACTIVE, row);
    match differing {
        "duration" => spec.duration_seconds = 3 * ONE_YEAR_SECONDS,
        "offering class" => spec.offering_class = "convertible".to_string(),
        "offering type" => spec.offering_type = "No Upfront".to_string(),
        "product description" => spec.product_description = "Windows".to_string(),
        other => panic!("the table names no purchase member {other:?}"),
    }
    engine
        .state::<Ec2State>()
        .lock()
        .insert_reserved_instances(spec)
        .expect("the store has room for the few purchases this suite seeds")
}

/// What one stored modification result carries: the target configuration as it was asked
/// for, and the Reserved Instances id the fulfilment minted for it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StoredResult {
    instance_count: Option<i32>,
    instance_type: Option<String>,
    availability_zone: Option<String>,
    reserved_instances_id: Option<String>,
}

/// What one stored modification carries, as the facts these tests assert.
#[derive(Debug, Clone)]
struct StoredModification {
    status: Option<String>,
    status_message: Option<String>,
    client_token: Option<String>,
    named: Vec<String>,
    results: Vec<StoredResult>,
    create_date: Option<i64>,
    effective_date: Option<i64>,
    update_date: Option<i64>,
}

/// The modification the store holds under `modification_id`, or a panic naming the case:
/// the operation answered an id, so the record it names has to be there.
fn stored(
    case: &str,
    engine: &engine::InMemoryEngine,
    modification_id: &str,
) -> StoredModification {
    let state = engine.state::<Ec2State>();
    let store = state.lock();
    let record = store
        .reserved_instances_modifications
        .get(modification_id)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds no modification {modification_id}"));
    StoredModification {
        status: record.view.status.clone(),
        status_message: record.view.status_message.clone(),
        client_token: record.view.client_token.clone(),
        named: record
            .view
            .reserved_instances_ids
            .iter()
            .flatten()
            .filter_map(|named| named.reserved_instances_id.clone())
            .collect(),
        results: record
            .view
            .modification_results
            .iter()
            .flatten()
            .map(|result| StoredResult {
                instance_count: result
                    .target_configuration
                    .as_ref()
                    .and_then(|target| target.instance_count),
                instance_type: result
                    .target_configuration
                    .as_ref()
                    .and_then(|target| target.instance_type.clone()),
                availability_zone: result
                    .target_configuration
                    .as_ref()
                    .and_then(|target| target.availability_zone.clone()),
                reserved_instances_id: result.reserved_instances_id.clone(),
            })
            .collect(),
        create_date: record.view.create_date.map(|at| at.secs()),
        effective_date: record.view.effective_date.map(|at| at.secs()),
        update_date: record.view.update_date.map(|at| at.secs()),
    }
}

/// How many modifications the store holds.
fn stored_count(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<Ec2State>()
        .lock()
        .reserved_instances_modifications
        .len()
}

/// What the store holds for one Reserved Instance: the facts a modification may change,
/// and the ones it must carry over from the purchase it replaces.
#[derive(Debug, Clone, PartialEq)]
struct StoredPurchase {
    state: Option<String>,
    instance_type: Option<String>,
    availability_zone: Option<String>,
    availability_zone_id: Option<String>,
    scope: Option<String>,
    instance_count: Option<i32>,
    duration: Option<i64>,
    offering_class: Option<String>,
    offering_type: Option<String>,
    product_description: Option<String>,
    end: Option<i64>,
}

/// The Reserved Instance `reserved_instances_id`, or `None` when the store holds none.
fn purchase_of(
    engine: &engine::InMemoryEngine,
    reserved_instances_id: &str,
) -> Option<StoredPurchase> {
    let state = engine.state::<Ec2State>();
    let store = state.lock();
    store
        .reserved_instances
        .get(reserved_instances_id)
        .map(|held| StoredPurchase {
            state: held.state.clone(),
            instance_type: held.instance_type.clone(),
            availability_zone: held.availability_zone.clone(),
            availability_zone_id: held.availability_zone_id.clone(),
            scope: held.scope.clone(),
            instance_count: held.instance_count,
            duration: held.duration,
            offering_class: held.offering_class.clone(),
            offering_type: held.offering_type.clone(),
            product_description: held.product_description.clone(),
            end: held.end.map(|at| at.secs()),
        })
}

/// The results one target row expects to be stored as, before a fulfilment has minted
/// anything for it.
fn expected_results(targets: &[TargetRow]) -> Vec<StoredResult> {
    targets
        .iter()
        .map(|(instance_count, instance_type, zone)| StoredResult {
            instance_count: Some(*instance_count),
            instance_type: instance_type.map(str::to_string),
            availability_zone: zone.map(str::to_string),
            reserved_instances_id: None,
        })
        .collect()
}

/// Whether `value` reads as a Reserved Instances modification id: the `rimod-` prefix and
/// then the hyphenated UUID form the store mints
/// (`ComAmazonawsEc2Amazonec2State::mint_reserved_instances_modification_id`).
///
/// The UUID half is read by the crate's own check, the one the Reserved Instances store
/// mints and resolves ids with, so this test and that mint cannot drift apart.
fn is_modification_id(value: &str) -> bool {
    value
        .strip_prefix(RESERVED_INSTANCES_MODIFICATION_ID_PREFIX)
        .and_then(|rest| rest.strip_prefix('-'))
        .is_some_and(reserved_instances_id_is_well_formed)
}

// ---------------------------------------------------------------------------
// What a modification records
// ---------------------------------------------------------------------------

/// The rule: a request the service can act on records a modification of the Reserved
/// Instances it named, `processing`, carrying the target configurations it was given and no
/// new Reserved Instances id yet, and answers that modification's id.
///
/// The rows differ along what the rule generalises over: how many purchases one call
/// modifies and how many reservations each holds, how many configurations they are moved
/// onto (one, split, merged), what each configuration changes (zone, instance type, scope),
/// and whether the request names a purchase more than once.
#[tokio::test]
async fn a_modification_is_recorded_processing_for_the_purchases_it_names() {
    // (case, the purchases to seed, how many times each is named, the target configurations)
    let cases: &[RecordCase] = &[
        (
            "one zonal purchase moved to another zone",
            &[("t3.micro", Some("us-east-1a"), 2)],
            1,
            &[(2, None, Some("us-east-1b"))],
        ),
        (
            "a Regional purchase split across two zones",
            &[("m5.large", None, 4)],
            1,
            &[(1, None, Some("us-east-1a")), (3, None, Some("us-east-1c"))],
        ),
        (
            "two zonal purchases merged into one Regional configuration",
            &[
                ("t3.micro", Some("us-east-1a"), 1),
                ("t3.micro", Some("us-east-1b"), 2),
            ],
            1,
            &[(3, None, None)],
        ),
        (
            "an instance type change inside one zone",
            &[("c5.xlarge", Some("us-east-1a"), 2)],
            1,
            &[(2, Some("c5.2xlarge"), Some("us-east-1a"))],
        ),
        (
            "a request that names the same purchase three times",
            &[("r5.2xlarge", Some("us-east-1c"), 3)],
            3,
            &[(3, Some("r5.xlarge"), Some("us-east-1a"))],
        ),
    ];

    for (case, purchases, repeats, targets) in cases {
        let (router, engine) = super::ec2_query::emulator();
        let seeded: Vec<String> = purchases
            .iter()
            .map(|row| seed(&engine, RESERVED_INSTANCE_STATE_ACTIVE, *row))
            .collect();
        let named: Vec<&str> = seeded
            .iter()
            .flat_map(|id| std::iter::repeat_n(id.as_str(), *repeats))
            .collect();

        let (status, body) = modify(&router, &named, targets, &[]).await;
        let id = modification_id(case, status, &body);
        assert!(
            is_modification_id(&id),
            "case {case:?}: the answered id must read as '{RESERVED_INSTANCES_MODIFICATION_ID_PREFIX}-<uuid>', \
             got {id:?}"
        );

        let recorded = stored(case, &engine, &id);
        assert_eq!(
            recorded.status.as_deref(),
            Some(RESERVED_INSTANCES_MODIFICATION_STATUS_PROCESSING),
            "case {case:?}: a recorded modification starts \
             '{RESERVED_INSTANCES_MODIFICATION_STATUS_PROCESSING}', got {:?}",
            recorded.status
        );
        assert_eq!(
            recorded.status_message, None,
            "case {case:?}: a modification that is still being worked on reports no reason, \
             got {:?}",
            recorded.status_message
        );
        assert_eq!(
            recorded.named, seeded,
            "case {case:?}: the modification must name each purchase once, in request order, \
             got {:?} for {seeded:?}",
            recorded.named
        );
        assert_eq!(
            recorded.results,
            expected_results(targets),
            "case {case:?}: the modification must carry the target configurations it was \
             given and no new Reserved Instances id yet, got {:?}",
            recorded.results
        );
        assert!(
            recorded.create_date.is_some()
                && recorded.create_date == recorded.effective_date
                && recorded.create_date == recorded.update_date,
            "case {case:?}: a modification of purchases already in effect is created, \
             effective and updated at the one instant, got {:?} / {:?} / {:?}",
            recorded.create_date,
            recorded.effective_date,
            recorded.update_date
        );

        // The purchases are NOT changed by recording the request: the caller that acts
        // straight after the call still sees them in effect, with the counts it modified.
        for (id, row) in seeded.iter().zip(*purchases) {
            let held = purchase_of(&engine, id)
                .unwrap_or_else(|| panic!("case {case:?}: the store must still hold {id}"));
            assert_eq!(
                (held.state.as_deref(), held.instance_count),
                (Some(RESERVED_INSTANCE_STATE_ACTIVE), Some(row.2)),
                "case {case:?}: {id} must still be in effect with its own count until the \
                 modification is fulfilled, got {held:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// What the clock's fulfilment does
// ---------------------------------------------------------------------------

/// The rule: the clock owes a recorded modification its fulfilment — one Reserved Instance
/// per target configuration, carrying what that configuration asked for and the term of the
/// purchase it replaces, which is left `retired` — and both halves are visible to a later
/// caller through operations that read the Reserved Instances store.
///
/// The rows differ in what the configurations change (the zone, the instance type, the
/// scope) and in whether the purchases are split or merged.
#[tokio::test]
async fn the_clock_fulfils_a_modification_by_replacing_the_purchases_it_named() {
    // (case, the purchases to seed, the target configurations)
    let cases: &[FulfilCase] = &[
        (
            "a zonal purchase moved to another zone",
            &[("t3.micro", Some("us-east-1a"), 2)],
            &[(2, None, Some("us-east-1b"))],
        ),
        (
            "a Regional purchase split across two zones and retyped",
            &[("m5.large", None, 4)],
            &[
                (1, Some("m5.xlarge"), Some("us-east-1a")),
                (3, None, Some("us-east-1c")),
            ],
        ),
        (
            "two zonal purchases merged into one Regional configuration",
            &[
                ("t3.micro", Some("us-east-1a"), 1),
                ("t3.micro", Some("us-east-1b"), 2),
            ],
            &[(3, None, None)],
        ),
    ];

    for (case, purchases, targets) in cases {
        let (router, engine) = super::ec2_query::emulator();
        let seeded: Vec<String> = purchases
            .iter()
            .map(|row| seed(&engine, RESERVED_INSTANCE_STATE_ACTIVE, *row))
            .collect();
        let named: Vec<&str> = seeded.iter().map(String::as_str).collect();
        let before: Vec<StoredPurchase> = seeded
            .iter()
            .map(|id| {
                purchase_of(&engine, id)
                    .unwrap_or_else(|| panic!("case {case:?}: the store must hold {id}"))
            })
            .collect();

        let (status, body) = modify(&router, &named, targets, &[]).await;
        let id = modification_id(case, status, &body);

        tokio::time::sleep(past_the_transition()).await;

        // The same purchases again: they are retired, which is a refusal only the
        // fulfilment can have produced — the clock's work is visible on the wire through
        // this operation alone.
        let (status, body) = modify(&router, &named, targets, &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a purchase a fulfilled modification retired cannot be modified \
             again, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_INPUT,
            "case {case:?}: expected the code {INVALID_INPUT}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!(
                "Invalid value for 'reservedInstancesIds'. Reserved Instance {} is in the \
                 '{RESERVED_INSTANCE_STATE_RETIRED}' state.",
                seeded[0]
            ),
            "case {case:?}: the refusal must name the member, the Reserved Instance and the \
             state the store holds it in, got {body}"
        );

        let fulfilled = stored(case, &engine, &id);
        assert_eq!(
            fulfilled.status.as_deref(),
            Some(RESERVED_INSTANCES_MODIFICATION_STATUS_FULFILLED),
            "case {case:?}: the modification must be \
             '{RESERVED_INSTANCES_MODIFICATION_STATUS_FULFILLED}' once its instant has \
             passed, got {:?}",
            fulfilled.status
        );
        assert_eq!(
            fulfilled.results.len(),
            targets.len(),
            "case {case:?}: one result per target configuration, got {:?}",
            fulfilled.results
        );

        // Each result now names a Reserved Instance the store really holds, carrying what
        // its configuration asked for and the term of the purchase it replaced.
        let zones = availability_zones_in_region(REGION);
        let mut minted: Vec<String> = Vec::new();
        for (result, (instance_count, instance_type, zone)) in
            fulfilled.results.iter().zip(*targets)
        {
            let new_id = result.reserved_instances_id.clone().unwrap_or_else(|| {
                panic!(
                    "case {case:?}: a fulfilled result must name the Reserved Instance it \
                     created, got {result:?}"
                )
            });
            let held = purchase_of(&engine, &new_id).unwrap_or_else(|| {
                panic!("case {case:?}: the store must hold the created {new_id}")
            });
            let expected_type = instance_type
                .map(str::to_string)
                .or_else(|| before[0].instance_type.clone());
            let expected_scope = if zone.is_some() {
                RESERVED_INSTANCE_SCOPE_ZONE
            } else {
                RESERVED_INSTANCE_SCOPE_REGION
            };
            let expected_zone_id = zone.and_then(|zone| {
                zones
                    .iter()
                    .find(|known| known.zone_name == zone)
                    .map(|known| known.zone_id.clone())
            });
            assert_eq!(
                (
                    held.state.as_deref(),
                    held.instance_count,
                    held.instance_type.clone(),
                    held.availability_zone.clone(),
                    held.availability_zone_id.clone(),
                    held.scope.as_deref(),
                ),
                (
                    Some(RESERVED_INSTANCE_STATE_ACTIVE),
                    Some(*instance_count),
                    expected_type,
                    zone.map(str::to_string),
                    expected_zone_id,
                    Some(expected_scope),
                ),
                "case {case:?}: the created {new_id} must be in effect and carry what its \
                 target configuration asked for, got {held:?}"
            );
            assert_eq!(
                (
                    held.duration,
                    held.offering_class.clone(),
                    held.offering_type.clone(),
                    held.product_description.clone(),
                    held.end,
                ),
                (
                    before[0].duration,
                    before[0].offering_class.clone(),
                    before[0].offering_type.clone(),
                    before[0].product_description.clone(),
                    before[0].end,
                ),
                "case {case:?}: the created {new_id} must keep the term of the purchase it \
                 replaces, got {held:?} for {:?}",
                before[0]
            );
            minted.push(new_id);
        }

        // The purchases the request named are retired rather than dropped, so they are
        // still reportable.
        for id in &seeded {
            let held = purchase_of(&engine, id)
                .unwrap_or_else(|| panic!("case {case:?}: the store must still hold {id}"));
            assert_eq!(
                held.state.as_deref(),
                Some(RESERVED_INSTANCE_STATE_RETIRED),
                "case {case:?}: {id} must be left \
                 '{RESERVED_INSTANCE_STATE_RETIRED}', got {held:?}"
            );
        }

        // A second operation resolves the created ids: `DeleteQueuedReservedInstances`
        // reports a Reserved Instance that exists but holds no queued purchase as
        // 'not in queued state', and one it cannot find at all as 'id invalid'.
        let created: Vec<&str> = minted.iter().map(String::as_str).collect();
        let (status, body) = super::ec2_query::call(
            &router,
            REGION,
            "DeleteQueuedReservedInstances",
            &indexed("ReservedInstancesId", &created),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteQueuedReservedInstances, got \
             {status} with body {body}"
        );
        for new_id in &minted {
            assert!(
                body.contains(new_id),
                "case {case:?}: DeleteQueuedReservedInstances must resolve the created \
                 {new_id}, got {body}"
            );
        }
        assert!(
            !body.contains(ec2::ops::delete_queued_reserved_instances::RESERVED_INSTANCES_ID_INVALID),
            "case {case:?}: no created Reserved Instance may be reported as unknown, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// The Reserved Instances a modification may name
// ---------------------------------------------------------------------------

/// The rule: a `reservedInstancesIds` entry the account holds no Reserved Instance for is
/// refused with HTTP 400 `InvalidInput` naming the member and that id, and nothing is
/// recorded.
///
/// The rows differ in the form of the id and in where it sits in the batch: a value the
/// store simply does not hold, whatever form it takes, is one this operation cannot act on.
#[tokio::test]
async fn an_id_the_account_holds_no_reserved_instance_for_is_refused() {
    // (case, the id the refusal must name, whether a held purchase is named beside it)
    let cases: &[(&str, &str, bool)] = &[
        (
            "a well-formed id that names nothing",
            "1a2b3c4d-1234-5678-9abc-1234567890ab",
            false,
        ),
        (
            "the same id in upper case",
            "B847FA93-E282-4F55-B59A-1342F5BD7C02",
            false,
        ),
        (
            "a value that is not a UUID at all",
            "ri-0123456789abcdef0",
            false,
        ),
        ("an empty id", "", false),
        (
            "an unknown id beside one the account holds",
            "00000000-0000-4000-8000-000000000001",
            true,
        ),
    ];

    for (case, unknown, beside_a_held_one) in cases {
        let (router, engine) = super::ec2_query::emulator();
        let mut named: Vec<String> = Vec::new();
        let mut targets: Vec<TargetRow> = vec![(2, None, Some("us-east-1a"))];
        if *beside_a_held_one {
            named.push(seed(
                &engine,
                RESERVED_INSTANCE_STATE_ACTIVE,
                ("t3.micro", Some("us-east-1a"), 2),
            ));
            targets = vec![(2, None, Some("us-east-1b"))];
        }
        named.push((*unknown).to_string());
        let ids: Vec<&str> = named.iter().map(String::as_str).collect();

        let (status, body) = modify(&router, &ids, &targets, &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_INPUT,
            "case {case:?}: expected the code {INVALID_INPUT}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!(
                "Invalid value for 'reservedInstancesIds'. Reserved Instance {unknown} is not \
                 found."
            ),
            "case {case:?}: the refusal must name the member and the id, got {body}"
        );
        assert_eq!(
            stored_count(&engine),
            0,
            "case {case:?}: a refused request records nothing, the store holds {} \
             modifications",
            stored_count(&engine)
        );
    }
}

/// The rule: only a Reserved Instance that is in effect can be modified; one in any other
/// state is refused with HTTP 400 `InvalidInput` naming the member, the id and the state the
/// store holds it in.
///
/// The table is the modelled `ReservedInstanceState` catalog itself
/// (`ec2::ops::types::RESERVED_INSTANCE_STATE_VALUES`), so it covers every state a Reserved
/// Instance can be in rather than the ones an example happened to name — `active` being the
/// one row that is served.
#[tokio::test]
async fn a_reserved_instance_that_is_not_in_effect_cannot_be_modified() {
    for state in ec2::ops::types::RESERVED_INSTANCE_STATE_VALUES {
        let case = format!("a purchase in the '{state}' state");
        let (router, engine) = super::ec2_query::emulator();
        let id = seed(&engine, state, ("t3.micro", Some("us-east-1a"), 2));
        let targets: &[TargetRow] = &[(2, None, Some("us-east-1b"))];

        let (status, body) = modify(&router, &[&id], targets, &[]).await;
        if *state == RESERVED_INSTANCE_STATE_ACTIVE {
            let recorded = modification_id(&case, status, &body);
            assert!(
                is_modification_id(&recorded),
                "case {case:?}: the one state a modification is served from must answer an \
                 id, got {recorded:?}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_INPUT,
            "case {case:?}: expected the code {INVALID_INPUT}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!(
                "Invalid value for 'reservedInstancesIds'. Reserved Instance {id} is in the \
                 '{state}' state."
            ),
            "case {case:?}: the refusal must name the member, the id and the held state, \
             got {body}"
        );
        assert_eq!(
            stored_count(&engine),
            0,
            "case {case:?}: a refused request records nothing, the store holds {} \
             modifications",
            stored_count(&engine)
        );
    }
}

// ---------------------------------------------------------------------------
// The target configurations a modification may ask for
// ---------------------------------------------------------------------------

/// The rule: the Reserved Instances of one request must be interchangeable — "identical,
/// except for Availability Zone, network platform, and instance type"
/// (`ModifyReservedInstances`) — so a batch whose purchases differ in any other member of
/// their offering is refused with HTTP 400 `InvalidInput` in those words, and nothing is
/// recorded.
///
/// The rows are the members that identity is read from
/// (`ec2::state::reserved_instances_identity`), one row each, with a batch that differs only
/// in the three members the rule excepts as the served control.
#[tokio::test]
async fn the_reserved_instances_of_one_request_must_be_interchangeable() {
    // (case, the member the second purchase differs in — `None` for the control)
    let cases: &[(&str, Option<&str>)] = &[
        ("a purchase of another term", Some("duration")),
        ("a purchase of another offering class", Some("offering class")),
        ("a purchase of another offering type", Some("offering type")),
        (
            "a purchase of another product description",
            Some("product description"),
        ),
        ("purchases differing only in zone and type", None),
    ];

    for (case, differing) in cases {
        let (router, engine) = super::ec2_query::emulator();
        let first = seed(
            &engine,
            RESERVED_INSTANCE_STATE_ACTIVE,
            ("t3.micro", Some("us-east-1a"), 2),
        );
        // The second purchase differs in the member the row names, or — for the control —
        // only in the Availability Zone and the instance type the rule excepts.
        let second = match differing {
            Some(member) => seed_differing(&engine, member, ("t3.micro", Some("us-east-1b"), 2)),
            None => seed(
                &engine,
                RESERVED_INSTANCE_STATE_ACTIVE,
                ("m5.large", Some("us-east-1b"), 2),
            ),
        };
        let targets: &[TargetRow] = &[(4, None, Some("us-east-1c"))];

        let (status, body) = modify(&router, &[&first, &second], targets, &[]).await;
        if differing.is_none() {
            let recorded = modification_id(case, status, &body);
            assert!(
                is_modification_id(&recorded),
                "case {case:?}: purchases differing only in the excepted members must be \
                 served, got {recorded:?}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_INPUT,
            "case {case:?}: expected the code {INVALID_INPUT}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            "Invalid value for 'reservedInstancesIds'. The Reserved Instances to be modified \
             must be identical, except for Availability Zone, network platform, and instance \
             type.",
            "case {case:?}: the refusal must name the member and the rule, got {body}"
        );
        assert_eq!(
            stored_count(&engine),
            0,
            "case {case:?}: a refused request records nothing, the store holds {} \
             modifications",
            stored_count(&engine)
        );
    }
}

/// The rule: the target configurations together account for exactly the reservations the
/// Reserved Instances being modified hold; any other total is refused with HTTP 400
/// `InvalidInput` naming the member and both counts.
///
/// The rows differ in how the totals miss each other (too few, too many, none at all) and
/// in how many purchases and configurations the two totals are spread over, with the
/// matching row served as the control.
#[tokio::test]
async fn the_target_configurations_must_account_for_every_reservation() {
    // (case, the purchases to seed, the target configurations, the two totals)
    let cases: &[CountCase] = &[
        (
            "one configuration short of the count held",
            &[("t3.micro", Some("us-east-1a"), 3)],
            &[(2, None, Some("us-east-1b"))],
            2,
            3,
        ),
        (
            "one configuration over the count held",
            &[("t3.micro", Some("us-east-1a"), 1)],
            &[(4, None, Some("us-east-1b"))],
            4,
            1,
        ),
        (
            "two configurations whose counts do not add up to two purchases",
            &[
                ("m5.large", Some("us-east-1a"), 2),
                ("m5.large", Some("us-east-1b"), 2),
            ],
            &[(1, None, Some("us-east-1a")), (1, None, Some("us-east-1c"))],
            2,
            4,
        ),
        (
            "three configurations summing past the counts held",
            &[("c5.xlarge", None, 5)],
            &[
                (2, None, Some("us-east-1a")),
                (2, None, Some("us-east-1b")),
                (2, None, Some("us-east-1c")),
            ],
            6,
            5,
        ),
        (
            "the counts that do add up",
            &[
                ("m5.large", Some("us-east-1a"), 2),
                ("m5.large", Some("us-east-1b"), 2),
            ],
            &[(1, None, Some("us-east-1a")), (3, None, Some("us-east-1c"))],
            4,
            4,
        ),
    ];

    for (case, purchases, targets, asked, held) in cases {
        let (router, engine) = super::ec2_query::emulator();
        let seeded: Vec<String> = purchases
            .iter()
            .map(|row| seed(&engine, RESERVED_INSTANCE_STATE_ACTIVE, *row))
            .collect();
        let ids: Vec<&str> = seeded.iter().map(String::as_str).collect();

        let (status, body) = modify(&router, &ids, targets, &[]).await;
        if asked == held {
            let recorded = modification_id(case, status, &body);
            assert!(
                is_modification_id(&recorded),
                "case {case:?}: matching totals must be served, got {recorded:?}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_INPUT,
            "case {case:?}: expected the code {INVALID_INPUT}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!(
                "Invalid value for 'targetConfigurations'. The total instance count of the \
                 target configurations ({asked}) must equal the instance count of the \
                 Reserved Instances being modified ({held})."
            ),
            "case {case:?}: the refusal must name the member and both counts, got {body}"
        );
        assert_eq!(
            stored_count(&engine),
            0,
            "case {case:?}: a refused request records nothing, the store holds {} \
             modifications",
            stored_count(&engine)
        );
    }
}

/// The rule: a target configuration this service cannot read is refused with the code EC2
/// answers for that parameter, before anything is looked up or stored.
///
/// The rows are the ways one configuration can be unreadable: the count it is "a required
/// field" of is missing or not a count at all, the Availability Zone it names is not one of
/// the Region's, and the scope it declares contradicts whether it named a zone.
#[tokio::test]
async fn a_target_configuration_this_service_cannot_read_is_refused() {
    // (case, the fields of the one target configuration, the code the refusal carries)
    let cases: &[ConfigurationCase] = &[
        (
            "no instance count",
            &[("AvailabilityZone", "us-east-1b")],
            MISSING_PARAMETER,
        ),
        (
            "an instance count of zero",
            &[("InstanceCount", "0"), ("AvailabilityZone", "us-east-1b")],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "a negative instance count",
            &[("InstanceCount", "-2"), ("AvailabilityZone", "us-east-1b")],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an Availability Zone of another Region",
            &[("InstanceCount", "2"), ("AvailabilityZone", "eu-west-1a")],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an Availability Zone name no Region has",
            &[("InstanceCount", "2"), ("AvailabilityZone", "us-east-1z")],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an Availability Zone id this Region does not have",
            &[("InstanceCount", "2"), ("AvailabilityZoneId", "use1-az9")],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "Regional scope beside an Availability Zone",
            &[
                ("InstanceCount", "2"),
                ("AvailabilityZone", "us-east-1b"),
                ("Scope", RESERVED_INSTANCE_SCOPE_REGION),
            ],
            INVALID_PARAMETER_COMBINATION,
        ),
        (
            "zonal scope naming no Availability Zone",
            &[
                ("InstanceCount", "2"),
                ("Scope", RESERVED_INSTANCE_SCOPE_ZONE),
            ],
            INVALID_PARAMETER_COMBINATION,
        ),
        (
            "a scope outside the modelled catalog",
            &[
                ("InstanceCount", "2"),
                ("AvailabilityZone", "us-east-1b"),
                ("Scope", "Zone"),
            ],
            BOUNDARY_VALIDATION_ERROR,
        ),
        (
            "an instance type outside the modelled catalog",
            &[
                ("InstanceCount", "2"),
                ("AvailabilityZone", "us-east-1b"),
                ("InstanceType", "t3.enormous"),
            ],
            INSTANCE_TYPE_REFUSAL,
        ),
    ];

    for (case, fields, code) in cases {
        let (router, engine) = super::ec2_query::emulator();
        let id = seed(
            &engine,
            RESERVED_INSTANCE_STATE_ACTIVE,
            ("t3.micro", Some("us-east-1a"), 2),
        );
        let mut params = indexed("ReservedInstancesId", &[&id]);
        for (name, value) in *fields {
            params.push(param(&format!("{TARGETS}.1.{name}"), value));
        }
        let (status, body) =
            super::ec2_query::call(&router, REGION, "ModifyReservedInstances", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            *code,
            "case {case:?}: expected the code {code}, got {body}"
        );
        assert_eq!(
            stored_count(&engine),
            0,
            "case {case:?}: a refused request records nothing, the store holds {} \
             modifications",
            stored_count(&engine)
        );
    }
}

// ---------------------------------------------------------------------------
// Idempotency
// ---------------------------------------------------------------------------

/// The rule: a `ClientToken` makes the request idempotent — a repeat of the same request
/// answers with the modification the first call recorded and records no second one — and the
/// same token carrying a different request is refused as an idempotency mismatch.
#[tokio::test]
async fn a_repeated_client_token_answers_with_the_modification_it_already_recorded() {
    let (router, engine) = super::ec2_query::emulator();
    let first_id = seed(
        &engine,
        RESERVED_INSTANCE_STATE_ACTIVE,
        ("t3.micro", Some("us-east-1a"), 2),
    );
    let second_id = seed(
        &engine,
        RESERVED_INSTANCE_STATE_ACTIVE,
        ("t3.micro", Some("us-east-1b"), 2),
    );
    let token = "the-token-of-this-modification";
    let targets: &[TargetRow] = &[(2, None, Some("us-east-1c"))];

    let case = "the first call with a token";
    let (status, body) = modify(&router, &[&first_id], targets, &[param("ClientToken", token)]).await;
    let recorded = modification_id(case, status, &body);
    assert_eq!(
        stored(case, &engine, &recorded).client_token.as_deref(),
        Some(token),
        "case {case:?}: the stored modification must carry the token that minted it"
    );

    let case = "the same request again with the same token";
    let (status, body) = modify(&router, &[&first_id], targets, &[param("ClientToken", token)]).await;
    let repeated = modification_id(case, status, &body);
    assert_eq!(
        repeated, recorded,
        "case {case:?}: a repeat must answer with the modification already recorded, got \
         {repeated} for {recorded}"
    );
    assert_eq!(
        stored_count(&engine),
        1,
        "case {case:?}: a repeat records nothing, the store holds {} modifications",
        stored_count(&engine)
    );

    // (case, the ids the second request names, its target configurations)
    let mismatches: &[(&str, &[&str], &[TargetRow])] = &[
        (
            "the same token naming another Reserved Instance",
            &[&second_id],
            &[(2, None, Some("us-east-1c"))],
        ),
        (
            "the same token asking for another configuration",
            &[&first_id],
            &[(2, None, Some("us-east-1a"))],
        ),
        (
            "the same token splitting the count differently",
            &[&first_id],
            &[(1, None, Some("us-east-1c")), (1, None, Some("us-east-1a"))],
        ),
    ];
    for (case, ids, targets) in mismatches {
        let (status, body) = modify(&router, ids, targets, &[param("ClientToken", token)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            IDEMPOTENT_PARAMETER_MISMATCH,
            "case {case:?}: expected the code {IDEMPOTENT_PARAMETER_MISMATCH}, got {body}"
        );
        assert_eq!(
            stored_count(&engine),
            1,
            "case {case:?}: a refused repeat records nothing, the store holds {} \
             modifications",
            stored_count(&engine)
        );
    }

    // A token longer than this crate holds any caller text to is refused before anything
    // is looked up.
    let case = "a token past the character ceiling";
    let long_token = "t".repeat(MAX_CLIENT_TOKEN_CHARS + 1);
    let (status, body) = modify(
        &router,
        &[&first_id],
        targets,
        &[param("ClientToken", &long_token)],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        INVALID_PARAMETER_VALUE,
        "case {case:?}: expected the code {INVALID_PARAMETER_VALUE}, got {body}"
    );
    assert_eq!(
        stored_count(&engine),
        1,
        "case {case:?}: a refused request records nothing, the store holds {} modifications",
        stored_count(&engine)
    );
}
