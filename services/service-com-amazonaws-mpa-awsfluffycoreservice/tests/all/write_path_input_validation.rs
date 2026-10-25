// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two input rules every WRITE path of this service holds:
//! what a reference to another resource must resolve to, and which tag keys a caller may
//! write.
//!
//! 1. **A reference must resolve.** `CreateApprovalTeam$Policies` ATTACHES policies to
//!    the team it mints, and a policy is AWS-owned: no operation of this model creates
//!    one, so the only attachable references are the policy versions the catalog holds
//!    (`ListPolicies` / `ListPolicyVersions`). A reference whose policy or version the
//!    catalog cannot resolve is refused with `ValidationException` (400) and nothing is
//!    stored — the modelled `@pattern` fixes a reference's FORM, not the existence of
//!    what it names. The accepted half of the rule is asserted through the reads: the
//!    attachments `GetApprovalTeam` reports are exactly the ones `ListResourcePolicies`
//!    resolves and `GetResourcePolicy` answers for, so the two reads of one attachment
//!    list can never disagree.
//!
//! 2. **A reserved tag key is AWS's, not the caller's.** A tag key in the
//!    [`state::RESERVED_TAG_KEY_PREFIX`] namespace cannot be written by a caller, so
//!    EVERY path that writes tags — `CreateApprovalTeam`, `CreateIdentitySource` and
//!    `TagResource` — refuses such a request with `ValidationException` (400) and writes
//!    nothing, and accepts a key that merely resembles the prefix.
//!
//! The rows span the class each rule generalises over: for the references, which part of
//! the reference resolves to nothing (policy name, category, version id, partition), how
//! many references the request carries and where in the list the unresolvable one sits;
//! for the tag keys, the form of the key (the bare prefix, a prefixed key, the prefix in
//! another case, the prefix elsewhere in the key, a key that only starts like it),
//! crossed with each operation that writes tags.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the route, the URI bindings, the JSON envelope
//! and the HTTP status are all exercised.

use std::collections::BTreeMap;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_team_lifecycle::{encode_label, error_code, expected_arn, text, Harness, REGION};

/// The policy a fresh store seeds (`state::seeded_policy_catalog`): category and name.
const SEEDED_POLICY: (&str, &str) = ("IdentityCenter", "MPAApprovalTeamPolicy");

/// A catalog entry this suite records so a row can attach a version that is NOT the
/// policy's default, and name a version id one past the ones it holds.
const TWO_VERSION_POLICY: (&str, &str) = ("ResourceAccessManager", "mpa.ram_team-policy");

/// Record `TWO_VERSION_POLICY` with versions 1 and 2, its default being version 1.
///
/// No modelled operation mints a policy — the catalog is AWS's — so a table that needs
/// an entry a fresh store does not seed writes it directly, as `resource_policies.rs`
/// does.
fn record_two_version_policy(harness: &Harness) -> String {
    let created = SystemTime::now();
    let mut policy = state::CatalogPolicy::with_first_version(
        TWO_VERSION_POLICY.0,
        TWO_VERSION_POLICY.1,
        state::PolicyType::AwsRam,
        created,
    );
    policy.insert_version(state::CatalogPolicyVersion {
        version_id: 2,
        status: state::PolicyStatus::Attachable,
        creation_time: created,
        last_updated_time: created,
    });
    let arn = policy.arn.clone();
    let store = state::service_state(&*harness.engine).expect("the service store");
    store.lock().insert_policy(policy);
    arn
}

/// The qualified reference to the seeded policy's default version.
fn seeded_default() -> String {
    format!(
        "{}/{}",
        state::policy_arn(SEEDED_POLICY.0, SEEDED_POLICY.1),
        state::DEFAULT_POLICY_VERSION
    )
}

/// A `CreateApprovalTeam` body every modelled constraint accepts, attaching `policies`
/// and carrying `tags`.
fn create_body(name: &str, policies: &[String], tags: &[(String, String)]) -> Value {
    let mut body = json!({
        "Name": name,
        "Description": "write path validation",
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
    });
    if !tags.is_empty() {
        body["Tags"] = Value::Object(
            tags.iter()
                .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                .collect(),
        );
    }
    body
}

/// A `CreateIdentitySource` body every modelled constraint accepts, carrying `tags`.
/// `InstanceArn` satisfies `IdcInstanceArn`'s pattern
/// `^arn:.+:sso:::instance/(?:sso)?ins-[a-zA-Z0-9-.]{16}$`.
fn identity_source_body(tags: &[(String, String)]) -> Value {
    let mut body = json!({
        "IdentitySourceParameters": {
            "IamIdentityCenter": {
                "InstanceArn": "arn:aws:sso:::instance/ssoins-1234567890abcdef",
                "Region": REGION,
            }
        }
    });
    if !tags.is_empty() {
        body["Tags"] = Value::Object(
            tags.iter()
                .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                .collect(),
        );
    }
    body
}

/// `POST /approval-teams/?List` — how many teams the store reports.
async fn team_count(harness: &Harness, case: &str) -> usize {
    let (status, body) = harness.list("").await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListApprovalTeams returned {status} with {body:?}"
    );
    body["ApprovalTeams"].as_array().map_or(0, Vec::len)
}

/// `POST /identity-sources/?List` — how many identity sources the store reports.
async fn identity_source_count(harness: &Harness, case: &str) -> usize {
    let (status, body) = harness.call("POST", "/identity-sources/?List", None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListIdentitySources returned {status} with {body:?}"
    );
    body["IdentitySources"].as_array().map_or(0, Vec::len)
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

/// `PUT /tags/{ResourceArn}` — `TagResource`.
async fn tag_resource(harness: &Harness, arn: &str, tags: &[(String, String)]) -> (u16, Value) {
    harness
        .call(
            "PUT",
            &format!("/tags/{}", encode_label(arn)),
            Some(json!({
                "Tags": Value::Object(
                    tags.iter()
                        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                        .collect(),
                )
            })),
        )
        .await
}

/// `GET /tags/{ResourceArn}` — the tags `ListTagsForResource` reports.
async fn tags_of(harness: &Harness, arn: &str, case: &str) -> BTreeMap<String, String> {
    let (status, body) = harness
        .call("GET", &format!("/tags/{}", encode_label(arn)), None)
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource returned {status} with {body:?}"
    );
    body.get("Tags")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("case {case:?}: ListTagsForResource reported no Tags: {body:?}"))
        .iter()
        .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
        .collect()
}

/// One `(key, value)` pair as a tag map entry.
fn tag(key: &str) -> Vec<(String, String)> {
    vec![(key.to_string(), "value".to_string())]
}

/// `Policies` lists a request names that the catalog cannot resolve, one row per part of
/// the reference that resolves to nothing and per position in the list. Every row
/// satisfies `QualifiedPolicyArn`'s `@pattern`, so each one reaches the handler.
fn unattachable_reference_rows(two_version_policy: &str) -> Vec<(&'static str, Vec<String>)> {
    let seeded = state::policy_arn(SEEDED_POLICY.0, SEEDED_POLICY.1);
    vec![
        (
            "a policy name the catalog does not hold",
            vec![format!(
                "{}/$DEFAULT",
                state::policy_arn(SEEDED_POLICY.0, "NoSuchPolicy")
            )],
        ),
        (
            "a category the catalog does not hold",
            vec![format!(
                "{}/1",
                state::policy_arn("NoSuchCategory", SEEDED_POLICY.1)
            )],
        ),
        (
            "a version id the policy does not hold",
            vec![format!("{seeded}/2")],
        ),
        (
            // `PolicyVersionId` is `range(min: 1)`, so this is the largest id a
            // reference can carry at all.
            "the largest version id a reference can carry",
            vec![format!("{seeded}/{}", i32::MAX)],
        ),
        (
            "a version id one past the ones a recorded policy holds",
            vec![format!("{two_version_policy}/3")],
        ),
        (
            // `QualifiedPolicyArn`'s pattern allows any partition (`.{1,63}`), and the
            // catalog holds this policy in `aws` only.
            "a policy of another partition",
            vec![format!(
                "arn:aws-us-gov:mpa:::aws:policy/{}/{}/$DEFAULT",
                SEEDED_POLICY.0, SEEDED_POLICY.1
            )],
        ),
        (
            "an unresolvable reference after a resolvable one",
            vec![seeded_default(), format!("{seeded}/9")],
        ),
        (
            "an unresolvable reference before resolvable ones",
            vec![
                format!("{}/$DEFAULT", state::policy_arn("IdentityCenter", "Nope")),
                seeded_default(),
                format!("{two_version_policy}/2"),
            ],
        ),
    ]
}

/// A create naming a policy version this service does not hold is refused with
/// `ValidationException` (400) and stores NOTHING — so no team can report an attachment
/// the resource-policy reads cannot resolve.
#[tokio::test]
async fn a_create_naming_an_unattachable_policy_is_refused_and_stores_nothing() {
    let rows = {
        // The rows are built once, against the ARN the recorded entry carries; each row
        // then runs against a store of its own.
        let harness = Harness::new();
        let two_version_policy = record_two_version_policy(&harness);
        unattachable_reference_rows(&two_version_policy)
    };

    for (index, (case, policies)) in rows.into_iter().enumerate() {
        let harness = Harness::new();
        record_two_version_policy(&harness);
        let name = format!("unattachable-{index}");
        let (status, body) = harness.create(&create_body(&name, &policies, &[])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {policies:?}, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected error code ValidationException, got {body:?}"
        );
        assert!(
            !text(&body, "Message").is_empty(),
            "case {case:?}: the refusal carries no Message: {body:?}"
        );

        // Nothing was stored: the team does not read back, the listing is empty, and the
        // name is still free for a conforming create.
        let (status, read) = harness.get(&expected_arn(&name)).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused create left a readable team: {read:?}"
        );
        let stored = team_count(&harness, case).await;
        assert_eq!(
            stored, 0,
            "case {case:?}: the store holds {stored} team(s) after a refused create"
        );

        let (status, created) = harness
            .create(&create_body(&name, &[seeded_default()], &[]))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the name was not released by the refusal: {created:?}"
        );
    }
}

/// One row of the accepted-attachment table: the references a create attaches, and the
/// `(PolicyName, PolicyType)` each of them resolves to, in attachment order.
struct AttachableCase {
    case: &'static str,
    policies: Vec<String>,
    resolved: Vec<(&'static str, &'static str)>,
}

/// Every reference a create ACCEPTS resolves for every reader: the attachments
/// `GetApprovalTeam` reports are exactly what `ListResourcePolicies` resolves, and
/// `GetResourcePolicy` answers for each of them.
#[tokio::test]
async fn the_attachments_a_create_accepts_resolve_for_every_reader() {
    let harness = Harness::new();
    let two_version_policy = record_two_version_policy(&harness);
    let seeded = state::policy_arn(SEEDED_POLICY.0, SEEDED_POLICY.1);

    let rows = vec![
        AttachableCase {
            case: "the seeded policy, attached as $DEFAULT",
            policies: vec![seeded_default()],
            resolved: vec![(SEEDED_POLICY.1, "AWS_MANAGED")],
        },
        AttachableCase {
            case: "the seeded policy, attached by version id",
            policies: vec![format!("{seeded}/1")],
            resolved: vec![(SEEDED_POLICY.1, "AWS_MANAGED")],
        },
        AttachableCase {
            case: "a recorded policy, attached by a version that is not its default",
            policies: vec![format!("{two_version_policy}/2")],
            resolved: vec![(TWO_VERSION_POLICY.1, "AWS_RAM")],
        },
        AttachableCase {
            case: "every resolvable form at once",
            policies: vec![
                seeded_default(),
                format!("{two_version_policy}/1"),
                format!("{two_version_policy}/2"),
            ],
            resolved: vec![
                (SEEDED_POLICY.1, "AWS_MANAGED"),
                (TWO_VERSION_POLICY.1, "AWS_RAM"),
                (TWO_VERSION_POLICY.1, "AWS_RAM"),
            ],
        },
    ];

    for (
        index,
        AttachableCase {
            case,
            policies,
            resolved,
        },
    ) in rows.into_iter().enumerate()
    {
        let name = format!("attachable-{index}");
        let (status, created) = harness
            .create(&create_body(&name, &policies, &[]))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: create returned {status} with {created:?}"
        );
        let arn = text(&created, "Arn");

        let (status, team) = harness.get(&arn).await;
        assert_eq!(status, 200, "case {case:?}: get returned {team:?}");
        let reported: Vec<String> = team["Policies"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|reference| text(reference, "PolicyArn"))
            .collect();
        assert_eq!(
            reported, policies,
            "case {case:?}: GetApprovalTeam reports {reported:?}, not the attached \
             {policies:?}"
        );

        // The second read of the same attachment list must agree with the first: every
        // attachment the team reports resolves to a resource policy.
        let (status, listed) = list_resource_policies(&harness, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListResourcePolicies returned {status} with {listed:?}"
        );
        let entries = listed["ResourcePolicies"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            entries.len(),
            policies.len(),
            "case {case:?}: ListResourcePolicies reports {} of {} attachments: {listed:?}",
            entries.len(),
            policies.len()
        );
        let names: Vec<String> = entries
            .iter()
            .map(|entry| text(entry, "PolicyName"))
            .collect();
        let expected_names: Vec<String> = resolved
            .iter()
            .map(|(policy_name, _)| (*policy_name).to_string())
            .collect();
        assert_eq!(
            names, expected_names,
            "case {case:?}: ListResourcePolicies reports {names:?}, not {expected_names:?}"
        );

        for (policy_name, policy_type) in &resolved {
            let (status, body) =
                get_resource_policy(&harness, &arn, policy_name, policy_type).await;
            assert_eq!(
                status, 200,
                "case {case:?}: GetResourcePolicy({policy_name}, {policy_type}) returned \
                 {status} with {body:?}"
            );
            assert_eq!(
                text(&body, "PolicyName"),
                *policy_name,
                "case {case:?}: wrong PolicyName in {body:?}"
            );
        }
    }
}

/// The tag-key rows: the forms the reserved namespace covers, and the ones that only
/// resemble it. `TagKey` is `length(min: 1, max: 128)` with no modelled pattern, so
/// every row reaches the handler.
fn tag_key_rows() -> Vec<(&'static str, String, bool)> {
    vec![
        ("the bare reserved prefix", state::RESERVED_TAG_KEY_PREFIX.to_string(), true),
        ("a reserved key", "aws:owner".to_string(), true),
        (
            "a reserved key with further colons",
            "aws:cloudformation:stack-name".to_string(),
            true,
        ),
        ("the prefix in upper case", "AWS:Owner".to_string(), true),
        ("the prefix in mixed case", "Aws:owner".to_string(), true),
        (
            "the longest reserved key the model allows",
            format!("aws:{}", "k".repeat(124)),
            true,
        ),
        ("the prefix without its colon", "awsome".to_string(), false),
        ("the prefix alone, without its colon", "aws".to_string(), false),
        ("the prefix later in the key", "my:aws:thing".to_string(), false),
        ("a key that starts past the prefix", " aws:owner".to_string(), false),
        ("an unrelated key", "owner".to_string(), false),
    ]
}

/// `CreateApprovalTeam` refuses a reserved tag key with `ValidationException` (400) and
/// stores no team, and accepts a key outside the namespace.
#[tokio::test]
async fn a_create_naming_a_reserved_tag_key_is_refused_and_stores_nothing() {
    for (index, (case, key, reserved)) in tag_key_rows().into_iter().enumerate() {
        let harness = Harness::new();
        let name = format!("tagged-create-{index}");
        let (status, body) = harness
            .create(&create_body(&name, &[seeded_default()], &tag(&key)))
            .await;

        if reserved {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for key {key:?}, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected error code ValidationException, got {body:?}"
            );
            let stored = team_count(&harness, case).await;
            assert_eq!(
                stored, 0,
                "case {case:?}: the store holds {stored} team(s) after a refused create"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case:?}: a key outside the reserved namespace was refused: {body:?}"
        );
        let listed = tags_of(&harness, &text(&body, "Arn"), case).await;
        assert_eq!(
            listed,
            BTreeMap::from([(key.clone(), "value".to_string())]),
            "case {case:?}: the created team reports {listed:?}"
        );
    }
}

/// `TagResource` refuses a reserved tag key with `ValidationException` (400) and leaves
/// the resource's tags exactly as they were, and accepts a key outside the namespace.
#[tokio::test]
async fn tagging_a_resource_with_a_reserved_key_is_refused_and_writes_nothing() {
    let harness = Harness::new();
    let (status, created) = harness
        .create(&create_body(
            "tag-target",
            &[seeded_default()],
            &[("env".to_string(), "test".to_string())],
        ))
        .await;
    assert_eq!(status, 200, "create returned {created:?}");
    let arn = text(&created, "Arn");
    let mut expected = BTreeMap::from([("env".to_string(), "test".to_string())]);

    for (case, key, reserved) in tag_key_rows() {
        let (status, body) = tag_resource(&harness, &arn, &tag(&key)).await;
        if reserved {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for key {key:?}, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected error code ValidationException, got {body:?}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: a key outside the reserved namespace was refused: {body:?}"
            );
            expected.insert(key.clone(), "value".to_string());
        }

        let listed = tags_of(&harness, &arn, case).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: the resource reports {listed:?}, not {expected:?}"
        );
    }
}

/// `CreateIdentitySource` holds the same rule: a reserved tag key is
/// `ValidationException` (400) and no source is stored.
#[tokio::test]
async fn an_identity_source_create_naming_a_reserved_tag_key_is_refused() {
    for (case, key, reserved) in tag_key_rows() {
        let harness = Harness::new();
        let (status, body) = harness
            .call("POST", "/identity-sources", Some(identity_source_body(&tag(&key))))
            .await;

        if reserved {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for key {key:?}, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected error code ValidationException, got {body:?}"
            );
            let stored = identity_source_count(&harness, case).await;
            assert_eq!(
                stored, 0,
                "case {case:?}: the store holds {stored} identity source(s) after a \
                 refused create"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case:?}: a key outside the reserved namespace was refused: {body:?}"
        );
        let listed = tags_of(&harness, &text(&body, "IdentitySourceArn"), case).await;
        assert_eq!(
            listed,
            BTreeMap::from([(key.clone(), "value".to_string())]),
            "case {case:?}: the created identity source reports {listed:?}"
        );
    }
}

/// The namespace rule itself, over the same rows: the prefix is matched at the START of
/// the key and ignoring case, and nowhere else.
#[test]
fn the_reserved_namespace_covers_exactly_the_prefixed_keys() {
    for (case, key, reserved) in tag_key_rows() {
        assert_eq!(
            state::is_reserved_tag_key(&key),
            reserved,
            "case {case:?}: expected is_reserved_tag_key({key:?}) == {reserved}"
        );
    }

    // A request is refused for ANY reserved key it names, wherever in the map it sits.
    let keys = ["env", "owner", "aws:owner"];
    assert!(
        state::names_reserved_tag_key(keys),
        "a key set holding {keys:?} names a reserved key"
    );
    assert!(
        !state::names_reserved_tag_key(["env", "owner"]),
        "a key set of unreserved keys names no reserved key"
    );
    assert!(
        !state::names_reserved_tag_key(std::iter::empty::<&str>()),
        "an empty key set names no reserved key"
    );
}
