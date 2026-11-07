// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateDataQualityRuleset`: the patch a client applies to the
//! ruleset `CreateDataQualityRuleset` registered, read back through
//! `GetDataQualityRuleset`.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.
//!
//! The patch suite is a TABLE: each row is one update along the axes the rule generalises
//! over (which of the two optional members the request names, the form and size of the DQDL
//! text and of the description, and which members the stored ruleset carries that the
//! request has no member for). The description-plus-ruleset update a lifecycle client sends
//! is one row among them.

use serde_json::{json, Value};

use crate::wire::{call, emulator, error_code, error_message, ok};

/// The longest `Ruleset` the model accepts: "len 1..65536" on `DataQualityRulesetString`
/// (`smithy_query operation=UpdateDataQualityRuleset`). The shortest text a stored ruleset
/// can carry is [`SHORTEST_STORED_RULESET`].
const MAX_RULESET_LENGTH: usize = 65_536;

/// The shortest `Ruleset` a write can carry: the model's `length 1..65536` admits one byte,
/// but the service also requires the text to declare a rule (see
/// [`crate::data_quality_ruleset_parsing`]), and the shortest DQDL that declares one is a
/// rule list holding a single entry — so this is the shortest text a stored ruleset can have.
const SHORTEST_STORED_RULESET: &str = "Rules=[x]";

/// The longest `Description` the model accepts: "len 0..2048" on `DescriptionString`
/// (`smithy_query operation=UpdateDataQualityRuleset`). Its lower bound is zero, so the
/// empty string is a description a request may name.
const MAX_DESCRIPTION_LENGTH: usize = 2_048;

/// The members `UpdateDataQualityRuleset` answers with: the three the published response
/// shape binds (`smithy_query operation=UpdateDataQualityRuleset`) plus the four the service
/// also reports from the stored record — see [`crate::response_members`], which asserts that
/// projection.
const UPDATE_RESPONSE_MEMBERS: [&str; 7] = [
    "Name",
    "Description",
    "Ruleset",
    "Identifier",
    "TargetTable",
    "CreatedOn",
    "LastModifiedOn",
];

/// Create a ruleset, and fail the test if the create is refused.
async fn create(router: &smithy_server_runtime::MultiServiceRouter, body: &Value) {
    ok(router, "CreateDataQualityRuleset", body).await;
}

/// The `GetDataQualityRuleset` response for `name`.
async fn get(router: &smithy_server_runtime::MultiServiceRouter, name: &str) -> Value {
    ok(router, "GetDataQualityRuleset", &json!({ "Name": name })).await
}

/// The `UpdateDataQualityRuleset` response for one patch.
async fn update(router: &smithy_server_runtime::MultiServiceRouter, body: &Value) -> Value {
    ok(router, "UpdateDataQualityRuleset", body).await
}

/// A DQDL ruleset whose text is padded out to `length` bytes, so a row can sit exactly on
/// a modelled bound. The padding is a comment, so the result is still one ruleset rather
/// than a string of the right size.
fn ruleset_of_length(length: usize) -> String {
    let head = "Rules = [ ColumnExists \"id\" ] # ";
    if length <= head.len() {
        return "x".repeat(length);
    }
    format!("{head}{}", "x".repeat(length - head.len()))
}

/// The timestamp member `name` of a ruleset projection, as seconds since the epoch.
fn instant(case: &str, member: &str, body: &Value) -> f64 {
    body[member]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case}: {member} must be a timestamp, got {body}"))
}

// ---------------------------------------------------------------------------
// The patch
// ---------------------------------------------------------------------------

/// An update writes the members it names onto the stored ruleset, preserves the members it
/// omits, and answers with the record as it now stands — which is what a later
/// `GetDataQualityRuleset` of that name reports. The members the request has no member for
/// at all (the target table, the security configuration, the creation instant) are left
/// exactly as the create wrote them.
#[tokio::test]
async fn an_update_writes_the_members_it_names_and_preserves_the_rest() {
    let router = emulator();

    let longest_ruleset = ruleset_of_length(MAX_RULESET_LENGTH);
    let shortest_ruleset = SHORTEST_STORED_RULESET.to_string();
    let longest_description = "d".repeat(MAX_DESCRIPTION_LENGTH);

    // (case, the create, the patch, the Description expected afterwards, the Ruleset
    // expected afterwards)
    let cases: Vec<(&str, Value, Value, Value, Value)> = vec![
        (
            // The shape a lifecycle client sends: both optional members at once.
            "both optional members named",
            json!({
                "Name": "orders-ruleset",
                "Ruleset": "Rules = [ ColumnExists \"order_id\" ]",
                "Description": "the first description",
            }),
            json!({
                "Name": "orders-ruleset",
                "Ruleset": "Rules = [ ColumnExists \"order_id\", RowCount > 0 ]",
                "Description": "the second description",
            }),
            json!("the second description"),
            json!("Rules = [ ColumnExists \"order_id\", RowCount > 0 ]"),
        ),
        (
            "only the Ruleset named, over a ruleset that carries a description",
            json!({
                "Name": "ruleset-only",
                "Ruleset": "Rules = [ ColumnExists \"a\" ]",
                "Description": "kept across the update",
            }),
            json!({
                "Name": "ruleset-only",
                "Ruleset": "Rules = [ IsComplete \"a\" ]",
            }),
            json!("kept across the update"),
            json!("Rules = [ IsComplete \"a\" ]"),
        ),
        (
            "only the Description named, over a ruleset that carries none",
            json!({
                "Name": "description-only",
                "Ruleset": "Rules = [ ColumnExists \"b\" ]",
            }),
            json!({
                "Name": "description-only",
                "Description": "added by the update",
            }),
            json!("added by the update"),
            json!("Rules = [ ColumnExists \"b\" ]"),
        ),
        (
            // `DescriptionString` has `length 0..2048`, so the empty string is a
            // description a request may name, and it is not an omission.
            "an empty Description at the model's lower bound",
            json!({
                "Name": "empty-description",
                "Ruleset": "Rules = [ ColumnExists \"c\" ]",
                "Description": "replaced by the empty string",
            }),
            json!({
                "Name": "empty-description",
                "Description": "",
            }),
            json!(""),
            json!("Rules = [ ColumnExists \"c\" ]"),
        ),
        (
            "a Description at the model's upper bound",
            json!({
                "Name": "longest-description",
                "Ruleset": "Rules = [ ColumnExists \"d\" ]",
            }),
            json!({
                "Name": "longest-description",
                "Description": longest_description.clone(),
            }),
            json!(longest_description),
            json!("Rules = [ ColumnExists \"d\" ]"),
        ),
        (
            "a Ruleset at the model's upper bound",
            json!({
                "Name": "longest-ruleset",
                "Ruleset": "Rules = [ ColumnExists \"e\" ]",
            }),
            json!({
                "Name": "longest-ruleset",
                "Ruleset": longest_ruleset.clone(),
            }),
            Value::Null,
            json!(longest_ruleset),
        ),
        (
            "the shortest Ruleset a stored ruleset can carry",
            json!({
                "Name": "shortest-ruleset",
                "Ruleset": "Rules = [ ColumnExists \"f\" ]",
            }),
            json!({
                "Name": "shortest-ruleset",
                "Ruleset": shortest_ruleset.clone(),
            }),
            Value::Null,
            json!(shortest_ruleset),
        ),
        (
            "a multi-line Ruleset carrying several rules",
            json!({
                "Name": "multi-line-ruleset",
                "Ruleset": "Rules = [ ColumnExists \"g\" ]",
            }),
            json!({
                "Name": "multi-line-ruleset",
                "Ruleset": "Rules = [\n    ColumnExists \"g\",\n    Completeness \"g\" > 0.95,\n    Uniqueness \"g\" > 0.9\n]",
                "Description": "several rules over one column",
            }),
            json!("several rules over one column"),
            json!("Rules = [\n    ColumnExists \"g\",\n    Completeness \"g\" > 0.95,\n    Uniqueness \"g\" > 0.9\n]"),
        ),
        (
            // The patch has no member for a target table, a security configuration or the
            // create's tags, so none of them is this request's to change.
            "a ruleset carrying a target table, a security configuration and tags",
            json!({
                "Name": "fully-specified",
                "Ruleset": "Rules = [ ColumnExists \"h\" ]",
                "Description": "the first description",
                "TargetTable": {
                    "DatabaseName": "sales",
                    "TableName": "customers",
                    "CatalogId": crate::wire::ACCOUNT_ID,
                },
                "DataQualitySecurityConfiguration": "dq-security-config",
                "Tags": { "owner": "data-governance" },
            }),
            json!({
                "Name": "fully-specified",
                "Ruleset": "Rules = [ ColumnExists \"h\", RowCount > 1 ]",
            }),
            json!("the first description"),
            json!("Rules = [ ColumnExists \"h\", RowCount > 1 ]"),
        ),
        (
            // `NameString`'s pattern admits a space and a tab, so a name carrying them
            // addresses its ruleset like any other.
            "a name carrying spaces and a tab",
            json!({
                "Name": "ruleset one\ttwo",
                "Ruleset": "Rules = [ ColumnExists \"i\" ]",
            }),
            json!({
                "Name": "ruleset one\ttwo",
                "Ruleset": "Rules = [ ColumnExists \"i\", RowCount > 2 ]",
            }),
            Value::Null,
            json!("Rules = [ ColumnExists \"i\", RowCount > 2 ]"),
        ),
    ];

    for (case, create_body, patch, expected_description, expected_ruleset) in &cases {
        let name = create_body["Name"].as_str().expect("the case carries a Name");
        create(&router, create_body).await;
        let before = get(&router, name).await;
        let created_on = instant(case, "CreatedOn", &before);

        let answered = update(&router, patch).await;

        // The response projects the record as it now stands: the name it addressed, and
        // the two members after the patch has been applied.
        assert_eq!(
            answered.get("Name").and_then(Value::as_str),
            Some(name),
            "case {case}: the update must report the ruleset it addressed, got {answered}"
        );
        assert_eq!(
            answered.get("Ruleset"),
            Some(expected_ruleset),
            "case {case}: expected the Ruleset the record now holds, got {answered}"
        );
        if expected_description.is_null() {
            assert!(
                answered.get("Description").is_none_or(Value::is_null),
                "case {case}: a ruleset no write gave a description reports none, got {answered}"
            );
        } else {
            assert_eq!(
                answered.get("Description"),
                Some(expected_description),
                "case {case}: expected the Description the record now holds, got {answered}"
            );
        }
        // The answer carries the members of the ruleset projection and nothing else.
        for member in answered
            .as_object()
            .unwrap_or_else(|| panic!("case {case}: the answer is a structure, got {answered}"))
            .keys()
        {
            assert!(
                UPDATE_RESPONSE_MEMBERS.contains(&member.as_str()),
                "case {case}: the answer must carry only {UPDATE_RESPONSE_MEMBERS:?}, got \
                 {member} in {answered}"
            );
        }

        // The write landed in the store, so the read reports the same patched ruleset the
        // update answered with.
        let after = get(&router, name).await;
        for member in UPDATE_RESPONSE_MEMBERS {
            if member == "Identifier" {
                // `GetDataQualityRulesetResponse` binds no `Identifier`, so the read has
                // none to agree with.
                continue;
            }
            assert_eq!(
                after.get(member).cloned().unwrap_or(Value::Null),
                answered.get(member).cloned().unwrap_or(Value::Null),
                "case {case}: the read must report the {member} the update answered; update \
                 gave {answered}, read gave {after}"
            );
        }

        // The members this request has NO member for are not its to change.
        for member in [
            "TargetTable",
            "DataQualitySecurityConfiguration",
            "RecommendationRunId",
            "CreatedOn",
        ] {
            assert_eq!(
                after.get(member).cloned().unwrap_or(Value::Null),
                before.get(member).cloned().unwrap_or(Value::Null),
                "case {case}: an update must leave {member} as it was; expected {before}, got \
                 {after}"
            );
        }

        // The write is what moves "the last point in time when this data quality ruleset
        // was modified", which was the creation instant until now.
        let last_modified = instant(case, "LastModifiedOn", &after);
        assert!(
            last_modified > created_on,
            "case {case}: expected the update's instant to be later than the creation instant \
             {created_on}, got {last_modified}"
        );
    }

    // Each ruleset is its own record: after every update has landed, each one still reports
    // its own patched DQDL, so one update never wrote over another ruleset.
    for (case, create_body, _, _, expected_ruleset) in &cases {
        let name = create_body["Name"].as_str().expect("the case carries a Name");
        let read = get(&router, name).await;
        assert_eq!(
            read.get("Ruleset"),
            Some(expected_ruleset),
            "case {case}: each ruleset keeps its own patched Ruleset, got {read}"
        );
    }
}

/// Updates compose: each one patches the record the one before it left, so the ruleset a
/// read reports is the one the LAST write that named each member wrote. The rows differ
/// along which member each successive update names.
#[tokio::test]
async fn successive_updates_each_patch_what_the_last_one_left() {
    let router = emulator();

    create(
        &router,
        &json!({
            "Name": "composed",
            "Ruleset": "Rules = [ ColumnExists \"a\" ]",
            "Description": "the create's description",
        }),
    )
    .await;

    // (case, the patch, the Description expected afterwards, the Ruleset expected
    // afterwards)
    let steps: Vec<(&str, Value, &str, &str)> = vec![
        (
            "the Ruleset alone",
            json!({ "Name": "composed", "Ruleset": "Rules = [ RowCount > 0 ]" }),
            "the create's description",
            "Rules = [ RowCount > 0 ]",
        ),
        (
            "the Description alone",
            json!({ "Name": "composed", "Description": "the second description" }),
            "the second description",
            "Rules = [ RowCount > 0 ]",
        ),
        (
            "both members at once",
            json!({
                "Name": "composed",
                "Description": "the third description",
                "Ruleset": "Rules = [ RowCount > 1 ]",
            }),
            "the third description",
            "Rules = [ RowCount > 1 ]",
        ),
        (
            "the Ruleset alone again",
            json!({ "Name": "composed", "Ruleset": "Rules = [ RowCount > 2 ]" }),
            "the third description",
            "Rules = [ RowCount > 2 ]",
        ),
    ];

    let mut previous_instant = instant(
        "before any update",
        "LastModifiedOn",
        &get(&router, "composed").await,
    );

    for (case, patch, description, ruleset) in &steps {
        update(&router, patch).await;

        let read = get(&router, "composed").await;
        assert_eq!(
            read.get("Description").and_then(Value::as_str),
            Some(*description),
            "case {case}: expected the Description the last write to name it left, got {read}"
        );
        assert_eq!(
            read.get("Ruleset").and_then(Value::as_str),
            Some(*ruleset),
            "case {case}: expected the Ruleset the last write to name it left, got {read}"
        );

        let last_modified = instant(case, "LastModifiedOn", &read);
        assert!(
            last_modified > previous_instant,
            "case {case}: each write moves LastModifiedOn forward; expected later than \
             {previous_instant}, got {last_modified}"
        );
        previous_instant = last_modified;
    }
}

/// Both members of the request other than the `Name` that addresses the ruleset are
/// optional, so a request naming neither asks for no change: nothing is written, the
/// ruleset keeps the instant its last real write put on it, and the answer still reports
/// the stored record. The rows differ along which members the stored ruleset carries.
#[tokio::test]
async fn an_update_naming_no_member_to_change_writes_nothing() {
    let router = emulator();

    for (case, create_body) in [
        (
            "a ruleset carrying only the required members",
            json!({ "Name": "untouched-bare", "Ruleset": "Rules = [ ColumnExists \"a\" ]" }),
        ),
        (
            "a ruleset carrying every optional member",
            json!({
                "Name": "untouched-full",
                "Ruleset": "Rules = [ ColumnExists \"b\" ]",
                "Description": "untouched",
                "TargetTable": { "DatabaseName": "db", "TableName": "t" },
                "DataQualitySecurityConfiguration": "dq-security-config",
                "Tags": { "owner": "data-governance" },
            }),
        ),
    ] {
        let name = create_body["Name"].as_str().expect("the case carries a Name");
        create(&router, &create_body).await;
        let before = get(&router, name).await;

        let answered = update(&router, &json!({ "Name": name })).await;
        assert_eq!(
            answered.get("Name").and_then(Value::as_str),
            Some(name),
            "case {case}: the update must report the ruleset it addressed, got {answered}"
        );
        assert_eq!(
            answered.get("Ruleset").cloned().unwrap_or(Value::Null),
            before.get("Ruleset").cloned().unwrap_or(Value::Null),
            "case {case}: the answer reports the stored Ruleset; expected {before}, got \
             {answered}"
        );
        assert_eq!(
            answered.get("Description").cloned().unwrap_or(Value::Null),
            before.get("Description").cloned().unwrap_or(Value::Null),
            "case {case}: the answer reports the stored Description; expected {before}, got \
             {answered}"
        );

        let after = get(&router, name).await;
        assert_eq!(
            after, before,
            "case {case}: an update naming no member must leave the ruleset, LastModifiedOn \
             included, exactly as it was; expected {before}, got {after}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// An update for a name this account holds no ruleset under writes nothing and is refused
/// with the modelled `EntityNotFoundException`, in the one wording this service gives that
/// miss. The rows differ along how the requested name misses: never created, a prefix of a
/// stored name, a suffix of one, and a stored name in another case — the model documents no
/// case fold for a ruleset name, so a differently-cased name is a different ruleset.
#[tokio::test]
async fn updating_an_unknown_ruleset_is_refused_and_creates_nothing() {
    let router = emulator();
    create(
        &router,
        &json!({ "Name": "StoredRuleset", "Ruleset": "Rules = [ ColumnExists \"a\" ]" }),
    )
    .await;
    let before = get(&router, "StoredRuleset").await;

    for (case, name) in [
        ("never created", "no-such-ruleset"),
        ("a prefix of a stored name", "Stored"),
        ("a suffix of a stored name", "Ruleset"),
        ("a stored name lowercased", "storedruleset"),
        ("a stored name uppercased", "STOREDRULESET"),
    ] {
        let (status, body) = call(
            &router,
            "UpdateDataQualityRuleset",
            &json!({ "Name": name, "Ruleset": "Rules = [ RowCount > 0 ]" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {name:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException for {name:?}, got {body}"
        );
        // The refusal names the ruleset the request addressed, which is its whole address.
        let message = error_message(&body);
        assert!(
            message.contains(name),
            "case {case}: the refusal must name {name:?}, got {message:?}"
        );

        // An update is not a create: the refused name holds no ruleset afterwards either.
        let (read_status, read_body) =
            call(&router, "GetDataQualityRuleset", &json!({ "Name": name })).await;
        assert_eq!(
            read_status, 400,
            "case {case}: a refused update must store nothing under {name:?}, got \
             {read_status} with {read_body}"
        );
    }

    // The stored ruleset is untouched, so the misses above are misses and not a broken
    // lookup or a write that landed somewhere else.
    let after = get(&router, "StoredRuleset").await;
    assert_eq!(
        after, before,
        "case the stored name: a refused update must change nothing; expected {before}, got \
         {after}"
    );
}
