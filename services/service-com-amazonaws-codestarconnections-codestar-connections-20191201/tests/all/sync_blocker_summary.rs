// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetSyncBlockerSummary`.
//!
//! Every call goes over the wire: a request is dispatched through the
//! [`MultiServiceRouter`] exactly as the emulator's front door does it
//! (`POST /` + `X-Amz-Target: CodeStar_connections_20191201.<Operation>`, awsJson1_0
//! body), so what is asserted here is the service's HTTP behaviour and not a handler
//! called directly.
//!
//! A sync blocker is `AUTOMATED` — the service's Git-sync component raises it and no
//! modeled operation creates one — so the blockers these tests report are put in the
//! store directly, through the SAME records `UpdateSyncBlocker` reads and resolves
//! (`codestar::state::StoredSyncBlocker`). That is what makes these assertions evidence
//! that the response is computed from persisted state rather than from a constant.
//!
//! The tables span what the rule generalises over: how many blockers a resource holds
//! (none, one, several), their statuses, whether they carry contexts or a parent
//! resource, the instants they were raised at (including several within one clock tick),
//! and the published `ResourceName` bounds — the recorded request (an unsynced resource
//! with no blockers at all) is ONE row.

use std::time::{Duration, UNIX_EPOCH};

use aws_smithy_types::date_time::{DateTime, Format};
use serde_json::{json, Value};

use service_com_amazonaws_codestarconnections_codestar_connections_20191201 as codestar;

use codestar::ops::types::SyncBlockerContext;
use codestar::state::{
    ComAmazonawsCodestarconnectionsCodestarConnections20191201State as CodestarState,
    StoredSyncBlocker, BLOCKER_STATUS_ACTIVE, BLOCKER_STATUS_RESOLVED, BLOCKER_TYPE_AUTOMATED,
};

// The router harness this crate's suites share (one emulator, one engine, awsJson
// dispatch under a chosen target), rather than a fourth copy of it.
use super::connection_lifecycle::{Emulator, TARGET_PREFIX};

/// The service's FULLY QUALIFIED Coral name as a target prefix: the form an AWS SDK
/// client of this service sends, which carries dots of its own.
const SHAPE_ID_TARGET_PREFIX: &str =
    "com.amazonaws.codestar.connections.CodeStar_connections_20191201";

/// Both prefix forms a request may carry.
const TARGET_PREFIXES: &[&str] = &[TARGET_PREFIX, SHAPE_ID_TARGET_PREFIX];

/// Every value the model's `SyncConfigurationType` enum accepts.
///
/// Referenced rather than copied: this is the generated catalog
/// (`src/ops/types/shared_types.rs::SYNC_CONFIGURATION_TYPE_VALUES`, from
/// `smithy_query(query="shape", shape="SyncConfigurationType")`), so the tables below
/// widen with the model instead of drifting from it.
const SYNC_TYPES: &[&str] = codestar::ops::types::shared_types::SYNC_CONFIGURATION_TYPE_VALUES;

/// The `SyncType` the single-type tables address.
const SYNC_TYPE: &str = SYNC_TYPES[0];

/// The `BlockerStatus` values the model accepts (generated catalog, from
/// `smithy_query(query="shape", shape="BlockerStatus")`): `["ACTIVE", "RESOLVED"]`.
const BLOCKER_STATUSES: &[&str] = codestar::ops::types::shared_types::BLOCKER_STATUS_VALUES;

/// `GetSyncBlockerSummary` under one sync type, asserting 200, and return the
/// `SyncBlockerSummary`.
async fn get_summary(
    emulator: &Emulator,
    case: &str,
    sync_type: &str,
    resource_name: &str,
) -> Value {
    let (status, response) = emulator
        .call(
            "GetSyncBlockerSummary",
            json!({ "SyncType": sync_type, "ResourceName": resource_name }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetSyncBlockerSummary to answer 200, got {status} {response:?}"
    );
    response["SyncBlockerSummary"].clone()
}

/// Put one sync blocker in the store — what the Git-sync side does, and what this
/// reader must report.
fn seed_blocker(emulator: &Emulator, blocker: StoredSyncBlocker) {
    emulator
        .engine()
        .state::<CodestarState>()
        .lock()
        .sync_blockers
        .insert(blocker.id.clone(), blocker);
}

/// The blocker state holds under `id`, straight out of the store.
fn stored_blocker(emulator: &Emulator, id: &str) -> Option<StoredSyncBlocker> {
    emulator
        .engine()
        .state::<CodestarState>()
        .lock()
        .sync_blockers
        .get(id)
        .cloned()
}

/// How many sync blockers the store holds in total.
fn stored_blocker_count(emulator: &Emulator) -> usize {
    emulator
        .engine()
        .state::<CodestarState>()
        .lock()
        .sync_blockers
        .len()
}

/// How a seeded blocker is described in a table row.
struct Seed {
    id: String,
    resource: String,
    parent: Option<String>,
    sync_type: &'static str,
    status: &'static str,
    /// Seconds since the epoch the blocker was raised at.
    created_secs: u64,
    contexts: &'static [(&'static str, &'static str)],
}

impl Seed {
    /// This row as the record the store holds.
    ///
    /// A `RESOLVED` blocker carries the reason and the instant it was resolved at — the
    /// only state in which the modeled `ResolvedReason` / `ResolvedAt` are set.
    fn to_stored(&self) -> StoredSyncBlocker {
        let created_at = UNIX_EPOCH + Duration::from_secs(self.created_secs);
        let resolved = self.status == BLOCKER_STATUS_RESOLVED;
        StoredSyncBlocker {
            id: self.id.clone(),
            resource_name: self.resource.clone(),
            parent_resource_name: self.parent.clone(),
            sync_type: self.sync_type.to_string(),
            blocker_type: BLOCKER_TYPE_AUTOMATED.to_string(),
            status: self.status.to_string(),
            created_reason: format!("the sync attempt for {} failed", self.id),
            created_at,
            contexts: self
                .contexts
                .iter()
                .map(|(key, value)| SyncBlockerContext {
                    key: (*key).to_string(),
                    value: (*value).to_string(),
                })
                .collect(),
            resolved_reason: resolved.then(|| format!("{} was resolved by hand", self.id)),
            resolved_at: resolved.then(|| created_at + Duration::from_secs(60)),
        }
    }
}

/// A row of the summary table: what state holds for a resource, and what the summary of
/// that resource must then report.
struct Case {
    name: &'static str,
    resource: String,
    sync_type: &'static str,
    parent: Option<&'static str>,
    seeds: Vec<Seed>,
    /// The blocker ids the summary must report, most recent first.
    expected_ids: Vec<&'static str>,
}

/// Shorthand for one seeded blocker of `resource`.
fn seed(
    id: &str,
    resource: &str,
    parent: Option<&str>,
    status: &'static str,
    created_secs: u64,
    contexts: &'static [(&'static str, &'static str)],
) -> Seed {
    Seed {
        id: id.to_string(),
        resource: resource.to_string(),
        parent: parent.map(str::to_string),
        sync_type: SYNC_TYPE,
        status,
        created_secs,
        contexts,
    }
}

/// The epoch seconds a wire timestamp carries, whichever of the two forms awsJson may
/// render it in (a number for `epoch-seconds`, a date-time string otherwise).
fn epoch_seconds(case: &str, value: &Value) -> f64 {
    if let Some(seconds) = value.as_f64() {
        return seconds;
    }
    let text = value
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: timestamp {value:?} is neither number nor text"));
    DateTime::from_str(text, Format::DateTime)
        .unwrap_or_else(|error| panic!("case {case:?}: timestamp {text:?} does not parse: {error}"))
        .as_secs_f64()
}

/// Assert the wire `SyncBlocker` reports exactly the stored record it was built from.
fn assert_reports_stored_blocker(case: &str, reported: &Value, stored: &StoredSyncBlocker) {
    assert_eq!(
        reported["Id"].as_str(),
        Some(stored.id.as_str()),
        "case {case:?}: wrong Id in {reported:?}"
    );
    assert_eq!(
        reported["Type"].as_str(),
        Some(stored.blocker_type.as_str()),
        "case {case:?}: wrong Type in {reported:?}"
    );
    assert_eq!(
        reported["Status"].as_str(),
        Some(stored.status.as_str()),
        "case {case:?}: wrong Status in {reported:?}"
    );
    assert_eq!(
        reported["CreatedReason"].as_str(),
        Some(stored.created_reason.as_str()),
        "case {case:?}: wrong CreatedReason in {reported:?}"
    );
    let stored_created = stored
        .created_at
        .duration_since(UNIX_EPOCH)
        .expect("the seeded instant is after the epoch")
        .as_secs_f64();
    let reported_created = epoch_seconds(case, &reported["CreatedAt"]);
    assert_eq!(
        reported_created, stored_created,
        "case {case:?}: expected CreatedAt {stored_created}, got {reported_created} in {reported:?}"
    );

    // `Contexts` is absent when the blocker carries none, and otherwise reports every
    // stored pair in stored order.
    if stored.contexts.is_empty() {
        assert!(
            reported.get("Contexts").is_none_or(Value::is_null),
            "case {case:?}: a blocker with no stored contexts reported some in {reported:?}"
        );
    } else {
        let expected: Vec<Value> = stored
            .contexts
            .iter()
            .map(|context| json!({ "Key": context.key, "Value": context.value }))
            .collect();
        assert_eq!(
            reported["Contexts"],
            Value::Array(expected.clone()),
            "case {case:?}: expected Contexts {expected:?}, got {:?}",
            reported["Contexts"]
        );
    }

    // `ResolvedReason` / `ResolvedAt` are set only once the blocker is resolved.
    match (&stored.resolved_reason, stored.resolved_at) {
        (Some(reason), Some(at)) => {
            assert_eq!(
                reported["ResolvedReason"].as_str(),
                Some(reason.as_str()),
                "case {case:?}: wrong ResolvedReason in {reported:?}"
            );
            let expected = at
                .duration_since(UNIX_EPOCH)
                .expect("the seeded instant is after the epoch")
                .as_secs_f64();
            let actual = epoch_seconds(case, &reported["ResolvedAt"]);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected ResolvedAt {expected}, got {actual} in {reported:?}"
            );
        }
        _ => {
            assert!(
                reported.get("ResolvedReason").is_none_or(Value::is_null),
                "case {case:?}: an unresolved blocker reported a ResolvedReason in {reported:?}"
            );
            assert!(
                reported.get("ResolvedAt").is_none_or(Value::is_null),
                "case {case:?}: an unresolved blocker reported a ResolvedAt in {reported:?}"
            );
        }
    }
}

/// The `Id`s of the reported `LatestBlockers`, in report order.
fn reported_ids(case: &str, summary: &Value) -> Vec<String> {
    summary["LatestBlockers"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: LatestBlockers is not a list in {summary:?}"))
        .iter()
        .map(|blocker| {
            blocker["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a blocker has no Id in {summary:?}"))
                .to_string()
        })
        .collect()
}

/// RULE: `GetSyncBlockerSummary` reports the sync blockers persisted for the requested
/// `(SyncType, ResourceName)` pair — each rebuilt from its stored record, most recent
/// first — together with that resource's stored parent, and an empty `LatestBlockers`
/// list when the pair holds no blockers.
#[tokio::test]
async fn the_summary_reports_the_blockers_persisted_for_the_pair() {
    let emulator = Emulator::new();

    let cases = vec![
        // The recorded case: a resource this account never synced holds no blockers.
        Case {
            name: "nothing recorded for the resource",
            resource: "UnsyncedStack".to_string(),
            sync_type: SYNC_TYPE,
            parent: None,
            seeds: vec![],
            expected_ids: vec![],
        },
        Case {
            name: "one active blocker",
            resource: "SingleBlockerStack".to_string(),
            sync_type: SYNC_TYPE,
            parent: None,
            seeds: vec![seed(
                "single-one",
                "SingleBlockerStack",
                None,
                BLOCKER_STATUS_ACTIVE,
                1_700_000_000,
                &[],
            )],
            expected_ids: vec!["single-one"],
        },
        Case {
            name: "three blockers reported newest first, whatever order they were recorded in",
            resource: "ThreeBlockerStack".to_string(),
            sync_type: SYNC_TYPE,
            parent: None,
            seeds: vec![
                seed(
                    "three-middle",
                    "ThreeBlockerStack",
                    None,
                    BLOCKER_STATUS_RESOLVED,
                    1_700_000_200,
                    &[],
                ),
                seed(
                    "three-oldest",
                    "ThreeBlockerStack",
                    None,
                    BLOCKER_STATUS_RESOLVED,
                    1_700_000_100,
                    &[],
                ),
                seed(
                    "three-newest",
                    "ThreeBlockerStack",
                    None,
                    BLOCKER_STATUS_ACTIVE,
                    1_700_000_300,
                    &[],
                ),
            ],
            expected_ids: vec!["three-newest", "three-middle", "three-oldest"],
        },
        Case {
            // Raised within one clock tick: the id breaks the tie, so the order is total
            // and the same on every read.
            name: "blockers raised at the same instant are ordered by id",
            resource: "SameSecondStack".to_string(),
            sync_type: SYNC_TYPE,
            parent: None,
            seeds: vec![
                seed(
                    "same-b",
                    "SameSecondStack",
                    None,
                    BLOCKER_STATUS_RESOLVED,
                    1_700_000_000,
                    &[],
                ),
                seed(
                    "same-c",
                    "SameSecondStack",
                    None,
                    BLOCKER_STATUS_ACTIVE,
                    1_700_000_000,
                    &[],
                ),
                seed(
                    "same-a",
                    "SameSecondStack",
                    None,
                    BLOCKER_STATUS_RESOLVED,
                    1_700_000_000,
                    &[],
                ),
            ],
            expected_ids: vec!["same-a", "same-b", "same-c"],
        },
        Case {
            name: "contexts and a parent resource are reported from the records",
            resource: "NestedStack".to_string(),
            sync_type: SYNC_TYPE,
            parent: Some("ParentStack"),
            seeds: vec![
                seed(
                    "nested-with-contexts",
                    "NestedStack",
                    Some("ParentStack"),
                    BLOCKER_STATUS_ACTIVE,
                    1_700_000_500,
                    &[("Branch", "main"), ("ConfigFile", "stack.yaml")],
                ),
                seed(
                    "nested-resolved",
                    "NestedStack",
                    Some("ParentStack"),
                    BLOCKER_STATUS_RESOLVED,
                    1_700_000_400,
                    &[("Branch", "main")],
                ),
            ],
            expected_ids: vec!["nested-with-contexts", "nested-resolved"],
        },
        Case {
            // Records of one resource normally agree on its parent; where they do not,
            // the most recently raised record is the one reported.
            name: "records disagreeing on the parent report the most recently raised one",
            resource: "ReparentedStack".to_string(),
            sync_type: SYNC_TYPE,
            parent: Some("NewParentStack"),
            seeds: vec![
                seed(
                    "reparented-old",
                    "ReparentedStack",
                    Some("OldParentStack"),
                    BLOCKER_STATUS_RESOLVED,
                    1_700_000_410,
                    &[],
                ),
                seed(
                    "reparented-new",
                    "ReparentedStack",
                    Some("NewParentStack"),
                    BLOCKER_STATUS_ACTIVE,
                    1_700_000_420,
                    &[],
                ),
            ],
            expected_ids: vec!["reparented-new", "reparented-old"],
        },
        // `ResourceName` is modeled `len 1..100` with pattern
        // `^[0-9A-Za-z]+[0-9A-Za-z_\-]*$`: both published bounds, and the other
        // characters the pattern allows, address a resource just as well.
        Case {
            name: "resource name at the modeled minimum length",
            resource: "a".to_string(),
            sync_type: SYNC_TYPE,
            parent: None,
            seeds: vec![seed(
                "min-name",
                "a",
                None,
                BLOCKER_STATUS_ACTIVE,
                1_700_000_600,
                &[],
            )],
            expected_ids: vec!["min-name"],
        },
        Case {
            name: "resource name at the modeled maximum length",
            resource: "L".repeat(100),
            sync_type: SYNC_TYPE,
            parent: None,
            seeds: vec![Seed {
                id: "max-name".to_string(),
                resource: "L".repeat(100),
                parent: None,
                sync_type: SYNC_TYPE,
                status: BLOCKER_STATUS_RESOLVED,
                created_secs: 1_700_000_700,
                contexts: &[],
            }],
            expected_ids: vec!["max-name"],
        },
        Case {
            name: "resource name using the digits, underscores and hyphens the pattern allows",
            resource: "0Stack_name-9".to_string(),
            sync_type: SYNC_TYPE,
            parent: None,
            seeds: vec![
                seed(
                    "punctuated-b",
                    "0Stack_name-9",
                    None,
                    BLOCKER_STATUS_ACTIVE,
                    1_700_000_800,
                    &[],
                ),
                seed(
                    "punctuated-a",
                    "0Stack_name-9",
                    None,
                    BLOCKER_STATUS_ACTIVE,
                    1_700_000_900,
                    &[],
                ),
            ],
            expected_ids: vec!["punctuated-a", "punctuated-b"],
        },
    ];

    let mut stored: Vec<StoredSyncBlocker> = Vec::new();
    for case in &cases {
        for row in &case.seeds {
            let record = row.to_stored();
            seed_blocker(&emulator, record.clone());
            stored.push(record);
        }
    }

    for case in &cases {
        let name = case.name;
        let summary = get_summary(&emulator, name, case.sync_type, &case.resource).await;

        assert_eq!(
            summary["ResourceName"].as_str(),
            Some(case.resource.as_str()),
            "case {name:?}: the summary must name the resource it was asked about, got {summary:?}"
        );
        assert_eq!(
            summary["ParentResourceName"].as_str(),
            case.parent,
            "case {name:?}: expected ParentResourceName {:?}, got {summary:?}",
            case.parent
        );

        let ids = reported_ids(name, &summary);
        assert_eq!(
            ids, case.expected_ids,
            "case {name:?}: expected blockers {:?}, got {ids:?}",
            case.expected_ids
        );

        // Every reported blocker is the stored record, field for field.
        for reported in summary["LatestBlockers"]
            .as_array()
            .expect("LatestBlockers is a list")
        {
            let id = reported["Id"].as_str().expect("a blocker reports its Id");
            let record = stored
                .iter()
                .find(|candidate| candidate.id == id)
                .unwrap_or_else(|| panic!("case {name:?}: {id:?} is not a stored blocker"));
            assert_reports_stored_blocker(name, reported, record);
        }
    }
}

/// RULE: a blocker belongs to the `(SyncType, ResourceName)` pair it was raised for, so
/// the summary of a pair reports only that pair's blockers — never another resource's,
/// and never one raised under another sync type.
#[tokio::test]
async fn the_summary_reports_only_the_blockers_of_the_pair_it_addresses() {
    let emulator = Emulator::new();

    let resources = ["AlphaStack", "BetaStack", "GammaStack"];
    for (index, resource) in resources.iter().enumerate() {
        // Resource n holds n+1 blockers, each id naming its resource, and each raised at
        // its own instant so the report order is the one the rule states.
        for position in 0..=index {
            seed_blocker(
                &emulator,
                seed(
                    &format!("{resource}-{position}"),
                    resource,
                    None,
                    BLOCKER_STATUSES[position % BLOCKER_STATUSES.len()],
                    1_700_000_000 + position as u64,
                    &[],
                )
                .to_stored(),
            );
        }
    }

    for (index, resource) in resources.iter().enumerate() {
        let summary = get_summary(&emulator, resource, SYNC_TYPE, resource).await;
        let ids = reported_ids(resource, &summary);
        assert_eq!(
            ids.len(),
            index + 1,
            "case {resource:?}: expected {} blockers, got {ids:?}",
            index + 1
        );
        assert!(
            ids.iter().all(|id| id.starts_with(*resource)),
            "case {resource:?}: another resource's blockers leaked in: {ids:?}"
        );
    }

    // A resource nothing was recorded for still answers 200 with an empty list, even
    // though the store is not empty.
    let summary = get_summary(&emulator, "no blockers", SYNC_TYPE, "DeltaStack").await;
    assert_eq!(
        summary["LatestBlockers"],
        json!([]),
        "a resource with no recorded blockers must report an empty list, got {summary:?}"
    );

    // Every sync type the model declares addresses its OWN blockers: the pair, not the
    // resource name alone, is the address. One blocker per declared sync type is seeded
    // against one shared resource name, and each type's summary must report only its
    // own. A single-valued enum is one row of this table today and widens with the
    // model.
    let shared_resource = "TypedStack";
    for sync_type in SYNC_TYPES {
        let mut row = seed(
            &format!("typed-{sync_type}"),
            shared_resource,
            None,
            BLOCKER_STATUS_ACTIVE,
            1_700_001_500,
            &[],
        );
        row.sync_type = sync_type;
        seed_blocker(&emulator, row.to_stored());
    }
    for sync_type in SYNC_TYPES {
        let summary = get_summary(&emulator, sync_type, sync_type, shared_resource).await;
        let ids = reported_ids(sync_type, &summary);
        let expected = vec![format!("typed-{sync_type}")];
        assert_eq!(
            ids, expected,
            "case {sync_type:?}: expected only this sync type's blockers {expected:?}, got {ids:?}"
        );
    }
}

/// RULE: the summary is a reporting operation, so reading it changes neither the stored
/// blockers nor what a later reader sees.
#[tokio::test]
async fn reading_the_summary_does_not_change_what_a_later_reader_sees() {
    let emulator = Emulator::new();

    let resource = "StableStack";
    for row in [
        seed(
            "stable-active",
            resource,
            Some("StableParent"),
            BLOCKER_STATUS_ACTIVE,
            1_700_001_000,
            &[("Branch", "main")],
        ),
        seed(
            "stable-resolved",
            resource,
            Some("StableParent"),
            BLOCKER_STATUS_RESOLVED,
            1_700_000_999,
            &[],
        ),
    ] {
        seed_blocker(&emulator, row.to_stored());
    }

    let describe = |id: &str| {
        let record =
            stored_blocker(&emulator, id).unwrap_or_else(|| panic!("{id:?} is in state"));
        (
            record.status,
            record.resolved_reason,
            record.resolved_at,
            record.parent_resource_name,
        )
    };
    let before = [describe("stable-active"), describe("stable-resolved")];

    let first = get_summary(&emulator, "first read", SYNC_TYPE, resource).await;
    let second = get_summary(&emulator, "second read", SYNC_TYPE, resource).await;
    assert_eq!(
        first, second,
        "two reads of one resource must agree: {first:?} then {second:?}"
    );

    let after = [describe("stable-active"), describe("stable-resolved")];
    assert_eq!(
        after, before,
        "reading a summary must not change the stored blockers: {before:?} became {after:?}"
    );

    // And a resource with no blockers is not conjured into state by reading it.
    let unknown = "NeverSyncedStack";
    let summary = get_summary(&emulator, "unknown resource", SYNC_TYPE, unknown).await;
    assert_eq!(
        summary["LatestBlockers"],
        json!([]),
        "case \"unknown resource\": expected an empty list, got {summary:?}"
    );
    let blocker_count = stored_blocker_count(&emulator);
    assert_eq!(
        blocker_count, 2,
        "reading summaries must not add or remove stored blockers, got {blocker_count}"
    );
}

/// RULE: a request outside a modeled constraint is refused as a bad request —
/// `ResourceName` is `len 1..100` with pattern `^[0-9A-Za-z]+[0-9A-Za-z_\-]*$`, so each
/// published bound holds and one past it does not.
#[tokio::test]
async fn a_resource_name_outside_the_modeled_constraints_is_refused() {
    let emulator = Emulator::new();

    // (case, ResourceName, must be accepted)
    let cases: Vec<(String, String, bool)> = vec![
        ("len 0 (below the minimum)".to_string(), String::new(), false),
        ("len 1 (the minimum)".to_string(), "n".to_string(), true),
        ("len 100 (the maximum)".to_string(), "n".repeat(100), true),
        (
            "len 101 (past the maximum)".to_string(),
            "n".repeat(101),
            false,
        ),
        (
            "a leading underscore the pattern forbids".to_string(),
            "_Stack".to_string(),
            false,
        ),
        (
            "a character outside the pattern".to_string(),
            "Stack/one".to_string(),
            false,
        ),
    ];

    for (case, resource_name, accepted) in &cases {
        let (status, response) = emulator
            .call(
                "GetSyncBlockerSummary",
                json!({ "SyncType": SYNC_TYPE, "ResourceName": resource_name }),
            )
            .await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: a name inside the modeled constraints was refused: {status} {response:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: a name outside the modeled constraints was accepted: {status} {response:?}"
            );
        }
    }

    // A `SyncType` the model does not declare is refused too, rather than reported as a
    // pair with no blockers.
    for sync_type in ["", "CFN_STACK", "cfn_stack_sync"] {
        let (status, response) = emulator
            .call(
                "GetSyncBlockerSummary",
                json!({ "SyncType": sync_type, "ResourceName": "SomeStack" }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {sync_type:?}: an unmodeled SyncType was accepted: {status} {response:?}"
        );
    }
}

/// RULE: an awsJson request names its operation in the segment after the LAST dot of
/// `x-amz-target`, whatever the target prefix in front of it — so the operation answers
/// from its own handler under the short prefix AND under the fully qualified Coral prefix
/// an AWS SDK client of this service sends, never the framework's
/// "operation <routing key> is not implemented in this emulator" (which means the target
/// named no operation at all).
#[tokio::test]
async fn the_operation_is_reachable_under_either_target_prefix() {
    let emulator = Emulator::new();

    let resource = "RoutedStack";
    seed_blocker(
        &emulator,
        seed(
            "routed-one",
            resource,
            None,
            BLOCKER_STATUS_ACTIVE,
            1_700_002_000,
            &[],
        )
        .to_stored(),
    );

    for prefix in TARGET_PREFIXES {
        let target = format!("{prefix}.GetSyncBlockerSummary");
        let (status, response) = emulator
            .call_target(
                &target,
                json!({ "SyncType": SYNC_TYPE, "ResourceName": resource }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {target:?}: the target must reach the operation, got {status} {response:?}"
        );
        let ids = reported_ids(&target, &response["SyncBlockerSummary"]);
        assert_eq!(
            ids,
            vec!["routed-one"],
            "case {target:?}: expected the stored blocker, got {ids:?}"
        );
    }

    // A target whose last segment names no modeled operation is still refused, so the
    // rule resolves operations rather than accepting anything after the last dot.
    for target in [
        format!("{TARGET_PREFIX}.NoSuchOperation"),
        format!("{SHAPE_ID_TARGET_PREFIX}.NoSuchOperation"),
    ] {
        let (status, response) = emulator.call_target(&target, json!({})).await;
        assert!(
            status >= 400,
            "case {target:?}: a target naming no modeled operation must be refused, got {status} {response:?}"
        );
    }
}
