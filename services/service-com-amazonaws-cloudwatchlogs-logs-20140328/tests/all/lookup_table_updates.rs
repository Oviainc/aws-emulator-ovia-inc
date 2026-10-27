// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateLookupTable`, proved through the read it is observed with,
//! `GetLookupTable`.
//!
//! The rule under test: a lookup-table update identified by `lookupTableArn` REPLACES the
//! whole content of that table with the CSV body the request carries ("This is a full
//! replacement operation. All existing content is replaced"), re-derives from that body
//! everything the table reports about its content — the column names of its header row, the
//! data rows after it, and its size in bytes — stamps the table's last-updated instant with
//! the instant of the write, and leaves every member the request did NOT name as it was. The
//! operation carries no version or precondition member, so a write is never refused for being
//! based on a stale read: whatever a later call writes is what the table then holds. A
//! request naming both content sources or neither, a CSV body that cannot be read, a
//! `queryId` naming no completed or cancelled query, a `kmsKeyId` that is not a KMS key's, or
//! an ARN no table of this account is stored under writes nothing and is refused.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The table spans what the rule generalises over rather than one request: replacement bodies
//! differing in column count (fewer, the same and more columns than the body they replace),
//! in data-row count (none, one, many — growing and shrinking the table), in CSV quoting (a
//! field carrying the separator, a newline and an escaped quote), in line terminator (LF and
//! CRLF), in whether a blank line sits between the rows, in numeric text no re-rendering may
//! touch, and in encoding width (multibyte UTF-8, where the size in BYTES is not the
//! character count) — each crossed with the optional members absent (leave the stored one),
//! present (replace it), and, for the KMS key, the empty string the operation documents as
//! "remove the KMS key".

use serde_json::{json, Map, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    lookup_table_arn, mint_query_id, query_not_found_message,
    ComAmazonawsCloudwatchlogsLogs20140328State, QueryRecord, DEFAULT_QUERY_LANGUAGE,
    LOOKUP_TABLE_SOURCE_EXCLUSIVE, LOOKUP_TABLE_SOURCE_REQUIRED, MILLIS_PER_SECOND,
    QUERY_STATUS_COMPLETE,
};

use crate::harness::{error_code, now_ms, Logs, REGION};

/// The operations this suite drives: the create that stands a table up, the update under
/// test, and the read it is observed with.
const CREATE: &str = "CreateLookupTable";
const UPDATE: &str = "UpdateLookupTable";
const GET: &str = "GetLookupTable";

/// The code CloudWatch Logs answers for a parameter it will not accept ("A parameter is
/// specified incorrectly").
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The code answered for a request refused on a rule spanning two members, which no single
/// member's constraint can express.
const VALIDATION: &str = "ValidationException";

/// The code answered when the resource a request names is not there.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The code answered for a request this emulator's store cannot complete ("The service
/// cannot complete the request").
const SERVICE_UNAVAILABLE: &str = "ServiceUnavailableException";

/// The KMS key the table is created under: "The ARN of the KMS key to use to encrypt the
/// lookup table data."
const KMS_KEY: &str = "arn:aws:kms:eu-west-2:000000000000:key/aaaabbbb-cccc-dddd-eeee-ffff00001111";

/// Another KMS key, for the rows that REPLACE the key the table was created under — "You can
/// use this parameter to add, update, or remove the KMS key."
const OTHER_KMS_KEY: &str =
    "arn:aws:kms:eu-west-2:000000000000:key/99998888-7777-6666-5555-444433332222";

/// The description the table is created with, so a row that names none proves the stored one
/// is LEFT rather than cleared.
const CREATED_DESCRIPTION: &str = "the description the create gave";

/// The content the table is created with: two columns and two data rows, so a replacement can
/// be observed growing it, shrinking it and changing its columns.
const CREATED_BODY: &str = "user,team\nalice,platform\nbob,platform\n";

/// One accepted update: the CSV body it writes, the shape that body describes, the optional
/// members it names, and what the table must report for each of those members afterwards.
struct Update {
    /// What this row varies, for the assertion messages.
    case: &'static str,
    /// The name (and so the ARN) the row's own table is created under.
    name: &'static str,
    table_body: &'static str,
    /// The column names of the replacement body's header row, as CSV reads them.
    fields: &'static [&'static str],
    /// The replacement body's data rows, excluding its header row.
    records: i64,
    /// `description` as the request names it, or `None` when it names none.
    description: Option<&'static str>,
    /// `kmsKeyId` as the request names it, or `None` when it names none; `Some("")` is the
    /// documented "remove the KMS key".
    kms_key_id: Option<&'static str>,
    /// What the table must report for `description` after the update.
    reported_description: Option<&'static str>,
    /// What the table must report for `kmsKeyId` after the update.
    reported_kms_key_id: Option<&'static str>,
}

impl Update {
    /// A row naming nothing but the replacement body, so both optional members must survive
    /// the update exactly as the create left them.
    const fn body_only(
        case: &'static str,
        name: &'static str,
        table_body: &'static str,
        fields: &'static [&'static str],
        records: i64,
    ) -> Self {
        Self {
            case,
            name,
            table_body,
            fields,
            records,
            description: None,
            kms_key_id: None,
            reported_description: Some(CREATED_DESCRIPTION),
            reported_kms_key_id: Some(KMS_KEY),
        }
    }
}

/// The accepted corpus. Each row's replacement body differs along what the rule generalises
/// over, and the last rows carry the optional members.
fn updates() -> Vec<Update> {
    vec![
        Update::body_only(
            "one column and no data rows, shrinking the table",
            "update_one_column",
            "user\n",
            &["user"],
            0,
        ),
        Update::body_only(
            "the same columns with one data row",
            "update_one_row",
            "user,team\ncarol,data\n",
            &["user", "team"],
            1,
        ),
        Update::body_only(
            "more columns and more rows, growing the table",
            "update_grown",
            "user,team,role\nalice,platform,admin\nbob,platform,dev\ncarol,data,dev\n",
            &["user", "team", "role"],
            3,
        ),
        Update::body_only(
            "a quoted field carrying the separator",
            "update_quoted_separator",
            "user,note\nalice,\"platform, core\"\n",
            &["user", "note"],
            1,
        ),
        Update::body_only(
            "a quoted field carrying a newline, which is one field and one record",
            "update_quoted_newline",
            "user,note\nalice,\"first\nsecond\"\n",
            &["user", "note"],
            1,
        ),
        Update::body_only(
            "a quoted field carrying an escaped quote",
            "update_escaped_quote",
            "user,note\nalice,\"she said \"\"hi\"\"\"\n",
            &["user", "note"],
            1,
        ),
        Update::body_only(
            "CRLF line terminators",
            "update_crlf",
            "user,team\r\nalice,platform\r\ndan,data\r\n",
            &["user", "team"],
            2,
        ),
        Update::body_only(
            "a blank line between the rows, which CSV skips rather than counting",
            "update_blank_line",
            "user\nalice\n\nbob\n",
            &["user"],
            2,
        ),
        Update::body_only(
            "numeric text whose precision no re-rendering may touch",
            "update_numeric_text",
            "user,score\nalice,0007.50000\nbob,9007199254740993\ncarol,1e3\n",
            &["user", "score"],
            3,
        ),
        Update::body_only(
            "multibyte UTF-8, where the size in BYTES is not the character count",
            "update_multibyte",
            "användare,lag\nål,plattform\n",
            &["användare", "lag"],
            1,
        ),
        Update {
            case: "a new description, which replaces the stored one",
            name: "update_described",
            table_body: "user\nalice\n",
            fields: &["user"],
            records: 1,
            description: Some("the description the update gave"),
            kms_key_id: None,
            reported_description: Some("the description the update gave"),
            reported_kms_key_id: Some(KMS_KEY),
        },
        Update {
            case: "an empty description, which the modelled member admits (len 0..1024)",
            name: "update_empty_description",
            table_body: "user\nalice\n",
            fields: &["user"],
            records: 1,
            description: Some(""),
            kms_key_id: None,
            reported_description: Some(""),
            reported_kms_key_id: Some(KMS_KEY),
        },
        Update {
            case: "another KMS key, which replaces the stored one",
            name: "update_rekeyed",
            table_body: "user\nalice\n",
            fields: &["user"],
            records: 1,
            description: None,
            kms_key_id: Some(OTHER_KMS_KEY),
            reported_description: Some(CREATED_DESCRIPTION),
            reported_kms_key_id: Some(OTHER_KMS_KEY),
        },
        Update {
            case: "the empty string, which removes the KMS key",
            name: "update_dekeyed",
            table_body: "user\nalice\n",
            fields: &["user"],
            records: 1,
            description: None,
            kms_key_id: Some(""),
            reported_description: Some(CREATED_DESCRIPTION),
            reported_kms_key_id: None,
        },
        Update {
            case: "a bare KMS key id, which KMS resolves rather than this service",
            name: "update_bare_key_id",
            table_body: "user\nalice\n",
            fields: &["user"],
            records: 1,
            description: Some("both optional members at once"),
            kms_key_id: Some("aaaabbbb-cccc-dddd-eeee-ffff00001111"),
            reported_description: Some("both optional members at once"),
            reported_kms_key_id: Some("aaaabbbb-cccc-dddd-eeee-ffff00001111"),
        },
    ]
}

/// The request body one row sends.
fn update_request(arn: &str, row: &Update) -> Value {
    let mut request = Map::new();
    request.insert("lookupTableArn".to_string(), json!(arn));
    request.insert("tableBody".to_string(), json!(row.table_body));
    if let Some(description) = row.description {
        request.insert("description".to_string(), json!(description));
    }
    if let Some(kms_key_id) = row.kms_key_id {
        request.insert("kmsKeyId".to_string(), json!(kms_key_id));
    }
    Value::Object(request)
}

/// One string member of a response, or `None` when the response omits it.
fn member(body: &Value, key: &str) -> Option<String> {
    body.get(key).and_then(Value::as_str).map(str::to_string)
}

/// One integer member of a response, or `None` when the response omits it.
fn number(body: &Value, key: &str) -> Option<i64> {
    body.get(key).and_then(Value::as_i64)
}

/// The table `name` holds, created with [`CREATED_BODY`], a description and a KMS key, with
/// the ARN it answers and the instant it was created.
async fn create_table(logs: &Logs, name: &str) -> (String, i64) {
    let created = logs
        .ok(
            CREATE,
            &json!({
                "lookupTableName": name,
                "tableBody": CREATED_BODY,
                "description": CREATED_DESCRIPTION,
                "kmsKeyId": KMS_KEY,
            }),
        )
        .await;
    let arn = member(&created, "lookupTableArn")
        .unwrap_or_else(|| panic!("case {name:?}: a create answers lookupTableArn"));
    let created_at = number(&created, "createdAt")
        .unwrap_or_else(|| panic!("case {name:?}: a create answers createdAt"));
    (arn, created_at)
}

/// Every accepted row: the update replaces the table's whole content, re-derives from the new
/// body everything the table reports about its content, stamps the write's instant, and moves
/// an optional member only when the request named it.
#[tokio::test]
async fn an_updated_lookup_table_is_reported_back_exactly() {
    let logs = Logs::new();

    for row in updates() {
        let (arn, created_at) = create_table(&logs, row.name).await;

        let before = now_ms();
        let updated = logs.ok(UPDATE, &update_request(&arn, &row)).await;
        let after = now_ms();

        assert_eq!(
            member(&updated, "lookupTableArn").as_deref(),
            Some(arn.as_str()),
            "case {:?}: the update answers the ARN of the table it wrote, got {updated}",
            row.case
        );
        let last_updated = number(&updated, "lastUpdatedTime")
            .unwrap_or_else(|| panic!("case {:?}: an update answers lastUpdatedTime", row.case));
        assert!(
            (before..=after).contains(&last_updated),
            "case {:?}: lastUpdatedTime is the instant of the call, expected it in \
             {before}..={after}, got {last_updated}",
            row.case
        );
        assert!(
            last_updated >= created_at,
            "case {:?}: the write is not before the create ({created_at}), got {last_updated}",
            row.case
        );

        let read = logs.ok(GET, &json!({"lookupTableArn": arn})).await;
        let body = member(&read, "tableBody")
            .unwrap_or_else(|| panic!("case {:?}: the read reports tableBody", row.case));
        assert_eq!(
            body, row.table_body,
            "case {:?}: the replacement body round-trips verbatim, expected {:?}, got {body:?}",
            row.case, row.table_body
        );
        let expected_size = i64::try_from(row.table_body.len()).expect("the body fits in an i64");
        assert_eq!(
            number(&read, "sizeBytes"),
            Some(expected_size),
            "case {:?}: sizeBytes is the NEW body's length in BYTES, expected {expected_size}, \
             got {read}",
            row.case
        );
        assert_eq!(
            number(&read, "lastUpdatedTime"),
            Some(last_updated),
            "case {:?}: the read reports the instant the update answered ({last_updated}), got \
             {read}",
            row.case
        );
        assert_eq!(
            member(&read, "lookupTableName").as_deref(),
            Some(row.name),
            "case {:?}: an update does not rename the table, got {read}",
            row.case
        );
        assert_eq!(
            member(&read, "lookupTableArn").as_deref(),
            Some(arn.as_str()),
            "case {:?}: an update does not move the table's ARN, got {read}",
            row.case
        );
        assert_eq!(
            member(&read, "description").as_deref(),
            row.reported_description,
            "case {:?}: the description moves only when the update named one, expected {:?}, \
             got {read}",
            row.case,
            row.reported_description
        );
        assert_eq!(
            member(&read, "kmsKeyId").as_deref(),
            row.reported_kms_key_id,
            "case {:?}: the KMS key moves only when the update named one, expected {:?}, got \
             {read}",
            row.case,
            row.reported_kms_key_id
        );

        // `tableFields` and `recordsCount` are members of the model's `LookupTable` shape,
        // which `GetLookupTable` has no members for, so the shape the update derived from the
        // replacement body is read off the record it stored.
        let state = logs
            .engine()
            .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
        let store = state.lock();
        let stored = store
            .lookup_tables
            .get(row.name)
            .unwrap_or_else(|| panic!("case {:?}: the table is still stored", row.case));
        assert_eq!(
            stored.table_fields, row.fields,
            "case {:?}: the NEW header row names the table's columns, expected {:?}, got {:?}",
            row.case, row.fields, stored.table_fields
        );
        assert_eq!(
            stored.records_count, row.records,
            "case {:?}: the NEW data rows are the table's records, expected {}, got {}",
            row.case, row.records, stored.records_count
        );
        assert_eq!(
            stored.created_at_ms, created_at,
            "case {:?}: an update does not move the instant the table was created, expected \
             {created_at}, got {}",
            row.case, stored.created_at_ms
        );
    }
}

/// `UpdateLookupTable` carries no version, revision or precondition member, so no write is
/// refused for being based on a stale read: each accepted write replaces the content the
/// previous one left, and the LAST one is what the table holds — including a write that puts
/// back content an earlier call had already replaced.
#[tokio::test]
async fn the_last_write_is_what_the_table_holds() {
    let logs = Logs::new();
    const NAME: &str = "update_last_writer";
    let (arn, _created_at) = create_table(&logs, NAME).await;

    // The third write carries the content of the first, which the second had already
    // replaced: a caller writing from a stale read is accepted and wins.
    let writes = [
        ("the first write", "user\nalice\n", 1_i64),
        ("the second write", "user,team\nbob,data\ncarol,data\n", 2),
        ("a write of the content the second replaced", "user\nalice\n", 1),
    ];

    let mut previous_last_updated = 0_i64;
    for (case, body, records) in writes {
        let updated = logs
            .ok(UPDATE, &json!({"lookupTableArn": arn, "tableBody": body}))
            .await;
        let last_updated = number(&updated, "lastUpdatedTime")
            .unwrap_or_else(|| panic!("case {case:?}: an update answers lastUpdatedTime"));
        assert!(
            last_updated >= previous_last_updated,
            "case {case:?}: the last-updated instant never goes backwards, expected at least \
             {previous_last_updated}, got {last_updated}"
        );
        previous_last_updated = last_updated;

        let read = logs.ok(GET, &json!({"lookupTableArn": arn})).await;
        assert_eq!(
            member(&read, "tableBody").as_deref(),
            Some(body),
            "case {case:?}: the table holds what the latest write gave it, got {read}"
        );

        let state = logs
            .engine()
            .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
        let store = state.lock();
        let stored = store
            .lookup_tables
            .get(NAME)
            .unwrap_or_else(|| panic!("case {case:?}: the table is still stored"));
        assert_eq!(
            stored.records_count, records,
            "case {case:?}: the record count follows the latest body, expected {records}, got {}",
            stored.records_count
        );
        assert_eq!(
            store.lookup_tables.len(),
            1,
            "case {case:?}: an update replaces the table's content and creates no second \
             table, got {:?}",
            store.lookup_tables.keys().collect::<Vec<_>>()
        );
    }
}

/// One refused update: the request it sends (its `lookupTableArn` filled in from the stored
/// table when `own_arn` is set), the code it must answer and the text its message must carry.
struct Refusal {
    case: &'static str,
    /// Whether the request addresses the table this suite created; `false` rows carry their
    /// own `lookupTableArn`.
    own_arn: bool,
    request: Value,
    code: &'static str,
    in_message: String,
}

/// Every update this operation refuses, and what it refuses it with.
fn refusals() -> Vec<Refusal> {
    let unknown_query = mint_query_id(404);
    vec![
        Refusal {
            case: "both content sources",
            own_arn: true,
            request: json!({"tableBody": "user\nalice\n", "queryId": unknown_query.clone()}),
            code: VALIDATION,
            in_message: LOOKUP_TABLE_SOURCE_EXCLUSIVE.to_string(),
        },
        Refusal {
            case: "neither content source, so there is nothing to replace the content with",
            own_arn: true,
            request: json!({}),
            code: VALIDATION,
            in_message: LOOKUP_TABLE_SOURCE_REQUIRED.to_string(),
        },
        Refusal {
            case: "neither content source, with only a description to change",
            own_arn: true,
            request: json!({"description": "a description on its own"}),
            code: VALIDATION,
            in_message: LOOKUP_TABLE_SOURCE_REQUIRED.to_string(),
        },
        Refusal {
            case: "a queryId naming no query of this account",
            own_arn: true,
            request: json!({"queryId": unknown_query.clone()}),
            code: INVALID_PARAMETER,
            in_message: query_not_found_message(&unknown_query),
        },
        Refusal {
            case: "a data row with fewer fields than the header row",
            own_arn: true,
            request: json!({"tableBody": "user,team\nalice\n"}),
            code: INVALID_PARAMETER,
            in_message: "tableBody".to_string(),
        },
        Refusal {
            case: "a data row with more fields than the header row",
            own_arn: true,
            request: json!({"tableBody": "user,team\nalice,platform,admin\n"}),
            code: INVALID_PARAMETER,
            in_message: "tableBody".to_string(),
        },
        Refusal {
            case: "a header row naming no column at all",
            own_arn: true,
            request: json!({"tableBody": ","}),
            code: INVALID_PARAMETER,
            in_message: "header row".to_string(),
        },
        Refusal {
            case: "a body that is one blank line, so there is no header row",
            own_arn: true,
            request: json!({"tableBody": "\n"}),
            code: INVALID_PARAMETER,
            in_message: "header row".to_string(),
        },
        Refusal {
            case: "a kmsKeyId naming another service's resource",
            own_arn: true,
            request: json!({"tableBody": "user\nalice\n", "kmsKeyId": "arn:aws:s3:::a-bucket"}),
            code: INVALID_PARAMETER,
            in_message: "kmsKeyId".to_string(),
        },
        Refusal {
            case: "a lookupTableArn that is not an ARN at all",
            own_arn: false,
            request: json!({"lookupTableArn": "update_refused", "tableBody": "user\nalice\n"}),
            code: INVALID_PARAMETER,
            in_message: "Unable to parse lookupTableArn".to_string(),
        },
        Refusal {
            case: "a lookup-table ARN for a name nothing created",
            own_arn: false,
            request: json!({
                "lookupTableArn": lookup_table_arn(REGION, "update_never_created"),
                "tableBody": "user\nalice\n",
            }),
            code: NOT_FOUND,
            in_message: "Lookup table 'update_never_created' not found in account".to_string(),
        },
        Refusal {
            case: "this table's name in ANOTHER Region, which this account does not hold here",
            own_arn: false,
            request: json!({
                "lookupTableArn": lookup_table_arn("us-east-1", "update_refusals"),
                "tableBody": "user\nalice\n",
            }),
            code: NOT_FOUND,
            in_message: "Lookup table 'update_refusals' not found in account".to_string(),
        },
        Refusal {
            case: "a log group's ARN, which names no lookup table",
            own_arn: false,
            request: json!({
                "lookupTableArn": format!(
                    "arn:aws:logs:{REGION}:000000000000:log-group:update_refusals"
                ),
                "tableBody": "user\nalice\n",
            }),
            code: NOT_FOUND,
            in_message: "does not exist".to_string(),
        },
    ]
}

/// Every refused update answers its own code with a message carrying the rule, and leaves the
/// stored table exactly as it was.
#[tokio::test]
async fn a_refused_update_writes_nothing() {
    let logs = Logs::new();
    const NAME: &str = "update_refusals";
    let (arn, created_at) = create_table(&logs, NAME).await;

    for refusal in refusals() {
        let request = {
            let mut request = refusal
                .request
                .as_object()
                .expect("a refusal row's request is an object")
                .clone();
            if refusal.own_arn {
                request.insert("lookupTableArn".to_string(), json!(arn));
            }
            Value::Object(request)
        };

        let (code, message) = logs.refusal(UPDATE, &request).await;
        assert_eq!(
            code, refusal.code,
            "case {:?}: expected {}, got {code} {message:?}",
            refusal.case, refusal.code
        );
        assert!(
            message.contains(&refusal.in_message),
            "case {:?}: the refusal states {:?}, got {message:?}",
            refusal.case,
            refusal.in_message
        );

        let read = logs.ok(GET, &json!({"lookupTableArn": arn})).await;
        assert_eq!(
            member(&read, "tableBody").as_deref(),
            Some(CREATED_BODY),
            "case {:?}: a refused update replaces no content, got {read}",
            refusal.case
        );
        assert_eq!(
            member(&read, "description").as_deref(),
            Some(CREATED_DESCRIPTION),
            "case {:?}: a refused update moves no description, got {read}",
            refusal.case
        );
        assert_eq!(
            member(&read, "kmsKeyId").as_deref(),
            Some(KMS_KEY),
            "case {:?}: a refused update moves no KMS key, got {read}",
            refusal.case
        );
        assert_eq!(
            number(&read, "lastUpdatedTime"),
            Some(created_at),
            "case {:?}: a refused update does not stamp the table, expected {created_at}, got \
             {read}",
            refusal.case
        );
    }
}

/// A `queryId` naming a query that has not finished and was not stopped replaces nothing:
/// the member is "The ID of a completed or cancelled CloudWatch Logs query whose results
/// replace the lookup table content", so a query still scheduled or running names no results
/// at all and the request is refused with the id it named.
#[tokio::test]
async fn an_unfinished_query_replaces_nothing() {
    let logs = Logs::new();
    const GROUP: &str = "update_query_group";
    const NAME: &str = "update_unfinished_query";
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;
    let (arn, _created_at) = create_table(&logs, NAME).await;

    // A window around the present, in the epoch SECONDS `StartQuery` takes: a query whose
    // window ends before the log group it reads was created is refused for the window, which
    // is not what this case is about.
    let now_secs = now_ms() / MILLIS_PER_SECOND;
    let started = logs
        .ok(
            "StartQuery",
            &json!({
                "logGroupName": GROUP,
                "queryString": "fields @timestamp, @message",
                "startTime": now_secs - 3_600,
                "endTime": now_secs + 60,
            }),
        )
        .await;
    let query_id =
        member(&started, "queryId").expect("a started query answers the id it is read by");

    let (code, message) = logs
        .refusal(UPDATE, &json!({"lookupTableArn": arn, "queryId": query_id}))
        .await;
    assert_eq!(
        code, INVALID_PARAMETER,
        "case \"a query that has not finished\": expected {INVALID_PARAMETER}, got {code} \
         {message:?}"
    );
    let expected = query_not_found_message(&query_id);
    assert_eq!(
        message, expected,
        "case \"a query that has not finished\": expected {expected:?}, got {message:?}"
    );

    let read = logs.ok(GET, &json!({"lookupTableArn": arn})).await;
    assert_eq!(
        member(&read, "tableBody").as_deref(),
        Some(CREATED_BODY),
        "case \"a query that has not finished\": the table's content is untouched, got {read}"
    );
}

/// A `queryId` naming a query that HAS finished names a result set this emulator does not
/// hold: nothing here runs a query over the stored events, so there are no rows to replace
/// the table's content with. The request is refused through the error the operation models
/// for a request the service cannot complete — never with rows nothing computed, and never by
/// emptying the table on the caller's behalf.
#[tokio::test]
async fn a_finished_query_has_no_results_to_write() {
    let logs = Logs::new();
    const NAME: &str = "update_finished_query";
    let (arn, created_at) = create_table(&logs, NAME).await;

    // A query the store holds in a terminal status, through the same record constructor
    // `StartQuery` uses: the rule is about the STATUS the store holds it in, so waiting out
    // the clock that would move it there is not what is under test here.
    let query_id = mint_query_id(7);
    {
        let state = logs
            .engine()
            .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
        let mut store = state.lock();
        let now = now_ms();
        let now_secs = now / MILLIS_PER_SECOND;
        let mut record = QueryRecord::new(
            &query_id,
            DEFAULT_QUERY_LANGUAGE,
            "fields @timestamp, @message",
            Vec::new(),
            now_secs - 3_600,
            now_secs,
            now,
        );
        record.status = QUERY_STATUS_COMPLETE.to_string();
        record.running_due_ms = None;
        record.complete_due_ms = None;
        store.queries.insert(query_id.clone(), record);
    }

    let (status, body) = logs
        .call(UPDATE, &json!({"lookupTableArn": arn, "queryId": query_id}))
        .await;
    assert_eq!(
        status, 500,
        "case \"a finished query whose results this emulator does not hold\": expected HTTP \
         500, got {status} {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, SERVICE_UNAVAILABLE,
        "case \"a finished query whose results this emulator does not hold\": expected \
         {SERVICE_UNAVAILABLE}, got {code} {body}"
    );

    let read = logs.ok(GET, &json!({"lookupTableArn": arn})).await;
    assert_eq!(
        member(&read, "tableBody").as_deref(),
        Some(CREATED_BODY),
        "case \"a finished query whose results this emulator does not hold\": the table's \
         content is untouched, got {read}"
    );
    assert_eq!(
        number(&read, "lastUpdatedTime"),
        Some(created_at),
        "case \"a finished query whose results this emulator does not hold\": the table is \
         not stamped, expected {created_at}, got {read}"
    );
}
