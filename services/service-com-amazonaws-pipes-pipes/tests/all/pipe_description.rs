// SPDX-License-Identifier: CC0-1.0

//! What a pipe's `Description` is worth storing, end to end over the wire.
//!
//! `PipeDescription` is `@length(min: 0, max: 512)`, so the EMPTY STRING is a value a
//! request may carry — and it is not a description: a pipe holds one or holds none, and the
//! empty string is how a caller says "none". Recorded behaviour: `Description=""` is stored
//! exactly as an omitted `Description` is, so the pipe reports NO `Description` member at
//! all for it, while any non-empty description is preserved byte for byte.
//!
//! Both writes that can set a description are covered, and the rows span what the rule
//! generalises over: absent, empty, whitespace (which is NOT empty), one character, unicode
//! text, and the longest `PipeDescription` allows. Every assertion goes through
//! `MultiServiceRouter::dispatch` — the one path a socket-served request takes — reusing the
//! harness `pipe_lifecycle` and `pipe_update` already own.

use serde_json::{json, Value};

use crate::pipe_lifecycle::{create, describe, minimal_body, router, PAST_SETTLE};
use crate::pipe_update::update;

const ROLE: &str = "arn:aws:iam::000000000000:role/pipes-role";
const SOURCE: &str = "arn:aws:sqs:us-west-2:000000000000:orders";
const TARGET: &str = "arn:aws:lambda:us-west-2:000000000000:function:handler";

/// `PipeDescription`'s published maximum. Source: the `PipeDescription` shape of
/// `smithy-model/com-amazonaws-pipes-pipes.json` (`@length(min: 0, max: 512)`).
const MAX_DESCRIPTION: usize = 512;

/// One row: the description a request carries, and what a read must report for it —
/// `Value::Null` for the member being absent.
struct Row {
    case: &'static str,
    /// `None` for a request that carries no `Description` member at all.
    sent: Option<String>,
    reported: Value,
}

fn rows() -> Vec<Row> {
    let longest = "d".repeat(MAX_DESCRIPTION);
    vec![
        Row {
            case: "absent",
            sent: None,
            reported: Value::Null,
        },
        Row {
            case: "empty",
            sent: Some(String::new()),
            reported: Value::Null,
        },
        Row {
            // Whitespace is not the empty string: it is a description, so it is kept.
            case: "one-space",
            sent: Some(" ".to_string()),
            reported: json!(" "),
        },
        Row {
            case: "one-character",
            sent: Some("d".to_string()),
            reported: json!("d"),
        },
        Row {
            case: "unicode",
            sent: Some("données — 日本語 ✅ 🚀".to_string()),
            reported: json!("données — 日本語 ✅ 🚀"),
        },
        Row {
            case: "longest-allowed",
            sent: Some(longest.clone()),
            reported: json!(longest),
        },
    ]
}

/// RULE: `CreatePipe` stores an empty `Description` exactly as an omitted one — the pipe
/// reports no `Description` at all — and stores every other description as sent.
#[tokio::test]
async fn create_pipe_stores_an_empty_description_as_no_description() {
    for (index, row) in rows().into_iter().enumerate() {
        let router = router();
        let name = format!("createdesc{index}");
        let mut body = minimal_body(SOURCE, TARGET);
        if let Some(sent) = &row.sent {
            body["Description"] = json!(sent);
        }
        let created = create(&router, &name, body).await;
        assert_eq!(
            created.status,
            200,
            "case {:?}: CreatePipe must succeed, got {created:?}",
            row.case
        );

        let described = describe(&router, &name).await;
        assert_eq!(
            described.member("Description"),
            &row.reported,
            "case {:?}: DescribePipe must report Description as {}, got {:?}",
            row.case,
            row.reported,
            described.member("Description")
        );
    }
}

/// RULE: the same holds for `UpdatePipe` — an empty `Description` CLEARS the one the pipe
/// carried (leaving it reporting none, exactly as a pipe created without one), any other
/// description replaces it, and an update that carries no `Description` leaves the stored
/// one alone.
#[tokio::test]
async fn update_pipe_clears_the_description_with_an_empty_one() {
    for (index, row) in rows().into_iter().enumerate() {
        let router = router();
        let name = format!("updatedesc{index}");
        let mut body = minimal_body(SOURCE, TARGET);
        body["Description"] = json!("before");
        let created = create(&router, &name, body).await;
        assert_eq!(
            created.status,
            200,
            "case {:?}: the fixture pipe must be created, got {created:?}",
            row.case
        );
        tokio::time::sleep(PAST_SETTLE).await;

        let mut update_body = json!({"RoleArn": ROLE});
        if let Some(sent) = &row.sent {
            update_body["Description"] = json!(sent);
        }
        let updated = update(&router, &name, update_body).await;
        assert_eq!(
            updated.status,
            200,
            "case {:?}: UpdatePipe must succeed, got {updated:?}",
            row.case
        );
        tokio::time::sleep(PAST_SETTLE).await;

        // An update carrying no `Description` leaves the stored one; every other row
        // reports what the rule says of the value it sent.
        let expected = if row.sent.is_none() {
            json!("before")
        } else {
            row.reported.clone()
        };
        let described = describe(&router, &name).await;
        assert_eq!(
            described.member("Description"),
            &expected,
            "case {:?}: DescribePipe must report Description as {expected}, got {:?}",
            row.case,
            described.member("Description")
        );
    }
}
