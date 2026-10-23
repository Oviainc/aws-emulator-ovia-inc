// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetResourcePolicy`, the read that resolves one policy
//! attached to a resource.
//!
//! The rule under test: `GetResourcePolicy` answers the `(ResourceArn, PolicyName,
//! PolicyType)` triple a request names from the policies RECORDED on that resource —
//! an approval team's `Policies` references are its attached resource policies,
//! resolved through the policy catalog, with `$DEFAULT` resolving to the policy's
//! current default version — and refuses every triple that names no recorded
//! attachment with `ResourceNotFoundException` (404), a `ResourceArn` that is not a
//! well-formed ARN with `InvalidParameterException` (400) and an ARN outside the
//! caller's account with `AccessDeniedException` (403), the last decided before the
//! resource is resolved.
//!
//! The tables below span the class each rule generalises over rather than one recorded
//! shape: the attachment table varies the catalog entry (including the one a fresh
//! store seeds, which this issue never cited), its `PolicyType`, its version count and
//! which version is the default, and the form of the reference the team attaches
//! (`$DEFAULT` or an explicit version id); the not-found table varies WHY the triple
//! resolves to nothing (unknown name, wrong case, wrong type, wrong resource, unknown
//! resource, an attachment the catalog does not hold); the malformed table varies which
//! field of the ARN grammar is missing or empty.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the route, the JSON envelope and the HTTP
//! status are all exercised.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::ops::types::shared_types as model;
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_team_lifecycle::{error_code, expected_arn, text, Harness, REGION};
use crate::authorization_gate::expected_denial;

/// The policy a fresh store seeds, from `state::seeded_policy_catalog` — the entry
/// every table below includes so that one row is a record no test wrote.
const SEEDED_POLICY: (&str, &str) = ("IdentityCenter", "MPAApprovalTeamPolicy");

/// One catalog entry a row of the attachment table reads back through.
struct PolicyCase {
    case: &'static str,
    /// Category and name segments of the policy's ARN.
    category: &'static str,
    name: &'static str,
    policy_type: state::PolicyType,
    /// Every version id the catalog entry holds.
    versions: Vec<i32>,
    /// Which of them is the entry's default.
    default_version: i32,
    /// Whether the team attaches `$DEFAULT` (true) or `explicit_version` (false).
    attach_default: bool,
    /// The version id the team attaches when it does not attach `$DEFAULT`.
    explicit_version: i32,
}

impl PolicyCase {
    /// The version id the attachment resolves to: the explicit one, or the entry's
    /// default when the reference names `$DEFAULT`.
    fn expected_version(&self) -> i32 {
        if self.attach_default {
            self.default_version
        } else {
            self.explicit_version
        }
    }
}

/// The attachment table. Rows differ along the catalog entry (the seeded one and
/// entries recorded for the test), its `PolicyType`, how many versions it holds, which
/// one is its default, the form of the reference the team attaches, and the characters
/// and length of the policy's name (`[a-zA-Z0-9_\.-]`, the set `UnqualifiedPolicyArn`'s
/// pattern allows, up to `PolicyName`'s `@length(max: 64)`).
fn policy_cases() -> Vec<PolicyCase> {
    vec![
        PolicyCase {
            case: "the seeded policy, attached as $DEFAULT",
            category: SEEDED_POLICY.0,
            name: SEEDED_POLICY.1,
            policy_type: state::PolicyType::AwsManaged,
            versions: vec![1],
            default_version: 1,
            attach_default: true,
            explicit_version: 1,
        },
        PolicyCase {
            case: "an entry whose default is not its newest version, attached as $DEFAULT",
            category: "IdentityCenter",
            name: "MPAApprovalTeamFullAccess",
            policy_type: state::PolicyType::AwsManaged,
            versions: vec![1, 2, 3],
            default_version: 2,
            attach_default: true,
            explicit_version: 3,
        },
        PolicyCase {
            case: "an older version attached by its id, not the default",
            category: "IdentityCenter",
            name: "MPAApprovalTeamReadOnly",
            policy_type: state::PolicyType::AwsManaged,
            versions: vec![1, 2, 3],
            default_version: 3,
            attach_default: false,
            explicit_version: 1,
        },
        PolicyCase {
            case: "an AWS_RAM policy, attached by a multi-digit version id",
            category: "ResourceAccessManager",
            name: "mpa.ram_team-policy",
            policy_type: state::PolicyType::AwsRam,
            versions: vec![1, 42, 142],
            default_version: 1,
            attach_default: false,
            explicit_version: 142,
        },
        PolicyCase {
            case: "a name at PolicyName's modelled length bound",
            category: "IdentityCenter",
            // `PolicyName` is `@length(max: 64)`.
            name: "nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn",
            policy_type: state::PolicyType::AwsManaged,
            versions: vec![1],
            default_version: 1,
            attach_default: true,
            explicit_version: 1,
        },
    ]
}

/// A fixed instant the recorded versions are dated from, so no assertion depends on
/// when the test ran.
fn base_instant() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

/// Record a catalog entry for `case` and answer its unqualified ARN.
///
/// No modelled operation mints a policy or one of its versions — the catalog is AWS's —
/// so a table that needs an entry or a version history writes it directly, the way
/// `record_management_account` records an organization. The seeded entry is recorded
/// again with the shape it already has, so every row goes through one code path.
fn record_policy(harness: &Harness, case: &PolicyCase) -> String {
    let mut versions = BTreeMap::new();
    for (index, &version_id) in case.versions.iter().enumerate() {
        let created = base_instant() + Duration::from_secs(60 * index as u64);
        versions.insert(
            version_id,
            state::CatalogPolicyVersion {
                version_id,
                status: state::PolicyStatus::Attachable,
                creation_time: created,
                last_updated_time: created,
            },
        );
    }
    let policy = state::CatalogPolicy {
        arn: state::policy_arn(case.category, case.name),
        name: case.name.to_string(),
        policy_type: case.policy_type,
        default_version: case.default_version,
        versions,
    };
    let arn = policy.arn.clone();
    let store = state::service_state(&*harness.engine).expect("the service store");
    store.lock().insert_policy(policy);
    arn
}

/// The reference a team attaches for `case`: its policy ARN qualified by `$DEFAULT` or
/// by an explicit version id, the two forms `QualifiedPolicyArn` allows.
fn attachment(case: &PolicyCase) -> String {
    let policy_arn = state::policy_arn(case.category, case.name);
    if case.attach_default {
        format!("{policy_arn}/$DEFAULT")
    } else {
        state::policy_version_arn(&policy_arn, case.explicit_version)
    }
}

/// A create body the modelled constraints accept, carrying `policies` as its
/// `Policies` references. `Policies` is `length(1..10)`.
fn create_body(name: &str, policies: &[String]) -> Value {
    json!({
        "Name": name,
        "Description": "resource policies",
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

/// One `GetResourcePolicy` call, over the wire.
async fn get_resource_policy(
    harness: &Harness,
    resource_arn: &str,
    policy_name: &str,
    policy_type: &str,
) -> (u16, Value) {
    harness
        .call(
            "POST",
            "/GetResourcePolicy",
            Some(json!({
                "ResourceArn": resource_arn,
                "PolicyName": policy_name,
                "PolicyType": policy_type,
            })),
        )
        .await
}

/// A team attaching one reference per row of [`policy_cases`], and its ARN.
async fn team_with_every_policy(harness: &Harness, name: &str) -> String {
    let mut policies = Vec::new();
    for case in &policy_cases() {
        record_policy(harness, case);
        policies.push(attachment(case));
    }
    let (status, created) = harness.create(&create_body(name, &policies)).await;
    assert_eq!(status, 200, "create returned {created:?}");
    let arn = text(&created, "Arn");
    assert_eq!(arn, expected_arn(name), "create minted the wrong Arn");
    arn
}

/// Every policy attached to a resource reads back by name and type, reporting the
/// catalog entry's own members, the version in effect and that version's document.
#[tokio::test]
async fn every_attached_policy_reads_back_by_name_and_type() {
    let harness = Harness::new();
    let team_arn = team_with_every_policy(&harness, "policy-reader").await;

    // The attachments this read answers from are the ones the team itself reports, so
    // the two readers never disagree about what is attached.
    let (status, team) = harness.get(&team_arn).await;
    assert_eq!(status, 200, "the created team reads back as {team:?}");
    let attached: Vec<String> = team
        .get("Policies")
        .and_then(Value::as_array)
        .expect("GetApprovalTeam reports Policies")
        .iter()
        .map(|reference| text(reference, "PolicyArn"))
        .collect();
    let expected: Vec<String> = policy_cases().iter().map(attachment).collect();
    assert_eq!(
        attached, expected,
        "GetApprovalTeam reports attachments {attached:?}, not the created {expected:?}"
    );

    let mut documents: Vec<String> = Vec::new();
    for case in policy_cases() {
        let PolicyCase {
            case: name_of_case,
            category,
            name,
            policy_type,
            ..
        } = &case;
        let (status, body) =
            get_resource_policy(&harness, &team_arn, name, policy_type.as_wire()).await;
        assert_eq!(
            status, 200,
            "case {name_of_case:?}: GetResourcePolicy returned {body:?}"
        );
        assert_eq!(
            text(&body, "ResourceArn"),
            team_arn,
            "case {name_of_case:?}: wrong ResourceArn in {body:?}"
        );
        assert_eq!(
            text(&body, "PolicyName"),
            *name,
            "case {name_of_case:?}: wrong PolicyName in {body:?}"
        );
        assert!(
            name.len() <= 64,
            "case {name_of_case:?}: the reported PolicyName is {} characters, past the \
             modelled bound",
            name.len()
        );
        assert_eq!(
            text(&body, "PolicyType"),
            policy_type.as_wire(),
            "case {name_of_case:?}: wrong PolicyType in {body:?}"
        );
        assert!(
            model::POLICY_TYPE_VALUES.contains(&text(&body, "PolicyType").as_str()),
            "case {name_of_case:?}: PolicyType is not a token the model declares: {body:?}"
        );
        // The version in EFFECT: the attached version id, or the entry's current
        // default when the attachment names `$DEFAULT`.
        assert_eq!(
            text(&body, "PolicyVersionArn"),
            state::policy_version_arn(
                &state::policy_arn(category, name),
                case.expected_version()
            ),
            "case {name_of_case:?}: PolicyVersionArn is not the version in effect in {body:?}"
        );

        // The document is a document: valid JSON, inside the member's modelled bound,
        // and the catalog version's own (two reads agree).
        let document = text(&body, "PolicyDocument");
        let parsed: Value = serde_json::from_str(&document).unwrap_or_else(|e| {
            panic!("case {name_of_case:?}: PolicyDocument is not JSON: {e} ({document:?})")
        });
        assert!(
            parsed["Statement"][0]["Effect"].is_string(),
            "case {name_of_case:?}: the document carries no statement: {parsed:?}"
        );
        assert!(
            document.len() <= 400_000,
            "case {name_of_case:?}: PolicyDocument is {} bytes, past the modelled bound",
            document.len()
        );
        let (status, again) =
            get_resource_policy(&harness, &team_arn, name, policy_type.as_wire()).await;
        assert_eq!(
            status, 200,
            "case {name_of_case:?}: the second read returned {again:?}"
        );
        assert_eq!(
            text(&again, "PolicyDocument"),
            document,
            "case {name_of_case:?}: two reads of one attachment report different documents"
        );

        documents.push(document);
    }

    let distinct: std::collections::BTreeSet<&String> = documents.iter().collect();
    assert_eq!(
        distinct.len(),
        documents.len(),
        "two different policy versions report the same document: {documents:?}"
    );

    // The reads changed nothing a later caller sees: the team is exactly as created.
    let (status, after) = harness.get(&team_arn).await;
    assert_eq!(status, 200, "the team is gone after the reads: {after:?}");
    assert_eq!(
        text(&after, "VersionId"),
        text(&team, "VersionId"),
        "a read bumped the team's version"
    );
    assert_eq!(
        after.get("Policies"),
        team.get("Policies"),
        "a read changed the team's attachments"
    );
}

/// An attachment whose default version MOVES is reported against the catalog, not
/// against a copy taken when the team was created: the same team and the same triple
/// report the policy's new default version once the catalog records it.
#[tokio::test]
async fn an_attachment_of_default_follows_the_catalogs_default_version() {
    let harness = Harness::new();
    let case = PolicyCase {
        case: "default version moved",
        category: "IdentityCenter",
        name: "MPAApprovalTeamPolicy",
        policy_type: state::PolicyType::AwsManaged,
        versions: vec![1, 2],
        default_version: 1,
        attach_default: true,
        explicit_version: 1,
    };
    let policy_arn = record_policy(&harness, &case);
    let (status, created) = harness
        .create(&create_body("default-follower", &[attachment(&case)]))
        .await;
    assert_eq!(status, 200, "create returned {created:?}");
    let team_arn = text(&created, "Arn");

    for default_version in [1, 2] {
        {
            let store = state::service_state(&*harness.engine).expect("the service store");
            let mut guard = store.lock();
            let policy = guard
                .policies
                .get_mut(&policy_arn)
                .expect("the recorded policy");
            policy.default_version = default_version;
        }
        let (status, body) =
            get_resource_policy(&harness, &team_arn, case.name, "AWS_MANAGED").await;
        assert_eq!(
            status, 200,
            "default {default_version}: GetResourcePolicy returned {body:?}"
        );
        assert_eq!(
            text(&body, "PolicyVersionArn"),
            state::policy_version_arn(&policy_arn, default_version),
            "default {default_version}: the read did not follow the catalog's default \
             version: {body:?}"
        );
    }
}

/// A triple that names no recorded attachment is `ResourceNotFoundException` (404),
/// whichever of its three fields is the one that resolves to nothing.
#[tokio::test]
async fn a_triple_that_names_no_attachment_is_not_found() {
    let harness = Harness::new();
    let team_arn = team_with_every_policy(&harness, "policy-reader").await;
    // A second team attaching only the seeded policy, so a row can ask it for a policy
    // that exists in the catalog but is attached to the other team. Its second
    // reference names a policy the catalog does NOT hold, which is reportable by
    // nothing.
    let uncatalogued = "arn:aws:mpa:::aws:policy/IdentityCenter/NotInTheCatalog/$DEFAULT";
    let (status, created) = harness
        .create(&create_body(
            "other-team",
            &[
                format!(
                    "{}/$DEFAULT",
                    state::policy_arn(SEEDED_POLICY.0, SEEDED_POLICY.1)
                ),
                uncatalogued.to_string(),
            ],
        ))
        .await;
    assert_eq!(status, 200, "create returned {created:?}");
    let other_arn = text(&created, "Arn");

    // (case, resource ARN, policy name, policy type)
    let rows: Vec<(&str, String, String, &str)> = vec![
        (
            "a policy name no attachment carries",
            team_arn.clone(),
            "ThisPolicyDoesNotExist".to_string(),
            "AWS_MANAGED",
        ),
        (
            "an attached name in the wrong case",
            team_arn.clone(),
            "mpaapprovalteampolicy".to_string(),
            "AWS_MANAGED",
        ),
        (
            "an attached name with the other policy type",
            team_arn.clone(),
            SEEDED_POLICY.1.to_string(),
            "AWS_RAM",
        ),
        (
            "the policy's category instead of its name",
            team_arn.clone(),
            SEEDED_POLICY.0.to_string(),
            "AWS_MANAGED",
        ),
        (
            "the policy's whole ARN instead of its name",
            team_arn.clone(),
            format!("{}/$DEFAULT", state::policy_arn(SEEDED_POLICY.0, SEEDED_POLICY.1)),
            "AWS_MANAGED",
        ),
        (
            "a name attached to another resource, not this one",
            other_arn.clone(),
            "MPAApprovalTeamReadOnly".to_string(),
            "AWS_MANAGED",
        ),
        (
            "an attachment the catalog does not hold",
            other_arn.clone(),
            "NotInTheCatalog".to_string(),
            "AWS_MANAGED",
        ),
        (
            "a well-formed team ARN no team carries",
            expected_arn("never-created"),
            SEEDED_POLICY.1.to_string(),
            "AWS_MANAGED",
        ),
        (
            "an ARN of another resource kind in the caller's account",
            format!(
                "arn:aws:mpa:{REGION}:{}:identity-source/ssoins-0001",
                engine::emulator_account_id()
            ),
            SEEDED_POLICY.1.to_string(),
            "AWS_MANAGED",
        ),
        (
            "an empty policy name",
            team_arn.clone(),
            String::new(),
            "AWS_MANAGED",
        ),
    ];

    for (case, resource_arn, policy_name, policy_type) in rows {
        let (status, body) =
            get_resource_policy(&harness, &resource_arn, &policy_name, policy_type).await;
        assert_eq!(status, 404, "case {case:?}: expected HTTP 404, got {status} with {body:?}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );
        // The refusal is built from the request: the triple that resolved to nothing.
        // The 404 and the error code are the requirement; the text is this service's.
        assert_eq!(
            text(&body, "Message"),
            format!(
                "No {policy_type} policy named '{policy_name}' is attached to resource \
                 '{resource_arn}'."
            ),
            "case {case:?}: wrong refusal message in {body:?}"
        );
    }

    // Nothing a refused read touched changed: both teams are still there.
    for arn in [&team_arn, &other_arn] {
        let (status, team) = harness.get(arn).await;
        assert_eq!(status, 200, "team {arn} is gone after the refusals: {team:?}");
    }
}

/// A `ResourceArn` that is not a well-formed ARN is `InvalidParameterException` (400):
/// the member carries no modelled pattern, so this operation is where the value is
/// refused.
#[tokio::test]
async fn a_resource_arn_that_is_not_an_arn_is_an_invalid_parameter() {
    let harness = Harness::new();
    let team_arn = team_with_every_policy(&harness, "policy-reader").await;

    // Rows differ along which part of `arn:<partition>:<service>:<region>:<account>:
    // <resource>` is missing or empty.
    let rows: Vec<(&str, String)> = vec![
        ("an empty value", String::new()),
        ("not an ARN at all", "not-an-arn-at-all".to_string()),
        ("the prefix alone", "arn".to_string()),
        ("a partition but nothing else", "arn:aws".to_string()),
        (
            "five fields, no resource field",
            format!("arn:aws:mpa:{REGION}:{}", engine::emulator_account_id()),
        ),
        (
            "an empty resource field",
            format!("arn:aws:mpa:{REGION}:{}:", engine::emulator_account_id()),
        ),
        (
            "an empty partition",
            format!(
                "arn::mpa:{REGION}:{}:approval-team/t",
                engine::emulator_account_id()
            ),
        ),
        (
            "an empty service",
            format!(
                "arn:aws::{REGION}:{}:approval-team/t",
                engine::emulator_account_id()
            ),
        ),
        (
            "the prefix in the wrong case",
            format!(
                "ARN:aws:mpa:{REGION}:{}:approval-team/t",
                engine::emulator_account_id()
            ),
        ),
        ("a team ARN with its prefix dropped", team_arn["arn:".len()..].to_string()),
    ];

    for (case, resource_arn) in rows {
        let (status, body) =
            get_resource_policy(&harness, &resource_arn, SEEDED_POLICY.1, "AWS_MANAGED").await;
        assert_eq!(status, 400, "case {case:?}: expected HTTP 400, got {status} with {body:?}");
        assert_eq!(
            error_code(&body),
            "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {body:?}"
        );
    }
}

/// A `ResourceArn` outside the caller's own account is denied before the resource is
/// resolved — the same rule, and the same refusal text, as every other operation of
/// this service that identifies its resource by ARN.
#[tokio::test]
async fn a_resource_arn_outside_the_callers_account_is_denied() {
    let harness = Harness::new();
    let team_arn = team_with_every_policy(&harness, "policy-reader").await;

    // Rows differ along the account field — the one thing the rule keys on — plus the
    // region, the partition, and whether the team name is one the caller DOES own.
    let rows: Vec<(&str, String)> = vec![
        (
            "another account, same region",
            format!("arn:aws:mpa:{REGION}:999999999999:approval-team/probe-team"),
        ),
        (
            "another account, another region",
            "arn:aws:mpa:eu-west-1:210987654321:approval-team/probe-team".to_string(),
        ),
        (
            "an account one digit from the caller's",
            format!("arn:aws:mpa:{REGION}:000000000001:approval-team/probe-team"),
        ),
        (
            "another account, another partition",
            "arn:aws-us-gov:mpa:us-gov-west-1:100000000000:approval-team/probe-team".to_string(),
        ),
        (
            "another account, a team name the caller does own",
            format!("arn:aws:mpa:{REGION}:999999999999:approval-team/policy-reader"),
        ),
        (
            // A global resource's ARN carries no account field, and an ARN with no
            // account field is not one in the caller's account: the gate keys on that
            // field alone, so this is a denial and not the 404 an unrecorded resource
            // in the caller's own account earns.
            "an account-less ARN",
            format!(
                "{}/$DEFAULT",
                state::policy_arn(SEEDED_POLICY.0, SEEDED_POLICY.1)
            ),
        ),
    ];

    for (case, resource_arn) in rows {
        let (status, body) =
            get_resource_policy(&harness, &resource_arn, SEEDED_POLICY.1, "AWS_MANAGED").await;
        assert_eq!(status, 403, "case {case:?}: expected HTTP 403, got {status} with {body:?}");
        assert_eq!(
            error_code(&body),
            "AccessDeniedException",
            "case {case:?}: expected AccessDeniedException, got {body:?}"
        );
        assert_eq!(
            text(&body, "Message"),
            expected_denial("GetResourcePolicy", &resource_arn),
            "case {case:?}: wrong denial message in {body:?}"
        );
    }

    // The caller's own team is untouched by a denial.
    let (status, team) = harness.get(&team_arn).await;
    assert_eq!(status, 200, "the caller's own team is gone: {team:?}");
}

/// A resource policy is reported only for a resource the store records: on a store with
/// no teams in it, the seeded policy is in the catalog and still reported for nobody.
#[tokio::test]
async fn nothing_is_reported_for_a_store_with_no_resources() {
    let harness = Harness::without_organization();
    let (status, body) = get_resource_policy(
        &harness,
        &expected_arn("never-created"),
        SEEDED_POLICY.1,
        "AWS_MANAGED",
    )
    .await;
    assert_eq!(status, 404, "expected HTTP 404 on an empty store, got {status} with {body:?}");
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException on an empty store, got {body:?}"
    );
}

/// The policy-ARN grammar the attachments are read through, over the class the pattern
/// `^arn:.{1,63}:mpa:::aws:policy/[a-zA-Z0-9_\.-]{1,1023}/[a-zA-Z0-9_\.-]{1,1023}/(?:[\d]+|\$DEFAULT)$`
/// describes: what this service mints it reads back, and these are the ways a value
/// falls outside it.
#[test]
fn the_policy_version_arn_grammar_is_the_modelled_one() {
    // Every ARN this service mints is read back as the policy and version it names.
    for case in policy_cases() {
        let policy_arn = state::policy_arn(case.category, case.name);
        for (form, arn, expected) in [
            (
                "$DEFAULT",
                format!("{policy_arn}/$DEFAULT"),
                state::PolicyVersionSelector::Default,
            ),
            (
                "an explicit version id",
                state::policy_version_arn(&policy_arn, case.explicit_version),
                state::PolicyVersionSelector::Id(case.explicit_version),
            ),
        ] {
            let parsed = state::parse_policy_version_arn(&arn).unwrap_or_else(|| {
                panic!("case {:?} / {form}: {arn:?} was not read back", case.case)
            });
            assert_eq!(
                parsed.policy_arn, policy_arn,
                "case {:?} / {form}: wrong policy ARN read out of {arn:?}",
                case.case
            );
            assert_eq!(
                parsed.version, expected,
                "case {:?} / {form}: wrong version read out of {arn:?}",
                case.case
            );
        }
        assert!(
            state::is_policy_arn(&policy_arn),
            "case {:?}: {policy_arn:?} is not recognised as a policy ARN",
            case.case
        );
    }

    // Rows differ along which part of the grammar the value breaks.
    let refused: Vec<(&str, String)> = vec![
        ("no version segment", "arn:aws:mpa:::aws:policy/IdentityCenter/P".to_string()),
        (
            "a version that is neither a number nor $DEFAULT",
            "arn:aws:mpa:::aws:policy/IdentityCenter/P/LATEST".to_string(),
        ),
        (
            "a signed version",
            "arn:aws:mpa:::aws:policy/IdentityCenter/P/+1".to_string(),
        ),
        (
            // `PolicyVersionId` is `range(min: 1)`.
            "a version below the modelled minimum",
            "arn:aws:mpa:::aws:policy/IdentityCenter/P/0".to_string(),
        ),
        (
            "a version past what a version id can hold",
            "arn:aws:mpa:::aws:policy/IdentityCenter/P/99999999999".to_string(),
        ),
        (
            "a fourth path segment",
            "arn:aws:mpa:::aws:policy/IdentityCenter/P/1/2".to_string(),
        ),
        (
            "a region in the ARN",
            format!("arn:aws:mpa:{REGION}::aws:policy/IdentityCenter/P/1"),
        ),
        (
            "an account in the ARN",
            format!(
                "arn:aws:mpa:::{}:policy/IdentityCenter/P/1",
                engine::emulator_account_id()
            ),
        ),
        (
            "another service's namespace",
            "arn:aws:iam:::aws:policy/IdentityCenter/P/1".to_string(),
        ),
        (
            "not the aws:policy namespace",
            "arn:aws:mpa:::customer:policy/IdentityCenter/P/1".to_string(),
        ),
        (
            "a name character the pattern excludes",
            "arn:aws:mpa:::aws:policy/IdentityCenter/P@1/1".to_string(),
        ),
        ("an empty name segment", "arn:aws:mpa:::aws:policy/IdentityCenter//1".to_string()),
        ("a team ARN", expected_arn("team")),
        ("not an ARN at all", "not-an-arn-at-all".to_string()),
    ];
    for (case, arn) in refused {
        assert!(
            state::parse_policy_version_arn(&arn).is_none(),
            "case {case:?}: {arn:?} was read as a policy version"
        );
    }
}

/// The ARN grammar the `ResourceArn` refusal is decided by: six colon-separated fields
/// with a non-empty partition, service and resource, and an EMPTY region and account
/// allowed (a global resource's ARN carries neither).
#[test]
fn only_a_six_field_arn_is_well_formed() {
    let own = engine::emulator_account_id();
    // (case, value, well formed)
    let rows: Vec<(&str, String, bool)> = vec![
        ("a team ARN", expected_arn("team"), true),
        (
            "an account-less managed policy ARN",
            "arn:aws:mpa:::aws:policy/IdentityCenter/P/$DEFAULT".to_string(),
            true,
        ),
        ("another service's ARN", "arn:aws:s3:::bucket/key".to_string(), true),
        (
            "an ARN with a resource type and id",
            format!("arn:aws:mpa:us-east-1:{own}:session/s-1"),
            true,
        ),
        ("an empty value", String::new(), false),
        ("not an ARN at all", "not-an-arn-at-all".to_string(), false),
        ("the prefix alone", "arn".to_string(), false),
        ("five fields", format!("arn:aws:mpa:us-east-1:{own}"), false),
        (
            "an empty resource field",
            format!("arn:aws:mpa:us-east-1:{own}:"),
            false,
        ),
        (
            "an empty partition",
            format!("arn::mpa:us-east-1:{own}:approval-team/t"),
            false,
        ),
        (
            "an empty service",
            format!("arn:aws::us-east-1:{own}:approval-team/t"),
            false,
        ),
        (
            "the prefix in the wrong case",
            format!("ARN:aws:mpa:us-east-1:{own}:approval-team/t"),
            false,
        ),
    ];

    for (case, value, well_formed) in rows {
        assert_eq!(
            state::is_well_formed_arn(&value),
            well_formed,
            "case {case:?}: expected is_well_formed_arn == {well_formed} for {value:?}"
        );
    }
}
