// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for HOW THIS SERVICE WORDS THE REJECTIONS it states with a fixed
//! rule, across every operation the rule reaches.
//!
//! Three rules are under test, each asserted over a TABLE whose rows span the class it
//! generalises over rather than one recorded request:
//!
//! 1. **The not-found constant.** Every ARN-addressed resource lookup that resolves to
//!    nothing answers 404 `ResourceNotFoundException` with the ONE message
//!    [`state::RESOURCE_NOT_FOUND_MESSAGE`] — the same string whichever operation was
//!    called and whichever ARN it was given, so the refusal names nothing the request
//!    carried. The rows cross the operations that resolve a resource by ARN with ARNs
//!    that resolve to nothing, varied along the resource name (shortest, longest,
//!    punctuated), the region and the resource kind. `GetResourcePolicy` additionally
//!    resolves its `ResourceArn` BEFORE it matches `PolicyName`/`PolicyType`, so its
//!    answer for an unrecorded resource is byte-identical across the whole product of
//!    those two members.
//!
//! 2. **The tag family's not-found message IS the `ResourceArn`.** `TagResource`,
//!    `UntagResource` and `ListTagsForResource` each answer 404 with the request's
//!    `ResourceArn` verbatim and nothing else — the message is built from the request,
//!    so the rows vary the ARN and assert the message equals it exactly, and (for
//!    `TagResource`) vary the tag count from none to one past the per-resource maximum
//!    to show existence is decided before the quota.
//!
//! 3. **The malformed-ARN constant.** A value that is not a well-formed MPA ARN is
//!    refused with 400 `ValidationException` carrying [`state::INVALID_ARN_MESSAGE`],
//!    on every member that carries an ARN — whether the model declares that member's
//!    ARN grammar as a `@pattern` (where the refusal happens at the boundary, before
//!    any handler, including for operations that are still stubs) or does not (where
//!    the handler applies the service's own ARN rule). The refusal names neither the
//!    member nor the grammar, which is what distinguishes it from the framework's
//!    per-member constraint report.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the route, the URI bindings, the JSON
//! envelope and the HTTP status are all exercised.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_team_lifecycle::{encode_label, error_code, expected_arn, text, Harness, REGION};

/// The `Message` of an error envelope, however this protocol spelled the member.
///
/// A MODELLED error renders its member as the model names it (`Message`, from
/// `ResourceNotFoundException$Message`); a refusal the framework answers before any
/// handler ran renders the synthetic shape's `message`. Both are the same field to a
/// client, so a table that spans both paths reads it through here.
fn message(body: &Value) -> String {
    let modelled = text(body, "Message");
    if modelled.is_empty() {
        return text(body, "message");
    }
    modelled
}

/// The policy a fresh store seeds (`state::seeded_policy_catalog`): category and name.
const SEEDED_POLICY: (&str, &str) = ("IdentityCenter", "MPAApprovalTeamPolicy");

/// Every value `PolicyType` accepts, from
/// `smithy_query(query="shape",shape="PolicyType")` (`values: [AWS_MANAGED, AWS_RAM]`).
const POLICY_TYPES: [&str; 2] = ["AWS_MANAGED", "AWS_RAM"];

/// `ApprovalTeamName` is `length(max: 64)` with pattern `^[a-zA-Z0-9._-]+$`
/// (`smithy_query(query="shape",shape="ApprovalTeamName")`).
const MAX_TEAM_NAME: usize = 64;

/// The qualified reference to the seeded policy's default version, which satisfies
/// `QualifiedPolicyArn`'s pattern.
fn seeded_attachment() -> String {
    format!(
        "{}/{}",
        state::policy_arn(SEEDED_POLICY.0, SEEDED_POLICY.1),
        state::DEFAULT_POLICY_VERSION
    )
}

/// A create body the modelled constraints accept, attaching the policy the catalog
/// seeds so the team's attachment resolves.
fn create_body(name: &str) -> Value {
    json!({
        "Name": name,
        "Description": "rejection wording",
        "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 1 } },
        "Approvers": [{
            "PrimaryIdentityId": "identity-0001",
            "PrimaryIdentitySourceArn":
                format!(
                    "arn:aws:mpa:{REGION}:{}:identity-source/ssoins-0001",
                    engine::emulator_account_id()
                ),
        }],
        "Policies": [{ "PolicyArn": seeded_attachment() }],
    })
}

/// Create a team and return the ARN the service minted for it.
async fn create_team(harness: &Harness, name: &str) -> String {
    let (status, body) = harness.create(&create_body(name)).await;
    assert_eq!(
        status, 200,
        "CreateApprovalTeam({name}) returned {status} with {body:?}"
    );
    let arn = text(&body, "Arn");
    assert!(!arn.is_empty(), "CreateApprovalTeam({name}) minted no Arn: {body:?}");
    arn
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

// =============================================================================
// Rule 1: one constant message for every resource lookup that resolves to nothing
// =============================================================================

/// The operations that resolve a resource by ARN and answer 404 when nothing is recorded
/// under it. Each is called through its own route, so the table exercises the real wire
/// path of every one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lookup {
    GetApprovalTeam,
    UpdateApprovalTeam,
    StartActiveApprovalTeamDeletion,
    ListPolicyVersions,
    GetResourcePolicy,
}

impl Lookup {
    /// Whether this operation addresses an approval team (as opposed to a policy), which
    /// is what decides the shape of the ARNs its rows carry.
    fn addresses_a_team(self) -> bool {
        self != Lookup::ListPolicyVersions
    }
}

/// Call `op` with `arn` as the resource it addresses.
async fn lookup(harness: &Harness, op: Lookup, arn: &str) -> (u16, Value) {
    let label = encode_label(arn);
    match op {
        Lookup::GetApprovalTeam => harness.call("GET", &format!("/approval-teams/{label}"), None).await,
        Lookup::UpdateApprovalTeam => {
            harness
                .call(
                    "PATCH",
                    &format!("/approval-teams/{label}"),
                    Some(json!({ "Description": "a new description" })),
                )
                .await
        }
        Lookup::StartActiveApprovalTeamDeletion => {
            harness
                .call("POST", &format!("/approval-teams/{label}?Delete"), None)
                .await
        }
        Lookup::ListPolicyVersions => {
            harness
                .call("POST", &format!("/policies/{label}/?List"), None)
                .await
        }
        Lookup::GetResourcePolicy => {
            get_resource_policy(harness, arn, SEEDED_POLICY.1, "AWS_MANAGED").await
        }
    }
}

/// Team ARNs that satisfy `ApprovalTeamArn`'s pattern and name no stored team. Rows
/// differ along the name's length and form and along the region.
fn unrecorded_team_arns() -> Vec<(String, String)> {
    let own = engine::emulator_account_id();
    vec![
        ("a team name never created".to_string(), expected_arn("never-created")),
        // `ApprovalTeamName`'s pattern accepts a single character.
        ("the shortest legal team name".to_string(), expected_arn("a")),
        (
            "the longest legal team name".to_string(),
            expected_arn(&"n".repeat(MAX_TEAM_NAME)),
        ),
        (
            "a name of dots, underscores and dashes".to_string(),
            expected_arn("Team.Alpha_1-x"),
        ),
        (
            "another region's team ARN".to_string(),
            format!("arn:aws:mpa:eu-west-1:{own}:approval-team/ghost"),
        ),
        (
            "a one-character region".to_string(),
            format!("arn:aws:mpa:a:{own}:approval-team/ghost"),
        ),
    ]
}

/// Policy ARNs that satisfy `UnqualifiedPolicyArn`'s pattern and name no catalog entry.
fn unrecorded_policy_arns() -> Vec<(String, String)> {
    vec![
        (
            "a policy name no catalog entry carries".to_string(),
            state::policy_arn(SEEDED_POLICY.0, "NeverSeeded"),
        ),
        (
            "a category no catalog entry carries".to_string(),
            state::policy_arn("NoSuchCategory", SEEDED_POLICY.1),
        ),
        (
            "the seeded policy's name in another partition".to_string(),
            format!(
                "arn:aws-us-gov:mpa:::aws:policy/{}/{}",
                SEEDED_POLICY.0, SEEDED_POLICY.1
            ),
        ),
    ]
}

/// Rule 1: the answer to a resource lookup that resolves to nothing is 404
/// `ResourceNotFoundException` with ONE service-wide message, identical across every
/// operation and every ARN.
#[tokio::test]
async fn every_resource_lookup_that_resolves_to_nothing_answers_the_same_message() {
    let harness = Harness::new();
    // A live team and the seeded policy, so the table's refusals are about what the
    // store does NOT record rather than about an empty store.
    let live = create_team(&harness, "live-team").await;

    let operations = [
        Lookup::GetApprovalTeam,
        Lookup::UpdateApprovalTeam,
        Lookup::StartActiveApprovalTeamDeletion,
        Lookup::ListPolicyVersions,
        Lookup::GetResourcePolicy,
    ];

    // Every (status, code, message) triple the table produced: the rule is that there is
    // exactly ONE of them.
    let mut answers: BTreeSet<(u16, String, String)> = BTreeSet::new();

    for op in operations {
        let rows = if op.addresses_a_team() {
            unrecorded_team_arns()
        } else {
            unrecorded_policy_arns()
        };
        for (case, arn) in rows {
            let case = format!("{op:?} / {case}");
            let (status, body) = lookup(&harness, op, &arn).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: wrong error code in {body:?}"
            );
            assert_eq!(
                message(&body),
                state::RESOURCE_NOT_FOUND_MESSAGE,
                "case {case:?}: wrong not-found message in {body:?}"
            );
            // The message is resource-opaque: nothing the request carried is in it.
            assert!(
                !message(&body).contains(&arn),
                "case {case:?}: the refusal leaked the ARN asked for: {body:?}"
            );
            answers.insert((status, error_code(&body), message(&body)));
        }
    }

    assert_eq!(
        answers.len(),
        1,
        "the not-found answer varied with the operation or the ARN: {answers:?}"
    );

    // The control: a resource the store DOES record is not refused, so the constant is
    // the answer to a failed lookup and not to every call.
    let (status, body) = lookup(&harness, Lookup::GetApprovalTeam, &live).await;
    assert_eq!(status, 200, "the live team must still be readable: {body:?}");
}

/// Rule 1, applied to `GetResourcePolicy`: the resource is resolved FIRST, so for a
/// `ResourceArn` the store records nothing under, the answer does not depend on
/// `PolicyName` or `PolicyType` at all.
#[tokio::test]
async fn get_resource_policy_resolves_the_resource_before_the_policy_name() {
    let harness = Harness::new();
    let live = create_team(&harness, "policy-reader").await;
    let missing = expected_arn("never-created");

    // Rows span the `PolicyName` class: a name nothing carries, the empty name, the name
    // that IS attached to a resource this store records, the longest name
    // (`PolicyName` is `length(max: 1000)` on the request member), and a name whose form
    // no policy ARN could hold.
    let names: Vec<String> = vec![
        "nonexistent-policy-name".to_string(),
        String::new(),
        SEEDED_POLICY.1.to_string(),
        "p".repeat(1000),
        "a name with spaces and /slashes/".to_string(),
    ];

    let mut answers: BTreeSet<(u16, String, String)> = BTreeSet::new();
    for name in &names {
        for policy_type in POLICY_TYPES {
            let case = format!("PolicyName {name:?} / PolicyType {policy_type}");
            let (status, body) = get_resource_policy(&harness, &missing, name, policy_type).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: wrong error code in {body:?}"
            );
            assert_eq!(
                message(&body),
                state::RESOURCE_NOT_FOUND_MESSAGE,
                "case {case:?}: wrong not-found message in {body:?}"
            );
            answers.insert((status, error_code(&body), message(&body)));
        }
    }
    assert_eq!(
        answers.len(),
        1,
        "the answer for an unrecorded resource varied with PolicyName/PolicyType: {answers:?}"
    );

    // The control: the same (PolicyName, PolicyType) IS resolved for a resource the store
    // records, so the refusal above is the resource lookup failing and not the matching.
    let (status, body) = get_resource_policy(&harness, &live, SEEDED_POLICY.1, "AWS_MANAGED").await;
    assert_eq!(
        status, 200,
        "the attached policy must be readable for a recorded resource: {body:?}"
    );
    assert_eq!(
        text(&body, "PolicyName"),
        SEEDED_POLICY.1,
        "the attached policy reported the wrong name: {body:?}"
    );
}

// =============================================================================
// Rule 2: the tag family's not-found message is the ResourceArn itself
// =============================================================================

/// The tag family, and how each member of it is called.
#[derive(Debug, Clone, Copy)]
enum TagCall {
    /// `PUT /tags/{ResourceArn}` with `n` tags.
    Tag(usize),
    /// `POST /tags/{ResourceArn}` with one tag key.
    Untag,
    /// `GET /tags/{ResourceArn}`.
    List,
}

/// `n` distinct tags, as a request's `Tags` map.
fn tag_map(n: usize) -> Value {
    Value::Object(
        (0..n)
            .map(|i| (format!("key-{i:03}"), Value::String(format!("value-{i:03}"))))
            .collect(),
    )
}

async fn tag_call(harness: &Harness, call: TagCall, arn: &str) -> (u16, Value) {
    let uri = format!("/tags/{}", encode_label(arn));
    match call {
        TagCall::Tag(n) => {
            harness
                .call("PUT", &uri, Some(json!({ "Tags": tag_map(n) })))
                .await
        }
        TagCall::Untag => {
            harness
                .call("POST", &uri, Some(json!({ "TagKeys": ["key-000"] })))
                .await
        }
        TagCall::List => harness.call("GET", &uri, None).await,
    }
}

/// Rule 2: on the tag family a not-found message IS the `ResourceArn` the request sent,
/// verbatim and with no surrounding prose — so the message varies with the request while
/// the rule does not.
#[tokio::test]
async fn the_tag_family_answers_a_missing_resource_with_the_resource_arn_itself() {
    let harness = Harness::new();
    let live = create_team(&harness, "live-team").await;
    let own = engine::emulator_account_id();

    // Rows differ along WHY the ARN names nothing: a team never created, a team in
    // another region, another resource kind, a near-miss of a live team's ARN, and the
    // longest and shortest ARNs these members accept.
    let rows: Vec<(&str, String)> = vec![
        ("a team name never created", expected_arn("never-created")),
        (
            "a team ARN in a region holding no such team",
            format!("arn:aws:mpa:eu-west-1:{own}:approval-team/live-team"),
        ),
        (
            "an identity-source ARN, a resource kind this service does not store",
            format!("arn:aws:mpa:{REGION}:{own}:identity-source/ssoins-0001"),
        ),
        (
            "a session ARN, a resource kind this service does not store",
            format!("arn:aws:mpa:{REGION}:{own}:session/live-team/0001"),
        ),
        ("a live team's ARN with one character added", format!("{live}x")),
        (
            "the longest team name",
            expected_arn(&"n".repeat(MAX_TEAM_NAME)),
        ),
    ];

    // `TagResource` is called with no tags, with one, and with one past the per-resource
    // maximum: existence is decided before the quota, so all three answer alike and none
    // of them is `TooManyTagsException`.
    let calls = [
        TagCall::Tag(0),
        TagCall::Tag(1),
        TagCall::Tag(state::MAX_TAGS_PER_RESOURCE + 1),
        TagCall::Untag,
        TagCall::List,
    ];

    for (row, arn) in &rows {
        for call in calls {
            let case = format!("{call:?} / {row}");
            let (status, body) = tag_call(&harness, call, arn).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: wrong error code in {body:?}"
            );
            // The whole message, and nothing but the ARN.
            assert_eq!(
                message(&body),
                *arn,
                "case {case:?}: the message is not the ResourceArn verbatim: {body:?}"
            );
        }
    }

    // None of those refusals touched the resource that does exist.
    let (status, body) = tag_call(&harness, TagCall::List, &live).await;
    assert_eq!(status, 200, "the live team's tags must still be readable: {body:?}");
}

/// One row of the `UntagResource` table.
struct UntagCase {
    case: &'static str,
    /// The keys the call asks to remove.
    remove: Vec<&'static str>,
    /// What the resource must hold once it has.
    remaining: Vec<(String, String)>,
}

/// `UntagResource` round-trip: what it removes is read back through
/// `ListTagsForResource`, per key and idempotently.
#[tokio::test]
async fn untag_resource_removes_the_keys_it_names_and_nothing_else() {
    let harness = Harness::new();
    let arn = create_team(&harness, "tagged-team").await;

    let tags = |body: &Value| -> BTreeMap<String, String> {
        body.get("Tags")
            .and_then(Value::as_object)
            .map(|map| {
                map.iter()
                    .map(|(key, value)| {
                        (key.clone(), value.as_str().unwrap_or_default().to_string())
                    })
                    .collect()
            })
            .unwrap_or_default()
    };

    let pair = |key: &str, value: &str| (key.to_string(), value.to_string());

    // (case, keys to remove, what must remain)
    let rows: Vec<UntagCase> = vec![
        UntagCase {
            case: "no keys removes nothing",
            remove: vec![],
            remaining: vec![pair("a", "1"), pair("b", "2"), pair("c", "3")],
        },
        UntagCase {
            case: "one key the resource carries",
            remove: vec!["b"],
            remaining: vec![pair("a", "1"), pair("c", "3")],
        },
        UntagCase {
            case: "a key the resource does not carry is a no-op",
            remove: vec!["never-held"],
            remaining: vec![pair("a", "1"), pair("b", "2"), pair("c", "3")],
        },
        UntagCase {
            case: "several keys at once, one of them not held",
            remove: vec!["a", "c", "never-held"],
            remaining: vec![pair("b", "2")],
        },
        UntagCase {
            case: "every key the resource carries",
            remove: vec!["a", "b", "c"],
            remaining: Vec::new(),
        },
    ];

    for UntagCase {
        case,
        remove,
        remaining,
    } in rows
    {
        // Re-tag to the same starting point, which `TagResource`'s merge makes exact.
        let (status, body) = harness
            .call(
                "PUT",
                &format!("/tags/{}", encode_label(&arn)),
                Some(json!({ "Tags": { "a": "1", "b": "2", "c": "3" } })),
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: TagResource returned {status} with {body:?}");

        let (status, body) = harness
            .call(
                "POST",
                &format!("/tags/{}", encode_label(&arn)),
                Some(json!({ "TagKeys": remove })),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UntagResource returned {status} with {body:?}"
        );

        // Read back through the implemented reader: the removal is in the store, not in
        // this response.
        let (status, body) = harness
            .call("GET", &format!("/tags/{}", encode_label(&arn)), None)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource returned {status} with {body:?}"
        );
        let expected: BTreeMap<String, String> = remaining.into_iter().collect();
        assert_eq!(
            tags(&body),
            expected,
            "case {case:?}: wrong tags remain after UntagResource({remove:?})"
        );

        // Idempotent: the same call again leaves the same tags.
        let (status, body) = harness
            .call(
                "POST",
                &format!("/tags/{}", encode_label(&arn)),
                Some(json!({ "TagKeys": remove })),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the repeated UntagResource returned {status} with {body:?}"
        );
        let (_, body) = harness
            .call("GET", &format!("/tags/{}", encode_label(&arn)), None)
            .await;
        assert_eq!(
            tags(&body),
            expected,
            "case {case:?}: the repeated UntagResource({remove:?}) changed the tags"
        );
    }
}

// =============================================================================
// Rule 3: one constant message for the malformed-ARN class
// =============================================================================

/// Every operation of this service whose input carries an Amazon Resource Name, named
/// by the operation because each of them carries exactly one such member:
/// `Arn` (`GetApprovalTeam`, `UpdateApprovalTeam`, `StartActiveApprovalTeamDeletion`,
/// `DeleteInactiveApprovalTeamVersion`, `StartApprovalTeamBaseline`),
/// `ApprovalTeamArn` (`ListSessions`), `SessionArn` (`GetSession`, `CancelSession`),
/// `PolicyVersionArn` (`GetPolicyVersion`), `PolicyArn` (`ListPolicyVersions`) and
/// `ResourceArn` (the tag family and `GetResourcePolicy`).
#[derive(Debug, Clone, Copy)]
enum ArnMember {
    GetApprovalTeam,
    UpdateApprovalTeam,
    StartActiveApprovalTeamDeletion,
    DeleteInactiveApprovalTeamVersion,
    ListSessions,
    StartApprovalTeamBaseline,
    GetSession,
    CancelSession,
    GetPolicyVersion,
    ListPolicyVersions,
    TagResource,
    UntagResource,
    ListTagsForResource,
    GetResourcePolicy,
}

impl ArnMember {
    /// The ARN grammar the MODEL declares for this member as a `@pattern`, or `None`
    /// when the member is a plain `String` and the service applies its own ARN rule in
    /// the handler instead. From
    /// `smithy_query(query="shape",shape="ApprovalTeamArn,SessionArn,QualifiedPolicyArn,UnqualifiedPolicyArn,TagResourceRequest,UntagResourceRequest,GetResourcePolicyRequest")`.
    fn modelled_pattern(self) -> Option<&'static str> {
        const TEAM: &str =
            "^arn:aws(-[^:]+)?:mpa:[a-z0-9-]{1,20}:[0-9]{12}:approval-team/[a-zA-Z0-9._-]+$";
        const SESSION: &str =
            "^arn:aws(-[^:]+)?:mpa:[a-z0-9-]{1,20}:[0-9]{12}:session/[a-zA-Z0-9._-]+/[a-zA-Z0-9_-]+$";
        const QUALIFIED_POLICY: &str =
            "^arn:.{1,63}:mpa:::aws:policy/[a-zA-Z0-9_\\.-]{1,1023}/[a-zA-Z0-9_\\.-]{1,1023}/(?:[\\d]+|\\$DEFAULT)$";
        const UNQUALIFIED_POLICY: &str =
            "^arn:.{1,63}:mpa:::aws:policy/[a-zA-Z0-9_\\.-]{1,1023}/[a-zA-Z0-9_\\.-]{1,1023}$";
        match self {
            Self::GetApprovalTeam
            | Self::UpdateApprovalTeam
            | Self::StartActiveApprovalTeamDeletion
            | Self::DeleteInactiveApprovalTeamVersion
            | Self::ListSessions
            | Self::StartApprovalTeamBaseline => Some(TEAM),
            Self::GetSession | Self::CancelSession => Some(SESSION),
            Self::GetPolicyVersion => Some(QUALIFIED_POLICY),
            Self::ListPolicyVersions => Some(UNQUALIFIED_POLICY),
            Self::TagResource
            | Self::UntagResource
            | Self::ListTagsForResource
            | Self::GetResourcePolicy => None,
        }
    }
}

/// Call the operation `member` belongs to, with `arn` in that member.
async fn call_with_arn(harness: &Harness, member: ArnMember, arn: &str) -> (u16, Value) {
    let label = encode_label(arn);
    match member {
        ArnMember::GetApprovalTeam => {
            harness.call("GET", &format!("/approval-teams/{label}"), None).await
        }
        ArnMember::UpdateApprovalTeam => {
            harness
                .call(
                    "PATCH",
                    &format!("/approval-teams/{label}"),
                    Some(json!({ "Description": "a new description" })),
                )
                .await
        }
        ArnMember::StartActiveApprovalTeamDeletion => {
            harness
                .call("POST", &format!("/approval-teams/{label}?Delete"), None)
                .await
        }
        ArnMember::DeleteInactiveApprovalTeamVersion => {
            harness
                .call("DELETE", &format!("/approval-teams/{label}/1"), None)
                .await
        }
        ArnMember::ListSessions => {
            harness
                .call("POST", &format!("/approval-teams/{label}/sessions/?List"), None)
                .await
        }
        ArnMember::StartApprovalTeamBaseline => {
            harness
                .call("POST", &format!("/approval-teams/{label}/baseline"), Some(json!({})))
                .await
        }
        ArnMember::GetSession => {
            harness.call("GET", &format!("/sessions/{label}"), None).await
        }
        ArnMember::CancelSession => {
            harness.call("PUT", &format!("/sessions/{label}"), None).await
        }
        ArnMember::GetPolicyVersion => {
            harness.call("GET", &format!("/policy-versions/{label}"), None).await
        }
        ArnMember::ListPolicyVersions => {
            harness.call("POST", &format!("/policies/{label}/?List"), None).await
        }
        ArnMember::TagResource => {
            harness
                .call("PUT", &format!("/tags/{label}"), Some(json!({ "Tags": tag_map(1) })))
                .await
        }
        ArnMember::UntagResource => {
            harness
                .call("POST", &format!("/tags/{label}"), Some(json!({ "TagKeys": ["key-000"] })))
                .await
        }
        ArnMember::ListTagsForResource => {
            harness.call("GET", &format!("/tags/{label}"), None).await
        }
        ArnMember::GetResourcePolicy => {
            get_resource_policy(harness, arn, SEEDED_POLICY.1, "AWS_MANAGED").await
        }
    }
}

/// Every ARN-carrying member of this service.
const ARN_MEMBERS: [ArnMember; 14] = [
    ArnMember::GetApprovalTeam,
    ArnMember::UpdateApprovalTeam,
    ArnMember::StartActiveApprovalTeamDeletion,
    ArnMember::DeleteInactiveApprovalTeamVersion,
    ArnMember::ListSessions,
    ArnMember::StartApprovalTeamBaseline,
    ArnMember::GetSession,
    ArnMember::CancelSession,
    ArnMember::GetPolicyVersion,
    ArnMember::ListPolicyVersions,
    ArnMember::TagResource,
    ArnMember::UntagResource,
    ArnMember::ListTagsForResource,
    ArnMember::GetResourcePolicy,
];

/// Values that are not Amazon Resource Names at all: they fail every ARN grammar, so
/// they are the malformed-ARN class for EVERY ARN-carrying member, whether or not the
/// model declares that member's pattern. Rows differ along which part of
/// `arn:<partition>:<service>:<region>:<account>:<resource>` is missing, empty or
/// mis-cased.
fn values_that_are_not_arns() -> Vec<(&'static str, String)> {
    let own = engine::emulator_account_id();
    vec![
        ("not an ARN at all", "not-an-arn-at-all".to_string()),
        ("the prefix alone", "arn".to_string()),
        ("a partition but nothing else", "arn:aws".to_string()),
        (
            "five fields, no resource field",
            format!("arn:aws:mpa:{REGION}:{own}"),
        ),
        (
            "an empty resource field",
            format!("arn:aws:mpa:{REGION}:{own}:"),
        ),
        (
            "an empty partition",
            format!("arn::mpa:{REGION}:{own}:approval-team/t"),
        ),
        (
            "an empty service",
            format!("arn:aws::{REGION}:{own}:approval-team/t"),
        ),
        (
            "the prefix in the wrong case",
            format!("ARN:aws:mpa:{REGION}:{own}:approval-team/t"),
        ),
    ]
}

/// Rule 3, part one: a value that is not an ARN at all earns 400 `ValidationException`
/// with this service's one malformed-ARN message, on every member that carries an ARN.
#[tokio::test]
async fn a_value_that_is_not_an_arn_is_the_malformed_arn_refusal_on_every_member() {
    let harness = Harness::new();

    let mut answers: BTreeSet<(u16, String, String)> = BTreeSet::new();
    for member in ARN_MEMBERS {
        for (row, value) in values_that_are_not_arns() {
            let case = format!("{member:?} / {row}");
            let (status, body) = call_with_arn(&harness, member, &value).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: wrong error code in {body:?}"
            );
            // The message is the service's constant: it names neither the member nor the
            // grammar, and echoes nothing the request carried.
            assert_eq!(
                message(&body),
                state::INVALID_ARN_MESSAGE,
                "case {case:?}: wrong malformed-ARN message in {body:?}"
            );
            answers.insert((status, error_code(&body), message(&body)));
        }
    }

    assert_eq!(
        answers.len(),
        1,
        "the malformed-ARN answer varied with the member or the value: {answers:?}"
    );
}

/// Rule 3, part two: a value that IS a well-formed ARN but not the MPA ARN the member's
/// `@pattern` declares is the same refusal — applied per member, because what counts as
/// that member's grammar is.
#[tokio::test]
async fn an_arn_outside_the_members_modelled_grammar_is_the_malformed_arn_refusal() {
    let harness = Harness::new();
    let own = engine::emulator_account_id();
    let team = expected_arn("some-team");
    let session = format!("arn:aws:mpa:{REGION}:{own}:session/some-team/0001");
    let unqualified = state::policy_arn(SEEDED_POLICY.0, SEEDED_POLICY.1);
    let qualified = seeded_attachment();

    // (member, case, a well-formed ARN that fails THAT member's modelled pattern)
    let rows: Vec<(ArnMember, &str, String)> = vec![
        (
            ArnMember::GetApprovalTeam,
            "another service's ARN",
            "arn:aws:iam::123456789012:policy/foo".to_string(),
        ),
        (
            ArnMember::GetApprovalTeam,
            "an MPA session ARN in a team member",
            session.clone(),
        ),
        (
            ArnMember::UpdateApprovalTeam,
            "an account field of the wrong width",
            format!("arn:aws:mpa:{REGION}:00000000000:approval-team/t"),
        ),
        (
            ArnMember::StartActiveApprovalTeamDeletion,
            "an upper-case region",
            format!("arn:aws:mpa:US-EAST-1:{own}:approval-team/t"),
        ),
        (
            ArnMember::DeleteInactiveApprovalTeamVersion,
            "a team name holding a character the pattern excludes",
            format!("arn:aws:mpa:{REGION}:{own}:approval-team/t@1"),
        ),
        (
            ArnMember::ListSessions,
            "a policy ARN in a team member",
            unqualified.clone(),
        ),
        (
            ArnMember::StartApprovalTeamBaseline,
            "an empty team name",
            format!("arn:aws:mpa:{REGION}:{own}:approval-team/"),
        ),
        (
            ArnMember::GetSession,
            "a team ARN in a session member",
            team.clone(),
        ),
        (
            ArnMember::CancelSession,
            "a session ARN with no session id segment",
            format!("arn:aws:mpa:{REGION}:{own}:session/some-team"),
        ),
        (
            ArnMember::GetPolicyVersion,
            "an unqualified policy ARN where a version is required",
            unqualified.clone(),
        ),
        (
            ArnMember::GetPolicyVersion,
            "another service's policy ARN",
            "arn:aws:iam::123456789012:policy/foo".to_string(),
        ),
        (
            ArnMember::GetPolicyVersion,
            "a version segment that is neither digits nor $DEFAULT",
            format!("{unqualified}/LATEST"),
        ),
        (
            ArnMember::ListPolicyVersions,
            "a qualified policy ARN where no version is allowed",
            qualified.clone(),
        ),
        (
            ArnMember::ListPolicyVersions,
            "an account-bearing policy ARN",
            format!("arn:aws:mpa::{own}:aws:policy/{}/{}", SEEDED_POLICY.0, SEEDED_POLICY.1),
        ),
    ];

    for (member, case, value) in rows {
        let case = format!("{member:?} / {case}");
        assert!(
            member.modelled_pattern().is_some(),
            "case {case:?}: this row is only about members whose ARN grammar the model declares"
        );
        let (status, body) = call_with_arn(&harness, member, &value).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: wrong error code in {body:?}"
        );
        assert_eq!(
            message(&body),
            state::INVALID_ARN_MESSAGE,
            "case {case:?}: wrong malformed-ARN message in {body:?}"
        );
    }
}

/// The predicate the refusal seam decides the ARN class with recognises the grammar of
/// EVERY ARN-valued member this model declares one for, and nothing else.
#[test]
fn every_modelled_arn_pattern_is_recognised_as_one() {
    for member in ARN_MEMBERS {
        if let Some(pattern) = member.modelled_pattern() {
            assert!(
                state::is_arn_pattern(pattern),
                "{member:?}: the ARN pattern {pattern:?} is not recognised as an ARN grammar"
            );
        }
    }

    // Patterns of members whose values are NOT ARNs keep the framework's own constraint
    // report: `ApprovalTeamName`'s `^[a-zA-Z0-9._-]+$` and `Description`'s, from
    // `smithy_query(query="shape",shape="ApprovalTeamName")`.
    for pattern in ["^[a-zA-Z0-9._-]+$", "^[\\S\\s]*$", "arn:aws:mpa:unanchored"] {
        assert!(
            !state::is_arn_pattern(pattern),
            "the non-ARN pattern {pattern:?} was taken for an ARN grammar"
        );
    }
}
