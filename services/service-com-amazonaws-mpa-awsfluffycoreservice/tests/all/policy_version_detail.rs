// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetPolicyVersion`: ONE version of a catalog policy.
//!
//! Every request goes over the wire the real client uses: a `restJson1` HTTP request
//! (`GET /policy-versions/{PolicyVersionArn}`, the ARN bound to a single path segment)
//! dispatched through the `MultiServiceRouter` the harness in `approval_team_lifecycle.rs`
//! mounts, so the route, the URI binding, the JSON envelope and the HTTP status are all
//! exercised.
//!
//! The rule under test: `GetPolicyVersion` reports the version the store holds for the
//! policy its qualified `PolicyVersionArn` names, every member computed from that stored
//! version and the policy that owns it; the ARN's version qualifier is either the
//! version's id or `$DEFAULT`, which resolves against the policy's recorded default
//! version; an ARN the catalog holds no version for is `ResourceNotFoundException` (404);
//! and the read changes nothing a later caller sees.
//!
//! So the tables below span VERSION COUNT (one version, several, a full page and past
//! it), the position of the DEFAULT version (first, middle, last), contiguous and
//! NON-CONTIGUOUS version ids, both `PolicyType` tokens, both `PolicyStatus` tokens and
//! several policy-name and category forms (including every character class
//! `UnqualifiedPolicyArn`'s pattern allows and `PolicyName`'s length bound), crossed with
//! the two QUALIFIER forms the model declares. Each member is checked against the record
//! it came from and against the model's own catalogs (`PolicyType`'s and `PolicyStatus`'s
//! enum values, `PolicyName`'s `length(max: 64)`, `PolicyVersionId`'s `range(min: 1)`,
//! `QualifiedPolicyArn`'s pattern, `PolicyDocument`'s `length(max: 400000)`), and the
//! document is checked to be the one the resource-policy reads report for the very same
//! version, so the two readers cannot drift apart.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::Value;
use service_com_amazonaws_mpa_awsfluffycoreservice::ops::types::shared_types as model;
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_team_lifecycle::{encode_label, error_code, text, Harness};
use crate::policy_versions::{
    assert_instant, base_instant, is_qualified_policy_arn, status_for, stored_policy,
    MAX_POLICY_NAME,
};
use crate::resource_policies::{create_body, get_resource_policy};

/// `PolicyDocument` is `length(max: 400000)`.
const MAX_POLICY_DOCUMENT: usize = 400_000;

/// The qualifier `QualifiedPolicyArn`'s pattern allows in place of a version id.
const DEFAULT_QUALIFIER: &str = "$DEFAULT";

/// One row of the policy table: a catalog policy and the versions it holds.
struct DetailCase {
    case: &'static str,
    category: &'static str,
    name: String,
    policy_type: state::PolicyType,
    /// `(version id, status)`, in no particular order — the handler resolves a version by
    /// ARN, so this table's order must not matter.
    versions: Vec<(i32, state::PolicyStatus)>,
    default_version: i32,
}

/// `count` contiguous versions, `1..=count`, the older ones deprecated.
fn contiguous(count: i32) -> Vec<(i32, state::PolicyStatus)> {
    (1..=count).map(|id| (id, status_for(id, count))).collect()
}

/// The policy table: rows differ along version count, default-version position, version id
/// spacing, `PolicyType`, `PolicyStatus` and the form of the policy name and category.
fn detail_cases() -> Vec<DetailCase> {
    vec![
        DetailCase {
            case: "one version, which is the default",
            category: "IdentityCenter",
            name: "SingleVersionPolicy".to_string(),
            policy_type: state::PolicyType::AwsManaged,
            versions: contiguous(1),
            default_version: 1,
        },
        DetailCase {
            case: "two versions, the newer is the default, every name character class",
            category: "IdentityCenter",
            name: "Two.Version_Policy-1".to_string(),
            policy_type: state::PolicyType::AwsRam,
            versions: contiguous(2),
            default_version: 2,
        },
        DetailCase {
            case: "three versions, a middle default",
            category: "Organizations",
            name: "MiddleDefaultPolicy".to_string(),
            policy_type: state::PolicyType::AwsManaged,
            versions: contiguous(3),
            default_version: 2,
        },
        DetailCase {
            case: "non-contiguous, multi-digit ids declared out of order",
            category: "ResourceAccessManager",
            name: "mpa.ram_team-policy".to_string(),
            policy_type: state::PolicyType::AwsRam,
            versions: vec![
                (142, state::PolicyStatus::Attachable),
                (1, state::PolicyStatus::Deprecated),
                (42, state::PolicyStatus::Attachable),
                (2, state::PolicyStatus::Deprecated),
            ],
            default_version: 42,
        },
        DetailCase {
            case: "seven versions, the first is the default",
            category: "Ec2",
            name: "SevenVersionPolicy".to_string(),
            policy_type: state::PolicyType::AwsManaged,
            versions: contiguous(7),
            default_version: 1,
        },
        DetailCase {
            // `PolicyName` is `length(max: 64)`; `ListPolicyVersions` tops out at one page
            // of 20, so this is a policy whose history fills a page.
            case: "one full page of versions, maximum-length name",
            category: "IdentityCenter",
            name: "N".repeat(MAX_POLICY_NAME),
            policy_type: state::PolicyType::AwsRam,
            versions: contiguous(20),
            default_version: 20,
        },
        DetailCase {
            case: "more versions than one page, the last is the default",
            category: "IdentityCenter",
            name: "JustPastOnePagePolicy".to_string(),
            policy_type: state::PolicyType::AwsManaged,
            versions: contiguous(21),
            default_version: 21,
        },
    ]
}

/// `GET /policy-versions/{PolicyVersionArn}`.
async fn get_policy_version(harness: &Harness, policy_version_arn: &str) -> (u16, Value) {
    harness
        .call(
            "GET",
            &format!("/policy-versions/{}", encode_label(policy_version_arn)),
            None,
        )
        .await
}

/// Record a catalog policy holding this row's versions and answer its unqualified ARN.
///
/// No modelled operation mints a policy or one of its versions — the catalog is AWS's — so
/// a table that needs a version history writes it directly, the way
/// `record_management_account` records an organization. Each version is dated from
/// `base_instant`, a minute apart, and last updated half a minute after it was created, so
/// every reported timestamp is a distinct value read out of the record.
fn record_policy(harness: &Harness, case: &DetailCase) -> String {
    let mut policy = state::CatalogPolicy {
        arn: state::policy_arn(case.category, &case.name),
        name: case.name.clone(),
        policy_type: case.policy_type,
        default_version: case.default_version,
        versions: BTreeMap::new(),
    };
    for &(version_id, status) in &case.versions {
        let created =
            base_instant() + Duration::from_secs(u64::from(version_id.unsigned_abs()) * 60);
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

/// Every member of one reported version, against the records it came from and the model's
/// constraints.
fn assert_detail(
    case: &str,
    reported: &Value,
    policy: &state::CatalogPolicy,
    version: &state::CatalogPolicyVersion,
) {
    let expected_arn = state::policy_version_arn(&policy.arn, version.version_id);
    assert_eq!(
        text(reported, "Arn"),
        expected_arn,
        "case {case:?}: expected Arn {expected_arn:?}, got {reported:?}"
    );
    assert_eq!(
        text(reported, "PolicyArn"),
        policy.arn,
        "case {case:?}: expected PolicyArn {:?}, got {reported:?}",
        policy.arn
    );
    assert_eq!(
        reported.get("VersionId").and_then(Value::as_i64),
        Some(i64::from(version.version_id)),
        "case {case:?}: expected VersionId {}, got {reported:?}",
        version.version_id
    );
    assert_eq!(
        text(reported, "PolicyType"),
        policy.policy_type.as_wire(),
        "case {case:?}: expected PolicyType {:?}, got {reported:?}",
        policy.policy_type.as_wire()
    );
    assert_eq!(
        text(reported, "Name"),
        policy.name,
        "case {case:?}: expected Name {:?}, got {reported:?}",
        policy.name
    );
    assert_eq!(
        text(reported, "Status"),
        version.status.as_wire(),
        "case {case:?}: expected Status {:?}, got {reported:?}",
        version.status.as_wire()
    );
    assert_eq!(
        reported.get("IsDefault").and_then(Value::as_bool),
        Some(version.version_id == policy.default_version),
        "case {case:?}: IsDefault must hold exactly for the policy's DefaultVersion {}, \
         got {:?}",
        policy.default_version,
        reported.get("IsDefault")
    );
    // The document this service reports for this version of this policy — the very value
    // the resource-policy reads report for it, so the two readers cannot disagree.
    assert_eq!(
        text(reported, "Document"),
        state::policy_version_document(policy, version.version_id),
        "case {case:?}: Document is not the document this service holds for this version: \
         {reported:?}"
    );
    assert_instant(case, reported, "CreationTime", version.creation_time);
    assert_instant(case, reported, "LastUpdatedTime", version.last_updated_time);

    // The model's own catalogs and bounds, not this test's opinion.
    let policy_type = text(reported, "PolicyType");
    assert!(
        model::POLICY_TYPE_VALUES.contains(&policy_type.as_str()),
        "case {case:?}: PolicyType {policy_type:?} is not a token the model declares ({:?})",
        model::POLICY_TYPE_VALUES
    );
    let status = text(reported, "Status");
    assert!(
        model::POLICY_STATUS_VALUES.contains(&status.as_str()),
        "case {case:?}: Status {status:?} is not a token the model declares ({:?})",
        model::POLICY_STATUS_VALUES
    );
    let name = text(reported, "Name");
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
        is_qualified_policy_arn(&text(reported, "Arn")),
        "case {case:?}: Arn {:?} does not satisfy QualifiedPolicyArn's pattern",
        text(reported, "Arn")
    );
    let document = text(reported, "Document");
    assert!(
        !document.is_empty() && document.len() <= MAX_POLICY_DOCUMENT,
        "case {case:?}: Document is {} bytes, outside PolicyDocument's \
         length(max: {MAX_POLICY_DOCUMENT})",
        document.len()
    );
    let parsed: Value = serde_json::from_str(&document)
        .unwrap_or_else(|e| panic!("case {case:?}: Document is not JSON: {e} ({document:?})"));
    assert!(
        parsed["Statement"][0]["Effect"].is_string(),
        "case {case:?}: the document carries no statement: {parsed:?}"
    );
}

/// The `PolicyVersion` object a 200 answer carries.
fn policy_version(case: &str, body: &Value) -> Value {
    body.get("PolicyVersion")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no PolicyVersion in {body:?}"))
}

/// Every version the store holds is reported from its own record when its numeric
/// qualified ARN is asked for, and the read changes nothing a later caller sees.
#[tokio::test]
async fn every_stored_version_is_reported_by_its_qualified_arn() {
    for row in detail_cases() {
        // Reading the catalog is not an organization-only operation, so it answers a
        // caller for whom no organization is recorded.
        let harness = Harness::without_organization();
        let policy_arn = record_policy(&harness, &row);
        let policy = stored_policy(&harness, &policy_arn);

        for version in policy.versions.values() {
            let arn = state::policy_version_arn(&policy_arn, version.version_id);
            let case = format!("{} (version {})", row.case, version.version_id);
            let (status, body) = get_policy_version(&harness, &arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: GetPolicyVersion returned {status} with {body:?}"
            );
            assert_detail(&case, &policy_version(&case, &body), &policy, version);

            // A read changes nothing: the same call answers the same, and the stored
            // record is exactly as it was.
            let (status, again) = get_policy_version(&harness, &arn).await;
            assert_eq!(status, 200, "case {case:?}: the second call returned {again:?}");
            assert_eq!(
                again, body,
                "case {case:?}: GetPolicyVersion is a read: two calls must agree"
            );
        }

        // Two versions of one policy are two resources: each reports its own id, ARN and
        // document.
        let documents: Vec<String> = policy
            .versions
            .values()
            .map(|version| state::policy_version_document(&policy, version.version_id))
            .collect();
        assert_eq!(
            documents.iter().collect::<std::collections::BTreeSet<_>>().len(),
            documents.len(),
            "case {:?}: two versions of one policy must not share a document",
            row.case
        );

        assert_eq!(
            format!("{:?}", stored_policy(&harness, &policy_arn)),
            format!("{policy:?}"),
            "case {:?}: GetPolicyVersion must not change the record it reports",
            row.case
        );
    }
}

/// The `$DEFAULT` qualifier names whichever version the policy records as its default: the
/// answer is the one that version's numeric ARN gives, down to the `Arn` it reports.
#[tokio::test]
async fn the_default_qualifier_resolves_to_the_recorded_default_version() {
    for row in detail_cases() {
        let harness = Harness::without_organization();
        let policy_arn = record_policy(&harness, &row);
        let policy = stored_policy(&harness, &policy_arn);
        let default = policy
            .version(policy.default_version)
            .unwrap_or_else(|| panic!("case {:?}: the default must be a stored version", row.case));

        let alias = format!("{policy_arn}/{DEFAULT_QUALIFIER}");
        let (status, body) = get_policy_version(&harness, &alias).await;
        assert_eq!(
            status, 200,
            "case {:?}: $DEFAULT returned {status} with {body:?}",
            row.case
        );
        let reported = policy_version(row.case, &body);
        assert_detail(row.case, &reported, &policy, default);
        assert_eq!(
            reported.get("IsDefault").and_then(Value::as_bool),
            Some(true),
            "case {:?}: the version $DEFAULT names must report IsDefault, got {reported:?}",
            row.case
        );

        let numeric = state::policy_version_arn(&policy_arn, policy.default_version);
        let (status, direct) = get_policy_version(&harness, &numeric).await;
        assert_eq!(
            status, 200,
            "case {:?}: {numeric} returned {status} with {direct:?}",
            row.case
        );
        assert_eq!(
            body, direct,
            "case {:?}: $DEFAULT and the default version's own ARN name one resource",
            row.case
        );
    }
}

/// An ARN the catalog holds no version for is `ResourceNotFoundException` (404), whatever
/// its form — the catalog is the only thing consulted.
#[tokio::test]
async fn an_arn_the_catalog_holds_no_version_for_is_not_found() {
    let harness = Harness::without_organization();
    let row = &detail_cases()[3];
    let known = record_policy(&harness, row);

    // A policy whose recorded default names no version it holds: `$DEFAULT` then resolves
    // to nothing, so there is no resource to report.
    let dangling = record_policy(
        &harness,
        &DetailCase {
            case: "a default that names no stored version",
            category: "IdentityCenter",
            name: "DanglingDefaultPolicy".to_string(),
            policy_type: state::PolicyType::AwsManaged,
            versions: contiguous(2),
            default_version: 9,
        },
    );

    // Every row satisfies `QualifiedPolicyArn`'s pattern, so the request reaches the
    // handler; none of them names a version the catalog holds.
    let rows: [(&str, String); 9] = [
        (
            "a policy name no entry carries",
            format!("{}/1", state::policy_arn("IdentityCenter", "NeverSeeded")),
        ),
        (
            "a category no entry carries",
            format!("{}/1", state::policy_arn("NoSuchCategory", "mpa.ram_team-policy")),
        ),
        (
            "$DEFAULT of a policy no entry carries",
            format!(
                "{}/{DEFAULT_QUALIFIER}",
                state::policy_arn("IdentityCenter", "NeverSeeded")
            ),
        ),
        (
            "another partition",
            "arn:aws-us-gov:mpa:::aws:policy/ResourceAccessManager/mpa.ram_team-policy/1"
                .to_string(),
        ),
        ("a version id this policy does not hold", format!("{known}/3")),
        ("a version id past every one held", format!("{known}/999")),
        (
            "below PolicyVersionId's range(min: 1)",
            format!("{known}/0"),
        ),
        (
            "a version id no PolicyVersionId can hold",
            format!("{known}/99999999999999999999"),
        ),
        (
            "$DEFAULT of a policy whose default names no stored version",
            format!("{dangling}/{DEFAULT_QUALIFIER}"),
        ),
    ];

    for (case, arn) in rows {
        let (status, body) = get_policy_version(&harness, &arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: {arn} must be 404, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: wrong error code in {body:?}"
        );
    }

    // The refusal is about the version's existence, not about the request: every version
    // the store does hold answers 200.
    for version_id in [1, 2, 42, 142] {
        let arn = state::policy_version_arn(&known, version_id);
        let (status, body) = get_policy_version(&harness, &arn).await;
        assert_eq!(
            status, 200,
            "case {arn:?}: a stored version must be reported, got {status} with {body:?}"
        );
    }
}

/// An ARN outside `QualifiedPolicyArn`'s modelled pattern is refused with
/// `ValidationException` (400) before the handler runs — a bad request, not a missing
/// resource.
#[tokio::test]
async fn an_arn_outside_the_modelled_pattern_is_refused() {
    let harness = Harness::without_organization();
    let known = record_policy(&harness, &detail_cases()[0]);

    let rows: [(&str, String); 6] = [
        ("an unqualified policy ARN", known.clone()),
        ("a qualifier that is neither digits nor $DEFAULT", format!("{known}/v1")),
        ("another dollar qualifier", format!("{known}/$LATEST")),
        ("a trailing empty qualifier", format!("{known}//")),
        ("one segment too many", format!("{known}/1/2")),
        (
            "an approval-team ARN",
            format!(
                "arn:aws:mpa:us-east-1:{}:approval-team/Team/1",
                engine::emulator_account_id()
            ),
        ),
    ];

    for (case, arn) in rows {
        let (status, body) = get_policy_version(&harness, &arn).await;
        assert_eq!(
            status, 400,
            "case {case:?}: {arn} must be refused, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: wrong error code in {body:?}"
        );
    }

    // The refusal is about the ARN's form: the same policy's own version ARN is accepted.
    let (status, body) = get_policy_version(&harness, &format!("{known}/1")).await;
    assert_eq!(
        status, 200,
        "a well-formed ARN must be accepted, got {status} with {body:?}"
    );
}

/// The document `GetPolicyVersion` reports for a version is the one the resource-policy
/// reads report for that same version, whichever qualifier form the attachment used: both
/// resolve one qualified ARN through the store's single resolver.
#[tokio::test]
async fn the_document_agrees_with_the_resource_policy_read_of_the_same_version() {
    // A team can only be created by the organization's management account, which
    // `Harness::new` records.
    let harness = Harness::new();
    let row = &detail_cases()[3];
    let policy_arn = record_policy(&harness, row);
    let policy = stored_policy(&harness, &policy_arn);

    // The two qualifier forms an attachment may carry: `$DEFAULT`, and an explicit
    // version id that is NOT the default.
    let attachments = [
        (
            "attached as $DEFAULT",
            format!("{policy_arn}/{DEFAULT_QUALIFIER}"),
            policy.default_version,
        ),
        (
            "attached by an explicit, non-default version id",
            state::policy_version_arn(&policy_arn, 142),
            142,
        ),
    ];

    for (index, (case, attachment, expected_version)) in attachments.iter().enumerate() {
        let team_name = format!("DocumentAgreement{index}");
        let (status, created) = harness
            .create(&create_body(&team_name, std::slice::from_ref(attachment)))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateApprovalTeam returned {status} with {created:?}"
        );
        let team_arn = text(&created, "Arn");

        let (status, attached) = get_resource_policy(
            &harness,
            &team_arn,
            &policy.name,
            policy.policy_type.as_wire(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetResourcePolicy returned {status} with {attached:?}"
        );

        let version_arn = state::policy_version_arn(&policy_arn, *expected_version);
        assert_eq!(
            text(&attached, "PolicyVersionArn"),
            version_arn,
            "case {case:?}: the attachment must resolve to version {expected_version}: \
             {attached:?}"
        );

        let (status, body) = get_policy_version(&harness, attachment).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetPolicyVersion returned {status} with {body:?}"
        );
        let reported = policy_version(case, &body);
        assert_eq!(
            text(&reported, "Document"),
            text(&attached, "PolicyDocument"),
            "case {case:?}: the two readers report different documents for {version_arn}"
        );
        assert_eq!(
            text(&reported, "Arn"),
            version_arn,
            "case {case:?}: GetPolicyVersion must resolve the attachment's own version: \
             {reported:?}"
        );
    }
}

/// The versions a fresh store seeds are reported out of state, and `GetPolicyVersion`
/// agrees with `ListPolicyVersions` on every member the two share.
#[tokio::test]
async fn the_seeded_catalog_is_reported_and_agrees_with_the_listing() {
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

    for policy_arn in &seeded {
        let policy = stored_policy(&harness, policy_arn);
        let (status, listing) = harness
            .call(
                "POST",
                &format!("/policies/{}/?List", encode_label(policy_arn)),
                None,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {policy_arn:?}: ListPolicyVersions returned {status} with {listing:?}"
        );
        let listed = listing["PolicyVersions"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            listed.len(),
            policy.versions.len(),
            "case {policy_arn:?}: the listing must cover the seeded versions: {listing:?}"
        );

        for entry in &listed {
            let version_id =
                i32::try_from(entry.get("VersionId").and_then(Value::as_i64).unwrap_or(0))
                    .unwrap_or(0);
            let version = policy
                .version(version_id)
                .unwrap_or_else(|| panic!("case {policy_arn:?}: {version_id} is in no record"));
            let arn = state::policy_version_arn(policy_arn, version_id);
            let case = format!("seeded {policy_arn} version {version_id}");
            let (status, body) = get_policy_version(&harness, &arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: GetPolicyVersion returned {status} with {body:?}"
            );
            let reported = policy_version(&case, &body);
            assert_detail(&case, &reported, &policy, version);

            // The two readers answer out of the one record, so every shared member agrees.
            for member in [
                "Arn",
                "PolicyArn",
                "Name",
                "PolicyType",
                "Status",
                "VersionId",
                "IsDefault",
                "CreationTime",
                "LastUpdatedTime",
            ] {
                assert_eq!(
                    reported.get(member),
                    entry.get(member),
                    "case {case:?}: {member} disagrees between GetPolicyVersion \
                     ({:?}) and ListPolicyVersions ({:?})",
                    reported.get(member),
                    entry.get(member)
                );
            }
        }
    }
}
