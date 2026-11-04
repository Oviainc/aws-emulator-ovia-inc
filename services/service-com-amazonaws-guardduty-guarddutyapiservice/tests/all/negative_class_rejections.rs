// SPDX-License-Identifier: CC0-1.0

//! The documented REFUSAL classes of the operations that address a resource by id,
//! end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. An operation handed a well-formed id, name or pair its own state does not
//!    resolve answers the CLIENT-ERROR class its model declares for that absence —
//!    the modelled `ResourceNotFoundException`/404 where the operation lists one, and
//!    otherwise the `BadRequestException` envelope under the `InvalidInputException`
//!    class with HTTP 400 — never HTTP 501 and never a hollow success.
//! 2. A refused request writes NOTHING: the state a later read reports is exactly the
//!    state before the refusal.
//! 3. The same operations ACCEPT what their state does resolve, and the resource is
//!    readable back through the matching read — so each refusal above is a lookup and
//!    not a constant.
//!
//! The rows per operation span the ways an id can fail to resolve (well formed but
//! absent, malformed, a near miss, the right id with the wrong partner member) and,
//! where an operation has a success path, the ways a request can be acceptable.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};

use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    now_millis, service_state, ReceivedInvitationRecord, FINDING_ATTRIBUTES,
    INVALID_FINDING_IDS_MESSAGE, RELATIONSHIP_INVITED, SET_ID_NOT_FOUND_MESSAGE,
};

use super::client_rejections::UNUSABLE_INPUT_VALUE;
use super::detector_lifecycle::{
    all_finding_ids, as_delegated_admin, call, enabled_detector, error_code, error_shape,
    new_router, router_over, Router,
};

/// The error CLASS every 400 on this service carries in the body's `__type` and in
/// its `type` member.
const INVALID_INPUT: &str = "InvalidInputException";

/// The modelled error SHAPE a 400 is enveloped in, as `x-amzn-errortype` carries it.
const BAD_REQUEST: &str = "BadRequestException";

/// The modelled 404 shape, which the custom-detection-rule operations declare and
/// therefore answer as the shape itself.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The modelled 409 shape.
const CONFLICT: &str = "ConflictException";

/// The Region the shared harness signs its requests for.
const REGION: &str = "us-east-1";

/// `ListCustomDetectionRules` / `GetCustomDetectionRule` live under this prefix
/// (`smithy_query(operation="ListCustomDetectionRules")`).
const RULES_PATH: &str = "/custom-detection-rule/rule";

/// Every `AssociationMode` member (`smithy_query(shape="AssociationMode")`).
const MODES: [&str; 2] = ["DRY_RUN", "LIVE"];

// ---------------------------------------------------------------------------
// Shared assertions.
// ---------------------------------------------------------------------------

/// Assert a response is this service's 400 client rejection: the status, the class in
/// all three places it appears, and a message that names `member` without echoing
/// `value`.
fn assert_bad_request(case: &str, status: u16, body: &Value, member: &str, value: &str) {
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_shape(body),
        BAD_REQUEST,
        "case {case}: expected the modelled envelope shape, got {body:?}"
    );
    assert_eq!(
        error_code(body),
        INVALID_INPUT,
        "case {case}: expected the invalid-input class in __type, got {body:?}"
    );
    assert_eq!(
        body["type"],
        json!(INVALID_INPUT),
        "case {case}: expected the invalid-input class in the type member, got {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(member),
        "case {case}: expected a message naming {member:?}, got {message:?}"
    );
    assert!(
        value.is_empty() || !message.contains(value),
        "case {case}: a rejection must not echo the caller's value {value:?}, got {message:?}"
    );
}

/// Assert a response is this service's 400 client rejection carrying the FIXED
/// sentence its resource family answers with: the status, the class in all three
/// places it appears, the whole message, and no echo of `value`.
///
/// The sibling above is for the families whose refusal names the member at fault;
/// this one is for the families whose refusal is one fixed text naming nothing — the
/// absent-resource and finding-identifier classes — where "contains the member name"
/// is not the rule and the message itself is.
fn assert_bad_request_message(
    case: &str,
    status: u16,
    body: &Value,
    expected_message: &str,
    value: &str,
) {
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_shape(body),
        BAD_REQUEST,
        "case {case}: expected the modelled envelope shape, got {body:?}"
    );
    assert_eq!(
        error_code(body),
        INVALID_INPUT,
        "case {case}: expected the invalid-input class in __type, got {body:?}"
    );
    assert_eq!(
        body["type"],
        json!(INVALID_INPUT),
        "case {case}: expected the invalid-input class in the type member, got {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert_eq!(
        message, expected_message,
        "case {case}: expected message {expected_message:?}, got {message:?}"
    );
    assert!(
        value.is_empty() || !message.contains(value),
        "case {case}: a rejection must not echo the caller's value {value:?}, got {message:?}"
    );
}

/// Assert a response is a modelled error SHAPE (404 / 409): the status, the shape in
/// both `__type` and the `x-amzn-errortype` header, and the shape repeated in the
/// exception's own `type` member.
fn assert_modelled_error(case: &str, status: u16, body: &Value, expected: &str, code: u16) {
    assert_eq!(
        status, code,
        "case {case}: expected HTTP {code}, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        expected,
        "case {case}: expected {expected} in __type, got {body:?}"
    );
    assert_eq!(
        error_shape(body),
        expected,
        "case {case}: expected {expected} in x-amzn-errortype, got {body:?}"
    );
    assert_eq!(
        body["type"],
        json!(expected),
        "case {case}: expected {expected} in the type member, got {body:?}"
    );
}

/// The ids of the rules GuardDuty publishes, read over the wire so the test cannot
/// drift from the catalogue.
async fn published_rule_ids(router: &Router) -> Vec<String> {
    let (status, body) = call(router, "POST", RULES_PATH, Some(json!({}))).await;
    assert_eq!(
        status, 200,
        "list-custom-detection-rules: expected 200, got {status} {body:?}"
    );
    let ids: Vec<String> = body["rules"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|rule| rule["ruleId"].as_str().map(str::to_string))
        .collect();
    assert!(
        ids.len() > 1,
        "list-custom-detection-rules: expected a catalogue of more than one rule, got {ids:?}"
    );
    ids
}

// ---------------------------------------------------------------------------
// AcceptInvitation / AcceptAdministratorInvitation: a pair that names no pending
// invitation.
// ---------------------------------------------------------------------------

/// The two names GuardDuty binds the SAME acceptance under — "this operation is
/// deprecated, use AcceptAdministratorInvitation instead"
/// (`smithy_query(operation="AcceptInvitation")`) — as
/// `(path segment, administrator member, read's output member)`. The pair is the
/// axis the rules below are asserted over, so neither name can drift from the other.
const ACCEPTANCES: [(&str, &str, &str); 2] = [
    ("master", "masterId", "master"),
    ("administrator", "administratorId", "administrator"),
];

/// Both acceptances model exactly `BadRequestException` (400) and
/// `InternalServerErrorException` (500)
/// (`smithy_query(operation="AcceptInvitation,AcceptAdministratorInvitation")`), so
/// there is no not-found shape and no success for an invitation that is not pending:
/// the absence case is the 400.
///
/// The rows differ along the operation NAME, along the administrator account id
/// across the account-id space, and along the invitation id's shape — nothing a
/// request can carry makes an invitation exist.
#[tokio::test]
async fn accept_invitation_without_a_pending_invitation_is_refused_and_establishes_nothing() {
    let administrators = [
        ("111122223333", "the account id the GuardDuty docs use"),
        ("000000000000", "the low end of the account-id space"),
        ("999999999999", "the high end of the account-id space"),
    ];
    let invitations = [
        "bogus",
        "d4b94fc952d6912b8f3060768220e2e7",
        "11111111-2222-3333-4444-555555555555",
        "i",
    ];

    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    for acceptance in ACCEPTANCES {
        let (segment, member, _) = acceptance;
        for (administrator_id, why) in administrators {
            for invitation_id in invitations {
                let case = format!(
                    "{segment}: {member} {administrator_id} ({why}) with invitationId {invitation_id:?}"
                );
                let (status, body) =
                    accept(&router, acceptance, &detector_id, administrator_id, invitation_id)
                        .await;
                assert_bad_request(&case, status, &body, member, administrator_id);

                // And no administrator relationship was established: every member of
                // the read's shape is optional, so "no administrator" is their
                // ABSENCE, through BOTH reads.
                for read in ACCEPTANCES {
                    let reported = administrator_account(&router, read, &detector_id).await;
                    assert_eq!(
                        reported["accountId"],
                        Value::Null,
                        "case {case}: a refused acceptance must leave no administrator, got {reported:?}"
                    );
                }
            }
        }

        // An administrator id that cannot be an account id at all is the PARAMETER
        // failure, and is refused the same way. The rows are 12-character values
        // that reach the handler (the model constrains nothing) plus two outside
        // that length.
        for (administrator_id, why) in [
            ("abcdefghijkl", "12 letters: not digits at all"),
            ("1111-2222-33", "grouped with hyphens"),
            ("12345678901", "11 digits: one short"),
            ("1234567890123", "13 digits: one too many"),
        ] {
            let case = format!("{segment}: malformed {member} {administrator_id} ({why})");
            let (status, body) =
                accept(&router, acceptance, &detector_id, administrator_id, "bogus").await;
            assert_bad_request(&case, status, &body, member, administrator_id);
        }
    }
}

/// The refusal above is a LOOKUP: an invitation the store does hold is accepted, and
/// the relationship it establishes is what both administrator reads then report.
///
/// The invitations are seeded through this service's own state module, so the rows can
/// span inviting account ids and invitation-id shapes the one account id this emulator
/// presents does not reach; `InviteMembers` delivering one over the wire is the
/// `invitation_delivery` suite's subject. The rows differ along the operation NAME, the
/// inviting account id and the invitation id's shape.
#[tokio::test]
async fn accept_invitation_accepts_a_pending_invitation_and_the_read_reports_it() {
    let cases = [
        ("111122223333", "d4b94fc952d6912b8f3060768220e2e7"),
        ("222233334444", "11111111-2222-3333-4444-555555555555"),
        ("333344445555", "i"),
    ];

    for acceptance in ACCEPTANCES {
        let (segment, member, _) = acceptance;
        for (administrator_id, invitation_id) in cases {
            let case = format!("{segment}: invitation {invitation_id:?} from {administrator_id}");
            let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
            let router = router_over(Arc::clone(&engine));
            let detector_id = enabled_detector(&router).await;
            seed_invitation(
                &engine,
                REGION,
                &detector_id,
                administrator_id,
                invitation_id,
                now_millis(),
            );

            // The right administrator with the WRONG invitation id validates
            // nothing, so it is still the refusal — the pair identifies the
            // invitation, not either member alone.
            let (status, body) = accept(
                &router,
                acceptance,
                &detector_id,
                administrator_id,
                "not-the-invitation",
            )
            .await;
            assert_bad_request(&case, status, &body, member, administrator_id);
            let reported = administrator_account(&router, acceptance, &detector_id).await;
            assert_eq!(
                reported["accountId"],
                Value::Null,
                "case {case}: a mismatched invitationId must establish nothing, got {reported:?}"
            );

            let (status, body) = accept(
                &router,
                acceptance,
                &detector_id,
                administrator_id,
                invitation_id,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: accepting the pending invitation expected 200, got {status} {body:?}"
            );

            // BOTH reads report the one stored relationship.
            for read in ACCEPTANCES {
                let reported = administrator_account(&router, read, &detector_id).await;
                assert_eq!(
                    reported["accountId"],
                    json!(administrator_id),
                    "case {case}: expected the administrator account id, got {reported:?}"
                );
                assert_eq!(
                    reported["invitationId"],
                    json!(invitation_id),
                    "case {case}: expected the accepted invitation id, got {reported:?}"
                );
                assert_eq!(
                    reported["relationshipStatus"],
                    json!("Enabled"),
                    "case {case}: expected the accepted relationship status, got {reported:?}"
                );
                assert!(
                    reported["invitedAt"].as_str().is_some_and(|at| !at.is_empty()),
                    "case {case}: expected the invitation's own invitedAt, got {reported:?}"
                );
            }

            // The invitation is no longer pending, so accepting it again is the
            // refusal again — and the relationship the first call established is
            // untouched.
            let established = administrator_account(&router, acceptance, &detector_id).await;
            let (status, body) = accept(
                &router,
                acceptance,
                &detector_id,
                administrator_id,
                invitation_id,
            )
            .await;
            assert_bad_request(&case, status, &body, member, administrator_id);
            assert_eq!(
                administrator_account(&router, acceptance, &detector_id).await,
                established,
                "case {case}: a refused repeat must leave the relationship where it was"
            );
        }
    }
}

/// One acceptance: `POST /detector/{DetectorId}/{segment}` carrying the
/// administrator member the operation names
/// (`smithy_query(operation="AcceptInvitation,AcceptAdministratorInvitation")`).
async fn accept(
    router: &Router,
    acceptance: (&str, &str, &str),
    detector_id: &str,
    administrator_id: &str,
    invitation_id: &str,
) -> (u16, Value) {
    let (segment, member, _) = acceptance;
    let mut body = serde_json::Map::new();
    body.insert(member.to_string(), json!(administrator_id));
    body.insert("invitationId".to_string(), json!(invitation_id));
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/{segment}"),
        Some(Value::Object(body)),
    )
    .await
}

/// The administrator relationship one of the two reads reports —
/// `GET /detector/{DetectorId}/{segment}`.
async fn administrator_account(
    router: &Router,
    acceptance: (&str, &str, &str),
    detector_id: &str,
) -> Value {
    let (segment, _, output_member) = acceptance;
    let (status, body) = call(
        router,
        "GET",
        &format!("/detector/{detector_id}/{segment}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "reading /{segment}: expected 200, got {status} {body:?}"
    );
    body[output_member].clone()
}

/// Put one pending invitation in the detector's inbox, through this service's own
/// state module — the store the handlers read.
///
/// `region` is the Region the detector's create was signed for, because a detector is
/// a REGIONAL resource and the store resolves it under that Region. Shared with the
/// `administrator_disassociation` suite, which needs the same seam for the same
/// reason: an invitation from an account id other than the one this emulator presents
/// reaches the inbox only through the store (`InviteMembers` delivers one from that
/// account id, which the `invitation_delivery` suite drives over the wire).
pub(crate) fn seed_invitation(
    engine: &engine::InMemoryEngine,
    region: &str,
    detector_id: &str,
    administrator_id: &str,
    invitation_id: &str,
    invited_at_millis: i64,
) {
    let state = service_state(engine).expect("this service's state");
    let mut state = state.lock();
    let detector = state
        .detector_in_mut(region, detector_id)
        .expect("the detector the create minted");
    detector.received_invitations.insert(
        administrator_id.to_string(),
        ReceivedInvitationRecord {
            administrator_account_id: administrator_id.to_string(),
            invitation_id: invitation_id.to_string(),
            relationship_status: RELATIONSHIP_INVITED.to_string(),
            invited_at_millis,
        },
    );
}

// ---------------------------------------------------------------------------
// ArchiveFindings / UnarchiveFindings: a FindingId the detector does not hold.
// ---------------------------------------------------------------------------

/// The finding types the detector under test is populated with. Source: the finding
/// types this service's sample catalogue publishes (`CreateSampleFindings`).
const SAMPLE_TYPES: [&str; 3] = [
    "Backdoor:EC2/DenialOfService.Tcp",
    "Discovery:S3/MaliciousIPCaller",
    "PenTest:IAMUser/PentooLinux",
];

/// Both directions of the archive write resolve every `FindingIds` entry against the
/// detector's own findings before they write, so a batch naming one it does not hold
/// is refused whole and moves nothing.
///
/// The rows differ along the way an id fails to resolve and along where in the batch
/// it sits: a well-formed id nobody minted and a real id upper-cased (both of the
/// finding-id form, so both the finding-identifier refusal), values that cannot be a
/// finding id at all (a parameter refusal), alone and mixed in beside ids the
/// detector does hold.
#[tokio::test]
async fn archiving_a_finding_id_the_detector_does_not_hold_is_refused_and_moves_nothing() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    create_samples(&router, &detector_id, &SAMPLE_TYPES).await;
    let held = all_finding_ids(&router, &detector_id, 50).await;
    assert_eq!(
        held.len(),
        SAMPLE_TYPES.len(),
        "expected one finding per sample type, got {held:?}"
    );

    // A mixed starting state, so a refused request that wrote in either direction
    // would show up below.
    set_archived_ok(&router, &detector_id, &held[..1], true).await;
    let before = archived_ids(&router, &detector_id).await;

    let upper_cased = held[0].to_uppercase();
    // Each row: the id the batch names, why it resolves to nothing, and the message
    // the refusal carries — the finding-identifier sentence for a value that IS of
    // the finding-id form but names no finding of this detector, and the generic
    // input-parameter sentence for a value that could not be a finding id at all.
    let absent: [(&str, &str, String); 5] = [
        (
            "ffffffffffffffffffffffffffffffff",
            "32 hexadecimal characters nobody minted",
            INVALID_FINDING_IDS_MESSAGE.to_string(),
        ),
        (
            upper_cased.as_str(),
            "a held id upper-cased, which is still well formed",
            INVALID_FINDING_IDS_MESSAGE.to_string(),
        ),
        (
            "not-a-finding-id",
            "not hexadecimal at all",
            UNUSABLE_INPUT_VALUE.to_string(),
        ),
        (
            "abc123",
            "too short to be a finding id",
            UNUSABLE_INPUT_VALUE.to_string(),
        ),
        (
            "0123456789abcdef0123456789abcdef0",
            "33 characters: one past the id length",
            UNUSABLE_INPUT_VALUE.to_string(),
        ),
    ];

    for (id, why, expected_message) in absent {
        // The batch SHAPES: the absent id alone, after a held id, and before one.
        let batches: [(&str, Vec<String>); 3] = [
            ("alone", vec![id.to_string()]),
            ("after a held id", vec![held[1].clone(), id.to_string()]),
            ("before a held id", vec![id.to_string(), held[2].clone()]),
        ];
        for (shape, batch) in batches {
            for archived in [true, false] {
                let case = format!("{why}, {shape}, archived={archived}");
                let (status, body) = set_archived(&router, &detector_id, &batch, archived).await;
                assert_bad_request_message(
                    &case,
                    status,
                    &body,
                    &expected_message,
                    id,
                );
                assert_eq!(
                    archived_ids(&router, &detector_id).await,
                    before,
                    "case {case}: a refused batch must move no finding's archive state"
                );
            }
        }
    }

    // And the same calls over ids the detector DOES hold still succeed, so the
    // refusal above is a lookup and not a blanket refusal.
    set_archived_ok(&router, &detector_id, &held, true).await;
    assert_eq!(
        archived_ids(&router, &detector_id).await,
        held.iter().cloned().collect::<BTreeSet<String>>(),
        "archiving every held id must archive every held id"
    );
    set_archived_ok(&router, &detector_id, &held, false).await;
    assert_eq!(
        archived_ids(&router, &detector_id).await,
        BTreeSet::new(),
        "unarchiving every held id must leave nothing archived"
    );
}

async fn create_samples(router: &Router, detector_id: &str, types: &[&str]) {
    let (status, body) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings/create"),
        Some(json!({"findingTypes": types})),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-sample-findings {types:?}: expected 200, got {status} {body:?}"
    );
}

async fn set_archived(
    router: &Router,
    detector_id: &str,
    ids: &[String],
    archived: bool,
) -> (u16, Value) {
    let path = if archived { "archive" } else { "unarchive" };
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings/{path}"),
        Some(json!({"findingIds": ids})),
    )
    .await
}

async fn set_archived_ok(router: &Router, detector_id: &str, ids: &[String], archived: bool) {
    let (status, body) = set_archived(router, detector_id, ids, archived).await;
    assert_eq!(
        status, 200,
        "set archived={archived} over {ids:?}: expected 200, got {status} {body:?}"
    );
}

/// The ids `ListFindings` selects with an equality criterion on `service.archived`.
async fn archived_ids(router: &Router, detector_id: &str) -> BTreeSet<String> {
    let (status, body) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings"),
        Some(json!({
            "findingCriteria": {"criterion": {"service.archived": {"equals": ["true"]}}}
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "list-findings filtered on archived: expected 200, got {status} {body:?}"
    );
    body["findingIds"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|id| id.as_str().map(str::to_string))
        .collect()
}

// ---------------------------------------------------------------------------
// CreateFilter: a criterion key that names no finding attribute.
// ---------------------------------------------------------------------------

/// A `FindingCriteria.Criterion` key must NAME a field of a finding. The model types
/// the criterion as a map keyed by a plain string, so the boundary accepts any key and
/// the handler is what refuses one that names no field — the same check `ListFindings`
/// and `GetFindings` apply.
///
/// The rows differ along the way a key fails to name a field: a dotted path into
/// nothing, a near miss on a real field, a casing variant, a path extending a scalar
/// that has nothing beneath it, and a field the emulator reports but cannot order
/// numerically.
#[tokio::test]
async fn create_filter_with_an_unknown_criterion_field_is_refused_and_persists_nothing() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    let unknown: [(&str, Value, &str); 6] = [
        (
            "this.field.does.not.exist",
            json!({"eq": ["x"]}),
            "a dotted path into nothing",
        ),
        ("sevrity", json!({"eq": ["8"]}), "a typo on a real attribute"),
        ("AccountId", json!({"eq": ["111122223333"]}), "a casing variant"),
        (
            "service.archived.value",
            json!({"eq": ["true"]}),
            "a path extending a real attribute",
        ),
        ("", json!({"eq": ["x"]}), "the empty key"),
        (
            "type",
            json!({"greaterThan": 1}),
            "a real attribute ordered numerically, which it cannot be",
        ),
    ];

    for (index, (field, condition, why)) in unknown.into_iter().enumerate() {
        let name = format!("filter-{index}");
        let case = format!("criterion key {field:?} ({why})");
        let (status, body) = create_filter(
            &router,
            &detector_id,
            json!({
                "name": name,
                "findingCriteria": criteria(field, condition),
            }),
        )
        .await;
        assert_bad_request(&case, status, &body, "findingCriteria", "");

        // No filter was persisted under that name, so the refusal left nothing
        // behind: neither the list nor the detail read knows it.
        assert!(
            !filter_names(&router, &detector_id).await.contains(&name),
            "case {case}: a refused create must persist no filter named {name:?}"
        );
        let (status, body) = call(
            &router,
            "GET",
            &format!("/detector/{detector_id}/filter/{name}"),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: reading the refused filter expected 400, got {status} {body:?}"
        );
    }

    // Every field a stored finding populates IS accepted, and a saved filter reports
    // the criteria it was created with. The rows are every one of them, so a criterion
    // a findings read resolves is one a filter can carry.
    for (index, field) in FINDING_ATTRIBUTES.iter().enumerate() {
        let name = format!("accepted-{index}");
        let case = format!("criterion key {field:?}");
        let condition = json!({"neq": ["an-impossible-value"]});
        let (status, body) = create_filter(
            &router,
            &detector_id,
            json!({"name": name, "findingCriteria": criteria(field, condition.clone())}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected the create to succeed, got {status} {body:?}"
        );
        assert_eq!(
            body["name"],
            json!(name),
            "case {case}: expected the saved name, got {body:?}"
        );

        let (status, saved) = call(
            &router,
            "GET",
            &format!("/detector/{detector_id}/filter/{name}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: reading the saved filter expected 200, got {status} {saved:?}"
        );
        assert_eq!(
            saved["findingCriteria"]["criterion"][*field]["neq"],
            condition["neq"],
            "case {case}: expected the criterion the create stored, got {saved:?}"
        );
    }
}

/// A `FindingCriteria` carrying exactly one criterion: `field` under `condition`.
///
/// Built through a map rather than a `json!` literal so the KEY can be a row of the
/// table under test. Shared with the `finding_field_catalog` suite, which asserts the
/// other side of the same rule, so the two cannot build a criterion differently.
pub(crate) fn criteria(field: &str, condition: Value) -> Value {
    let mut criterion = serde_json::Map::new();
    criterion.insert(field.to_string(), condition);
    json!({"criterion": Value::Object(criterion)})
}

pub(crate) async fn create_filter(router: &Router, detector_id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/filter"),
        Some(body),
    )
    .await
}

async fn filter_names(router: &Router, detector_id: &str) -> Vec<String> {
    let (status, body) = call(
        router,
        "GET",
        &format!("/detector/{detector_id}/filter"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "list-filters: expected 200, got {status} {body:?}"
    );
    body["filterNames"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|name| name.as_str().map(str::to_string))
        .collect()
}

// ---------------------------------------------------------------------------
// CreateMembers: an AccountId that is not an account id.
// ---------------------------------------------------------------------------

/// `AccountDetail.AccountId` is an AWS account id — 12 decimal digits — and the model
/// constrains the LENGTH only (`len 12..12`,
/// `smithy_query(shape="AccountId")`), so a 12-character value that is not an account
/// id reaches the handler and is refused there rather than recorded as a member that
/// can never exist.
///
/// The rows differ along the ALPHABET at the right length and along the length, and
/// the list POSITION of the offending entry, since the whole request is refused.
#[tokio::test]
async fn create_members_with_a_malformed_account_id_is_refused_and_records_nothing() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    let malformed: [(&str, &str); 5] = [
        ("abcdefghijkl", "12 letters: not digits at all"),
        ("12345678901a", "11 digits and a letter"),
        ("1234 6789012", "12 characters, one of them a space"),
        ("+11122223333", "a leading plus sign"),
        ("1111-2222-33", "grouped with hyphens"),
    ];
    let good = ("111122223333", "member-one@example.com");

    for (account_id, why) in malformed {
        let details = [
            ("alone", vec![json!({"accountId": account_id, "email": "bad@example.com"})]),
            (
                "after a well-formed entry",
                vec![
                    json!({"accountId": good.0, "email": good.1}),
                    json!({"accountId": account_id, "email": "bad@example.com"}),
                ],
            ),
        ];
        for (shape, account_details) in details {
            let case = format!("accountId {account_id:?} ({why}), {shape}");
            let (status, body) = call(
                &router,
                "POST",
                &format!("/detector/{detector_id}/member"),
                Some(json!({"accountDetails": account_details})),
            )
            .await;
            assert_bad_request(&case, status, &body, "accountId", account_id);
            assert_eq!(
                member_ids(&router, &detector_id).await,
                BTreeSet::new(),
                "case {case}: a refused create must record no member"
            );
        }
    }

    // A well-formed account id IS recorded, so the refusal above is a form check and
    // not a blanket refusal.
    let (status, body) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/member"),
        Some(json!({"accountDetails": [{"accountId": good.0, "email": good.1}]})),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-members with a well-formed account id: got {status} {body:?}"
    );
    assert_eq!(
        member_ids(&router, &detector_id).await,
        BTreeSet::from([good.0.to_string()]),
        "a well-formed account id must be recorded as a member"
    );
}

async fn member_ids(router: &Router, detector_id: &str) -> BTreeSet<String> {
    let (status, body) = call(
        router,
        "GET",
        &format!("/detector/{detector_id}/member"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "list-members: expected 200, got {status} {body:?}"
    );
    body["members"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|member| member["accountId"].as_str().map(str::to_string))
        .collect()
}

// ---------------------------------------------------------------------------
// UpdateIPSet: an IpSetId that was never created.
// ---------------------------------------------------------------------------

/// `UpdateIPSet` models `AccessDeniedException` (403), `BadRequestException` (400) and
/// `InternalServerErrorException` (500) and no not-found shape
/// (`smithy_query(operation="UpdateIPSet")`), so an `IpSetId` the detector does not
/// hold is the 400 — never a success for an update that changed nothing.
///
/// The rows differ along the way the id fails to resolve and along the body the update
/// carries, which must make no difference to the refusal.
#[tokio::test]
async fn update_ip_set_with_an_id_the_detector_does_not_hold_is_refused_and_lists_nothing() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    let absent: [(&str, &str); 4] = [
        ("never-existed-ipset", "not an id this service mints"),
        (
            "deadbeefdeadbeefdeadbeefdeadbeef",
            "32 hexadecimal characters nobody minted",
        ),
        ("abc123", "too short"),
        (
            "00000000000000000000000000000000",
            "the low end of the id space",
        ),
    ];
    let bodies = [
        json!({"name": "x"}),
        json!({"activate": true}),
        json!({"location": "s3://my-bucket/list.txt"}),
        json!({}),
    ];

    for (ip_set_id, why) in absent {
        for body in &bodies {
            let case = format!("ipSetId {ip_set_id:?} ({why}) with {body}");
            let (status, response) = call(
                &router,
                "POST",
                &format!("/detector/{detector_id}/ipset/{ip_set_id}"),
                Some(body.clone()),
            )
            .await;
            assert_bad_request_message(
                &case,
                status,
                &response,
                SET_ID_NOT_FOUND_MESSAGE,
                ip_set_id,
            );
            assert!(
                !ip_set_ids(&router, &detector_id).await.contains(ip_set_id),
                "case {case}: a refused update must not make the id appear in ListIPSets"
            );
        }
    }
}

async fn ip_set_ids(router: &Router, detector_id: &str) -> BTreeSet<String> {
    let (status, body) = call(
        router,
        "GET",
        &format!("/detector/{detector_id}/ipset"),
        None,
    )
    .await;
    assert_eq!(status, 200, "list-ip-sets: expected 200, got {status} {body:?}");
    body["ipSetIds"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|id| id.as_str().map(str::to_string))
        .collect()
}

// ---------------------------------------------------------------------------
// CreateCustomDetectionRuleAssociation: a RuleId the catalogue does not publish.
// ---------------------------------------------------------------------------

/// A custom detection rule is published BY GuardDuty, so an association can only name
/// one the catalogue carries. The operation models `ResourceNotFoundException` (404)
/// (`smithy_query(operation="CreateCustomDetectionRuleAssociation")`), which is the
/// only class that lets a client tell "this rule was never authored" from "you may not
/// do this" (`AccessDeniedException`), so an unpublished id is the modelled 404.
///
/// The rows differ along the FORM of the unpublished id — every one satisfies
/// `RuleId`'s own pattern, so each reaches the handler — and along the `Mode`.
#[tokio::test]
async fn associating_an_unpublished_rule_is_refused_with_the_modelled_404() {
    let router = new_router();
    let published = published_rule_ids(&router).await;

    let unpublished = [
        (
            "d4b94fc952d6912b8f3060768220e2e7",
            "a uuid4 hex, as a client generates",
        ),
        ("no-such-rule", "a plausible but unpublished slug"),
        ("a", "a single character: the low end of RuleId"),
        (
            "cloudtrail-logging-disabled-x",
            "one segment past a published id",
        ),
    ];

    for (rule_id, why) in unpublished {
        for mode in MODES {
            let case = format!("ruleId {rule_id:?} ({why}) in mode {mode}");
            let (status, body) = create_association(&router, json!({"ruleId": rule_id, "mode": mode}))
                .await;
            assert_modelled_error(&case, status, &body, NOT_FOUND, 404);
            let message = body["message"].as_str().unwrap_or_default();
            // The rule id is what says WHICH rule the catalogue does not carry, and
            // it came from the request, so the fixed text carries it.
            assert_eq!(
                message,
                format!("Rule not found: {rule_id}"),
                "case {case}: expected the fixed not-found text carrying the rule id, got {message:?}"
            );
        }
    }

    // A published rule IS associable, in either mode, and the association reads back
    // through `GetCustomDetectionRuleAssociation` with the values the create minted.
    // A rule carries at most one association, so each mode takes a published rule of
    // its own.
    for (index, mode) in MODES.iter().enumerate() {
        let rule_id = &published[index % published.len()];
        let case = format!("published ruleId {rule_id:?} in mode {mode}");
        let (status, created) = create_association(
            &router,
            json!({"ruleId": rule_id, "mode": mode, "tags": {"team": "security"}}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected the create to succeed, got {status} {created:?}"
        );
        let association = created["ruleAssociation"].clone();
        let association_id = association["associationId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected an associationId, got {created:?}"))
            .to_string();
        assert_eq!(
            association["ruleId"],
            json!(rule_id),
            "case {case}: expected the rule it associates, got {association:?}"
        );
        assert_eq!(
            association["mode"],
            json!(mode),
            "case {case}: expected the mode the request asked for, got {association:?}"
        );
        assert_eq!(
            association["accountId"],
            json!(::engine::emulator_account_id()),
            "case {case}: expected the serving account, got {association:?}"
        );
        assert!(
            association["arn"]
                .as_str()
                .is_some_and(|arn| arn.contains(rule_id) && arn.contains(&association_id)),
            "case {case}: expected an ARN naming both ids, got {association:?}"
        );

        let (status, read) = call(
            &router,
            "GET",
            &format!("/custom-detection-rule/rule/{rule_id}/association/{association_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: reading the association expected 200, got {status} {read:?}"
        );
        assert_eq!(
            read["ruleAssociation"], association,
            "case {case}: the read must report exactly what the create returned, got {read:?}"
        );
        assert_eq!(
            read["tags"],
            json!({"team": "security"}),
            "case {case}: expected the tags the create recorded, got {read:?}"
        );

        // A second create for the same pair is the modelled conflict: creating is not
        // updating.
        let (status, body) =
            create_association(&router, json!({"ruleId": rule_id, "mode": mode})).await;
        assert_modelled_error(&case, status, &body, CONFLICT, 409);
    }

    // An association the account does not hold is the read's own modelled 404: a
    // published rule with no association, and a well-formed association id against a
    // rule that does have one.
    let other = &published[1];
    let (status, body) = call(
        &router,
        "GET",
        &format!("/custom-detection-rule/rule/{other}/association/aaaaaaaaaaaa"),
        None,
    )
    .await;
    assert_modelled_error(
        "reading an association of an unassociated rule",
        status,
        &body,
        NOT_FOUND,
        404,
    );
}

/// `CreateCustomDetectionRuleAssociation` is `POST /custom-detection-rule/association`
/// (`smithy_query(operation="CreateCustomDetectionRuleAssociation")`).
async fn create_association(router: &Router, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        "/custom-detection-rule/association",
        Some(body),
    )
    .await
}

/// A replayed `ClientToken` answers with the association the first call made rather
/// than the conflict, and makes no second one: `ClientToken` is `@idempotencyToken`.
#[tokio::test]
async fn a_replayed_client_token_answers_with_the_association_it_made() {
    let router = new_router();
    let rule_id = published_rule_ids(&router).await[0].clone();

    for token in ["a-plain-token", "11111111-2222-3333-4444-555555555555", "t"] {
        let router = new_router();
        let case = format!("clientToken {token:?}");
        let request = json!({"ruleId": rule_id, "mode": "DRY_RUN", "clientToken": token});
        let (status, first) = create_association(&router, request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case}: the first create got {status} {first:?}"
        );
        let (status, replayed) = create_association(&router, request).await;
        assert_eq!(
            status, 200,
            "case {case}: the replayed create got {status} {replayed:?}"
        );
        assert_eq!(
            replayed["ruleAssociation"], first["ruleAssociation"],
            "case {case}: a replay must answer with the association the first call made"
        );
    }
}

// ---------------------------------------------------------------------------
// GetCustomDetectionRuleOrgConfiguration: a (RuleId, Mode) pair with no
// configuration.
// ---------------------------------------------------------------------------

/// `Configuration` is `@required` on the response
/// (`smithy_query(operation="GetCustomDetectionRuleOrgConfiguration")`), so there is
/// no success to report for a pair with no configuration behind it: the operation
/// models `ResourceNotFoundException` (404) and that is the answer, never a fabricated
/// `Configuration`.
///
/// The rows differ along WHY the pair resolves to nothing: a rule the catalogue does
/// not publish, a published rule nothing configured, and a published rule configured
/// in the OTHER mode.
#[tokio::test]
async fn reading_an_absent_org_configuration_is_refused_with_the_modelled_404() {
    let router = new_router();
    let published = published_rule_ids(&router).await;

    for rule_id in [
        "e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1",
        "no-such-rule",
        published[0].as_str(),
        published[1].as_str(),
    ] {
        for mode in MODES {
            let case = format!("ruleId {rule_id:?} in mode {mode}, nothing configured");
            let (status, body) = get_org_configuration(&router, rule_id, mode).await;
            assert_modelled_error(&case, status, &body, NOT_FOUND, 404);
        }
    }

    // Configure ONE pair; the other mode of the same rule is still absent.
    let rule_id = published[0].as_str();
    let (status, body) = create_org_configuration(
        &router,
        json!({"ruleId": rule_id, "mode": "DRY_RUN", "includeAccountIds": ["111122223333"]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-org-configuration: expected 200, got {status} {body:?}"
    );

    let (status, read) = get_org_configuration(&router, rule_id, "DRY_RUN").await;
    assert_eq!(
        status, 200,
        "reading the configured pair expected 200, got {status} {read:?}"
    );
    let configuration = read["configuration"].clone();
    assert_eq!(
        configuration["ruleId"],
        json!(rule_id),
        "expected the rule it configures, got {configuration:?}"
    );
    assert_eq!(
        configuration["mode"],
        json!("DRY_RUN"),
        "expected the mode it was created for, got {configuration:?}"
    );
    assert_eq!(
        configuration["includeAccountIds"],
        json!(["111122223333"]),
        "expected the accounts the create recorded, got {configuration:?}"
    );
    assert_eq!(
        configuration["excludeAccountIds"],
        json!([]),
        "expected no excluded accounts, got {configuration:?}"
    );
    // The roll-out is a transition the CLOCK owes, and the span is never zero, so a
    // read straight after the create still observes the transient status.
    assert_eq!(
        configuration["status"],
        json!("PROCESSING"),
        "expected the transitional status straight after the create, got {configuration:?}"
    );

    let (status, body) = get_org_configuration(&router, rule_id, "LIVE").await;
    assert_modelled_error(
        "the other mode of a configured rule",
        status,
        &body,
        NOT_FOUND,
        404,
    );

    // A second create for the configured pair is the modelled conflict, and an
    // unpublished rule is the modelled 404 on the create too.
    let (status, body) =
        create_org_configuration(&router, json!({"ruleId": rule_id, "mode": "DRY_RUN"})).await;
    assert_modelled_error("a second org configuration", status, &body, CONFLICT, 409);
    let (status, body) =
        create_org_configuration(&router, json!({"ruleId": "no-such-rule", "mode": "LIVE"})).await;
    assert_modelled_error(
        "an org configuration for an unpublished rule",
        status,
        &body,
        NOT_FOUND,
        404,
    );

    // `IncludeAccountIds` and `ExcludeAccountIds` are mutually exclusive, and an
    // entry of either that is not an account id is the parameter failure.
    let (status, body) = create_org_configuration(
        &router,
        json!({
            "ruleId": published[1].as_str(),
            "mode": "LIVE",
            "includeAccountIds": ["111122223333"],
            "excludeAccountIds": ["222233334444"],
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "naming both account scopes expected 400, got {status} {body:?}"
    );
    for bad in ["abcdefghijkl", "1111-2222-33"] {
        let case = format!("includeAccountIds entry {bad:?}");
        let (status, body) = create_org_configuration(
            &router,
            json!({"ruleId": published[1].as_str(), "mode": "LIVE", "includeAccountIds": [bad]}),
        )
        .await;
        assert_bad_request(&case, status, &body, "accountId", bad);
    }
}

/// `GetCustomDetectionRuleOrgConfiguration` is
/// `GET /custom-detection-rule/org-configuration/{RuleId}?mode=` and the create is
/// `POST /custom-detection-rule/org-configuration`
/// (`smithy_query(operation="GetCustomDetectionRuleOrgConfiguration,CreateCustomDetectionRuleOrgConfiguration")`).
async fn get_org_configuration(router: &Router, rule_id: &str, mode: &str) -> (u16, Value) {
    as_delegated_admin(router, REGION).await;
    call(
        router,
        "GET",
        &format!("/custom-detection-rule/org-configuration/{rule_id}?mode={mode}"),
        None,
    )
    .await
}

async fn create_org_configuration(router: &Router, body: Value) -> (u16, Value) {
    as_delegated_admin(router, REGION).await;
    call(
        router,
        "POST",
        "/custom-detection-rule/org-configuration",
        Some(body),
    )
    .await
}
