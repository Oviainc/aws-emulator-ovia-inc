// SPDX-License-Identifier: CC0-1.0

//! `DeleteSpendingLimit`, end to end over HTTP.
//!
//! The rules these suites pin:
//!
//! * A delete REMOVES the limit its `spendingLimitArn` names and nothing else:
//!   `SearchSpendingLimits` stops reporting that limit, every other stored limit is
//!   reported exactly as it was, and the device the limit was about is untouched.
//! * `spendingLimitArn` is the only thing that resolves the limit: an ARN naming no
//!   stored limit is `ResourceNotFoundException` (404), and a value that is not an ARN of
//!   a spending limit is `ValidationException` (400) — and neither touches the store.
//! * A removed limit stays removed: a second delete, and an update of the same ARN,
//!   resolve nothing, while a fresh create on the same device mints a new limit.
//! * A `clientToken` that minted a limit no longer replays once that limit is gone, so
//!   the same create body is a new limit rather than the ARN that no longer resolves.
//!
//! Each suite asserts its rule over a TABLE spanning the class rather than one recorded
//! call: the rows differ along the members the deleted limit was created with (the
//! amount's form, whether it carried a period and how long, the tag count), the device it
//! was about, the order the limits are deleted in, and the kind of ARN the request names.
//! Everything goes through the production router, reusing the harness in
//! `quantum_task_loop.rs`, so what is pinned is what a Braket client sees.

use serde_json::{json, Value};

use crate::quantum_task_loop::{encode, member, Braket, Reply};
use crate::spending_limit_updates::{update_limit, ACCOUNT, REGION};
use crate::spending_limits::{
    a_device, create_limit, limit_body, limit_by_arn, limits_on, now_epoch, period, CREATED,
};

/// `DELETE /spending-limit/{spendingLimitArn}/delete`.
async fn delete_limit(braket: &Braket, arn: &str) -> Reply {
    braket
        .call(
            "DELETE",
            &format!("/spending-limit/{}/delete", encode(arn)),
            None,
        )
        .await
}

/// One row of the table: how the limit to be deleted was created.
struct Row {
    case: &'static str,
    amount: &'static str,
    /// Whether the create carried a `timePeriod`, and the seconds its start and end fall
    /// from the suite's reference instant.
    period_offsets: Option<(i64, i64)>,
    /// The tags the create carried, as key/value pairs.
    tags: &'static [(&'static str, &'static str)],
}

/// Create the limit `row` describes on `device` and return its ARN.
async fn create_row(braket: &Braket, device: &str, token: &str, row: &Row, start: i64) -> String {
    let mut body = limit_body(token, device, row.amount);
    if let Some((from, to)) = row.period_offsets {
        body["timePeriod"] = period(start + from, start + to);
    }
    if !row.tags.is_empty() {
        let mut tags = json!({});
        for (key, value) in row.tags {
            tags[*key] = json!(*value);
        }
        body["tags"] = tags;
    }
    let reply = create_limit(braket, body).await;
    assert_eq!(
        reply.status, CREATED,
        "case {:?}: creating the limit to delete: expected {CREATED}, got {} {}",
        row.case, reply.status, reply.body
    );
    reply.str_member("spendingLimitArn")
}

/// Every ARN `SearchSpendingLimits` reports on `device`.
async fn arns_on(braket: &Braket, device: &str) -> Vec<String> {
    limits_on(braket, device)
        .await
        .iter()
        .map(|limit| member(limit, "spendingLimitArn").to_string())
        .collect()
}

/// A delete removes the limit it names and leaves every other limit exactly as it was,
/// whatever the removed limit was created with and whichever order the removals come in.
#[tokio::test]
async fn a_delete_removes_the_named_limit_and_leaves_the_others() {
    let braket = Braket::new();
    let device = a_device(&braket, "QPU", "ONLINE").await;
    let start = now_epoch();

    // The rows span the forms a stored limit can take: the amount whole, with one and
    // with two decimals, zero and large; the period absent (the default window), a
    // second, a month and one that has already ended; and no tags, one tag and several.
    let rows: Vec<Row> = vec![
        Row {
            case: "a whole amount, no period, no tags",
            amount: "250",
            period_offsets: None,
            tags: &[],
        },
        Row {
            case: "one decimal place, a one-second period, one tag",
            amount: "0.5",
            period_offsets: Some((0, 1)),
            tags: &[("team", "quantum")],
        },
        Row {
            case: "two decimal places, an hour, several tags",
            amount: "999.99",
            period_offsets: Some((0, 3600)),
            tags: &[("team", "quantum"), ("env", "test"), ("owner", "ada")],
        },
        Row {
            case: "zero, a month, no tags",
            amount: "0",
            period_offsets: Some((0, 30 * 24 * 60 * 60)),
            tags: &[],
        },
        Row {
            case: "a large amount, a period already over, one tag",
            amount: "1000000",
            period_offsets: Some((-3_600, -1_800)),
            tags: &[("env", "prod")],
        },
    ];

    let mut arns: Vec<String> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        arns.push(create_row(&braket, &device, &format!("tok-create-{index}"), row, start).await);
    }
    assert_eq!(
        arns_on(&braket, &device).await.len(),
        rows.len(),
        "every row was created before the first delete"
    );

    // The removals come in an order that is not the creation order, so what a delete
    // removes cannot be the first or the last entry by accident: the middle row, then the
    // first, the last, and the two left over.
    let order = [2_usize, 0, 4, 3, 1];
    let mut remaining: Vec<String> = arns.clone();
    for position in order {
        let row = &rows[position];
        let arn = arns[position].clone();
        let before = limit_by_arn(&braket, &device, &arn).await;
        let stored_amount = member(&before, "spendingLimit").to_string();
        assert_eq!(
            stored_amount, row.amount,
            "case {:?}: the limit to delete holds the amount it was created with, got {before}",
            row.case
        );

        let reply = delete_limit(&braket, &arn).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200, got {} {}",
            row.case, reply.status, reply.body
        );
        assert_eq!(
            reply.error, None,
            "case {:?}: a served delete carries no error, got {:?}",
            row.case, reply.error
        );

        remaining.retain(|stored| stored != &arn);
        let reported = arns_on(&braket, &device).await;
        assert!(
            !reported.contains(&arn),
            "case {:?}: the search still reports the deleted {arn}: {reported:?}",
            row.case
        );
        assert_eq!(
            reported.len(),
            remaining.len(),
            "case {:?}: expected {} limits left, got {reported:?}",
            row.case,
            remaining.len()
        );
        for survivor in &remaining {
            assert!(
                reported.contains(survivor),
                "case {:?}: the delete dropped {survivor} as well: {reported:?}",
                row.case
            );
            let position = arns
                .iter()
                .position(|stored| stored == survivor)
                .expect("a survivor is one of the created limits");
            let still = limit_by_arn(&braket, &device, survivor).await;
            assert_eq!(
                member(&still, "spendingLimit"),
                rows[position].amount,
                "case {:?}: the survivor {survivor} changed, got {still}",
                row.case
            );
        }
    }

    assert!(
        arns_on(&braket, &device).await.is_empty(),
        "every limit was deleted"
    );
    // The device the limits were about is published by AWS, not created through the API:
    // deleting a limit about it removes nothing else.
    let published = braket.get_device(&device).await;
    assert_eq!(
        published.status, 200,
        "the device outlives every limit about it, got {} {}",
        published.status, published.body
    );
}

/// What `spendingLimitArn` resolves to decides the answer: a stored limit is removed, an
/// ARN naming none is a 404, and a value that is not an ARN of a spending limit is a 400.
/// A refused delete removes nothing.
#[tokio::test]
async fn the_arn_decides_which_limit_a_delete_resolves() {
    let braket = Braket::new();
    let device = a_device(&braket, "QPU", "ONLINE").await;
    let start = now_epoch();
    let row = Row {
        case: "the limit a refused delete must not touch",
        amount: "100.50",
        period_offsets: Some((0, 3600)),
        tags: &[("team", "quantum")],
    };
    let arn = create_row(&braket, &device, "tok-create-arn", &row, start).await;

    // (case, the ARN the request names, expected status, expected error code) — a
    // well-formed spending-limit ARN naming nothing stored, the same in another region and
    // under another account, a resource field with no id (which `SpendingLimitArn`'s own
    // `@pattern` admits), an ARN of another Braket resource, and values that are no ARN at
    // all.
    let rows: Vec<(&str, String, u16, &str)> = vec![
        (
            "a well-formed ARN naming no stored limit",
            format!("arn:aws:braket:{REGION}:{ACCOUNT}:spending-limit/ghost-1"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a well-formed ARN in another region",
            format!("arn:aws:braket:us-east-1:{ACCOUNT}:spending-limit/ghost-2"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a well-formed ARN under another account",
            format!("arn:aws:braket:{REGION}:123456789012:spending-limit/ghost-3"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an empty resource id",
            format!("arn:aws:braket:{REGION}:{ACCOUNT}:spending-limit/"),
            400,
            "ValidationException",
        ),
        (
            "an ARN of another Braket resource",
            format!("arn:aws:braket:{REGION}:{ACCOUNT}:quantum-task/t1"),
            400,
            "ValidationException",
        ),
        (
            "an ARN of another service",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:spending-limit/l1"),
            400,
            "ValidationException",
        ),
        (
            "no ARN at all",
            "not-an-arn".to_string(),
            400,
            "ValidationException",
        ),
    ];

    for (case, named, status, code) in &rows {
        let reply = delete_limit(&braket, named).await;
        assert_eq!(
            reply.status, *status,
            "case {case:?}: expected {status}, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some(*code),
            "case {case:?}: expected {code}, got {:?} in {}",
            reply.error,
            reply.body
        );
        if *status == 404 {
            assert_eq!(
                reply.message(),
                format!("Spending limit {named} not found"),
                "case {case:?}: the 404 names the ARN the request carried"
            );
        } else {
            assert_eq!(
                reply.message(),
                "spendingLimitArn is not valid.",
                "case {case:?}: the 400 is this operation's own sentence"
            );
        }

        let reported = arns_on(&braket, &device).await;
        assert_eq!(
            reported,
            vec![arn.clone()],
            "case {case:?}: a refused delete removes nothing, got {reported:?}"
        );
    }

    // The control: the ARN the create minted does resolve, and then it does not.
    let served = delete_limit(&braket, &arn).await;
    assert_eq!(
        served.status, 200,
        "the ARN the create minted resolves: expected 200, got {} {}",
        served.status, served.body
    );
    assert!(
        arns_on(&braket, &device).await.is_empty(),
        "the served delete removed the limit"
    );
}

/// A removed limit stays removed: no door of the resource resolves its ARN again, and the
/// device is free to carry a new limit.
#[tokio::test]
async fn a_deleted_limit_resolves_for_no_door_of_the_resource() {
    let braket = Braket::new();
    let device = a_device(&braket, "QPU", "ONLINE").await;
    let start = now_epoch();
    let row = Row {
        case: "the limit to delete twice",
        amount: "77",
        period_offsets: Some((0, 7200)),
        tags: &[],
    };
    let arn = create_row(&braket, &device, "tok-create-twice", &row, start).await;

    let first = delete_limit(&braket, &arn).await;
    assert_eq!(
        first.status, 200,
        "the first delete: expected 200, got {} {}",
        first.status, first.body
    );

    // Repeating the delete, several times over, resolves nothing: the ARN names no limit.
    for attempt in 1..=3 {
        let repeat = delete_limit(&braket, &arn).await;
        assert_eq!(
            repeat.status, 404,
            "repeat {attempt}: expected 404, got {} {}",
            repeat.status, repeat.body
        );
        assert_eq!(
            repeat.error.as_deref(),
            Some("ResourceNotFoundException"),
            "repeat {attempt}: expected ResourceNotFoundException, got {:?} in {}",
            repeat.error,
            repeat.body
        );
        assert_eq!(
            repeat.message(),
            format!("Spending limit {arn} not found"),
            "repeat {attempt}: the 404 names the ARN the request carried"
        );
    }

    // Nor does the other door that resolves a limit by its ARN.
    let update = update_limit(
        &braket,
        &arn,
        json!({ "clientToken": "tok-update-gone", "spendingLimit": "1" }),
    )
    .await;
    assert_eq!(
        update.status, 404,
        "an update of a deleted limit: expected 404, got {} {}",
        update.status, update.body
    );

    // The device carries a new limit afterwards, with an ARN of its own.
    let fresh = create_row(&braket, &device, "tok-create-fresh", &row, start).await;
    assert_ne!(
        fresh, arn,
        "a create after the delete mints a new ARN rather than the deleted one"
    );
    assert_eq!(
        arns_on(&braket, &device).await,
        vec![fresh.clone()],
        "the device carries only the new limit"
    );
}

/// A `clientToken` that minted a limit no longer replays once that limit is gone: the
/// same create body mints a new limit instead of answering with an ARN that resolves to
/// nothing.
#[tokio::test]
async fn the_client_token_of_a_deleted_limit_no_longer_replays() {
    let braket = Braket::new();
    let device = a_device(&braket, "QPU", "ONLINE").await;
    let start = now_epoch();

    // (case, the token's create body) — the rows differ in which optional members the
    // create carried, because the token is remembered against the whole request.
    let mut with_period = limit_body("tok-period", &device, "10");
    with_period["timePeriod"] = period(start, start + 3600);
    let mut with_tags = limit_body("tok-tags", &device, "20.25");
    with_tags["tags"] = json!({ "team": "quantum" });
    let rows: Vec<(&str, Value)> = vec![
        (
            "the bare required members",
            limit_body("tok-bare", &device, "5"),
        ),
        ("a period as well", with_period),
        ("tags as well", with_tags),
    ];

    for (case, body) in rows {
        let first = create_limit(&braket, body.clone()).await;
        assert_eq!(
            first.status, CREATED,
            "case {case:?}: the first create: expected {CREATED}, got {} {}",
            first.status, first.body
        );
        let arn = first.str_member("spendingLimitArn");

        // While the limit is there, the token replays to it rather than creating a second.
        let replay = create_limit(&braket, body.clone()).await;
        assert_eq!(
            replay.str_member("spendingLimitArn"),
            arn,
            "case {case:?}: a replayed token answers with the ARN it minted"
        );

        let deleted = delete_limit(&braket, &arn).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected 200, got {} {}",
            deleted.status, deleted.body
        );

        let again = create_limit(&braket, body).await;
        assert_eq!(
            again.status, CREATED,
            "case {case:?}: the create after the delete: expected {CREATED}, got {} {}",
            again.status, again.body
        );
        let minted = again.str_member("spendingLimitArn");
        assert_ne!(
            minted, arn,
            "case {case:?}: the token must not replay to the deleted {arn}"
        );
        let reported = arns_on(&braket, &device).await;
        assert_eq!(
            reported,
            vec![minted.clone()],
            "case {case:?}: the device carries only the newly minted limit, got {reported:?}"
        );

        // Leave the store empty for the next row.
        let cleanup = delete_limit(&braket, &minted).await;
        assert_eq!(
            cleanup.status, 200,
            "case {case:?}: clearing the store: expected 200, got {} {}",
            cleanup.status, cleanup.body
        );
    }
}
