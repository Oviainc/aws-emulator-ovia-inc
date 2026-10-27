// SPDX-License-Identifier: CC0-1.0

//! A Resolver on an Outpost is bound to an Outpost the account HAS, and a
//! Resolver the account does not have is reported absent rather than refused.
//!
//! Two rules, each stated once and then exercised over a set of inputs:
//!
//! 1. `CreateOutpostResolver` resolves `OutpostArn` against the account's own
//!    AWS Outposts inventory — the Outposts `ListOutposts` answers from, in the
//!    Region the request is signed for — and creates nothing for an ARN that
//!    names no Outpost there, however well the value fits the member's pattern.
//!    The refusal is the `ValidationException` the operation models, HTTP 400,
//!    carrying the service error code of this fault and the ARN the request
//!    supplied.
//! 2. `GetOutpostResolver` and `DeleteOutpostResolver` are satisfied by the
//!    absence of the Resolver: both answer HTTP 200 with no `OutpostResolver`
//!    member for an `Id` the account holds no Resolver under, so a defensive
//!    delete and a retried delete answer as the delete that removed the record
//!    did, and a caller probing with the reader is told it is not there rather
//!    than handed an exception.
//!
//! Every case goes over the wire both services actually speak, through the
//! shared [`super::harness`]: the Outposts are racked through the AWS Outposts
//! API on the same engine, so what the create resolves is state another service
//! wrote and not a value this suite handed it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    call, emulator_with_outposts, error_type, ok, rack_outpost, string_member, unrack_outpost,
    REGION,
};

/// The service error code of the "this account has no such Outpost" fault, as
/// recorded on AWS for `CreateOutpostResolver`. It is the per-fault
/// discriminator: the HTTP status (400) and the exception shape name are shared
/// by this operation's other `ValidationException` faults.
const RSLVR_CODE_OUTPOST_NOT_FOUND: &str = "RSLVR-03007";

/// The service error code of the "this instance count is below the minimum"
/// fault, which the operation checks BEFORE it resolves the Outpost.
const RSLVR_CODE_INSTANCE_COUNT: &str = "RSLVR-03002";

/// The message `CreateOutpostResolver` refuses an unresolvable `OutpostArn`
/// with: the bracketed service error code, the fixed sentence, and the ARN the
/// request supplied.
fn expected_refusal(outpost_arn: &str) -> String {
    format!(r#"[{RSLVR_CODE_OUTPOST_NOT_FOUND}] Cannot find the Outpost with Arn "{outpost_arn}"."#)
}

/// A `CreateOutpostResolver` body on `outpost_arn`.
fn create_body(creator: &str, outpost_arn: &str) -> Value {
    json!({
        "CreatorRequestId": creator,
        "Name": creator,
        "PreferredInstanceType": "m5.large",
        "OutpostArn": outpost_arn,
    })
}

/// The ids `ListOutpostResolvers` reports.
async fn listed_ids(router: &MultiServiceRouter) -> Vec<String> {
    let page = ok(router, "ListOutpostResolvers", json!({})).await;
    page.get("OutpostResolvers")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(|item| string_member(item, "Id")).collect())
        .unwrap_or_default()
}

/// An `OutpostArn` of `region` built around `tail`, the 17 hex characters of an
/// Outpost id, in `partition` and under `account`.
///
/// The form is the member's documented pattern
/// `^arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:outpost/op-[a-f0-9]{17}$`
/// (`smithy_query(query="shape",shape="OutpostResolver")`), so every value built
/// here reaches the handler instead of being refused at the boundary.
fn outpost_arn(partition: &str, region: &str, account: &str, tail: &str) -> String {
    assert_eq!(
        tail.len(),
        17,
        "case {tail:?}: an Outpost id carries 17 hex characters"
    );
    format!("arn:{partition}:outposts:{region}:{account}:outpost/op-{tail}")
}

/// The 17 hex characters of the Outpost id an `OutpostArn` carries.
fn outpost_tail(outpost_arn: &str) -> String {
    let tail = outpost_arn
        .rsplit_once("outpost/op-")
        .map(|(_, tail)| tail.to_string())
        .unwrap_or_else(|| panic!("a minted OutpostArn names an op- id, got {outpost_arn:?}"));
    assert_eq!(
        tail.len(),
        17,
        "a minted Outpost id carries 17 hex characters, got {tail:?}"
    );
    tail
}

/// Rule 1. A create is served only on an Outpost the account has in the signed
/// Region, and refused — creating nothing — on any other value the member's
/// pattern admits.
///
/// The table spans what the rule generalises over: which of the account's
/// Outposts is named (so the check is a lookup and not a count), an ARN nothing
/// racked, a near miss of a racked ARN, an Outpost racked in another Region, an
/// ARN of another partition, and an ARN of another account.
#[tokio::test]
async fn a_resolver_can_only_be_created_on_an_outpost_the_account_has() {
    let router = emulator_with_outposts();
    let first = rack_outpost(&router, REGION, "first").await;
    let second = rack_outpost(&router, REGION, "second").await;
    // An Outpost of another Region: racked through the same API, addressed to
    // that Region, so it is a real Outpost of the account — just not one a
    // Resolver created in [`REGION`] can sit on.
    let elsewhere = rack_outpost(&router, "eu-west-1", "elsewhere").await;

    let tail = outpost_tail(&first);
    let near_miss = {
        let mut flipped: Vec<char> = tail.chars().collect();
        flipped[0] = if flipped[0] == 'a' { 'b' } else { 'a' };
        outpost_arn(
            "aws",
            REGION,
            "000000000000",
            &flipped.into_iter().collect::<String>(),
        )
    };

    let served = [
        ("the first Outpost the account racked", first.clone()),
        ("the second Outpost the account racked", second),
    ];
    let refused = [
        (
            "an ARN nothing racked",
            outpost_arn("aws", REGION, "000000000000", "00000000000000000"),
        ),
        ("a racked ARN with one character changed", near_miss),
        ("an Outpost racked in another Region", elsewhere),
        (
            "an ARN of another partition",
            outpost_arn("aws-us-gov", "us-gov-west-1", "000000000000", &tail),
        ),
        (
            "an ARN of another account",
            outpost_arn("aws", REGION, "123456789012", &tail),
        ),
    ];

    let mut created = Vec::new();
    for (case, outpost) in served {
        let answered = ok(
            &router,
            "CreateOutpostResolver",
            create_body(&format!("req-{}", created.len()), &outpost),
        )
        .await;
        let resolver = answered
            .get("OutpostResolver")
            .unwrap_or_else(|| panic!("case {case:?}: the create must answer a Resolver"));
        assert_eq!(
            string_member(resolver, "OutpostArn"),
            outpost,
            "case {case:?}: the Resolver must sit on the Outpost requested, got {answered}"
        );
        created.push(string_member(resolver, "Id"));
    }

    for (case, outpost) in refused {
        let (status, answered) = call(
            &router,
            "CreateOutpostResolver",
            create_body("req-refused", &outpost),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: the create must answer HTTP 400, got {status}: {answered}"
        );
        assert!(
            error_type(&answered).contains("ValidationException"),
            "case {case:?}: the create must answer ValidationException, got {answered}"
        );
        assert_eq!(
            string_member(&answered, "Message"),
            expected_refusal(&outpost),
            "case {case:?}: the refusal must name the Outpost the request asked for, got {answered}"
        );
        assert_eq!(
            listed_ids(&router).await,
            created,
            "case {case:?}: a refused create must store nothing"
        );
    }
}

/// Rule 1, continued: the Outpost is resolved against state, so the same ARN is
/// served while the Outpost is racked and refused once it is gone — and the
/// Resolver already on it stays exactly where it was.
#[tokio::test]
async fn an_outpost_taken_out_of_the_account_no_longer_takes_a_resolver() {
    let router = emulator_with_outposts();
    let outpost = rack_outpost(&router, REGION, "temporary").await;

    let created = ok(
        &router,
        "CreateOutpostResolver",
        create_body("req-while-racked", &outpost),
    )
    .await;
    let id = string_member(
        created
            .get("OutpostResolver")
            .unwrap_or_else(|| panic!("the create must answer a Resolver, got {created}")),
        "Id",
    );

    unrack_outpost(&router, REGION, &outpost).await;

    let (status, answered) = call(
        &router,
        "CreateOutpostResolver",
        create_body("req-after-unracked", &outpost),
    )
    .await;
    assert_eq!(
        status, 400,
        "the same ARN must be refused once the account no longer has that Outpost, got {status}: {answered}"
    );
    assert_eq!(
        string_member(&answered, "Message"),
        expected_refusal(&outpost),
        "the refusal must name the Outpost the request asked for, got {answered}"
    );
    assert_eq!(
        listed_ids(&router).await,
        vec![id],
        "the refused create must leave the Resolver already on file alone"
    );
}

/// Rule 1's place in the order of checks: the `InstanceCount` bound is cheaper
/// than the Outpost lookup, so a request that breaks both is refused for the
/// count — each fault stating its own service error code.
#[tokio::test]
async fn the_instance_count_bound_is_checked_before_the_outpost_is_resolved() {
    let router = emulator_with_outposts();
    let racked = rack_outpost(&router, REGION, "ordered").await;
    let unracked = outpost_arn("aws", REGION, "000000000000", "11111111111111111");

    for (case, outpost, instance_count, expected_code) in [
        (
            "both faults at once",
            unracked.clone(),
            1,
            RSLVR_CODE_INSTANCE_COUNT,
        ),
        (
            "a bad count on an Outpost the account has",
            racked.clone(),
            1,
            RSLVR_CODE_INSTANCE_COUNT,
        ),
        (
            "an acceptable count on an Outpost it has not",
            unracked,
            4,
            RSLVR_CODE_OUTPOST_NOT_FOUND,
        ),
    ] {
        let mut body = create_body("req-order", &outpost);
        body["InstanceCount"] = json!(instance_count);
        let (status, answered) = call(&router, "CreateOutpostResolver", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the create must answer HTTP 400, got {status}: {answered}"
        );
        let message = string_member(&answered, "Message");
        assert!(
            message.starts_with(&format!("[{expected_code}] ")),
            "case {case:?}: the refusal must carry the {expected_code} service error code, got {message:?}"
        );
    }

    // The ordering above refused every request: nothing was created.
    assert!(
        listed_ids(&router).await.is_empty(),
        "no refused create may store a Resolver"
    );
}

/// Rule 2. A Resolver the account does not have is reported absent: the reader
/// and the delete answer HTTP 200 with no `OutpostResolver`.
///
/// The table spans what the rule generalises over: an id nothing ever minted,
/// ids of other shapes and lengths, the id of a Resolver this very test
/// deleted, and a repeat of that delete — against a store that holds another
/// Resolver throughout, so the answers are about the id asked for.
#[tokio::test]
async fn a_resolver_the_account_does_not_have_is_reported_absent() {
    let router = emulator_with_outposts();
    let outpost = rack_outpost(&router, REGION, "absent-cases").await;

    let kept = string_member(
        ok(
            &router,
            "CreateOutpostResolver",
            create_body("req-kept", &outpost),
        )
        .await
        .get("OutpostResolver")
        .expect("the create answers a Resolver"),
        "Id",
    );
    let doomed = string_member(
        ok(
            &router,
            "CreateOutpostResolver",
            create_body("req-doomed", &outpost),
        )
        .await
        .get("OutpostResolver")
        .expect("the create answers a Resolver"),
        "Id",
    );
    let removed = ok(
        &router,
        "DeleteOutpostResolver",
        json!({ "Id": doomed.clone() }),
    )
    .await;
    assert_eq!(
        string_member(
            removed
                .get("OutpostResolver")
                .expect("the delete reports the Resolver it removed"),
            "Id"
        ),
        doomed,
        "the delete of a stored Resolver must report that Resolver, got {removed}"
    );

    for (case, id) in [
        ("the Resolver this case just deleted", doomed.clone()),
        (
            "an id nothing ever minted",
            "rslvr-op-0000000000000000".to_string(),
        ),
        (
            "an id of another resource kind",
            "rslvr-out-0123456789abcdef".to_string(),
        ),
        ("one character, the shortest ResourceId", "r".to_string()),
        (
            "the longest ResourceId the model allows",
            "r".repeat(64),
        ),
        (
            "a near miss of the stored id",
            kept[..kept.len() - 1].to_string(),
        ),
    ] {
        for operation in ["GetOutpostResolver", "DeleteOutpostResolver"] {
            let answered = ok(&router, operation, json!({ "Id": id.clone() })).await;
            assert_eq!(
                answered.get("OutpostResolver"),
                None,
                "case {case:?}: {operation} must answer with no OutpostResolver member, got {answered}"
            );
        }
    }

    // None of those calls removed, created or hid anything: the Resolver that
    // was never deleted is still the one and only one on file.
    assert_eq!(
        listed_ids(&router).await,
        vec![kept.clone()],
        "the calls above must leave the store holding exactly the kept Resolver"
    );
    let read_back = ok(&router, "GetOutpostResolver", json!({ "Id": kept.clone() })).await;
    assert_eq!(
        string_member(
            read_back
                .get("OutpostResolver")
                .unwrap_or_else(|| panic!("the kept Resolver must still be readable, got {read_back}")),
            "Id"
        ),
        kept,
        "the kept Resolver must still read back under its own id, got {read_back}"
    );
}
