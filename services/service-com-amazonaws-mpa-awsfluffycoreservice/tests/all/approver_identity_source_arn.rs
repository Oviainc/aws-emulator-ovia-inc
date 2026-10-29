// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the identity source an approver authenticates through, on BOTH
//! write paths that take an `ApprovalTeamRequestApprover`: `CreateApprovalTeam$Approvers`
//! and `UpdateApprovalTeam$Approvers`.
//!
//! The rule under test: `PrimaryIdentitySourceArn` is documented to carry the Amazon
//! Resource Name of the identity source an approver authenticates through, and the model
//! gives it no ARN `@pattern` (`String`, `@length(max: 1000)`, from
//! `smithy_query(query="shape",shape="ApprovalTeamRequestApprover")`), so the handler is
//! where that class is decided —
//!
//! * a value that is not an ARN this service could have minted for an identity source
//!   addresses no identity source at all, so it is the malformed-ARN class this service
//!   answers with one sentence ([`state::INVALID_ARN_MESSAGE`], `ValidationException`,
//!   400), and nothing is written: no team is created and the team a refused update
//!   addressed keeps its version and its approvers;
//! * a value that IS one is recorded and reported back, member for member, by
//!   `GetApprovalTeam` — off the team itself when the change is applied where it was
//!   asked for, and off `PendingUpdate` while the team's approvers are being asked to
//!   approve it.
//!
//! The tables below span the class each rule generalises over rather than one request: for
//! the refusals, which part of the grammar
//! `arn:<partition>:mpa:<region>:<account>:identity-source/<id>` the value misses, which
//! service namespace and which resource kind it names, the length of the value and where
//! in the approver list it sits; for the accepted values, the partition, the region, the
//! form of the resource id, and the length of the ARN up to the member's published bound.
//! Where a row names a published bound or grammar, a comment names the modelled member it
//! comes from.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the `POST /approval-teams` and
//! `PATCH /approval-teams/{Arn}` routes, the JSON envelope and the HTTP status are all
//! exercised.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_team_lifecycle::{error_code, text, Harness, REGION};
use crate::identity_source_deletion::create_source;
use crate::pending_team_update::postpone_approval;

/// Slack over a clock-owed span before the transition it owes is expected to have been
/// materialised.
const CLOCK_SLACK: Duration = Duration::from_millis(250);

/// A policy ARN the seeded catalog holds, in the form `PolicyReference$PolicyArn`'s
/// modelled pattern requires.
const POLICY_ARN: &str = "arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy/$DEFAULT";

/// An identity-source ARN in the grammar this service's resource ARNs take — the one
/// `ApprovalTeamArn`'s own pattern spells
/// (`^arn:aws(-[^:]+)?:mpa:[a-z0-9-]{1,20}:[0-9]{12}:approval-team/[a-zA-Z0-9._-]+$`), with
/// the resource name the `IdentitySource` resource's `arn` template carries.
fn source_arn(partition: &str, region: &str, id: &str) -> String {
    format!(
        "arn:{partition}:mpa:{region}:{}:identity-source/{id}",
        engine::emulator_account_id()
    )
}

/// One approver: identity `index`, authenticated by `source`.
fn approver_member(index: usize, source: &str) -> Value {
    json!({
        "PrimaryIdentityId": format!("identity-{index:04}"),
        "PrimaryIdentitySourceArn": source,
    })
}

/// A `CreateApprovalTeam` body every modelled constraint accepts, naming `approvers`.
fn create_body(name: &str, approvers: Vec<Value>) -> Value {
    json!({
        "Name": name,
        "Description": "a team whose approvers name an identity source",
        "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 1 } },
        "Approvers": approvers,
        "Policies": [{ "PolicyArn": POLICY_ARN }],
    })
}

/// The `PrimaryIdentitySourceArn` of every approver a team read reports, either off the
/// team itself (`member` is `None`) or off the change it has been asked to approve
/// (`Some("PendingUpdate")`).
fn reported_sources(body: &Value, member: Option<&str>) -> Vec<String> {
    let approvers = match member {
        None => &body["Approvers"],
        Some(member) => &body[member]["Approvers"],
    };
    approvers
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|approver| text(approver, "PrimaryIdentitySourceArn"))
        .collect()
}

/// The refusal table: values that address no identity source of this service. Rows differ
/// along which part of the ARN grammar the value misses, which service namespace and
/// resource kind it names, and how long the value is.
fn malformed_sources() -> Vec<(&'static str, String)> {
    let account = engine::emulator_account_id();
    vec![
        ("no colons at all", "not-an-arn".to_string()),
        // `PrimaryIdentitySourceArn` is `@length(max: 1000)` with no minimum, so an empty
        // value satisfies every modelled constraint.
        ("an empty value", String::new()),
        (
            "an identity source's id, not its ARN",
            "ids-000000000001".to_string(),
        ),
        (
            "the arn prefix misspelled",
            format!("srn:aws:mpa:{REGION}:{account}:identity-source/ids-000000000001"),
        ),
        ("too few fields", "arn:aws:mpa".to_string()),
        (
            "five fields, no resource",
            format!("arn:aws:mpa:{REGION}:{account}"),
        ),
        (
            "an empty resource",
            format!("arn:aws:mpa:{REGION}:{account}:"),
        ),
        (
            "an empty partition",
            format!("arn::mpa:{REGION}:{account}:identity-source/ids-000000000001"),
        ),
        (
            // `aws(-[^:]+)?` is the partition segment of this service's ARN patterns.
            "a partition outside the aws family",
            format!("arn:not-aws:mpa:{REGION}:{account}:identity-source/ids-000000000001"),
        ),
        (
            // `[a-z0-9-]{1,20}` in the same pattern, so an empty region is no ARN of this
            // service.
            "no region",
            format!("arn:aws:mpa::{account}:identity-source/ids-000000000001"),
        ),
        (
            // `[0-9]{12}` in the same pattern.
            "an account that is not twelve digits",
            format!("arn:aws:mpa:{REGION}:12345:identity-source/ids-000000000001"),
        ),
        (
            "another service's namespace",
            format!("arn:aws:sso:{REGION}:{account}:identity-source/ids-000000000001"),
        ),
        (
            // The IAM Identity Center instance a source is associated with is not the
            // identity source: `IdcInstanceArn`'s pattern, not this service's.
            "the IAM Identity Center instance's ARN",
            "arn:aws:sso:::instance/ssoins-1234567890abcdef".to_string(),
        ),
        (
            // `ApprovalTeamArn`: another resource kind of this same service.
            "an approval team's ARN",
            format!("arn:aws:mpa:{REGION}:{account}:approval-team/some-team"),
        ),
        (
            // `QualifiedPolicyArn`: an AWS-owned, global resource of this service.
            "an AWS-managed MPA policy's ARN",
            "arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy/$DEFAULT".to_string(),
        ),
        (
            "the resource kind with no id",
            format!("arn:aws:mpa:{REGION}:{account}:identity-source"),
        ),
        (
            "the resource kind with an empty id",
            format!("arn:aws:mpa:{REGION}:{account}:identity-source/"),
        ),
        (
            // `[a-zA-Z0-9._-]+` is the resource-id class this service's ARN patterns
            // declare, so a tilde is not one of its characters.
            "a resource id outside the id character class",
            format!("arn:aws:mpa:{REGION}:{account}:identity-source/ids~1"),
        ),
        (
            // `@length(max: 1000)`: the longest value a conforming request can carry.
            "a value at the member's maximum that is not an ARN",
            "x".repeat(1000),
        ),
    ]
}

/// The accepted table: values that ARE ARNs of an identity source of this service. Rows
/// differ along the partition, the region, the form of the resource id and the length of
/// the ARN.
fn well_formed_sources() -> Vec<(&'static str, String)> {
    vec![
        (
            "the commercial partition, the signed region",
            source_arn("aws", REGION, "ids-000000000001"),
        ),
        (
            // `aws(-[^:]+)?` admits every suffixed partition.
            "the China partition",
            source_arn("aws-cn", "cn-north-1", "ids-000000000002"),
        ),
        (
            "the GovCloud partition",
            source_arn("aws-us-gov", "us-gov-west-1", "ids-000000000003"),
        ),
        (
            "a region other than the one the request is signed for",
            source_arn("aws", "eu-west-1", "ids-000000000004"),
        ),
        (
            "an id in the IAM Identity Center instance form",
            source_arn("aws", REGION, "ssoins-0001"),
        ),
        (
            // `[a-zA-Z0-9._-]+` is the whole resource-id class.
            "an id spanning the resource-id character class",
            source_arn("aws", REGION, "Ids.000_000-1"),
        ),
        (
            // `@length(max: 1000)`: an ARN padded in its id to the member's maximum.
            "an ARN at the member's maximum length",
            {
                let base = source_arn("aws", REGION, "ids-");
                format!("{base}{}", "0".repeat(1000 - base.len()))
            },
        ),
    ]
}

/// A value that is not an identity source's ARN is refused with this service's one
/// malformed-ARN sentence on BOTH write paths, and nothing is written: the create stores
/// no team and the refused update leaves the team it addressed exactly as it was.
#[tokio::test]
async fn an_approver_identity_source_that_is_no_identity_source_arn_is_refused() {
    // Creating a team is restricted to the organization's management account, so the
    // table runs as it; that gate is tested in `authorization_gate.rs`.
    let harness = Harness::new();
    let good = create_source(&harness, 1, 0).await;

    // The team a refused update addresses, created once: every update below is refused, so
    // its version and its approvers must be the ones it was created with at the end.
    let (status, created) = harness
        .create(&create_body("baseline-team", vec![approver_member(0, &good)]))
        .await;
    assert_eq!(
        status, 200,
        "creating the baseline team returned {status} with {created:?}"
    );
    let team_arn = text(&created, "Arn");
    let baseline_version = text(&created, "VersionId");

    for (row, (case, malformed)) in malformed_sources().into_iter().enumerate() {
        // Where in the list the offending approver sits alternates, so the rule is not
        // read off the first entry alone.
        let approvers = if row.is_multiple_of(2) {
            vec![
                approver_member(0, &malformed),
                approver_member(1, &good),
            ]
        } else {
            vec![
                approver_member(0, &good),
                approver_member(1, &malformed),
            ]
        };

        let name = format!("refused-team-{row:02}");
        let (status, body) = harness
            .create(&create_body(&name, approvers.clone()))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from the create, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected error code ValidationException from the create, got \
             {body:?}"
        );
        assert_eq!(
            text(&body, "Message"),
            state::INVALID_ARN_MESSAGE,
            "case {case:?}: expected the malformed-ARN message from the create, got {body:?}"
        );
        // A refused create stores nothing, so the ARN the team would have carried
        // resolves to no team at all.
        let refused_arn = state::team_arn(REGION, &name);
        let (status, absent) = harness.get(&refused_arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the refused create stored a team, got {status} with {absent:?}"
        );

        let (status, body) = harness
            .update(&team_arn, &json!({ "Approvers": approvers }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from the update, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected error code ValidationException from the update, got \
             {body:?}"
        );
        assert_eq!(
            text(&body, "Message"),
            state::INVALID_ARN_MESSAGE,
            "case {case:?}: expected the malformed-ARN message from the update, got {body:?}"
        );

        let (status, read) = harness.get(&team_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: reading the team back returned {status} with {read:?}"
        );
        assert_eq!(
            text(&read, "VersionId"),
            baseline_version,
            "case {case:?}: the refused update moved the team's version: {read:?}"
        );
        assert_eq!(
            reported_sources(&read, None),
            vec![good.clone()],
            "case {case:?}: the refused update changed the team's approvers: {read:?}"
        );
        assert!(
            read.get("PendingUpdate").is_none_or(Value::is_null),
            "case {case:?}: the refused update left a change waiting on the team: {read:?}"
        );
    }
}

/// An identity source's ARN is recorded by the create and reported back by
/// `GetApprovalTeam` member for member, whatever form the ARN takes.
#[tokio::test]
async fn a_well_formed_approver_identity_source_is_recorded_and_reported() {
    let harness = Harness::new();

    for (row, (case, source)) in well_formed_sources().into_iter().enumerate() {
        let name = format!("accepted-team-{row:02}");
        let (status, created) = harness
            .create(&create_body(
                &name,
                vec![approver_member(0, &source), approver_member(1, &source)],
            ))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the create, got {status} with {created:?}"
        );
        let team_arn = text(&created, "Arn");

        let (status, read) = harness.get(&team_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: reading the team back returned {status} with {read:?}"
        );
        assert_eq!(
            reported_sources(&read, None),
            vec![source.clone(), source.clone()],
            "case {case:?}: the team does not report the identity source it was created \
             with: {read:?}"
        );
    }
}

/// The same value through the other write path: an update naming an identity source's ARN
/// is accepted, and the change the team is asked to approve reports that ARN.
#[tokio::test]
async fn an_update_records_and_reports_a_well_formed_approver_identity_source() {
    for (case, source) in well_formed_sources() {
        // A store of its own per row: each row creates the same team name.
        let harness = Harness::new();
        let created_with = create_source(&harness, 1, 0).await;
        let (status, created) = harness
            .create(&create_body(
                "updated-team",
                vec![approver_member(0, &created_with)],
            ))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: creating the team returned {status} with {created:?}"
        );
        let team_arn = text(&created, "Arn");

        // The change is put to the approvers of an ACTIVE team, so the invitations the
        // clock owes are waited out first.
        tokio::time::sleep(state::INVITATION_RESPONSE_DELAY + CLOCK_SLACK).await;

        let (status, updated) = harness
            .update(
                &team_arn,
                &json!({ "Approvers": [approver_member(1, &source)] }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the update returned {status} with {updated:?}"
        );
        // The change stays waiting on the team's approvers for the rest of the row, so the
        // read below reports the set the update asked for.
        postpone_approval(&harness, &team_arn);

        let (status, read) = harness.get(&team_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: reading the team back returned {status} with {read:?}"
        );
        assert_eq!(
            reported_sources(&read, Some("PendingUpdate")),
            vec![source.clone()],
            "case {case:?}: the requested change does not report the identity source the \
             update named: {read:?}"
        );
        assert_eq!(
            reported_sources(&read, None),
            vec![created_with.clone()],
            "case {case:?}: the team's live approvers changed before the team approved the \
             change: {read:?}"
        );
    }
}
