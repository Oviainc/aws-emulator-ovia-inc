// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetIdentitySource`.
//!
//! The rule under test: an ARN-addressed read resolves its ARN only as an ARN of the
//! resource kind that member addresses, and answers out of the stored record —
//!
//! * a value that is not an ARN of this service's `identity-source` resource — or one
//!   whose resource id is not an identity-source id this service could have minted —
//!   names no identity source whatever the store holds, so it is refused with the sentence
//!   that names the member it rejected
//!   ([`state::INVALID_IDENTITY_SOURCE_ARN_MESSAGE`], `ValidationException`, 400);
//! * an identity-source ARN outside the caller's account is `AccessDeniedException`
//!   (403), decided before the source is resolved;
//! * an identity-source ARN of the caller's whose id IS in that form and that the store
//!   holds no record for is `ResourceNotFoundException` (404) with this service's one
//!   not-found sentence ([`state::RESOURCE_NOT_FOUND_MESSAGE`]);
//! * otherwise every reported member is the stored record's, so this read and
//!   `ListIdentitySources` cannot disagree about one source, and reading twice changes
//!   nothing a later caller sees.
//!
//! The tables below span the class each rule generalises over rather than one request:
//! the FORM of the IAM Identity Center instance ARN, the instance's region, the number of
//! tags, the status the source is in when it is read (`CREATING`, `ACTIVE`, `DELETING`),
//! and — for the refusals — which part of the ARN grammar the value misses, which
//! resource kind and which service namespace it names, which account it names, and the
//! form of a resource id the store never minted. Where a row names a published bound
//! (`IdcInstanceArn`'s pattern, `ApprovalTeamArn`'s grammar, 50 tags per resource) a
//! comment names the modelled member or the project constant it comes from.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the
//! `GET /identity-sources/{IdentitySourceArn}` route, the URI label, the JSON envelope
//! and the HTTP status are all exercised.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::ops::types::shared_types as model;
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_team_lifecycle::{encode_label, error_code, instant, text, Harness, REGION};

/// Slack over a clock-owed span before the transition it owes is expected to have been
/// materialised.
const CLOCK_SLACK: Duration = Duration::from_millis(250);

/// One row of the create/get table: what the create names, and therefore what the read
/// must report.
struct SourceCase {
    case: &'static str,
    instance_arn: String,
    /// `IamIdentityCenter$Region`: the INSTANCE's region, which need not be the region the
    /// request is signed for.
    instance_region: &'static str,
    tags: usize,
}

/// An IAM Identity Center instance ARN that satisfies `IdcInstanceArn`'s modelled pattern
/// `^arn:.+:sso:::instance/(?:sso)?ins-[a-zA-Z0-9-.]{16}$` and its
/// `length(min: 20, max: 2048)`: the partition is `.+`, the `sso` prefix of the instance
/// id is optional, and the id's last 16 characters are `[a-zA-Z0-9-.]`.
fn instance_arn(partition: &str, prefix: &str, id: &str) -> String {
    assert_eq!(
        id.len(),
        16,
        "IdcInstanceArn's pattern fixes the instance id at 16 characters, {id:?} has {}",
        id.len()
    );
    format!("arn:{partition}:sso:::instance/{prefix}ins-{id}")
}

/// Rows differ along the instance-ARN form (partition, the optional `sso` prefix, the
/// character classes the id allows), the instance's region and the number of tags.
fn source_cases() -> Vec<SourceCase> {
    vec![
        SourceCase {
            case: "ins- form, instance in the caller's region, no tags",
            instance_arn: instance_arn("aws", "", "1234567890abcdef"),
            instance_region: REGION,
            tags: 0,
        },
        SourceCase {
            case: "ssoins- form, instance in another region, one tag",
            instance_arn: instance_arn("aws", "sso", "abcdef1234567890"),
            instance_region: "us-east-1",
            tags: 1,
        },
        SourceCase {
            case: "another partition, dots and dashes in the id, three tags",
            instance_arn: instance_arn("aws-us-gov", "", "a.b-c.d-12345678"),
            instance_region: "us-gov-west-1",
            tags: 3,
        },
        SourceCase {
            case: "mixed-case id, the maximum tags a resource carries",
            instance_arn: instance_arn("aws", "sso", "AbCdEf0123456789"),
            // `state::MAX_TAGS_PER_RESOURCE` is the ceiling this service enforces,
            // corroborated by `UntagResource$TagKeys`' `length(min: 0, max: 50)`.
            instance_region: "eu-west-1",
            tags: state::MAX_TAGS_PER_RESOURCE,
        },
    ]
}

/// The `CreateIdentitySource` body for a row.
fn create_body(case: &SourceCase) -> Value {
    let mut body = json!({
        "IdentitySourceParameters": {
            "IamIdentityCenter": {
                "InstanceArn": case.instance_arn,
                "Region": case.instance_region,
            }
        }
    });
    if case.tags > 0 {
        body["Tags"] = Value::Object(
            (1..=case.tags)
                .map(|t| (format!("key{t:02}"), json!(format!("value{t:02}"))))
                .collect(),
        );
    }
    body
}

/// `POST /identity-sources`, answering the ARN this service minted.
async fn create_source(harness: &Harness, case: &SourceCase) -> String {
    let (status, created) = harness
        .call("POST", "/identity-sources", Some(create_body(case)))
        .await;
    assert_eq!(
        status, 200,
        "case {:?}: create returned {status} with {created:?}",
        case.case
    );
    let arn = text(&created, "IdentitySourceArn");
    assert!(
        !arn.is_empty(),
        "case {:?}: create returned no IdentitySourceArn: {created:?}",
        case.case
    );
    arn
}

/// `GET /identity-sources/{IdentitySourceArn}` — the label is non-greedy, so the ARN is
/// percent-encoded into one path segment.
pub(crate) async fn get_source(harness: &Harness, arn: &str) -> (u16, Value) {
    harness
        .call("GET", &format!("/identity-sources/{}", encode_label(arn)), None)
        .await
}

/// The `ListIdentitySources` entry for `arn`, or `None` when no page carries it.
async fn listed(harness: &Harness, arn: &str) -> Option<Value> {
    let (status, body) = harness.call("POST", "/identity-sources/?List", None).await;
    assert_eq!(status, 200, "ListIdentitySources returned {body:?}");
    body.get("IdentitySources")
        .and_then(Value::as_array)
        .and_then(|sources| {
            sources
                .iter()
                .find(|source| text(source, "IdentitySourceArn") == arn)
                .cloned()
        })
}

/// One read, against the record the store holds for `arn`.
fn assert_reported_from_state(case: &str, reported: &Value, arn: &str, harness: &Harness) {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let guard = store.lock();
    let stored = guard
        .identity_source(arn)
        .unwrap_or_else(|| panic!("case {case:?}: nothing is stored under {arn:?}"));

    assert_eq!(
        text(reported, "IdentitySourceArn"),
        stored.arn,
        "case {case:?}: expected IdentitySourceArn {:?}, got {reported:?}",
        stored.arn
    );
    assert_eq!(
        text(reported, "IdentitySourceType"),
        stored.source_type.as_wire(),
        "case {case:?}: expected IdentitySourceType {:?}, got {reported:?}",
        stored.source_type.as_wire()
    );
    assert!(
        model::IDENTITY_SOURCE_TYPE_VALUES
            .contains(&text(reported, "IdentitySourceType").as_str()),
        "case {case:?}: IdentitySourceType {:?} is not a token the model declares ({:?})",
        text(reported, "IdentitySourceType"),
        model::IDENTITY_SOURCE_TYPE_VALUES
    );

    let status = text(reported, "Status");
    assert!(
        model::IDENTITY_SOURCE_STATUS_VALUES.contains(&status.as_str()),
        "case {case:?}: Status {status:?} is not a token the model declares ({:?})",
        model::IDENTITY_SOURCE_STATUS_VALUES
    );
    assert_eq!(
        status,
        stored.status.as_wire(),
        "case {case:?}: expected the stored Status {:?}, got {status:?}",
        stored.status.as_wire()
    );

    // Every `IdentitySourceStatusCode` the model declares is a failure reason, and
    // `StatusMessage` explains one, so a source in a status this service reaches on its
    // own carries neither.
    assert!(
        reported.get("StatusCode").is_none(),
        "case {case:?}: a source this service created reports no StatusCode: {reported:?}"
    );
    assert!(
        reported.get("StatusMessage").is_none(),
        "case {case:?}: a source this service created reports no StatusMessage: {reported:?}"
    );

    // The parameters are the instance the create named, under the union member the
    // source's type selects.
    let parameters = &reported["IdentitySourceParameters"]["IamIdentityCenter"];
    assert_eq!(
        text(parameters, "InstanceArn"),
        stored.instance_arn,
        "case {case:?}: expected InstanceArn {:?}, got {reported:?}",
        stored.instance_arn
    );
    assert_eq!(
        text(parameters, "Region"),
        stored.instance_region,
        "case {case:?}: expected Region {:?}, got {reported:?}",
        stored.instance_region
    );
    assert_eq!(
        parameters.get("ApprovalPortalUrl").and_then(Value::as_str),
        stored.approval_portal_url.as_deref(),
        "case {case:?}: expected ApprovalPortalUrl {:?}, got {reported:?}",
        stored.approval_portal_url
    );

    // `CreationTime` is the stored instant, which does not move between reads.
    let shown = instant(reported, "CreationTime");
    let recorded = stored
        .creation_time
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("a creation time after the epoch")
        .as_secs_f64();
    assert!(
        (shown - recorded).abs() < 1.0,
        "case {case:?}: expected CreationTime {recorded}, got {shown}"
    );
}

/// Every created identity source is reported by `GetIdentitySource` out of the record the
/// create wrote, member for member, and the read agrees with `ListIdentitySources`.
#[tokio::test]
async fn a_created_identity_source_is_reported_out_of_state() {
    // Creating an identity source is restricted to the organization's management account,
    // so this table runs as it; the gate itself is tested in `authorization_gate.rs`.
    let harness = Harness::new();

    let cases = source_cases();
    let mut arns: Vec<String> = Vec::new();
    for case in &cases {
        arns.push(create_source(&harness, case).await);
    }

    for (case, arn) in cases.iter().zip(arns.iter()) {
        let (status, body) = get_source(&harness, arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: get returned {status} with {body:?}",
            case.case
        );
        assert_reported_from_state(case.case, &body, arn, &harness);

        // The two reads of one source report the same record: every member
        // `ListIdentitySources` carries for it is the one this read carries.
        let entry = listed(&harness, arn)
            .await
            .unwrap_or_else(|| panic!("case {:?}: {arn:?} is not listed", case.case));
        for member in [
            "IdentitySourceArn",
            "IdentitySourceType",
            "Status",
            "CreationTime",
        ] {
            assert_eq!(
                body.get(member),
                entry.get(member),
                "case {:?}: the two reads disagree about {member}: {body:?} vs {entry:?}",
                case.case
            );
        }
        assert_eq!(
            body["IdentitySourceParameters"]["IamIdentityCenter"],
            entry["IdentitySourceParameters"]["IamIdentityCenter"],
            "case {:?}: the two reads disagree about the instance parameters",
            case.case
        );
    }

    // A read changes nothing a later caller sees. The clock-owed activation is settled
    // first, so what is compared is two reads of the same state.
    tokio::time::sleep(state::IDENTITY_SOURCE_ACTIVATION_DELAY + CLOCK_SLACK).await;
    for (case, arn) in cases.iter().zip(arns.iter()) {
        let (status, first) = get_source(&harness, arn).await;
        assert_eq!(status, 200, "case {:?}: get returned {first:?}", case.case);
        let (status, again) = get_source(&harness, arn).await;
        assert_eq!(status, 200, "case {:?}: get returned {again:?}", case.case);
        assert_eq!(
            first, again,
            "case {:?}: GetIdentitySource is a read: two calls must agree",
            case.case
        );
    }
}

/// The read reports the status the CLOCK owes — `CREATING` with no portal, then `ACTIVE`
/// with the portal of its instance, then `DELETING` once a deletion is started and 404
/// once the removal comes due. No read starts any of those transitions.
#[tokio::test]
async fn a_get_reports_the_status_the_clock_owes() {
    for case in source_cases() {
        let harness = Harness::new();
        let arn = create_source(&harness, &case).await;

        let (status, creating) = get_source(&harness, &arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: get returned {status} with {creating:?}",
            case.case
        );
        assert_eq!(
            text(&creating, "Status"),
            state::IdentitySourceStatus::Creating.as_wire(),
            "case {:?}: a source whose activation is not due yet is CREATING, got {creating:?}",
            case.case
        );
        assert!(
            creating["IdentitySourceParameters"]["IamIdentityCenter"]
                .get("ApprovalPortalUrl")
                .is_none(),
            "case {:?}: a CREATING source has no approval portal yet: {creating:?}",
            case.case
        );

        tokio::time::sleep(state::IDENTITY_SOURCE_ACTIVATION_DELAY + CLOCK_SLACK).await;
        let (status, active) = get_source(&harness, &arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: get returned {status} with {active:?}",
            case.case
        );
        assert_eq!(
            text(&active, "Status"),
            state::IdentitySourceStatus::Active.as_wire(),
            "case {:?}: a source whose activation is due is ACTIVE, got {active:?}",
            case.case
        );
        let portal = active["IdentitySourceParameters"]["IamIdentityCenter"]
            .get("ApprovalPortalUrl")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            !portal.is_empty(),
            "case {:?}: an ACTIVE source reports the portal of its instance: {active:?}",
            case.case
        );
        assert_reported_from_state(case.case, &active, &arn, &harness);

        // A started deletion is reported as the model's other non-terminal status, and the
        // source is gone — 404, not a report — once its removal comes due.
        let (status, deleted) = harness
            .call("DELETE", &format!("/identity-sources/{}", encode_label(&arn)), None)
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: delete returned {status} with {deleted:?}",
            case.case
        );
        let (status, deleting) = get_source(&harness, &arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: get returned {status} with {deleting:?}",
            case.case
        );
        assert_eq!(
            text(&deleting, "Status"),
            state::IdentitySourceStatus::Deleting.as_wire(),
            "case {:?}: a source whose deletion has started is DELETING, got {deleting:?}",
            case.case
        );

        tokio::time::sleep(state::IDENTITY_SOURCE_DELETION_DELAY + CLOCK_SLACK).await;
        let (status, gone) = get_source(&harness, &arn).await;
        assert_eq!(
            status, 404,
            "case {:?}: a removed source is not found, got {status} with {gone:?}",
            case.case
        );
        assert_eq!(
            error_code(&gone),
            "ResourceNotFoundException",
            "case {:?}: expected error code ResourceNotFoundException, got {gone:?}",
            case.case
        );
    }
}

/// A value that is not an ARN of this service's identity-source resource — or one whose
/// resource id is not an identity-source id this service could have minted — names no
/// identity source at all, so it is refused as the value that addresses none:
/// `ValidationException` (400) with [`state::INVALID_IDENTITY_SOURCE_ARN_MESSAGE`],
/// decided before anything is looked up.
#[tokio::test]
async fn an_arn_that_names_no_identity_source_is_refused_as_malformed() {
    let harness = Harness::new();
    let owned = create_source(&harness, &source_cases()[0]).await;
    let account = engine::emulator_account_id();

    // (case, the value asked for). Rows differ along WHICH part of the ARN grammar
    // `arn:<partition>:mpa:<region>:<account>:identity-source/<id>` the value misses: the
    // ARN syntax itself, the partition, the service namespace, the region, the account,
    // the resource kind and the resource id.
    let rows: Vec<(&str, String)> = vec![
        ("no colons at all", "not-an-arn".to_string()),
        ("an identity source's id, not its ARN", "ids-000000000001".to_string()),
        ("the arn prefix misspelled", format!("srn:aws:mpa:{REGION}:{account}:identity-source/ids-000000000001")),
        ("too few fields", "arn:aws:mpa".to_string()),
        ("five fields, no resource", format!("arn:aws:mpa:{REGION}:{account}")),
        ("an empty resource", format!("arn:aws:mpa:{REGION}:{account}:")),
        ("an empty partition", format!("arn::mpa:{REGION}:{account}:identity-source/ids-000000000001")),
        (
            "a partition outside the aws family",
            format!("arn:not-aws:mpa:{REGION}:{account}:identity-source/ids-000000000001"),
        ),
        (
            // `ApprovalTeamArn`'s pattern declares the region segment of this service's
            // ARNs as `[a-z0-9-]{1,20}`, so an empty one is no ARN of this service.
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
            "another service's namespace",
            format!("arn:aws:sso:{REGION}:{account}:identity-source/ids-000000000001"),
        ),
        (
            // The IAM Identity Center instance an identity source is associated with is
            // not the identity source: `IdcInstanceArn`'s own pattern, not this service's.
            "the IAM Identity Center instance's ARN",
            instance_arn("aws", "sso", "1234567890abcdef"),
        ),
        (
            // `ApprovalTeamArn`: another resource kind of this same service.
            "an approval team's ARN",
            format!("arn:aws:mpa:{REGION}:{account}:approval-team/some-team"),
        ),
        (
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
            // form is no id this service could have issued (`identity_source_id_form.rs`
            // tables the form itself).
            "a uuid-shaped resource id",
            format!("arn:aws:mpa:{REGION}:{account}:identity-source/0f5e0119-ab78-418e-b630-3b421ab2fd6f"),
        ),
        (
            "a word as the resource id",
            format!("arn:aws:mpa:{REGION}:{account}:identity-source/nonexistent"),
        ),
        (
            "a dotted resource id",
            format!("arn:aws:mpa:{REGION}:{account}:identity-source/identity.source.1"),
        ),
    ];

    for (case, value) in rows {
        let (status, body) = get_source(&harness, &value).await;
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

    // A refused read is a read: the caller's own source is still reported.
    let (status, body) = get_source(&harness, &owned).await;
    assert_eq!(
        status, 200,
        "the refusals above changed what the caller's own source reports: {body:?}"
    );
}

/// An identity-source ARN of the caller's whose id IS in the form this service mints and
/// that the store holds no record for is `ResourceNotFoundException` (404) with this
/// service's one not-found sentence.
#[tokio::test]
async fn an_identity_source_arn_with_no_record_is_not_found() {
    let harness = Harness::new();
    let owned = create_source(&harness, &source_cases()[0]).await;
    let account = engine::emulator_account_id();
    let owned_id = owned
        .rsplit('/')
        .next()
        .expect("the resource id of a minted ARN")
        .to_string();

    // (case, region, resource id). Rows differ along the body of an id the store never
    // minted and along the region the ARN names.
    let rows: Vec<(&str, &str, String)> = vec![
        (
            "an id in this service's own minted form that was never issued",
            REGION,
            "ids-000000009999".to_string(),
        ),
        (
            "the shortest body the minted form admits",
            REGION,
            "ids-1".to_string(),
        ),
        (
            "a minted-form id whose body carries letters",
            REGION,
            "ids-a1b2c3".to_string(),
        ),
        (
            // The ARN carries the region the source lives in, so the caller's own id in
            // another region names another resource — one this store does not hold.
            "the caller's own id in another region",
            "eu-west-1",
            owned_id,
        ),
    ];

    for (case, region, resource) in rows {
        let arn = format!("arn:aws:mpa:{region}:{account}:identity-source/{resource}");
        let (status, body) = get_source(&harness, &arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected error code ResourceNotFoundException, got {body:?}"
        );
        assert_eq!(
            text(&body, "Message"),
            state::RESOURCE_NOT_FOUND_MESSAGE,
            "case {case:?}: expected this service's not-found message, got {body:?}"
        );
        assert!(
            listed(&harness, &arn).await.is_none(),
            "case {case:?}: the read recorded an identity source under {arn:?}"
        );
    }

    // The store still holds exactly the one source the create wrote.
    let store = state::service_state(&*harness.engine).expect("the service store");
    let held = store.lock().identity_sources.len();
    assert_eq!(
        held, 1,
        "the store holds {held} source(s), expected only the one that was created"
    );
}

/// The account boundary is decided before the source is resolved: an identity-source ARN
/// outside the caller's account is `AccessDeniedException` (403), whatever the store holds
/// under it.
#[tokio::test]
async fn an_identity_source_outside_the_callers_account_is_denied_before_resolution() {
    let harness = Harness::new();
    let owned = create_source(&harness, &source_cases()[0]).await;
    let owned_id = owned
        .rsplit('/')
        .next()
        .expect("the resource id of a minted ARN")
        .to_string();

    // (case, account, region, resource id)
    let rows: Vec<(&str, &str, &str, String)> = vec![
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
        // Decided before the source is resolved, so an ARN whose resource id the caller
        // DOES own is refused just the same.
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
        let (status, body) = get_source(&harness, &arn).await;
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

    // The caller's own source is untouched and still reported.
    let (status, body) = get_source(&harness, &owned).await;
    assert_eq!(
        status, 200,
        "a denied cross-account read changed the caller's own source: {body:?}"
    );
}
