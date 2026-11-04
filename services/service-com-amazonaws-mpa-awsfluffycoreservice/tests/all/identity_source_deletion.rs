// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteIdentitySource`.
//!
//! The rule under test: the call takes the identity source its `IdentitySourceArn` names
//! out of the store — the stored record is put into `DELETING` and is gone once that
//! removal comes due ([`state::IDENTITY_SOURCE_DELETION_DELAY`]), so
//! `ListIdentitySources` reports it `DELETING` straight after the call and omits it
//! afterwards, and nothing else in the store moves. An ARN under which no identity source
//! is stored is a success that changes nothing (the operation is `@idempotent` and models
//! no `ResourceNotFoundException`); a source a stored approval team's approvers
//! authenticate through is `ConflictException` (409); an ARN outside the caller's account
//! is `AccessDeniedException` (403), decided before the source is resolved; and a value
//! that is not an ARN of this service's identity-source resource — the kind of ARN the
//! member addresses, down to the form of the resource id — is `ValidationException` (400),
//! decided before the account boundary and before anything is looked up.
//!
//! The tables below span that class along what the rule generalises over: the stored
//! source's status when the delete is issued (`CREATING` and `ACTIVE`, the two statuses a
//! created source is in), how many sources the store holds and which of them is deleted
//! (the first, a middle one, the last), the number of tags on the deleted source, the
//! form of the ARN asked for, and the account it names. Where a row names a published
//! bound (`IdcInstanceArn`'s pattern, 50 tags per resource, `IdentitySourceStatus`'
//! tokens) a comment names the modelled member or the project constant it comes from.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the
//! `DELETE /identity-sources/{IdentitySourceArn}` route, the URI label, the error
//! envelope and the HTTP status are all exercised.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::ops::types::shared_types as model;
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_team_lifecycle::{encode_label, error_code, text, Harness, REGION};

/// Slack over a clock-owed span before the transition it owes is expected to have been
/// materialised.
const CLOCK_SLACK: Duration = Duration::from_millis(250);

/// An IAM Identity Center instance ARN that satisfies `IdcInstanceArn`'s modelled pattern
/// `^arn:.+:sso:::instance/(?:sso)?ins-[a-zA-Z0-9-.]{16}$`: the instance id's last 16
/// characters are `[a-zA-Z0-9-.]` and the `sso` prefix is optional.
fn instance_arn(n: usize) -> String {
    format!("arn:aws:sso:::instance/ssoins-{n:016}")
}

/// `POST /identity-sources` for the `n`-th source of a test, with `tags` tags.
fn create_body(n: usize, tags: usize) -> Value {
    let mut body = json!({
        "IdentitySourceParameters": {
            "IamIdentityCenter": {
                "InstanceArn": instance_arn(n),
                "Region": REGION,
            }
        }
    });
    if tags > 0 {
        body["Tags"] = Value::Object(
            (1..=tags)
                .map(|t| (format!("key{t:02}"), json!(format!("value{t:02}"))))
                .collect(),
        );
    }
    body
}

/// Create one identity source and answer the ARN this service minted for it.
pub(crate) async fn create_source(harness: &Harness, n: usize, tags: usize) -> String {
    let (status, created) = harness
        .call("POST", "/identity-sources", Some(create_body(n, tags)))
        .await;
    assert_eq!(
        status, 200,
        "creating identity source {n} returned {status} with {created:?}"
    );
    let arn = text(&created, "IdentitySourceArn");
    assert!(
        !arn.is_empty(),
        "creating identity source {n} returned no IdentitySourceArn: {created:?}"
    );
    arn
}

/// `DELETE /identity-sources/{IdentitySourceArn}` — the label is non-greedy, so the ARN
/// is percent-encoded into one path segment.
pub(crate) async fn delete_source(harness: &Harness, arn: &str) -> (u16, Value) {
    harness
        .call("DELETE", &format!("/identity-sources/{}", encode_label(arn)), None)
        .await
}

/// The status `ListIdentitySources` reports for `arn`, or `None` when the page does not
/// carry it at all.
pub(crate) async fn listed_status(harness: &Harness, arn: &str) -> Option<String> {
    let (status, body) = harness.call("POST", "/identity-sources/?List", None).await;
    assert_eq!(status, 200, "ListIdentitySources returned {body:?}");
    body.get("IdentitySources")
        .and_then(Value::as_array)
        .and_then(|sources| {
            sources
                .iter()
                .find(|source| text(source, "IdentitySourceArn") == arn)
                .map(|source| text(source, "Status"))
        })
}

/// The stored source's `(Status, deletion_due)`, straight out of the store.
pub(crate) fn stored(harness: &Harness, arn: &str) -> Option<(&'static str, Option<SystemTime>)> {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let guard = store.lock();
    guard
        .identity_source(arn)
        .map(|source| (source.status.as_wire(), source.deletion_due))
}

/// One row of the deletion table.
struct Case {
    case: &'static str,
    /// How many identity sources the store holds when the delete is issued.
    sources: usize,
    /// Which of them is deleted (0-based, in creation order).
    delete: usize,
    /// Tags on the deleted source.
    tags: usize,
    /// Whether the source's clock-owed activation is awaited first, so the delete is
    /// issued against an `ACTIVE` source rather than a `CREATING` one.
    activate_first: bool,
}

/// Rows differ along the number of stored sources, which of them is deleted, the number
/// of tags it carries and the status it is in when the delete is issued.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "the only source, still CREATING, untagged",
            sources: 1,
            delete: 0,
            tags: 0,
            activate_first: false,
        },
        Case {
            case: "the only source, ACTIVE, one tag",
            sources: 1,
            delete: 0,
            tags: 1,
            activate_first: true,
        },
        Case {
            case: "the first of three, ACTIVE, three tags",
            sources: 3,
            delete: 0,
            tags: 3,
            activate_first: true,
        },
        Case {
            case: "a middle one of three, still CREATING",
            sources: 3,
            delete: 1,
            tags: 0,
            activate_first: false,
        },
        Case {
            case: "the last of three, ACTIVE, the maximum tags a resource carries",
            sources: 3,
            delete: 2,
            // `state::MAX_TAGS_PER_RESOURCE`: the ceiling this service enforces,
            // corroborated by `UntagResource$TagKeys`' `length(min: 0, max: 50)`.
            tags: state::MAX_TAGS_PER_RESOURCE,
            activate_first: true,
        },
    ]
}

/// The rule, over the whole table: the named source is `DELETING` until its removal comes
/// due and gone afterwards, and no other source moves.
#[tokio::test]
async fn a_deleted_identity_source_is_deleting_then_gone_and_nothing_else_moves() {
    for case in cases() {
        let label = case.case;
        // Creating an identity source is restricted to the organization's management
        // account, so the table runs as it; that gate is tested in `authorization_gate.rs`.
        let harness = Harness::new();

        let mut arns: Vec<String> = Vec::new();
        for n in 0..case.sources {
            let tags = if n == case.delete { case.tags } else { 0 };
            arns.push(create_source(&harness, n + 1, tags).await);
        }
        let target = arns[case.delete].clone();

        if case.activate_first {
            tokio::time::sleep(state::IDENTITY_SOURCE_ACTIVATION_DELAY + CLOCK_SLACK).await;
            assert_eq!(
                listed_status(&harness, &target).await.as_deref(),
                Some(state::IdentitySourceStatus::Active.as_wire()),
                "case {label:?}: the source did not become ACTIVE before the delete"
            );
        }

        let (status, body) = delete_source(&harness, &target).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200, got {status} with {body:?}"
        );

        // Until the removal comes due the source is reported DELETING — the non-terminal
        // `IdentitySourceStatus` token the model declares for a deletion in progress.
        let deleting = state::IdentitySourceStatus::Deleting.as_wire();
        assert!(
            model::IDENTITY_SOURCE_STATUS_VALUES.contains(&deleting),
            "case {label:?}: {deleting:?} is not a token the model declares ({:?})",
            model::IDENTITY_SOURCE_STATUS_VALUES
        );
        assert_eq!(
            listed_status(&harness, &target).await.as_deref(),
            Some(deleting),
            "case {label:?}: the source was not reported {deleting:?} after the delete"
        );
        let after = stored(&harness, &target);
        assert_eq!(
            after.map(|(status, _)| status),
            Some(deleting),
            "case {label:?}: the stored record was not put into {deleting:?}, got {after:?}"
        );
        let due = after.and_then(|(_, due)| due);
        assert!(
            due.is_some(),
            "case {label:?}: the stored record carries no removal due instant: {after:?}"
        );

        // A repeat call is the same success and does not postpone the removal: the due
        // instant the first call recorded stands.
        let (repeat, repeated) = delete_source(&harness, &target).await;
        assert_eq!(
            repeat, 200,
            "case {label:?}: the repeated delete returned {repeat} with {repeated:?}"
        );
        assert_eq!(
            stored(&harness, &target).and_then(|(_, due)| due),
            due,
            "case {label:?}: the repeated delete moved the removal's due instant"
        );

        // Once the removal is due the source is gone: it is not listed, the store holds
        // no record, and it is no longer a taggable resource.
        tokio::time::sleep(state::IDENTITY_SOURCE_DELETION_DELAY + CLOCK_SLACK).await;
        assert_eq!(
            listed_status(&harness, &target).await,
            None,
            "case {label:?}: the deleted source is still listed"
        );
        assert_eq!(
            stored(&harness, &target),
            None,
            "case {label:?}: the store still holds the deleted source"
        );
        let (tag_status, tags) = harness
            .call("GET", &format!("/tags/{}", encode_label(&target)), None)
            .await;
        assert_eq!(
            tag_status, 404,
            "case {label:?}: the deleted source is still a taggable resource: {tags:?}"
        );
        assert_eq!(
            error_code(&tags),
            "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {tags:?}"
        );

        // Every other source is exactly as it was: a delete removes the one source its
        // ARN names.
        for (index, arn) in arns.iter().enumerate() {
            if index == case.delete {
                continue;
            }
            assert_eq!(
                listed_status(&harness, arn).await.as_deref(),
                Some(state::IdentitySourceStatus::Active.as_wire()),
                "case {label:?}: source {index} is no longer reported ACTIVE"
            );
        }
        let store = state::service_state(&*harness.engine).expect("the service store");
        let remaining = store.lock().identity_sources.len();
        assert_eq!(
            remaining,
            case.sources - 1,
            "case {label:?}: the store holds {remaining} source(s), expected {}",
            case.sources - 1
        );
    }
}

/// An identity-source ARN in the caller's own account under which no identity source is
/// stored: the source is already absent, so the idempotent delete succeeds and writes
/// nothing. A value that addresses no identity source at all is NOT this class — see
/// `a_malformed_identity_source_arn_is_refused`.
#[tokio::test]
async fn deleting_an_identity_source_that_is_not_stored_changes_nothing() {
    let harness = Harness::new();

    // A source of the caller's, so a no-op delete can be shown to leave the store alone.
    let bystander = create_source(&harness, 1, 2).await;

    // An approval team of the caller's: a resource of ANOTHER type, whose ARN this
    // operation must not resolve to anything.
    let (status, team) = harness
        .call(
            "POST",
            "/approval-teams",
            Some(json!({
                "Name": "bystander-team",
                "Description": "a team no identity-source delete may touch",
                "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 1 } },
                "Approvers": [{
                    "PrimaryIdentityId": "identity-0001",
                    "PrimaryIdentitySourceArn": format!(
                        "arn:aws:mpa:{REGION}:{}:identity-source/ids-unrelated",
                        engine::emulator_account_id()
                    ),
                }],
                "Policies": [{
                    "PolicyArn":
                        "arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy/$DEFAULT"
                }],
            })),
        )
        .await;
    assert_eq!(status, 200, "creating the bystander team returned {team:?}");
    let team_arn = text(&team, "Arn");

    let account = engine::emulator_account_id();
    // (case, the ARN asked for). Every row IS an ARN of this service's identity-source
    // resource in the caller's OWN account, carrying an id in the form this service mints —
    // the only values that reach the lookup at all — and differs along what the store holds
    // nothing under: an id never minted, a region nothing was created in, an id body of
    // letters and digits, and the longest id a conforming request can carry.
    let rows: [(&str, String); 5] = [
        (
            "an identity-source id this account never minted",
            format!("arn:aws:mpa:{REGION}:{account}:identity-source/ids-000000009999"),
        ),
        (
            "the bystander's id in another region",
            format!("arn:aws:mpa:eu-west-1:{account}:identity-source/ids-000000000001"),
        ),
        (
            // The id's body is letters and digits, the class the form this service mints
            // admits (`identity_source_id_form.rs` tables the form itself).
            "a minted-form id whose body carries letters",
            format!("arn:aws:mpa:{REGION}:{account}:identity-source/ids-a1b2c3"),
        ),
        (
            // `aws(-[^:]+)?` in the same pattern admits every suffixed partition.
            "an identity source of the China partition",
            format!("arn:aws-cn:mpa:cn-north-1:{account}:identity-source/ids-000000000001"),
        ),
        (
            // `@length(max: 1000)`: an ARN padded in its id to the member's maximum.
            "an ARN at the member's maximum length",
            {
                let base =
                    format!("arn:aws:mpa:{REGION}:{account}:identity-source/ids-");
                format!("{base}{}", "0".repeat(1000 - base.len()))
            },
        ),
    ];

    for (case, arn) in rows {
        let (status, body) = delete_source(&harness, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body:?}"
        );
        assert_eq!(
            stored(&harness, &arn),
            None,
            "case {case:?}: the call recorded an identity source under an ARN that held none"
        );
        // The caller's own source and team are untouched.
        assert!(
            listed_status(&harness, &bystander).await.is_some(),
            "case {case:?}: the call removed an unrelated identity source"
        );
        let (status, reread) = harness
            .call("GET", &format!("/approval-teams/{}", encode_label(&team_arn)), None)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the call removed an unrelated approval team: {reread:?}"
        );
    }

    let store = state::service_state(&*harness.engine).expect("the service store");
    let remaining = store.lock().identity_sources.len();
    assert_eq!(
        remaining, 1,
        "the store holds {remaining} source(s), expected only the bystander"
    );
}

/// An identity source a stored approval team's approvers authenticate through is in use:
/// the delete is refused with the modelled `ConflictException` (409) and the source stays
/// exactly as it was. Once no team references it, the same call deletes it.
#[tokio::test]
async fn an_identity_source_in_use_by_an_approval_team_is_refused() {
    let harness = Harness::new();
    let source = create_source(&harness, 1, 0).await;
    let other = create_source(&harness, 2, 0).await;

    // Rows differ along WHERE in the team's approver list the reference sits, so the
    // refusal cannot depend on it being the first approver.
    let rows: [(&str, usize); 3] = [
        ("the team's only approver", 1),
        ("the first of two approvers", 2),
        ("the last of three approvers", 3),
    ];

    for (case, approvers) in rows {
        let team_name = format!("team-{approvers}");
        let referencing = if approvers == 1 { 0 } else { approvers - 1 };
        let approver_bodies: Vec<Value> = (0..approvers)
            .map(|index| {
                let source_arn = if index == referencing {
                    source.clone()
                } else {
                    other.clone()
                };
                json!({
                    "PrimaryIdentityId": format!("identity-{index:04}"),
                    "PrimaryIdentitySourceArn": source_arn,
                })
            })
            .collect();
        let (status, team) = harness
            .call(
                "POST",
                "/approval-teams",
                Some(json!({
                    "Name": team_name,
                    "Description": "a team authenticated by the source under test",
                    "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 1 } },
                    "Approvers": approver_bodies,
                    "Policies": [{
                        "PolicyArn":
                            "arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy/$DEFAULT"
                    }],
                })),
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: creating the team returned {team:?}");
        let team_arn = text(&team, "Arn");
        let team_version = text(&team, "VersionId");

        let before = stored(&harness, &source);
        let (status, body) = delete_source(&harness, &source).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected HTTP 409, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case:?}: expected error code ConflictException, got {body:?}"
        );
        assert_eq!(
            stored(&harness, &source),
            before,
            "case {case:?}: the refused delete changed the stored source"
        );

        // The conflict is about the stored reference, not about the ARN: with the team
        // gone, the same call is accepted.
        let (status, removed) = harness
            .call(
                "DELETE",
                &format!(
                    "/approval-teams/{}/{}",
                    encode_label(&team_arn),
                    encode_label(&team_version)
                ),
                None,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: deleting the team returned {status} with {removed:?}"
        );
    }

    let (status, body) = delete_source(&harness, &source).await;
    assert_eq!(
        status, 200,
        "no team references the source any more, got {status} with {body:?}"
    );
    // The removal is CLOCK-OWED: it is materialised by the next call, so the read goes
    // over the wire first and the store is inspected afterwards.
    tokio::time::sleep(state::IDENTITY_SOURCE_DELETION_DELAY + CLOCK_SLACK).await;
    assert_eq!(
        listed_status(&harness, &source).await,
        None,
        "the source no team references is still listed"
    );
    assert_eq!(
        stored(&harness, &source),
        None,
        "the source no team references is still stored"
    );
    assert!(
        listed_status(&harness, &other).await.is_some(),
        "the unrelated identity source was removed too"
    );
}

/// The account boundary is decided before the source is resolved: an ARN outside the
/// caller's account is `AccessDeniedException` (403), not the no-op success an absent
/// source earns.
#[tokio::test]
async fn an_identity_source_outside_the_callers_account_is_denied_before_resolution() {
    let harness = Harness::new();
    let owned = create_source(&harness, 1, 1).await;
    let owned_id = owned
        .rsplit('/')
        .next()
        .expect("the resource id of a minted ARN")
        .to_string();

    // (case, account, region, resource id)
    let rows: [(&str, &str, &str, String); 4] = [
        (
            "another account",
            "999999999999",
            REGION,
            "ids-000000000001".to_string(),
        ),
        (
            "an account one digit from the caller's",
            "000000000001",
            REGION,
            "ids-000000000001".to_string(),
        ),
        // The denial is decided before the source is resolved, so an ARN whose resource
        // id the caller DOES own is refused just the same.
        (
            "another account, a resource id the caller owns",
            "999999999999",
            REGION,
            owned_id,
        ),
        (
            "another account in another region",
            "999999999999",
            "eu-west-1",
            "ids-000000000002".to_string(),
        ),
    ];

    for (case, account, region, resource) in rows {
        let arn = format!("arn:aws:mpa:{region}:{account}:identity-source/{resource}");
        let (status, body) = delete_source(&harness, &arn).await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected HTTP 403, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDeniedException",
            "case {case:?}: expected error code AccessDeniedException, got {body:?}"
        );
    }

    // An ARN that names NO account — the form an AWS-managed MPA policy's ARN takes
    // (`arn:aws:mpa:::aws:policy/...`) — carries no account for this boundary to compare
    // and names no identity source either, so it is the malformed-ARN class the table in
    // `a_malformed_identity_source_arn_is_refused` asserts, decided before this boundary.

    // A refused request is a refusal: the caller's own source is exactly as it was.
    assert!(
        listed_status(&harness, &owned).await.is_some(),
        "a denied cross-account delete removed the caller's own identity source"
    );
}

/// `IdentitySourceArn` carries no ARN pattern of its own (`length(max: 1000)` only), so a
/// value that is not an ARN of this service's identity-source resource — the kind of ARN
/// the member addresses, down to the form of its resource id — is refused with the
/// sentence that names the member it rejected:
/// `ValidationException` (400) with [`state::INVALID_IDENTITY_SOURCE_ARN_MESSAGE`],
/// decided before the account boundary and before anything is looked up.
#[tokio::test]
async fn a_malformed_identity_source_arn_is_refused() {
    let harness = Harness::new();
    let owned = create_source(&harness, 1, 0).await;

    let account = engine::emulator_account_id();
    // (case, the value asked for) — rows differ along WHICH part of the grammar
    // `arn:<partition>:mpa:<region>:<account>:identity-source/<id>` the value misses: the
    // ARN syntax itself, the partition, the service namespace, the region, the account,
    // the resource kind and the resource id.
    let rows: [(&str, String); 20] = [
        ("no colons at all", "not-an-arn".to_string()),
        (
            "an identity source's id, not its ARN",
            "ids-000000000001".to_string(),
        ),
        ("the arn prefix misspelled", format!("srn:aws:mpa:{REGION}:{account}:identity-source/ids-1")),
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
            format!("arn::mpa:{REGION}:{account}:identity-source/ids-1"),
        ),
        (
            // `aws(-[^:]+)?` is the partition segment of this service's ARN patterns
            // (`ApprovalTeamArn`).
            "a partition outside the aws family",
            format!("arn:not-aws:mpa:{REGION}:{account}:identity-source/ids-1"),
        ),
        (
            "an empty service",
            format!("arn:aws::{REGION}:{account}:identity-source/ids-1"),
        ),
        (
            "another service's namespace",
            format!("arn:aws:sso:{REGION}:{account}:identity-source/ids-000000000001"),
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
            "no account",
            format!("arn:aws:mpa:{REGION}::identity-source/ids-000000000001"),
        ),
        (
            // `ApprovalTeamArn`: another resource kind of this same service.
            "an approval team's ARN",
            format!("arn:aws:mpa:{REGION}:{account}:approval-team/some-team"),
        ),
        (
            // `QualifiedPolicyArn`: an AWS-owned, global resource of this service, whose
            // ARN names no account at all.
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
            // An identity-source id is minted by this service, so a value outside that
            // form is no id this service could have issued.
            "a uuid-shaped resource id",
            format!(
                "arn:aws:mpa:{REGION}:{account}:identity-source/\
                 0f5e0119-ab78-418e-b630-3b421ab2fd6f"
            ),
        ),
        (
            "a resource id spanning the resource-id character class",
            format!("arn:aws:mpa:{REGION}:{account}:identity-source/Ids.000_000-9"),
        ),
    ];

    for (case, value) in rows {
        let (status, body) = delete_source(&harness, &value).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected error code ValidationException, got {body:?}"
        );
        assert_eq!(
            text(&body, "Message"),
            state::INVALID_IDENTITY_SOURCE_ARN_MESSAGE,
            "case {case:?}: expected the member-named refusal, got {body:?}"
        );
    }

    // None of it touched the caller's own source.
    assert!(
        listed_status(&harness, &owned).await.is_some(),
        "a refused malformed-ARN delete removed the caller's own identity source"
    );
}

/// Deleting a source frees the slot it held, so the service's identity-source ceiling
/// counts what is stored rather than what was ever created.
#[tokio::test]
async fn a_removed_identity_source_frees_the_accounts_quota_slot() {
    let harness = Harness::new();

    // `state::MAX_IDENTITY_SOURCES` comes from `ListIdentitySources`' own output
    // (`IdentitySources` is `length(max: 20)`, `MaxResults` is `range(1..20)`).
    let mut arns: Vec<String> = Vec::new();
    for n in 1..=state::MAX_IDENTITY_SOURCES {
        arns.push(create_source(&harness, n, 0).await);
    }

    let (status, refused) = harness
        .call(
            "POST",
            "/identity-sources",
            Some(create_body(state::MAX_IDENTITY_SOURCES + 1, 0)),
        )
        .await;
    assert_eq!(
        status, 402,
        "a create past the ceiling returned {status} with {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "ServiceQuotaExceededException",
        "expected error code ServiceQuotaExceededException, got {refused:?}"
    );

    let (status, body) = delete_source(&harness, &arns[0]).await;
    assert_eq!(status, 200, "the delete returned {status} with {body:?}");
    tokio::time::sleep(state::IDENTITY_SOURCE_DELETION_DELAY + CLOCK_SLACK).await;

    let (status, created) = harness
        .call(
            "POST",
            "/identity-sources",
            Some(create_body(state::MAX_IDENTITY_SOURCES + 1, 0)),
        )
        .await;
    assert_eq!(
        status, 200,
        "the slot the removal freed is still taken: {status} with {created:?}"
    );
    assert_ne!(
        text(&created, "IdentitySourceArn"),
        arns[0],
        "the new source reused the removed source's ARN: {created:?}"
    );
}
