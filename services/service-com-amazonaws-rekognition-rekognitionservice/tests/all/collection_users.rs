// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the USERS of a Rekognition collection over the wire
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: RekognitionService.<Operation>`),
//! dispatched through the same router the production emulator mounts.
//!
//! Every assertion goes over HTTP and reads only the status, the wire error code and the
//! JSON body: nothing reaches into the service's state struct, so a response that is not
//! computed from persisted state cannot pass.
//!
//! Each test states a RULE and asserts it over a TABLE of inputs:
//!
//! * `CreateUser` records a user under the `UserId` the caller supplied in the
//!   collection it named, and that record is what every reader answers from: `ListUsers`
//!   reports the user with the `UserStatus` of a user nothing has been associated with
//!   yet, and `DescribeCollection` counts it. The rows span the published `UserId`
//!   shape — every character class its pattern allows, and both ends of its length
//!   range — and two collections, so a user belongs to the collection it was created in;
//! * a create is idempotent on its `ClientRequestToken`: the same token with the same
//!   request completes the create ONCE, the same token with a different request is
//!   refused as a parameter mismatch, and a `UserId` the collection already holds is a
//!   conflict however it is reached;
//! * a user is created only in a collection that exists;
//! * a `ListUsers` page is cut out of the collection's own order, `MaxResults` caps it,
//!   a `NextToken` resumes exactly where the page ended, and a token this list did not
//!   mint names no position;
//! * the store is bounded: past a collection's user capacity a create is refused with
//!   the modelled quota error and stores nothing.

use serde_json::{json, Value};
use service_com_amazonaws_rekognition_rekognitionservice::state;

// The wire harness is the collection suite's, so a request is built one way whichever
// operation is being exercised.
use crate::face_collections::{call, harness, Reply};

/// `MaxResults` is `MaxUserResults`, `range 1..500` on `ListUsers`
/// (`smithy_query(query="operation",operation="ListUsers")`).
const MAX_USER_RESULTS_MIN: i64 = 1;
const MAX_USER_RESULTS_MAX: i64 = 500;

/// The `UserId` length range, `len 1..128`
/// (`smithy_query(query="shape",shape="UserId")`).
const USER_ID_LEN_MIN: usize = 1;
const USER_ID_LEN_MAX: usize = 128;

/// The `UserStatus` a user carries before any face is associated with it. `UserStatus`
/// is ACTIVE | UPDATING | CREATING | CREATED
/// (`smithy_query(query="shape",shape="UserStatus")`), and a user a create has just
/// recorded is created.
const CREATED: &str = "CREATED";

async fn create_collection(
    router: &smithy_server_runtime::MultiServiceRouter,
    collection_id: &str,
) {
    let reply = call(
        router,
        "CreateCollection",
        json!({ "CollectionId": collection_id }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {collection_id}: the collection must be created, got {:?}",
        reply.body
    );
}

/// `CreateUser` in `collection_id` for `user_id`, with `token` as its
/// `ClientRequestToken` when one is given.
async fn create_user(
    router: &smithy_server_runtime::MultiServiceRouter,
    collection_id: &str,
    user_id: &str,
    token: Option<&str>,
) -> Reply {
    let mut body = json!({ "CollectionId": collection_id, "UserId": user_id });
    if let Some(token) = token {
        body["ClientRequestToken"] = json!(token);
    }
    call(router, "CreateUser", body).await
}

/// Every `(UserId, UserStatus)` `ListUsers` reports for `collection_id`, walking the
/// pages to the end.
async fn list_users(
    router: &smithy_server_runtime::MultiServiceRouter,
    collection_id: &str,
) -> Vec<(String, String)> {
    let mut users = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut body = json!({ "CollectionId": collection_id });
        if let Some(cursor) = &token {
            body["NextToken"] = json!(cursor);
        }
        let reply = call(router, "ListUsers", body).await;
        assert_eq!(
            reply.status, 200,
            "case {collection_id}: listing the users must succeed, got {:?}",
            reply.body
        );
        users.extend(reply.list("Users").iter().map(user_entry));
        token = reply.string("NextToken");
        if token.is_none() {
            return users;
        }
        // A page carries at least one user while users remain, so the walk cannot need
        // more pages than the collection may hold: a handler that minted a cursor
        // forever fails here instead of hanging the run.
        pages += 1;
        assert!(
            pages <= state::MAX_USERS_PER_COLLECTION,
            "case {collection_id}: paging must terminate, walked {} user(s) over {pages} pages",
            users.len()
        );
    }
}

/// One reported `User`, as the pair a caller branches on.
fn user_entry(user: &Value) -> (String, String) {
    (
        user.get("UserId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        user.get("UserStatus")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    )
}

/// The `UserCount` `DescribeCollection` reports for `collection_id`.
async fn user_count(
    router: &smithy_server_runtime::MultiServiceRouter,
    collection_id: &str,
) -> Option<i64> {
    call(
        router,
        "DescribeCollection",
        json!({ "CollectionId": collection_id }),
    )
    .await
    .field("UserCount")
    .and_then(Value::as_i64)
}

#[tokio::test]
async fn a_created_user_is_the_user_every_reader_reports() {
    let router = harness();
    create_collection(&router, "people").await;
    create_collection(&router, "others").await;

    // Rows span the published `UserId` shape rather than one example: every character
    // class `^[a-zA-Z0-9_.\-:]+$` allows, and both ends of `len 1..128`.
    let cases: Vec<String> = vec![
        "alice".to_string(),
        "Bob".to_string(),
        "user_42".to_string(),
        "first.last".to_string(),
        "team-east".to_string(),
        "ns:member".to_string(),
        "A".repeat(USER_ID_LEN_MIN),
        "u".repeat(USER_ID_LEN_MAX),
    ];

    for user_id in &cases {
        let created = create_user(&router, "people", user_id, None).await;
        assert_eq!(
            created.status, 200,
            "case {user_id:?}: creating the user must succeed, got {:?}",
            created.body
        );
        // `CreateUserResponse` models no members, so the answer is the bare success:
        // what the create recorded is read back below, never echoed here.
        assert!(
            created.field("UserId").is_none(),
            "case {user_id:?}: the create answers with no members, got {:?}",
            created.body
        );
    }

    // Every created user, and only those, is reported — each with the status of a user
    // no face has been associated with yet.
    let mut expected: Vec<(String, String)> = cases
        .iter()
        .map(|user_id| (user_id.clone(), CREATED.to_string()))
        .collect();
    expected.sort();
    let reported = list_users(&router, "people").await;
    let mut sorted = reported.clone();
    sorted.sort();
    assert_eq!(
        sorted, expected,
        "every created user must be reported with its status, got {reported:?}"
    );
    assert_eq!(
        user_count(&router, "people").await,
        Some(cases.len() as i64),
        "case people: the described user count must be the number of users created"
    );

    // A user belongs to the collection it was created in: the other collection reports
    // none of them, and the same id created there is a second, independent user.
    assert_eq!(
        list_users(&router, "others").await,
        Vec::new(),
        "case others: a collection nothing was created in holds no user"
    );
    assert_eq!(
        user_count(&router, "others").await,
        Some(0),
        "case others: a collection nothing was created in counts no user"
    );
    assert_eq!(
        create_user(&router, "others", "alice", None).await.status,
        200,
        "case others/alice: the same id in another collection is another user"
    );
    assert_eq!(
        list_users(&router, "others").await,
        vec![("alice".to_string(), CREATED.to_string())],
        "case others: the other collection reports only its own user"
    );
}

/// One row of the idempotency table: the request a LATER call makes, and the answer the
/// rule requires for it.
struct TokenCase {
    case: &'static str,
    collection_id: &'static str,
    user_id: &'static str,
    token: Option<&'static str>,
    status: u16,
    error_code: &'static str,
    message: &'static str,
}

#[tokio::test]
async fn a_create_completes_once_per_client_request_token() {
    let router = harness();
    create_collection(&router, "people").await;
    create_collection(&router, "others").await;

    // The token that commits the first request. Every row below is about how a LATER
    // request relates to it.
    let committed = "token-one";
    assert_eq!(
        create_user(&router, "people", "alice", Some(committed))
            .await
            .status,
        200,
        "the first create must succeed"
    );

    // Rows: the same token with the same request (a retry, which completes the create
    // only once); the same token with a different user, and with a different
    // collection (a parameter mismatch); a different token, and no token at all, for a
    // user the collection already holds (a conflict). Each refusal names the class and
    // the text the service words that class with.
    let cases = vec![
        TokenCase {
            case: "same token, same request",
            collection_id: "people",
            user_id: "alice",
            token: Some(committed),
            status: 200,
            error_code: "",
            message: "",
        },
        TokenCase {
            case: "same token, other user",
            collection_id: "people",
            user_id: "carol",
            token: Some(committed),
            status: 400,
            error_code: "IdempotentParameterMismatchException",
            // Spelled out, not read from the implementation's own constant: the text a
            // recorded AWS refusal of a reused token carries.
            message: "Service Unavailable",
        },
        TokenCase {
            case: "same token, other collection",
            collection_id: "others",
            user_id: "alice",
            token: Some(committed),
            status: 400,
            error_code: "IdempotentParameterMismatchException",
            message: "Service Unavailable",
        },
        TokenCase {
            case: "other token, existing user",
            collection_id: "people",
            user_id: "alice",
            token: Some("token-two"),
            status: 400,
            error_code: "ConflictException",
            message: "The user id: alice already exists",
        },
        TokenCase {
            case: "no token, existing user",
            collection_id: "people",
            user_id: "alice",
            token: None,
            status: 400,
            error_code: "ConflictException",
            message: "The user id: alice already exists",
        },
    ];

    for TokenCase {
        case,
        collection_id,
        user_id,
        token,
        status,
        error_code,
        message,
    } in cases
    {
        let reply = create_user(&router, collection_id, user_id, token).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (status, error_code),
            "case {case:?}: expected {status} {error_code:?}, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.string("Message").unwrap_or_default(),
            message,
            "case {case:?}: expected the message {message:?}, got {:?}",
            reply.body
        );
        // Whatever the answer, the store still holds exactly the one user the one
        // completed create recorded: a retry creates no second user, and a refusal
        // creates none at all.
        assert_eq!(
            list_users(&router, "people").await,
            vec![("alice".to_string(), CREATED.to_string())],
            "case {case:?}: the completed create must have recorded exactly one user"
        );
        assert_eq!(
            user_count(&router, "people").await,
            Some(1),
            "case {case:?}: the described user count must still be one"
        );
    }

    // A refused request commits nothing, so the token it was refused under is still
    // free: used for a request that CAN be carried out, it carries it out.
    assert_eq!(
        create_user(&router, "people", "carol", Some("token-two"))
            .await
            .status,
        200,
        "case token-two/carol: a token a refusal never committed stays usable"
    );
    assert_eq!(
        list_users(&router, "people").await,
        vec![
            ("alice".to_string(), CREATED.to_string()),
            ("carol".to_string(), CREATED.to_string()),
        ],
        "both completed creates must be reported"
    );
}

#[tokio::test]
async fn a_user_is_created_only_in_a_collection_that_exists() {
    let router = harness();
    create_collection(&router, "people").await;

    // Rows: a collection id nothing created, and one created and then deleted. Both
    // name no collection, so neither can hold a user.
    call(
        &router,
        "CreateCollection",
        json!({ "CollectionId": "gone" }),
    )
    .await;
    call(
        &router,
        "DeleteCollection",
        json!({ "CollectionId": "gone" }),
    )
    .await;

    for collection_id in ["missing", "gone"] {
        let reply = create_user(&router, collection_id, "alice", None).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "ResourceNotFoundException"),
            "case {collection_id}: a user cannot be created in a collection that does not \
             exist, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.string("Message").as_deref(),
            Some(format!("The collection id: {collection_id} does not exist").as_str()),
            "case {collection_id}: the refusal names the collection the request asked for, \
             got {:?}",
            reply.body
        );
        let listed = call(
            &router,
            "ListUsers",
            json!({ "CollectionId": collection_id }),
        )
        .await;
        assert_eq!(
            (listed.status, listed.error_code().as_str()),
            (400, "ResourceNotFoundException"),
            "case {collection_id}: listing a collection that does not exist is refused the \
             same way, got {:?}",
            listed.body
        );
    }

    // The live collection is untouched by either refusal.
    assert_eq!(
        list_users(&router, "people").await,
        Vec::new(),
        "a refused create must store nothing"
    );
}

#[tokio::test]
async fn the_users_page_is_cut_out_of_the_collections_own_order() {
    let router = harness();
    create_collection(&router, "people").await;

    let created = [
        "alice", "bob", "carol", "dave", "erin", "frank", "grace",
    ];
    for user_id in created {
        create_user(&router, "people", user_id, None).await;
    }
    let order: Vec<String> = list_users(&router, "people")
        .await
        .into_iter()
        .map(|(user_id, _)| user_id)
        .collect();
    assert_eq!(
        order.len(),
        created.len(),
        "every created user must be reported once, got {order:?}"
    );

    // Rows differ along the page size: one user per page, through more than the
    // collection holds, plus both published bounds of `MaxUserResults` and "no
    // MaxResults at all".
    for max_results in [
        Some(MAX_USER_RESULTS_MIN),
        Some(2),
        Some(3),
        Some(7),
        Some(11),
        Some(MAX_USER_RESULTS_MAX),
        None,
    ] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({ "CollectionId": "people" });
            if let Some(max) = max_results {
                body["MaxResults"] = json!(max);
            }
            if let Some(cursor) = &token {
                body["NextToken"] = json!(cursor);
            }
            let reply = call(&router, "ListUsers", body).await;
            assert_eq!(
                reply.status, 200,
                "case MaxResults={max_results:?}: page {pages} must succeed, got {:?}",
                reply.body
            );
            let page: Vec<String> = reply
                .list("Users")
                .iter()
                .map(|user| user_entry(user).0)
                .collect();
            if let Some(max) = max_results {
                assert!(
                    page.len() as i64 <= max,
                    "case MaxResults={max_results:?}: a page must not exceed the cap, got {} \
                     users",
                    page.len()
                );
            }
            walked.extend(page);
            pages += 1;
            token = reply.string("NextToken");
            assert_eq!(
                token.is_some(),
                walked.len() < order.len(),
                "case MaxResults={max_results:?}: a NextToken must come back exactly while \
                 users remain, walked {} of {}",
                walked.len(),
                order.len()
            );
            if token.is_none() {
                break;
            }
            assert!(
                pages <= order.len() + 1,
                "case MaxResults={max_results:?}: paging must terminate, walked {walked:?}"
            );
        }
        assert_eq!(
            walked, order,
            "case MaxResults={max_results:?}: the pages must walk the collection's order \
             exactly once"
        );
    }

    // A page resumes where the previous one ended whatever happens in between: a user
    // created after the cursor was minted is reported by the resumed page when it falls
    // after the position the cursor names.
    let first = call(
        &router,
        "ListUsers",
        json!({ "CollectionId": "people", "MaxResults": 3 }),
    )
    .await;
    let cursor = first
        .string("NextToken")
        .expect("a further page exists after the first three users");
    create_user(&router, "people", "zoe", None).await;
    let resumed = call(
        &router,
        "ListUsers",
        json!({ "CollectionId": "people", "NextToken": cursor }),
    )
    .await;
    let after: Vec<String> = resumed
        .list("Users")
        .iter()
        .map(|user| user_entry(user).0)
        .collect();
    assert_eq!(
        after,
        order[3..]
            .iter()
            .cloned()
            .chain(["zoe".to_string()])
            .collect::<Vec<String>>(),
        "a cursor resumes at the position it named, got {:?}",
        resumed.body
    );

    // Rows: a token that is not hex at all, hex that decodes to nothing this service
    // minted, and another list's cursor — none names a position in this list.
    let faces_cursor = state::encode_sequence_cursor(0);
    for token in ["not-a-token", "00", faces_cursor.as_str()] {
        let reply = call(
            &router,
            "ListUsers",
            json!({ "CollectionId": "people", "NextToken": token }),
        )
        .await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "InvalidPaginationTokenException"),
            "case {token:?}: a token this list did not mint must be refused, got {:?}",
            reply.body
        );
    }

    // One past each published bound of `MaxUserResults` is outside the model's range,
    // so it is refused rather than served.
    for max_results in [MAX_USER_RESULTS_MIN - 1, MAX_USER_RESULTS_MAX + 1] {
        let reply = call(
            &router,
            "ListUsers",
            json!({ "CollectionId": "people", "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case MaxResults={max_results}: a page size outside the modelled range must be \
             refused, got {:?}",
            reply.body
        );
    }
}

#[tokio::test]
async fn the_store_refuses_to_grow_past_its_user_capacity() {
    let router = harness();
    create_collection(&router, "full").await;

    for index in 0..state::MAX_USERS_PER_COLLECTION {
        let user_id = format!("user-{index:05}");
        assert_eq!(
            create_user(&router, "full", &user_id, None).await.status,
            200,
            "case {user_id}: creating up to the capacity must succeed"
        );
    }
    let over = create_user(&router, "full", "one-too-many", None).await;
    assert_eq!(
        (over.status, over.error_code().as_str()),
        (400, "ServiceQuotaExceededException"),
        "case users={}: creating past the capacity must be refused, got {:?}",
        state::MAX_USERS_PER_COLLECTION,
        over.body
    );
    assert_eq!(
        user_count(&router, "full").await,
        Some(state::MAX_USERS_PER_COLLECTION as i64),
        "a refused create must store nothing"
    );
}
