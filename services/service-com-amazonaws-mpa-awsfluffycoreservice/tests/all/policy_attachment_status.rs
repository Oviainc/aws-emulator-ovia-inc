// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the STATUS half of the one attach gate this service holds: a
//! `Policies` reference is attachable only when the catalog both holds the version it
//! names AND records that version in a status that still allows attaching it.
//!
//! `PolicyStatus` is the catalog's own statement of whether one version of a policy may
//! still be attached to a team — the two tokens the model declares are `ATTACHABLE` and
//! `DEPRECATED` (`smithy_query(query="shape",shape="PolicyStatus")`), and this service
//! PUBLISHES that token for every version it holds through `ListPolicyVersions` and
//! `GetPolicyVersion`. So the verdict of the attach gate follows the published status:
//! a create naming a version this service reports as attachable is served, and one naming
//! a version it reports in any other status earns `ValidationException` (400) and stores
//! nothing — while the version itself keeps being resolved, so the catalog reads go on
//! reporting it.
//!
//! The table spans the class the rule generalises over: the status of the version the
//! reference resolves to, how the reference names it (an explicit `PolicyVersionId` or
//! `$DEFAULT`, which resolves against the policy's recorded default), which version of
//! the history is the deprecated one (first, middle, last), the version-id spacing, how
//! many references the request carries and where in the list the unattachable one sits,
//! the other half of the gate (a reference that resolves to no version at all) ahead of
//! and behind it, and a reference long enough to push the refusal past the bound its own
//! `Message` member declares.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the route, the URI bindings, the JSON envelope
//! and the HTTP status are all exercised.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::ops::types::shared_types as model;
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_team_lifecycle::{
    encode_label, error_code, expected_arn, recorded_policy_document, text, Harness, REGION,
};

/// Category the policies this suite records sit in — one segment of
/// `UnqualifiedPolicyArn`'s pattern, so any `[a-zA-Z0-9_\.-]` value does.
const CATEGORY: &str = "ResourceAccessManager";

/// Every token `PolicyStatus` declares, paired with the status a catalog record holds for
/// it. `model::POLICY_STATUS_VALUES` is rendered from the model's own enum, so a token
/// added to it fails this table instead of going untested.
fn status_tokens() -> Vec<(&'static str, state::PolicyStatus)> {
    let tokens = vec![
        ("ATTACHABLE", state::PolicyStatus::Attachable),
        ("DEPRECATED", state::PolicyStatus::Deprecated),
    ];
    let named: Vec<&str> = tokens.iter().map(|(token, _)| *token).collect();
    assert_eq!(
        named, model::POLICY_STATUS_VALUES,
        "the table covers {named:?}, not the model's {:?}",
        model::POLICY_STATUS_VALUES
    );
    tokens
}

/// A fixed instant the recorded versions are dated from, so no assertion below depends on
/// when the test ran.
fn base_instant() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

/// Record a catalog policy holding `versions` — `(version id, status)` — with
/// `default_version` as its default, and answer its unqualified ARN.
///
/// No modelled operation mints a policy or one of its versions (the catalog is AWS's), so
/// a table that needs a version history writes it directly, the way `policy_versions.rs`
/// and `write_path_input_validation.rs` do.
fn record_policy(
    harness: &Harness,
    category: &str,
    name: &str,
    versions: &[(i32, state::PolicyStatus)],
    default_version: i32,
) -> String {
    let mut policy = state::CatalogPolicy {
        arn: state::policy_arn(category, name),
        name: name.to_string(),
        policy_type: state::PolicyType::AwsRam,
        default_version,
        versions: BTreeMap::new(),
    };
    for &(version_id, status) in versions {
        let created = base_instant() + Duration::from_secs(u64::from(version_id.unsigned_abs()) * 60);
        policy.insert_version(state::CatalogPolicyVersion {
            version_id,
            status,
            creation_time: created,
            last_updated_time: created + Duration::from_secs(30),
            document: recorded_policy_document(name, version_id),
        });
    }
    let arn = policy.arn.clone();
    let store = state::service_state(&*harness.engine).expect("the service store");
    store.lock().insert_policy(policy);
    arn
}

/// A `CreateApprovalTeam` body every modelled constraint accepts, attaching `policies`.
fn create_body(name: &str, policies: &[String]) -> Value {
    json!({
        "Name": name,
        "Description": "policy attachment status",
        "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 1 } },
        "Approvers": [{
            "PrimaryIdentityId": "identity-0001",
            "PrimaryIdentitySourceArn": format!(
                "arn:aws:mpa:{REGION}:{}:identity-source/ssoins-0001",
                engine::emulator_account_id()
            ),
        }],
        "Policies": policies
            .iter()
            .map(|arn| json!({ "PolicyArn": arn }))
            .collect::<Vec<_>>(),
    })
}

/// The sentence the attach gate refuses a reference whose version the catalog records in
/// a status that no longer allows attaching it with. The status tokens are
/// `PolicyStatus`' own (see [`status_tokens`]).
fn not_attachable_message(reference: &str, status: &str) -> String {
    bounded(format!(
        "Policies names a policy version whose status is {status}: {reference}. Attach a \
         policy version ListPolicyVersions reports as ATTACHABLE."
    ))
}

/// The sentence the same gate refuses a reference the catalog holds no version for with —
/// the other half of the rule, unchanged by the status check.
fn absent_message(reference: &str) -> String {
    bounded(format!(
        "Policies names a policy version that does not exist: {reference}. Attach a \
         policy version ListPolicies and ListPolicyVersions report."
    ))
}

/// `message` cut to the bound every `Message` member of this model declares
/// (`@length(max: 1000)`, [`state::MAX_EXCEPTION_MESSAGE`]). Every sentence this suite
/// builds is ASCII, so the cut is at a character boundary.
fn bounded(message: String) -> String {
    if message.len() <= state::MAX_EXCEPTION_MESSAGE {
        return message;
    }
    message[..state::MAX_EXCEPTION_MESSAGE].to_string()
}

/// What the gate must answer for one row's `Policies` list.
enum Verdict {
    /// Every reference is attachable, and each resolves to the version id named here, in
    /// attachment order.
    Served(Vec<i32>),
    /// The reference named here resolves to a version the catalog records in this status,
    /// which is not attachable.
    NotAttachable {
        qualifier: &'static str,
        status: &'static str,
    },
    /// The reference named here resolves to no version at all.
    Absent { qualifier: &'static str },
}

/// One row of the attach table.
struct AttachCase {
    case: &'static str,
    /// The policy's version history, `(version id, status)`.
    versions: Vec<(i32, state::PolicyStatus)>,
    /// The version id the policy records as its default, which `$DEFAULT` resolves to.
    default_version: i32,
    /// The version qualifiers the create attaches, in order: a `PolicyVersionId` or
    /// `$DEFAULT`, the two forms `QualifiedPolicyArn`'s pattern allows.
    attach: Vec<&'static str>,
    verdict: Verdict,
}

/// The attach table.
fn attach_cases() -> Vec<AttachCase> {
    use state::PolicyStatus::{Attachable, Deprecated};
    vec![
        AttachCase {
            case: "the policy's only version, deprecated, named by id",
            versions: vec![(1, Deprecated)],
            default_version: 1,
            attach: vec!["1"],
            verdict: Verdict::NotAttachable {
                qualifier: "1",
                status: "DEPRECATED",
            },
        },
        AttachCase {
            case: "the policy's only version, deprecated, named as the default",
            versions: vec![(1, Deprecated)],
            default_version: 1,
            attach: vec!["$DEFAULT"],
            verdict: Verdict::NotAttachable {
                qualifier: "$DEFAULT",
                status: "DEPRECATED",
            },
        },
        AttachCase {
            case: "a deprecated version while the default is attachable",
            versions: vec![(1, Deprecated), (2, Attachable)],
            default_version: 2,
            attach: vec!["1"],
            verdict: Verdict::NotAttachable {
                qualifier: "1",
                status: "DEPRECATED",
            },
        },
        AttachCase {
            case: "the attachable version of a policy that also holds a deprecated one",
            versions: vec![(1, Deprecated), (2, Attachable)],
            default_version: 2,
            attach: vec!["2"],
            verdict: Verdict::Served(vec![2]),
        },
        AttachCase {
            case: "$DEFAULT resolving to an attachable default",
            versions: vec![(1, Deprecated), (2, Attachable)],
            default_version: 2,
            attach: vec!["$DEFAULT"],
            verdict: Verdict::Served(vec![2]),
        },
        AttachCase {
            case: "$DEFAULT resolving to a deprecated default",
            versions: vec![(1, Deprecated), (2, Attachable)],
            default_version: 1,
            attach: vec!["$DEFAULT"],
            verdict: Verdict::NotAttachable {
                qualifier: "$DEFAULT",
                status: "DEPRECATED",
            },
        },
        AttachCase {
            case: "a deprecated version before attachable ones",
            versions: vec![(1, Deprecated), (2, Attachable), (3, Attachable)],
            default_version: 3,
            attach: vec!["1", "2", "$DEFAULT"],
            verdict: Verdict::NotAttachable {
                qualifier: "1",
                status: "DEPRECATED",
            },
        },
        AttachCase {
            case: "a deprecated version among attachable ones",
            versions: vec![(1, Attachable), (2, Deprecated), (3, Attachable)],
            default_version: 3,
            attach: vec!["1", "2", "3"],
            verdict: Verdict::NotAttachable {
                qualifier: "2",
                status: "DEPRECATED",
            },
        },
        AttachCase {
            case: "a deprecated version after attachable ones",
            versions: vec![(1, Attachable), (2, Attachable), (3, Deprecated)],
            default_version: 2,
            attach: vec!["1", "$DEFAULT", "3"],
            verdict: Verdict::NotAttachable {
                qualifier: "3",
                status: "DEPRECATED",
            },
        },
        AttachCase {
            case: "every attachable version of a sparse history at once",
            versions: vec![(1, Deprecated), (4, Attachable), (9, Attachable)],
            default_version: 9,
            attach: vec!["4", "9", "$DEFAULT"],
            verdict: Verdict::Served(vec![4, 9, 9]),
        },
        AttachCase {
            case: "a deprecated version of a sparse history",
            versions: vec![(1, Attachable), (4, Deprecated), (9, Attachable)],
            default_version: 9,
            attach: vec!["4"],
            verdict: Verdict::NotAttachable {
                qualifier: "4",
                status: "DEPRECATED",
            },
        },
        AttachCase {
            case: "a version the policy does not hold, before a deprecated one",
            versions: vec![(1, Deprecated), (2, Attachable)],
            default_version: 2,
            attach: vec!["7", "1"],
            verdict: Verdict::Absent { qualifier: "7" },
        },
        AttachCase {
            case: "a deprecated version before a version the policy does not hold",
            versions: vec![(1, Deprecated), (2, Attachable)],
            default_version: 2,
            attach: vec!["1", "7"],
            verdict: Verdict::NotAttachable {
                qualifier: "1",
                status: "DEPRECATED",
            },
        },
        AttachCase {
            case: "a deprecated version attached ten times over",
            versions: vec![(1, Attachable), (2, Deprecated)],
            default_version: 1,
            // `Policies` is `length(1..10)` — the longest list a request may carry.
            attach: vec!["$DEFAULT", "1", "1", "1", "1", "1", "1", "1", "1", "2"],
            verdict: Verdict::NotAttachable {
                qualifier: "2",
                status: "DEPRECATED",
            },
        },
        AttachCase {
            case: "a full list of attachable versions",
            versions: vec![(1, Attachable), (2, Deprecated), (3, Attachable)],
            default_version: 3,
            attach: vec!["1", "3", "$DEFAULT", "1", "3", "$DEFAULT", "1", "3", "1", "3"],
            verdict: Verdict::Served(vec![1, 3, 3, 1, 3, 3, 1, 3, 1, 3]),
        },
    ]
}

/// The category a row uses to push its refusal past the message bound: the ARN grammar
/// bounds each of its two segments at 1023 characters, so a long category makes a long
/// `QualifiedPolicyArn` (`@length(max: 1224)`) while the policy's own `PolicyName` stays
/// inside the `@length(max: 64)` the catalog reads report it under.
fn long_category() -> String {
    "c".repeat(900)
}

/// `POST /policies/{PolicyArn}/?List` — the statuses `ListPolicyVersions` publishes,
/// keyed by version id.
async fn published_statuses(
    harness: &Harness,
    policy_arn: &str,
    case: &str,
) -> BTreeMap<i64, String> {
    let (status, body) = harness
        .call(
            "POST",
            &format!("/policies/{}/?List", encode_label(policy_arn)),
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListPolicyVersions returned {status} with {body:?}"
    );
    body["PolicyVersions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            (
                entry["VersionId"].as_i64().unwrap_or_default(),
                text(entry, "Status"),
            )
        })
        .collect()
}

/// `GET /policy-versions/{PolicyVersionArn}` — `GetPolicyVersion`.
async fn get_policy_version(harness: &Harness, version_arn: &str) -> (u16, Value) {
    harness
        .call(
            "GET",
            &format!("/policy-versions/{}", encode_label(version_arn)),
            None,
        )
        .await
}

/// `POST /resource-policies/{ResourceArn}/?List` — `ListResourcePolicies`.
async fn list_resource_policies(harness: &Harness, resource_arn: &str) -> (u16, Value) {
    harness
        .call(
            "POST",
            &format!("/resource-policies/{}/?List", encode_label(resource_arn)),
            None,
        )
        .await
}

/// `POST /GetResourcePolicy`.
async fn get_resource_policy(
    harness: &Harness,
    resource_arn: &str,
    policy_name: &str,
) -> (u16, Value) {
    harness
        .call(
            "POST",
            "/GetResourcePolicy",
            Some(json!({
                "ResourceArn": resource_arn,
                "PolicyName": policy_name,
                "PolicyType": state::PolicyType::AwsRam.as_wire(),
            })),
        )
        .await
}

/// How many teams `ListApprovalTeams` reports.
async fn team_count(harness: &Harness, case: &str) -> usize {
    let (status, body) = harness.list("").await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListApprovalTeams returned {status} with {body:?}"
    );
    body["ApprovalTeams"].as_array().map_or(0, Vec::len)
}

/// The attach gate's verdict follows the status the catalog records for the version a
/// reference resolves to: an attachable version is attached and resolves for every
/// reader, and a version in any other status is refused with `ValidationException` (400)
/// while the catalog goes on publishing it.
#[tokio::test]
async fn the_attach_gate_follows_the_recorded_status_of_the_version_a_reference_resolves_to() {
    for (index, row) in attach_cases().into_iter().enumerate() {
        let AttachCase {
            case,
            versions,
            default_version,
            attach,
            verdict,
        } = row;
        let harness = Harness::new();
        let policy_name = format!("mpa.ram_status-policy-{index}");
        let policy_arn = record_policy(
            &harness,
            CATEGORY,
            &policy_name,
            &versions,
            default_version,
        );
        let references: Vec<String> = attach
            .iter()
            .map(|qualifier| format!("{policy_arn}/{qualifier}"))
            .collect();
        let team_name = format!("attach-status-{index}");
        let (status, body) = harness
            .create(&create_body(&team_name, &references))
            .await;

        match &verdict {
            Verdict::Served(resolved) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: attaching {references:?} returned {status} with {body:?}"
                );
                let arn = text(&body, "Arn");

                // The attachments the team reports are the references it was created
                // with, and every one of them resolves for the resource-policy reads.
                let (status, team) = harness.get(&arn).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: GetApprovalTeam returned {status} with {team:?}"
                );
                let reported: Vec<String> = team["Policies"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .map(|reference| text(reference, "PolicyArn"))
                    .collect();
                assert_eq!(
                    reported, references,
                    "case {case:?}: GetApprovalTeam reports {reported:?}, not the attached \
                     {references:?}"
                );

                let (status, listed) = list_resource_policies(&harness, &arn).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: ListResourcePolicies returned {status} with {listed:?}"
                );
                let entries = listed["ResourcePolicies"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                assert_eq!(
                    entries.len(),
                    references.len(),
                    "case {case:?}: ListResourcePolicies reports {} of {} attachments: \
                     {listed:?}",
                    entries.len(),
                    references.len()
                );

                // The first attachment of this policy is the one `GetResourcePolicy`
                // answers for its `(PolicyName, PolicyType)`, and the version it reports
                // is the one that attachment resolves to.
                let (status, answered) = get_resource_policy(&harness, &arn, &policy_name).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: GetResourcePolicy returned {status} with {answered:?}"
                );
                let first = resolved
                    .first()
                    .copied()
                    .expect("a served row attaches at least one policy");
                let expected_version_arn = state::policy_version_arn(&policy_arn, first);
                assert_eq!(
                    text(&answered, "PolicyVersionArn"),
                    expected_version_arn,
                    "case {case:?}: GetResourcePolicy reports version {:?}, not \
                     {expected_version_arn:?}",
                    text(&answered, "PolicyVersionArn")
                );
            }
            Verdict::NotAttachable { qualifier, status: token } => {
                let reference = format!("{policy_arn}/{qualifier}");
                let expected = not_attachable_message(&reference, token);
                assert_eq!(
                    status, 400,
                    "case {case:?}: attaching {references:?} returned {status} with {body:?}"
                );
                assert_eq!(
                    error_code(&body),
                    "ValidationException",
                    "case {case:?}: expected error code ValidationException, got {body:?}"
                );
                assert_eq!(
                    text(&body, "Message"),
                    expected,
                    "case {case:?}: the refusal reads {:?}, not {expected:?}",
                    text(&body, "Message")
                );
            }
            Verdict::Absent { qualifier } => {
                let reference = format!("{policy_arn}/{qualifier}");
                let expected = absent_message(&reference);
                assert_eq!(
                    status, 400,
                    "case {case:?}: attaching {references:?} returned {status} with {body:?}"
                );
                assert_eq!(
                    error_code(&body),
                    "ValidationException",
                    "case {case:?}: expected error code ValidationException, got {body:?}"
                );
                assert_eq!(
                    text(&body, "Message"),
                    expected,
                    "case {case:?}: the refusal reads {:?}, not {expected:?}",
                    text(&body, "Message")
                );
            }
        }

        if !matches!(verdict, Verdict::Served(_)) {
            // A refused create stores nothing: no team reads back, the listing is empty,
            // and the name is still free.
            let (status, read) = harness.get(&expected_arn(&team_name)).await;
            assert_eq!(
                status, 404,
                "case {case:?}: a refused create left a readable team: {read:?}"
            );
            let stored = team_count(&harness, case).await;
            assert_eq!(
                stored, 0,
                "case {case:?}: the store holds {stored} team(s) after a refused create"
            );
        }

        // Whatever the verdict, the catalog still holds and publishes every version of
        // the policy: the status gates ATTACHING a version, it does not unresolve it.
        let published = published_statuses(&harness, &policy_arn, case).await;
        let expected: BTreeMap<i64, String> = versions
            .iter()
            .map(|&(version_id, status)| (i64::from(version_id), status.as_wire().to_string()))
            .collect();
        assert_eq!(
            published, expected,
            "case {case:?}: ListPolicyVersions publishes {published:?}, not {expected:?}"
        );
        for &(version_id, status) in &versions {
            let version_arn = state::policy_version_arn(&policy_arn, version_id);
            let (code, detail) = get_policy_version(&harness, &version_arn).await;
            assert_eq!(
                code, 200,
                "case {case:?}: GetPolicyVersion({version_arn}) returned {code} with {detail:?}"
            );
            assert_eq!(
                text(&detail["PolicyVersion"], "Status"),
                status.as_wire(),
                "case {case:?}: GetPolicyVersion reports {:?} for version {version_id}",
                text(&detail["PolicyVersion"], "Status")
            );
        }
    }
}

/// The refusal is a MODELLED response shape, so a reference long enough to overflow the
/// bound its own `Message` member declares is cut at that bound rather than answered
/// outside it.
#[tokio::test]
async fn the_status_refusal_keeps_its_modelled_message_bound() {
    let harness = Harness::new();
    let policy_name = "mpa.ram_long-arn-policy";
    let policy_arn = record_policy(
        &harness,
        &long_category(),
        policy_name,
        &[(1, state::PolicyStatus::Deprecated), (2, state::PolicyStatus::Attachable)],
        2,
    );
    let reference = state::policy_version_arn(&policy_arn, 1);
    let case = "a deprecated version of a policy with a long ARN";
    let unbounded = format!(
        "Policies names a policy version whose status is DEPRECATED: {reference}. Attach a \
         policy version ListPolicyVersions reports as ATTACHABLE."
    );
    assert!(
        unbounded.len() > state::MAX_EXCEPTION_MESSAGE,
        "case {case:?}: the row does not reach the bound: the sentence is {} characters",
        unbounded.len()
    );

    let (status, body) = harness
        .create(&create_body(
            "long-arn-attach",
            ::std::slice::from_ref(&reference),
        ))
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: attaching {reference:?} returned {status} with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case {case:?}: expected error code ValidationException, got {body:?}"
    );
    let message = text(&body, "Message");
    assert_eq!(
        message.len(),
        state::MAX_EXCEPTION_MESSAGE,
        "case {case:?}: the refusal is {} characters long",
        message.len()
    );
    assert_eq!(
        message,
        bounded(unbounded),
        "case {case:?}: the refusal reads {message:?}"
    );

    // The attachable version of the same policy is still attachable, however long its
    // reference is.
    let (status, created) = harness
        .create(&create_body(
            "long-arn-served",
            &[state::policy_version_arn(&policy_arn, 2)],
        ))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the attachable version was refused: {created:?}"
    );
}

/// The same rule, over every token `PolicyStatus` declares: whether a version may be
/// attached is exactly what [`state::PolicyStatus::is_attachable`] says of the status the
/// catalog records, and the token the catalog reads publish is the one recorded.
#[tokio::test]
async fn every_declared_status_token_decides_the_same_way_at_the_gate() {
    for (index, (token, status)) in status_tokens().into_iter().enumerate() {
        let harness = Harness::new();
        let policy_name = format!("mpa.ram_token-policy-{index}");
        let policy_arn = record_policy(&harness, CATEGORY, &policy_name, &[(1, status)], 1);
        let reference = state::policy_version_arn(&policy_arn, 1);

        let published = published_statuses(&harness, &policy_arn, token).await;
        assert_eq!(
            published.get(&1).map(String::as_str),
            Some(token),
            "case {token:?}: ListPolicyVersions publishes {published:?}"
        );

        let (code, body) = harness
            .create(&create_body(
                &format!("token-{index}"),
                ::std::slice::from_ref(&reference),
            ))
            .await;
        if status.is_attachable() {
            assert_eq!(
                code, 200,
                "case {token:?}: a version published as attachable was refused: {body:?}"
            );
        } else {
            assert_eq!(
                code, 400,
                "case {token:?}: a version published as {token} was attached: {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {token:?}: expected error code ValidationException, got {body:?}"
            );
            assert_eq!(
                text(&body, "Message"),
                not_attachable_message(&reference, token),
                "case {token:?}: the refusal reads {:?}",
                text(&body, "Message")
            );
        }
    }
}

/// The gate itself, over the same class: it answers with the refusal the first
/// unattachable reference earns and with nothing when every reference is attachable.
#[test]
fn the_gate_names_the_first_unattachable_reference() {
    for (token, status) in status_tokens() {
        let mut policy = state::CatalogPolicy {
            arn: state::policy_arn(CATEGORY, "mpa.ram_gate-policy"),
            name: "mpa.ram_gate-policy".to_string(),
            policy_type: state::PolicyType::AwsManaged,
            default_version: 1,
            versions: BTreeMap::new(),
        };
        policy.insert_version(state::CatalogPolicyVersion {
            version_id: 1,
            status,
            creation_time: base_instant(),
            last_updated_time: base_instant(),
            document: recorded_policy_document("mpa.ram_gate-policy", 1),
        });
        let arn = policy.arn.clone();
        let mut store = state::ComAmazonawsMpaAwsfluffycoreserviceState::default();
        store.insert_policy(policy);

        let attachable = state::policy_version_arn(&arn, 1);
        let absent = state::policy_version_arn(&arn, 2);
        let refusal = store.unattachable_policy_reference([attachable.as_str()]);
        assert_eq!(
            refusal.is_none(),
            status.is_attachable(),
            "case {token:?}: the gate answered {refusal:?} for a {token} version"
        );

        // A reference the catalog cannot resolve is refused whatever follows it, and the
        // refusal names that first reference.
        let refusal = store
            .unattachable_policy_reference([absent.as_str(), attachable.as_str()])
            .unwrap_or_else(|| panic!("case {token:?}: an absent reference must be refused"));
        assert!(
            refusal.message.contains(&absent),
            "case {token:?}: the refusal names no reference: {:?}",
            refusal.message
        );
    }
}
