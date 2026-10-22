// SPDX-License-Identifier: CC0-1.0

//! End-to-end restJson1 tests for the field-option slice of Cases
//! (`BatchPutFieldOptions` + `ListFieldOptions`), driven through the real
//! `MultiServiceRouter` so every assertion goes over the wire the SDK speaks.
//!
//! The rules under test:
//!
//! * a field option is identified by its `value` within its field, so
//!   `BatchPutFieldOptions` CREATES a value the field does not hold and UPDATES
//!   one it does, and `ListFieldOptions` answers the stored set computed from
//!   that state — never from the request;
//! * field options belong to a single-select field, so a put against a field of
//!   any other `FieldType` is refused with `ValidationException` (400) and such
//!   a field lists no options;
//! * an option value outside the character set the model documents is reported
//!   in the response's per-option `errors` list and not persisted, while the
//!   well-formed options of the same request are;
//! * a listing is ordered by option value, `values` filters it, `maxResults`
//!   sizes the page and `nextToken` walks every option exactly once;
//! * every id a request carries is resolved against persisted state first: an
//!   unknown domain or field is refused with `ResourceNotFoundException` (404).
//!
//! The harness (`Emulator`, `domain`, `fields_of_every_type`) is the one
//! `case_lifecycle.rs` already owns.

use serde_json::{json, Value};
use service_com_amazonaws_connectcases_amazonconnectcases::state::{
    MAX_BATCH_FIELD_OPTIONS, MAX_FIELD_OPTIONS_PER_FIELD, MAX_FIELD_OPTION_TEXT_LEN,
};

use crate::case_lifecycle::{domain, field_of_type, fields_of_every_type, string_at, Emulator};

/// `FieldOptionValue` forms the service accepts: "max length 100 and must be
/// alphanumeric with hyphens and underscores"
/// (`smithy_query(query="shape", shape="FieldOption")`, member `value`). The rows
/// differ along the axes that rule generalises over — letters, digits, case,
/// hyphen, underscore and the published maximum length — and none of them is the
/// value the recorded unit used.
fn well_formed_values() -> Vec<(&'static str, String)> {
    vec![
        ("lowercase letters", "urgent".to_string()),
        ("digits only", "12345".to_string()),
        ("mixed case", "HighPriority".to_string()),
        ("hyphenated", "needs-review".to_string()),
        ("underscored", "on_hold".to_string()),
        ("single character", "x".to_string()),
        // The published maximum: `FieldOptionValue` is `length 1..100`.
        ("at the length maximum", "v".repeat(MAX_FIELD_OPTION_TEXT_LEN)),
    ]
}

/// `FieldOptionValue` forms the model's own `pattern` (`^.*[\S]$`) and `length`
/// (`1..100`) accept but the documented character rule above does not, so the
/// boundary validator passes them and the handler has to report them per option.
fn malformed_values() -> Vec<(&'static str, &'static str)> {
    vec![
        ("an inner space", "needs review"),
        ("a dot", "needs.review"),
        ("a comma", "a,b"),
        ("a slash", "a/b"),
        ("a plus", "a+b"),
        ("a non-ASCII letter", "übergabe"),
        ("a leading space", " urgent"),
    ]
}

/// One `FieldOption` request entry.
fn option(name: &str, value: &str, active: bool) -> Value {
    json!({ "name": name, "value": value, "active": active })
}

/// `BatchPutFieldOptions`: `PUT /domains/{domainId}/fields/{fieldId}/options`.
fn put_path(domain_id: &str, field_id: &str) -> String {
    format!("/domains/{domain_id}/fields/{field_id}/options")
}

/// `ListFieldOptions`: `POST /domains/{domainId}/fields/{fieldId}/options-list`,
/// whose `maxResults`, `nextToken` and `values` are `httpQuery` bindings.
fn list_path(domain_id: &str, field_id: &str, query: &str) -> String {
    let base = format!("/domains/{domain_id}/fields/{field_id}/options-list");
    if query.is_empty() {
        base
    } else {
        format!("{base}?{query}")
    }
}

/// Put `options` and hand back the response's `errors` list.
async fn put_options(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    field_id: &str,
    options: Vec<Value>,
) -> Vec<Value> {
    let response = emulator
        .ok(
            case,
            "PUT",
            &put_path(domain_id, field_id),
            json!({ "options": options }),
        )
        .await;
    response
        .get("errors")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: BatchPutFieldOptions returned no errors list: {response:?}")
        })
        .clone()
}

/// List one page, returning its `options` list and `nextToken`.
async fn list_page(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    field_id: &str,
    query: &str,
) -> (Vec<Value>, Option<String>) {
    let (status, body) = emulator
        .send("POST", &list_path(domain_id, field_id, query), None)
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListFieldOptions, got {status} with body {body:?}"
    );
    let options = body
        .get("options")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: ListFieldOptions returned no options list: {body:?}")
        })
        .clone();
    let next = body
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (options, next)
}

/// Every option of a field, in the order the service lists them.
async fn list_all(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    field_id: &str,
) -> Vec<(String, String, bool)> {
    let (options, next) = list_page(emulator, case, domain_id, field_id, "").await;
    assert!(
        next.is_none() || options.len() == 100,
        "case {case:?}: a full listing under the default page size must not paginate, got \
         nextToken {next:?} with {} options",
        options.len()
    );
    options.iter().map(|option| triple(option, case)).collect()
}

/// A listed option as `(value, name, active)`.
fn triple(option: &Value, case: &str) -> (String, String, bool) {
    (
        string_at(option, "value", case),
        string_at(option, "name", case),
        option
            .get("active")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| panic!("case {case:?}: no boolean member active in {option:?}")),
    )
}

/// A domain holding one field of every `FieldType`, and the id of its
/// single-select field — the one field options belong to.
async fn single_select_field(emulator: &Emulator, name: &str) -> (String, String) {
    let domain_id = domain(emulator, name).await;
    let fields = fields_of_every_type(emulator, &domain_id).await;
    let field_id = field_of_type(&fields, "SingleSelect");
    (domain_id, field_id)
}

#[tokio::test]
async fn a_field_option_set_round_trips_through_state_keyed_by_its_value() {
    let emulator = Emulator::new();
    let (domain_id, field_id) = single_select_field(&emulator, "option-round-trip").await;

    // A field that has never been written to holds no options.
    assert!(
        list_all(&emulator, "before any put", &domain_id, &field_id)
            .await
            .is_empty(),
        "a field with no options must list none"
    );

    // 1. Every well-formed value form is created and read back exactly.
    let rows = well_formed_values();
    let created: Vec<Value> = rows
        .iter()
        .map(|(case, value)| option(&format!("name of {case}"), value, true))
        .collect();
    let errors = put_options(&emulator, "create every form", &domain_id, &field_id, created).await;
    assert!(
        errors.is_empty(),
        "case \"create every form\": every value is well formed, so no option may fail: \
         {errors:?}"
    );

    let listed = list_all(&emulator, "after create", &domain_id, &field_id).await;
    let mut expected: Vec<(String, String, bool)> = rows
        .iter()
        .map(|(case, value)| (value.clone(), format!("name of {case}"), true))
        .collect();
    expected.sort();
    assert_eq!(
        listed, expected,
        "after create: the listing must be the stored set ordered by value, got {listed:?}"
    );

    // 2. A put on a value the field already holds UPDATES that option, and one
    //    it does not hold is added — the set grows by exactly the new values.
    let (_, updated_value) = &rows[0];
    let update = vec![
        option("renamed and deactivated", updated_value, false),
        option("a later addition", "late-arrival", true),
    ];
    let errors = put_options(&emulator, "update one, add one", &domain_id, &field_id, update).await;
    assert!(
        errors.is_empty(),
        "case \"update one, add one\": no option may fail: {errors:?}"
    );

    let listed = list_all(&emulator, "after update", &domain_id, &field_id).await;
    expected.retain(|(value, _, _)| value != updated_value);
    expected.push((
        updated_value.clone(),
        "renamed and deactivated".to_string(),
        false,
    ));
    expected.push(("late-arrival".to_string(), "a later addition".to_string(), true));
    expected.sort();
    assert_eq!(
        listed, expected,
        "after update: an existing value must be updated in place and a new one appended, got \
         {listed:?}"
    );

    // 3. The operation is idempotent (`@idempotent` in the model): putting the
    //    same set again changes nothing a reader can see.
    let replay: Vec<Value> = expected
        .iter()
        .map(|(value, name, active)| option(name, value, *active))
        .collect();
    let errors = put_options(&emulator, "replay the set", &domain_id, &field_id, replay).await;
    assert!(
        errors.is_empty(),
        "case \"replay the set\": no option may fail: {errors:?}"
    );
    let after_replay = list_all(&emulator, "after replay", &domain_id, &field_id).await;
    assert_eq!(
        after_replay, expected,
        "after replay: re-putting the stored set must leave it unchanged, got {after_replay:?}"
    );

    // 4. A listing is a read: it changes nothing a later caller sees.
    let again = list_all(&emulator, "a second listing", &domain_id, &field_id).await;
    assert_eq!(
        again, expected,
        "a second listing must answer exactly what the first did, got {again:?}"
    );
}

#[tokio::test]
async fn field_options_belong_to_a_single_select_field() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "option-field-type").await;
    let fields = fields_of_every_type(&emulator, &domain_id).await;

    // One row per `FieldType` the model declares (`FIELD_TYPE_VALUES`): the
    // single-select field takes options, every other type is refused.
    for (field_type, field_id) in &fields {
        let case = format!("BatchPutFieldOptions on a {field_type} field");
        let body = json!({ "options": [option("an option", "an-option", true)] });
        if *field_type == "SingleSelect" {
            let errors = put_options(
                &emulator,
                &case,
                &domain_id,
                field_id,
                vec![option("an option", "an-option", true)],
            )
            .await;
            assert!(
                errors.is_empty(),
                "case {case:?}: a single-select field accepts options: {errors:?}"
            );
            let listed = list_all(&emulator, &case, &domain_id, field_id).await;
            assert_eq!(
                listed,
                vec![(
                    "an-option".to_string(),
                    "an option".to_string(),
                    true
                )],
                "case {case:?}: the option must be readable back, got {listed:?}"
            );
        } else {
            let (status, code, message) = emulator
                .failure_detail(
                    &case,
                    "PUT",
                    &put_path(&domain_id, field_id),
                    Some(body.clone()),
                )
                .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ValidationException"),
                "case {case:?}: options may only be set on a SingleSelect field, got \
                 {status}/{code} ({message})"
            );
            // A field that cannot hold options holds none — a read, not a refusal.
            let listed = list_all(&emulator, &case, &domain_id, field_id).await;
            assert!(
                listed.is_empty(),
                "case {case:?}: a {field_type} field must list no options, got {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_option_value_outside_the_documented_character_set_is_reported_per_option() {
    let emulator = Emulator::new();
    let (domain_id, field_id) = single_select_field(&emulator, "option-value-rule").await;

    for (case, bad) in malformed_values() {
        let case = format!("value with {case}");
        // Each request pairs the malformed value with a well-formed one, so the
        // response has to report the one and persist the other.
        let good = format!("ok-{}", bad.len());
        let errors = put_options(
            &emulator,
            &case,
            &domain_id,
            &field_id,
            vec![
                option("a bad option", bad, true),
                option("a good option", &good, true),
            ],
        )
        .await;

        assert_eq!(
            errors.len(),
            1,
            "case {case:?}: exactly the malformed option must be reported, got {errors:?}"
        );
        let reported = &errors[0];
        assert_eq!(
            string_at(reported, "value", &case),
            bad,
            "case {case:?}: the error must name the value that caused it, got {reported:?}"
        );
        assert_eq!(
            string_at(reported, "errorCode", &case),
            "ValidationException",
            "case {case:?}: the per-option error code must classify it, got {reported:?}"
        );
        assert!(
            !string_at(reported, "message", &case).is_empty(),
            "case {case:?}: the per-option error must carry a message, got {reported:?}"
        );

        let listed = list_all(&emulator, &case, &domain_id, &field_id).await;
        assert!(
            listed.iter().any(|(value, _, _)| *value == good),
            "case {case:?}: the well-formed option of the same request must persist, got {listed:?}"
        );
        assert!(
            !listed.iter().any(|(value, _, _)| value == bad),
            "case {case:?}: the malformed option must not be persisted, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn next_token_walks_every_option_exactly_once() {
    let emulator = Emulator::new();
    let (domain_id, field_id) = single_select_field(&emulator, "option-pagination").await;

    // 30 options, written in two requests, so the stored order is not the
    // request order of either.
    let total = 30usize;
    let values: Vec<String> = (0..total).map(|i| format!("opt-{i:03}")).collect();
    for chunk in values.chunks(17) {
        let batch: Vec<Value> = chunk
            .iter()
            .map(|value| option(&format!("name {value}"), value, true))
            .collect();
        let errors = put_options(&emulator, "seed the options", &domain_id, &field_id, batch).await;
        assert!(
            errors.is_empty(),
            "case \"seed the options\": no option may fail: {errors:?}"
        );
    }
    let mut sorted = values.clone();
    sorted.sort();

    // The rows differ along page size, including both published bounds of
    // `maxResults` (`range 1..100`) and a size larger than the set.
    for page_size in [1usize, 2, 7, 29, 30, 100] {
        let case = format!("maxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut query = format!("maxResults={page_size}");
            if let Some(cursor) = &token {
                query.push_str(&format!("&nextToken={cursor}"));
            }
            let (options, next) = list_page(&emulator, &case, &domain_id, &field_id, &query).await;
            pages += 1;
            assert!(
                pages <= total + 1,
                "case {case:?}: pagination must terminate, walked {pages} pages"
            );
            assert!(
                options.len() <= page_size,
                "case {case:?}: a page must hold at most maxResults options, got {}",
                options.len()
            );
            if next.is_some() {
                assert_eq!(
                    options.len(),
                    page_size,
                    "case {case:?}: a page that is followed by another must be full, got {}",
                    options.len()
                );
            }
            for option in &options {
                walked.push(string_at(option, "value", &case));
            }
            token = next;
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, sorted,
            "case {case:?}: paging must walk every option exactly once in value order, got \
             {walked:?}"
        );
    }

    // A token this service did not mint is not a position: it is refused.
    for (case, forged) in [
        ("hex that decodes to foreign bytes", "6e6f742d612d637572736f72"),
        ("not hex at all", "not-a-token"),
        ("an empty token", ""),
    ] {
        let (status, code, message) = emulator
            .failure_detail(
                case,
                "POST",
                &list_path(&domain_id, &field_id, &format!("nextToken={forged}")),
                None,
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: a forged nextToken must be refused, got {status}/{code} ({message})"
        );
    }

    // A genuine cursor is scoped to the field it was minted for, so it is not a
    // position in another field's option set.
    let case = "a cursor minted for another field";
    let created = emulator
        .ok(
            case,
            "POST",
            &format!("/domains/{domain_id}/fields"),
            json!({ "name": "a second single select", "type": "SingleSelect" }),
        )
        .await;
    let other_field = string_at(&created, "fieldId", case);
    let errors = put_options(
        &emulator,
        case,
        &domain_id,
        &other_field,
        vec![option("elsewhere", "elsewhere", true)],
    )
    .await;
    assert!(
        errors.is_empty(),
        "case {case:?}: no option may fail: {errors:?}"
    );
    let (_, minted) = list_page(&emulator, case, &domain_id, &field_id, "maxResults=1").await;
    let minted = minted.unwrap_or_else(|| panic!("case {case:?}: a partial page must mint a token"));
    let (status, code, message) = emulator
        .failure_detail(
            case,
            "POST",
            &list_path(&domain_id, &other_field, &format!("nextToken={minted}")),
            None,
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "case {case:?}: a cursor from another field must be refused, got {status}/{code} \
         ({message})"
    );

    // One past each published bound of `maxResults` is refused by the model's
    // own `range 1..100`.
    for (case, max_results) in [("below the minimum", "0"), ("above the maximum", "101")] {
        let (status, code, message) = emulator
            .failure_detail(
                case,
                "POST",
                &list_path(&domain_id, &field_id, &format!("maxResults={max_results}")),
                None,
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: maxResults={max_results} is outside the modelled range, got \
             {status}/{code} ({message})"
        );
    }
}

#[tokio::test]
async fn values_filters_the_listing_to_the_option_it_names() {
    let emulator = Emulator::new();
    let (domain_id, field_id) = single_select_field(&emulator, "option-filter").await;

    let stored = ["alpha", "beta", "gamma-1", "delta_2"];
    let batch: Vec<Value> = stored
        .iter()
        .map(|value| option(&format!("name {value}"), value, true))
        .collect();
    let errors = put_options(&emulator, "seed the options", &domain_id, &field_id, batch).await;
    assert!(
        errors.is_empty(),
        "case \"seed the options\": no option may fail: {errors:?}"
    );

    // One row per stored value, plus a value the field does not hold.
    for value in stored {
        let case = format!("values={value}");
        let (options, next) =
            list_page(&emulator, &case, &domain_id, &field_id, &format!("values={value}")).await;
        let listed: Vec<(String, String, bool)> =
            options.iter().map(|option| triple(option, &case)).collect();
        assert_eq!(
            listed,
            vec![(
                value.to_string(),
                format!("name {value}"),
                true
            )],
            "case {case:?}: the filter must answer exactly the option it names, got {listed:?}"
        );
        assert!(
            next.is_none(),
            "case {case:?}: a single-option page is the last one, got nextToken {next:?}"
        );
    }

    let case = "values names no stored option";
    let (options, _) = list_page(
        &emulator,
        case,
        &domain_id,
        &field_id,
        "values=epsilon",
    )
    .await;
    assert!(
        options.is_empty(),
        "case {case:?}: a filter on an absent value must answer no options, got {options:?}"
    );

    // `values` is capped at one entry by the model (`length ..1`).
    let case = "two filter values";
    let (status, code, message) = emulator
        .failure_detail(
            case,
            "POST",
            &list_path(&domain_id, &field_id, "values=alpha&values=beta"),
            None,
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "case {case:?}: values accepts at most one entry, got {status}/{code} ({message})"
    );
}

#[tokio::test]
async fn an_unknown_domain_or_field_is_refused_before_anything_is_written() {
    let emulator = Emulator::new();
    let (domain_id, field_id) = single_select_field(&emulator, "option-unknown-ids").await;
    let body = json!({ "options": [option("an option", "an-option", true)] });

    // One row per (operation, unresolvable id) pair.
    let cases: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "BatchPutFieldOptions on an unknown domain",
            "PUT",
            put_path("no-such-domain", &field_id),
            Some(body.clone()),
        ),
        (
            "BatchPutFieldOptions on an unknown field",
            "PUT",
            put_path(&domain_id, "no-such-field"),
            Some(body.clone()),
        ),
        (
            "ListFieldOptions on an unknown domain",
            "POST",
            list_path("no-such-domain", &field_id, ""),
            None,
        ),
        (
            "ListFieldOptions on an unknown field",
            "POST",
            list_path(&domain_id, "no-such-field", ""),
            None,
        ),
    ];
    for (case, method, path, payload) in cases {
        let (status, code, message) = emulator
            .failure_detail(case, method, &path, payload)
            .await;
        assert_eq!(
            (status, code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: an id the store does not hold must be refused, got {status}/{code} \
             ({message})"
        );
    }

    // Nothing the refused calls carried was written.
    let listed = list_all(&emulator, "after the refusals", &domain_id, &field_id).await;
    assert!(
        listed.is_empty(),
        "a refused put must persist nothing, got {listed:?}"
    );
}

#[tokio::test]
async fn an_option_set_is_bounded_and_a_repeated_value_is_refused() {
    let emulator = Emulator::new();
    let (domain_id, field_id) = single_select_field(&emulator, "option-bounds").await;

    // The same value twice in one request has no defined meaning.
    let case = "a value repeated in one request";
    let (status, code, message) = emulator
        .failure_detail(
            case,
            "PUT",
            &put_path(&domain_id, &field_id),
            Some(json!({
                "options": [
                    option("first", "repeated", true),
                    option("second", "repeated", false),
                ]
            })),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "case {case:?}: two entries for one value must be refused, got {status}/{code} ({message})"
    );

    // One past the model's own per-request cap (`options` is `length ..50`).
    let case = "one option past the request cap";
    let too_many: Vec<Value> = (0..=MAX_BATCH_FIELD_OPTIONS)
        .map(|i| option(&format!("name {i}"), &format!("over-{i:04}"), true))
        .collect();
    let (status, code, message) = emulator
        .failure_detail(
            case,
            "PUT",
            &put_path(&domain_id, &field_id),
            Some(json!({ "options": too_many })),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "case {case:?}: at most {MAX_BATCH_FIELD_OPTIONS} options may be put at once, got \
         {status}/{code} ({message})"
    );

    // Fill the field to its ceiling, then one more NEW value is refused with
    // the modelled quota error while an UPDATE of a value it already holds is
    // still accepted.
    let mut minted = 0usize;
    while minted < MAX_FIELD_OPTIONS_PER_FIELD {
        let batch: Vec<Value> = (minted..(minted + MAX_BATCH_FIELD_OPTIONS))
            .map(|i| option(&format!("name {i}"), &format!("fill-{i:05}"), true))
            .collect();
        let errors = put_options(&emulator, "fill the field", &domain_id, &field_id, batch).await;
        assert!(
            errors.is_empty(),
            "case \"fill the field\": no option may fail: {errors:?}"
        );
        minted += MAX_BATCH_FIELD_OPTIONS;
    }

    let case = "one option past the field ceiling";
    let (status, code, message) = emulator
        .failure_detail(
            case,
            "PUT",
            &put_path(&domain_id, &field_id),
            Some(json!({ "options": [option("one too many", "one-too-many", true)] })),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (402, "ServiceQuotaExceededException"),
        "case {case:?}: a full field must refuse a new option, got {status}/{code} ({message})"
    );

    let case = "an update at the field ceiling";
    let errors = put_options(
        &emulator,
        case,
        &domain_id,
        &field_id,
        vec![option("renamed", "fill-00000", false)],
    )
    .await;
    assert!(
        errors.is_empty(),
        "case {case:?}: updating a value the field already holds is not growth: {errors:?}"
    );
    let (options, _) = list_page(
        &emulator,
        case,
        &domain_id,
        &field_id,
        "values=fill-00000",
    )
    .await;
    assert_eq!(
        options.iter().map(|o| triple(o, case)).collect::<Vec<_>>(),
        vec![("fill-00000".to_string(), "renamed".to_string(), false)],
        "case {case:?}: the update must be what a later read sees, got {options:?}"
    );
}

#[tokio::test]
async fn an_empty_option_list_is_accepted_and_changes_nothing() {
    let emulator = Emulator::new();
    let (domain_id, field_id) = single_select_field(&emulator, "option-empty-put").await;

    let errors = put_options(
        &emulator,
        "seed one option",
        &domain_id,
        &field_id,
        vec![option("kept", "kept", true)],
    )
    .await;
    assert!(
        errors.is_empty(),
        "case \"seed one option\": no option may fail: {errors:?}"
    );

    let case = "an empty options list";
    let errors = put_options(&emulator, case, &domain_id, &field_id, Vec::new()).await;
    assert!(
        errors.is_empty(),
        "case {case:?}: nothing was submitted, so nothing can fail: {errors:?}"
    );
    let listed = list_all(&emulator, case, &domain_id, &field_id).await;
    assert_eq!(
        listed,
        vec![("kept".to_string(), "kept".to_string(), true)],
        "case {case:?}: a put that names no option must leave the set alone, got {listed:?}"
    );
}
