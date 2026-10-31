// SPDX-License-Identifier: CC0-1.0

//! The `ids`/`names` selector rule every id-or-name batch reader of this service answers
//! by, end to end over the wire.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body — so the
//! boundary validator, the protocol codec and the handler all take part, exactly as they do
//! for an SDK client.
//!
//! The rule under test: a batch read that takes both an `ids` and a `names` member selects
//! its resources by exactly ONE of them — `ids` and `names` each document "You can't
//! provide names and IDs in the same request"
//! (`smithy_query(query="doc", shape="BatchGetCollectionRequest", member="ids")`,
//! `smithy_query(query="doc", shape="BatchGetCollectionGroupRequest", member="ids")`), and a
//! request carrying neither selects nothing at all — so a request naming both members and
//! one naming neither are each refused, with one sentence per cause whichever resource is
//! being read. The table spans what the rule generalises over: which member is present,
//! how many entries it carries, and whether those entries resolve to a stored resource;
//! each row is driven through BOTH readers and their answers compared, which is what a
//! second copy of the branch could not keep true.

use serde_json::{json, Value};
use service_com_amazonaws_opensearchserverless_opensearchserverless::state::batch_selector_refusal;

use crate::collection_lifecycle::{at, call, emulator, text, CLIENT_ERROR};

/// The sentence the rule's "both members" cause is worded with.
const BOTH: &str = "ids and names cannot be provided in the same request";

/// The sentence its "neither member" cause is worded with.
const NEITHER: &str = "one of ids or names must be provided";

/// A well-formed id that names no stored resource: inside `CollectionId`'s modelled
/// `^[a-z0-9]{3,40}$`, which is also `CollectionGroupId`'s own pattern
/// (`smithy_query(query="shape", shape="CollectionId,CollectionGroupId")`).
const UNKNOWN_ID: &str = "nosuchresource000000";

/// A name that names no stored resource, inside the create requests' own name pattern.
const UNKNOWN_NAME: &str = "nosuch-resource";

/// What the rule says about one `(ids, names)` combination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// Exactly one selector is named: the read is served.
    Served,
    /// Both or neither are named: refused with this sentence.
    Refused(&'static str),
}

/// One entry of a selector list — one that resolves to the stored resource, and one that
/// resolves to nothing, so a refused row is refused whether or not its keys resolve.
#[derive(Debug, Clone, Copy)]
enum Key {
    Stored,
    Unknown,
}

/// One row of the table: which of the two members the request carries, with which entries,
/// and what the rule says about that combination.
struct Row {
    case: &'static str,
    ids: Option<Vec<Key>>,
    names: Option<Vec<Key>>,
    expected: Verdict,
}

/// One batch reader: the operation, the resource stored for it, and the two members its
/// answer carries.
struct Reader {
    operation: &'static str,
    stored_id: String,
    stored_name: String,
    details: &'static str,
    errors: &'static str,
}

impl Reader {
    /// `keys` as the ids of this reader's resource.
    fn ids(&self, keys: &[Key]) -> Vec<String> {
        keys.iter()
            .map(|key| match key {
                Key::Stored => self.stored_id.clone(),
                Key::Unknown => UNKNOWN_ID.to_string(),
            })
            .collect()
    }

    /// `keys` as the names of this reader's resource.
    fn names(&self, keys: &[Key]) -> Vec<String> {
        keys.iter()
            .map(|key| match key {
                Key::Stored => self.stored_name.clone(),
                Key::Unknown => UNKNOWN_NAME.to_string(),
            })
            .collect()
    }

    /// The request body for one row's selectors.
    fn body(&self, ids: Option<&[Key]>, names: Option<&[Key]>) -> Value {
        let mut body = json!({});
        if let Some(keys) = ids {
            body["ids"] = json!(self.ids(keys));
        }
        if let Some(keys) = names {
            body["names"] = json!(self.names(keys));
        }
        body
    }
}

/// How many entries of `field` the answer carries.
fn count(answer: &Value, field: &str) -> usize {
    at(answer, field).as_array().map(Vec::len).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// One selector rule, one answer, whichever batch reader takes the request.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn both_batch_readers_answer_the_selector_rule_the_same_way() {
    let router = emulator();

    // One collection and one collection group, so every row's `Key::Stored` entry resolves
    // for the reader it is sent to.
    let (status, code, answer) = call(
        &router,
        "CreateCollection",
        json!({
            "name": "selector-collection",
            "encryptionConfig": { "aWSOwnedKey": true },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateCollection(selector-collection): expected 200, got {status} {code} with {answer}"
    );
    let collection_id = text(&answer, "createCollectionDetail.id");

    let (status, code, answer) = call(
        &router,
        "CreateCollectionGroup",
        json!({ "name": "selector-group", "standbyReplicas": "ENABLED" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateCollectionGroup(selector-group): expected 200, got {status} {code} with {answer}"
    );
    let group_id = text(&answer, "createCollectionGroupDetail.id");

    let readers = [
        Reader {
            operation: "BatchGetCollection",
            stored_id: collection_id,
            stored_name: "selector-collection".to_string(),
            details: "collectionDetails",
            errors: "collectionErrorDetails",
        },
        Reader {
            operation: "BatchGetCollectionGroup",
            stored_id: group_id,
            stored_name: "selector-group".to_string(),
            details: "collectionGroupDetails",
            errors: "collectionGroupErrorDetails",
        },
    ];

    // The rows differ along which member is present, how many entries it carries, and
    // whether each entry resolves — the three things the rule does NOT look at beyond
    // which of the two members was sent.
    let rows: Vec<Row> = vec![
        Row {
            case: "neither member",
            ids: None,
            names: None,
            expected: Verdict::Refused(NEITHER),
        },
        Row {
            case: "both members, one stored key each",
            ids: Some(vec![Key::Stored]),
            names: Some(vec![Key::Stored]),
            expected: Verdict::Refused(BOTH),
        },
        Row {
            case: "both members, a stored id and an unknown name",
            ids: Some(vec![Key::Stored]),
            names: Some(vec![Key::Unknown]),
            expected: Verdict::Refused(BOTH),
        },
        Row {
            case: "both members, two ids and three names",
            ids: Some(vec![Key::Stored, Key::Unknown]),
            names: Some(vec![Key::Stored, Key::Unknown, Key::Unknown]),
            expected: Verdict::Refused(BOTH),
        },
        Row {
            case: "ids only, one stored id",
            ids: Some(vec![Key::Stored]),
            names: None,
            expected: Verdict::Served,
        },
        Row {
            case: "ids only, one unknown id",
            ids: Some(vec![Key::Unknown]),
            names: None,
            expected: Verdict::Served,
        },
        Row {
            case: "ids only, a stored and an unknown id",
            ids: Some(vec![Key::Stored, Key::Unknown]),
            names: None,
            expected: Verdict::Served,
        },
        Row {
            case: "names only, one stored name",
            ids: None,
            names: Some(vec![Key::Stored]),
            expected: Verdict::Served,
        },
        Row {
            case: "names only, an unknown and a stored name",
            ids: None,
            names: Some(vec![Key::Unknown, Key::Stored]),
            expected: Verdict::Served,
        },
    ];

    for Row {
        case: row,
        ids,
        names,
        expected,
    } in &rows
    {
        // The one implementation of the rule agrees with the row's expectation, read off
        // the same selectors the requests below carry.
        let probe_ids = ids.as_ref().map(|keys| readers[0].ids(keys));
        let probe_names = names.as_ref().map(|keys| readers[0].names(keys));
        let stated = batch_selector_refusal(probe_ids.as_deref(), probe_names.as_deref());
        let expected_message = match expected {
            Verdict::Served => None,
            Verdict::Refused(message) => Some(*message),
        };
        assert_eq!(
            stated, expected_message,
            "case {row}: expected the shared rule to answer {expected_message:?}, got {stated:?}"
        );

        // Every reader's answer to this row, so the rows that are refused are compared
        // across readers below.
        let mut answers: Vec<(&str, u16, String, String)> = Vec::new();
        for reader in &readers {
            let operation = reader.operation;
            let body = reader.body(ids.as_deref(), names.as_deref());
            let (status, code, answer) = call(&router, operation, body).await;
            match expected {
                Verdict::Refused(message) => {
                    assert_eq!(
                        (status, code.as_str(), text(&answer, "message").as_str()),
                        (CLIENT_ERROR, "ValidationException", *message),
                        "case {operation}({row}): expected {CLIENT_ERROR} ValidationException \
                         {message:?}, got {status} {code} with {answer}"
                    );
                }
                Verdict::Served => {
                    assert_eq!(
                        status, 200,
                        "case {operation}({row}): expected 200, got {status} {code} with {answer}"
                    );
                    // A served row answers every entry it carried: the stored keys as
                    // details and the unknown ones as error details.
                    let keys: &[Key] = ids.as_deref().or(names.as_deref()).unwrap_or(&[]);
                    let stored = keys
                        .iter()
                        .filter(|key| matches!(key, Key::Stored))
                        .count();
                    let unknown = keys.len() - stored;
                    assert_eq!(
                        (count(&answer, reader.details), count(&answer, reader.errors)),
                        (stored, unknown),
                        "case {operation}({row}): expected {stored} details and {unknown} \
                         error details, got {answer}"
                    );
                }
            }
            answers.push((operation, status, code, text(&answer, "message")));
        }

        // A refusal is the rule's, not the resource's: both readers answer the same status,
        // the same error shape and the same sentence for the same shape of request.
        if matches!(expected, Verdict::Refused(_)) {
            let (first_op, first_status, first_code, first_message) = answers[0].clone();
            for (operation, status, code, message) in &answers[1..] {
                assert_eq!(
                    (*status, code.as_str(), message.as_str()),
                    (first_status, first_code.as_str(), first_message.as_str()),
                    "case {row}: {operation} must answer exactly as {first_op} does, got \
                     {status} {code} {message:?} against {first_status} {first_code} \
                     {first_message:?}"
                );
            }
        }
    }
}
