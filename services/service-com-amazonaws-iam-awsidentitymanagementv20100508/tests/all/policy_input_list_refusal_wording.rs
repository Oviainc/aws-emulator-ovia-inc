// SPDX-License-Identifier: CC0-1.0

//! How the operations that take a LIST of policy documents refuse an entry they cannot read
//! as a policy.
//!
//! The rule under test: an entry of `PolicyInputList` that is not the "complete, valid JSON
//! text of an IAM policy" the member documents is refused `InvalidInput` (HTTP 400) with the
//! message naming the offending entry by its POSITION in that list, counted from one — the
//! only variable part of the sentence, and a value taken from the request. The member
//! carries several documents, so a caller submitting several in one call is told WHICH one
//! was rejected; an operation taking a SINGLE policy document has nothing to position and
//! answers its own fixed sentence instead.
//!
//! The suite is a TABLE whose rows differ along what the rule generalises over: the position
//! of the offending entry, how many entries the list carries, the way the entry fails to be
//! a policy document (not JSON at all, truncated JSON, a JSON value that is not an object,
//! an object describing no statement), and which of the four operations that take the member
//! was called. No single request is the specification.
//!
//! The awsQuery harness is the one the other suites in this crate share.

use smithy_server_runtime::MultiServiceRouter;

use crate::request_condition_context::{create_user, user_arn};
use crate::user_lifecycle::{call, element, error_code, router};

/// The error code every refusal here carries (`InvalidInputException`, `awsQueryError` code
/// `InvalidInput`, HTTP 400).
const INVALID_INPUT: &str = "InvalidInput";

/// A document every row's list carries beside the offending one: a policy the operations
/// read without complaint, so the refusal is about the entry the row positions and not about
/// the list being short.
const READABLE: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:ListBucket","Resource":"*"}]}"#;

/// One of the operations that take a `PolicyInputList`, with the other parameters one call
/// of it needs.
///
/// The two simulations take the list beside the actions they decide; the two context-key
/// readers take it alone. The `Principal` spellings of both take the entity whose own
/// policies join the request's, named by ARN.
struct Operation {
    /// The awsQuery `Action` the operation is called under.
    name: &'static str,
    /// Whether the operation names a principal whose own policies take part.
    names_a_principal: bool,
    /// Whether the operation decides actions, and so takes an `ActionNames` list.
    decides_actions: bool,
}

/// Every operation that takes the member, so each row is asked of all of them.
const OPERATIONS: &[Operation] = &[
    Operation {
        name: "SimulateCustomPolicy",
        names_a_principal: false,
        decides_actions: true,
    },
    Operation {
        name: "SimulatePrincipalPolicy",
        names_a_principal: true,
        decides_actions: true,
    },
    Operation {
        name: "GetContextKeysForCustomPolicy",
        names_a_principal: false,
        decides_actions: false,
    },
    Operation {
        name: "GetContextKeysForPrincipalPolicy",
        names_a_principal: true,
        decides_actions: false,
    },
];

/// One call of `operation` carrying `documents` in `PolicyInputList`.
async fn call_with_documents(
    router: &MultiServiceRouter,
    operation: &Operation,
    principal_arn: &str,
    documents: &[&str],
) -> (u16, String) {
    let mut owned: Vec<(String, String)> = Vec::new();
    for (position, document) in (1..).zip(documents) {
        owned.push((
            format!("PolicyInputList.member.{position}"),
            (*document).to_string(),
        ));
    }
    if operation.decides_actions {
        owned.push((
            "ActionNames.member.1".to_string(),
            "s3:ListBucket".to_string(),
        ));
    }
    if operation.names_a_principal {
        owned.push(("PolicySourceArn".to_string(), principal_arn.to_string()));
    }
    let params: Vec<(&str, &str)> = owned
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    call(router, operation.name, &params).await
}

/// One row of the table: the list a request carries, and the position of the entry the
/// refusal must name.
struct ListCase {
    case: &'static str,
    /// The entries of `PolicyInputList`, in the order the request carries them.
    documents: &'static [&'static str],
    /// The 1-based position the refusal must name.
    position: usize,
}

/// A value that is not JSON at all.
const NOT_JSON: &str = "{not valid json";

/// A value whose JSON ran out of input part way through a statement list.
const TRUNCATED: &str = r#"{"Version": "2012-10-17", "Statement": ["#;

/// A value that IS JSON, and is not an object, so it can carry none of the policy grammar's
/// top-level elements.
const NOT_AN_OBJECT: &str = r#"["Statement"]"#;

/// A value that is a JSON object and describes no statement — the one element the IAM JSON
/// policy grammar requires — so it is no more readable as a policy than the others.
const NO_STATEMENT: &str = r#"{"Version":"2012-10-17"}"#;

/// Rows spanning the position of the offending entry in lists of one, two and three, and
/// each way an entry fails to be a policy document.
const LIST_CASES: &[ListCase] = &[
    ListCase {
        case: "the only entry, which is not JSON",
        documents: &[NOT_JSON],
        position: 1,
    },
    ListCase {
        case: "the only entry, whose JSON is truncated",
        documents: &[TRUNCATED],
        position: 1,
    },
    ListCase {
        case: "the only entry, which is JSON that is not an object",
        documents: &[NOT_AN_OBJECT],
        position: 1,
    },
    ListCase {
        case: "the only entry, an object describing no statement",
        documents: &[NO_STATEMENT],
        position: 1,
    },
    ListCase {
        case: "the second of two entries",
        documents: &[READABLE, TRUNCATED],
        position: 2,
    },
    ListCase {
        case: "the first of two entries",
        documents: &[NOT_JSON, READABLE],
        position: 1,
    },
    ListCase {
        case: "the third of three entries",
        documents: &[READABLE, READABLE, NOT_AN_OBJECT],
        position: 3,
    },
    ListCase {
        case: "the middle of three entries",
        documents: &[READABLE, NO_STATEMENT, READABLE],
        position: 2,
    },
    ListCase {
        case: "two offending entries, of which the first is named",
        documents: &[READABLE, NOT_JSON, TRUNCATED],
        position: 2,
    },
];

/// The message a refusal of the entry at `position` carries.
fn expected_message(position: usize) -> String {
    format!("Policy input list item {position} has invalid content")
}

/// RULE: an entry of `PolicyInputList` that is not a policy document is refused
/// `InvalidInput` naming that entry's 1-based position in the list, whichever operation took
/// the list and however the entry fails to be a document.
#[tokio::test]
async fn an_unreadable_policy_input_list_entry_is_refused_by_its_position() {
    let router = router();
    let principal = "list-item-principal";
    create_user(&router, "the named principal", principal, &[]).await;
    let principal_arn = user_arn(principal);

    for operation in OPERATIONS {
        for case in LIST_CASES {
            let (status, body) =
                call_with_documents(&router, operation, &principal_arn, case.documents).await;
            let code = error_code(&body);
            assert_eq!(
                (status, code.as_str()),
                (400, INVALID_INPUT),
                "case {:?}/{:?}: expected HTTP 400 {INVALID_INPUT}, got {status} {code}: {body}",
                operation.name,
                case.case
            );
            let message = element(&body, "Message");
            let expected = expected_message(case.position);
            assert_eq!(
                message, expected,
                "case {:?}/{:?}: expected the message {expected:?}, got {message:?}: {body}",
                operation.name,
                case.case
            );
        }
    }
}

/// RULE: a list whose every entry IS a policy document is not refused on this ground at all
/// — the refusal is about the entry, not about the member being present.
#[tokio::test]
async fn a_list_of_readable_entries_is_not_refused() {
    let router = router();
    let principal = "list-item-readable";
    create_user(&router, "the named principal", principal, &[]).await;
    let principal_arn = user_arn(principal);

    for operation in OPERATIONS {
        for count in 1..=3 {
            let documents = vec![READABLE; count];
            let (status, body) =
                call_with_documents(&router, operation, &principal_arn, &documents).await;
            assert_eq!(
                status, 200,
                "case {:?}/{count} readable entries: expected 200, got {status}: {body}",
                operation.name
            );
        }
    }
}
