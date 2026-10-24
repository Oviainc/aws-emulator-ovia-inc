// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `CreateLookupTable` and the read it is proved through,
//! `GetLookupTable`.
//!
//! The rule under test: `CreateLookupTable` persists ONE lookup table under the name that
//! identifies it ("The name must be unique within your account and Region"), reads the CSV
//! body it was handed into the column names of its header row and the data rows after it,
//! keeps that body VERBATIM, and hands back the ARN that identifies the table plus the
//! instant it was created; `GetLookupTable` reports, under that ARN, exactly what the create
//! stored — the body byte-for-byte, its size in bytes, and each optional member only when
//! the create named it. A request naming both content sources or neither, a CSV body that
//! cannot be read, a `queryId` naming no query, a `kmsKeyId` that is not a KMS key's, a name
//! the account already holds, or a create past the account's table ceiling creates nothing
//! and is refused; so is a read of an ARN no table is stored under.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The table spans what the rule generalises over rather than one request: bodies differing
//! in column count (one and several), in data-row count (none, one, many), in CSV quoting (a
//! field carrying the separator, a newline, an escaped quote), in numeric text whose
//! precision no re-rendering may touch (a long decimal, leading zeros, an integer past the
//! double-precision range, an exponent), in line terminator (LF and CRLF), in whether a
//! blank line sits between the rows (CSV skips one rather than counting it as a record), and
//! in encoding width (multibyte UTF-8, where the size in BYTES is not the character count),
//! each with
//! the optional members absent, present on their own and all together. The account's table
//! ceiling, its retained-byte budget (at the bound and one byte past it) and each refusal
//! have their own cases.

use serde_json::{json, Map, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    log_group_arn, lookup_table_arn, parse_table_body, query_not_found_message,
    ComAmazonawsCloudwatchlogsLogs20140328State, LookupTableRecord, TableBodyShape,
    LOOKUP_TABLE_SOURCE_EXCLUSIVE, LOOKUP_TABLE_SOURCE_REQUIRED, MAX_LOOKUP_TABLES,
    MAX_LOOKUP_TABLE_BYTES_TOTAL,
};

use crate::harness::{now_ms, Logs, REGION};

/// The operation that creates a lookup table, and the one that reports it back.
const CREATE: &str = "CreateLookupTable";
const GET: &str = "GetLookupTable";

/// The code CloudWatch Logs answers for a parameter it will not accept, which is what
/// `CreateLookupTable` and `GetLookupTable` both declare for one ("A parameter is specified
/// incorrectly").
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The code answered for a request refused on a rule spanning two members, which no single
/// member's constraint can express.
const VALIDATION: &str = "ValidationException";

/// The code answered when the resource a request names is not there.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The code answered when a create names a table name the account already holds.
const ALREADY_EXISTS: &str = "ResourceAlreadyExistsException";

/// The code answered when a create would grow the account's store past its ceiling.
const LIMIT_EXCEEDED: &str = "LimitExceededException";

/// A KMS key ARN, for the rows that assign one: "The ARN of the KMS key to use to encrypt
/// the lookup table data."
const KMS_KEY: &str = "arn:aws:kms:eu-west-2:000000000000:key/aaaabbbb-cccc-dddd-eeee-ffff00001111";

/// One accepted create: the CSV body it uploads, the shape that body describes, and the
/// optional members it names — each of which the read must report back exactly when the
/// create named it.
struct Row {
    /// What this row varies, for the assertion messages.
    case: &'static str,
    name: &'static str,
    table_body: &'static str,
    /// The column names of the body's header row, as CSV reads them.
    fields: &'static [&'static str],
    /// The body's data rows, excluding its header row.
    records: i64,
    description: Option<&'static str>,
    kms_key_id: Option<&'static str>,
    tags: Option<&'static [(&'static str, &'static str)]>,
}

impl Row {
    /// A row naming nothing but the required name and the CSV body.
    const fn bare(
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
            tags: None,
        }
    }
}

/// The accepted corpus. Each row's body differs along what the rule generalises over, and
/// the last rows carry the optional members.
fn rows() -> Vec<Row> {
    vec![
        Row::bare(
            "one column and no data rows",
            "users_header_only",
            "user",
            &["user"],
            0,
        ),
        Row::bare(
            "several columns and one data row",
            "users_one_row",
            "user,team,role\nalice,platform,admin\n",
            &["user", "team", "role"],
            1,
        ),
        Row::bare(
            "many data rows",
            "users_many_rows",
            "user,team\nalice,platform\nbob,payments\ncarol,search\ndave,platform\neve,search\n",
            &["user", "team"],
            5,
        ),
        Row::bare(
            "a quoted field carrying the separator",
            "codes_quoted_separator",
            "code,label\n404,\"not found, really\"\n",
            &["code", "label"],
            1,
        ),
        Row::bare(
            "a quoted field carrying a newline, which is one record and not two",
            "codes_quoted_newline",
            "code,label\n500,\"line one\nline two\"\n",
            &["code", "label"],
            1,
        ),
        Row::bare(
            "a quoted field carrying an escaped quote",
            "codes_escaped_quote",
            "code,label\n200,\"the \"\"ok\"\" code\"\n",
            &["code", "label"],
            1,
        ),
        Row::bare(
            "numeric text whose precision no re-rendering may touch",
            "metrics_precision",
            "metric,value\nlatency,0.100000000000000005551\ncount,000123\n\
             big,9007199254740993\nexp,1e-7\nnegzero,-0.0\n",
            &["metric", "value"],
            5,
        ),
        Row::bare(
            "an interior blank line, which CSV skips rather than counting as a record",
            "codes_blank_line",
            "code,label\n404,missing\n\n500,oops\n",
            &["code", "label"],
            2,
        ),
        Row::bare(
            "CRLF line terminators",
            "codes_crlf",
            "code,label\r\n404,missing\r\n",
            &["code", "label"],
            1,
        ),
        Row::bare(
            "multibyte UTF-8 content, where the size in bytes is not the character count",
            "users_unicode",
            "名前,説明\nログ,テスト\n",
            &["名前", "説明"],
            1,
        ),
        Row {
            description: Some("a description of the lookup table"),
            ..Row::bare(
                "a description",
                "users_described",
                "user\nalice\n",
                &["user"],
                1,
            )
        },
        Row {
            kms_key_id: Some(KMS_KEY),
            ..Row::bare(
                "a customer KMS key",
                "users_encrypted",
                "user\nalice\n",
                &["user"],
                1,
            )
        },
        Row {
            tags: Some(&[("team", "platform"), ("env", "test")]),
            ..Row::bare("tags", "users_tagged", "user\nalice\n", &["user"], 1)
        },
        Row {
            description: Some("every optional member at once"),
            kms_key_id: Some(KMS_KEY),
            tags: Some(&[("team", "platform")]),
            ..Row::bare(
                "every optional member at once",
                "users_everything",
                "user,team\nalice,platform\n",
                &["user", "team"],
                1,
            )
        },
    ]
}

/// The request body one row sends.
fn create_request(row: &Row) -> Value {
    let mut request = Map::new();
    request.insert("lookupTableName".to_string(), json!(row.name));
    request.insert("tableBody".to_string(), json!(row.table_body));
    if let Some(description) = row.description {
        request.insert("description".to_string(), json!(description));
    }
    if let Some(kms_key_id) = row.kms_key_id {
        request.insert("kmsKeyId".to_string(), json!(kms_key_id));
    }
    if let Some(tags) = row.tags {
        let mut map = Map::new();
        for (key, value) in tags {
            map.insert((*key).to_string(), json!(value));
        }
        request.insert("tags".to_string(), Value::Object(map));
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

/// Every accepted row: the create persists the table and answers the ARN that identifies
/// it, and the read reports back the body verbatim with the metadata derived from it.
#[tokio::test]
async fn a_created_lookup_table_is_reported_back_exactly() {
    let logs = Logs::new();

    for row in rows() {
        let before = now_ms();
        let created = logs.ok(CREATE, &create_request(&row)).await;
        let after = now_ms();

        let expected_arn = lookup_table_arn(REGION, row.name);
        let arn = member(&created, "lookupTableArn")
            .unwrap_or_else(|| panic!("case {:?}: a create answers lookupTableArn", row.case));
        assert_eq!(
            arn, expected_arn,
            "case {:?}: the create answers the ARN built for the name it was given, expected \
             {expected_arn}, got {arn}",
            row.case
        );
        let created_at = number(&created, "createdAt")
            .unwrap_or_else(|| panic!("case {:?}: a create answers createdAt", row.case));
        assert!(
            (before..=after).contains(&created_at),
            "case {:?}: createdAt is the instant of the call, expected it in \
             {before}..={after}, got {created_at}",
            row.case
        );

        let read = logs.ok(GET, &json!({"lookupTableArn": arn})).await;
        let body = member(&read, "tableBody")
            .unwrap_or_else(|| panic!("case {:?}: the read reports tableBody", row.case));
        assert_eq!(
            body, row.table_body,
            "case {:?}: the body round-trips verbatim, expected {:?}, got {body:?}",
            row.case, row.table_body
        );
        assert_eq!(
            member(&read, "lookupTableName").as_deref(),
            Some(row.name),
            "case {:?}: the read reports the name the create was given, got {read}",
            row.case
        );
        assert_eq!(
            member(&read, "lookupTableArn").as_deref(),
            Some(arn.as_str()),
            "case {:?}: the read reports the ARN it was read under, got {read}",
            row.case
        );
        assert_eq!(
            member(&read, "description").as_deref(),
            row.description,
            "case {:?}: the read reports the description only when the create named one, got \
             {read}",
            row.case
        );
        assert_eq!(
            member(&read, "kmsKeyId").as_deref(),
            row.kms_key_id,
            "case {:?}: the read reports the KMS key only when the create named one, got {read}",
            row.case
        );
        let expected_size = i64::try_from(row.table_body.len()).expect("the body fits in an i64");
        assert_eq!(
            number(&read, "sizeBytes"),
            Some(expected_size),
            "case {:?}: sizeBytes is the body's length in BYTES, expected {expected_size}, got \
             {read}",
            row.case
        );
        assert_eq!(
            number(&read, "lastUpdatedTime"),
            Some(created_at),
            "case {:?}: a table nothing has updated was last updated when it was created \
             ({created_at}), got {read}",
            row.case
        );

        // `tableFields` and `recordsCount` are members of the model's `LookupTable` shape,
        // which only `DescribeLookupTables` reports — still a scaffold stub — so the shape
        // the create derived from the body is read off the record it stored.
        let state = logs
            .engine()
            .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
        let store = state.lock();
        let stored = store
            .lookup_tables
            .get(row.name)
            .unwrap_or_else(|| panic!("case {:?}: the create persists the table", row.case));
        assert_eq!(
            stored.table_fields, row.fields,
            "case {:?}: the header row names the table's columns, expected {:?}, got {:?}",
            row.case, row.fields, stored.table_fields
        );
        assert_eq!(
            stored.records_count, row.records,
            "case {:?}: the data rows after the header are the table's records, expected {}, \
             got {}",
            row.case, row.records, stored.records_count
        );
        let expected_tags: Vec<(String, String)> = row
            .tags
            .unwrap_or(&[])
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        let mut got_tags: Vec<(String, String)> = stored
            .tags
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        got_tags.sort();
        let mut expected_tags = expected_tags;
        expected_tags.sort();
        assert_eq!(
            got_tags, expected_tags,
            "case {:?}: the create stores the tags it was given, expected {expected_tags:?}, \
             got {got_tags:?}",
            row.case
        );
    }
}

/// The ARN segments are pinned LITERALLY here, once, so the assertions above — which build
/// the expected ARN with the same helper the handler uses — cannot pass on an ARN of the
/// wrong SHAPE. The form is the one every named CloudWatch Logs resource of this service
/// carries: `arn:aws:logs:<region>:<account>:<resource-type>:<name>`, with the region taken
/// from the request's credential scope and the account from the emulator's own id.
#[tokio::test]
async fn the_created_arn_carries_the_documented_segments() {
    let logs = Logs::new();
    const NAME: &str = "users_arn_form";

    let created = logs
        .ok(
            CREATE,
            &json!({"lookupTableName": NAME, "tableBody": "user\nalice\n"}),
        )
        .await;
    let arn = member(&created, "lookupTableArn").expect("a create answers lookupTableArn");
    let expected = format!(
        "arn:aws:logs:{REGION}:{}:lookup-table:{NAME}",
        engine::emulator_account_id()
    );
    assert_eq!(
        arn, expected,
        "case \"the ARN's segments\": expected {expected}, got {arn}"
    );
}

/// One refused create: the request, the code it must answer, and the text its message must
/// carry so the caller can fix the request from it.
struct Refusal {
    case: &'static str,
    request: Value,
    code: &'static str,
    in_message: String,
}

/// Every create this operation refuses, and what it refuses it with.
fn refusals() -> Vec<Refusal> {
    vec![
        Refusal {
            case: "both content sources",
            request: json!({
                "lookupTableName": "refused",
                "tableBody": "user\nalice\n",
                "queryId": "11112222-3333-4444-5555-666677778888",
            }),
            code: VALIDATION,
            in_message: LOOKUP_TABLE_SOURCE_EXCLUSIVE.to_string(),
        },
        Refusal {
            case: "neither content source",
            request: json!({"lookupTableName": "refused"}),
            code: VALIDATION,
            in_message: LOOKUP_TABLE_SOURCE_REQUIRED.to_string(),
        },
        Refusal {
            case: "a queryId naming no completed or cancelled query",
            request: json!({
                "lookupTableName": "refused",
                "queryId": "11112222-3333-4444-5555-666677778888",
            }),
            code: INVALID_PARAMETER,
            in_message: query_not_found_message("11112222-3333-4444-5555-666677778888"),
        },
        Refusal {
            case: "a data row with fewer fields than the header row",
            request: json!({"lookupTableName": "refused", "tableBody": "user,team\nalice\n"}),
            code: INVALID_PARAMETER,
            in_message: "tableBody".to_string(),
        },
        Refusal {
            case: "a data row with more fields than the header row",
            request: json!({
                "lookupTableName": "refused",
                "tableBody": "user,team\nalice,platform,admin\n",
            }),
            code: INVALID_PARAMETER,
            in_message: "tableBody".to_string(),
        },
        Refusal {
            case: "a header row naming no column at all",
            request: json!({"lookupTableName": "refused", "tableBody": ","}),
            code: INVALID_PARAMETER,
            in_message: "header row".to_string(),
        },
        Refusal {
            case: "a body that is one blank line, so there is no header row",
            request: json!({"lookupTableName": "refused", "tableBody": "\n"}),
            code: INVALID_PARAMETER,
            in_message: "header row".to_string(),
        },
        Refusal {
            case: "a kmsKeyId that is not a KMS key's ARN",
            request: json!({
                "lookupTableName": "refused",
                "tableBody": "user\nalice\n",
                "kmsKeyId": "arn:aws:s3:::a-bucket",
            }),
            code: INVALID_PARAMETER,
            in_message: "kmsKeyId".to_string(),
        },
    ]
}

/// Every refused create answers its own code with a message carrying the rule, and persists
/// nothing.
#[tokio::test]
async fn a_refused_create_persists_nothing() {
    let logs = Logs::new();

    for refusal in refusals() {
        let (code, message) = logs.refusal(CREATE, &refusal.request).await;
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

        let state = logs
            .engine()
            .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
        let stored = state.lock().lookup_tables.len();
        assert_eq!(
            stored, 0,
            "case {:?}: a refused create persists nothing, got {stored} tables",
            refusal.case
        );
    }
}

/// "The table name must be unique within your account and Region": a second create under a
/// name the account already holds is refused and leaves the stored table as it was.
#[tokio::test]
async fn a_duplicate_table_name_is_refused_and_overwrites_nothing() {
    let logs = Logs::new();
    const NAME: &str = "users_unique";
    const FIRST: &str = "user,team\nalice,platform\n";

    let created = logs
        .ok(CREATE, &json!({"lookupTableName": NAME, "tableBody": FIRST}))
        .await;
    let arn = member(&created, "lookupTableArn").expect("a create answers lookupTableArn");

    let (code, message) = logs
        .refusal(
            CREATE,
            &json!({"lookupTableName": NAME, "tableBody": "user\nbob\n"}),
        )
        .await;
    assert_eq!(
        code, ALREADY_EXISTS,
        "case \"a name the account already holds\": expected {ALREADY_EXISTS}, got {code} \
         {message:?}"
    );

    let read = logs.ok(GET, &json!({"lookupTableArn": arn})).await;
    assert_eq!(
        member(&read, "tableBody").as_deref(),
        Some(FIRST),
        "case \"a name the account already holds\": the refused create overwrites nothing, got \
         {read}"
    );
}

/// A create that would grow the account's table store past the emulator's own ceiling is
/// refused with the `LimitExceededException` the operation models, and stores nothing.
#[tokio::test]
async fn a_create_past_the_account_ceiling_is_refused() {
    let logs = Logs::new();

    // The store filled to exactly its ceiling, through the same record constructor the
    // create uses: the rule is about the COUNT the store holds, so how each table got
    // there does not matter.
    {
        let state = logs
            .engine()
            .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
        let mut store = state.lock();
        for index in 0..MAX_LOOKUP_TABLES {
            let name = format!("ceiling_{index}");
            let body = "user\nalice\n".to_string();
            let shape = parse_table_body(&body).expect("the filler body is valid CSV");
            let record = LookupTableRecord::new(
                &name,
                &lookup_table_arn(REGION, &name),
                body,
                shape,
                now_ms(),
            );
            store.lookup_tables.insert(name, record);
        }
    }

    let (code, message) = logs
        .refusal(
            CREATE,
            &json!({"lookupTableName": "one_past_the_ceiling", "tableBody": "user\nalice\n"}),
        )
        .await;
    assert_eq!(
        code, LIMIT_EXCEEDED,
        "case \"one create past the ceiling\": expected {LIMIT_EXCEEDED}, got {code} {message:?}"
    );
    assert!(
        message.contains(&MAX_LOOKUP_TABLES.to_string()),
        "case \"one create past the ceiling\": the refusal states the ceiling \
         ({MAX_LOOKUP_TABLES}), got {message:?}"
    );

    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let stored = state.lock().lookup_tables.len();
    assert_eq!(
        stored, MAX_LOOKUP_TABLES,
        "a refused create persists nothing, expected {MAX_LOOKUP_TABLES} tables, got {stored}"
    );
}

/// A create whose body would take the account's retained lookup-table BYTES past the
/// emulator's own byte budget is refused with the same modelled `LimitExceededException` —
/// the bound the table-count ceiling alone does not give, since one table may carry the
/// whole modelled 10 MB `tableBody`. The budget's exact bound is accepted and one byte past
/// it is not.
#[tokio::test]
async fn a_create_past_the_account_byte_budget_is_refused() {
    let logs = Logs::new();

    // The store filled to one byte short of the budget, through the same record constructor
    // the create uses: the rule is about the BYTES the store holds, so how they got there
    // does not matter. The body is built in ONE sized allocation, and its shape directly
    // rather than parsed, so filling the budget costs one pass and not three.
    const HEADER: &str = "user\n";
    const DATA_ROW: &str = "a\n";
    let rows = (MAX_LOOKUP_TABLE_BYTES_TOTAL - HEADER.len() - 1) / DATA_ROW.len();
    let filler = {
        // ONE allocation, sized exactly, and filled in place: no second copy of the body
        // exists at any point, so the test's peak is the body itself.
        let mut body = String::with_capacity(HEADER.len() + rows * DATA_ROW.len());
        body.push_str(HEADER);
        for _ in 0..rows {
            body.push_str(DATA_ROW);
        }
        body
    };
    let free = MAX_LOOKUP_TABLE_BYTES_TOTAL - filler.len();
    assert_eq!(
        free, 1,
        "the filler leaves exactly one byte of the budget free, got {free}"
    );
    {
        let records = i64::try_from(rows).expect("a row count");
        let state = logs
            .engine()
            .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
        let mut store = state.lock();
        let name = "budget_filler".to_string();
        let record = LookupTableRecord::new(
            &name,
            &lookup_table_arn(REGION, &name),
            filler,
            TableBodyShape {
                fields: vec!["user".to_string()],
                records_count: records,
            },
            now_ms(),
        );
        store.lookup_tables.insert(name, record);
    }

    // The one byte the budget still has free: a body of exactly that size takes the store to
    // the bound, which is not past it.
    logs.ok(
        CREATE,
        &json!({"lookupTableName": "budget_exact", "tableBody": "u"}),
    )
    .await;

    let (code, message) = logs
        .refusal(
            CREATE,
            &json!({"lookupTableName": "budget_one_past", "tableBody": "u"}),
        )
        .await;
    assert_eq!(
        code, LIMIT_EXCEEDED,
        "case \"one byte past the byte budget\": expected {LIMIT_EXCEEDED}, got {code} \
         {message:?}"
    );
    assert!(
        message.contains(&MAX_LOOKUP_TABLE_BYTES_TOTAL.to_string()),
        "case \"one byte past the byte budget\": the refusal states the budget \
         ({MAX_LOOKUP_TABLE_BYTES_TOTAL}), got {message:?}"
    );

    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let store = state.lock();
    assert_eq!(
        store.lookup_table_bytes(),
        MAX_LOOKUP_TABLE_BYTES_TOTAL,
        "a refused create retains nothing, so the store still holds exactly the budget"
    );
    assert!(
        !store.lookup_tables.contains_key("budget_one_past"),
        "a refused create persists no table, got {:?}",
        store.lookup_tables.keys().collect::<Vec<_>>()
    );
}

/// Reading an ARN no lookup table is stored under is a missing resource, not an empty
/// report — and a `lookupTableArn` that is not an ARN at all is refused for its form.
#[tokio::test]
async fn reading_a_table_the_account_does_not_hold_is_refused() {
    let logs = Logs::new();
    const NAME: &str = "users_held";
    logs.ok(
        CREATE,
        &json!({"lookupTableName": NAME, "tableBody": "user\nalice\n"}),
    )
    .await;

    // (case, the ARN read, the code it answers)
    let cases: [(&str, String, &str); 5] = [
        (
            "a value that is not an ARN",
            "users_held".to_string(),
            INVALID_PARAMETER,
        ),
        (
            "a lookup-table ARN for a name nothing created",
            lookup_table_arn(REGION, "users_never_created"),
            NOT_FOUND,
        ),
        (
            "this table's name in ANOTHER region, which this account does not hold here",
            lookup_table_arn("us-east-1", NAME),
            NOT_FOUND,
        ),
        (
            "a well-formed ARN of another resource kind",
            log_group_arn(REGION, NAME, false),
            NOT_FOUND,
        ),
        (
            "an ARN of another service entirely",
            "arn:aws:s3:::a-bucket".to_string(),
            NOT_FOUND,
        ),
    ];

    for (case, arn, expected) in cases {
        let (code, message) = logs.refusal(GET, &json!({"lookupTableArn": arn})).await;
        assert_eq!(
            code, expected,
            "case {case:?}: expected {expected}, got {code} {message:?}"
        );
    }

    // The table the account DOES hold still reports, so none of the refusals above touched
    // the store.
    let read = logs
        .ok(
            GET,
            &json!({"lookupTableArn": lookup_table_arn(REGION, NAME)}),
        )
        .await;
    assert_eq!(
        member(&read, "lookupTableName").as_deref(),
        Some(NAME),
        "the table the account holds still reports, got {read}"
    );
}
