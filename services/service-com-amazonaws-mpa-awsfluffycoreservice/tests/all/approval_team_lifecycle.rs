// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the approval-team lifecycle: `CreateApprovalTeam`,
//! `GetApprovalTeam`, `ListApprovalTeams`, `UpdateApprovalTeam` and
//! `StartActiveApprovalTeamDeletion`.
//!
//! Every request goes over the wire the real client uses: a `restJson1` HTTP request
//! dispatched through the `MultiServiceRouter` this service is mounted in, so the
//! route, the URI bindings, the JSON envelope and the HTTP status are all exercised.
//!
//! The tables below span the class each rule generalises over (approver count, policy
//! count, description length, name form, page size, pending-window length), not just
//! one recorded shape. Where a table names a published bound (20 approvers, 10 policies,
//! a 256-character description, `MaxResults` 1..20) a comment names the modelled member
//! it comes from.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::ops::types::shared_types as model;
use service_com_amazonaws_mpa_awsfluffycoreservice::state;
use service_com_amazonaws_mpa_awsfluffycoreservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Region the test signs with, so the minted ARN's region segment is observable and is
/// NOT the `DEFAULT_REGION` fallback.
pub(crate) const REGION: &str = "us-west-2";

/// Slack over [`state::INVITATION_RESPONSE_DELAY`] before a clock-owed invitation
/// response is expected to have been materialised.
const CLOCK_SLACK: Duration = Duration::from_millis(250);

/// A policy ARN that satisfies `PolicyReference$PolicyArn`'s modelled pattern
/// `^arn:.{1,63}:mpa:::aws:policy/[a-zA-Z0-9_\.-]{1,1023}/[a-zA-Z0-9_\.-]{1,1023}/(?:[\d]+|\$DEFAULT)$`.
fn policy_arn(n: usize) -> String {
    if n.is_multiple_of(2) {
        format!("arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy{n}/$DEFAULT")
    } else {
        format!("arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy{n}/{n}")
    }
}

/// An approver: `(PrimaryIdentityId, PrimaryIdentitySourceArn)`. `IdentityId` is
/// `length(1..100)` with no pattern; `PrimaryIdentitySourceArn` is a plain string.
fn approver(n: usize) -> (String, String) {
    (
        format!("identity-{n:04}"),
        format!("arn:aws:mpa:{REGION}:{}:identity-source/ssoins-{n:04}", engine::emulator_account_id()),
    )
}

/// One row of the create/get round-trip table.
struct TeamCase {
    case: String,
    name: String,
    description: String,
    min_approvals: i64,
    approvers: Vec<(String, String)>,
    policies: Vec<String>,
    tags: Vec<(String, String)>,
}

/// The create/get table: rows differ along approver count, policy count, description
/// length, name form, `MinApprovalsRequired` and tag presence.
fn team_cases() -> Vec<TeamCase> {
    vec![
        TeamCase {
            case: "single approver, single policy, no tags".to_string(),
            name: "demo-team".to_string(),
            description: "demo".to_string(),
            min_approvals: 1,
            approvers: vec![approver(1)],
            policies: vec![policy_arn(1)],
            tags: Vec::new(),
        },
        TeamCase {
            case: "two approvers, M below N, tagged".to_string(),
            name: "Team.Alpha_1".to_string(),
            description: "alpha team".to_string(),
            min_approvals: 1,
            approvers: vec![approver(1), approver(2)],
            policies: vec![policy_arn(1), policy_arn(2)],
            tags: vec![
                ("env".to_string(), "test".to_string()),
                ("owner".to_string(), "platform".to_string()),
            ],
        },
        TeamCase {
            case: "three approvers, M equal to N".to_string(),
            name: "a".to_string(),
            // `Description` is `length(min: 1, max: 256)` — the minimum.
            description: "d".to_string(),
            min_approvals: 3,
            approvers: vec![approver(1), approver(2), approver(3)],
            policies: vec![policy_arn(3)],
            tags: Vec::new(),
        },
        TeamCase {
            case: "maximum approvers and policies, maximum description".to_string(),
            // `ApprovalTeamName` is `length(max: 64)`, pattern `^[a-zA-Z0-9._-]+$`.
            name: "n".repeat(64),
            // `Description` is `length(min: 1, max: 256)` — the maximum.
            description: "x".repeat(256),
            min_approvals: 20,
            // `Approvers` is `length(1..20)`; `Policies` is `length(1..10)`.
            approvers: (1..=20).map(approver).collect(),
            policies: (1..=10).map(policy_arn).collect(),
            tags: vec![("only".to_string(), String::new())],
        },
    ]
}

/// The service mounted in a router, plus the engine its store lives in.
pub(crate) struct Harness {
    router: MultiServiceRouter,
    pub(crate) engine: Arc<engine::InMemoryEngine>,
}

impl Harness {
    /// The harness these lifecycle tables use: the caller's own account recorded as
    /// the organization's management account, the only state in which
    /// `CreateApprovalTeam` is permitted. These tables are about what the lifecycle
    /// operations DO once the caller may create a team; the gate itself is tested in
    /// `authorization_gate.rs`.
    pub(crate) fn new() -> Self {
        let harness = Self::without_organization();
        harness.record_management_account(engine::emulator_account_id());
        harness
    }

    /// The service mounted in a router with the store left as it starts: no
    /// organization, and therefore no management account.
    pub(crate) fn without_organization() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signature verification is the emulator's, not this service's; these requests
        // carry a credential scope so they ROUTE, not a real signature.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Record `account_id` as the management account of the organization the store
    /// describes. No MPA operation records it, so a test that needs a created team
    /// sets it directly, as [`Harness::expire_deletion_window`] sets a due instant.
    pub(crate) fn record_management_account(&self, account_id: &str) {
        let store = state::service_state(&*self.engine).expect("the service store");
        store.lock().organization_management_account_id = Some(account_id.to_string());
    }

    /// Dispatch one request and return `(status, parsed JSON body)`.
    pub(crate) async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> (u16, Value) {
        let bytes = match &body {
            None => Bytes::new(),
            Some(value) => Bytes::from(serde_json::to_vec(value).expect("serialize the body")),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("mpa.{REGION}.amazonaws.com"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/mpa/aws4_request, \
                     SignedHeaders=host, Signature=abc"
                ),
            )
            .body(bytes)
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.into_body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    pub(crate) async fn create(&self, body: &Value) -> (u16, Value) {
        self.call("POST", "/approval-teams", Some(body.clone())).await
    }

    pub(crate) async fn get(&self, arn: &str) -> (u16, Value) {
        self.call("GET", &format!("/approval-teams/{}", encode_label(arn)), None)
            .await
    }

    pub(crate) async fn update(&self, arn: &str, body: &Value) -> (u16, Value) {
        self.call(
            "PATCH",
            &format!("/approval-teams/{}", encode_label(arn)),
            Some(body.clone()),
        )
        .await
    }

    pub(crate) async fn start_deletion(&self, arn: &str, body: Option<Value>) -> (u16, Value) {
        self.call(
            "POST",
            &format!("/approval-teams/{}?Delete", encode_label(arn)),
            body,
        )
        .await
    }

    pub(crate) async fn list(&self, query: &str) -> (u16, Value) {
        let uri = if query.is_empty() {
            "/approval-teams/?List".to_string()
        } else {
            format!("/approval-teams/?List&{query}")
        };
        self.call("POST", &uri, None).await
    }

    /// Force a stored team's pending deletion window to have already elapsed, so the
    /// clock-owed removal is due on the next call. Only the DUE INSTANT is moved; the
    /// removal itself is the service's own.
    fn expire_deletion_window(&self, arn: &str) {
        let store = state::service_state(&*self.engine).expect("the service store");
        let mut guard = store.lock();
        let team = guard.team_mut(arn).expect("a stored team to expire");
        team.deletion_completion_time = Some(SystemTime::now() - Duration::from_secs(1));
    }

    /// The tags recorded for a stored team, read straight out of the store. The wire
    /// reader (`ListTagsForResource`) is exercised in `resource_arn_reads.rs`; these
    /// tables assert what `CreateApprovalTeam` recorded.
    fn stored_tags(&self, arn: &str) -> Vec<(String, String)> {
        let store = state::service_state(&*self.engine).expect("the service store");
        let guard = store.lock();
        guard
            .team(arn)
            .expect("a stored team")
            .tags
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }
}

/// Percent-encode an ARN for a non-greedy `@httpLabel`, which binds ONE path segment.
pub(crate) fn encode_label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            ':' => "%3A".to_string(),
            '/' => "%2F".to_string(),
            '$' => "%24".to_string(),
            other => other.to_string(),
        })
        .collect()
}

fn create_body(case: &TeamCase, client_token: Option<&str>) -> Value {
    let mut body = json!({
        "Name": case.name,
        "Description": case.description,
        "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": case.min_approvals } },
        "Approvers": case
            .approvers
            .iter()
            .map(|(id, source)| json!({ "PrimaryIdentityId": id, "PrimaryIdentitySourceArn": source }))
            .collect::<Vec<_>>(),
        "Policies": case
            .policies
            .iter()
            .map(|arn| json!({ "PolicyArn": arn }))
            .collect::<Vec<_>>(),
    });
    if !case.tags.is_empty() {
        body["Tags"] = Value::Object(
            case.tags
                .iter()
                .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                .collect(),
        );
    }
    if let Some(token) = client_token {
        body["ClientToken"] = json!(token);
    }
    body
}

pub(crate) fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The error code a `restJson1` error envelope carries in `__type`.
pub(crate) fn error_code(body: &Value) -> String {
    text(body, "__type")
}

/// A timestamp member as epoch seconds, however the protocol rendered it
/// (`IsoTimestamp` is `@timestampFormat("date-time")`, but a number is read too).
pub(crate) fn instant(body: &Value, member: &str) -> f64 {
    match body.get(member) {
        Some(Value::Number(n)) => n.as_f64().unwrap_or_default(),
        Some(Value::String(s)) => {
            aws_smithy_types::DateTime::from_str(s, aws_smithy_types::date_time::Format::DateTime)
                .unwrap_or_else(|e| panic!("{member} is not a readable timestamp: {s:?} ({e})"))
                .as_secs_f64()
        }
        other => panic!("{member} is missing or not a timestamp: {other:?}"),
    }
}

/// The team ARN this service must mint for `name` in `REGION`.
pub(crate) fn expected_arn(name: &str) -> String {
    format!(
        "arn:aws:mpa:{REGION}:{}:approval-team/{name}",
        engine::emulator_account_id()
    )
}

/// Wait out a clock-owed invitation response.
fn wait_for_the_clock() {
    std::thread::sleep(state::INVITATION_RESPONSE_DELAY + CLOCK_SLACK);
}

/// Every status token this service writes is one the model declares. The catalogs come
/// from the generated `shared_types` value lists, which are rendered from the model's
/// `ApprovalTeamStatus` / `ApprovalTeamStatusCode` / `IdentityStatus` /
/// `ApproverLastActivity` enums.
#[test]
fn the_status_tokens_this_service_writes_are_ones_the_model_declares() {
    let statuses = [
        state::TeamStatus::Active,
        state::TeamStatus::Inactive,
        state::TeamStatus::Deleting,
        state::TeamStatus::Pending,
    ];
    for status in statuses {
        assert!(
            model::APPROVAL_TEAM_STATUS_VALUES.contains(&status.as_wire()),
            "case {status:?}: expected a token ApprovalTeamStatus declares, got {:?}",
            status.as_wire()
        );
    }

    for code in [
        state::TeamStatusCode::PendingActivation,
        state::TeamStatusCode::DeletePendingApproval,
    ] {
        assert!(
            model::APPROVAL_TEAM_STATUS_CODE_VALUES.contains(&code.as_wire()),
            "case {code:?}: expected a token ApprovalTeamStatusCode declares, got {:?}",
            code.as_wire()
        );
    }

    for identity in [
        state::ApproverIdentityStatus::Pending,
        state::ApproverIdentityStatus::Accepted,
        state::ApproverIdentityStatus::Rejected,
        state::ApproverIdentityStatus::Invalid,
    ] {
        assert!(
            model::IDENTITY_STATUS_VALUES.contains(&identity.as_wire()),
            "case {identity:?}: expected a token IdentityStatus declares, got {:?}",
            identity.as_wire()
        );
    }

    for activity in [
        state::ApproverLastActivity::Baselined,
        state::ApproverLastActivity::RespondedToInvitation,
        state::ApproverLastActivity::Voted,
    ] {
        assert!(
            model::APPROVER_LAST_ACTIVITY_VALUES.contains(&activity.as_wire()),
            "case {activity:?}: expected a token ApproverLastActivity declares, got {:?}",
            activity.as_wire()
        );
    }
}

/// A created team is STORED, and `GetApprovalTeam` reads every field back out of the
/// store — across the whole class of team shapes, not one of them.
#[tokio::test]
async fn create_then_get_round_trips_every_stored_field() {
    let harness = Harness::new();

    for case in team_cases() {
        let label = &case.case;
        let (status, created) = harness.create(&create_body(&case, None)).await;
        assert_eq!(status, 200, "case {label}: create returned {created:?}");
        assert_eq!(
            text(&created, "Arn"),
            expected_arn(&case.name),
            "case {label}: create minted the wrong Arn"
        );
        assert_eq!(
            text(&created, "Name"),
            case.name,
            "case {label}: create echoed the wrong Name"
        );
        assert!(
            !text(&created, "VersionId").is_empty(),
            "case {label}: create returned no VersionId: {created:?}"
        );
        let creation_time = instant(&created, "CreationTime");

        let arn = text(&created, "Arn");
        let (status, team) = harness.get(&arn).await;
        assert_eq!(status, 200, "case {label}: get returned {team:?}");
        assert_eq!(text(&team, "Arn"), arn, "case {label}: get echoed a different Arn");
        assert_eq!(text(&team, "Name"), case.name, "case {label}: wrong Name");
        assert_eq!(
            text(&team, "Description"),
            case.description,
            "case {label}: wrong Description"
        );
        assert_eq!(
            text(&team, "VersionId"),
            text(&created, "VersionId"),
            "case {label}: get reported a different VersionId than create"
        );
        assert!(
            (instant(&team, "CreationTime") - creation_time).abs() < 1.0,
            "case {label}: get reported CreationTime {} for created {creation_time}",
            instant(&team, "CreationTime")
        );
        assert_eq!(
            team["ApprovalStrategy"]["MofN"]["MinApprovalsRequired"].as_i64(),
            Some(case.min_approvals),
            "case {label}: wrong ApprovalStrategy: {:?}",
            team["ApprovalStrategy"]
        );
        assert_eq!(
            team["NumberOfApprovers"].as_u64(),
            Some(case.approvers.len() as u64),
            "case {label}: wrong NumberOfApprovers"
        );
        assert_eq!(
            text(&team, "Status"),
            state::TeamStatus::Pending.as_wire(),
            "case {label}: a team whose approvers have not answered must be PENDING"
        );
        assert_eq!(
            text(&team, "StatusCode"),
            state::TeamStatusCode::PendingActivation.as_wire(),
            "case {label}: wrong StatusCode"
        );
        assert!(
            team.get("LastUpdateTime").is_none(),
            "case {label}: a team that was never updated has no LastUpdateTime: {team:?}"
        );

        let policies: Vec<String> = team["Policies"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|p| text(p, "PolicyArn"))
            .collect();
        assert_eq!(policies, case.policies, "case {label}: wrong Policies");

        let approvers = team["Approvers"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            approvers.len(),
            case.approvers.len(),
            "case {label}: wrong number of Approvers: {approvers:?}"
        );
        let mut ids = BTreeSet::new();
        for (index, (expected, actual)) in case.approvers.iter().zip(&approvers).enumerate() {
            assert_eq!(
                text(actual, "PrimaryIdentityId"),
                expected.0,
                "case {label}: approver {index} has the wrong PrimaryIdentityId"
            );
            assert_eq!(
                text(actual, "PrimaryIdentitySourceArn"),
                expected.1,
                "case {label}: approver {index} has the wrong PrimaryIdentitySourceArn"
            );
            assert_eq!(
                text(actual, "PrimaryIdentityStatus"),
                state::ApproverIdentityStatus::Pending.as_wire(),
                "case {label}: approver {index} has not been invited yet, so it is PENDING"
            );
            let id = text(actual, "ApproverId");
            assert!(
                !id.is_empty(),
                "case {label}: approver {index} was minted no ApproverId"
            );
            assert!(
                ids.insert(id.clone()),
                "case {label}: ApproverId {id:?} was minted twice"
            );
        }

        assert_eq!(
            harness.stored_tags(&arn),
            case.tags,
            "case {label}: the stored Tags differ from the request"
        );
    }
}

/// A team is `PENDING` until its approvers answer their invitations, and `ACTIVE` once
/// they all have. The response the clock owes is stamped with the instant it was DUE, so
/// it does not move between reads.
#[tokio::test]
async fn an_invited_team_becomes_active_once_its_approvers_respond() {
    let harness = Harness::new();

    // One row per approver count, so activation is not keyed to a single team shape.
    for approver_count in [1_usize, 2, 5] {
        let case = TeamCase {
            case: format!("{approver_count} approvers"),
            name: format!("activation-{approver_count}"),
            description: "activation".to_string(),
            min_approvals: 1,
            approvers: (1..=approver_count).map(approver).collect(),
            policies: vec![policy_arn(1)],
            tags: Vec::new(),
        };
        let label = case.case.clone();
        let (status, created) = harness.create(&create_body(&case, None)).await;
        assert_eq!(status, 200, "case {label}: create returned {created:?}");
        let arn = text(&created, "Arn");

        let (_, before) = harness.get(&arn).await;
        assert_eq!(
            text(&before, "Status"),
            state::TeamStatus::Pending.as_wire(),
            "case {label}: a caller reading straight back must still see PENDING"
        );

        wait_for_the_clock();

        let (status, after) = harness.get(&arn).await;
        assert_eq!(status, 200, "case {label}: get returned {after:?}");
        assert_eq!(
            text(&after, "Status"),
            state::TeamStatus::Active.as_wire(),
            "case {label}: every approver has answered, so the team is ACTIVE"
        );
        assert!(
            after.get("StatusCode").is_none(),
            "case {label}: an ACTIVE team carries no status code: {after:?}"
        );

        let approvers = after["Approvers"].as_array().cloned().unwrap_or_default();
        let mut response_times = Vec::new();
        for (index, approver) in approvers.iter().enumerate() {
            assert_eq!(
                text(approver, "PrimaryIdentityStatus"),
                state::ApproverIdentityStatus::Accepted.as_wire(),
                "case {label}: approver {index} answered, so it is ACCEPTED"
            );
            assert_eq!(
                text(approver, "LastActivity"),
                state::ApproverLastActivity::RespondedToInvitation.as_wire(),
                "case {label}: approver {index} has the wrong LastActivity"
            );
            response_times.push(instant(approver, "ResponseTime"));
        }

        // Stable across reads: a second read reports the same response instants.
        let (_, again) = harness.get(&arn).await;
        let repeated: Vec<f64> = again["Approvers"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|a| instant(a, "ResponseTime"))
            .collect();
        assert_eq!(
            repeated, response_times,
            "case {label}: a materialised ResponseTime must not move between reads"
        );
    }
}

/// `ClientToken` replay returns the team the first call minted; a `Name` a live team
/// already holds conflicts; distinct names mint distinct teams.
#[tokio::test]
async fn client_token_replay_returns_the_same_team_and_a_duplicate_name_conflicts() {
    let harness = Harness::new();

    // Tokens differ in form (short, long, punctuated) — `Token` is `length(max: 4096)`
    // with no pattern.
    for token in ["t1", "a-much-longer-client-token-value", &"z".repeat(4096)] {
        let label = format!("token len {}", token.len());
        let case = TeamCase {
            case: label.clone(),
            name: format!("idem-{}", token.len()),
            description: "idempotency".to_string(),
            min_approvals: 1,
            approvers: vec![approver(1)],
            policies: vec![policy_arn(1)],
            tags: Vec::new(),
        };
        let body = create_body(&case, Some(token));

        let (status, first) = harness.create(&body).await;
        assert_eq!(status, 200, "case {label}: first create returned {first:?}");
        let (status, replay) = harness.create(&body).await;
        assert_eq!(status, 200, "case {label}: replay returned {replay:?}");
        assert_eq!(
            (text(&replay, "Arn"), text(&replay, "VersionId")),
            (text(&first, "Arn"), text(&first, "VersionId")),
            "case {label}: a replayed ClientToken must answer with the first team"
        );

        // A DIFFERENT token of the same length for the same name is a second team,
        // which conflicts.
        let other_token = format!("o{}", &token[1..]);
        let (status, conflict) = harness
            .create(&create_body(&case, Some(&other_token)))
            .await;
        assert_eq!(
            status, 409,
            "case {label}: a duplicate Name must conflict, got {conflict:?}"
        );
        assert_eq!(
            error_code(&conflict),
            "ConflictException",
            "case {label}: wrong error code for a duplicate Name"
        );
    }

    // Exactly one team per distinct name, and no replay created a second one.
    let (status, listed) = harness.list("MaxResults=20").await;
    assert_eq!(status, 200, "list returned {listed:?}");
    let names: Vec<String> = listed["ApprovalTeams"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|t| text(t, "Name"))
        .collect();
    assert_eq!(
        names,
        vec!["idem-2", "idem-32", "idem-4096"],
        "each distinct Name must appear exactly once"
    );
}

/// An `MofN` strategy that asks for more approvals (M) than the team has approvers (N)
/// can never be satisfied, so it is refused — at create and at update alike.
#[tokio::test]
async fn m_of_n_must_not_exceed_the_approver_count() {
    // (approver count, MinApprovalsRequired, accepted?)
    let rows: &[(usize, i64, bool)] = &[
        (1, 1, true),
        (2, 1, true),
        (3, 3, true),
        (20, 20, true),
        (1, 2, false),
        (2, 3, false),
        (3, 4, false),
        (19, 20, false),
    ];

    for (index, &(approver_count, min_approvals, accepted)) in rows.iter().enumerate() {
        let harness = Harness::new();
        let label = format!("{min_approvals} of {approver_count}");
        let case = TeamCase {
            case: label.clone(),
            name: format!("mofn-{index}"),
            description: "m of n".to_string(),
            min_approvals,
            approvers: (1..=approver_count).map(approver).collect(),
            policies: vec![policy_arn(1)],
            tags: Vec::new(),
        };
        let (status, body) = harness.create(&create_body(&case, None)).await;
        if accepted {
            assert_eq!(status, 200, "case {label}: create returned {body:?}");
        } else {
            assert_eq!(
                status, 400,
                "case {label}: an unsatisfiable strategy must be refused, got {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {label}: wrong error code"
            );
            continue;
        }

        // The same rule holds for the set an UPDATE produces: raising M past the
        // existing approver count is refused, and so is cutting N below M.
        let arn = text(&body, "Arn");
        let (status, refused) = harness
            .update(
                &arn,
                &json!({ "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": approver_count as i64 + 1 } } }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {label}: update raising M past N must be refused, got {refused:?}"
        );
        if approver_count > 1 {
            let (status, refused) = harness
                .update(
                    &arn,
                    &json!({
                        "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": approver_count as i64 } },
                        "Approvers": [{ "PrimaryIdentityId": approver(1).0, "PrimaryIdentitySourceArn": approver(1).1 }],
                    }),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {label}: update cutting N below M must be refused, got {refused:?}"
            );
        }
    }
}

/// A well-formed ARN no stored team carries is a 404, never a 501 and never an empty 200.
#[tokio::test]
async fn an_arn_no_team_carries_is_not_found() {
    let harness = Harness::new();
    let account = engine::emulator_account_id();

    let arns = [
        expected_arn("never-created"),
        format!("arn:aws:mpa:eu-west-1:{account}:approval-team/other-region"),
        format!("arn:aws-us-gov:mpa:us-gov-west-1:{account}:approval-team/other-partition"),
        format!("arn:aws:mpa:{REGION}:{account}:approval-team/{}", "n".repeat(64)),
    ];

    for arn in &arns {
        let (status, body) = harness.get(arn).await;
        assert_eq!(status, 404, "case {arn}: get returned {body:?}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {arn}: wrong error code"
        );

        let (status, body) = harness
            .update(arn, &json!({ "Description": "no such team" }))
            .await;
        assert_eq!(status, 404, "case {arn}: update returned {body:?}");

        let (status, body) = harness.start_deletion(arn, None).await;
        assert_eq!(status, 404, "case {arn}: start deletion returned {body:?}");
    }
}

/// Paging walks the stored teams in creation order and shows every one exactly once, for
/// every page size `MaxResults` allows. `MaxResults` is modelled `range(1..20)` with
/// `default: 20`.
#[tokio::test]
async fn list_pages_over_every_team_exactly_once() {
    let harness = Harness::new();
    let total = 7;
    let mut expected = Vec::new();
    for n in 1..=total {
        let case = TeamCase {
            case: format!("team {n}"),
            name: format!("page-team-{n:02}"),
            description: format!("team {n}"),
            min_approvals: 1,
            approvers: vec![approver(n)],
            policies: vec![policy_arn(n)],
            tags: Vec::new(),
        };
        let (status, created) = harness.create(&create_body(&case, None)).await;
        assert_eq!(status, 200, "creating team {n} returned {created:?}");
        expected.push(text(&created, "Arn"));
    }

    // Each published bound (1 and 20), the sizes between, and one past the maximum.
    for page_size in [1_i64, 2, 3, 6, 7, 20] {
        let label = format!("MaxResults={page_size}");
        let mut seen = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                None => format!("MaxResults={page_size}"),
                Some(t) => format!("MaxResults={page_size}&NextToken={t}"),
            };
            let (status, body) = harness.list(&query).await;
            assert_eq!(status, 200, "case {label}: list returned {body:?}");
            let page = body["ApprovalTeams"].as_array().cloned().unwrap_or_default();
            assert!(
                page.len() as i64 <= page_size,
                "case {label}: a page of {} exceeds MaxResults",
                page.len()
            );
            for team in &page {
                seen.push(text(team, "Arn"));
                assert_eq!(
                    text(team, "Status"),
                    state::TeamStatus::Pending.as_wire(),
                    "case {label}: a listed team reports its stored Status"
                );
                assert!(
                    team["NumberOfApprovers"].as_u64() == Some(1),
                    "case {label}: a listed team reports its approver count: {team:?}"
                );
            }
            pages += 1;
            assert!(pages <= total + 1, "case {label}: paging did not terminate");
            match body.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case {label}: paging must yield every team exactly once, in creation order"
        );
    }

    // `MaxResults` omitted takes the modelled default of 20, which holds all 7.
    let (status, body) = harness.list("").await;
    assert_eq!(status, 200, "default page returned {body:?}");
    assert_eq!(
        body["ApprovalTeams"].as_array().map(Vec::len),
        Some(total),
        "the default MaxResults of 20 must hold every team: {body:?}"
    );
    assert!(
        body.get("NextToken").is_none(),
        "a complete page carries no NextToken: {body:?}"
    );

    // A token this service never issued is refused, whatever its form.
    // `%20` is a percent-encoded space: the query decoder hands the handler " ".
    for bad in ["not-a-token", "12x", "-1", "%20"] {
        let (status, body) = harness.list(&format!("NextToken={bad}")).await;
        assert_eq!(
            status, 400,
            "case {bad:?}: a foreign NextToken must be refused, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {bad:?}: wrong error code"
        );
    }
}

/// `UpdateApprovalTeam` applies the fields it is given, records a new version, and a
/// following `GetApprovalTeam` reports both.
#[tokio::test]
async fn update_applies_the_supplied_fields_and_records_a_new_version() {
    /// (label, patch body, expected description / min approvals / approver ids after)
    struct UpdateCase {
        label: &'static str,
        patch: Value,
        description: &'static str,
        min_approvals: i64,
        approver_ids: Vec<String>,
        version_changes: bool,
    }

    let harness = Harness::new();
    let base = TeamCase {
        case: "update".to_string(),
        name: "update-team".to_string(),
        description: "before".to_string(),
        min_approvals: 2,
        approvers: vec![approver(1), approver(2), approver(3)],
        policies: vec![policy_arn(1)],
        tags: Vec::new(),
    };
    let (status, created) = harness.create(&create_body(&base, None)).await;
    assert_eq!(status, 200, "create returned {created:?}");
    let arn = text(&created, "Arn");
    let (_, first) = harness.get(&arn).await;
    let original_ids: Vec<String> = first["Approvers"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|a| text(a, "ApproverId"))
        .collect();

    let cases = vec![
        UpdateCase {
            label: "description only",
            patch: json!({ "Description": "after" }),
            description: "after",
            min_approvals: 2,
            approver_ids: original_ids.clone(),
            version_changes: true,
        },
        UpdateCase {
            label: "strategy only",
            patch: json!({ "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 3 } } }),
            description: "after",
            min_approvals: 3,
            approver_ids: original_ids.clone(),
            version_changes: true,
        },
        UpdateCase {
            label: "approvers reordered keeps every ApproverId",
            patch: json!({
                "Approvers": [
                    { "PrimaryIdentityId": approver(3).0, "PrimaryIdentitySourceArn": approver(3).1 },
                    { "PrimaryIdentityId": approver(2).0, "PrimaryIdentitySourceArn": approver(2).1 },
                    { "PrimaryIdentityId": approver(1).0, "PrimaryIdentitySourceArn": approver(1).1 },
                ],
            }),
            description: "after",
            min_approvals: 3,
            approver_ids: vec![
                original_ids[2].clone(),
                original_ids[1].clone(),
                original_ids[0].clone(),
            ],
            version_changes: true,
        },
        UpdateCase {
            label: "every field at once",
            patch: json!({
                "Description": "final",
                "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 1 } },
                "Approvers": [
                    { "PrimaryIdentityId": approver(1).0, "PrimaryIdentitySourceArn": approver(1).1 },
                ],
            }),
            description: "final",
            min_approvals: 1,
            approver_ids: vec![original_ids[0].clone()],
            version_changes: true,
        },
        UpdateCase {
            label: "nothing to apply leaves the version alone",
            patch: json!({ "UpdateActions": ["SYNCHRONIZE_MFA_DEVICES"] }),
            description: "final",
            min_approvals: 1,
            approver_ids: vec![original_ids[0].clone()],
            version_changes: false,
        },
    ];

    let mut previous_version = text(&created, "VersionId");
    for case in cases {
        let label = case.label;
        let (status, updated) = harness.update(&arn, &case.patch).await;
        assert_eq!(status, 200, "case {label}: update returned {updated:?}");
        let version = text(&updated, "VersionId");
        if case.version_changes {
            assert_ne!(
                version, previous_version,
                "case {label}: an applied update must record a NEW VersionId"
            );
        } else {
            assert_eq!(
                version, previous_version,
                "case {label}: an update that applies nothing keeps the VersionId"
            );
        }

        let (status, team) = harness.get(&arn).await;
        assert_eq!(status, 200, "case {label}: get returned {team:?}");
        assert_eq!(
            text(&team, "VersionId"),
            version,
            "case {label}: get must report the VersionId update returned"
        );
        assert_eq!(
            text(&team, "Description"),
            case.description,
            "case {label}: wrong Description after the update"
        );
        assert_eq!(
            team["ApprovalStrategy"]["MofN"]["MinApprovalsRequired"].as_i64(),
            Some(case.min_approvals),
            "case {label}: wrong ApprovalStrategy after the update"
        );
        let ids: Vec<String> = team["Approvers"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|a| text(a, "ApproverId"))
            .collect();
        assert_eq!(
            ids, case.approver_ids,
            "case {label}: a retained approver keeps its ApproverId"
        );
        if case.version_changes {
            assert!(
                team.get("LastUpdateTime").is_some(),
                "case {label}: an applied update records a LastUpdateTime: {team:?}"
            );
        }
        previous_version = version;
    }

    // An approver the update ADDS is newly invited, so it is PENDING with a fresh id.
    let (status, updated) = harness
        .update(
            &arn,
            &json!({
                "Approvers": [
                    { "PrimaryIdentityId": approver(1).0, "PrimaryIdentitySourceArn": approver(1).1 },
                    { "PrimaryIdentityId": approver(9).0, "PrimaryIdentitySourceArn": approver(9).1 },
                ],
            }),
        )
        .await;
    assert_eq!(status, 200, "adding an approver returned {updated:?}");
    let (_, team) = harness.get(&arn).await;
    let approvers = team["Approvers"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        text(&approvers[0], "ApproverId"),
        original_ids[0],
        "the retained approver keeps its id: {team:?}"
    );
    assert_ne!(
        text(&approvers[1], "ApproverId"),
        original_ids[0],
        "the added approver is minted a fresh id: {team:?}"
    );
    assert_eq!(
        text(&approvers[1], "PrimaryIdentityStatus"),
        state::ApproverIdentityStatus::Pending.as_wire(),
        "a newly invited approver is PENDING: {team:?}"
    );
    assert_eq!(
        team["NumberOfApprovers"].as_u64(),
        Some(2),
        "NumberOfApprovers follows the stored approver list: {team:?}"
    );
}

/// `StartActiveApprovalTeamDeletion` records the pending window on the stored team, and
/// once the window elapses the team is gone from both readers.
#[tokio::test]
async fn deletion_records_the_pending_window_then_the_clock_removes_the_team() {
    // `PendingWindowDays` has no modelled range; it is a count of days, so the rule is
    // "the window is that many days" for every value this service can record, and the
    // modelled default applies when the member is absent. The last row is the largest
    // window this service records (`state::MAX_PENDING_WINDOW_DAYS`).
    let rows: &[(Option<i64>, i64)] = &[
        (None, i64::from(state::DEFAULT_PENDING_WINDOW_DAYS)),
        (Some(1), 1),
        (Some(7), 7),
        (Some(30), 30),
        (Some(365), 365),
        (
            Some(i64::from(state::MAX_PENDING_WINDOW_DAYS)),
            i64::from(state::MAX_PENDING_WINDOW_DAYS),
        ),
    ];

    for (index, &(requested, expected_days)) in rows.iter().enumerate() {
        let harness = Harness::new();
        let label = format!("PendingWindowDays={requested:?}");
        let case = TeamCase {
            case: label.clone(),
            name: format!("delete-team-{index}"),
            description: "deletion".to_string(),
            min_approvals: 1,
            approvers: vec![approver(1)],
            policies: vec![policy_arn(1)],
            tags: Vec::new(),
        };
        let (status, created) = harness.create(&create_body(&case, None)).await;
        assert_eq!(status, 200, "case {label}: create returned {created:?}");
        let arn = text(&created, "Arn");

        // A team whose approvers have not answered is not ACTIVE, so its ACTIVE
        // deletion cannot be started.
        let (status, refused) = harness.start_deletion(&arn, None).await;
        assert_eq!(
            status, 409,
            "case {label}: a PENDING team cannot start ACTIVE deletion, got {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {label}: wrong error code"
        );

        wait_for_the_clock();

        let body = requested.map(|days| json!({ "PendingWindowDays": days }));
        let (status, deletion) = harness.start_deletion(&arn, body.clone()).await;
        assert_eq!(status, 200, "case {label}: start deletion returned {deletion:?}");
        let start = instant(&deletion, "DeletionStartTime");
        let completion = instant(&deletion, "DeletionCompletionTime");
        let expected_seconds = (expected_days * 24 * 60 * 60) as f64;
        assert!(
            (completion - start - expected_seconds).abs() < 1.0,
            "case {label}: the window must be {expected_days} days ({expected_seconds}s), \
             got {}s",
            completion - start
        );

        let (status, team) = harness.get(&arn).await;
        assert_eq!(status, 200, "case {label}: get returned {team:?}");
        assert_eq!(
            text(&team, "Status"),
            state::TeamStatus::Deleting.as_wire(),
            "case {label}: the team must report DELETING"
        );
        assert_eq!(
            text(&team, "StatusCode"),
            state::TeamStatusCode::DeletePendingApproval.as_wire(),
            "case {label}: wrong StatusCode"
        );

        // `@idempotent`: a repeat call answers with the window already recorded.
        let (status, repeat) = harness.start_deletion(&arn, body).await;
        assert_eq!(status, 200, "case {label}: the repeat returned {repeat:?}");
        assert!(
            (instant(&repeat, "DeletionStartTime") - start).abs() < f64::EPSILON
                && (instant(&repeat, "DeletionCompletionTime") - completion).abs() < f64::EPSILON,
            "case {label}: a repeat must not restart the window: {repeat:?}"
        );

        // A team in the pending window cannot be updated.
        let (status, conflict) = harness
            .update(&arn, &json!({ "Description": "too late" }))
            .await;
        assert_eq!(
            status, 409,
            "case {label}: updating a DELETING team must conflict, got {conflict:?}"
        );

        // Once the window has elapsed the team is gone from BOTH readers.
        harness.expire_deletion_window(&arn);
        let (status, gone) = harness.get(&arn).await;
        assert_eq!(
            status, 404,
            "case {label}: the team must be gone after the window, got {gone:?}"
        );
        let (status, listed) = harness.list("MaxResults=20").await;
        assert_eq!(status, 200, "case {label}: list returned {listed:?}");
        assert_eq!(
            listed["ApprovalTeams"].as_array().map(Vec::len),
            Some(0),
            "case {label}: list must omit a removed team: {listed:?}"
        );

        // And the name is free again.
        let (status, recreated) = harness.create(&create_body(&case, None)).await;
        assert_eq!(
            status, 200,
            "case {label}: the name must be reusable once the team is gone, got {recreated:?}"
        );
    }
}

/// A `PendingWindowDays` outside the window this service can record names no completion
/// instant it could report, so it is refused — at both ends, and without panicking or
/// moving the team. The model gives the member no `range` to close either end.
#[tokio::test]
async fn a_pending_window_this_service_cannot_record_is_refused() {
    let harness = Harness::new();
    let case = TeamCase {
        case: "window bounds".to_string(),
        name: "window-team".to_string(),
        description: "window".to_string(),
        min_approvals: 1,
        approvers: vec![approver(1)],
        policies: vec![policy_arn(1)],
        tags: Vec::new(),
    };
    let (status, created) = harness.create(&create_body(&case, None)).await;
    assert_eq!(status, 200, "create returned {created:?}");
    let arn = text(&created, "Arn");
    wait_for_the_clock();

    // Each published bound and one past it, at both ends, plus the integer extremes that
    // would overflow an unchecked day-to-instant conversion.
    let refused = [
        0_i64,
        -1,
        -365,
        i64::from(state::MAX_PENDING_WINDOW_DAYS) + 1,
        i64::from(i32::MAX),
        i64::from(i32::MIN),
    ];
    for days in refused {
        let (status, body) = harness
            .start_deletion(&arn, Some(json!({ "PendingWindowDays": days })))
            .await;
        assert_eq!(
            status, 400,
            "case PendingWindowDays={days}: must be refused, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case PendingWindowDays={days}: wrong error code"
        );

        // Every refusal leaves the team exactly as it was: still ACTIVE, no window.
        let (status, team) = harness.get(&arn).await;
        assert_eq!(status, 200, "case PendingWindowDays={days}: get returned {team:?}");
        assert_eq!(
            text(&team, "Status"),
            state::TeamStatus::Active.as_wire(),
            "case PendingWindowDays={days}: a refused deletion must not move the status: {team:?}"
        );
        assert!(
            team.get("StatusCode").is_none(),
            "case PendingWindowDays={days}: a refused deletion records no status code: {team:?}"
        );
    }

    // One day is the smallest window that names one, and it is accepted.
    let (status, accepted) = harness
        .start_deletion(&arn, Some(json!({ "PendingWindowDays": 1 })))
        .await;
    assert_eq!(status, 200, "a one-day window must be accepted, got {accepted:?}");
}

/// One identity cannot hold two seats on a team: a repeated approver would inflate the
/// approver count (N) that `MofN` is measured against without adding anyone who can
/// approve, so it is refused at create and at update alike.
#[tokio::test]
async fn an_identity_cannot_hold_two_seats_on_a_team() {
    let harness = Harness::new();

    // Rows differ in where the repeat sits and how many distinct identities surround it.
    let repeated: &[(&str, Vec<usize>)] = &[
        ("the same identity twice", vec![1, 1]),
        ("a repeat at the end", vec![1, 2, 2]),
        ("a repeat across the list", vec![1, 2, 3, 1]),
    ];

    for (index, (label, identities)) in repeated.iter().enumerate() {
        let case = TeamCase {
            case: (*label).to_string(),
            name: format!("dup-{index}"),
            description: "duplicates".to_string(),
            min_approvals: 1,
            approvers: identities.iter().copied().map(approver).collect(),
            policies: vec![policy_arn(1)],
            tags: Vec::new(),
        };
        let (status, body) = harness.create(&create_body(&case, None)).await;
        assert_eq!(
            status, 400,
            "case {label}: a repeated approver must be refused, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {label}: wrong error code"
        );
    }

    // Distinct identities from the SAME source, and the same identity id from DIFFERENT
    // sources, are different approvers and are accepted.
    let distinct = TeamCase {
        case: "distinct identities".to_string(),
        name: "dup-none".to_string(),
        description: "duplicates".to_string(),
        min_approvals: 2,
        approvers: vec![approver(1), approver(2), (approver(1).0, approver(3).1)],
        policies: vec![policy_arn(1)],
        tags: Vec::new(),
    };
    let (status, created) = harness.create(&create_body(&distinct, None)).await;
    assert_eq!(status, 200, "distinct identities must be accepted: {created:?}");
    let arn = text(&created, "Arn");

    // The same rule holds for an update.
    let (status, refused) = harness
        .update(
            &arn,
            &json!({
                "Approvers": [
                    { "PrimaryIdentityId": approver(1).0, "PrimaryIdentitySourceArn": approver(1).1 },
                    { "PrimaryIdentityId": approver(1).0, "PrimaryIdentitySourceArn": approver(1).1 },
                ],
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "an update repeating an approver must be refused, got {refused:?}"
    );
    let (_, team) = harness.get(&arn).await;
    assert_eq!(
        team["NumberOfApprovers"].as_u64(),
        Some(3),
        "a refused update must leave the approver set alone: {team:?}"
    );
}

/// The store is bounded: once it holds `MAX_APPROVAL_TEAMS` teams, another create earns
/// the modelled `ServiceQuotaExceededException` rather than growing the process.
#[tokio::test]
async fn the_store_refuses_to_grow_past_its_team_quota() {
    let harness = Harness::new();

    // Fill the store through the state the handlers share, so the test does not have to
    // make MAX_APPROVAL_TEAMS requests to reach the ceiling the handler enforces.
    {
        let store = state::service_state(&*harness.engine).expect("the service store");
        let mut guard = store.lock();
        for n in 0..state::MAX_APPROVAL_TEAMS {
            let name = format!("quota-filler-{n}");
            let seq = guard.next_team_seq();
            let arn = state::team_arn(REGION, &name);
            guard.insert_team(state::ApprovalTeam {
                arn,
                name,
                description: "filler".to_string(),
                approval_strategy: state::StoredApprovalStrategy::MofN {
                    min_approvals_required: 1,
                },
                approvers: Vec::new(),
                policies: vec![policy_arn(1)],
                version_id: "1".to_string(),
                version: 1,
                status: state::TeamStatus::Active,
                status_code: None,
                status_message: None,
                creation_time: SystemTime::now(),
                last_update_time: None,
                deletion_start_time: None,
                deletion_completion_time: None,
                tags: std::collections::BTreeMap::new(),
                seq,
            });
        }
    }

    let case = TeamCase {
        case: "past the quota".to_string(),
        name: "one-too-many".to_string(),
        description: "quota".to_string(),
        min_approvals: 1,
        approvers: vec![approver(1)],
        policies: vec![policy_arn(1)],
        tags: Vec::new(),
    };
    let (status, body) = harness.create(&create_body(&case, None)).await;
    assert_eq!(
        status, 402,
        "a create past the quota must be refused, got {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "wrong error code past the quota"
    );

    // And nothing was stored for the refused request.
    let (status, missing) = harness.get(&state::team_arn(REGION, &case.name)).await;
    assert_eq!(
        status, 404,
        "a refused create must store nothing, got {missing:?}"
    );
}
