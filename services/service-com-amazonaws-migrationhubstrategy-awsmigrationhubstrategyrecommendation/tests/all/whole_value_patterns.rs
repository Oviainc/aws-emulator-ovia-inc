// SPDX-License-Identifier: CC0-1.0

//! A member's modelled `@pattern` as a requirement on the WHOLE value, end to end over
//! HTTP.
//!
//! The rule under test is a property of the SERVICE rather than of any one operation,
//! member or value: a request carrying a string that the `@pattern` its member declares
//! does not match over its whole length is refused with 400 `ValidationException` and this
//! service's one sentence for an invalid request body, before the request reaches any
//! lookup or write. Smithy reads `@pattern` as a *find* — the expression has to match
//! somewhere — so a value whose spelling the model forbids but which carries a legal
//! fragment (`has spaces` against `[0-9a-z-:]+`) otherwise reaches the handler, which can
//! only report it as a resource that is not there: a different claim about a value that can
//! name no resource at all.
//!
//! So the tables below span the classes the rule generalises over rather than the one id
//! the issue cited: every expression the service's input members carry, each offending
//! character at the start, the middle and the end of a value, and each of the places a
//! value is bound (JSON body, URI label) and nested (a structure member, a structure inside
//! a list, an element of a list of strings). The complementary table pins that the refusal
//! is scoped to values that really fail the expression: a value the whole-value reading
//! admits — including one with a space INSIDE it, which `.*\S.*` admits — is never refused
//! as an invalid request body.
//!
//! The one sentence is spelled out here independently of `src/state.rs`'s constants, so a
//! wrong message in the crate fails here rather than agreeing with itself.

use serde_json::{json, Value};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::state::patterns::shadowed_expressions;
use smithy_server_runtime::MultiServiceRouter;

use crate::import_and_assess::{call, emulator, emulator_with_collector, Answer};
use crate::server_inventory::{emulator_with, server};

/// This service's message for a value that fails a rule the model declares, whichever
/// reading of the rule decided it.
///
/// `pub(crate)` so the sibling suite that probes an id member character by character
/// (`id_pattern_probe.rs`) holds the refusal to the same sentence.
pub(crate) const INVALID_REQUEST_BODY: &str = "Invalid request body";

/// Characters outside EVERY identifier `@pattern` this service's id shapes declare: the
/// three ASCII punctuation marks a caller is most likely to try, a space, and the two
/// whitespace characters a value can carry without showing one.
///
/// Each is outside `[0-9a-z-:]+` (`AsyncTaskId`, `RecommendationTaskId`) and outside
/// `[0-9a-zA-Z-]+` (`ApplicationComponentId`), from
/// `smithy_query(query="shape",shape="AsyncTaskId,RecommendationTaskId,ApplicationComponentId")`.
/// A letter case that one admits and the other does not is a row of its own in
/// [`ids_outside_the_whole_pattern`].
///
/// `pub(crate)` so `id_pattern_probe.rs` probes an id member with the same catalog.
pub(crate) const OUTSIDE_ANY_ID: &[(&str, char)] = &[
    ("a space", ' '),
    ("an exclamation mark", '!'),
    ("a full stop", '.'),
    ("a solidus", '/'),
    ("a line feed", '\n'),
    ("a tab", '\t'),
];

/// One request this service has to refuse: what it is probing, and how to send it.
///
/// `pub(crate)` with its two constructors so `id_pattern_probe.rs` builds its rows the same
/// way rather than standing up a second harness.
pub(crate) struct Case {
    /// What the row is probing, named in every assertion.
    pub(crate) what: String,
    pub(crate) method: &'static str,
    pub(crate) uri: String,
    pub(crate) body: Option<Value>,
}

pub(crate) fn post(what: String, uri: &str, body: Value) -> Case {
    Case {
        what,
        method: "POST",
        uri: uri.to_string(),
        body: Some(body),
    }
}

fn get(what: String, uri: String) -> Case {
    Case {
        what,
        method: "GET",
        uri,
        body: None,
    }
}

/// A `StopAssessment` body naming `id`.
///
/// `pub(crate)` for the sibling suite, which drives the same operation.
pub(crate) fn stop(id: &str) -> Value {
    json!({"assessmentId": id})
}

/// A `StartImportFileTask` body carrying the three required members, with `name`,
/// `S3Bucket` and `s3key` as given — every other member absent.
fn import(name: &str, bucket: &str, key: &str) -> Value {
    json!({"name": name, "S3Bucket": bucket, "s3key": key})
}

/// One `assessmentTargets` entry, with the `condition` the model requires.
fn assessment_target(name: &str, value: &str) -> Value {
    json!({"condition": "EQUALS", "name": name, "values": [value]})
}

/// `assessmentId` values an id's `@pattern` finds a legal fragment in but does not match as
/// a whole: each offending character at the start, in the middle and at the end, plus the
/// uppercase spelling of a legal id.
///
/// `AsyncTaskId` is `[0-9a-z-:]+` within `@length(0,52)`, so a value made of legal
/// characters plus ONE outsider is inside the length bound and is refused for its spelling
/// alone. `pub(crate)` so `id_pattern_probe.rs` runs the same rows against the operations
/// that take an id.
pub(crate) fn ids_outside_the_whole_pattern() -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();
    for (named, outsider) in OUTSIDE_ANY_ID {
        rows.push((
            format!("{named} leading a legal id"),
            format!("{outsider}assessment-1"),
        ));
        rows.push((
            format!("{named} inside a legal id"),
            format!("assessment{outsider}1"),
        ));
        rows.push((
            format!("{named} trailing a legal id"),
            format!("assessment-1{outsider}"),
        ));
    }
    rows.push((
        "the uppercase spelling of a legal id".to_string(),
        "ASSESSMENT-1".to_string(),
    ));
    rows.push((
        "a legal id with an underscore".to_string(),
        "assessment_1".to_string(),
    ));
    rows
}

/// Every input member whose generated schema carries a `@pattern`, each given a value its
/// expression finds a fragment of but does not match as a whole.
///
/// The members and their expressions come from
/// `smithy_query(query="closure",operation="StopAssessment,GetAssessment,GetRecommendationReportDetails,GetApplicationComponentDetails,GetApplicationComponentStrategies,UpdateApplicationComponentConfig,UpdateServerConfig,GetImportFileTask,GetServerDetails,StartImportFileTask,StartAssessment,ListServers,ListApplicationComponents")`:
/// the id members are `[0-9a-z-:]+` (`AsyncTaskId`, `RecommendationTaskId`) or
/// `[0-9a-zA-Z-]+` (`ApplicationComponentId`), `StartImportFileTaskRequest$S3Bucket` is
/// `[0-9a-z]+[0-9a-z\.\-]*[0-9a-z]+` (`importS3Bucket`), `secretsManagerKey` is `.*`
/// (`SecretsManagerKey`) and every remaining string member carries `.*\S.*` — which no
/// value WITHOUT a line terminator can fail as a whole, so those rows carry one.
///
/// The rows differ along what the rule generalises over: which expression, which member,
/// where the value is bound (URI label or JSON body), how deep it sits (a top-level member,
/// a structure member, a structure inside a list, an element of a list of strings) and
/// which character is the offending one.
fn members_outside_the_whole_pattern() -> Vec<Case> {
    vec![
        // `[0-9a-z-:]+`, bound to the body and to a URI label.
        post(
            "StopAssessment assessmentId with a space (the recorded probe)".to_string(),
            "/stop-assessment",
            stop("has spaces"),
        ),
        get(
            "GetAssessment id uppercased in a URI label".to_string(),
            "/get-assessment/ASSESSMENT-1".to_string(),
        ),
        get(
            "GetAssessment id with a space in a URI label".to_string(),
            "/get-assessment/assessment%201".to_string(),
        ),
        get(
            "GetRecommendationReportDetails id with a full stop in a URI label".to_string(),
            "/get-recommendation-report-details/report.1".to_string(),
        ),
        // `[0-9a-zA-Z-]+`: the uppercase letters this one DOES admit, so the offending
        // character is the punctuation.
        get(
            "GetApplicationComponentDetails applicationComponentId with a full stop"
                .to_string(),
            "/get-applicationcomponent-details/Component.1".to_string(),
        ),
        get(
            "GetApplicationComponentStrategies applicationComponentId with a space"
                .to_string(),
            "/get-applicationcomponent-strategies/Component%201".to_string(),
        ),
        post(
            "UpdateApplicationComponentConfig applicationComponentId with an underscore"
                .to_string(),
            "/update-applicationcomponent-config/",
            json!({"applicationComponentId": "Component_1"}),
        ),
        // The bucket expression `[0-9a-z]+[0-9a-z\.\-]*[0-9a-z]+`, on a name that carries
        // the documented prefix, so its spelling is the only rule the row breaks.
        post(
            "StartImportFileTask S3Bucket with an underscore".to_string(),
            "/start-import-file-task",
            import(
                "import-1",
                "migrationhub-strategy-import_data",
                "servers.csv",
            ),
        ),
        // `.*\S.*`: a value carrying a line terminator, at every depth this service binds
        // one at. (`name` carries that expression because it targets this service's own
        // `String` shape, which declares it — `smithy_query(query="shape",shape="String")`.)
        post(
            "StartImportFileTask name carrying a line feed".to_string(),
            "/start-import-file-task",
            import("import\n1", "migrationhub-strategy-import", "servers.csv"),
        ),
        post(
            "UpdateServerConfig serverId carrying a line feed".to_string(),
            "/update-server-config/",
            json!({"serverId": "ab\n1"}),
        ),
        post(
            "StartImportFileTask s3key carrying a line feed".to_string(),
            "/start-import-file-task",
            import("import-1", "migrationhub-strategy-import", "servers.csv\n"),
        ),
        post(
            "ListServers filterValue carrying a line feed".to_string(),
            "/list-servers",
            json!({"serverCriteria": "OS_NAME", "filterValue": "Windows\nServer"}),
        ),
        post(
            "ListServers nextToken carrying a line feed".to_string(),
            "/list-servers",
            json!({"nextToken": "cursor\n"}),
        ),
        post(
            "StartAssessment assessmentTargets name carrying a line feed".to_string(),
            "/start-assessment",
            json!({
                "s3bucketForAnalysisData": "migrationhub-strategy-analysis",
                "assessmentTargets": [assessment_target("OS\nNAME", "Linux")],
            }),
        ),
        post(
            "StartAssessment assessmentTargets values element carrying a line feed".to_string(),
            "/start-assessment",
            json!({
                "s3bucketForAnalysisData": "migrationhub-strategy-analysis",
                "assessmentTargets": [assessment_target("OS_NAME", "Lin\nux")],
            }),
        ),
        post(
            "StartImportFileTask groupId value carrying a line feed".to_string(),
            "/start-import-file-task",
            json!({
                "name": "import-1",
                "S3Bucket": "migrationhub-strategy-import",
                "s3key": "servers.csv",
                "groupId": [{"name": "ExternalId", "value": "group\n1"}],
            }),
        ),
        post(
            "ListApplicationComponents filterValue carrying a line feed".to_string(),
            "/list-applicationcomponents",
            json!({"applicationComponentCriteria": "SERVER_ID", "filterValue": "ab\n"}),
        ),
        // `.*` on a sensitive member: a value with a line terminator is outside it as a
        // whole, because the expression's `.` matches no line terminator.
        post(
            "UpdateApplicationComponentConfig secretsManagerKey carrying a line feed".to_string(),
            "/update-applicationcomponent-config/",
            json!({"applicationComponentId": "Component-1", "secretsManagerKey": "secret\nkey"}),
        ),
    ]
}

/// Every request carrying a value outside its member's expression as a WHOLE is refused
/// with 400 `ValidationException` and the service's one sentence — whatever the expression,
/// the member, the depth and the place the value was bound.
#[tokio::test]
async fn a_value_outside_a_members_whole_pattern_is_refused() {
    let router = emulator_with_collector();

    for Case {
        what,
        method,
        uri,
        body,
    } in members_outside_the_whole_pattern()
    {
        let answer = call(&router, method, &uri, body).await;
        assert_eq!(
            answer.status, 400,
            "case {what:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {what:?}: expected ValidationException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.string("/message"),
            INVALID_REQUEST_BODY,
            "case {what:?}: expected the service's {INVALID_REQUEST_BODY:?}, got {:?}",
            answer.body
        );
    }
}

/// Requests whose every value its member's expression matches as a WHOLE. None of them is
/// an invalid request body, whatever the operation then does with it: the id may still
/// resolve to nothing, which is a different refusal with a different sentence.
///
/// The rows deliberately include values a *find* and a whole-value match read the same way
/// — a space INSIDE a `.*\S.*` value, a colon-separated `[0-9a-z-:]+` id, a bucket name
/// with the full stops its expression admits — so the check cannot be refusing more than
/// the expression forbids.
fn members_inside_the_whole_pattern() -> Vec<Case> {
    vec![
        post(
            "StopAssessment with the all-zero uuid, which [0-9a-z-:]+ admits".to_string(),
            "/stop-assessment",
            stop("00000000-0000-0000-0000-000000000000"),
        ),
        post(
            "StopAssessment with a colon-separated id".to_string(),
            "/stop-assessment",
            stop("task:0:never-created"),
        ),
        post(
            "StopAssessment with 52 characters, at AsyncTaskId's @length max".to_string(),
            "/stop-assessment",
            stop(&"a".repeat(52)),
        ),
        get(
            "GetAssessment with a legal, never-issued id".to_string(),
            "/get-assessment/assessment-ffffffffffff".to_string(),
        ),
        get(
            "GetRecommendationReportDetails with a legal, never-issued id".to_string(),
            "/get-recommendation-report-details/report:0".to_string(),
        ),
        get(
            "GetApplicationComponentDetails with a mixed-case legal id".to_string(),
            "/get-applicationcomponent-details/Component-1".to_string(),
        ),
        get(
            "GetImportFileTask with a legal id carrying a space inside it".to_string(),
            "/get-import-file-task/task%201".to_string(),
        ),
        get(
            "GetServerDetails with a legal id carrying a space inside it".to_string(),
            "/get-server-details/server%201".to_string(),
        ),
        post(
            "UpdateServerConfig with a serverId carrying a space inside it".to_string(),
            "/update-server-config/",
            json!({"serverId": "server 1"}),
        ),
        post(
            "ListServers with a filterValue carrying spaces".to_string(),
            "/list-servers",
            json!({"serverCriteria": "OS_NAME", "filterValue": "Windows Server 2019"}),
        ),
        post(
            "StartImportFileTask with an underscored name and a dotted bucket".to_string(),
            "/start-import-file-task",
            import(
                "import_1-v2",
                "migrationhub-strategy-import.data",
                "inventory/2026/servers.csv",
            ),
        ),
        post(
            "StartAssessment with targets whose values carry spaces".to_string(),
            "/start-assessment",
            json!({
                "s3bucketForAnalysisData": "migrationhub-strategy-analysis",
                "assessmentTargets": [assessment_target("OS_NAME", "Windows Server 2019")],
            }),
        ),
        post(
            "UpdateApplicationComponentConfig with a secretsManagerKey carrying spaces"
                .to_string(),
            "/update-applicationcomponent-config/",
            json!({
                "applicationComponentId": "Component-1",
                "secretsManagerKey": "a secret key",
            }),
        ),
    ]
}

/// A value its member's expression admits over its whole length is never refused as an
/// invalid request body — the refusal is scoped to a spelling the model forbids, not
/// applied to every request.
#[tokio::test]
async fn a_value_inside_a_members_whole_pattern_is_not_refused_as_invalid() {
    let router = emulator_with_collector();

    for Case {
        what,
        method,
        uri,
        body,
    } in members_inside_the_whole_pattern()
    {
        let answer = call(&router, method, &uri, body).await;
        assert_ne!(
            answer.string("/message"),
            INVALID_REQUEST_BODY,
            "case {what:?}: a value its expression admits was refused as an invalid request \
             body: {:?}",
            answer.body
        );
        assert!(
            answer.status == 200 || (400..500).contains(&answer.status),
            "case {what:?}: expected a success or a 4xx, got {} with {:?}",
            answer.status,
            answer.body
        );
    }
}

/// The refusal happens before the WRITE too: a `serverId` outside the expression leaves the
/// server's reported configuration exactly as it was, and the same call with the server's
/// own id stores the preference the read then reports.
#[tokio::test]
async fn a_refused_update_leaves_the_stored_configuration_alone() {
    let router = emulator_with(vec![server("ab")]);

    let before = read_server_strategies(&router, "ab").await;
    assert_eq!(
        before.status, 200,
        "the server that sets this case up must be reported, got {} with {:?}",
        before.status, before.body
    );

    let refused = update_server_config(&router, "ab\n", "Retirement").await;
    assert_eq!(
        refused.status, 400,
        "a serverId carrying a line feed must be refused, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.string("/message"),
        INVALID_REQUEST_BODY,
        "the refusal must be the service's invalid-request-body sentence, got {:?}",
        refused.body
    );
    let after = read_server_strategies(&router, "ab").await;
    assert_eq!(
        after.body, before.body,
        "a refused update changed what the read reports: {:?} became {:?}",
        before.body, after.body
    );

    let accepted = update_server_config(&router, "ab", "Retirement").await;
    assert_eq!(
        accepted.status, 200,
        "the same update with the server's own id must be accepted, got {} with {:?}",
        accepted.status, accepted.body
    );
    let stored = read_server_strategies(&router, "ab").await;
    assert!(
        preferred_strategies(&stored).contains(&"Retirement".to_string()),
        "the accepted update must be reported back as the preferred strategy: {:?}",
        stored.body
    );
}

async fn update_server_config(
    router: &MultiServiceRouter,
    server_id: &str,
    strategy: &str,
) -> Answer {
    call(
        router,
        "POST",
        "/update-server-config/",
        Some(json!({
            "serverId": server_id,
            "strategyOption": {"strategy": strategy, "isPreferred": true},
        })),
    )
    .await
}

async fn read_server_strategies(router: &MultiServiceRouter, server_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/get-server-strategies/{server_id}"),
        None,
    )
    .await
}

/// The strategies the answer reports as preferred, by the `strategy` of the recommendation
/// each one carries (`serverStrategies[].recommendation.strategy`).
fn preferred_strategies(answer: &Answer) -> Vec<String> {
    answer.body["serverStrategies"]
        .as_array()
        .map(|strategies| {
            strategies
                .iter()
                .filter(|strategy| strategy["isPreferred"].as_bool().unwrap_or(false))
                .map(|strategy| {
                    strategy["recommendation"]["strategy"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every expression the whole-value check holds a value to is one the MODEL still declares,
/// each read off a member rather than written out in the crate — so a renamed member or a
/// dropped `@pattern` fails here instead of quietly admitting every value for every member
/// that shares that expression.
///
/// Each row names the member the expression is read from and the expression the model
/// declares for it, from
/// `smithy_query(query="shape",shape="AsyncTaskId,ApplicationComponentId,String,importS3Bucket,SecretsManagerKey")`,
/// spelled out here independently of the crate so a shadow built from the wrong member
/// fails this test.
#[test]
fn every_expression_the_check_enforces_is_one_the_model_declares() {
    let expected: &[(&str, &str)] = &[
        (
            "StopAssessmentRequest$assessmentId (AsyncTaskId)",
            "[0-9a-z-:]+",
        ),
        (
            "GetApplicationComponentDetailsRequest$applicationComponentId (ApplicationComponentId)",
            "[0-9a-zA-Z-]+",
        ),
        ("GetImportFileTaskRequest$id (String)", r".*\S.*"),
        (
            "StartImportFileTaskRequest$S3Bucket (importS3Bucket)",
            r"[0-9a-z]+[0-9a-z\.\-]*[0-9a-z]+",
        ),
        (
            "UpdateApplicationComponentConfigRequest$secretsManagerKey (SecretsManagerKey)",
            ".*",
        ),
    ];

    let read = shadowed_expressions();
    assert_eq!(
        read.len(),
        expected.len(),
        "the check reads {} expressions and this table names {}: {read:?}",
        read.len(),
        expected.len()
    );
    for ((case, expression), actual) in expected.iter().zip(read) {
        assert_eq!(
            actual,
            Some(*expression),
            "case {case:?}: expected the model's {expression:?}, got {actual:?}"
        );
    }
}

/// An account with no data source refuses a `StartAssessment` it would otherwise accept, so
/// the pattern refusal is not standing in for that one: the two are different sentences.
#[tokio::test]
async fn the_pattern_refusal_is_not_the_services_other_refusals() {
    let router = emulator();
    let answer = call(
        &router,
        "POST",
        "/start-assessment",
        Some(json!({"s3bucketForAnalysisData": "migrationhub-strategy-analysis"})),
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "an account with no collector cannot start an assessment, got {} with {:?}",
        answer.status, answer.body
    );
    assert_ne!(
        answer.string("/message"),
        INVALID_REQUEST_BODY,
        "a legally spelled request must not be refused as an invalid request body: {:?}",
        answer.body
    );
}
