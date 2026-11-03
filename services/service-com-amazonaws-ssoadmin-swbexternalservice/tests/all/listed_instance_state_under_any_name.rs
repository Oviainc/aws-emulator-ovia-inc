// SPDX-License-Identifier: CC0-1.0

//! The `State` view an enumerated IAM Identity Center instance carries does not depend on
//! that instance's `Name`, over the `awsJson1_1` wire.
//!
//! THE RULE: the `State` structure an enumeration serves beside an instance's `Status` is
//! read from that instance's own lifecycle field, so EVERY instance the account holds
//! carries it with `State.Name` equal to its `Status` — whatever spelling the instance's
//! own `Name` member carries, and whether it carries one at all, since the name is no part
//! of an instance's identity and decides nothing about the view.
//!
//! `instance_state_view.rs` pins that the member is served and mirrors the status through
//! the lifecycle, and `listed_instance_state_source.rs` pins the RECORD it is read from;
//! this suite spans the `Name` VALUE SPACE, which is where a generated, per-run instance
//! name turns up. The rows are the published bounds and character classes of the member's
//! own target shape
//! (`smithy_query(query="shape",shape="com.amazonaws.ssoadmin#NameType")`: `string`,
//! `length 0..255`, `pattern ^[\w+=,.@-]+$`) together with the create that names no `Name`
//! at all, since the member is optional; each row is read back through `DescribeInstance`
//! as well, so the listed view and the single-instance read of one instance cannot report
//! two different lifecycles.
//!
//! Every call goes through `MultiServiceRouter::dispatch`; the harness is the one
//! `ssoadmin_permission_sets.rs` owns, and the enumeration and the rule assertion the ones
//! `instance_state_view.rs` owns. Where a phase lives on the far side of a due instant the
//! test moves the STORED instant rather than sleeping — the transition is still applied by
//! the service's own `materialize_due`, exactly as in production.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::instance_create::hold_activation;
use crate::instance_state_view::{assert_state_mirrors_status, listed};
use crate::ssoadmin_permission_sets::{call_in_region, error_code, router_over, with_store};

/// The Region every instance below is enabled in, and the Region its calls are signed for:
/// a `RegionName` (`^([a-z]+-){2,3}\d$`) none of the sibling suites uses.
const REGION: &str = "sa-east-1";

/// A router over an account that holds NO instance, so the `CreateInstance` each row makes
/// is the call that enables the account's one instance — plus the engine behind it, for the
/// stored due instant each phase has to be moved past.
fn empty_account() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    with_store(&engine, |store| {
        store.instances.clear();
        // The account's ambient instance has been enabled once and is gone, so no later
        // request enables one beside the instance the row creates.
        store.ambient_instance_enabled = true;
    });
    (router_over(Arc::clone(&engine)), engine)
}

/// The `Name` values the rows span, each admissible under `NameType` (`length 0..255`,
/// `pattern ^[\w+=,.@-]+$`): `None` is the create that names none.
fn name_rows() -> Vec<(&'static str, Option<String>)> {
    vec![
        // The create that leaves the optional member out entirely.
        ("no Name member", None),
        // The shortest value the pattern's `+` quantifier admits, whatever the 0 minimum
        // allows.
        ("a one-character Name", Some("a".to_string())),
        // The shape of an ad-hoc generated name: a prefix and a hex suffix a caller's own
        // set-up minted, so nothing about the view may be keyed on a name the service did
        // not choose.
        (
            "an ad-hoc generated Name",
            Some(format!("adhoc-idc-{:08x}", 0x5bcd_3a90_u32)),
        ),
        // Every character class the pattern admits: `\w` (letters, digits, `_`) and the
        // literal `+ = , . @ -`.
        (
            "a Name using every character class the pattern admits",
            Some("A_name+with=every,allowed.char@-1".to_string()),
        ),
        // The published upper bound of the member's length.
        ("a 255-character Name", Some("n".repeat(255))),
    ]
}

/// `CreateInstance` for one row, as the created instance's ARN.
async fn create(case: &str, router: &MultiServiceRouter, name: Option<&str>) -> String {
    let mut request = json!({});
    if let Some(name) = name {
        request["Name"] = json!(name);
    }
    let (status, body) = call_in_region(router, REGION, "CreateInstance", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateInstance expected 200, got {status} {body}"
    );
    body["InstanceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no InstanceArn in {body}"))
        .to_string()
}

/// `DescribeInstance` for `instance_arn`, as a 200 body — the single-instance read the
/// listed view is checked against.
async fn describe(case: &str, router: &MultiServiceRouter, instance_arn: &str) -> Value {
    let (status, body) = call_in_region(
        router,
        REGION,
        "DescribeInstance",
        &json!({"InstanceArn": instance_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInstance expected 200, got {status} {body}"
    );
    body
}

/// The one entry the account's enumeration serves, and the rule asserted on it: its
/// `State.Name` is its own `Status`, the status is the one the phase owes, the `Name` it
/// reports is the one the create set, and the single-instance read of the same ARN agrees
/// on the lifecycle.
async fn assert_entry(
    case: &str,
    router: &MultiServiceRouter,
    instance_arn: &str,
    expected_name: Option<&str>,
    expected_status: &str,
) {
    let entries = listed(case, router, REGION, None).await;
    assert_eq!(
        entries.len(),
        1,
        "case {case:?}: the account's one instance is enumerated, got {entries:?}"
    );
    let entry = &entries[0];
    assert_eq!(
        entry["InstanceArn"].as_str(),
        Some(instance_arn),
        "case {case:?}: expected the created instance, got {entry}"
    );
    assert_eq!(
        entry["Name"].as_str(),
        expected_name,
        "case {case:?}: expected Name {expected_name:?}, got {:?}",
        entry["Name"]
    );
    assert_state_mirrors_status(case, entry);
    assert_eq!(
        entry["Status"].as_str(),
        Some(expected_status),
        "case {case:?}: expected Status {expected_status:?}, got {entry}"
    );
    assert_eq!(
        entry["State"]["Name"].as_str(),
        Some(expected_status),
        "case {case:?}: expected State.Name {expected_status:?}, got {entry}"
    );

    let described = describe(case, router, instance_arn).await;
    assert_eq!(
        described["Status"].as_str(),
        entry["State"]["Name"].as_str(),
        "case {case:?}: the listed State.Name and the described Status report one \
         lifecycle, got {} and {}",
        described["Status"],
        entry["State"]["Name"]
    );
}

/// THE RULE over the `Name` value space: every row's instance is served with its `State`
/// view, in the phase whose creation is still in flight and in the phase the activation has
/// fallen due alike.
#[tokio::test]
async fn every_listed_instance_carries_its_state_whatever_its_name() {
    for (row, name) in name_rows() {
        let (router, engine) = empty_account();
        let instance_arn = create(row, &router, name.as_deref()).await;

        // PHASE 1 — the creation is still in flight.
        hold_activation(&engine, SystemTime::now() + Duration::from_secs(3_600));
        let case = format!("{row} / creation in flight");
        assert_entry(
            &case,
            &router,
            &instance_arn,
            name.as_deref(),
            "CREATE_IN_PROGRESS",
        )
        .await;

        // PHASE 2 — the activation the clock owes has fallen due.
        hold_activation(&engine, SystemTime::now() - Duration::from_secs(1));
        let case = format!("{row} / activation due");
        assert_entry(&case, &router, &instance_arn, name.as_deref(), "ACTIVE").await;
    }
}

/// The same rule where the requested `Name` lies OUTSIDE the member's published value
/// space — empty, and one character past its 255 maximum: the create is refused, so the
/// account holds no instance and the enumeration serves no entry to carry a view. The rule
/// is about the instances the service HOLDS, and a refused create adds none.
#[tokio::test]
async fn a_name_outside_the_published_space_enables_no_instance() {
    // `NameType` is `length 0..255` with `pattern ^[\w+=,.@-]+$`, whose `+` admits no
    // empty value: the row below each sits one step outside one of those bounds.
    let rows: Vec<(&str, String)> = vec![
        ("an empty Name", String::new()),
        ("a 256-character Name", "n".repeat(256)),
    ];

    for (case, name) in rows {
        let (router, _engine) = empty_account();
        let (status, body) =
            call_in_region(&router, REGION, "CreateInstance", &json!({"Name": name})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );

        let entries = listed(case, &router, REGION, None).await;
        assert!(
            entries.is_empty(),
            "case {case:?}: a refused create enables no instance, got {entries:?}"
        );
    }
}
