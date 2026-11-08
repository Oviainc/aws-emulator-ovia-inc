// SPDX-License-Identifier: CC0-1.0

//! How `DescribeTimeToLive` ADDRESSES the table it reports on, and that reporting it
//! changes nothing a later caller sees.
//!
//! Two rules, each asserted over a SET of inputs rather than one example:
//!
//! 1. A `TableName` member targets `TableArn`, so it accepts the bare table name or the
//!    table's ARN ("You can also provide the Amazon Resource Name (ARN) of the table in
//!    this parameter" — the `DescribeTimeToLive$TableName` doc). Every spelling of one
//!    table's identity resolves to the SAME table, so the setting reported for it is the
//!    same whichever spelling asked; a value that addresses no table of the endpoint the
//!    request reached is an input refusal (HTTP 400 `ValidationException`) worded over
//!    what was sent, and an addressable table that is not there is HTTP 400
//!    `ResourceNotFoundException`. The member is resolved the same way wherever it
//!    appears, so this reader's verdict on a value is the table reader's
//!    (`DescribeTable`) verdict on it.
//! 2. "Gives a description of the Time to Live (TTL) status on the specified table" (the
//!    operation's doc): it REPORTS, so repeated reads answer the same description and
//!    leave the setting as a later `UpdateTimeToLive` finds it.
//!
//! Every call goes over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION;
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, emulator, error_code, message, ok, REGION};

/// The account every ARN this emulator mints carries, and so the account a caller
/// spelling a table's ARN by hand addresses.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// A second region, for the one addressing row that is about scope: an endpoint serves
/// one region, so an ARN naming another names nothing it can reach.
const OTHER_REGION: &str = "eu-west-1";

/// The one key attribute every table here is created with; no row varies it.
const PARTITION_KEY: &str = "Part";

/// The TTL attribute the tables below expire items by. Which attribute it is is pinned
/// by `tests/all/time_to_live.rs`; here it is only the value that must survive the round
/// trip through every spelling of the table's name.
const TTL_ATTRIBUTE: &str = "expires_at";

/// One row of the addressing table: what the row is called, the bare table name, and the
/// spellings of that table's identity (each as what the spelling is called and the value
/// a request carries).
type AddressingRow<'a> = (&'a str, &'a str, Vec<(String, String)>);

/// Wait out one transient span, so whatever the clock owes a table or its TTL setting is
/// due.
async fn settle() {
    tokio::time::sleep(STATUS_TRANSITION + Duration::from_millis(150)).await;
}

/// Create one on-demand table and answer the ARN the service minted for it.
async fn create_table(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(
        router,
        "CreateTable",
        json!({
            "TableName": name,
            "KeySchema": [{"AttributeName": PARTITION_KEY, "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": PARTITION_KEY, "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    created["TableDescription"]["TableArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The `TimeToLiveDescription` the reader reports for the table addressed by `table_name`
/// — a bare name or an ARN.
async fn described_ttl(router: &MultiServiceRouter, table_name: &str) -> Value {
    ok(router, "DescribeTimeToLive", json!({"TableName": table_name}))
        .await
        .get("TimeToLiveDescription")
        .cloned()
        .unwrap_or(Value::Null)
}

/// Turn TTL on or off for the table addressed by `table_name`.
async fn set_ttl(router: &MultiServiceRouter, table_name: &str, enabled: bool) -> Value {
    ok(
        router,
        "UpdateTimeToLive",
        json!({
            "TableName": table_name,
            "TimeToLiveSpecification": {"Enabled": enabled, "AttributeName": TTL_ATTRIBUTE},
        }),
    )
    .await
}

#[tokio::test]
async fn every_spelling_of_a_table_s_name_reaches_the_same_ttl_setting() {
    let router = emulator();

    // Rows differ in the FORM of the table name the spellings carry: each published
    // bound of the documented 3..=255 length (the `CreateTable$TableName` doc) and each
    // character class its `[a-zA-Z0-9_.-]+` pattern allows.
    let longest = "z".repeat(255);
    let names: [(&str, &str); 5] = [
        ("the shortest name", "abc"),
        ("a conventional name", "orders-2026"),
        ("a dotted name", "orders.v2.final"),
        ("an underscored name", "orders_v2"),
        ("the longest name", longest.as_str()),
    ];

    // Create every table first and wait ONE transition out: TTL is a setting of a settled
    // table, and a per-row wait would only make the suite slower.
    let mut rows: Vec<AddressingRow<'_>> = Vec::new();
    for (case, name) in names {
        let minted = create_table(&router, name).await;
        assert!(
            minted.ends_with(&format!(":table/{name}")),
            "case {case:?}: the create mints the table's own ARN, got {minted:?}"
        );
        // The spellings of one table's identity: the bare name, the ARN the create
        // minted, and the same ARN as a caller spells it for this endpoint's region.
        let spellings = vec![
            ("the bare name".to_string(), name.to_string()),
            ("the minted ARN".to_string(), minted),
            (
                "a caller-spelled ARN".to_string(),
                format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/{name}"),
            ),
        ];
        rows.push((case, name, spellings));
    }
    settle().await;

    // A table no `UpdateTimeToLive` has named reports TTL off, through every spelling —
    // and reports it again unchanged, because a read stores nothing.
    for (case, _, spellings) in &rows {
        for (spelling, value) in spellings {
            let fresh = described_ttl(&router, value).await;
            assert_eq!(
                fresh,
                json!({"TimeToLiveStatus": "DISABLED"}),
                "case {case:?} by {spelling}: a table with no TTL reports it off, got {fresh}"
            );
            let again = described_ttl(&router, value).await;
            assert_eq!(
                again, fresh,
                "case {case:?} by {spelling}: a second read reports the same description, got \
                 {again}"
            );
        }
    }

    // Those reads left the setting as a later change finds it: the ARN spelling may make
    // the change, and the bare name reads the result. ONE change per table — DynamoDB takes
    // one change to a table's TTL setting per modification interval, which
    // `tests/all/time_to_live.rs` pins — so this is the change every spelling is then read
    // against.
    for (case, name, spellings) in &rows {
        let by_arn = &spellings[1].1;
        let stored = set_ttl(&router, by_arn, true).await;
        assert_eq!(
            stored["TimeToLiveSpecification"].clone(),
            json!({"Enabled": true, "AttributeName": TTL_ATTRIBUTE}),
            "case {case:?}: a change addressed by ARN is accepted, got {stored}"
        );
        let during = described_ttl(&router, name).await;
        assert_eq!(
            during,
            json!({"TimeToLiveStatus": "ENABLING", "AttributeName": TTL_ATTRIBUTE}),
            "case {case:?}: the change is still processing, got {during}"
        );
    }
    settle().await;

    // Every spelling now reports the setting the ARN spelling stored.
    for (case, _, spellings) in &rows {
        let expected = json!({
            "TimeToLiveStatus": "ENABLED",
            "AttributeName": TTL_ATTRIBUTE,
        });
        for (spelling, value) in spellings {
            let settled = described_ttl(&router, value).await;
            assert_eq!(
                settled, expected,
                "case {case:?} by {spelling}: the stored setting is reported, got {settled}"
            );
        }
    }
}

#[tokio::test]
async fn a_table_name_the_ttl_reader_cannot_address_is_refused_as_the_table_reader_refuses_it() {
    let router = emulator();
    // One real table, so every refusal below is about the value that was sent and not
    // about an empty region.
    let present = "ttl-addressing-table";
    let minted = create_table(&router, present).await;
    settle().await;

    // (case, the submitted TableName, the error code it earns, a phrase its message
    // states). The rows span the ways a value can fail to address a table of the endpoint
    // the request reached: the ARN's shape (too few fields, another service, another
    // resource type, a deeper sub-resource), its scope (another region), the documented
    // 3..=255 `[a-zA-Z0-9_.-]+` form of the name inside it, and a well-formed address for
    // a table that is simply not there.
    let too_long = "z".repeat(256);
    let cases: Vec<(&str, String, &str, String)> = vec![
        (
            "an ARN with too few fields",
            format!("arn:aws:dynamodb:{REGION}:table/{present}"),
            "ValidationException",
            "Valid ARN format is".to_string(),
        ),
        (
            "an ARN of another service",
            format!("arn:aws:sqs:{REGION}:{ACCOUNT}:{present}"),
            "ValidationException",
            "at 'tableName' failed to satisfy constraint".to_string(),
        ),
        (
            "an ARN naming another resource type",
            format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:backup/{present}"),
            "ValidationException",
            format!("Invalid resource type in 'arn:aws:dynamodb:{REGION}:{ACCOUNT}:backup/{present}'"),
        ),
        (
            "an index sub-resource ARN",
            format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/{present}/index/by-part"),
            "ValidationException",
            "Invalid resource type in".to_string(),
        ),
        (
            "an ARN of another region",
            format!("arn:aws:dynamodb:{OTHER_REGION}:{ACCOUNT}:table/{present}"),
            "ValidationException",
            format!(
                "Invalid AWS region in 'arn:aws:dynamodb:{OTHER_REGION}:{ACCOUNT}:table/{present}'"
            ),
        ),
        (
            "a name below the documented minimum length",
            "ab".to_string(),
            "ValidationException",
            "Member must have length greater than or equal to 3".to_string(),
        ),
        (
            "a name above the documented maximum length",
            too_long.clone(),
            "ValidationException",
            "Member must have length less than or equal to 255".to_string(),
        ),
        (
            "a name outside the documented pattern",
            "not a name".to_string(),
            "ValidationException",
            "Member must satisfy regular expression pattern".to_string(),
        ),
        (
            "a name no table carries",
            "absent-ttl-table".to_string(),
            "ResourceNotFoundException",
            "Requested resource not found: Table: absent-ttl-table not found".to_string(),
        ),
        (
            "an in-region ARN naming no table",
            format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/absent-ttl-table"),
            "ResourceNotFoundException",
            "Requested resource not found: Table: absent-ttl-table not found".to_string(),
        ),
    ];

    for (case, table_name, code, stated) in &cases {
        let (status, body) = call(
            &router,
            "DescribeTimeToLive",
            json!({"TableName": table_name}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, *code),
            "case {case:?}: expected 400 {code} for {table_name:?}, got {status} with {body}"
        );
        let said = message(&body);
        assert!(
            said.contains(stated.as_str()),
            "case {case:?}: the refusal states {stated:?}, got {said:?}"
        );

        // The member is resolved the same way wherever it appears, so the table reader
        // answers this value exactly as the TTL reader did.
        let (table_status, table_body) =
            call(&router, "DescribeTable", json!({"TableName": table_name})).await;
        assert_eq!(
            (table_status, error_code(&table_body), message(&table_body)),
            (status, error_code(&body), said.clone()),
            "case {case:?}: both readers resolve the member alike, got {table_status} with \
             {table_body}"
        );
    }

    // The table that IS there is still served, by name and by its ARN: the refusals above
    // are about the values sent and not about the operation.
    for (spelling, value) in [("the bare name", present.to_string()), ("its ARN", minted)] {
        let served = described_ttl(&router, &value).await;
        assert_eq!(
            served,
            json!({"TimeToLiveStatus": "DISABLED"}),
            "case the present table by {spelling}: it is served, got {served}"
        );
    }
}
