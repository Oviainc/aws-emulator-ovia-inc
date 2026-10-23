// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListPolicyVersions`: the versions a catalog policy holds.
//!
//! Every request goes over the wire the real client uses: a `restJson1` HTTP request
//! (`POST /policies/{PolicyArn}/?List`, `PolicyArn` bound to a path segment and
//! `MaxResults` / `NextToken` to the query string) dispatched through the
//! `MultiServiceRouter` the harness in `approval_team_lifecycle.rs` mounts, so the route,
//! the URI bindings, the JSON envelope and the HTTP status are all exercised.
//!
//! The rule under test: `ListPolicyVersions` reports the versions the store holds for the
//! policy its `PolicyArn` names, in ascending version-id order, in pages of at most
//! `MaxResults`, carrying a `NextToken` exactly while more remain; each entry's members
//! are computed from the stored version and the policy that owns it, and a `PolicyArn`
//! the catalog does not hold is `ResourceNotFoundException` (404).
//!
//! So the tables below span VERSION COUNT (one version, several, the page bound, and past
//! it), the position of the DEFAULT version (first, middle, last), contiguous and
//! NON-CONTIGUOUS version ids, both `PolicyType` tokens, both `PolicyStatus` tokens and
//! several policy-name forms, crossed with PAGE SIZE (each published bound of
//! `MaxResults`, the sizes between, and the modelled default) — not one recorded policy.
//! Each entry's members are checked against the record they came from and against the
//! model's own catalogs (`PolicyType`'s and `PolicyStatus`'s enum values, `PolicyName`'s
//! `length(max: 64)`, `PolicyVersionId`'s `range(min: 1)`, `QualifiedPolicyArn`'s
//! pattern).

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use serde_json::Value;
use service_com_amazonaws_mpa_awsfluffycoreservice::ops::types::shared_types as model;
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_team_lifecycle::{encode_label, error_code, instant, text, Harness};

/// `MaxResults` is modelled `range(min: 1, max: 20)` with `default: 20`.
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 20;

/// `PolicyName` is `length(max: 64)`.
pub(crate) const MAX_POLICY_NAME: usize = 64;

/// `QualifiedPolicyArn` is `length(max: 1224)`.
const MAX_POLICY_ARN: usize = 1224;

/// Largest difference, in seconds, between a reported timestamp and the instant the store
/// holds: the wire renders `IsoTimestamp` as a `date-time` string, so a reported instant
/// is the stored one to well inside a millisecond.
pub(crate) const INSTANT_TOLERANCE: f64 = 0.001;

/// A fixed instant the recorded tables date their versions from, so no assertion below
/// depends on when the test ran.
pub(crate) fn base_instant() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

/// `POST /policies/{PolicyArn}/?List`, with `query` appended to the `?List` marker.
async fn list_policy_versions(harness: &Harness, policy_arn: &str, query: &str) -> (u16, Value) {
    let path = format!("/policies/{}/?List", encode_label(policy_arn));
    let uri = if query.is_empty() {
        path
    } else {
        format!("{path}&{query}")
    };
    harness.call("POST", &uri, None).await
}

/// One row of the version table.
struct VersionCase {
    case: &'static str,
    name: String,
    policy_type: state::PolicyType,
    /// `(version id, status)`, in no particular order — the handler, not this table, is
    /// what must order them.
    versions: Vec<(i32, state::PolicyStatus)>,
    default_version: i32,
}

/// Alternate the two `PolicyStatus` tokens the model declares, so no row below tests a
/// single value of it. An older version of a policy is the one AWS deprecates, so the
/// deprecated ones here are the lower ids.
pub(crate) fn status_for(version_id: i32, newest: i32) -> state::PolicyStatus {
    if version_id < newest {
        state::PolicyStatus::Deprecated
    } else {
        state::PolicyStatus::Attachable
    }
}

/// `count` contiguous versions, `1..=count`.
fn contiguous(count: i32) -> Vec<(i32, state::PolicyStatus)> {
    (1..=count).map(|id| (id, status_for(id, count))).collect()
}

/// The version table: rows differ along version count, default-version position, version
/// id spacing, `PolicyType`, `PolicyStatus` and the form of the policy name.
fn version_cases() -> Vec<VersionCase> {
    vec![
        VersionCase {
            case: "one version, which is the default",
            name: "SingleVersionPolicy".to_string(),
            policy_type: state::PolicyType::AwsManaged,
            versions: contiguous(1),
            default_version: 1,
        },
        VersionCase {
            case: "two versions, the newer is the default",
            name: "Two.Version_Policy-1".to_string(),
            policy_type: state::PolicyType::AwsRam,
            versions: contiguous(2),
            default_version: 2,
        },
        VersionCase {
            case: "three versions, a middle default",
            name: "MiddleDefaultPolicy".to_string(),
            policy_type: state::PolicyType::AwsManaged,
            versions: contiguous(3),
            default_version: 2,
        },
        VersionCase {
            case: "non-contiguous version ids, declared out of order",
            name: "SparseVersionPolicy".to_string(),
            policy_type: state::PolicyType::AwsRam,
            versions: vec![
                (8, state::PolicyStatus::Attachable),
                (1, state::PolicyStatus::Deprecated),
                (5, state::PolicyStatus::Attachable),
                (2, state::PolicyStatus::Deprecated),
            ],
            default_version: 5,
        },
        VersionCase {
            case: "seven versions, the first is the default",
            name: "SevenVersionPolicy".to_string(),
            policy_type: state::PolicyType::AwsManaged,
            versions: contiguous(7),
            default_version: 1,
        },
        VersionCase {
            // `PolicyVersions` is `length(max: 20)` and `MaxResults` tops out at 20: a
            // policy with exactly one page of versions.
            case: "one full page of versions, maximum-length name",
            name: "N".repeat(MAX_POLICY_NAME),
            policy_type: state::PolicyType::AwsRam,
            versions: contiguous(20),
            default_version: 20,
        },
        VersionCase {
            case: "one more than a page of versions",
            name: "JustPastOnePagePolicy".to_string(),
            policy_type: state::PolicyType::AwsManaged,
            versions: contiguous(21),
            default_version: 11,
        },
        VersionCase {
            case: "many pages of versions",
            name: "ManyVersionPolicy".to_string(),
            policy_type: state::PolicyType::AwsRam,
            versions: contiguous(45),
            default_version: 45,
        },
    ]
}

/// Record a catalog policy holding `versions`, with `default_version` as its default, and
/// answer its ARN.
///
/// No modelled operation mints a policy or one of its versions — the catalog is AWS's —
/// so a table that needs a version history writes it directly, the way
/// `record_management_account` records an organization. Each version is dated from
/// [`base_instant`], a minute apart, and last updated half a minute after it was created,
/// so every reported timestamp is a distinct value read out of the record.
fn record_policy(harness: &Harness, case: &VersionCase) -> String {
    let mut policy = state::CatalogPolicy {
        arn: state::policy_arn("IdentityCenter", &case.name),
        name: case.name.clone(),
        policy_type: case.policy_type,
        default_version: case.default_version,
        versions: BTreeMap::new(),
    };
    for &(version_id, status) in &case.versions {
        let created = base_instant() + Duration::from_secs(u64::from(version_id.unsigned_abs()) * 60);
        policy.insert_version(state::CatalogPolicyVersion {
            version_id,
            status,
            creation_time: created,
            last_updated_time: created + Duration::from_secs(30),
        });
    }
    let arn = policy.arn.clone();
    let store = state::service_state(&*harness.engine).expect("the service store");
    store.lock().insert_policy(policy);
    arn
}

/// The catalog record the store holds for `arn`.
pub(crate) fn stored_policy(harness: &Harness, arn: &str) -> state::CatalogPolicy {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let guard = store.lock();
    guard.policy(arn).expect("a stored policy").clone()
}

/// Every version id the store holds for `arn`, in ascending order.
fn stored_version_ids(harness: &Harness, arn: &str) -> Vec<i32> {
    let mut ids: Vec<i32> = stored_policy(harness, arn)
        .versions
        .keys()
        .copied()
        .collect();
    ids.sort_unstable();
    ids
}

/// Every member of one listed version, against the records it came from and the model's
/// constraints.
fn assert_entry(
    case: &str,
    listed: &Value,
    policy: &state::CatalogPolicy,
    version: &state::CatalogPolicyVersion,
) {
    let expected_arn = format!("{}/{}", policy.arn, version.version_id);
    assert_eq!(
        text(listed, "Arn"),
        expected_arn,
        "case {case:?}: expected Arn {expected_arn:?}, got {listed:?}"
    );
    assert_eq!(
        text(listed, "PolicyArn"),
        policy.arn,
        "case {case:?}: expected PolicyArn {:?}, got {listed:?}",
        policy.arn
    );
    assert_eq!(
        listed.get("VersionId").and_then(Value::as_i64),
        Some(i64::from(version.version_id)),
        "case {case:?}: expected VersionId {}, got {listed:?}",
        version.version_id
    );
    assert_eq!(
        text(listed, "PolicyType"),
        policy.policy_type.as_wire(),
        "case {case:?}: expected PolicyType {:?}, got {listed:?}",
        policy.policy_type.as_wire()
    );
    assert_eq!(
        text(listed, "Name"),
        policy.name,
        "case {case:?}: expected Name {:?}, got {listed:?}",
        policy.name
    );
    assert_eq!(
        text(listed, "Status"),
        version.status.as_wire(),
        "case {case:?}: expected Status {:?}, got {listed:?}",
        version.status.as_wire()
    );
    assert_eq!(
        listed.get("IsDefault").and_then(Value::as_bool),
        Some(version.version_id == policy.default_version),
        "case {case:?}: IsDefault must hold exactly for the policy's DefaultVersion \
         {}, got {listed:?}",
        policy.default_version
    );
    assert_instant(case, listed, "CreationTime", version.creation_time);
    assert_instant(case, listed, "LastUpdatedTime", version.last_updated_time);

    // The model's own catalogs and bounds, not this test's opinion.
    let policy_type = text(listed, "PolicyType");
    assert!(
        model::POLICY_TYPE_VALUES.contains(&policy_type.as_str()),
        "case {case:?}: PolicyType {policy_type:?} is not a token the model declares \
         ({:?})",
        model::POLICY_TYPE_VALUES
    );
    let status = text(listed, "Status");
    assert!(
        model::POLICY_STATUS_VALUES.contains(&status.as_str()),
        "case {case:?}: Status {status:?} is not a token the model declares ({:?})",
        model::POLICY_STATUS_VALUES
    );
    let name = text(listed, "Name");
    assert!(
        !name.is_empty() && name.len() <= MAX_POLICY_NAME,
        "case {case:?}: Name {name:?} is {} bytes, outside PolicyName's \
         length(max: {MAX_POLICY_NAME})",
        name.len()
    );
    assert!(
        version.version_id >= 1,
        "case {case:?}: VersionId {} is below PolicyVersionId's range(min: 1)",
        version.version_id
    );
    assert!(
        is_qualified_policy_arn(&text(listed, "Arn")),
        "case {case:?}: Arn {:?} does not satisfy QualifiedPolicyArn's pattern",
        text(listed, "Arn")
    );
}

/// One timestamp member against the instant the store holds.
pub(crate) fn assert_instant(case: &str, listed: &Value, member: &str, expected: SystemTime) {
    let expected_secs = expected
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs_f64();
    let actual = instant(listed, member);
    assert!(
        (actual - expected_secs).abs() < INSTANT_TOLERANCE,
        "case {case:?}: expected {member} {expected_secs}, got {actual} in {listed:?}"
    );
}

/// Whether `arn` satisfies `QualifiedPolicyArn`'s modelled pattern
/// `^arn:.{1,63}:mpa:::aws:policy/[a-zA-Z0-9_\.-]{1,1023}/[a-zA-Z0-9_\.-]{1,1023}/(?:[\d]+|\$DEFAULT)$`
/// and its `length(max: 1224)`.
pub(crate) fn is_qualified_policy_arn(arn: &str) -> bool {
    if arn.len() > MAX_POLICY_ARN {
        return false;
    }
    let Some(rest) = arn.strip_prefix("arn:") else {
        return false;
    };
    let Some((partition, rest)) = rest.split_once(':') else {
        return false;
    };
    if partition.is_empty() || partition.len() > 63 {
        return false;
    }
    let Some(path) = rest.strip_prefix("mpa:::aws:policy/") else {
        return false;
    };
    let segments: Vec<&str> = path.split('/').collect();
    if segments.len() != 3 {
        return false;
    }
    let qualifier = segments[2];
    let qualifier_ok = qualifier == "$DEFAULT"
        || (!qualifier.is_empty() && qualifier.bytes().all(|b| b.is_ascii_digit()));
    qualifier_ok
        && segments[..2].iter().all(|segment| {
            (1..=1023).contains(&segment.len())
                && segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
        })
}

/// Walk every page for `policy_arn` at `page_size` (or the modelled default when it is
/// `None`) and answer the listed entries in the order they were returned.
async fn walk(
    harness: &Harness,
    case: &str,
    policy_arn: &str,
    page_size: Option<i64>,
    version_count: usize,
) -> Vec<Value> {
    let mut seen: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    let limit = page_size.unwrap_or(MAX_RESULTS_MAX);
    loop {
        let mut query = Vec::new();
        if let Some(size) = page_size {
            query.push(format!("MaxResults={size}"));
        }
        if let Some(t) = &token {
            query.push(format!("NextToken={t}"));
        }
        let (status, body) = list_policy_versions(harness, policy_arn, &query.join("&")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListPolicyVersions returned {status} with {body:?}"
        );
        let page = body["PolicyVersions"].as_array().cloned().unwrap_or_default();
        assert!(
            page.len() as i64 <= limit,
            "case {case:?}: a page of {} exceeds MaxResults {limit}",
            page.len()
        );
        seen.extend(page.iter().cloned());
        pages += 1;
        assert!(
            pages <= version_count + 1,
            "case {case:?}: paging did not terminate after {pages} pages"
        );
        match body.get("NextToken").and_then(Value::as_str) {
            Some(next) => {
                assert_eq!(
                    Some(next.to_string()),
                    page.last()
                        .and_then(|listed| listed.get("VersionId"))
                        .and_then(Value::as_i64)
                        .map(|id| id.to_string()),
                    "case {case:?}: a NextToken must name the last version of its page, \
                     got {next:?}"
                );
                token = Some(next.to_string());
            }
            None => {
                assert_eq!(
                    seen.len(),
                    version_count,
                    "case {case:?}: paging ended without a NextToken while versions \
                     remained"
                );
                break;
            }
        }
    }
    seen
}

/// The versions a fresh store seeds are reported as the store holds them: every member
/// read out of the record, no `NextToken` while the default page covers them, and nothing
/// changed by the read.
#[tokio::test]
async fn the_seeded_catalogs_versions_are_reported_out_of_state() {
    // No organization is recorded: reading the catalog is not an organization-only
    // operation, so it answers a plain caller.
    let harness = Harness::without_organization();
    let seeded: Vec<String> = {
        let store = state::service_state(&*harness.engine).expect("the service store");
        let guard = store.lock();
        guard.policies.keys().cloned().collect()
    };
    assert!(
        !seeded.is_empty(),
        "a fresh store must seed the policy catalog; it holds {seeded:?}"
    );

    for arn in &seeded {
        let policy = stored_policy(&harness, arn);
        assert!(
            !policy.versions.is_empty(),
            "a seeded policy must hold at least one version: {policy:?}"
        );
        assert!(
            policy.versions.contains_key(&policy.default_version),
            "a policy's DefaultVersion must be a version it holds: {policy:?}"
        );

        let (status, body) = list_policy_versions(&harness, arn, "").await;
        assert_eq!(
            status, 200,
            "case {arn:?}: ListPolicyVersions returned {status} with {body:?}"
        );
        let listed = body["PolicyVersions"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            listed.len(),
            policy.versions.len(),
            "case {arn:?}: the default MaxResults of {MAX_RESULTS_MAX} must hold the \
             seeded versions: {body:?}"
        );
        for (entry, version) in listed.iter().zip(policy.versions.values()) {
            assert_entry(arn, entry, &policy, version);
        }
        assert!(
            body.get("NextToken").is_none(),
            "case {arn:?}: a complete page carries no NextToken: {body:?}"
        );

        // A read changes nothing a later caller sees: the same call answers the same, and
        // the store is exactly as it was.
        let (status, again) = list_policy_versions(&harness, arn, "").await;
        assert_eq!(status, 200, "case {arn:?}: the second call returned {again:?}");
        assert_eq!(
            again, body,
            "case {arn:?}: ListPolicyVersions is a read: two calls must agree"
        );
        assert_eq!(
            stored_version_ids(&harness, arn),
            policy.versions.keys().copied().collect::<Vec<i32>>(),
            "case {arn:?}: ListPolicyVersions must not change the versions it reports"
        );
    }
}

/// Every version of a policy is reported from its stored record, in ascending version-id
/// order, exactly once per walk, for every version history and every page size.
#[tokio::test]
async fn paging_reports_every_version_exactly_once_from_state() {
    for row in version_cases() {
        let harness = Harness::without_organization();
        let arn = record_policy(&harness, &row);
        let policy = stored_policy(&harness, &arn);
        let expected: Vec<i64> = stored_version_ids(&harness, &arn)
            .into_iter()
            .map(i64::from)
            .collect();
        assert_eq!(
            expected.len(),
            row.versions.len(),
            "case {:?}: the store must hold every recorded version",
            row.case
        );

        // Each published bound of `MaxResults` (1 and 20), the sizes between, and the
        // modelled default (`MaxResults` omitted).
        for page_size in [
            Some(MAX_RESULTS_MIN),
            Some(2),
            Some(3),
            Some(7),
            Some(MAX_RESULTS_MAX),
            None,
        ] {
            let case = format!(
                "{} ({} versions, MaxResults={})",
                row.case,
                expected.len(),
                page_size.map_or("default".to_string(), |size| size.to_string())
            );
            let seen = walk(&harness, &case, &arn, page_size, expected.len()).await;
            let ids: Vec<i64> = seen
                .iter()
                .map(|listed| {
                    listed
                        .get("VersionId")
                        .and_then(Value::as_i64)
                        .unwrap_or_else(|| panic!("case {case:?}: no VersionId in {listed:?}"))
                })
                .collect();
            assert_eq!(
                ids, expected,
                "case {case:?}: paging must yield every version exactly once, in \
                 ascending version-id order"
            );
            assert_eq!(
                ids.iter().collect::<BTreeSet<_>>().len(),
                ids.len(),
                "case {case:?}: paging returned a version twice: {ids:?}"
            );
            for listed in &seen {
                let version_id = i32::try_from(
                    listed.get("VersionId").and_then(Value::as_i64).unwrap_or(0),
                )
                .unwrap_or(0);
                let version = policy
                    .version(version_id)
                    .unwrap_or_else(|| panic!("case {case:?}: {version_id} is in no record"));
                assert_entry(&case, listed, &policy, version);
            }
            let defaults = seen
                .iter()
                .filter(|listed| listed.get("IsDefault").and_then(Value::as_bool) == Some(true))
                .count();
            assert_eq!(
                defaults, 1,
                "case {case:?}: exactly one listed version is the default, got {defaults}"
            );
        }

        assert_eq!(
            stored_version_ids(&harness, &arn)
                .into_iter()
                .map(i64::from)
                .collect::<Vec<i64>>(),
            expected,
            "case {:?}: ListPolicyVersions must not change the versions it reports",
            row.case
        );
    }
}

/// A `PolicyArn` the catalog does not hold is `ResourceNotFoundException` (404), whatever
/// its form — the catalog is the only thing consulted.
#[tokio::test]
async fn an_unknown_policy_is_not_found() {
    let harness = Harness::without_organization();
    let known = record_policy(&harness, &version_cases()[1]);

    // Every row satisfies `UnqualifiedPolicyArn`'s pattern, so the request reaches the
    // handler; none of them is a policy the catalog holds.
    let rows: [(&str, String); 5] = [
        (
            "a name no policy carries",
            state::policy_arn("IdentityCenter", "NeverSeeded"),
        ),
        (
            "a category no policy carries",
            state::policy_arn("NoSuchCategory", "MPAApprovalTeamPolicy"),
        ),
        (
            "a maximum-length name",
            state::policy_arn("IdentityCenter", &"z".repeat(MAX_POLICY_NAME)),
        ),
        (
            "another partition",
            "arn:aws-us-gov:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy".to_string(),
        ),
        (
            "a near-miss of a known policy's name",
            state::policy_arn("IdentityCenter", "Two.Version_Policy-2"),
        ),
    ];

    for (case, policy_arn) in rows {
        let (status, body) = list_policy_versions(&harness, &policy_arn, "").await;
        assert_eq!(
            status, 404,
            "case {case:?}: an unknown policy must be 404, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: wrong error code in {body:?}"
        );
    }

    // The refusal is about the policy's existence, not about the request: the policy the
    // store does hold answers 200.
    let (status, body) = list_policy_versions(&harness, &known, "").await;
    assert_eq!(
        status, 200,
        "a policy the catalog holds must be listed, got {status} with {body:?}"
    );
}

/// A `NextToken` this service never issued is refused with `ValidationException` (400),
/// whatever its form; the token the service did issue is accepted.
#[tokio::test]
async fn a_foreign_next_token_is_refused() {
    let harness = Harness::without_organization();
    let row = &version_cases()[3];
    let arn = record_policy(&harness, row);

    // (case, the token). `%20` is a percent-encoded space: the query decoder hands the
    // handler " ".
    let rows: [(&str, String); 7] = [
        ("not a token at all", "not-a-token".to_string()),
        ("a space", "%20".to_string()),
        ("below PolicyVersionId's range(min: 1)", "0".to_string()),
        ("a negative version id", "-1".to_string()),
        ("a version id the policy does not hold", "3".to_string()),
        ("a version id past every one held", "99".to_string()),
        (
            "a qualified policy-version ARN",
            encode_label(&format!("{arn}/1")),
        ),
    ];

    for (case, token) in rows {
        let (status, body) = list_policy_versions(&harness, &arn, &format!("NextToken={token}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a foreign NextToken must be refused, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: wrong error code in {body:?}"
        );
    }

    // The token the service DID issue is accepted, so the refusal is about a token's
    // provenance and not about tokens.
    let (status, body) = list_policy_versions(&harness, &arn, "MaxResults=1").await;
    assert_eq!(status, 200, "the first page returned {body:?}");
    let token = text(&body, "NextToken");
    assert!(
        !token.is_empty(),
        "a partial page must carry a NextToken: {body:?}"
    );
    let (status, body) =
        list_policy_versions(&harness, &arn, &format!("MaxResults=1&NextToken={token}")).await;
    assert_eq!(
        status, 200,
        "the service's own NextToken must be accepted, got {status} with {body:?}"
    );
}

/// `MaxResults` outside its modelled `range(1..20)` is refused before the handler runs:
/// each published bound is accepted, each value one past it is not.
#[tokio::test]
async fn max_results_outside_its_modelled_range_is_refused() {
    let harness = Harness::without_organization();
    let arn = record_policy(&harness, &version_cases()[2]);

    // (MaxResults, accepted)
    let rows: [(i64, bool); 5] = [
        (MAX_RESULTS_MIN - 1, false),
        (MAX_RESULTS_MIN, true),
        (MAX_RESULTS_MAX, true),
        (MAX_RESULTS_MAX + 1, false),
        (-1, false),
    ];

    for (max_results, accepted) in rows {
        let (status, body) =
            list_policy_versions(&harness, &arn, &format!("MaxResults={max_results}")).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case MaxResults={max_results}: expected 200, got {status} with {body:?}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case MaxResults={max_results}: expected 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case MaxResults={max_results}: wrong error code in {body:?}"
        );
    }
}
