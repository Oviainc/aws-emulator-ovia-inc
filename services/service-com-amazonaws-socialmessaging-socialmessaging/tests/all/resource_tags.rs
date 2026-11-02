// SPDX-License-Identifier: CC0-1.0

//! What `ListTagsForResource` reports, over `restJson1`.
//!
//! RULE: a tag read reports exactly the tags held against the resource its `resourceArn`
//! names — for every resource kind this service tags — and reports them without changing
//! anything, so two reads in a row answer the same list. An ARN nothing is held against
//! reports NO tags rather than a failure: the operation models no not-found error
//! (`smithy_query(query="operation", operation="ListTagsForResource")` lists
//! `InternalServiceException`, `InvalidParametersException`, `ThrottledRequestException`),
//! and the recorded read of an ARN naming no resource answered 200 with an empty list. A
//! value that is not an `arn:`-prefixed string at all is refused by the member's own
//! `@pattern` before any handler runs.
//!
//! The rows differ along what the rule generalises over: the resource KIND (a linked
//! WhatsApp Business Account, a phone number registered under one), the tag COUNT (none,
//! one, several), the tag VALUE (set, and the empty value a tag sent without one reads
//! back as), and — for the ARNs that name nothing — the ARN's vendor, its Region, its
//! resource type and whether it parses as an ARN at all.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, console_call, emulator, query_escape, start_signup, ACCOUNT_ID};

/// One tag, as the wire carries it.
fn tag(key: &str, value: &str) -> Value {
    json!({ "key": key, "value": value })
}

/// Read the tags of `resource_arn`.
async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> crate::harness::Answer {
    let uri = format!("/v1/tags/list?resourceArn={}", query_escape(resource_arn));
    call(router, "GET", &uri, None).await
}

/// The `(key, value)` pairs a tag read answered with, in the order it reported them.
fn pairs(answer: &crate::harness::Answer) -> Vec<(String, String)> {
    answer
        .at("/tags")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            (
                entry["key"].as_str().unwrap_or_default().to_string(),
                entry["value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Link one account, tagging the account and each of its phone numbers, and return the
/// `(account ARN, phone-number ARNs)` the service minted for them.
async fn link_tagged(
    router: &MultiServiceRouter,
    waba_id: &str,
    account_tags: &[Value],
    numbers: &[(&str, Vec<Value>)],
) -> (String, Vec<String>) {
    let token = start_signup(router).await;
    let phone_numbers: Vec<Value> = numbers
        .iter()
        .map(|(meta_id, tags)| {
            json!({ "id": meta_id, "twoFactorPin": "123456", "tags": tags })
        })
        .collect();
    let finalize = console_call(
        router,
        "POST",
        "/v1/whatsapp/signup",
        Some(json!({
            "setupFinalization": {
                "associateInProgressToken": token,
                "phoneNumbers": phone_numbers,
                "waba": { "id": waba_id, "tags": account_tags },
            }
        })),
    )
    .await;
    assert_eq!(
        finalize.status, 200,
        "setup {waba_id:?}: expected 200 from AssociateWhatsAppBusinessAccount, got {} {:?}",
        finalize.status, finalize.body
    );
    let id = finalize.string_at("/linkedWhatsAppBusinessAccountId");
    let uri = format!("/v1/whatsapp/waba/details?id={}", query_escape(&id));
    let account = call(router, "GET", &uri, None).await;
    assert_eq!(
        account.status, 200,
        "setup {waba_id:?}: expected 200 from GetLinkedWhatsAppBusinessAccount, got {} {:?}",
        account.status, account.body
    );
    let arns: Vec<String> = account
        .at("/account/phoneNumbers")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|number| number["arn"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        arns.len(),
        numbers.len(),
        "setup {waba_id:?}: expected one registered number per entry, got {arns:?}"
    );
    (account.string_at("/account/arn"), arns)
}

/// One row of the table below: the case, the ARN read, and the `(key, value)` tags the
/// read must report for it, in order.
type Row = (&'static str, String, Vec<(&'static str, &'static str)>);

/// RULE: the read reports the tags the named resource holds, whichever kind of resource
/// it is and however many tags it has — and reporting them changes nothing.
#[tokio::test]
async fn a_tag_read_reports_the_tags_the_named_resource_holds() {
    let router = emulator();

    // A tagged account whose three numbers differ in what they were tagged with: several
    // tags, one tag sent WITHOUT a value, and none at all.
    let (tagged_account, tagged_numbers) = link_tagged(
        &router,
        "waba-01234567890123456789012345678901",
        &[tag("env", "prod"), tag("team", "growth")],
        &[
            (
                "phone-number-id-aaaa",
                vec![tag("line", "support"), tag("env", "prod")],
            ),
            ("phone-number-id-bbbb", vec![json!({ "key": "owner" })]),
            ("phone-number-id-cccc", vec![]),
        ],
    )
    .await;
    // A second account, tagged with nothing, so "no tags" is a property of the resource
    // and not of an empty store.
    let (untagged_account, untagged_numbers) = link_tagged(
        &router,
        "waba-99999999999999999999999999999999",
        &[],
        &[("phone-number-id-dddd", vec![tag("line", "sales")])],
    )
    .await;

    let rows: Vec<Row> = vec![
        (
            "an account tagged with two tags",
            tagged_account.clone(),
            vec![("env", "prod"), ("team", "growth")],
        ),
        (
            "an account tagged with nothing",
            untagged_account,
            Vec::new(),
        ),
        (
            "a phone number tagged with two tags",
            tagged_numbers[0].clone(),
            vec![("env", "prod"), ("line", "support")],
        ),
        (
            "a phone number tagged with a valueless tag",
            tagged_numbers[1].clone(),
            vec![("owner", "")],
        ),
        (
            "a phone number tagged with nothing",
            tagged_numbers[2].clone(),
            Vec::new(),
        ),
        (
            "a phone number under an untagged account",
            untagged_numbers[0].clone(),
            vec![("line", "sales")],
        ),
    ];

    for (case, resource_arn, expected) in rows {
        let expected: Vec<(String, String)> = expected
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        let answer = list_tags(&router, &resource_arn).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.at("/statusCode"),
            &json!(200),
            "case {case:?}: expected statusCode 200, got {:?}",
            answer.at("/statusCode")
        );
        let actual = pairs(&answer);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the tags {expected:?}, got {actual:?}"
        );

        // A read leaves the resource as it found it: the second read answers the first.
        let again = list_tags(&router, &resource_arn).await;
        let repeated = pairs(&again);
        assert_eq!(
            repeated, actual,
            "case {case:?}: a second read must report {actual:?}, got {repeated:?}"
        );
    }
}

/// RULE: an ARN nothing is held against reports no tags (not a failure — this operation
/// models no not-found error), and a value that is not an `arn:`-prefixed string is
/// refused by the member's own `@pattern` first.
#[tokio::test]
async fn an_arn_naming_nothing_reports_no_tags() {
    let router = emulator();
    let (account_arn, _) = link_tagged(
        &router,
        "waba-55555555555555555555555555555555",
        &[tag("env", "prod")],
        &[("phone-number-id-eeee", vec![])],
    )
    .await;

    // ARN-form values that name no resource of this service: another vendor's, this
    // service's own forms with ids nothing was minted under, a resource type this service
    // does not have, the tagged account's ARN in another Region, and a value that carries
    // the `arn:` prefix but none of an ARN's other fields.
    let naming_nothing: Vec<(&str, String)> = vec![
        ("an ARN of another vendor", "arn:aws:s3:::a-bucket".to_string()),
        (
            "an account ARN of this service that names no account",
            format!(
                "arn:aws:social-messaging:us-east-1:{ACCOUNT_ID}:waba/00000000000000000000000000000000"
            ),
        ),
        (
            "a phone-number ARN of this service that names no number",
            format!(
                "arn:aws:social-messaging:us-east-1:{ACCOUNT_ID}:phone-number-id/00000000000000000000000000000000"
            ),
        ),
        (
            "a resource type this service does not have",
            format!("arn:aws:social-messaging:us-east-1:{ACCOUNT_ID}:flow/1"),
        ),
        (
            "the tagged account's ARN in another Region",
            account_arn.replace("us-east-1", "eu-west-1"),
        ),
        ("an ARN prefix and nothing else", "arn:".to_string()),
    ];

    for (case, resource_arn) in naming_nothing {
        let answer = list_tags(&router, &resource_arn).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        let actual = pairs(&answer);
        assert!(
            actual.is_empty(),
            "case {case:?}: expected no tags, got {actual:?}"
        );
    }

    // A value that is not an ARN never reaches the handler: the member's `@pattern`
    // refuses it.
    let refused: Vec<(&str, String)> = vec![
        (
            "the account's own short id, which is not an ARN",
            "waba-55555555555555555555555555555555".to_string(),
        ),
        ("a value that is not an ARN at all", "not-an-arn".to_string()),
        (
            "an ARN whose prefix is in upper case",
            account_arn.replacen("arn:", "ARN:", 1),
        ),
    ];
    for (case, resource_arn) in refused {
        let answer = list_tags(&router, &resource_arn).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} {:?} {:?}",
            answer.status,
            answer.error_type,
            answer.body
        );
    }

    // The account those reads ran beside still reports its own tags, so no read of
    // another ARN changed anything.
    let answer = list_tags(&router, &account_arn).await;
    let actual = pairs(&answer);
    assert_eq!(
        actual,
        vec![("env".to_string(), "prod".to_string())],
        "case \"the tagged account after the other reads\": expected its one tag, got {actual:?}"
    );
}
