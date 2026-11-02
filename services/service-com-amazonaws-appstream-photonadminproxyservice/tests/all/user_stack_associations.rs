// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for AppStream's user/stack associations over the wire.
//!
//! Every case drives the awsJson1_1 front door through the shared
//! [`Emulator`](crate::stack_fleet_lifecycle::Emulator) harness, so what is exercised is
//! the dispatch → deserialize → handle → serialize walk.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! recorded request:
//!
//! * associating a batch of users with stacks persists one edge per association, so a
//!   later read reports every one of them exactly as the batch gave it — whatever the
//!   batch's size or shape;
//! * the edge is the `(AuthenticationType, UserName, StackName)` identity, so
//!   associating the same pair twice is idempotent and the read still reports one
//!   association per pair;
//! * an association naming a stack or a user the account does not hold is NOT applied and
//!   is reported in the result's `errors` list with the modelled error code for what was
//!   missing, while the rest of the batch is applied and the call answers HTTP 200;
//! * disassociating removes exactly the edges it names and leaves the others, and
//!   removing an edge that is already absent is not an error;
//! * the association operations manage USERPOOL identities only, so any other
//!   authentication type is refused with `InvalidParameterCombinationException` (400);
//! * a read must be given either a stack name or a user identity, reports only stored
//!   associations, and `MaxResults` sizes its page: following `NextToken` walks every
//!   matching association exactly once;
//! * a request outside a modelled constraint (a page smaller than one record, a token
//!   this service never issued) is refused and persists nothing, and so is an association
//!   past the store's own ceiling.

use serde_json::{json, Value};

use crate::refusal_messages::refusal;
use crate::stack_fleet_lifecycle::{assert_error, Emulator};

/// The account every emulated resource belongs to.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The only authentication type the user pool operations accept, which
/// `DescribeUserStackAssociations.AuthenticationType` documents as "You must specify
/// USERPOOL".
const USER_POOL: &str = "USERPOOL";

/// The other authentication types the model declares (`AuthenticationType`'s values, kept
/// as `AUTHENTICATION_TYPE_VALUES` in `src/ops/types/shared_types.rs`). None of them names
/// a user pool identity, so these operations refuse every one.
const OTHER_AUTHENTICATION_TYPES: &[&str] = &["API", "AWS_AD", "SAML"];

/// The bounds the model puts on `UserStackAssociationList` (len 1..25): the largest batch
/// one call admits, and the most associations one page of the read can carry.
const MAX_BATCH: usize = 25;

/// The maximum length the model puts on `Username` (len 1..128).
const USER_NAME_MAX: usize = 128;

/// AWS's sentence for a user pool username that is not in the pool. It has no variable
/// part, so the refusal echoes nothing back.
const USER_NOT_FOUND_MESSAGE: &str = "A user with that username does not exist.";

/// The sentence a read naming neither a stack nor a user identity is refused with.
const MISSING_FILTER_MESSAGE: &str = "You must specify either the stack name, or the user \
     name and the authentication type for the user.";

/// The emulator's memory ceiling, read from the crate so the test tracks the constant
/// rather than a copy of its value.
const CEILING: usize =
    service_com_amazonaws_appstream_photonadminproxyservice::state::MAX_RECORDS_PER_STORE;

/// AWS's `ResourceNotFoundException` sentence for a named resource the account does not
/// hold, which is also the message a batch reports a missing stack with.
fn not_found_sentence(kind: &str, name: &str) -> String {
    format!("The {kind} {name} for account {ACCOUNT} does not exist.")
}

/// The sentence the store reports when it stands at its ceiling.
fn limit_sentence(store: &str) -> String {
    format!(
        "The requested limit exceeds the permitted limit for an account: this emulator \
         holds at most {CEILING} {store}"
    )
}

/// One `UserStackAssociation` as a request carries it.
fn association(stack: &str, user: &str, send_email_notification: Option<bool>) -> Value {
    let mut item = json!({
        "StackName": stack,
        "UserName": user,
        "AuthenticationType": USER_POOL,
    });
    if let Some(flag) = send_email_notification {
        item["SendEmailNotification"] = json!(flag);
    }
    item
}

/// A string member of a reported association.
fn member(association: &Value, name: &str) -> String {
    association
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Associations in a stable order, so two collections of them compare as sets.
fn sorted(mut associations: Vec<Value>) -> Vec<Value> {
    associations.sort_by_key(|association| {
        (member(association, "UserName"), member(association, "StackName"))
    });
    associations
}

/// Create a stack the associations can name.
async fn create_stack(emulator: &Emulator, case: &str, name: &str) {
    emulator.ok(case, "CreateStack", json!({ "Name": name })).await;
}

/// Create a user pool user the associations can name.
async fn create_user(emulator: &Emulator, case: &str, name: &str) {
    emulator
        .ok(
            case,
            "CreateUser",
            json!({
                "UserName": name,
                "AuthenticationType": USER_POOL,
                "MessageAction": "SUPPRESS",
            }),
        )
        .await;
}

/// Run one batch operation and return the `errors` list it reports.
async fn batch(emulator: &Emulator, case: &str, operation: &str, items: &[Value]) -> Vec<Value> {
    let payload = emulator
        .ok(case, operation, json!({ "UserStackAssociations": items }))
        .await;
    payload
        .get("errors")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: {operation} must report an errors list, got {payload}")
        })
        .clone()
}

/// One page of a read: the associations it reports, and the token it carries.
async fn describe(emulator: &Emulator, case: &str, request: Value) -> (Vec<Value>, Option<Value>) {
    let payload = emulator
        .ok(case, "DescribeUserStackAssociations", request)
        .await;
    let associations = payload
        .get("UserStackAssociations")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a UserStackAssociations list, got {payload}")
        })
        .clone();
    (associations, payload.get("NextToken").cloned())
}

/// Every association a read reports for `request`, following `NextToken` to the end.
async fn all_reported(emulator: &Emulator, case: &str, request: Value) -> Vec<Value> {
    let mut seen: Vec<Value> = Vec::new();
    let mut next_token: Option<Value> = None;
    loop {
        let mut page_request = request.clone();
        if let Some(token) = &next_token {
            page_request["NextToken"] = token.clone();
        }
        let (associations, token) = describe(emulator, case, page_request).await;
        assert!(
            associations.len() <= MAX_BATCH,
            "case {case:?}: a page holds at most {MAX_BATCH} associations, got {}",
            associations.len()
        );
        let page_len = associations.len();
        seen.extend(associations);
        match token {
            Some(token) => {
                assert!(
                    page_len > 0,
                    "case {case:?}: a page that carries a token must carry a record too"
                );
                next_token = Some(token);
            }
            None => break,
        }
    }
    seen
}

/// Every association stored for one stack.
async fn for_stack(emulator: &Emulator, case: &str, stack: &str) -> Vec<Value> {
    all_reported(emulator, case, json!({ "StackName": stack })).await
}

/// Every association stored for one user identity.
async fn for_user(emulator: &Emulator, case: &str, user: &str) -> Vec<Value> {
    all_reported(
        emulator,
        case,
        json!({ "UserName": user, "AuthenticationType": USER_POOL }),
    )
    .await
}

// ---------------------------------------------------------------------------
// The association table
// ---------------------------------------------------------------------------

/// One row of the association table: the resources it needs, and the batch it sends.
struct Batch {
    case: &'static str,
    stacks: Vec<String>,
    users: Vec<String>,
    items: Vec<Value>,
}

impl Batch {
    /// A row associating the cross product of its stacks and users, each association
    /// carrying the welcome-email flag `send_email_notification` gives it.
    fn cross(
        case: &'static str,
        stacks: &[&str],
        users: &[&str],
        send_email_notification: Option<bool>,
    ) -> Self {
        let items = users
            .iter()
            .flat_map(|user| {
                stacks
                    .iter()
                    .map(move |stack| association(stack, user, send_email_notification))
            })
            .collect();
        Self {
            case,
            stacks: stacks.iter().map(|stack| stack.to_string()).collect(),
            users: users.iter().map(|user| user.to_string()).collect(),
            items,
        }
    }
}

/// The association table. The rows differ along what the rule generalises over: how many
/// associations the batch carries, how they fan out (one stack to many users, one user to
/// many stacks, a full cross product), whether each asks for a welcome email, and the form
/// and size of the user names.
fn batches() -> Vec<Batch> {
    let longest_user_name = format!("{}@example.com", "a".repeat(USER_NAME_MAX - 12));
    let mut rows = vec![
        Batch::cross("one user, one stack, no email flag", &["alpha"], &["one@example.com"], None),
        Batch::cross(
            "welcome email requested",
            &["alpha"],
            &["notified@example.com"],
            Some(true),
        ),
        Batch::cross(
            "welcome email declined",
            &["alpha"],
            &["silent@example.com"],
            Some(false),
        ),
        Batch::cross(
            "one user, three stacks",
            &["alpha", "beta.9", "gamma_3-x"],
            &["fanned-out@example.com"],
            Some(true),
        ),
        Batch::cross(
            "three users, one stack",
            &["shared"],
            &["a@example.com", "b@example.com", "ユーザー@example.com"],
            None,
        ),
        Batch::cross(
            "the longest user name the model allows",
            &["alpha"],
            &[longest_user_name.as_str()],
            Some(false),
        ),
    ];
    // The largest batch the model admits: a 5 × 5 cross product is exactly 25
    // associations.
    let stacks: Vec<String> = (1..=5).map(|index| format!("stack-{index}")).collect();
    let users: Vec<String> = (1..=5).map(|index| format!("user-{index}@example.com")).collect();
    let mut largest = Batch::cross(
        "the largest batch the model admits",
        &stacks.iter().map(String::as_str).collect::<Vec<_>>(),
        &users.iter().map(String::as_str).collect::<Vec<_>>(),
        None,
    );
    assert_eq!(
        largest.items.len(),
        MAX_BATCH,
        "the largest row must be exactly the model's maximum, got {}",
        largest.items.len()
    );
    // Half the associations in the largest batch ask for a welcome email, so one batch
    // carries both forms of the member.
    for (index, item) in largest.items.iter_mut().enumerate() {
        if index % 2 == 0 {
            item["SendEmailNotification"] = json!(index % 4 == 0);
        }
    }
    rows.push(largest);
    rows
}

/// Create every stack and user a row names.
async fn provision(emulator: &Emulator, row: &Batch) {
    for stack in &row.stacks {
        create_stack(emulator, row.case, stack).await;
    }
    for user in &row.users {
        create_user(emulator, row.case, user).await;
    }
}

// ---------------------------------------------------------------------------
// Associating, and reading the associations back
// ---------------------------------------------------------------------------

#[tokio::test]
async fn associating_persists_every_edge_a_later_read_reports() {
    for row in batches() {
        let case = row.case;
        let emulator = Emulator::new();
        provision(&emulator, &row).await;

        // Nothing is associated yet, so every read reports no association at all.
        for stack in &row.stacks {
            let before = for_stack(&emulator, case, stack).await;
            assert!(
                before.is_empty(),
                "case {case:?}: a stack with no associations must report none, got {before:?}"
            );
        }

        let errors = batch(&emulator, case, "BatchAssociateUserStack", &row.items).await;
        assert!(
            errors.is_empty(),
            "case {case:?}: every association names an existing stack and user, got {errors:?}"
        );

        // Read back by stack: every association the batch carried is reported, exactly as
        // the batch gave it.
        let mut by_stack: Vec<Value> = Vec::new();
        for stack in &row.stacks {
            by_stack.extend(for_stack(&emulator, case, stack).await);
        }
        assert_eq!(
            sorted(by_stack.clone()),
            sorted(row.items.clone()),
            "case {case:?}: the read must render the associations the batch persisted, got {:?}",
            sorted(by_stack.clone())
        );

        // Read back by user identity: each user's associations and no one else's.
        for user in &row.users {
            let reported = for_user(&emulator, case, user).await;
            let expected: Vec<Value> = row
                .items
                .iter()
                .filter(|item| member(item, "UserName") == *user)
                .cloned()
                .collect();
            assert_eq!(
                sorted(reported.clone()),
                sorted(expected),
                "case {case:?}: a read of one user must report that user's associations, \
                 got {reported:?}"
            );
        }

        // Both filters together name one association ("either or both").
        let first = row.items[0].clone();
        let narrowed = all_reported(
            &emulator,
            case,
            json!({
                "StackName": member(&first, "StackName"),
                "UserName": member(&first, "UserName"),
                "AuthenticationType": USER_POOL,
            }),
        )
        .await;
        assert_eq!(
            narrowed,
            vec![first.clone()],
            "case {case:?}: a read naming both must report exactly that association, \
             got {narrowed:?}"
        );

        // Reading again reports the same associations: the read changed nothing.
        let mut again: Vec<Value> = Vec::new();
        for stack in &row.stacks {
            again.extend(for_stack(&emulator, case, stack).await);
        }
        assert_eq!(
            sorted(again.clone()),
            sorted(by_stack),
            "case {case:?}: a read must not change what a later caller sees, got {again:?}"
        );
    }
}

#[tokio::test]
async fn associating_the_same_user_and_stack_twice_is_idempotent() {
    // Rows: what the repeat carries beside the identity — the same request again, or the
    // other welcome-email flag. The identity is the edge, so neither row adds a second
    // association.
    for (case, repeat_flag) in [
        ("identical request", Some(true)),
        ("the other welcome-email flag", Some(false)),
        ("no welcome-email flag", None),
    ] {
        let emulator = Emulator::new();
        create_stack(&emulator, case, "alpha").await;
        create_user(&emulator, case, "repeat@example.com").await;

        let first = association("alpha", "repeat@example.com", Some(true));
        let errors = batch(&emulator, case, "BatchAssociateUserStack", &[first]).await;
        assert!(errors.is_empty(), "case {case:?}: the first batch applies, got {errors:?}");

        let repeat = association("alpha", "repeat@example.com", repeat_flag);
        let errors = batch(
            &emulator,
            case,
            "BatchAssociateUserStack",
            std::slice::from_ref(&repeat),
        )
        .await;
        assert!(errors.is_empty(), "case {case:?}: a repeat applies too, got {errors:?}");

        let reported = for_stack(&emulator, case, "alpha").await;
        assert_eq!(
            reported,
            vec![repeat],
            "case {case:?}: the pair must be one association, carrying what the last \
             request gave it, got {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Per-item failures
// ---------------------------------------------------------------------------

/// The one member of an `errors` list, with its code and message.
fn only_error(case: &str, errors: &[Value]) -> (String, String, Value) {
    assert_eq!(
        errors.len(),
        1,
        "case {case:?}: exactly the association that could not be applied must be \
         reported, got {errors:?}"
    );
    let error = &errors[0];
    (
        member(error, "ErrorCode"),
        member(error, "ErrorMessage"),
        error
            .get("UserStackAssociation")
            .cloned()
            .unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn an_association_naming_a_missing_resource_is_reported_and_the_rest_applied() {
    // Rows: which side of the association the account does not hold. The last row has
    // neither side, and is reported against the stack it asked for — the resource the
    // user is being entitled to.
    let stored_stack = "stored";
    let stored_user = "stored@example.com";
    for (case, stack, user, code, message) in [
        (
            "unknown stack",
            "absent-stack",
            stored_user,
            "STACK_NOT_FOUND",
            not_found_sentence("stack", "absent-stack"),
        ),
        (
            "unknown user",
            stored_stack,
            "absent@example.com",
            "USER_NAME_NOT_FOUND",
            USER_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "neither side exists",
            "absent-stack",
            "absent@example.com",
            "STACK_NOT_FOUND",
            not_found_sentence("stack", "absent-stack"),
        ),
    ] {
        let emulator = Emulator::new();
        create_stack(&emulator, case, stored_stack).await;
        create_user(&emulator, case, stored_user).await;

        let failing = association(stack, user, Some(true));
        let applied = association(stored_stack, stored_user, None);
        let errors = batch(
            &emulator,
            case,
            "BatchAssociateUserStack",
            &[failing.clone(), applied.clone()],
        )
        .await;

        let (reported_code, reported_message, reported_association) = only_error(case, &errors);
        assert_eq!(
            reported_code, code,
            "case {case:?}: expected error code {code}, got {reported_code}"
        );
        assert_eq!(
            reported_message, message,
            "case {case:?}: expected message {message:?}, got {reported_message:?}"
        );
        assert_eq!(
            reported_association, failing,
            "case {case:?}: the failure must carry the association as the request gave \
             it, got {reported_association}"
        );
        if code == "USER_NAME_NOT_FOUND" {
            assert!(
                !reported_message.contains(user),
                "case {case:?}: the sentence states that no user has that username \
                 without echoing it, got {reported_message:?}"
            );
        }

        // The rest of the batch was applied, and the failing association was not.
        let reported = for_stack(&emulator, case, stored_stack).await;
        assert_eq!(
            reported,
            vec![applied],
            "case {case:?}: the associations that could be applied must be stored, \
             got {reported:?}"
        );
        let absent = for_user(&emulator, case, user).await;
        let expected_absent: Vec<Value> = if user == stored_user {
            for_user(&emulator, case, stored_user).await
        } else {
            Vec::new()
        };
        assert_eq!(
            absent, expected_absent,
            "case {case:?}: the association that was reported must not be stored, \
             got {absent:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Disassociating
// ---------------------------------------------------------------------------

#[tokio::test]
async fn disassociating_removes_exactly_the_edges_it_names() {
    let stacks = ["alpha", "beta"];
    let users = ["one@example.com", "two@example.com"];
    // Rows: which of the four stored associations the batch removes — one, several, all
    // four, and one that was never associated (already absent, so nothing to remove).
    for (case, removed) in [
        ("one of four", vec![(0_usize, 0_usize)]),
        ("one user from both stacks", vec![(0, 0), (1, 0)]),
        ("all four", vec![(0, 0), (1, 0), (0, 1), (1, 1)]),
        ("none of them", vec![]),
    ] {
        let emulator = Emulator::new();
        for stack in stacks {
            create_stack(&emulator, case, stack).await;
        }
        for user in users {
            create_user(&emulator, case, user).await;
        }
        let stored: Vec<Value> = users
            .iter()
            .flat_map(|user| stacks.iter().map(|stack| association(stack, user, Some(false))))
            .collect();
        let errors = batch(&emulator, case, "BatchAssociateUserStack", &stored).await;
        assert!(errors.is_empty(), "case {case:?}: the setup batch applies, got {errors:?}");

        let to_remove: Vec<Value> = removed
            .iter()
            .map(|(stack, user)| association(stacks[*stack], users[*user], None))
            .collect();
        if !to_remove.is_empty() {
            let errors = batch(
                &emulator,
                case,
                "BatchDisassociateUserStack",
                &to_remove,
            )
            .await;
            assert!(
                errors.is_empty(),
                "case {case:?}: every association names an existing stack, got {errors:?}"
            );
        }

        let expected: Vec<Value> = stored
            .iter()
            .filter(|item| {
                !to_remove.iter().any(|gone| {
                    member(gone, "StackName") == member(item, "StackName")
                        && member(gone, "UserName") == member(item, "UserName")
                })
            })
            .cloned()
            .collect();
        let mut reported: Vec<Value> = Vec::new();
        for stack in stacks {
            reported.extend(for_stack(&emulator, case, stack).await);
        }
        assert_eq!(
            sorted(reported.clone()),
            sorted(expected.clone()),
            "case {case:?}: only the named associations must be removed, got {reported:?}"
        );

        // Removing the same associations again removes nothing and reports nothing: an
        // edge that is already absent is what this call asks for.
        if !to_remove.is_empty() {
            let errors = batch(
                &emulator,
                case,
                "BatchDisassociateUserStack",
                &to_remove,
            )
            .await;
            assert!(
                errors.is_empty(),
                "case {case:?}: removing an absent edge is not an error, got {errors:?}"
            );
        }
        let mut after: Vec<Value> = Vec::new();
        for stack in stacks {
            after.extend(for_stack(&emulator, case, stack).await);
        }
        assert_eq!(
            sorted(after.clone()),
            sorted(expected),
            "case {case:?}: a repeated removal must change nothing, got {after:?}"
        );
    }
}

#[tokio::test]
async fn disassociating_from_a_stack_the_account_does_not_hold_is_reported() {
    // Rows: the stack the removal names. A stack the account does not hold is one no user
    // can be disassociated from, however the rest of the batch looks.
    let case = "unknown stack";
    let emulator = Emulator::new();
    create_stack(&emulator, case, "stored").await;
    create_user(&emulator, case, "stored@example.com").await;
    let stored = association("stored", "stored@example.com", None);
    batch(
        &emulator,
        case,
        "BatchAssociateUserStack",
        std::slice::from_ref(&stored),
    )
    .await;

    let failing = association("absent-stack", "stored@example.com", None);
    let errors = batch(
        &emulator,
        case,
        "BatchDisassociateUserStack",
        &[failing.clone(), stored.clone()],
    )
    .await;
    let (code, message, reported_association) = only_error(case, &errors);
    assert_eq!(
        code, "STACK_NOT_FOUND",
        "case {case:?}: expected error code STACK_NOT_FOUND, got {code}"
    );
    assert_eq!(
        message,
        not_found_sentence("stack", "absent-stack"),
        "case {case:?}: expected AWS's not-found sentence, got {message:?}"
    );
    assert_eq!(
        reported_association, failing,
        "case {case:?}: the failure must carry the association as the request gave it, \
         got {reported_association}"
    );

    // The association the batch could remove is gone.
    let reported = for_stack(&emulator, case, "stored").await;
    assert!(
        reported.is_empty(),
        "case {case:?}: the rest of the batch must still be removed, got {reported:?}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn only_user_pool_identities_can_be_associated() {
    for authentication_type in OTHER_AUTHENTICATION_TYPES {
        let case = format!("AuthenticationType {authentication_type}");
        let emulator = Emulator::new();
        create_stack(&emulator, &case, "alpha").await;
        create_user(&emulator, &case, "pooled@example.com").await;

        let elsewhere = json!({
            "StackName": "alpha",
            "UserName": "pooled@example.com",
            "AuthenticationType": authentication_type,
        });
        let expected = format!(
            "AuthenticationType must be {USER_POOL} for user pool operations, was \
             {authentication_type}"
        );
        for operation in ["BatchAssociateUserStack", "BatchDisassociateUserStack"] {
            let message = refusal(
                &emulator,
                &case,
                operation,
                json!({ "UserStackAssociations": [elsewhere.clone()] }),
                400,
                "InvalidParameterCombinationException",
            )
            .await;
            assert_eq!(
                message, expected,
                "case {case:?}: {operation} must say which authentication type it was \
                 given, got {message:?}"
            );
        }
        let message = refusal(
            &emulator,
            &case,
            "DescribeUserStackAssociations",
            json!({ "UserName": "pooled@example.com", "AuthenticationType": authentication_type }),
            400,
            "InvalidParameterCombinationException",
        )
        .await;
        assert_eq!(
            message, expected,
            "case {case:?}: the read must say which authentication type it was given, \
             got {message:?}"
        );

        // The refusals changed nothing, and a user pool association still works.
        let reported = for_stack(&emulator, &case, "alpha").await;
        assert!(
            reported.is_empty(),
            "case {case:?}: a refused batch must persist nothing, got {reported:?}"
        );
        let errors = batch(
            &emulator,
            &case,
            "BatchAssociateUserStack",
            &[association("alpha", "pooled@example.com", None)],
        )
        .await;
        assert!(errors.is_empty(), "case {case:?}: a USERPOOL batch applies, got {errors:?}");
        assert_eq!(
            for_stack(&emulator, &case, "alpha").await.len(),
            1,
            "case {case:?}: a user pool association must still be stored"
        );
    }
}

#[tokio::test]
async fn a_read_naming_neither_a_stack_nor_a_user_identity_is_refused() {
    // Rows: reads that name no identity the operation can answer about — nothing at all,
    // a user name with no authentication type, an authentication type with no user name,
    // and a page size but no identity.
    let emulator = Emulator::new();
    create_stack(&emulator, "setup", "alpha").await;
    create_user(&emulator, "setup", "stored@example.com").await;
    batch(
        &emulator,
        "setup",
        "BatchAssociateUserStack",
        &[association("alpha", "stored@example.com", None)],
    )
    .await;

    for (case, request) in [
        ("no member at all", json!({})),
        (
            "user name without authentication type",
            json!({ "UserName": "stored@example.com" }),
        ),
        (
            "authentication type without user name",
            json!({ "AuthenticationType": USER_POOL }),
        ),
        ("page size only", json!({ "MaxResults": 10 })),
    ] {
        let message = refusal(
            &emulator,
            case,
            "DescribeUserStackAssociations",
            request,
            400,
            "InvalidParameterCombinationException",
        )
        .await;
        assert_eq!(
            message, MISSING_FILTER_MESSAGE,
            "case {case:?}: expected {MISSING_FILTER_MESSAGE:?}, got {message:?}"
        );
    }

    // Every refusal left the stored association readable.
    assert_eq!(
        for_stack(&emulator, "after refusals", "alpha").await.len(),
        1,
        "case \"after refusals\": the stored association must still be reported"
    );
}

#[tokio::test]
async fn a_page_the_read_cannot_answer_with_is_refused() {
    let emulator = Emulator::new();
    create_stack(&emulator, "setup", "alpha").await;
    create_user(&emulator, "setup", "stored@example.com").await;
    batch(
        &emulator,
        "setup",
        "BatchAssociateUserStack",
        &[association("alpha", "stored@example.com", None)],
    )
    .await;

    // Rows: page sizes that cannot hold a record. Each is refused rather than answered
    // with an empty page that silently drops the stored associations.
    for max_results in [0_i64, -1, i64::from(i32::MIN)] {
        let case = format!("MaxResults {max_results}");
        assert_error(
            &emulator,
            &case,
            "DescribeUserStackAssociations",
            json!({ "StackName": "alpha", "MaxResults": max_results }),
            400,
            "ValidationException",
        )
        .await;
    }

    // Rows: tokens a client could hand-craft. The cursor is an opaque encoding this
    // service mints, so none of these is a token it issued.
    for token in ["alpha", "stored@example.com", "abc", "ff"] {
        let case = format!("NextToken {token:?}");
        assert_error(
            &emulator,
            &case,
            "DescribeUserStackAssociations",
            json!({ "StackName": "alpha", "NextToken": token }),
            400,
            "ValidationException",
        )
        .await;
    }

    assert_eq!(
        for_stack(&emulator, "after refusals", "alpha").await.len(),
        1,
        "case \"after refusals\": the stored association must still be reported"
    );
}

// ---------------------------------------------------------------------------
// Paging the read
// ---------------------------------------------------------------------------

/// The user names the pagination case associates: enough that every page size below cuts
/// the collection somewhere.
fn paged_user_names() -> Vec<String> {
    (1..=7).map(|index| format!("user-{index:02}@example.com")).collect()
}

#[tokio::test]
async fn following_next_token_reports_every_association_exactly_once() {
    let emulator = Emulator::new();
    create_stack(&emulator, "setup", "paged").await;
    let mut expected: Vec<Value> = Vec::new();
    for name in paged_user_names() {
        create_user(&emulator, "setup", &name).await;
        expected.push(association("paged", &name, None));
    }
    batch(&emulator, "setup", "BatchAssociateUserStack", &expected).await;

    // Rows: the page size the request asks for — sizes that cut the collection, one that
    // holds it exactly, the model's own page maximum, the largest `MaxResults` the model
    // admits (range 0..500), and a request that names no size at all.
    for max_results in [Some(1_i64), Some(2), Some(3), Some(7), Some(25), Some(500), None] {
        let case = format!("MaxResults {max_results:?}");
        let mut seen: Vec<Value> = Vec::new();
        let mut next_token: Option<Value> = None;
        loop {
            let mut request = json!({ "StackName": "paged" });
            if let Some(max_results) = max_results {
                request["MaxResults"] = json!(max_results);
            }
            if let Some(token) = &next_token {
                request["NextToken"] = token.clone();
            }
            let (associations, token) = describe(&emulator, &case, request).await;
            if let Some(max_results) = max_results {
                assert!(
                    i64::try_from(associations.len()).expect("a page fits in an i64") <= max_results,
                    "case {case:?}: a page must hold at most {max_results} associations, got {}",
                    associations.len()
                );
            }
            seen.extend(associations);
            match token {
                Some(token) => next_token = Some(token),
                None => break,
            }
        }
        assert_eq!(
            seen,
            expected,
            "case {case:?}: following NextToken must report every association exactly \
             once, in a stable order, got {seen:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The store's ceiling
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_association_store_refuses_an_edge_past_its_ceiling() {
    let case = "user/stack associations";
    let emulator = Emulator::new();
    for stack in ["filled", "spare"] {
        create_stack(&emulator, case, stack).await;
    }

    // Fill the association store to its ceiling, a batch at a time: one stack, one
    // association per user.
    let names: Vec<String> = (0..CEILING)
        .map(|index| format!("u{index:05}@example.com"))
        .collect();
    for name in &names {
        create_user(&emulator, case, name).await;
    }
    for chunk in names.chunks(MAX_BATCH) {
        let items: Vec<Value> = chunk
            .iter()
            .map(|name| association("filled", name, None))
            .collect();
        let errors = batch(&emulator, case, "BatchAssociateUserStack", &items).await;
        assert!(errors.is_empty(), "case {case:?}: filling the store applies, got {errors:?}");
    }

    // A NEW edge would grow the store past its ceiling, so it is reported and not stored.
    let refused = association("spare", &names[0], None);
    let errors = batch(
        &emulator,
        case,
        "BatchAssociateUserStack",
        std::slice::from_ref(&refused),
    )
    .await;
    let (code, message, reported_association) = only_error(case, &errors);
    assert_eq!(
        code, "INTERNAL_ERROR",
        "case {case:?}: expected error code INTERNAL_ERROR, got {code}"
    );
    assert_eq!(
        message,
        limit_sentence("user/stack associations"),
        "case {case:?}: expected the store's limit sentence, got {message:?}"
    );
    assert_eq!(
        reported_association, refused,
        "case {case:?}: the failure must carry the association as the request gave it, \
         got {reported_association}"
    );
    let spare = for_stack(&emulator, case, "spare").await;
    assert!(
        spare.is_empty(),
        "case {case:?}: the reported association must not be stored, got {spare:?}"
    );

    // An edge the store already holds inserts nothing, so it is never refused.
    let errors = batch(
        &emulator,
        case,
        "BatchAssociateUserStack",
        &[association("filled", &names[0], None)],
    )
    .await;
    assert!(
        errors.is_empty(),
        "case {case:?}: a repeat of a stored edge must still apply, got {errors:?}"
    );
}
