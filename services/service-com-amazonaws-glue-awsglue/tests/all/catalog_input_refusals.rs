// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for HOW the Data Catalog refuses a request member whose value is
//! outside what the model admits for it.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: a request Glue's OWN input validation rejects — a value outside an
//! enum's value set, a `ResourceArn` outside `GlueResourceArn`'s pattern, a required name
//! member supplied as null or empty — is refused with HTTP 400 and the catalog's own
//! modelled `InvalidInputException`, whose prose travels in the `Message` member the model
//! declares and states the sentence this service words that check with. It is never the
//! framework's cross-service `ValidationException` and its lowercase `message`, which the
//! recordings show answering a DIFFERENT class: a value past a `@length` maximum, which
//! this suite pins on the same members from the other side.
//!
//! Each suite below also asserts the other side of its rule: a value the service DOES
//! admit reaches the operation's own behaviour instead of being refused, so the refusals
//! are decisions and not a blanket.
//!
//! The rows differ along what the rule generalises over: which member carries the value,
//! which operation read it, which constraint class the value broke (a required name
//! supplied as none, an enum's value set, an ARN's `@pattern`) and — for the enumerated
//! members — which value set the member's own enum declares, since two operations spelling
//! one member name do not always admit the same values.

use serde_json::{json, Value};

use crate::wire::{call, emulator, error_code, error_message, ok, ACCOUNT_ID, REGION};

/// The widest name the catalog accepts: `NameString`'s modelled `@length` maximum
/// ("length 1..255", `smithy_query(query="shape",shape="NameString")`), which every
/// database-name and table-name member below targets. Written here independently of the
/// handler, so the suite pins the bound and not the code.
const MAX_NAME_CHARS: usize = 255;

/// The sentence the catalog refuses an attribute that is no member of the attribute enum
/// its list targets with. Fixed text, naming neither the entry sent nor the admitted set.
const ATTRIBUTE_NOT_VALID: &str = "Attribute name is not a valid attribute to get.";

/// The refusal every row here must earn: HTTP 400, the catalog's own
/// `InvalidInputException`, and — when the row states one — that exact sentence in the
/// modelled `Message` member. The framework's lowercase `message` must be absent whatever
/// the row, because the answer is the operation's modelled error and not a boundary
/// refusal.
fn assert_catalog_input_refusal(case: &str, status: u16, body: &Value, expected: Option<&str>) {
    assert_eq!(
        status, 400,
        "case {case}: a value outside what the model admits is refused with 400, got \
         {status} with {body}"
    );
    assert!(
        error_code(body).ends_with("InvalidInputException"),
        "case {case}: expected InvalidInputException, got {} in {body}",
        error_code(body)
    );
    assert!(
        body.get("message").is_none(),
        "case {case}: the catalog's own error carries no lowercase message member, got \
         {body}"
    );
    match expected {
        Some(sentence) => assert_eq!(
            error_message(body),
            sentence,
            "case {case}: unexpected wording, got {body}"
        ),
        // A row whose sentence another suite pins still has to carry one: an error with no
        // prose at all is unreadable to the client that has to fix its request.
        None => assert!(
            !error_message(body).is_empty() && error_message(body) != "<no Message>",
            "case {case}: the refusal must state a message, got {body}"
        ),
    }
}

// ---------------------------------------------------------------------------
// A database or table name supplied as null or empty
// ---------------------------------------------------------------------------

/// One request that supplies no value for a name member, either way a request can supply
/// none: the member absent from the body, and the member carrying the empty string.
struct NameRow {
    case: &'static str,
    operation: &'static str,
    request: Value,
    expected: &'static str,
}

/// The sentence the catalog refuses a resource's OWN name member supplied as null or empty
/// with. Written here independently of the handler, so the suite pins the wording and not
/// the code.
const DATABASE_NAME_NULL_OR_EMPTY: &str = "Database name cannot be equal to null or empty";

/// The same for a table's own name member.
const TABLE_NAME_NULL_OR_EMPTY: &str = "Table name cannot be equal to null or empty";

/// The sentence it refuses a `DatabaseName` — the member addressing the database an
/// operation works in, rather than a database's own name — supplied as null or empty with.
const DATABASE_NAME_REQUIRED: &str = "Database name is required";

/// Every name member of the catalog's database and table operations, each supplied no
/// value in both of the two ways a request can supply none.
fn name_rows() -> Vec<NameRow> {
    vec![
        NameRow {
            case: "CreateDatabase with an empty DatabaseInput.Name",
            operation: "CreateDatabase",
            request: json!({ "DatabaseInput": { "Name": "" } }),
            expected: DATABASE_NAME_NULL_OR_EMPTY,
        },
        NameRow {
            case: "CreateDatabase with no DatabaseInput.Name at all",
            operation: "CreateDatabase",
            request: json!({ "DatabaseInput": {} }),
            expected: DATABASE_NAME_NULL_OR_EMPTY,
        },
        NameRow {
            case: "UpdateDatabase addressing an empty Name",
            operation: "UpdateDatabase",
            request: json!({ "Name": "", "DatabaseInput": { "Name": "input_refusals_db" } }),
            expected: DATABASE_NAME_NULL_OR_EMPTY,
        },
        NameRow {
            case: "UpdateDatabase addressing no Name at all",
            operation: "UpdateDatabase",
            request: json!({ "DatabaseInput": { "Name": "input_refusals_db" } }),
            expected: DATABASE_NAME_NULL_OR_EMPTY,
        },
        NameRow {
            case: "UpdateDatabase submitting an empty DatabaseInput.Name",
            operation: "UpdateDatabase",
            request: json!({ "Name": "input_refusals_db", "DatabaseInput": { "Name": "" } }),
            expected: DATABASE_NAME_NULL_OR_EMPTY,
        },
        NameRow {
            case: "CreateTable with an empty DatabaseName",
            operation: "CreateTable",
            request: json!({ "DatabaseName": "", "TableInput": { "Name": "events" } }),
            expected: DATABASE_NAME_REQUIRED,
        },
        NameRow {
            case: "CreateTable with no DatabaseName at all",
            operation: "CreateTable",
            request: json!({ "TableInput": { "Name": "events" } }),
            expected: DATABASE_NAME_REQUIRED,
        },
        NameRow {
            case: "CreateTable with an empty TableInput.Name",
            operation: "CreateTable",
            request: json!({
                "DatabaseName": "input_refusals_db",
                "TableInput": { "Name": "" },
            }),
            expected: TABLE_NAME_NULL_OR_EMPTY,
        },
        NameRow {
            case: "CreateTable with no TableInput.Name at all",
            operation: "CreateTable",
            request: json!({ "DatabaseName": "input_refusals_db", "TableInput": {} }),
            expected: TABLE_NAME_NULL_OR_EMPTY,
        },
        NameRow {
            case: "UpdateTable with an empty TableInput.Name",
            operation: "UpdateTable",
            request: json!({
                "DatabaseName": "input_refusals_db",
                "TableInput": { "Name": "" },
            }),
            expected: TABLE_NAME_NULL_OR_EMPTY,
        },
        NameRow {
            case: "UpdateTable with no DatabaseName at all",
            operation: "UpdateTable",
            request: json!({ "TableInput": { "Name": "events" } }),
            expected: DATABASE_NAME_REQUIRED,
        },
        NameRow {
            case: "GetTable with an empty Name",
            operation: "GetTable",
            request: json!({ "DatabaseName": "input_refusals_db", "Name": "" }),
            expected: TABLE_NAME_NULL_OR_EMPTY,
        },
        NameRow {
            case: "GetTable with no Name at all",
            operation: "GetTable",
            request: json!({ "DatabaseName": "input_refusals_db" }),
            expected: TABLE_NAME_NULL_OR_EMPTY,
        },
        NameRow {
            case: "GetTable with an empty DatabaseName",
            operation: "GetTable",
            request: json!({ "DatabaseName": "", "Name": "events" }),
            expected: DATABASE_NAME_REQUIRED,
        },
        NameRow {
            case: "GetTable with no DatabaseName at all",
            operation: "GetTable",
            request: json!({ "Name": "events" }),
            expected: DATABASE_NAME_REQUIRED,
        },
        NameRow {
            case: "GetTables with an empty DatabaseName",
            operation: "GetTables",
            request: json!({ "DatabaseName": "" }),
            expected: DATABASE_NAME_REQUIRED,
        },
        NameRow {
            case: "GetTables with no DatabaseName at all",
            operation: "GetTables",
            request: json!({}),
            expected: DATABASE_NAME_REQUIRED,
        },
    ]
}

/// A name member supplied as null or empty is the catalog's own input error, in the
/// catalog's own wording — and the refused write stores nothing.
#[tokio::test]
async fn a_name_supplied_as_null_or_empty_is_the_catalogs_own_input_error() {
    let router = emulator();
    // A database and a table the rows above address, so a row is refused for its NAME and
    // not for a database or table that does not exist.
    ok(
        &router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": "input_refusals_db" } }),
    )
    .await;
    ok(
        &router,
        "CreateTable",
        &json!({
            "DatabaseName": "input_refusals_db",
            "TableInput": { "Name": "events", "TableType": "EXTERNAL_TABLE" },
        }),
    )
    .await;

    for row in name_rows() {
        let (status, body) = call(&router, row.operation, &row.request).await;
        assert_catalog_input_refusal(row.case, status, &body, Some(row.expected));
    }

    // Nothing the refused writes named was stored: the catalog still holds exactly the one
    // database and the one table the successful creates made.
    let databases = ok(&router, "GetDatabases", &json!({})).await;
    let held: Vec<&str> = databases["DatabaseList"]
        .as_array()
        .expect("DatabaseList is a list")
        .iter()
        .map(|database| database["Name"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        held,
        vec!["input_refusals_db"],
        "a refused create must store nothing, got {databases}"
    );
    let tables = ok(
        &router,
        "GetTables",
        &json!({ "DatabaseName": "input_refusals_db" }),
    )
    .await;
    assert_eq!(
        crate::wire::table_names(&tables),
        vec!["events".to_string()],
        "a refused table write must store nothing, got {tables}"
    );
}

/// A name PAST the member's modelled maximum is the other layer's refusal and keeps it: the
/// cross-service constraint validator's `ValidationException`, whose lowercase `message`
/// carries the validation envelope naming the member path and the bound. The two layers
/// divide the same member, so this is the other side of the suite above.
#[tokio::test]
async fn a_name_past_the_modelled_maximum_keeps_the_constraint_refusal() {
    let router = emulator();
    let long = "d".repeat(MAX_NAME_CHARS + 1);

    // (case, operation, request, the member path the clause names).
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "CreateDatabase with a DatabaseInput.Name past the maximum",
            "CreateDatabase",
            json!({ "DatabaseInput": { "Name": long } }),
            "databaseInput.name",
        ),
        (
            "CreateTable with a TableInput.Name past the maximum",
            "CreateTable",
            json!({ "DatabaseName": "any_db", "TableInput": { "Name": long } }),
            "tableInput.name",
        ),
        (
            "GetTable with a Name past the maximum",
            "GetTable",
            json!({ "DatabaseName": "any_db", "Name": long }),
            "name",
        ),
        (
            "GetTables with a DatabaseName past the maximum",
            "GetTables",
            json!({ "DatabaseName": long }),
            "databaseName",
        ),
    ];

    for (case, operation, request, member_path) in &rows {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            status, 400,
            "case {case}: a name past the modelled maximum is refused, got {status} with \
             {body}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {case}: expected the validator's ValidationException, got {} in {body}",
            error_code(&body)
        );
        let clause = body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("<no message>");
        assert!(
            clause.starts_with("1 validation error detected: ")
                && clause.contains(&format!("at '{member_path}'"))
                && clause.contains(&format!(
                    "Member must have length less than or equal to {MAX_NAME_CHARS}"
                )),
            "case {case}: expected the validation envelope naming {member_path} and the \
             bound, got {clause:?}"
        );
        // The clause quotes the value the caller submitted, whole: a validation message is
        // the only place the server reports the offending input back, so a client that
        // compares the echo against what it sent has to see its own bytes.
        assert!(
            clause.contains(&long),
            "case {case}: the clause must echo the submitted name whole, got {clause:?}"
        );
    }
}

/// A name far past the modelled maximum is still echoed whole by the clause, rather than
/// summarised: the echo is the caller's own bytes at any length the member can carry.
#[tokio::test]
async fn a_constraint_clause_echoes_the_whole_submitted_name() {
    let router = emulator();
    // Four times the maximum, which is also past the framework walk's own echo ceiling, so
    // the row exercises the restoring path and not just a short value.
    let very_long = "d".repeat(MAX_NAME_CHARS * 4);

    let (status, body) = call(
        &router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": very_long } }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a name far past the maximum is refused, got {status} with {body}"
    );
    let clause = body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>");
    assert!(
        clause.contains(&very_long),
        "the clause must echo the whole submitted name, got {} characters of message",
        clause.len()
    );
}

/// A name AT the modelled maximum is inside the bound, so it is served and reads back:
/// the refusals above are a decision about the bound and not about long names.
#[tokio::test]
async fn a_name_at_the_modelled_maximum_is_served() {
    let router = emulator();
    let at_maximum = "d".repeat(MAX_NAME_CHARS);

    ok(
        &router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": at_maximum } }),
    )
    .await;
    let read = ok(&router, "GetDatabase", &json!({ "Name": at_maximum })).await;
    assert_eq!(
        read["Database"]["Name"].as_str().unwrap_or_default(),
        at_maximum,
        "a database created at the modelled maximum reads back under that name, got {read}"
    );

    let table_at_maximum = "t".repeat(MAX_NAME_CHARS);
    ok(
        &router,
        "CreateTable",
        &json!({
            "DatabaseName": at_maximum,
            "TableInput": { "Name": table_at_maximum, "TableType": "EXTERNAL_TABLE" },
        }),
    )
    .await;
    let table = ok(
        &router,
        "GetTable",
        &json!({ "DatabaseName": at_maximum, "Name": table_at_maximum }),
    )
    .await;
    assert_eq!(
        table["Table"]["Name"].as_str().unwrap_or_default(),
        table_at_maximum,
        "a table created at the modelled maximum reads back under that name, got {table}"
    );
}

// ---------------------------------------------------------------------------
// A share type outside the enum its member targets
// ---------------------------------------------------------------------------

/// The sentence the catalog refuses a share type outside that enum with: fixed text plus
/// the value the request sent, echoed as sent.
fn share_type_not_valid(submitted: &str) -> String {
    format!("Resource share type {submitted} is not valid")
}

/// A share type no enum member spells is the catalog's own input error, naming the value
/// the request sent — and which values an operation admits is its OWN member's enum:
/// `SearchTables` and `GetDatabases` target `ResourceShareType` (FOREIGN, ALL, FEDERATED)
/// while `GetTables` targets `TableResourceShareType` (FEDERATED, ALL), so `FOREIGN` is a
/// served value for the first two and a refused one for the third
/// (`smithy_query(query="shape",shape="ResourceShareType,TableResourceShareType")`).
#[tokio::test]
async fn a_share_type_outside_the_modelled_enum_is_the_catalogs_own_input_error() {
    let router = emulator();
    ok(
        &router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": "share_type_db" } }),
    )
    .await;

    // (case, operation, the share type sent). Each value is outside the enum THAT
    // operation's member targets: a member's value in the wrong case, a word no member
    // spells, the empty string, and a value another operation's enum does admit.
    let refused: Vec<(&str, &str, &str)> = vec![
        ("SearchTables, a member in lower case", "SearchTables", "foreign"),
        ("SearchTables, a word no member spells", "SearchTables", "LOCAL"),
        ("SearchTables, the empty string", "SearchTables", ""),
        ("GetTables, a member in lower case", "GetTables", "federated"),
        (
            "GetTables, a value only the other enum admits",
            "GetTables",
            "FOREIGN",
        ),
        ("GetDatabases, a member in lower case", "GetDatabases", "all"),
        (
            "GetDatabases, a word no member spells",
            "GetDatabases",
            "SHARED",
        ),
    ];

    for (case, operation, share_type) in &refused {
        let mut request = json!({ "ResourceShareType": share_type });
        if *operation == "GetTables" {
            request["DatabaseName"] = json!("share_type_db");
        }
        let (status, body) = call(&router, operation, &request).await;
        assert_catalog_input_refusal(case, status, &body, Some(&share_type_not_valid(share_type)));
    }

    // (case, operation, a share type that operation's own enum declares). Each is served,
    // so the refusals above are about the value set and not about the member.
    let served: Vec<(&str, &str, &str)> = vec![
        ("SearchTables FOREIGN", "SearchTables", "FOREIGN"),
        ("SearchTables ALL", "SearchTables", "ALL"),
        ("SearchTables FEDERATED", "SearchTables", "FEDERATED"),
        ("GetTables ALL", "GetTables", "ALL"),
        ("GetTables FEDERATED", "GetTables", "FEDERATED"),
        ("GetDatabases FOREIGN", "GetDatabases", "FOREIGN"),
        ("GetDatabases ALL", "GetDatabases", "ALL"),
        ("GetDatabases FEDERATED", "GetDatabases", "FEDERATED"),
    ];

    for (case, operation, share_type) in &served {
        let mut request = json!({ "ResourceShareType": share_type });
        if *operation == "GetTables" {
            request["DatabaseName"] = json!("share_type_db");
        }
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 200,
            "case {case}: a share type the member's own enum declares is served, got \
             {status} with {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// An attribute outside the enum its list targets
// ---------------------------------------------------------------------------

/// An `AttributesToGet` entry that is no member of the attribute enum its list targets is
/// the catalog's own input error, in the catalog's own fixed wording — while a projection
/// the enum DOES admit reaches the operation's own validation of the combination, which is
/// a different refusal with a different sentence.
#[tokio::test]
async fn an_attribute_outside_the_modelled_enum_is_the_catalogs_own_input_error() {
    let router = emulator();
    ok(
        &router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": "attributes_db" } }),
    )
    .await;
    ok(
        &router,
        "CreateTable",
        &json!({
            "DatabaseName": "attributes_db",
            "TableInput": { "Name": "events", "TableType": "EXTERNAL_TABLE" },
        }),
    )
    .await;

    // (case, operation, AttributesToGet). `GetTable` and `GetTables` project through
    // `TableAttributes` (NAME, TABLE_TYPE, DEFAULT, LATEST_ICEBERG_METADATA) and
    // `GetDatabases` through `DatabaseAttributes` (NAME, TARGET_DATABASE)
    // (`smithy_query(query="shape",shape="TableAttributes,DatabaseAttributes")`), so each
    // row below carries an entry outside its own list's value set: a member in the wrong
    // case, a word no member spells, and a member of the OTHER list's enum.
    let refused: Vec<(&str, &str, Value)> = vec![
        ("GetTables, a member in lower case", "GetTables", json!(["name"])),
        (
            "GetTables, a valid member beside a word no member spells",
            "GetTables",
            json!(["NAME", "OWNER"]),
        ),
        (
            "GetTables, a member of the database enum",
            "GetTables",
            json!(["TARGET_DATABASE"]),
        ),
        ("GetTable, a member in lower case", "GetTable", json!(["default"])),
        (
            "GetTable, a valid member beside a word no member spells",
            "GetTable",
            json!(["DEFAULT", "STORAGE_DESCRIPTOR"]),
        ),
        ("GetDatabases, a member in lower case", "GetDatabases", json!(["name"])),
        (
            "GetDatabases, a member of the table enum",
            "GetDatabases",
            json!(["NAME", "TABLE_TYPE"]),
        ),
    ];

    for (case, operation, attributes) in &refused {
        let mut request = json!({ "AttributesToGet": attributes });
        match *operation {
            "GetTables" => request["DatabaseName"] = json!("attributes_db"),
            "GetTable" => {
                request["DatabaseName"] = json!("attributes_db");
                request["Name"] = json!("events");
            }
            _ => {}
        }
        let (status, body) = call(&router, operation, &request).await;
        assert_catalog_input_refusal(case, status, &body, Some(ATTRIBUTE_NOT_VALID));
    }

    // A projection every entry of which IS an enum member, but whose COMBINATION the
    // operation does not document: still the catalog's own input error, and NOT the
    // attribute sentence above — the two checks say different things.
    let (status, body) = call(
        &router,
        "GetTables",
        &json!({ "DatabaseName": "attributes_db", "AttributesToGet": ["TABLE_TYPE"] }),
    )
    .await;
    assert_catalog_input_refusal(
        "GetTables, a documented member in an undocumented combination",
        status,
        &body,
        None,
    );
    assert_ne!(
        error_message(&body),
        ATTRIBUTE_NOT_VALID,
        "a combination of admitted members is not an unadmitted attribute, got {body}"
    );

    // And a projection the operation documents is served, so the refusals above are about
    // the value set and not about the member.
    let page = ok(
        &router,
        "GetTables",
        &json!({ "DatabaseName": "attributes_db", "AttributesToGet": ["NAME"] }),
    )
    .await;
    assert_eq!(
        crate::wire::table_names(&page),
        vec!["events".to_string()],
        "a documented projection is served, got {page}"
    );
}

// ---------------------------------------------------------------------------
// A ResourceArn that is no Glue resource ARN
// ---------------------------------------------------------------------------

/// A `ResourceArn` that is no Glue resource ARN at all is the catalog's own input error on
/// every tagging operation — not the framework's `ValidationException` for the member's
/// `@pattern` — while a well-formed ARN reaches the lookup: one naming a resource this
/// store holds none of is the modelled `EntityNotFoundException`, and one naming a stored
/// resource is served.
#[tokio::test]
async fn a_resource_arn_that_is_no_glue_arn_is_the_catalogs_own_input_error() {
    let router = emulator();
    ok(
        &router,
        "CreateDatabase",
        &json!({
            "DatabaseInput": { "Name": "tagged_db" },
            "Tags": { "team": "data" },
        }),
    )
    .await;

    // (case, the ResourceArn sent). Every row is outside `GlueResourceArn`'s modelled
    // `@pattern` or names no taggable Glue resource: a bare resource name, another
    // service's namespace, an ARN missing fields, the empty string, and a Glue ARN whose
    // resource portion names no type at all.
    let arns: Vec<(&str, String)> = vec![
        ("a bare resource name", "tagged_db".to_string()),
        (
            "another service's namespace",
            format!("arn:aws:s3:::{REGION}-lake"),
        ),
        (
            "a Glue ARN missing its resource field",
            format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}"),
        ),
        ("the empty string", String::new()),
        (
            "a Glue ARN whose resource names no type",
            format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:tagged_db"),
        ),
        (
            "a Glue ARN of a resource kind the catalog tags none of",
            format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:catalog/default"),
        ),
    ];

    for (case, arn) in &arns {
        for operation in ["GetTags", "TagResource", "UntagResource"] {
            let request = match operation {
                "TagResource" => json!({ "ResourceArn": arn, "TagsToAdd": { "k": "v" } }),
                "UntagResource" => json!({ "ResourceArn": arn, "TagsToRemove": ["k"] }),
                _ => json!({ "ResourceArn": arn }),
            };
            let (status, body) = call(&router, operation, &request).await;
            assert_catalog_input_refusal(&format!("{operation} with {case}"), status, &body, None);
        }
    }

    // A well-formed ARN of a taggable kind this store holds nothing for is a MISS, not a
    // malformed request: the member still resolves.
    let (status, body) = call(
        &router,
        "GetTags",
        &json!({
            "ResourceArn": format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:database/absent_db"),
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "an ARN naming no stored resource is refused, got {status} with {body}"
    );
    assert!(
        error_code(&body).ends_with("EntityNotFoundException"),
        "an ARN naming no stored resource is a miss, got {} in {body}",
        error_code(&body)
    );

    // And the ARN of the database the create tagged serves its tag set.
    let tags = ok(
        &router,
        "GetTags",
        &json!({
            "ResourceArn": format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:database/tagged_db"),
        }),
    )
    .await;
    assert_eq!(
        tags["Tags"],
        json!({ "team": "data" }),
        "the ARN of a tagged database serves its tags, got {tags}"
    );
}
