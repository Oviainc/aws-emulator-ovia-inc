// SPDX-License-Identifier: CC0-1.0

//! What the audit record of a REFUSED call carries, for the two kinds of refusal a caller
//! can meet: one decided at the permission check, and one the operation itself answered.
//!
//! RULE: a call STOPPED AT AUTHORIZATION is recorded with no request parameters and no
//! response elements — `requestParameters` and `responseElements` are both JSON `null` —
//! because those parameters report what a call that was authorized and DISPATCHED carried.
//! A call that WAS dispatched records the parameters it was dispatched with, whatever it
//! then answered: a success, and a refusal the operation reached of its own (an entity that
//! is not there, a name the account already holds) alike. So a reader of event history
//! tells a call whose body reached the service from one stopped at the permission check,
//! and the input of a denied request reaches no record.
//!
//! The table below spans what the rule generalises over rather than the one call that
//! motivated it: which operation was refused (a create, a read, a listing, a tag write),
//! which refusal it was (no `Allow` for the request, an explicit `Deny` about it), whether
//! the request carried one member, none at all or a nested list of them, and — on the other
//! side of the rule — a dispatched call that succeeded and three the operation itself
//! refused, the last of them with the very code and status an authorization refusal
//! carries, so the rule cannot be "whatever answered 403".
//!
//! Every call goes over the wire as a real awsQuery form POST through the shared harness,
//! and the records are read back out of the SHARED log in the engine the service was built
//! from: the one store CloudTrail's `LookupEvents` reports.

use std::sync::Arc;

use serde_json::{json, Value};
use service_common::management_events::{ManagementEvent, ManagementEventLog};
use smithy_server_runtime::MultiServiceRouter;

use crate::identity_policy_enforcement::{principal, statement};
use crate::role_lifecycle::borrowed;
use crate::user_lifecycle::{call, call_signed_by, element, router_with_engine};

/// The user the rows that address ONE entity are refused or served on, created with the
/// account's own credentials: no row's decision is about this user's permissions.
const TARGET: &str = "audited-subject";

/// A user name the account holds no user for, for the row whose authorized read is refused
/// by the operation itself.
const ABSENT: &str = "audited-nobody";

/// An Organizations entity path the model's own pattern admits
/// (`GenerateOrganizationsAccessReport$EntityPath`, `organizationsEntityPathType`:
/// `^o-[0-9a-z]{10,32}\/r-[0-9a-z]{4,32}[0-9a-z-\/]*$`), naming an organization this account
/// does not manage — which is the refusal that operation's own handler answers.
const ENTITY_PATH: &str = "o-a1b2c3d4e5/r-f6g7";

/// An `Allow` for every IAM action on every resource: what the explicit-`Deny` row is
/// layered on, so that row demonstrates the `Deny` and not a missing grant.
const ALLOW_EVERYTHING: &str =
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iam:*","Resource":"*"}]}"#;

/// One row: how the calling user is set up, the call it makes, and what the record of that
/// call must carry.
struct Case {
    case: &'static str,
    /// `(PolicyName, PolicyDocument)` pairs put on the calling user, in this order.
    policies: Vec<(String, String)>,
    /// The awsQuery `Action` and its form parameters.
    action: &'static str,
    params: Vec<(String, String)>,
    /// The HTTP status the caller must see.
    status: u16,
    /// The `errorCode` the record must carry, and `None` for a served call.
    error_code: Option<&'static str>,
    /// Whether the call reached the operation at all. A call that did NOT is the one whose
    /// record carries no parameters.
    dispatched: bool,
    /// For a DISPATCHED call, the `requestParameters` the record must carry, each as a
    /// pointer into that document and the value held there.
    recorded: Vec<(&'static str, Value)>,
    /// A value the REFUSED request carried that the recorded documents may not hold, for a
    /// call stopped at authorization.
    withheld: Option<&'static str>,
}

/// The rows. Each runs against its own fresh calling user, so the documents a row puts are
/// the only ones in force when it calls.
fn cases() -> Vec<Case> {
    let tags = vec![
        ("Tags.member.1.Key".to_string(), "env".to_string()),
        ("Tags.member.1.Value".to_string(), "prod".to_string()),
    ];
    vec![
        // The class the finding was filed on: a create refused for want of permission.
        // Nothing of the refused request is in its record — not even the name it asked for.
        Case {
            case: "a create stopped at authorization",
            policies: Vec::new(),
            action: "CreateUser",
            params: vec![("UserName".to_string(), "audited-refused-user".to_string())],
            status: 403,
            error_code: Some("AccessDenied"),
            dispatched: false,
            recorded: Vec::new(),
            withheld: Some("audited-refused-user"),
        },
        // The OTHER refusal at the permission check: a statement that explicitly denies the
        // request, over a grant for everything. The record is the same.
        Case {
            case: "a create an explicit deny refused",
            policies: vec![
                ("allow-everything".to_string(), ALLOW_EVERYTHING.to_string()),
                (
                    "deny-createuser".to_string(),
                    statement("Deny", "iam:CreateUser", "*"),
                ),
            ],
            action: "CreateUser",
            params: vec![("UserName".to_string(), "audited-denied-user".to_string())],
            status: 403,
            error_code: Some("AccessDenied"),
            dispatched: false,
            recorded: Vec::new(),
            withheld: Some("audited-denied-user"),
        },
        // A READ stopped at authorization: the kind of call differs, the record does not.
        Case {
            case: "a read stopped at authorization",
            policies: Vec::new(),
            action: "GetUser",
            params: vec![("UserName".to_string(), TARGET.to_string())],
            status: 403,
            error_code: Some("AccessDenied"),
            dispatched: false,
            recorded: Vec::new(),
            withheld: Some(TARGET),
        },
        // A request that carries NO member at all: its record carries `null` rather than an
        // empty document, which is what tells it from a dispatched listing.
        Case {
            case: "a listing stopped at authorization",
            policies: Vec::new(),
            action: "ListUsers",
            params: Vec::new(),
            status: 403,
            error_code: Some("AccessDenied"),
            dispatched: false,
            recorded: Vec::new(),
            withheld: None,
        },
        // A request carrying a nested LIST of members: none of it is recorded either.
        Case {
            case: "a tag write stopped at authorization",
            policies: Vec::new(),
            action: "TagUser",
            params: {
                let mut params = vec![("UserName".to_string(), TARGET.to_string())];
                params.extend(tags.clone());
                params
            },
            status: 403,
            error_code: Some("AccessDenied"),
            dispatched: false,
            recorded: Vec::new(),
            withheld: Some("prod"),
        },
        // The other side of the rule: a call that was authorized and dispatched records the
        // parameters it was dispatched with.
        Case {
            case: "an authorized read",
            policies: vec![(
                "allow-getuser".to_string(),
                statement("Allow", "iam:GetUser", "*"),
            )],
            action: "GetUser",
            params: vec![("UserName".to_string(), TARGET.to_string())],
            status: 200,
            error_code: None,
            dispatched: true,
            recorded: vec![("/requestParameters/userName", json!(TARGET))],
            withheld: None,
        },
        // A call that was authorized and dispatched and that the OPERATION then refused:
        // the parameters stay, because the request did reach the service.
        Case {
            case: "an authorized read of an entity nothing created",
            policies: vec![(
                "allow-getuser".to_string(),
                statement("Allow", "iam:GetUser", "*"),
            )],
            action: "GetUser",
            params: vec![("UserName".to_string(), ABSENT.to_string())],
            status: 404,
            error_code: Some("NoSuchEntityException"),
            dispatched: true,
            recorded: vec![("/requestParameters/userName", json!(ABSENT))],
            withheld: None,
        },
        // The same on a WRITE the operation refused: a name the account already holds.
        Case {
            case: "an authorized create of a name the account holds",
            policies: vec![(
                "allow-createuser".to_string(),
                statement("Allow", "iam:CreateUser", "*"),
            )],
            action: "CreateUser",
            params: vec![("UserName".to_string(), TARGET.to_string())],
            status: 409,
            error_code: Some("EntityAlreadyExistsException"),
            dispatched: true,
            recorded: vec![("/requestParameters/userName", json!(TARGET))],
            withheld: None,
        },
        // A dispatched write whose request carries the nested list: every member of it is
        // recorded, which is what the refused tag write above records none of.
        Case {
            case: "an authorized tag write",
            policies: vec![(
                "allow-taguser".to_string(),
                statement("Allow", "iam:TagUser", "*"),
            )],
            action: "TagUser",
            params: {
                let mut params = vec![("UserName".to_string(), TARGET.to_string())];
                params.extend(tags.clone());
                params
            },
            status: 200,
            error_code: None,
            dispatched: true,
            recorded: vec![
                ("/requestParameters/userName", json!(TARGET)),
                ("/requestParameters/tags/0/key", json!("env")),
                ("/requestParameters/tags/0/value", json!("prod")),
            ],
            withheld: None,
        },
        // The boundary of the rule, and the reason it is not "whatever answered 403": this
        // call is refused with the SAME code and status as the rows above, by the OPERATION
        // after the permission check admitted it — the account is not the management account
        // of the organization the entity path names — so the record keeps what the call was
        // dispatched with.
        Case {
            case: "an authorized report the operation itself refused",
            policies: vec![(
                "allow-organizationsreport".to_string(),
                statement("Allow", "iam:GenerateOrganizationsAccessReport", "*"),
            )],
            action: "GenerateOrganizationsAccessReport",
            params: vec![("EntityPath".to_string(), ENTITY_PATH.to_string())],
            status: 403,
            error_code: Some("AccessDenied"),
            dispatched: true,
            recorded: vec![("/requestParameters/entityPath", json!(ENTITY_PATH))],
            withheld: None,
        },
    ]
}

/// The emulator one run drives: the router every call goes through, and the engine holding
/// the shared management-event log those calls are recorded in.
struct Audited {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Audited {
    /// A router carrying only this service over one shared engine.
    fn new() -> Self {
        let (router, engine) = router_with_engine();
        Self { router, engine }
    }

    /// The most recent record in the shared log, as the CloudTrail event document a lookup
    /// reports.
    fn last_record(&self, case: &str) -> Value {
        let log = self.engine.state::<ManagementEventLog>();
        let held = log.lock();
        let latest = held
            .iter()
            .next_back()
            .map(ManagementEvent::to_cloudtrail_event_json);
        latest.unwrap_or_else(|| {
            panic!("case {case:?}: expected the call to be recorded, got an empty log")
        })
    }

    /// How many records the shared log holds.
    fn recorded(&self) -> usize {
        let log = self.engine.state::<ManagementEventLog>();
        let held = log.lock();
        held.len()
    }
}

/// The fixture entity the rows address, created with the account's own credentials.
async fn target_user(router: &MultiServiceRouter) {
    let (status, created) = call(router, "CreateUser", &[("UserName", TARGET)]).await;
    assert_eq!(
        status, 200,
        "the fixture CreateUser must succeed, got {status}: {created}"
    );
}

/// RULE: the record of a call stopped at AUTHORIZATION carries no request parameters and no
/// response elements, and the record of a call that was authorized and DISPATCHED carries
/// the parameters it was dispatched with — whether the operation then served it or refused
/// it itself.
#[tokio::test]
async fn a_call_stopped_at_authorization_is_recorded_without_its_parameters() {
    let audited = Audited::new();
    target_user(&audited.router).await;

    for (row, case) in cases().into_iter().enumerate() {
        let user_name = format!("auditedcaller{row}");
        let access_key_id = principal(&audited.router, case.case, &user_name, &case.policies).await;
        let params = borrowed(&case.params);
        let (status, body) = call_signed_by(
            &audited.router,
            Some(&access_key_id),
            case.action,
            &params,
        )
        .await;
        assert_eq!(
            status, case.status,
            "case {:?}: expected HTTP {}, got {status}: {body}",
            case.case, case.status
        );

        let record = audited.last_record(case.case);
        assert_eq!(
            record.get("eventName"),
            Some(&json!(case.action)),
            "case {:?}: the record under test must be this call's, got {record}",
            case.case
        );
        assert_eq!(
            record.get("errorCode"),
            case.error_code.map(|code| json!(code)).as_ref(),
            "case {:?}: expected errorCode {:?}, got {record}",
            case.case,
            case.error_code
        );

        if case.dispatched {
            assert_ne!(
                record.get("requestParameters"),
                Some(&Value::Null),
                "case {:?}: a dispatched call records the parameters it was dispatched \
                 with, got {record}",
                case.case
            );
            for (pointer, expected) in &case.recorded {
                assert_eq!(
                    record.pointer(pointer),
                    Some(expected),
                    "case {:?}: expected {pointer} to be {expected}, got {record}",
                    case.case
                );
            }
            continue;
        }

        assert_eq!(
            record.get("requestParameters"),
            Some(&Value::Null),
            "case {:?}: a call stopped at authorization records no parameters, got {record}",
            case.case
        );
        assert_eq!(
            record.get("responseElements"),
            Some(&Value::Null),
            "case {:?}: a call stopped at authorization answered no values, got {record}",
            case.case
        );
        // The refusal itself is still recorded, and names the caller it was decided about:
        // what the rule withholds is the refused request's own input, not the event.
        assert_eq!(
            record.pointer("/userIdentity/accessKeyId"),
            Some(&json!(access_key_id)),
            "case {:?}: the record names the credential the refused call was signed with, \
             got {record}",
            case.case
        );
        assert!(
            record
                .get("errorMessage")
                .and_then(Value::as_str)
                .is_some_and(|message| message.contains(&user_name)),
            "case {:?}: the record carries the refusal the caller was answered with, got \
             {record}",
            case.case
        );
        if let Some(withheld) = case.withheld {
            // The event is still reported in full — the refusal the caller saw names the
            // resource the decision was made about, exactly as the caller's own message
            // does — so what the rule withholds is the recorded DOCUMENTS, which carry no
            // member of the refused request at all.
            let documents = json!({
                "requestParameters": record.get("requestParameters"),
                "responseElements": record.get("responseElements"),
            })
            .to_string();
            assert!(
                !documents.contains(withheld),
                "case {:?}: the recorded documents may carry no part of the refused request, \
                 got {withheld:?} in {documents}",
                case.case
            );
        }
    }
}

/// RULE: a call stopped at authorization is recorded ONCE and in full otherwise — the
/// record is a management event of this service, filed in its Region, with the caller and
/// the operation named — so withholding the parameters withholds nothing else.
#[tokio::test]
async fn a_refused_call_is_still_recorded_in_full_apart_from_its_parameters() {
    let audited = Audited::new();
    let access_key_id = principal(&audited.router, "a refused create", "auditedwhole", &[]).await;
    let before = audited.recorded();
    let (status, body) = call_signed_by(
        &audited.router,
        Some(&access_key_id),
        "CreateUser",
        &[("UserName", "audited-whole-user")],
    )
    .await;
    assert_eq!(
        status, 403,
        "the create must be stopped at authorization, got {status}: {body}"
    );
    let after = audited.recorded();
    assert_eq!(
        after,
        before + 1,
        "a refused call is recorded exactly once, got {} new records",
        after - before
    );

    let record = audited.last_record("a refused create");
    for (member, expected) in [
        ("eventName", json!("CreateUser")),
        (
            "eventSource",
            json!(
                service_com_amazonaws_iam_awsidentitymanagementv20100508::service::management_events::EVENT_SOURCE
            ),
        ),
        (
            "awsRegion",
            json!(
                service_com_amazonaws_iam_awsidentitymanagementv20100508::service::management_events::EVENT_REGION
            ),
        ),
        ("eventType", json!("AwsApiCall")),
        ("readOnly", json!(false)),
        ("errorCode", json!("AccessDenied")),
    ] {
        assert_eq!(
            record.get(member),
            Some(&expected),
            "case {member:?}: expected {expected}, got {record}"
        );
    }
    // The message the caller was answered with is the one the record carries.
    assert_eq!(
        record.get("errorMessage").and_then(Value::as_str),
        Some(element(&body, "Message").as_str()),
        "the record carries the refusal the caller saw, got {record}"
    );
}
