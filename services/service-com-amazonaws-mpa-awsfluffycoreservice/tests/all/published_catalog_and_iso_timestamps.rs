// SPDX-License-Identifier: CC0-1.0

//! The published policy catalog, and the WIRE FORM every timestamp of this service goes
//! out in.
//!
//! Two rules, both of them the whole service's rather than one operation's:
//!
//! 1. A timestamp member whose model declares `@timestampFormat("date-time")` is
//!    serialized as an RFC 3339 / ISO-8601 date-time STRING naming the stored instant,
//!    never as the epoch-seconds number the `restJson1` protocol defaults to. This
//!    model declares exactly ONE timestamp shape, `IsoTimestamp`, and it carries that
//!    trait, so EVERY timestamp member of EVERY shape of this service is a `date-time`
//!    one — which is why the table below drives every reader that reports an instant and
//!    asserts the rule over each member it finds, wherever in the document it sits.
//!
//! 2. A catalog read reports the policy's OWN display name and the VERSION'S OWN
//!    document, both read off the record the catalog holds, never derived from the
//!    policy's ARN or from the instant the store came into being. `PolicyName` carries
//!    no pattern, so a display name is a different string from the
//!    `[a-zA-Z0-9_\.-]` name segment of `UnqualifiedPolicyArn` whenever it holds a
//!    character that segment forbids (a space, for one), and `PolicyVersion$Document` is
//!    the version's published text.
//!
//! Every request goes over the wire the real client uses: a `restJson1` HTTP request
//! dispatched through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the route, the URI bindings, the JSON envelope
//! and the HTTP status are all exercised.
//!
//! The tables span what each rule generalises over: rule 1 over every reader of this
//! service that reports an instant (a team, its approvers, an identity source, a session,
//! a deletion window, a policy version) and over both the single-resource and the paged
//! readers; rule 2 over the form of the display name (equal to the ARN segment, carrying
//! spaces, carrying punctuation no ARN segment allows, at `PolicyName`'s published
//! length bound), the version count, the default version's position and both `PolicyType`
//! tokens.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::ops::types::shared_types as model;
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_sessions::{active_team, get_session, start_baseline};
use crate::approval_team_lifecycle::{encode_label, recorded_policy_document, text, Harness};
use crate::identity_source_deletion::create_source;
use crate::policy_versions::{assert_instant, base_instant, MAX_POLICY_NAME};
use crate::session_listing::list_sessions;

/// Every member name this model gives a `date-time` timestamp. Taken from the model, not
/// from this suite's opinion: `IsoTimestamp` is the one timestamp shape it declares and it
/// carries `@timestampFormat("date-time")`, and these are the members that target it
/// (`smithy_query(query="shape",shape="GetApprovalTeamResponse,GetApprovalTeamResponseApprover,
/// PendingUpdate,GetSessionResponse,GetSessionResponseApproverResponse,ListSessionsResponseSession,
/// GetIdentitySourceResponse,CreateIdentitySourceResponse,CreateApprovalTeamResponse,
/// ListApprovalTeamsResponseApprovalTeam,StartActiveApprovalTeamDeletionResponse,PolicyVersion,
/// PolicyVersionSummary")`).
const DATE_TIME_MEMBERS: &[&str] = &[
    "CompletionTime",
    "CreationTime",
    "DeletionCompletionTime",
    "DeletionStartTime",
    "ExpirationTime",
    "InitiationTime",
    "LastActivityTime",
    "LastUpdateTime",
    "LastUpdatedTime",
    "ResponseTime",
    "UpdateInitiationTime",
];

/// `POST /policies/?List` — `ListPolicies`.
async fn list_policies(harness: &Harness) -> (u16, Value) {
    harness.call("POST", "/policies/?List", None).await
}

/// `POST /policies/{PolicyArn}/?List` — `ListPolicyVersions`.
async fn list_policy_versions(harness: &Harness, policy_arn: &str) -> (u16, Value) {
    let uri = format!("/policies/{}/?List&MaxResults=20", encode_label(policy_arn));
    harness.call("POST", &uri, None).await
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

/// Every `(path, value)` in `body` whose member name is one of [`DATE_TIME_MEMBERS`],
/// however deeply nested — a timestamp is rarely the response's own member (an approver's
/// response time, a listed session's expiration), so the whole document is walked.
fn date_time_members(path: &str, body: &Value, found: &mut Vec<(String, Value)>) {
    match body {
        Value::Object(members) => {
            for (member, value) in members {
                let child = format!("{path}.{member}");
                if DATE_TIME_MEMBERS.contains(&member.as_str()) {
                    found.push((child.clone(), value.clone()));
                }
                date_time_members(&child, value, found);
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                date_time_members(&format!("{path}[{index}]"), item, found);
            }
        }
        _ => {}
    }
}

/// Rule 1 over one response: every `date-time` member it carries is an RFC 3339 string,
/// and the members `expected` names are present (so a reader that reported no timestamp
/// at all cannot pass vacuously). Answers how many such members were checked.
fn assert_date_time_form(case: &str, body: &Value, expected: &[&str]) -> usize {
    let mut found: Vec<(String, Value)> = Vec::new();
    date_time_members("$", body, &mut found);
    for (path, value) in &found {
        let rendered = value.as_str().unwrap_or_else(|| {
            panic!(
                "case {case:?}: {path} is modelled @timestampFormat(\"date-time\"), so it must \
                 be an RFC 3339 string, got {value:?}"
            )
        });
        DateTime::from_str(rendered, Format::DateTime).unwrap_or_else(|error| {
            panic!("case {case:?}: {path} is not an RFC 3339 date-time: {rendered:?} ({error})")
        });
        assert!(
            rendered.contains('T') && rendered.ends_with('Z'),
            "case {case:?}: {path} must be a UTC date-time string, got {rendered:?}"
        );
    }
    for member in expected {
        assert!(
            found
                .iter()
                .any(|(path, _)| path.ends_with(&format!(".{member}"))),
            "case {case:?}: no {member} in {body:?}"
        );
    }
    found.len()
}

/// The catalog as the store holds it, in ARN order — the record every assertion below is
/// made against.
fn stored_catalog(harness: &Harness) -> Vec<state::CatalogPolicy> {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let guard = store.lock();
    guard.policies.values().cloned().collect()
}

/// The catalog a FRESH store publishes, in ARN order: the seed alone, without the entries
/// the harness records for the attachment tables.
fn published_catalog() -> Vec<state::CatalogPolicy> {
    state::ComAmazonawsMpaAwsfluffycoreserviceState::default()
        .policies
        .values()
        .cloned()
        .collect()
}

/// RULE 1: every `date-time` member of every reader of this service is an RFC 3339
/// string. The rows are the readers themselves, each driven end to end.
#[tokio::test]
async fn every_date_time_member_is_an_rfc3339_string_in_every_reader() {
    let harness = Harness::new();
    let mut checked = 0usize;

    // A team, read back through each of its three readers. `active_team` waits out the
    // clock-owed invitation responses, so each approver carries a ResponseTime and a
    // LastActivityTime too.
    let team_arn = active_team(&harness, "iso-timestamps", 2, 1).await;
    let (status, team) = harness.get(&team_arn).await;
    assert_eq!(status, 200, "GetApprovalTeam returned {status}: {team:?}");
    checked += assert_date_time_form(
        "GetApprovalTeam",
        &team,
        // A team never updated reports no LastUpdateTime, so the members named here are
        // the ones every ACTIVE team carries.
        &["CreationTime", "ResponseTime", "LastActivityTime"],
    );

    let (status, listed) = harness.list("MaxResults=20").await;
    assert_eq!(status, 200, "ListApprovalTeams returned {status}: {listed:?}");
    checked += assert_date_time_form("ListApprovalTeams", &listed, &["CreationTime"]);

    // An identity source: created, then read singly and in a page.
    let source_arn = create_source(&harness, 1, 0).await;
    let (status, source) = harness
        .call("GET", &format!("/identity-sources/{}", encode_label(&source_arn)), None)
        .await;
    assert_eq!(status, 200, "GetIdentitySource returned {status}: {source:?}");
    checked += assert_date_time_form("GetIdentitySource", &source, &["CreationTime"]);

    let (status, sources) = harness.call("POST", "/identity-sources/?List", None).await;
    assert_eq!(status, 200, "ListIdentitySources returned {status}: {sources:?}");
    checked += assert_date_time_form("ListIdentitySources", &sources, &["CreationTime"]);

    // A session, read singly and in a page.
    let (status, started) = start_baseline(&harness, &team_arn, None).await;
    assert_eq!(status, 200, "StartApprovalTeamBaseline returned {status}: {started:?}");
    let session_arn = text(&started, "BaselineSessionArn");
    let (status, session) = get_session(&harness, &session_arn).await;
    assert_eq!(status, 200, "GetSession returned {status}: {session:?}");
    checked += assert_date_time_form("GetSession", &session, &["InitiationTime", "ExpirationTime"]);

    let (status, sessions) = list_sessions(&harness, &team_arn, json!({ "MaxResults": 20 })).await;
    assert_eq!(status, 200, "ListSessions returned {status}: {sessions:?}");
    checked += assert_date_time_form(
        "ListSessions",
        &sessions,
        &["InitiationTime", "ExpirationTime"],
    );

    // The catalog readers, over every entry and version a fresh store publishes.
    let catalog = published_catalog();
    assert!(!catalog.is_empty(), "a fresh store must seed the catalog");
    let (status, policies) = list_policies(&harness).await;
    assert_eq!(status, 200, "ListPolicies returned {status}: {policies:?}");
    checked += assert_date_time_form("ListPolicies", &policies, &[]);
    for policy in &catalog {
        let (status, versions) = list_policy_versions(&harness, &policy.arn).await;
        assert_eq!(status, 200, "ListPolicyVersions returned {status}: {versions:?}");
        checked += assert_date_time_form(
            "ListPolicyVersions",
            &versions,
            &["CreationTime", "LastUpdatedTime"],
        );
        for version in policy.versions.values() {
            let arn = state::policy_version_arn(&policy.arn, version.version_id);
            let (status, detail) = get_policy_version(&harness, &arn).await;
            assert_eq!(status, 200, "GetPolicyVersion returned {status}: {detail:?}");
            checked += assert_date_time_form(
                "GetPolicyVersion",
                &detail,
                &["CreationTime", "LastUpdatedTime"],
            );
        }
    }

    // The deletion window, last: it moves the team out of ACTIVE.
    let (status, deletion) = harness.start_deletion(&team_arn, None).await;
    assert_eq!(
        status, 200,
        "StartActiveApprovalTeamDeletion returned {status}: {deletion:?}"
    );
    checked += assert_date_time_form(
        "StartActiveApprovalTeamDeletion",
        &deletion,
        &["DeletionStartTime", "DeletionCompletionTime"],
    );

    assert!(
        checked >= 15,
        "the readers above must between them report more than {checked} timestamp members"
    );
}

/// One row of the catalog table: a policy, the form of its display name, and the versions
/// it holds.
struct CatalogCase {
    case: &'static str,
    category: &'static str,
    /// The ARN's name segment, inside `[a-zA-Z0-9_\.-]`.
    arn_name: &'static str,
    /// The policy's display name, which `PolicyName` constrains only in length.
    display_name: String,
    policy_type: state::PolicyType,
    /// `(version id, status)`, in no particular order.
    versions: Vec<(i32, state::PolicyStatus)>,
    default_version: i32,
}

/// The catalog table: rows differ along the form of the display name (equal to the ARN
/// segment, carrying spaces, carrying punctuation no ARN segment allows, at `PolicyName`'s
/// `length(max: 64)`), the version count, the default version's position, version-id
/// spacing, both `PolicyType` tokens and both `PolicyStatus` tokens.
fn catalog_cases() -> Vec<CatalogCase> {
    vec![
        CatalogCase {
            case: "a display name carrying spaces, one version",
            category: "IdentityCenter",
            arn_name: "VaultProtectionPolicy",
            display_name: "Vault Protection Policy".to_string(),
            policy_type: state::PolicyType::AwsManaged,
            versions: vec![(1, state::PolicyStatus::Attachable)],
            default_version: 1,
        },
        CatalogCase {
            case: "a display name no part of the ARN carries",
            category: "Organizations",
            arn_name: "ops.break-glass_01",
            display_name: "Break-glass review (ops)".to_string(),
            policy_type: state::PolicyType::AwsRam,
            versions: vec![
                (1, state::PolicyStatus::Deprecated),
                (2, state::PolicyStatus::Attachable),
            ],
            default_version: 2,
        },
        CatalogCase {
            case: "the maximum-length display name, three versions, a middle default",
            category: "ResourceAccessManager",
            arn_name: "MaxNamePolicy",
            // `PolicyName` is `length(max: 64)`; the spaces are what no ARN segment holds.
            display_name: format!("{} Policy", "A".repeat(MAX_POLICY_NAME - 7)),
            policy_type: state::PolicyType::AwsManaged,
            versions: vec![
                (1, state::PolicyStatus::Deprecated),
                (7, state::PolicyStatus::Attachable),
                (42, state::PolicyStatus::Attachable),
            ],
            default_version: 7,
        },
        CatalogCase {
            case: "a display name equal to the ARN's name segment",
            category: "Ec2",
            arn_name: "SameNamePolicy",
            display_name: "SameNamePolicy".to_string(),
            policy_type: state::PolicyType::AwsRam,
            versions: vec![(3, state::PolicyStatus::Attachable)],
            default_version: 3,
        },
    ]
}

/// Record `case` in the catalog and answer its unqualified ARN. No modelled operation
/// mints a policy — the catalog is the service's own — so a table that needs one writes it
/// directly, the way `policy_versions.rs` does. Each version carries its own document and
/// its own pair of instants, so no assertion below can pass by reporting another version's.
fn record_policy(harness: &Harness, case: &CatalogCase) -> String {
    let mut policy = state::CatalogPolicy {
        arn: state::policy_arn(case.category, case.arn_name),
        name: case.display_name.clone(),
        policy_type: case.policy_type,
        default_version: case.default_version,
        versions: std::collections::BTreeMap::new(),
    };
    for &(version_id, status) in &case.versions {
        let created =
            base_instant() + Duration::from_secs(u64::from(version_id.unsigned_abs()) * 60);
        policy.insert_version(state::CatalogPolicyVersion {
            version_id,
            status,
            creation_time: created,
            last_updated_time: created + Duration::from_secs(30),
            document: recorded_policy_document(case.arn_name, version_id),
        });
    }
    let arn = policy.arn.clone();
    let store = state::service_state(&*harness.engine).expect("the service store");
    store.lock().insert_policy(policy);
    arn
}

/// RULE 2: every catalog reader reports the policy's own display name and the version's
/// own document, whatever the ARN says.
#[tokio::test]
async fn a_catalog_read_reports_the_entrys_own_name_and_the_versions_own_document() {
    let harness = Harness::without_organization();

    for case in catalog_cases() {
        let policy_arn = record_policy(&harness, &case);
        let stored = stored_catalog(&harness)
            .into_iter()
            .find(|policy| policy.arn == policy_arn)
            .expect("the recorded policy");

        // `ListPolicies`: the entry's Name is the policy's own.
        let (status, policies) = list_policies(&harness).await;
        assert_eq!(
            status, 200,
            "case {:?}: ListPolicies returned {status}: {policies:?}",
            case.case
        );
        let listed_policy = policies["Policies"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|entry| text(entry, "Arn") == policy_arn)
            .unwrap_or_else(|| panic!("case {:?}: {policy_arn} is not listed", case.case));
        assert_eq!(
            text(&listed_policy, "Name"),
            case.display_name,
            "case {:?}: ListPolicies must report the policy's display name {:?}, got {listed_policy:?}",
            case.case,
            case.display_name
        );

        // `ListPolicyVersions`: each summary's Name is the policy's own and each pair of
        // instants is the version's own.
        let (status, versions) = list_policy_versions(&harness, &policy_arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: ListPolicyVersions returned {status}: {versions:?}",
            case.case
        );
        let summaries = versions["PolicyVersions"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            summaries.len(),
            stored.versions.len(),
            "case {:?}: ListPolicyVersions reports {} of {} versions: {versions:?}",
            case.case,
            summaries.len(),
            stored.versions.len()
        );
        let mut documents: BTreeSet<String> = BTreeSet::new();
        for version in stored.versions.values() {
            let version_arn = state::policy_version_arn(&policy_arn, version.version_id);
            let summary = summaries
                .iter()
                .find(|entry| text(entry, "Arn") == version_arn)
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: no summary for {version_arn}: {versions:?}",
                        case.case
                    )
                });
            let row = format!("{} (version {})", case.case, version.version_id);
            assert_eq!(
                text(summary, "Name"),
                case.display_name,
                "case {row:?}: expected the display name {:?}, got {summary:?}",
                case.display_name
            );
            if case.display_name != case.arn_name {
                assert_ne!(
                    text(summary, "Name"),
                    case.arn_name,
                    "case {row:?}: Name must not be the ARN's name segment: {summary:?}"
                );
            }
            assert_instant(&row, summary, "CreationTime", version.creation_time);
            assert_instant(&row, summary, "LastUpdatedTime", version.last_updated_time);

            // `GetPolicyVersion`: the same name, and the document the version holds.
            let (status, detail) = get_policy_version(&harness, &version_arn).await;
            assert_eq!(
                status, 200,
                "case {row:?}: GetPolicyVersion returned {status}: {detail:?}"
            );
            let reported = detail["PolicyVersion"].clone();
            assert_eq!(
                text(&reported, "Name"),
                case.display_name,
                "case {row:?}: expected the display name {:?}, got {reported:?}",
                case.display_name
            );
            assert_eq!(
                text(&reported, "Document"),
                version.document,
                "case {row:?}: expected the document the catalog holds for this version, \
                 got {reported:?}"
            );
            assert_instant(&row, &reported, "CreationTime", version.creation_time);
            assert_instant(&row, &reported, "LastUpdatedTime", version.last_updated_time);
            assert!(
                model::POLICY_STATUS_VALUES.contains(&text(&reported, "Status").as_str()),
                "case {row:?}: Status is not a token the model declares: {reported:?}"
            );
            documents.insert(text(&reported, "Document"));
        }
        // A document belongs to the VERSION, so two versions of one policy do not share
        // one: a reader that answered with the policy's (or a synthesized) document would
        // report the same text twice.
        assert_eq!(
            documents.len(),
            stored.versions.len(),
            "case {:?}: {} versions reported {} distinct documents",
            case.case,
            stored.versions.len(),
            documents.len()
        );
    }
}

/// RULE 2, for the entries the catalog PUBLISHES: their identity and their instants are
/// the catalog's own, so two stores answer alike and neither reports the instant it came
/// into being.
#[tokio::test]
async fn the_published_catalog_is_the_same_in_every_store() {
    let before_any_store = SystemTime::now();
    let first = Harness::without_organization();
    // A second catalog built after the first: a seeded instant taken from the clock would
    // differ between the two.
    std::thread::sleep(Duration::from_millis(20));
    let second = Harness::without_organization();

    let published = published_catalog();
    assert!(
        !published.is_empty(),
        "a fresh store must seed the published catalog"
    );
    assert_eq!(
        format!("{published:?}"),
        format!("{:?}", published_catalog()),
        "two stores must hold the same published catalog"
    );
    let held = stored_catalog(&first);
    for policy in &published {
        assert!(
            held.iter().any(|entry| format!("{entry:?}") == format!("{policy:?}")),
            "a mounted store must hold the published entry {policy:?}"
        );
    }

    for policy in &published {
        let case = policy.arn.clone();
        assert!(
            !policy.name.is_empty() && policy.name.len() <= MAX_POLICY_NAME,
            "case {case:?}: the display name {:?} is outside PolicyName's length(max: {MAX_POLICY_NAME})",
            policy.name
        );
        for version in policy.versions.values() {
            let row = format!("{case} (version {})", version.version_id);
            assert!(
                version.creation_time < before_any_store,
                "case {row:?}: a published version's CreationTime is the catalog's own, so it \
                 cannot be the instant this store came into being"
            );
            assert!(
                version.last_updated_time >= version.creation_time,
                "case {row:?}: LastUpdatedTime must not precede CreationTime"
            );

            // The document the catalog publishes: a real IAM policy document that grants
            // the named session actions, never a service-wide wildcard.
            let document: Value = serde_json::from_str(&version.document).unwrap_or_else(|error| {
                panic!("case {row:?}: the published document is not JSON: {error}")
            });
            let statements = document["Statement"].as_array().cloned().unwrap_or_default();
            assert!(
                !statements.is_empty(),
                "case {row:?}: the published document carries no statement: {document:?}"
            );
            for statement in &statements {
                let actions: Vec<String> = match &statement["Action"] {
                    Value::String(one) => vec![one.clone()],
                    Value::Array(many) => many
                        .iter()
                        .map(|action| action.as_str().unwrap_or_default().to_string())
                        .collect(),
                    other => panic!("case {row:?}: a statement has no Action: {other:?}"),
                };
                assert!(
                    !actions.is_empty(),
                    "case {row:?}: a statement grants no action: {statement:?}"
                );
                for action in &actions {
                    assert!(
                        action.starts_with(&format!("{}:", state::SERVICE_ACTION_PREFIX))
                            && !action.ends_with('*'),
                        "case {row:?}: the published document must grant named {} actions, \
                         got {action:?}",
                        state::SERVICE_ACTION_PREFIX
                    );
                }
                assert!(
                    statement["Condition"].is_object(),
                    "case {row:?}: the published statement carries no condition: {statement:?}"
                );
            }

            // And the wire reports exactly that record.
            let version_arn = state::policy_version_arn(&policy.arn, version.version_id);
            let (status, detail) = get_policy_version(&first, &version_arn).await;
            assert_eq!(
                status, 200,
                "case {row:?}: GetPolicyVersion returned {status}: {detail:?}"
            );
            let reported = detail["PolicyVersion"].clone();
            assert_eq!(
                text(&reported, "Name"),
                policy.name,
                "case {row:?}: expected the published display name {:?}, got {reported:?}",
                policy.name
            );
            assert_eq!(
                text(&reported, "Document"),
                version.document,
                "case {row:?}: expected the published document, got {reported:?}"
            );
            assert_instant(&row, &reported, "CreationTime", version.creation_time);
            assert_instant(&row, &reported, "LastUpdatedTime", version.last_updated_time);

            // The same version read through the other store answers the same, so nothing
            // about a published entry depends on the store that serves it.
            let (status, again) = get_policy_version(&second, &version_arn).await;
            assert_eq!(
                status, 200,
                "case {row:?}: the second store returned {status}: {again:?}"
            );
            assert_eq!(
                again, detail,
                "case {row:?}: two stores must report one published version alike"
            );
        }
    }
}
