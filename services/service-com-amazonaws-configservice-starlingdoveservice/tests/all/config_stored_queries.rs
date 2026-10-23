// SPDX-License-Identifier: CC0-1.0

//! The stored-query write-and-read path, end to end over the wire.
//!
//! Four rules are pinned here, each over a table of inputs rather than one case:
//!
//! 1. a stored query is read back exactly as it was saved — the text of every member
//!    the caller supplies survives byte for byte, whatever script, emoji, combining
//!    marks or whitespace it carries, up to each published length bound;
//! 2. the service, not the caller, assigns a stored query its identity: a `QueryId`
//!    distinct per query and a `QueryArn` of the form `QueryArn`'s pattern requires,
//!    in the Region the request is signed for;
//! 3. saving under a name the account already holds UPDATES that query in place: same
//!    id and ARN, the new expression, and no second resource;
//! 4. a request that breaks the operation's contract is refused with the modelled
//!    error and persists nothing, and a name nobody saved is not found.
//!
//! The harness (`Emulator`) is the one `config_setup_and_rule_loop` already builds;
//! every case drives the emulator the way a client does, through the
//! `MultiServiceRouter`.

use serde_json::{json, Value};

use crate::config_setup_and_rule_loop::Emulator;

/// The published length bounds of the members `PutStoredQuery` persists, from
/// `smithy_query(query="shape", shape="StoredQuery")` on this service's model:
/// `Expression` (QueryExpression) is 1..4096, `Description` (QueryDescription) is
/// 0..256 and `QueryName` is 1..64.
const EXPRESSION_MAX: usize = 4096;
/// See [`EXPRESSION_MAX`].
const DESCRIPTION_MAX: usize = 256;
/// See [`EXPRESSION_MAX`].
const QUERY_NAME_MAX: usize = 64;

/// The stored-query quota for one account in one Region (`PutStoredQuery`: "You can
/// create upto 300 queries in a single Amazon Web Services account and a single Amazon
/// Web Services Region").
const STORED_QUERIES_QUOTA: usize = 300;

/// A `PutStoredQuery` body for `name`, carrying `expression` and `description` only
/// when one is given — an absent member is absent from the request, not an empty one.
fn put_body(name: &str, description: Option<&str>, expression: Option<&str>) -> Value {
    let mut query = json!({ "QueryName": name });
    if let Some(description) = description {
        query["Description"] = json!(description);
    }
    if let Some(expression) = expression {
        query["Expression"] = json!(expression);
    }
    json!({ "StoredQuery": query })
}

/// A minimal query expression; its text is never asserted on, only read back.
const SELECT_BUCKETS: &str =
    "SELECT resourceId, resourceType WHERE resourceType = 'AWS::S3::Bucket'";

/// Save `name` and return the `QueryArn` the service answered with.
async fn save(emulator: &Emulator, name: &str, expression: &str) -> String {
    let put = emulator
        .ok("PutStoredQuery", &put_body(name, None, Some(expression)))
        .await;
    put["QueryArn"]
        .as_str()
        .unwrap_or_else(|| panic!("PutStoredQuery must answer a QueryArn, got {put}"))
        .to_string()
}

/// The `StoredQuery` `GetStoredQuery` reports for `name`.
async fn read_back(emulator: &Emulator, name: &str) -> Value {
    let got = emulator
        .ok("GetStoredQuery", &json!({ "QueryName": name }))
        .await;
    got.get("StoredQuery")
        .cloned()
        .unwrap_or_else(|| panic!("GetStoredQuery must answer a StoredQuery, got {got}"))
}

/// Check `arn` against the form `QueryArn`'s pattern requires:
/// `arn:aws:config:<region>:<account-id>:stored-query/<name>/query-<id>`.
fn assert_query_arn(case: &str, arn: &str, region: &str, name: &str, query_id: &str) {
    let prefix = format!("arn:aws:config:{region}:");
    assert!(
        arn.starts_with(&prefix),
        "case {case:?}: expected a query ARN starting {prefix:?}, got {arn:?}"
    );
    let suffix = format!(":stored-query/{name}/query-{query_id}");
    assert!(
        arn.ends_with(&suffix),
        "case {case:?}: expected a query ARN ending {suffix:?}, got {arn:?}"
    );
    let account = arn.split(':').nth(4).unwrap_or_default();
    assert!(
        account.len() == 12 && account.chars().all(|digit| digit.is_ascii_digit()),
        "case {case:?}: expected a 12-digit account id in the query ARN, got {account:?} \
         in {arn:?}"
    );
}

#[tokio::test]
async fn a_stored_query_is_read_back_exactly_as_it_was_saved() {
    // Rows differ along the TEXT of the two members a stored query carries: script,
    // emoji (including a ZWJ sequence), combining marks, embedded whitespace and
    // newlines, and each published length bound of `Expression` and `Description`.
    let longest_name = "n".repeat(QUERY_NAME_MAX);
    let rows: Vec<(&str, &str, Option<String>, String)> = vec![
        (
            "plain ASCII",
            "ascii-query",
            Some("all S3 buckets".to_string()),
            SELECT_BUCKETS.to_string(),
        ),
        (
            "no description at all",
            "no-description",
            None,
            SELECT_BUCKETS.to_string(),
        ),
        (
            "an empty description, the published minimum length",
            "empty-description",
            Some(String::new()),
            SELECT_BUCKETS.to_string(),
        ),
        (
            "non-Latin scripts in both members",
            "unicode-query",
            Some("ресурсы 資源 مصادر".to_string()),
            format!("{SELECT_BUCKETS} /* बकेट สำรวจ */"),
        ),
        (
            "emoji, including a ZWJ sequence and a flag",
            "emoji-query",
            Some("buckets 🪣👨‍👩‍👧‍👦🇯🇵".to_string()),
            format!("{SELECT_BUCKETS} /* 🚀 */"),
        ),
        (
            "combining marks and an astral-plane character",
            "combining-query",
            Some("e\u{0301}gal 𝄞".to_string()),
            format!("{SELECT_BUCKETS} /* a\u{0308}\u{0331} */"),
        ),
        (
            "newlines and tabs the pattern allows",
            "whitespace-query",
            Some("line one\nline two\tindented".to_string()),
            "SELECT\n\tresourceId\nWHERE\n\tresourceType = 'AWS::EC2::Volume'".to_string(),
        ),
        (
            "an expression of one character, the published minimum length",
            "shortest-expression",
            None,
            "*".to_string(),
        ),
        (
            "both members at their published maximum length",
            longest_name.as_str(),
            Some("d".repeat(DESCRIPTION_MAX)),
            "e".repeat(EXPRESSION_MAX),
        ),
        (
            "a multi-byte expression at the published maximum length in characters",
            "wide-expression",
            Some("キー".repeat(DESCRIPTION_MAX / 2)),
            "✓".repeat(EXPRESSION_MAX),
        ),
    ];

    for (case, name, description, expression) in rows {
        let emulator = Emulator::new();
        let arn = emulator
            .ok(
                "PutStoredQuery",
                &put_body(name, description.as_deref(), Some(&expression)),
            )
            .await;
        let arn = arn["QueryArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a QueryArn, got {arn}"))
            .to_string();

        let stored = read_back(&emulator, name).await;
        assert_eq!(
            stored["QueryName"].as_str(),
            Some(name),
            "case {case:?}: expected QueryName {name:?}, got {stored}"
        );
        assert_eq!(
            stored["Expression"].as_str(),
            Some(expression.as_str()),
            "case {case:?}: expected the expression to round-trip unchanged, got {:?}",
            stored["Expression"]
        );
        match description.as_deref() {
            Some(expected) => assert_eq!(
                stored["Description"].as_str(),
                Some(expected),
                "case {case:?}: expected the description to round-trip unchanged, got {:?}",
                stored["Description"]
            ),
            None => assert!(
                stored.get("Description").is_none(),
                "case {case:?}: expected no Description for a query saved without one, got {:?}",
                stored["Description"]
            ),
        }
        assert_eq!(
            stored["QueryArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected the read to report the ARN the save answered \
             ({arn:?}), got {:?}",
            stored["QueryArn"]
        );

        // A read reports state and changes nothing: the second read is the first.
        let again = read_back(&emulator, name).await;
        assert_eq!(
            again, stored,
            "case {case:?}: reading a stored query twice must report the same query, \
             got {again} then {stored}"
        );
    }
}

#[tokio::test]
async fn the_service_assigns_each_stored_query_its_id_and_arn() {
    // Rows differ along the Region the request is signed for, because the ARN carries
    // it; within each Region several queries are saved, so the ids can be compared.
    for region in ["us-east-1", "eu-west-3", "ap-southeast-2"] {
        let emulator = Emulator::in_region(region);
        let names = ["first-query", "second_query", "THIRD-3"];
        let mut identities: Vec<(String, String)> = Vec::new();

        for name in names {
            let case = format!("{region}/{name}");
            let arn = save(&emulator, name, SELECT_BUCKETS).await;
            let stored = read_back(&emulator, name).await;
            let query_id = stored["QueryId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a QueryId, got {stored}"))
                .to_string();

            assert!(
                !query_id.is_empty()
                    && query_id.len() <= 36
                    && !query_id.chars().any(char::is_whitespace),
                "case {case:?}: a QueryId is 1..36 non-whitespace characters, got \
                 {query_id:?}"
            );
            // A caller must not be able to pass an id or an ARN off as its own.
            assert_query_arn(&case, &arn, region, name, &query_id);
            assert_eq!(
                stored["QueryArn"].as_str(),
                Some(arn.as_str()),
                "case {case:?}: expected the saved ARN {arn:?}, got {:?}",
                stored["QueryArn"]
            );
            identities.push((query_id, arn));
        }

        let mut ids: Vec<&str> = identities.iter().map(|(id, _)| id.as_str()).collect();
        let saved = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(
            ids.len(),
            saved,
            "case {region:?}: every stored query must get a distinct QueryId, got {} \
             distinct of {saved}: {identities:?}",
            ids.len()
        );
        let mut arns: Vec<&str> = identities.iter().map(|(_, arn)| arn.as_str()).collect();
        arns.sort_unstable();
        arns.dedup();
        assert_eq!(
            arns.len(),
            saved,
            "case {region:?}: every stored query must get a distinct QueryArn, got {} \
             distinct of {saved}: {identities:?}",
            arns.len()
        );
    }
}

#[tokio::test]
async fn saving_under_a_name_the_account_holds_updates_that_query_in_place() {
    // Rows differ along what the second save carries: a new expression alone, a new
    // expression and a new description, and the very same body again (the idempotent
    // case the operation documents).
    struct Row {
        case: &'static str,
        description: Option<&'static str>,
        expression: &'static str,
        /// The description the query must carry after the second save.
        expected_description: Option<&'static str>,
    }

    let first_description = "the original description";
    let rows = [
        Row {
            case: "a new expression, no description in the update",
            description: None,
            expression: "SELECT resourceId WHERE resourceType = 'AWS::EC2::Instance'",
            // "When you are updating a query, you must provide a query name but
            // updating the description is optional": an update that carries none
            // leaves the stored description as it was.
            expected_description: Some(first_description),
        },
        Row {
            case: "a new expression and a new description",
            description: Some("the replacement description 🔁"),
            expression: "SELECT resourceId, awsRegion WHERE resourceType = 'AWS::IAM::Role'",
            expected_description: Some("the replacement description 🔁"),
        },
        Row {
            case: "the same body again",
            description: Some(first_description),
            expression: SELECT_BUCKETS,
            expected_description: Some(first_description),
        },
        Row {
            case: "an emptied description",
            description: Some(""),
            expression: SELECT_BUCKETS,
            expected_description: Some(""),
        },
    ];

    for row in rows {
        let emulator = Emulator::new();
        let name = "query-under-update";
        let created = emulator
            .ok(
                "PutStoredQuery",
                &put_body(name, Some(first_description), Some(SELECT_BUCKETS)),
            )
            .await;
        let created_arn = created["QueryArn"]
            .as_str()
            .expect("the create answers a QueryArn")
            .to_string();
        let created_id = read_back(&emulator, name).await["QueryId"]
            .as_str()
            .expect("the create assigns a QueryId")
            .to_string();

        let updated = emulator
            .ok(
                "PutStoredQuery",
                &put_body(name, row.description, Some(row.expression)),
            )
            .await;
        assert_eq!(
            updated["QueryArn"].as_str(),
            Some(created_arn.as_str()),
            "case {:?}: an update must answer the ARN the query was created with \
             ({created_arn:?}), got {:?}",
            row.case,
            updated["QueryArn"]
        );

        let stored = read_back(&emulator, name).await;
        assert_eq!(
            stored["QueryId"].as_str(),
            Some(created_id.as_str()),
            "case {:?}: an update must keep the QueryId {created_id:?}, got {:?}",
            row.case,
            stored["QueryId"]
        );
        assert_eq!(
            stored["Expression"].as_str(),
            Some(row.expression),
            "case {:?}: expected the updated expression, got {:?}",
            row.case,
            stored["Expression"]
        );
        assert_eq!(
            stored["Description"].as_str(),
            row.expected_description,
            "case {:?}: expected the description {:?}, got {:?}",
            row.case,
            row.expected_description,
            stored["Description"]
        );
    }
}

#[tokio::test]
async fn a_request_outside_the_contract_is_refused_and_persists_nothing() {
    // Rows differ along WHICH part of the contract the request breaks: a missing
    // mandatory member, each published length bound of the name and the expression
    // overshot by one, a name outside the published pattern, and a read of a name
    // nobody saved.
    struct Refusal {
        case: &'static str,
        operation: &'static str,
        body: Value,
        expected: &'static str,
    }

    let rows = vec![
        Refusal {
            case: "no expression, which is a mandatory field",
            operation: "PutStoredQuery",
            body: put_body("no-expression", Some("a description"), None),
            expected: "ValidationException",
        },
        Refusal {
            case: "an empty expression, one short of the published minimum length",
            operation: "PutStoredQuery",
            body: put_body("empty-expression", None, Some("")),
            expected: "ValidationException",
        },
        Refusal {
            case: "an expression one character past the published maximum length",
            operation: "PutStoredQuery",
            body: put_body(
                "long-expression",
                None,
                Some(&"e".repeat(EXPRESSION_MAX + 1)),
            ),
            expected: "ValidationException",
        },
        Refusal {
            case: "a description one character past the published maximum length",
            operation: "PutStoredQuery",
            body: put_body(
                "long-description",
                Some(&"d".repeat(DESCRIPTION_MAX + 1)),
                Some(SELECT_BUCKETS),
            ),
            expected: "ValidationException",
        },
        Refusal {
            case: "a name one character past the published maximum length",
            operation: "PutStoredQuery",
            body: put_body(&"n".repeat(QUERY_NAME_MAX + 1), None, Some(SELECT_BUCKETS)),
            expected: "ValidationException",
        },
        Refusal {
            case: "a name outside the published pattern",
            operation: "PutStoredQuery",
            body: put_body("not a valid name", None, Some(SELECT_BUCKETS)),
            expected: "ValidationException",
        },
        Refusal {
            case: "reading a query nobody saved",
            operation: "GetStoredQuery",
            body: json!({ "QueryName": "never-saved" }),
            expected: "ResourceNotFoundException",
        },
    ];

    for row in rows {
        let emulator = Emulator::new();
        // A query that IS saved must be untouched by the refusal beside it.
        let witness_arn = save(&emulator, "witness-query", SELECT_BUCKETS).await;

        let (status, code) = emulator.refused(row.operation, &row.body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, row.expected),
            "case {:?}: expected HTTP 400 {}, got {status} {code}",
            row.case,
            row.expected,
        );

        let name = row.body["StoredQuery"]["QueryName"]
            .as_str()
            .or_else(|| row.body["QueryName"].as_str())
            .unwrap_or_default();
        // A name the model itself refuses cannot be read back at all, so only a name
        // inside `QueryName`'s published length and pattern is checked for absence.
        let readable_name = !name.is_empty()
            && name.chars().count() <= QUERY_NAME_MAX
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if readable_name {
            let (status, code) = emulator
                .refused("GetStoredQuery", &json!({ "QueryName": name }))
                .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ResourceNotFoundException"),
                "case {:?}: a refused save must persist nothing, but reading {name:?} \
                 answered {status} {code}",
                row.case,
            );
        }

        let witness = read_back(&emulator, "witness-query").await;
        assert_eq!(
            witness["QueryArn"].as_str(),
            Some(witness_arn.as_str()),
            "case {:?}: the query saved before the refusal must be unchanged, got \
             {witness}",
            row.case,
        );
    }
}

#[tokio::test]
async fn the_stored_query_store_is_capped_at_this_regions_quota() {
    let emulator = Emulator::new();
    for index in 0..STORED_QUERIES_QUOTA {
        emulator
            .ok(
                "PutStoredQuery",
                &put_body(&format!("query-{index:03}"), None, Some(SELECT_BUCKETS)),
            )
            .await;
    }

    let (status, code) = emulator
        .refused(
            "PutStoredQuery",
            &put_body("one-too-many", None, Some(SELECT_BUCKETS)),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "case \"one past the quota\": expected HTTP 400 ValidationException, got \
         {status} {code}"
    );

    // An update to a query that already exists is not a new query, so the quota does
    // not refuse it.
    let updated = emulator
        .ok(
            "PutStoredQuery",
            &put_body("query-000", None, Some("SELECT resourceId")),
        )
        .await;
    let updated_arn = updated["QueryArn"].as_str().unwrap_or_default();
    assert!(
        updated_arn.contains(":stored-query/query-000/query-"),
        "case \"update at the quota\": expected the existing query's ARN, got {updated}"
    );
    let stored = read_back(&emulator, "query-000").await;
    assert_eq!(
        stored["Expression"].as_str(),
        Some("SELECT resourceId"),
        "case \"update at the quota\": expected the updated expression, got {stored}"
    );
}

#[tokio::test]
async fn the_tags_a_save_carries_are_the_querys_tag_set() {
    // Rows differ along the tag VALUE a save carries: ASCII, non-Latin scripts, emoji,
    // an empty value and one at the published maximum — `Tag$Value` length is 0..256
    // from `smithy_query(query="shape", shape="Tag")`.
    let rows: Vec<(&str, &str)> = vec![
        ("plain ASCII", "platform"),
        ("non-Latin scripts", "платформа 平台"),
        ("emoji", "team 🧑‍🚀🚀"),
        ("an empty value, the published minimum length", ""),
        ("combining marks", "cafe\u{0301}"),
    ];
    let longest_value = "v".repeat(256);

    for (case, value) in rows
        .into_iter()
        .chain([("a value at the published maximum length", longest_value.as_str())])
    {
        let emulator = Emulator::new();
        let name = "tagged-query";
        let created = emulator
            .ok(
                "PutStoredQuery",
                &json!({
                    "StoredQuery": { "QueryName": name, "Expression": SELECT_BUCKETS },
                    "Tags": [{ "Key": "owner", "Value": value }],
                }),
            )
            .await;
        let arn = created["QueryArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a QueryArn, got {created}"))
            .to_string();

        // The tags a save carries are this resource's tag set, so the service's own tag
        // read path reports them.
        let listed = emulator
            .ok("ListTagsForResource", &json!({ "ResourceArn": arn }))
            .await;
        assert_eq!(
            listed["Tags"],
            json!([{ "Key": "owner", "Value": value }]),
            "case {case:?}: expected the save's tags to be reported for {arn:?}, got \
             {listed}"
        );

        // "Tags are added at creation and cannot be updated ... tags will not be
        // updated, even if they are different": a later save of the same query leaves
        // the tag set alone.
        emulator
            .ok(
                "PutStoredQuery",
                &json!({
                    "StoredQuery": { "QueryName": name, "Expression": "SELECT resourceId" },
                    "Tags": [{ "Key": "owner", "Value": "replaced" }],
                }),
            )
            .await;
        let after = emulator
            .ok("ListTagsForResource", &json!({ "ResourceArn": arn }))
            .await;
        assert_eq!(
            after["Tags"],
            json!([{ "Key": "owner", "Value": value }]),
            "case {case:?}: an update must not change the query's tags, got {after}"
        );
    }
}

