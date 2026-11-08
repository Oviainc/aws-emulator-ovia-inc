// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a refusal, end to end over real HTTP.
//!
//! Three rules, each asserted over a SET of inputs rather than one recorded request:
//!
//! 1. An input-constraint refusal states the AWS envelope
//!    (`N validation error(s) detected: `) over the framework's own clauses, quoting
//!    every `@pattern` without the `^` / `$` the model spells it with — and withholding
//!    the rejected value for a member that carries a caller-chosen name, an
//!    encryption-key reference or a resource identifier the request URI binds
//!    (`@httpLabel`), where every other member's clause echoes it.
//! 2. A `nextToken` this service did not issue is refused with ONE fixed sentence that
//!    does not vary with the token supplied.
//! 3. A request naming a resource that does not exist is refused with a text that is a
//!    constant of the resource KIND and carries no identifier from the request.
//!
//! The harness is the sibling suite's (`crate::datastore_lifecycle`): real requests
//! through `MultiServiceRouter::dispatch`, nothing reaching into the state struct.

use serde_json::{json, Value};

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::DATASTORE_DELETION_DELAY;

use crate::datastore_lifecycle::{Emulator, SETTLE_MULTIPLE};
use crate::page_token_payload_forgery::{split_issued_token, token_carrying};

/// The `KmsKeyArn` `@pattern` with its anchors stripped — the expression this service
/// advertises. Source:
/// `smithy_query(query="shape",shape="KmsKeyArn")` -> pattern
/// `^arn:aws[a-zA-Z-]{0,16}:kms:…$`.
const KMS_KEY_ARN_PATTERN: &str = r"arn:aws[a-zA-Z-]{0,16}:kms:[a-z]{2}(-[a-z]{1,16}){1,3}-\d{1}:\d{12}:((key/[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12})|(alias/[a-zA-Z0-9:/_-]{1,256}))";

/// `ClientToken`'s `@pattern`, unanchored
/// (`smithy_query(query="shape",shape="ClientToken")`: len 1..64, `^[A-Za-z0-9._-]+$`).
const CLIENT_TOKEN_PATTERN: &str = r"[A-Za-z0-9._-]+";

/// `DatastoreName`'s `@pattern`, unanchored
/// (`smithy_query(query="shape",shape="DatastoreName")`: len 1..256,
/// `^[A-Za-z0-9._/#-]+$`).
const DATASTORE_NAME_PATTERN: &str = r"[A-Za-z0-9._/#-]+";

/// `LambdaArn`'s `@pattern`, unanchored
/// (`smithy_query(query="shape",shape="LambdaArn")`: pattern only, no length bound).
const LAMBDA_ARN_PATTERN: &str = r"arn:(aws[a-zA-Z-]*)?:lambda:[a-z]{2}((-gov)|(-iso(b?)))?-[a-z]+-\d{1}:\d{12}:function:[a-zA-Z0-9-_]+(:(\$LATEST|[a-zA-Z0-9-_]+))?";

/// `JobName`'s `@pattern`, unanchored
/// (`smithy_query(query="shape",shape="JobName")`: len 1..64, `^[A-Za-z0-9._/#-]+$`).
const JOB_NAME_PATTERN: &str = r"[A-Za-z0-9._/#-]+";

/// `NextToken`'s `@pattern`, unanchored
/// (`smithy_query(query="shape",shape="NextToken")`: len 1..8192,
/// `^\p{ASCII}{0,8192}$`).
const NEXT_TOKEN_PATTERN: &str = r"\p{ASCII}{0,8192}";

/// `RoleArn`'s `@pattern`, unanchored
/// (`smithy_query(query="shape",shape="RoleArn")`: len 20..2048,
/// `^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$`).
const ROLE_ARN_PATTERN: &str = r"arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+";

/// `DatastoreId`'s `@pattern`, unanchored
/// (`smithy_query(query="shape",shape="DatastoreId")`: `^[0-9a-z]{32}$`).
const DATASTORE_ID_PATTERN: &str = r"[0-9a-z]{32}";

/// The service's fixed invalid-`NextToken` sentence, as the wire corpus records it.
const INVALID_NEXT_TOKEN: &str =
    "Failed to perform requested operation due to invalid NextToken. Please try again.";

/// The service's constant text for a data store it does not hold, as the wire corpus
/// records it — identically from four different operations, which is what makes it a
/// constant of the service rather than of one request.
const DATASTORE_NOT_FOUND: &str = "Datastore resource is not found.";

/// A well-formed `DatastoreId` (`^[0-9a-z]{32}$`) no `CreateDatastore` in these cases
/// minted, so every operation below reaches its not-found path rather than a refusal
/// about the id's shape.
fn unknown_ids() -> [String; 4] {
    [
        "0".repeat(32),
        "z".repeat(32),
        // The identifier the recorded fixture probed: one row of the class, not the
        // requirement.
        "0f1e2d3c4b5a69788796a5b4c3d2e1f0".to_string(),
        "abcdefghijklmnopqrstuvwxyz012345".to_string(),
    ]
}

/// A body `StartDICOMImportJob` accepts on every member but the one a case breaks.
fn import_job_body(client_token: &str) -> Value {
    json!({
        "clientToken": client_token,
        "dataAccessRoleArn": "arn:aws:iam::000000000000:role/ImportRole",
        "inputS3Uri": "s3://imaging-in/",
        "outputS3Uri": "s3://imaging-out/",
    })
}

// ---------------------------------------------------------------------------
// 1. The constraint-violation envelope
// ---------------------------------------------------------------------------

/// One row of the constraint-clause table.
#[derive(Debug)]
struct ClauseCase {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    /// What the request put in the offending member, and whether this member's clause
    /// may carry it: a key-reference member withholds it, every other echoes it.
    offending: String,
    withholds_value: bool,
    /// Every clause the answer must state, in order: one per violation the request
    /// commits. A value one past a length bound that ALSO leaves its `@pattern`
    /// unsatisfiable commits two, and the envelope says so.
    clauses: Vec<String>,
}

/// A clause for a member that WITHHOLDS the rejected value: the member path, then the
/// requirement. The recorded form for a member carrying a caller-chosen name, an
/// encryption-key reference or a resource identifier the request URI binds.
fn clause(member: &str, requirement: &str) -> String {
    format!("{member} failed to satisfy constraint: {requirement}")
}

/// A clause that echoes the rejected value, which is every other member's form.
///
/// A value past 256 bytes is echoed as a 256-character prefix plus its byte count (the
/// framework's cap on how much of a request one clause may reflect), so the expectation
/// is built the same way rather than from the whole value.
fn clause_with_value(value: &str, member: &str, requirement: &str) -> String {
    let shown = if value.len() <= 256 {
        value.to_string()
    } else {
        let prefix: String = value.chars().take(256).collect();
        format!("{prefix}... [{} bytes]", value.len())
    };
    format!("Value '{shown}' at '{member}' failed to satisfy constraint: {requirement}")
}

/// A clause for a member that was absent altogether.
fn clause_for_absent(member: &str) -> String {
    format!("Value null at '{member}' failed to satisfy constraint: Member must not be null")
}

/// The requirement half of a `@pattern` clause.
fn pattern_requirement(unanchored: &str) -> String {
    format!("Member must satisfy regular expression pattern: {unanchored}")
}

/// The envelope `clauses` must arrive in: singular for one, plural joined with `; `.
fn envelope(clauses: &[String]) -> String {
    match clauses.len() {
        1 => format!("1 validation error detected: {}", clauses[0]),
        n => format!("{n} validation errors detected: {}", clauses.join("; ")),
    }
}

/// RULE: an input-constraint refusal is HTTP 400 `ValidationException` whose message is
/// the AWS envelope `N validation error(s) detected: ` over one clause per violation,
/// every `@pattern` requirement quoting the expression WITHOUT the model's `^` / `$`
/// anchors — and a member carrying a caller-chosen NAME, an encryption-key reference or a
/// resource identifier the request URI binds (`@httpLabel`) states its clause as
/// `<memberPath> failed to satisfy constraint: <requirement>`, withholding the rejected
/// value that every other member's clause echoes.
///
/// Rows span the axes the rule generalises over: the constraint CLASS (pattern, both
/// length bounds, both range bounds, an absent required member), the member (body
/// member, query parameter, URI label) and whether it withholds its value — including
/// `jobName`, a name member the issue never cited — the operation (CreateDatastore,
/// ListDatastores, StartDICOMImportJob, GetDICOMImportJob, GetDatastore) and, for one
/// pattern, two different offending values, which get the same message.
#[tokio::test]
async fn a_constraint_refusal_states_the_services_clause_for_every_member() {
    let unknown = "0".repeat(32);
    // 513 characters, one past `KmsKeyArn`'s published maximum of 512. No value that
    // long can satisfy the member's `@pattern` as well (the pattern's longest match is
    // shorter than the bound), so this row commits TWO violations and the plural
    // envelope is what it must get.
    let over_long_kms_arn = {
        let prefix = "arn:aws:kms:us-east-1:000000000000:alias/k";
        format!("{prefix}{}", "a".repeat(513 - prefix.len()))
    };
    let over_long_token = "a".repeat(8193);

    let rows: Vec<ClauseCase> = vec![
        ClauseCase {
            case: "CreateDatastore with a kmsKeyArn outside the pattern",
            method: "POST",
            uri: "/datastore".to_string(),
            body: Some(json!({
                "clientToken": "kms-pattern-1",
                "kmsKeyArn": "arn:aws:kms:us-east-1:000000000000:not-a-valid-key-ref",
            })),
            offending: "arn:aws:kms:us-east-1:000000000000:not-a-valid-key-ref".to_string(),
            withholds_value: true,
            clauses: vec![clause(
                "kmsKeyArn",
                &pattern_requirement(KMS_KEY_ARN_PATTERN),
            )],
        },
        ClauseCase {
            case: "CreateDatastore with a wholly different bad kmsKeyArn",
            method: "POST",
            uri: "/datastore".to_string(),
            body: Some(json!({
                "clientToken": "kms-pattern-2",
                "kmsKeyArn": "kms-key-1",
            })),
            offending: "kms-key-1".to_string(),
            withholds_value: true,
            clauses: vec![clause(
                "kmsKeyArn",
                &pattern_requirement(KMS_KEY_ARN_PATTERN),
            )],
        },
        ClauseCase {
            case: "CreateDatastore with a kmsKeyArn one character past the length bound",
            method: "POST",
            uri: "/datastore".to_string(),
            body: Some(json!({
                "clientToken": "kms-length",
                "kmsKeyArn": over_long_kms_arn.clone(),
            })),
            offending: over_long_kms_arn.clone(),
            withholds_value: true,
            clauses: vec![
                clause(
                    "kmsKeyArn",
                    "Member must have length less than or equal to 512",
                ),
                clause("kmsKeyArn", &pattern_requirement(KMS_KEY_ARN_PATTERN)),
            ],
        },
        ClauseCase {
            // The other side of the rule: an ARN naming another AWS resource is not a
            // name and not a key reference, so its clause echoes the value.
            case: "CreateDatastore with a lambdaAuthorizerArn outside the pattern",
            method: "POST",
            uri: "/datastore".to_string(),
            body: Some(json!({
                "clientToken": "lambda-pattern",
                "lambdaAuthorizerArn": "not-a-lambda-arn",
            })),
            offending: "not-a-lambda-arn".to_string(),
            withholds_value: false,
            clauses: vec![clause_with_value(
                "not-a-lambda-arn",
                "lambdaAuthorizerArn",
                &pattern_requirement(LAMBDA_ARN_PATTERN),
            )],
        },
        ClauseCase {
            case: "CreateDatastore with a clientToken outside the pattern",
            method: "POST",
            uri: "/datastore".to_string(),
            body: Some(json!({ "clientToken": "token with spaces!" })),
            offending: "token with spaces!".to_string(),
            withholds_value: false,
            clauses: vec![clause_with_value(
                "token with spaces!",
                "clientToken",
                &pattern_requirement(CLIENT_TOKEN_PATTERN),
            )],
        },
        ClauseCase {
            case: "CreateDatastore with a clientToken one character past the length bound",
            method: "POST",
            uri: "/datastore".to_string(),
            body: Some(json!({ "clientToken": "a".repeat(65) })),
            offending: "a".repeat(65),
            withholds_value: false,
            clauses: vec![clause_with_value(
                &"a".repeat(65),
                "clientToken",
                "Member must have length less than or equal to 64",
            )],
        },
        ClauseCase {
            case: "CreateDatastore with no clientToken at all",
            method: "POST",
            uri: "/datastore".to_string(),
            body: Some(json!({ "datastoreName": "no.token" })),
            offending: String::new(),
            withholds_value: false,
            clauses: vec![clause_for_absent("clientToken")],
        },
        ClauseCase {
            case: "CreateDatastore with a datastoreName outside the pattern",
            method: "POST",
            uri: "/datastore".to_string(),
            body: Some(json!({
                "clientToken": "name-pattern",
                "datastoreName": "bad name",
            })),
            offending: "bad name".to_string(),
            withholds_value: true,
            clauses: vec![clause(
                "datastoreName",
                &pattern_requirement(DATASTORE_NAME_PATTERN),
            )],
        },
        ClauseCase {
            case: "CreateDatastore with a datastoreName one character past the length bound",
            method: "POST",
            uri: "/datastore".to_string(),
            body: Some(json!({
                "clientToken": "name-length",
                "datastoreName": "n".repeat(257),
            })),
            offending: "n".repeat(257),
            withholds_value: true,
            clauses: vec![clause(
                "datastoreName",
                "Member must have length less than or equal to 256",
            )],
        },
        ClauseCase {
            case: "ListDatastores with maxResults below the published minimum",
            method: "GET",
            uri: "/datastore?maxResults=0".to_string(),
            body: None,
            offending: String::new(),
            withholds_value: false,
            clauses: vec![clause_with_value(
                "0",
                "maxResults",
                "Member must have value greater than or equal to 1",
            )],
        },
        ClauseCase {
            case: "ListDatastores with maxResults above the published maximum",
            method: "GET",
            uri: "/datastore?maxResults=51".to_string(),
            body: None,
            offending: String::new(),
            withholds_value: false,
            clauses: vec![clause_with_value(
                "51",
                "maxResults",
                "Member must have value less than or equal to 50",
            )],
        },
        ClauseCase {
            // One past `NextToken`'s length bound is also one past the longest value
            // its `@pattern` admits, so this row commits two violations too.
            case: "ListDatastores with a nextToken one character past the length bound",
            method: "GET",
            uri: format!("/datastore?nextToken={over_long_token}"),
            body: None,
            offending: over_long_token.clone(),
            withholds_value: false,
            clauses: vec![
                clause_with_value(
                    &over_long_token,
                    "nextToken",
                    "Member must have length less than or equal to 8192",
                ),
                clause_with_value(
                    &over_long_token,
                    "nextToken",
                    &pattern_requirement(NEXT_TOKEN_PATTERN),
                ),
            ],
        },
        ClauseCase {
            case: "StartDICOMImportJob with a dataAccessRoleArn outside the pattern",
            method: "POST",
            uri: format!("/startDICOMImportJob/datastore/{unknown}"),
            body: Some(json!({
                "clientToken": "role-pattern",
                "dataAccessRoleArn": "not-a-role-arn",
                "inputS3Uri": "s3://imaging-in/",
                "outputS3Uri": "s3://imaging-out/",
            })),
            offending: "not-a-role-arn".to_string(),
            withholds_value: false,
            clauses: vec![
                clause_with_value(
                    "not-a-role-arn",
                    "dataAccessRoleArn",
                    "Member must have length greater than or equal to 20",
                ),
                clause_with_value(
                    "not-a-role-arn",
                    "dataAccessRoleArn",
                    &pattern_requirement(ROLE_ARN_PATTERN),
                ),
            ],
        },
        ClauseCase {
            // `jobName` is a name member too (`JobName`: len 1..64,
            // `^[A-Za-z0-9._/#-]+$`), and the issue cited none of it: the rule covers
            // it because of what the member IS, not because it was recorded.
            case: "StartDICOMImportJob with a jobName outside the pattern",
            method: "POST",
            uri: format!("/startDICOMImportJob/datastore/{unknown}"),
            body: Some(json!({
                "clientToken": "job-name-pattern",
                "jobName": "bad job name!!",
                "dataAccessRoleArn": "arn:aws:iam::000000000000:role/ImportRole",
                "inputS3Uri": "s3://imaging-in/",
                "outputS3Uri": "s3://imaging-out/",
            })),
            offending: "bad job name!!".to_string(),
            withholds_value: true,
            clauses: vec![clause("jobName", &pattern_requirement(JOB_NAME_PATTERN))],
        },
        ClauseCase {
            // A URI LABEL: the identifier the request addressed a resource by. Its clause
            // carries no value — the rejected value is the path of the URI the caller
            // sent, not something a member of the request held — so every `@httpLabel` of
            // this service withholds it, `jobId` as much as the `datastoreId` beside it.
            case: "GetDICOMImportJob with a jobId past the length bound",
            method: "GET",
            uri: format!(
                "/getDICOMImportJob/datastore/{unknown}/job/{}",
                "j".repeat(33)
            ),
            body: None,
            offending: "j".repeat(33),
            withholds_value: true,
            clauses: vec![clause(
                "jobId",
                "Member must have length less than or equal to 32",
            )],
        },
        ClauseCase {
            // The same class, on the other label of the same URI and under another
            // constraint: `datastoreId` outside its `@pattern` is the clause the corpus
            // records, and it withholds the value too.
            case: "GetDatastore with a datastoreId outside the pattern",
            method: "GET",
            uri: "/datastore/not-a-valid-datastore-id!!".to_string(),
            body: None,
            offending: "not-a-valid-datastore-id!!".to_string(),
            withholds_value: true,
            clauses: vec![clause(
                "datastoreId",
                &pattern_requirement(DATASTORE_ID_PATTERN),
            )],
        },
    ];

    let emulator = Emulator::new();

    for row in &rows {
        let answer = emulator.call(row.method, &row.uri, row.body.clone()).await;
        let expected = envelope(&row.clauses);
        assert_eq!(
            answer.status, 400,
            "case {:?}: expected HTTP 400, got {answer:?}",
            row.case
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {:?}: expected ValidationException, got {answer:?}",
            row.case
        );
        let message = answer.string(&["message"]);
        assert_eq!(
            message, expected,
            "case {:?}: expected message {expected:?}, got {message:?}",
            row.case
        );
        // The structural halves of the rule, asserted on their own so a future
        // rewording cannot satisfy the equality above by accident: a withholding
        // member's clause carries neither the `Value '…' at` prefix nor the value, and
        // no clause anywhere quotes an anchored pattern.
        assert!(
            !message.contains("pattern: ^") && !message.ends_with('$'),
            "case {:?}: a pattern must be quoted unanchored, got {message:?}",
            row.case
        );
        if row.withholds_value {
            assert!(
                !message.contains("Value ") && !message.contains(row.offending.as_str()),
                "case {:?}: this member's value must not be echoed, got {message:?}",
                row.case
            );
        } else if !row.offending.is_empty() && row.offending.len() <= 256 {
            assert!(
                message.contains(row.offending.as_str()),
                "case {:?}: expected the clause to echo {:?}, got {message:?}",
                row.case,
                row.offending
            );
        }
    }
}

/// RULE, plural arm: two violations on one request are stated as
/// `2 validation errors detected: ` over the same clauses, joined with `; `.
///
/// An empty `clientToken` breaks both its length minimum and its pattern, and the walk
/// checks length before pattern.
#[tokio::test]
async fn two_violations_are_stated_in_the_plural_envelope() {
    let emulator = Emulator::new();
    let answer = emulator
        .create_datastore(json!({ "clientToken": "" }))
        .await;

    assert_eq!(
        answer.status, 400,
        "case \"empty clientToken\": expected HTTP 400, got {answer:?}"
    );
    let message = answer.string(&["message"]);
    let expected = envelope(&[
        clause_with_value(
            "",
            "clientToken",
            "Member must have length greater than or equal to 1",
        ),
        clause_with_value(
            "",
            "clientToken",
            &pattern_requirement(CLIENT_TOKEN_PATTERN),
        ),
    ]);
    assert_eq!(
        message, expected,
        "case \"empty clientToken\": expected message {expected:?}, got {message:?}"
    );
}

// ---------------------------------------------------------------------------
// 2. The invalid-NextToken sentence
// ---------------------------------------------------------------------------

/// RULE: a `nextToken` this service did not issue is refused with HTTP 400
/// `ValidationException` carrying one FIXED sentence, whatever the token was — and a
/// token it DID issue is still honoured.
///
/// Rows span the token axis: not hex at all, odd-length hex, hex that decodes to text
/// without this service's marker, a real token truncated, a real token's marker over a
/// payload this service would never have written (an unparseable instant, no instant
/// separator, an identifier outside the form it mints), and a maximum-length token.
#[tokio::test]
async fn an_unissued_pagination_token_is_refused_with_the_services_fixed_text() {
    let emulator = Emulator::new();
    for index in 0..3 {
        emulator
            .active_datastore(&format!("token.store.{index}"), &format!("token-{index}"))
            .await;
    }

    // A token this service really issued, so the mutated forms below differ from a
    // working one in exactly the way each row names.
    let first_page = emulator.list_datastores("maxResults=1").await;
    let issued = first_page
        .field(&["nextToken"])
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !issued.is_empty(),
        "case \"issued token\": expected a nextToken on a one-per-page listing, got {first_page:?}"
    );

    // The marker the token above really carries, read back OFF it
    // (`crate::page_token_payload_forgery::split_issued_token`) rather than spelled again
    // here: a marker names the collection — this listing, its region and its filter — so
    // a literal copy of one leaves these rows refused at the marker check instead of at
    // the payload branch each names.
    let (marker, _, identifier) = split_issued_token("issued token", &issued);

    let rows: Vec<(&str, String)> = vec![
        ("not hex at all", "not-a-token".to_string()),
        ("odd-length hex", "abc".to_string()),
        ("hex decoding to text without the marker", "6161".to_string()),
        (
            "hex decoding to a marker-less cursor",
            ::hex::encode("12345.0:0f1e2d3c4b5a69788796a5b4c3d2e1f0"),
        ),
        (
            "a real token cut down to its first bytes",
            issued[..10].to_string(),
        ),
        (
            "the marker over an unparseable instant",
            token_carrying(&marker, &format!("notanumber.0:{identifier}")),
        ),
        (
            "the marker over a payload with no instant separator",
            token_carrying(&marker, &format!("12345:{identifier}")),
        ),
        (
            "the marker over an identifier this service never minted",
            token_carrying(&marker, &format!("12345.0:{}", "z".repeat(32))),
        ),
        ("a maximum-length token", "a".repeat(8192)),
    ];

    for (case, token) in &rows {
        let answer = emulator
            .list_datastores(&format!("nextToken={token}"))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        let message = answer.string(&["message"]);
        assert_eq!(
            message, INVALID_NEXT_TOKEN,
            "case {case:?}: expected message {INVALID_NEXT_TOKEN:?}, got {message:?}"
        );
        assert!(
            !message.contains(token.as_str()),
            "case {case:?}: the token must not be echoed, got {message:?}"
        );
    }

    // The control: the token the service issued still serves the next page, so the
    // refusal above is about the token and not about paging.
    let resumed = emulator
        .list_datastores(&format!("maxResults=1&nextToken={issued}"))
        .await;
    assert_eq!(
        resumed.status, 200,
        "case \"issued token\": expected the issued token to page, got {resumed:?}"
    );
}

// ---------------------------------------------------------------------------
// 3. The not-found constant
// ---------------------------------------------------------------------------

/// RULE: a request naming a data store this service does not hold is refused with HTTP
/// 404 `ResourceNotFoundException` whose message is a constant of the resource kind —
/// no identifier from the request appears in it.
///
/// Rows span both axes the rule generalises over: the operation (every one in this
/// slice that resolves a data store) and the identifier (four distinct well-formed ids,
/// one of them the recorded fixture's).
#[tokio::test]
async fn a_missing_datastore_is_refused_with_the_services_constant_text() {
    let emulator = Emulator::new();

    for datastore_id in unknown_ids() {
        let job_id = "j".repeat(8);
        let probes: Vec<(&str, &str, String, Option<Value>)> = vec![
            (
                "GetDatastore",
                "GET",
                format!("/datastore/{datastore_id}"),
                None,
            ),
            (
                "DeleteDatastore",
                "DELETE",
                format!("/datastore/{datastore_id}"),
                None,
            ),
            (
                "GetDICOMImportJob",
                "GET",
                format!("/getDICOMImportJob/datastore/{datastore_id}/job/{job_id}"),
                None,
            ),
            (
                "StartDICOMImportJob",
                "POST",
                format!("/startDICOMImportJob/datastore/{datastore_id}"),
                Some(import_job_body("start-on-missing")),
            ),
        ];

        for (operation, method, uri, body) in probes {
            let answer = emulator.call(method, &uri, body).await;
            let case = format!("{operation} on {datastore_id}");
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected HTTP 404, got {answer:?}"
            );
            assert_eq!(
                answer.error_code, "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
            );
            let message = answer.string(&["message"]);
            assert_eq!(
                message, DATASTORE_NOT_FOUND,
                "case {case:?}: expected message {DATASTORE_NOT_FOUND:?}, got {message:?}"
            );
            assert!(
                !message.contains(datastore_id.as_str()),
                "case {case:?}: the requested id must not be echoed, got {message:?}"
            );
        }
    }

    // A data store that DID exist and whose deletion the clock has since made due is
    // the same refusal: the text does not depend on whether the id was ever real.
    let doomed = emulator.active_datastore("doomed.store", "doomed-token").await;
    let deleted = emulator.delete_datastore(&doomed).await;
    assert_eq!(
        deleted.status, 200,
        "case \"delete then read\": expected HTTP 200 from the delete, got {deleted:?}"
    );
    tokio::time::sleep(SETTLE_MULTIPLE * DATASTORE_DELETION_DELAY).await;
    let gone = emulator.get_datastore(&doomed).await;
    assert_eq!(
        gone.status, 404,
        "case \"delete then read\": expected HTTP 404, got {gone:?}"
    );
    let message = gone.string(&["message"]);
    assert_eq!(
        message, DATASTORE_NOT_FOUND,
        "case \"delete then read\": expected message {DATASTORE_NOT_FOUND:?}, got {message:?}"
    );
    assert!(
        !message.contains(doomed.as_str()),
        "case \"delete then read\": the requested id must not be echoed, got {message:?}"
    );
}

/// RULE, one resource kind down: an absent import job inside a data store that DOES
/// exist is refused with a text that is likewise constant — the same sentence for every
/// (data store, job) pair probed, carrying neither identifier.
#[tokio::test]
async fn a_missing_import_job_is_refused_with_a_constant_text() {
    let emulator = Emulator::new();
    let first = emulator.active_datastore("jobs.one", "jobs-one").await;
    let second = emulator.active_datastore("jobs.two", "jobs-two").await;

    // A real job in the first store, so that store's map is non-empty and the refusal
    // below is about the job that is missing, not about an empty collection.
    let submitted = emulator
        .start_import_job(&first, import_job_body("real-job"))
        .await;
    assert_eq!(
        submitted.status, 200,
        "case \"a real job\": expected HTTP 200, got {submitted:?}"
    );
    let real_job = submitted.string(&["jobId"]);

    // (case, data store, job id) — `JobId` is `^[0-9a-z]+$`, 1..32.
    let probes: Vec<(&str, &str, String)> = vec![
        ("short job id, store with a job", first.as_str(), "a".to_string()),
        (
            "32-character job id, store with a job",
            first.as_str(),
            "9".repeat(32),
        ),
        (
            "a real job id, but the other store",
            second.as_str(),
            real_job.clone(),
        ),
        ("store with no jobs at all", second.as_str(), "nosuchjob".to_string()),
    ];

    let mut messages: Vec<String> = Vec::new();
    for (case, datastore_id, job_id) in &probes {
        let answer = emulator.get_import_job(datastore_id, job_id).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        let message = answer.string(&["message"]);
        assert!(
            !message.contains(job_id.as_str()) && !message.contains(datastore_id),
            "case {case:?}: neither identifier may be echoed, got {message:?}"
        );
        messages.push(message);
    }

    let first_message = messages.first().cloned().unwrap_or_default();
    for (index, message) in messages.iter().enumerate() {
        assert_eq!(
            message, &first_message,
            "case {:?}: expected the same constant text as the first probe ({first_message:?}), \
             got {message:?}",
            probes[index].0
        );
    }

    // The job that DOES exist still reads back, so the refusal is about absence.
    let read = emulator.get_import_job(&first, &real_job).await;
    assert_eq!(
        read.status, 200,
        "case \"a real job\": expected the real job to read back, got {read:?}"
    );
}
