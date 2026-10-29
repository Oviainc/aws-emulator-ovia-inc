// SPDX-License-Identifier: CC0-1.0

//! How a refused parameter names ONE element of a collection, end to end over
//! HTTP.
//!
//! The rule under test: in the parameter names an `InvalidRequestException`
//! reports — in its `Message` clauses and in `RequiredParameters`, "the request
//! omitted one or more required parameters"
//! (`smithy_query(query="shape", shape="InvalidRequestException")`) — an element
//! of a collection is named by SUBSCRIPTING the collection member it belongs to
//! with the key that picks the element out of it (`Schedules[1]`, 1-based as the
//! validation path a request is walked along numbers a list element), at every
//! depth and whichever layer refused the request: the modelled-constraint walk
//! that precedes the handler, or the cross-member rules inside it. A member that
//! is not an element of anything — the collection itself included — is named by
//! the model's own member name alone, with no subscript.
//!
//! One operation answers both kinds of refusal in the same exception members, so
//! a client that reads a name to point a user at the field to correct must find
//! the same element named the same way from either; the second test is that
//! comparison for the same schedule.
//!
//! The member names and their constraints are the model's
//! (`smithy_query(query="shape", shape="PolicyDetails")`,
//! `shape="Schedule"`, `shape="CreateRule"`, `shape="CrossRegionCopyRule"`,
//! `shape="ShareRule"`, `shape="Tag"`, `shape="TimesList"`,
//! `shape="ScheduleList"`, `shape="PolicyDescription"`).
//!
//! The table spans the class rather than the one request the issue cited: a
//! member of a list element and of a list element nested inside another list
//! element, a collection member inside a list element, the first and later
//! elements of every list (including its last admitted position), a parameter
//! refused as invalid and one refused as omitted, a member that is no element at
//! all, and both `CreateLifecyclePolicy` and `UpdateLifecyclePolicy`.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dlm_dlm_20180112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for.
const REGION: &str = "us-west-2";

/// The modelled client error this service answers for a refused parameter, its
/// `@httpError` status and its `Code`.
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";
const INVALID_REQUEST_STATUS: u16 = 400;
const INVALID_PARAMETER_CODE: &str = "InvalidParameter";

/// `IntervalUnitValues` offers `HOURS` alone
/// (`smithy_query(query="shape", shape="IntervalUnitValues")`).
const CREATE_INTERVAL_UNIT: &str = "HOURS";

/// A creation interval `CreateRule$Interval`'s documentation publishes ("The
/// supported values are 1, 2, 3, 4, 6, 8, 12, and 24").
const CREATE_INTERVAL: i32 = 24;

/// The parameter that carries a policy's configuration, which every collection
/// in this file hangs off.
const POLICY_DETAILS: &str = "PolicyDetails";

/// `PolicyDetails$Schedules`, the list a schedule is an element of. Spelled from
/// the model's member name, as a parameter name is.
const SCHEDULES: &str = "PolicyDetails.Schedules";

/// `ScheduleList` is `@length(min: 1, max: 4)`, so position 4 is the last one a
/// schedule can be refused at and 5 schedules are more than the list admits.
const MAX_SCHEDULES: usize = 4;

/// The longest a `Tag` member may be (`@length(min: 0, max: 500)`), so a value
/// one character longer is outside it.
const MAX_TAG_LENGTH: usize = 500;

/// An `ExecutionRoleArn` satisfying `^arn:aws(-[a-z]{1,3}){0,2}:iam::\d+:role/`
/// in the emulator's own account, so `iam:PassRole` is granted.
fn role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/AWSDataLifecycleManagerDefaultRole",
        engine::emulator_account_id()
    )
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    fn policy(&self) -> &Value {
        &self.body["Policy"]
    }

    fn policy_id(&self) -> String {
        self.body["PolicyId"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    fn message(&self) -> &str {
        self.body["Message"]
            .as_str()
            .or_else(|| self.body["message"].as_str())
            .unwrap_or_default()
    }

    /// The names `RequiredParameters` reports, empty when the member is unset.
    fn required_parameters(&self) -> Vec<String> {
        self.body["RequiredParameters"]
            .as_array()
            .map(|names| {
                names
                    .iter()
                    .map(|name| name.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// A SigV4 credential scope naming this service — what routes the request to dlm.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/dlm/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("dlm.{REGION}.amazonaws.com"))
        .header("authorization", signed_for(REGION));
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

async fn create(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/policies", Some(request)).await
}

async fn get(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "GET", &format!("/policies/{id}"), None).await
}

async fn update(router: &MultiServiceRouter, id: &str, request: Value) -> Answer {
    call(router, "PATCH", &format!("/policies/{id}"), Some(request)).await
}

fn at(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// `text` as a `PolicyDescription`: the member is `@pattern(^[0-9A-Za-z _-]+$)`,
/// `@length(max: 500)`, so a case name is spelled in the characters it admits and
/// clipped to its bound. These rows are about the parameter NAME a refusal
/// reports, so their description must never be what is refused.
fn policy_description(text: &str) -> String {
    let spelled: String = text
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, ' ' | '_' | '-') {
                character
            } else {
                '-'
            }
        })
        .take(500)
        .collect();
    if spelled.is_empty() {
        "a policy".to_string()
    } else {
        spelled
    }
}

/// The name a refusal gives the element at `position` (1-based) of the
/// collection named `collection`.
fn element(collection: &str, position: usize) -> String {
    format!("{collection}[{position}]")
}

/// The name a refusal gives the schedule at `position` of `PolicyDetails$Schedules`.
fn schedule_at(position: usize) -> String {
    element(SCHEDULES, position)
}

/// The same name spelled the OTHER way a validation path could be read — the
/// index left as a dotted segment. No refusal may report this spelling, or one
/// element of one list is named two ways by one operation.
fn dotted(name: &str) -> String {
    name.replace('[', ".").replace(']', "")
}

/// A complete custom EBS snapshot configuration carrying `schedules`.
fn details_with(schedules: Vec<Value>) -> Value {
    json!({
        "PolicyType": "EBS_SNAPSHOT_MANAGEMENT",
        "ResourceTypes": ["VOLUME"],
        "TargetTags": [{"Key": "backup", "Value": "daily"}],
        "Schedules": Value::Array(schedules),
    })
}

/// A schedule stating exactly one form of each of its two rules — the leading
/// elements a later-position row sits behind, and the subject of an accepted
/// configuration.
fn good_schedule() -> Value {
    json!({
        "Name": "leading",
        "CreateRule": {"Interval": CREATE_INTERVAL, "IntervalUnit": CREATE_INTERVAL_UNIT},
        "RetainRule": {"Count": 1},
    })
}

/// `subject` as the element at `position` of `Schedules`, behind as many
/// acceptable schedules as it takes to put it there.
fn schedules_at(position: usize, subject: Value) -> Vec<Value> {
    let mut schedules: Vec<Value> = (1..position).map(|_| good_schedule()).collect();
    schedules.push(subject);
    schedules
}

/// A cross-Region copy rule the walk accepts, so a row refuses only the member
/// it is about.
fn good_copy_rule() -> Value {
    json!({"Target": "us-east-1", "Encrypted": false})
}

/// Which member of the exception reports the name, beside the message that
/// always states it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reported {
    /// A parameter whose VALUE is outside what the model admits: the message
    /// states it and neither structured list names it.
    Invalid,
    /// A parameter the request OMITTED: `RequiredParameters` names it.
    Required,
}

/// One refused request and the parameter name its refusal must report.
struct Row {
    case: String,
    /// The `PolicyDetails` the request carries, or `None` for a row that refuses
    /// a member outside the configuration.
    details: Option<Value>,
    /// Members merged into the request body on top of the ones it always carries.
    body: Value,
    /// The parameter name the refusal must report.
    named: String,
    reported_in: Reported,
}

impl Row {
    fn new(case: String, details: Value, named: String, reported_in: Reported) -> Self {
        Self {
            case,
            details: Some(details),
            body: json!({}),
            named,
            reported_in,
        }
    }
}

/// Every refusal whose parameter name reaches into a collection, plus the rows
/// that must carry no subscript at all.
fn rows() -> Vec<Row> {
    let too_long = "a".repeat(MAX_TAG_LENGTH + 1);
    let mut rows = Vec::new();

    // A member of a schedule, at the first element of Schedules, at a later one,
    // and at the last position the list admits: the subscript is that element's
    // own position, whatever the depth of the member below it.
    for position in [1usize, 2, MAX_SCHEDULES] {
        let schedule = schedule_at(position);

        rows.push(Row::new(
            format!("a Schedule Name outside its pattern, schedule {position}"),
            details_with(schedules_at(
                position,
                json!({
                    "Name": "not a legal name!",
                    "CreateRule": {"Interval": CREATE_INTERVAL, "IntervalUnit": CREATE_INTERVAL_UNIT},
                    "RetainRule": {"Count": 1},
                }),
            )),
            format!("{schedule}.Name"),
            Reported::Invalid,
        ));

        rows.push(Row::new(
            format!("a CronExpression outside its pattern, schedule {position}"),
            details_with(schedules_at(
                position,
                json!({
                    "CreateRule": {"CronExpression": "not-a-cron-expression"},
                    "RetainRule": {"Count": 1},
                }),
            )),
            format!("{schedule}.CreateRule.CronExpression"),
            Reported::Invalid,
        ));

        // A collection INSIDE a list element: `TimesList` is
        // `@length(min: 0, max: 1)`, and the collection itself is what is
        // refused, so it carries no subscript of its own.
        rows.push(Row::new(
            format!("more Times than the list admits, schedule {position}"),
            details_with(schedules_at(
                position,
                json!({
                    "CreateRule": {
                        "Interval": CREATE_INTERVAL,
                        "IntervalUnit": CREATE_INTERVAL_UNIT,
                        "Times": ["06:00", "07:00"],
                    },
                    "RetainRule": {"Count": 1},
                }),
            )),
            format!("{schedule}.CreateRule.Times"),
            Reported::Invalid,
        ));

        // A list element nested inside a list element: each list is subscripted
        // by the position of ITS element.
        rows.push(Row::new(
            format!("a TagsToAdd Value longer than its bound, schedule {position}"),
            details_with(schedules_at(
                position,
                json!({
                    "CreateRule": {"Interval": CREATE_INTERVAL, "IntervalUnit": CREATE_INTERVAL_UNIT},
                    "RetainRule": {"Count": 1},
                    "TagsToAdd": [
                        {"Key": "first", "Value": "kept"},
                        {"Key": "second", "Value": too_long},
                    ],
                }),
            )),
            format!("{schedule}.TagsToAdd[2].Value"),
            Reported::Invalid,
        ));

        rows.push(Row::new(
            format!("a CrossRegionCopyRule Target outside its pattern, schedule {position}"),
            details_with(schedules_at(
                position,
                json!({
                    "CreateRule": {"Interval": CREATE_INTERVAL, "IntervalUnit": CREATE_INTERVAL_UNIT},
                    "RetainRule": {"Count": 1},
                    "CrossRegionCopyRules": [{"Target": "not a target!", "Encrypted": true}],
                }),
            )),
            format!("{schedule}.CrossRegionCopyRules[1].Target"),
            Reported::Invalid,
        ));

        // `CrossRegionCopyRules` is `@length(min: 0, max: 3)`, so its last
        // admitted element is the third.
        rows.push(Row::new(
            format!("a CrossRegionCopyRule CmkArn outside its pattern, schedule {position}"),
            details_with(schedules_at(
                position,
                json!({
                    "CreateRule": {"Interval": CREATE_INTERVAL, "IntervalUnit": CREATE_INTERVAL_UNIT},
                    "RetainRule": {"Count": 1},
                    "CrossRegionCopyRules": [
                        good_copy_rule(),
                        good_copy_rule(),
                        {"Target": "eu-west-1", "Encrypted": true, "CmkArn": "not-a-key-arn"},
                    ],
                }),
            )),
            format!("{schedule}.CrossRegionCopyRules[3].CmkArn"),
            Reported::Invalid,
        ));

        rows.push(Row::new(
            format!("an empty ShareRule TargetAccounts, schedule {position}"),
            details_with(schedules_at(
                position,
                json!({
                    "CreateRule": {"Interval": CREATE_INTERVAL, "IntervalUnit": CREATE_INTERVAL_UNIT},
                    "RetainRule": {"Count": 1},
                    "ShareRules": [{"TargetAccounts": []}],
                }),
            )),
            format!("{schedule}.ShareRules[1].TargetAccounts"),
            Reported::Invalid,
        ));

        // The omitted half of the same refusal, named in `RequiredParameters`:
        // a member a nested list element must state, and the members the
        // schedule's own documented either/or rules require — the two layers
        // that build a name, over the same element.
        rows.push(Row::new(
            format!("a Tag in TargetTags stating no Key, schedule {position}"),
            json!({
                "PolicyType": "EBS_SNAPSHOT_MANAGEMENT",
                "ResourceTypes": ["VOLUME"],
                "TargetTags": [{"Key": "backup", "Value": "daily"}, {"Value": "keyless"}],
                "Schedules": Value::Array(schedules_at(position, good_schedule())),
            }),
            format!("{POLICY_DETAILS}.TargetTags[2].Key"),
            Reported::Required,
        ));

        rows.push(Row::new(
            format!("an interval CreateRule stating no Interval, schedule {position}"),
            details_with(schedules_at(
                position,
                json!({
                    "CreateRule": {"IntervalUnit": CREATE_INTERVAL_UNIT},
                    "RetainRule": {"Count": 1},
                }),
            )),
            format!("{schedule}.CreateRule.Interval"),
            Reported::Required,
        ));

        rows.push(Row::new(
            format!("a schedule stating no RetainRule beside a copy rule, schedule {position}"),
            details_with(schedules_at(
                position,
                json!({
                    "CreateRule": {"Interval": CREATE_INTERVAL, "IntervalUnit": CREATE_INTERVAL_UNIT},
                    "CrossRegionCopyRules": [good_copy_rule()],
                }),
            )),
            format!("{schedule}.RetainRule"),
            Reported::Required,
        ));
    }

    // And the members that are no element of anything: the collection itself,
    // refused for how many elements it holds, and a parameter outside every
    // collection. Neither may carry a subscript.
    rows.push(Row::new(
        "more schedules than the list admits".to_string(),
        details_with(
            (0..MAX_SCHEDULES + 1)
                .map(|_| good_schedule())
                .collect::<Vec<Value>>(),
        ),
        SCHEDULES.to_string(),
        Reported::Invalid,
    ));
    rows.push(Row {
        case: "a Description outside its pattern".to_string(),
        details: None,
        body: json!({"Description": "not a legal description!"}),
        named: "Description".to_string(),
        reported_in: Reported::Invalid,
    });

    rows
}

/// Assert `answer` is this service's modelled refusal: its status, its `Code` and
/// a message saying why.
fn assert_refusal(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, INVALID_REQUEST_STATUS,
        "case {case:?}: expected HTTP {INVALID_REQUEST_STATUS}, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        answer.error_code().ends_with(INVALID_REQUEST_EXCEPTION),
        "case {case:?}: expected the modelled {INVALID_REQUEST_EXCEPTION}, got {:?}",
        answer.error_code()
    );
    assert_eq!(
        at(&answer.body, "/Code"),
        json!(INVALID_PARAMETER_CODE),
        "case {case:?}: expected Code {INVALID_PARAMETER_CODE:?}, got {:?}",
        at(&answer.body, "/Code")
    );
}

/// Assert the refusal names `named` where this class of refusal reports a
/// parameter, and nowhere names it the other way an index could be spelled.
fn assert_names(case: &str, answer: &Answer, named: &str, reported_in: Reported) {
    assert_refusal(case, answer);
    assert!(
        answer.message().contains(named),
        "case {case:?}: the refusal must name {named:?}, got {:?}",
        answer.message()
    );
    let required = answer.required_parameters();
    match reported_in {
        Reported::Required => assert!(
            required.iter().any(|parameter| parameter == named),
            "case {case:?}: RequiredParameters must name {named:?}, got {required:?}"
        ),
        Reported::Invalid => assert!(
            required.is_empty(),
            "case {case:?}: an invalid value is not an omitted parameter, got \
             RequiredParameters {required:?}"
        ),
    }
    let other_spelling = dotted(named);
    if other_spelling != named {
        assert!(
            !answer.message().contains(other_spelling.as_str()),
            "case {case:?}: the refusal must not also spell the element {other_spelling:?}, got \
             {:?}",
            answer.message()
        );
        assert!(
            !required
                .iter()
                .any(|parameter| parameter == other_spelling.as_str()),
            "case {case:?}: RequiredParameters must not spell the element {other_spelling:?}, got \
             {required:?}"
        );
    }
    // Nothing BELOW the name is an element of it: the member the refusal names
    // is where the parameter ends, so no subscript follows it. This is what
    // holds the rule's other half for a member that is no element at all \u2014 the
    // collection itself included.
    let subscripted = format!("{named}[");
    assert!(
        !answer.message().contains(subscripted.as_str()),
        "case {case:?}: {named:?} is the parameter refused, so no element of it may be named, got \
         {:?}",
        answer.message()
    );
}

/// A request carrying `details` and `body` on top of the members every row
/// states.
fn request_for(row: &Row) -> Value {
    let mut request = json!({
        "ExecutionRoleArn": role_arn(),
        "Description": policy_description(&row.case),
        "State": "ENABLED",
    });
    if let Some(details) = &row.details {
        request[POLICY_DETAILS] = details.clone();
    }
    for (member, value) in row.body.as_object().expect("a body is a JSON object") {
        request[member] = value.clone();
    }
    request
}

/// RULE — an element of a collection is named by subscripting the collection
/// member with the element's position, at every depth and whichever layer
/// refuses the request; a member that is no element carries no subscript.
///
/// Every row is driven through `CreateLifecyclePolicy` and
/// `UpdateLifecyclePolicy`, which answer the same exception.
#[tokio::test]
async fn a_refused_parameter_names_a_collection_element_by_subscript() {
    let router = emulator();

    // The subject of the update rows, holding a configuration the service
    // accepts — which also proves a row is refused for the member it is about
    // and not for the configuration around it.
    let subject = create(
        &router,
        request_for(&Row::new(
            "element naming subject".to_string(),
            details_with(vec![good_schedule()]),
            String::new(),
            Reported::Invalid,
        )),
    )
    .await;
    assert_eq!(
        subject.status, 200,
        "the subject policy must be created, got {} with {:?}",
        subject.status, subject.body
    );
    let subject_id = subject.policy_id();
    let stored = at(get(&router, &subject_id).await.policy(), "/PolicyDetails");

    for row in rows() {
        let case = format!("{} (create)", row.case);
        let answer = create(&router, request_for(&row)).await;
        assert_names(&case, &answer, &row.named, row.reported_in);

        let case = format!("{} (update)", row.case);
        let answer = update(&router, &subject_id, request_for(&row)).await;
        assert_names(&case, &answer, &row.named, row.reported_in);
        assert_eq!(
            at(get(&router, &subject_id).await.policy(), "/PolicyDetails"),
            stored,
            "case {case:?}: a refused update must leave the stored configuration alone"
        );
    }
}

/// The element path a reported parameter name begins with: the subscripted
/// `Schedules` element, up to the schedule member that follows it.
fn schedule_element_in(case: &str, reported: &str) -> String {
    let from = reported.find(SCHEDULES).unwrap_or_else(|| {
        panic!("case {case:?}: expected a name under {SCHEDULES:?}, got {reported:?}")
    });
    let rest = &reported[from..];
    let to = rest.find(".CreateRule").unwrap_or_else(|| {
        panic!("case {case:?}: expected the schedule's CreateRule to be named, got {reported:?}")
    });
    rest[..to].to_string()
}

/// RULE, the half a single refusal cannot show — the two layers that build a
/// parameter name agree on it. The constraint walk ahead of the handler and the
/// cross-member rules inside it refuse two members of the SAME schedule, and
/// each names that schedule the one way.
#[tokio::test]
async fn the_boundary_and_the_handler_name_one_schedule_the_same_way() {
    let router = emulator();

    for position in 1..=MAX_SCHEDULES {
        let expected = schedule_at(position);

        // Refused by the modelled-constraint walk: a value outside
        // `CreateRule$CronExpression`'s `@pattern`.
        let case = format!("the boundary refusing schedule {position}");
        let boundary = create(
            &router,
            request_for(&Row::new(
                case.clone(),
                details_with(schedules_at(
                    position,
                    json!({
                        "CreateRule": {"CronExpression": "not-a-cron-expression"},
                        "RetainRule": {"Count": 1},
                    }),
                )),
                String::new(),
                Reported::Invalid,
            )),
        )
        .await;
        assert_refusal(&case, &boundary);
        let from_boundary = schedule_element_in(&case, boundary.message());

        // Refused by the handler's cross-member rule: the interval form of the
        // same `CreateRule`, stating no `Interval`.
        let case = format!("the handler refusing schedule {position}");
        let handler = create(
            &router,
            request_for(&Row::new(
                case.clone(),
                details_with(schedules_at(
                    position,
                    json!({
                        "CreateRule": {"IntervalUnit": CREATE_INTERVAL_UNIT},
                        "RetainRule": {"Count": 1},
                    }),
                )),
                String::new(),
                Reported::Invalid,
            )),
        )
        .await;
        assert_refusal(&case, &handler);
        let required = handler.required_parameters();
        let reported = required.first().cloned().unwrap_or_else(|| {
            panic!(
                "case {case:?}: RequiredParameters must name the omitted member, got {:?}",
                handler.body
            )
        });
        let from_handler = schedule_element_in(&case, &reported);

        assert_eq!(
            from_boundary, expected,
            "case {case:?}: the boundary must name the schedule {expected:?}, got \
             {from_boundary:?}"
        );
        assert_eq!(
            from_handler, from_boundary,
            "case {case:?}: one schedule must be named one way, got {from_handler:?} from the \
             handler and {from_boundary:?} from the boundary"
        );
    }
}
