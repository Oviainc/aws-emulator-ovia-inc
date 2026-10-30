// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the enablement an `ImportApiKeys` row leaves its API key with,
//! read back through `GetApiKey` and through `UpdateApiKey`'s own response, and for the
//! refusal of a header line that declares none of a required column.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder, the import call
//! and the API key reads are this crate's existing harness, reused from
//! `rest_api_lifecycle`, `api_key_import` and `api_key_lifecycle`.
//!
//! The RULES under test, each over a SET of inputs rather than one recorded call:
//!
//! * an API key file's `Enabled` column is optional, so a row that does not say leaves the
//!   imported key ENABLED, and a row that says so disables it — asserted over every form a
//!   row can take (no such column, an empty cell, a row stopping short of the header's
//!   width, each word in each spelling, a cell that is neither word) and read back both
//!   through the key's own read and through the response of an `UpdateApiKey` that does not
//!   touch `enabled`;
//! * a header line declaring none of a required column is 400 `BadRequestException` naming
//!   that column, over each required column and several column orders.

use serde_json::{json, Value};

use crate::api_key_import::{import, imported_ids};
use crate::api_key_lifecycle::get_key;
use crate::rest_api_lifecycle::{assert_failure, Answer, Gateway};

/// A distinct, well-formed key value for row `row` of case `case`: inside the published
/// 20-128 character range, and no two rows of this suite collide.
fn row_value(case: usize, row: usize) -> String {
    format!("enablement-api-key-{case:03}-{row:03}")
}

/// Import `document`, asserting the modelled 201 and returning the one id it created.
async fn imported_one(gateway: &Gateway, document: &str, case: &str) -> String {
    let answer = import(gateway, document, false).await;
    assert_eq!(
        answer.status, 201,
        "case {case}: expected 201 from ImportApiKeys, got {} with {:?}",
        answer.status, answer.body
    );
    let ids = imported_ids(&answer.body, case);
    assert_eq!(
        ids.len(),
        1,
        "case {case}: expected exactly one imported key, got {ids:?}"
    );
    ids[0].clone()
}

/// `UpdateApiKey` for `id`, replacing its name — a patch that says nothing about
/// `enabled`, so the response must report the stored enablement.
async fn rename_key(gateway: &Gateway, id: &str, name: &str) -> Answer {
    gateway
        .call(
            "PATCH",
            &format!("/apikeys/{id}"),
            Some(json!({
                "patchOperations": [{ "op": "replace", "path": "/name", "value": name }],
            })),
        )
        .await
}

/// The `enabled` member of a reported API key.
fn reported_enabled(body: &Value, case: &str) -> bool {
    body["enabled"]
        .as_bool()
        .unwrap_or_else(|| panic!("case {case}: expected a boolean enabled, got {body:?}"))
}

#[tokio::test]
async fn an_imported_key_is_enabled_unless_its_row_says_otherwise() {
    let gateway = Gateway::new();

    // Rows differ along what the rule generalises over: whether the row says anything
    // about enablement at all (no column, an empty cell, a row narrower than the header),
    // which word it says, the spelling of that word, where the column sits in the header,
    // and a cell that is neither word — which the import cannot read, so the key keeps the
    // column's default and the caller is warned.
    let rows: Vec<(&str, String, bool)> = vec![
        (
            "no Enabled column at all",
            format!("key,name\n{},no-column\n", row_value(1, 1)),
            true,
        ),
        (
            "an empty Enabled cell",
            format!("key,name,Enabled\n{},empty-cell,\n", row_value(2, 1)),
            true,
        ),
        (
            "a row that stops short of the Enabled column",
            format!("key,name,Enabled\n{},short-row\n", row_value(3, 1)),
            true,
        ),
        (
            "TRUE, as the format's example file spells it",
            format!("key,name,Enabled\n{},word-upper,TRUE\n", row_value(4, 1)),
            true,
        ),
        (
            "true, in lower case",
            format!("key,name,Enabled\n{},word-lower,true\n", row_value(5, 1)),
            true,
        ),
        (
            "True, capitalised",
            format!("key,name,enabled\n{},word-mixed,True\n", row_value(6, 1)),
            true,
        ),
        (
            "FALSE, as the format's example file spells it",
            format!("key,name,Enabled\n{},off-upper,FALSE\n", row_value(7, 1)),
            false,
        ),
        (
            "false, in lower case",
            format!("key,name,Enabled\n{},off-lower,false\n", row_value(8, 1)),
            false,
        ),
        (
            "False, capitalised",
            format!("key,name,ENABLED\n{},off-mixed,False\n", row_value(9, 1)),
            false,
        ),
        (
            "the Enabled column first in the header",
            format!("Enabled,key,name\nFALSE,{},off-first\n", row_value(10, 1)),
            false,
        ),
        (
            "an Enabled cell that is neither word",
            format!("key,name,Enabled\n{},unreadable,yes\n", row_value(11, 1)),
            true,
        ),
    ];

    for (case, document, enabled) in &rows {
        let id = imported_one(&gateway, document, case).await;

        // The stored record reports the enablement the row left it with.
        let read = get_key(&gateway, &id, false).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetApiKey, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            reported_enabled(&read.body, case),
            *enabled,
            "case {case}: expected enabled {enabled:?}, got {:?}",
            read.body
        );

        // An update that says nothing about `enabled` reports the SAME enablement, so the
        // patch response and the read cannot disagree about one record.
        let patched = rename_key(&gateway, &id, &format!("renamed-{id}")).await;
        assert_eq!(
            patched.status, 200,
            "case {case}: expected 200 from UpdateApiKey, got {} with {:?}",
            patched.status, patched.body
        );
        assert_eq!(
            reported_enabled(&patched.body, case),
            *enabled,
            "case {case}: UpdateApiKey must report the stored enablement {enabled:?}, got \
             {:?}",
            patched.body
        );
        let read = get_key(&gateway, &id, false).await;
        assert_eq!(
            reported_enabled(&read.body, case),
            *enabled,
            "case {case}: the update must not have changed enabled, got {:?}",
            read.body
        );
    }

    // A row that DISABLES the key can be enabled through the same patch path, so the
    // import's default is a starting point and not a state the key is stuck in.
    let id = imported_one(
        &gateway,
        &format!("key,name,Enabled\n{},togglable,FALSE\n", row_value(12, 1)),
        "the togglable key",
    )
    .await;
    let patched = gateway
        .call(
            "PATCH",
            &format!("/apikeys/{id}"),
            Some(json!({
                "patchOperations": [{ "op": "replace", "path": "/enabled", "value": "true" }],
            })),
        )
        .await;
    assert_eq!(
        patched.status, 200,
        "case the togglable key: expected 200 from UpdateApiKey, got {} with {:?}",
        patched.status, patched.body
    );
    assert!(
        reported_enabled(&patched.body, "the togglable key"),
        "case the togglable key: expected the patch to enable it, got {:?}",
        patched.body
    );
}

#[tokio::test]
async fn a_header_declaring_none_of_a_required_column_names_that_column() {
    let gateway = Gateway::new();

    // Rows differ in WHICH required column the header leaves out, in the columns it does
    // declare, and in their order; a header missing BOTH names the first one the format
    // requires. The expected sentence is the one API Gateway answers for this condition,
    // with the column quoted and capitalised.
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "no key column",
            format!("name,description\nnamed,{}\n", "no key here"),
            "Missing required column 'Key' ",
        ),
        (
            "no key column, with the Enabled column present",
            "name,Enabled\nnamed,TRUE\n".to_string(),
            "Missing required column 'Key' ",
        ),
        (
            "no name column",
            format!("key,description\n{},no name column\n", row_value(20, 1)),
            "Missing required column 'Name' ",
        ),
        (
            "no name column, in another order",
            format!("description,key\nno name column,{}\n", row_value(21, 1)),
            "Missing required column 'Name' ",
        ),
        (
            "neither required column",
            "description,Enabled\nnot a key file,true\n".to_string(),
            "Missing required column 'Key' ",
        ),
        (
            "a header line alone, declaring neither",
            "usageplanIds\n".to_string(),
            "Missing required column 'Key' ",
        ),
    ];

    for (case, document, expected) in &rows {
        let answer = import(&gateway, document, false).await;
        assert_failure(&answer, 400, "BadRequestException", case);
        assert_eq!(
            answer.body["message"].as_str(),
            Some(*expected),
            "case {case}: expected the message {expected:?}, got {:?}",
            answer.body["message"]
        );
    }

    // A header declaring both required columns imports, so the refusals above are about
    // the missing column and not about the document.
    let id = imported_one(
        &gateway,
        &format!("name,key\nboth-columns,{}\n", row_value(22, 1)),
        "both required columns",
    )
    .await;
    let read = get_key(&gateway, &id, false).await;
    assert_eq!(
        read.status, 200,
        "case both required columns: expected 200 from GetApiKey, got {} with {:?}",
        read.status, read.body
    );
}
