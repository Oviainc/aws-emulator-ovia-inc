// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteLookupTable`, the operation that removes a lookup table.
//!
//! The rule under test: a delete removes, permanently, exactly the ONE lookup table the
//! account holds under the ARN the request names — the table stops being reported by every
//! reader of it (the single-table read and the enumeration), the bytes its CSV content held
//! stop counting against the account's retained content, and the name it held is free for a
//! later create; every other table the account holds is left exactly as it was. An ARN no
//! stored table answers under — a name nothing created, another Region's or another
//! account's table, another resource kind, another service, or a table this account already
//! deleted — is a missing resource, and a `lookupTableArn` that is not an ARN at all is
//! refused for its form; neither removes anything.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The tables here span what the rule generalises over rather than one request: the seeded
//! corpus differs in column count (one and several), in data-row count (none, one, several),
//! in CSV quoting (a quoted field carrying the separator), in encoding width (a multibyte
//! row, where the bytes released are not the character count), in name case and in whether
//! one name is a prefix of another, and in which optional members the create named; the
//! deletes then run in an order that is neither the creation order nor the name order, so
//! the table removed is a first, a middle and a last one of the enumeration in turn.

use serde_json::{json, Map, Value};

use crate::harness::{Logs, REGION};
use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    log_group_arn, lookup_table_arn, resource_not_found_message, unparsable_arn_message,
    ComAmazonawsCloudwatchlogsLogs20140328State,
};

/// The operation under test, the one that puts a table in the store, and the two readers a
/// removal must take the table out of.
const DELETE: &str = "DeleteLookupTable";
const CREATE: &str = "CreateLookupTable";
const GET: &str = "GetLookupTable";
const DESCRIBE: &str = "DescribeLookupTables";

/// The code CloudWatch Logs answers for a parameter it will not accept, which is what
/// `DeleteLookupTable` declares for one ("A parameter is specified incorrectly").
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The code answered when the resource a request names is not there.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// A KMS key ARN, for the seeded tables that are encrypted with a customer key.
const KMS_KEY: &str = "arn:aws:kms:eu-west-2:000000000000:key/aaaabbbb-cccc-dddd-eeee-ffff00001111";

/// One seeded lookup table: what the create uploads and names, so a reader of a table the
/// delete did NOT remove can be held to it.
struct Seed {
    /// What this table varies, for the assertion messages.
    case: &'static str,
    name: &'static str,
    table_body: &'static str,
    description: Option<&'static str>,
    kms_key_id: Option<&'static str>,
    tags: Option<&'static [(&'static str, &'static str)]>,
}

impl Seed {
    /// A seed naming nothing but the required name and the CSV body.
    const fn bare(case: &'static str, name: &'static str, table_body: &'static str) -> Self {
        Self {
            case,
            name,
            table_body,
            description: None,
            kms_key_id: None,
            tags: None,
        }
    }
}

/// The seeded corpus, in the order the tables are CREATED in — neither the order they are
/// deleted in nor the order the enumeration reports them in.
fn seeds() -> Vec<Seed> {
    vec![
        Seed {
            description: Some("the user directory"),
            kms_key_id: Some(KMS_KEY),
            ..Seed::bare(
                "several columns and several data rows",
                "users",
                "user,team,region\nalice,platform,eu-west-2\nbob,data,us-east-1\n",
            )
        },
        Seed::bare("one column and no data rows", "metrics", "metric\n"),
        Seed {
            tags: Some(&[("team", "platform"), ("env", "test")]),
            ..Seed::bare(
                "a name another table's name is a prefix of",
                "users_eu",
                "user\nalice\n",
            )
        },
        Seed::bare(
            "an upper-case initial, which sorts before every lower-case one",
            "Users_upper",
            "user,team\nAlice,Platform\n",
        ),
        Seed::bare(
            "a quoted field carrying the separator, so one field is not one cell of text",
            "notes",
            "user,note\nalice,\"platform, data\"\n",
        ),
        Seed {
            description: Some("a multibyte row"),
            ..Seed::bare(
                "a multibyte body, whose bytes are not its character count",
                "users_utf8",
                "user\n\u{30ab}\u{30eb}\u{30ed}\u{30b9}\n",
            )
        },
    ]
}

/// The order the seeds are deleted in: neither the creation order nor the ascending name
/// order the enumeration reports, so the table removed is a middle, a last, a first and a
/// middle one of the remaining enumeration in turn.
const DELETE_ORDER: [usize; 6] = [2, 0, 3, 5, 1, 4];

/// The request body one seed's create sends.
fn create_request(seed: &Seed) -> Value {
    let mut request = Map::new();
    request.insert("lookupTableName".to_string(), json!(seed.name));
    request.insert("tableBody".to_string(), json!(seed.table_body));
    if let Some(description) = seed.description {
        request.insert("description".to_string(), json!(description));
    }
    if let Some(kms_key_id) = seed.kms_key_id {
        request.insert("kmsKeyId".to_string(), json!(kms_key_id));
    }
    if let Some(tags) = seed.tags {
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

/// The names the enumeration reports, in the order it reports them. The corpus is far
/// inside one page, so a single call sees all of it.
async fn listed_names(logs: &Logs) -> Vec<String> {
    let page = logs.ok(DESCRIBE, &json!({})).await;
    assert!(
        page.get("nextToken").is_none(),
        "the seeded corpus fits in one page, got {page}"
    );
    page.get("lookupTables")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{DESCRIBE} must return lookupTables, got {page}"))
        .iter()
        .map(|table| {
            member(table, "lookupTableName")
                .unwrap_or_else(|| panic!("a reported table carries its name, got {table}"))
        })
        .collect()
}

/// How many bytes of lookup-table content the account's store holds, which is what a
/// removal must release.
fn stored_bytes(logs: &Logs) -> usize {
    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let store = state.lock();
    store.lookup_table_bytes()
}

/// Every seeded table, deleted one at a time: the delete answers an empty Unit output, the
/// deleted table is gone from the single-table read and from the enumeration, a second
/// delete of the same ARN is refused (the removal is permanent), the bytes it held are
/// released, and every surviving table still reports exactly what its create stored.
#[tokio::test]
async fn each_deleted_lookup_table_is_gone_from_every_read() {
    let logs = Logs::new();
    let seeds = seeds();
    for seed in &seeds {
        logs.ok(CREATE, &create_request(seed)).await;
    }

    // What the enumeration reports while nothing has been deleted yet: every seeded name,
    // which is the baseline each removal is measured against.
    let mut remaining: Vec<String> = seeds.iter().map(|seed| seed.name.to_string()).collect();
    remaining.sort();
    assert_eq!(
        listed_names(&logs).await,
        remaining,
        "case \"the seeded corpus\": every created table is reported before any is deleted"
    );

    for index in DELETE_ORDER {
        let seed = &seeds[index];
        let case = seed.case;
        let arn = lookup_table_arn(REGION, seed.name);
        let bytes_before = stored_bytes(&logs);

        let body = logs.ok(DELETE, &json!({"lookupTableArn": arn})).await;
        let members = match &body {
            Value::Null => 0,
            Value::Object(map) => map.len(),
            other => panic!("case {case:?}: a Unit output is an empty body, got {other}"),
        };
        assert_eq!(
            members, 0,
            "case {case:?}: a Unit output carries no members, got {body}"
        );

        // "Deletes a lookup table permanently": the table is gone from the single-table
        // read, and a second delete of the same ARN names a resource that is no longer
        // there rather than succeeding again.
        let (code, message) = logs.refusal(GET, &json!({"lookupTableArn": arn})).await;
        assert_eq!(
            code, NOT_FOUND,
            "case {case:?}: the deleted table is no longer readable, expected {NOT_FOUND}, \
             got {code} {message:?}"
        );
        let (code, message) = logs.refusal(DELETE, &json!({"lookupTableArn": arn})).await;
        assert_eq!(
            code, NOT_FOUND,
            "case {case:?}: a second delete of the same ARN is refused, expected {NOT_FOUND}, \
             got {code} {message:?}"
        );
        assert_eq!(
            message,
            resource_not_found_message("lookup table"),
            "case {case:?}: the refusal states the resource kind that is missing, got \
             {message:?}"
        );

        // The table the service no longer holds is the one it no longer enumerates, and the
        // only one: the rest of the listing is untouched, in the same order.
        remaining.retain(|name| name != seed.name);
        assert_eq!(
            listed_names(&logs).await,
            remaining,
            "case {case:?}: only the deleted table is gone from the enumeration"
        );

        // The bytes the deleted table's content held stop counting against the account, and
        // nothing else's do.
        let released = seed.table_body.len();
        let bytes_after = stored_bytes(&logs);
        assert_eq!(
            bytes_after,
            bytes_before - released,
            "case {case:?}: the store drops the deleted table's {released} bytes alone, \
             expected {}, got {bytes_after}",
            bytes_before - released
        );

        // Every surviving table still reports its own content verbatim and its own optional
        // members, so the delete removed one record and touched no other.
        for other in seeds
            .iter()
            .filter(|other| remaining.iter().any(|name| name == other.name))
        {
            let read = logs
                .ok(
                    GET,
                    &json!({"lookupTableArn": lookup_table_arn(REGION, other.name)}),
                )
                .await;
            assert_eq!(
                member(&read, "tableBody").as_deref(),
                Some(other.table_body),
                "case {case:?}: the surviving table {:?} keeps its body verbatim, got {read}",
                other.name
            );
            assert_eq!(
                member(&read, "description").as_deref(),
                other.description,
                "case {case:?}: the surviving table {:?} keeps its description, got {read}",
                other.name
            );
            assert_eq!(
                member(&read, "kmsKeyId").as_deref(),
                other.kms_key_id,
                "case {case:?}: the surviving table {:?} keeps its KMS key, got {read}",
                other.name
            );
        }
    }

    assert!(
        listed_names(&logs).await.is_empty(),
        "case \"every table deleted\": the account enumerates no lookup table"
    );
    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let store = state.lock();
    assert!(
        store.lookup_tables.is_empty(),
        "case \"every table deleted\": the store holds no lookup table, got {:?}",
        store.lookup_tables.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        store.lookup_table_bytes(),
        0,
        "case \"every table deleted\": the store retains no lookup-table content"
    );
}

/// A deleted table's NAME is free again ("The name must be unique within your account and
/// Region" holds only of the tables the account still holds), and the content it held is
/// released: a create under the same name after the delete is accepted and the table that
/// comes back is the new content, not the deleted one.
#[tokio::test]
async fn a_deleted_table_frees_its_name_and_the_bytes_it_held() {
    let logs = Logs::new();
    const NAME: &str = "users_cycle";

    // A table that is never deleted, so what the store holds is never zero and a released
    // byte count is a real difference rather than an emptied store.
    const COMPANION: &str = "user,team\nalice,platform\n";
    logs.ok(
        CREATE,
        &json!({"lookupTableName": "companion", "tableBody": COMPANION}),
    )
    .await;

    // The same name re-created with bodies of DIFFERENT sizes, so the bytes released and the
    // content reported back cannot both be the first round's.
    let bodies = [
        "user\nalice\n",
        "user,team,region\nalice,platform,eu-west-2\nbob,data,us-east-1\n",
        "user\n\u{30ab}\u{30eb}\u{30ed}\u{30b9}\n",
        "user,note\nalice,\"platform, data\"\n",
    ];
    let arn = lookup_table_arn(REGION, NAME);

    for (round, body) in bodies.iter().enumerate() {
        let case = format!("round {round} body {body:?}");

        logs.ok(
            CREATE,
            &json!({"lookupTableName": NAME, "tableBody": body}),
        )
        .await;
        let held = stored_bytes(&logs);
        assert_eq!(
            held,
            COMPANION.len() + body.len(),
            "case {case}: the store holds the companion's bytes plus this body's, expected \
             {}, got {held}",
            COMPANION.len() + body.len()
        );
        let read = logs.ok(GET, &json!({"lookupTableArn": arn})).await;
        assert_eq!(
            member(&read, "tableBody").as_deref(),
            Some(*body),
            "case {case}: the re-created table reports THIS round's content, got {read}"
        );

        logs.ok(DELETE, &json!({"lookupTableArn": arn})).await;
        let held = stored_bytes(&logs);
        assert_eq!(
            held,
            COMPANION.len(),
            "case {case}: the delete releases this body's bytes, expected {}, got {held}",
            COMPANION.len()
        );
    }

    // The companion survived every round, content and all.
    let read = logs
        .ok(
            GET,
            &json!({"lookupTableArn": lookup_table_arn(REGION, "companion")}),
        )
        .await;
    assert_eq!(
        member(&read, "tableBody").as_deref(),
        Some(COMPANION),
        "case \"the companion table\": it is untouched by every delete, got {read}"
    );
}

/// Deleting an ARN no lookup table is stored under is a missing resource, not a silent
/// success — and a `lookupTableArn` that is not an ARN at all is refused for its form.
/// Neither removes anything.
#[tokio::test]
async fn deleting_a_table_the_account_does_not_hold_is_refused() {
    let logs = Logs::new();
    const NAME: &str = "users_held";
    const BODY: &str = "user,team\nalice,platform\n";
    logs.ok(
        CREATE,
        &json!({"lookupTableName": NAME, "tableBody": BODY}),
    )
    .await;

    // (case, the ARN deleted, the code it answers, the message it carries)
    let cases: [(&str, String, &str, String); 6] = [
        (
            "a value that is not an ARN",
            NAME.to_string(),
            INVALID_PARAMETER,
            unparsable_arn_message("lookupTableArn", NAME).expect("a name is not an ARN"),
        ),
        (
            "a lookup-table ARN for a name nothing created",
            lookup_table_arn(REGION, "users_never_created"),
            NOT_FOUND,
            resource_not_found_message("lookup table"),
        ),
        (
            "this table's name in ANOTHER Region, which this account does not hold here",
            lookup_table_arn("us-east-1", NAME),
            NOT_FOUND,
            resource_not_found_message("lookup table"),
        ),
        (
            "this table's name in ANOTHER account",
            format!("arn:aws:logs:{REGION}:111122223333:lookup-table:{NAME}"),
            NOT_FOUND,
            resource_not_found_message("lookup table"),
        ),
        (
            "a well-formed ARN of another resource kind",
            log_group_arn(REGION, NAME, false),
            NOT_FOUND,
            resource_not_found_message("lookup table"),
        ),
        (
            "an ARN of another service entirely",
            "arn:aws:s3:::a-bucket".to_string(),
            NOT_FOUND,
            resource_not_found_message("lookup table"),
        ),
    ];

    for (case, arn, expected_code, expected_message) in cases {
        let (code, message) = logs.refusal(DELETE, &json!({"lookupTableArn": arn})).await;
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected {expected_code}, got {code} {message:?}"
        );
        assert_eq!(
            message, expected_message,
            "case {case:?}: expected the message {expected_message:?}, got {message:?}"
        );
    }

    // The table the account DOES hold is still there, content and all: no refused delete
    // removed anything.
    let read = logs
        .ok(
            GET,
            &json!({"lookupTableArn": lookup_table_arn(REGION, NAME)}),
        )
        .await;
    assert_eq!(
        member(&read, "tableBody").as_deref(),
        Some(BODY),
        "a refused delete removes nothing, got {read}"
    );
    assert_eq!(
        listed_names(&logs).await,
        vec![NAME.to_string()],
        "a refused delete leaves the account's enumeration as it was"
    );
}
