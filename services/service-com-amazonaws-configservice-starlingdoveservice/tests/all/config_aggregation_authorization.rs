// SPDX-License-Identifier: CC0-1.0

//! The aggregation-authorization write-and-read loop, end to end over the wire.
//!
//! Six rules are pinned here, each over a TABLE of inputs rather than one case:
//!
//! 1. `PutAggregationAuthorization` persists the authorization the request names and
//!    `DescribeAggregationAuthorizations` reports it back, whatever authorized account
//!    and authorized Region the pair carries, with the ARN Config builds from that pair;
//! 2. the authorized PAIR identifies the authorization, so a repeat put of the same
//!    pair is idempotent — same ARN, same creation time, no duplicate, and the tags it
//!    was created with survive a request carrying different ones — while a pair
//!    differing in either half is a DIFFERENT authorization;
//! 3. the describe pages over a stable ordering (authorized account, then authorized
//!    Region), at the `Limit` the request asks for or the modelled maximum when it asks
//!    for none; its `NextToken` holds the KEY the next page resumes at, so a walk
//!    reports every authorization state holds at or after that key — whatever was
//!    granted or revoked between the pages — and a token it never issued is refused;
//!    reading changes nothing a later caller sees;
//! 4. `DeleteAggregationAuthorization` revokes exactly the pair it names, leaves every
//!    other authorization reported, succeeds on a pair that holds none (the model
//!    declares no not-found error), and the revoked pair can be granted again;
//! 5. the store is bounded: once it holds this emulator's maximum, a NEW pair is
//!    refused with the modelled client error while a repeat of an authorized pair
//!    still succeeds, and a revocation makes room for one more;
//! 6. a request outside the modelled constraints on `AuthorizedAccountId` and
//!    `AuthorizedAwsRegion` is refused — by the put and the delete alike — and
//!    persists nothing.
//!
//! The harness (`Emulator`) is the one `config_setup_and_rule_loop` already builds;
//! every case drives the emulator the way a client does, through the
//! `MultiServiceRouter`.

use serde_json::{json, Value};

use crate::config_setup_and_rule_loop::Emulator;

/// Twelve-digit aggregator account ids, the form `AccountId`'s `^\d{12}$` pattern
/// allows (`smithy_query(query="shape", shape="AccountId")`).
const ACCOUNT_A: &str = "111122223333";
/// See [`ACCOUNT_A`].
const ACCOUNT_B: &str = "444455556666";
/// See [`ACCOUNT_A`].
const ACCOUNT_C: &str = "777788889999";

/// The maximum length `AwsRegion` allows (`smithy_query(query="shape",
/// shape="AwsRegion")`: `length 1..64`).
const AWS_REGION_MAX_LENGTH: usize = 64;

/// The maximum `Limit` `DescribeAggregationAuthorizations` allows
/// (`smithy_query(query="shape", shape="Limit")`: `range 0..100`).
const LIMIT_MAX: i64 = 100;

/// `text` as the base64 the framework's page tokens are encoded with, so a case can
/// hand the operation a token of its own choosing.
fn base64(text: &str) -> String {
    aws_smithy_types::base64::encode(text)
}

/// The key an authorization is reported under: the authorized account id, then the
/// authorized Region (`DescribeAggregationAuthorizations` pages over this ordering).
fn key_of(pair: &(String, String)) -> String {
    format!("{}/{}", pair.0, pair.1)
}

/// The `AggregationAuthorization` a `PutAggregationAuthorization` response carries.
fn put_authorization(response: &Value, case: &str) -> Value {
    let authorization = &response["AggregationAuthorization"];
    assert!(
        authorization.is_object(),
        "case {case:?}: PutAggregationAuthorization must return an AggregationAuthorization, \
         got {response}"
    );
    authorization.clone()
}

/// Every authorization `DescribeAggregationAuthorizations` reports for `request`, with
/// the `NextToken` it answered.
async fn describe_page(emulator: &Emulator, request: &Value) -> (Vec<Value>, Option<String>) {
    let response = emulator
        .ok("DescribeAggregationAuthorizations", request)
        .await;
    let page = response["AggregationAuthorizations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {request}: expected a list, got {response}"))
        .clone();
    let next = response
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (page, next)
}

/// Every authorization in state, in the order the service reports them.
async fn all_authorizations(emulator: &Emulator) -> Vec<Value> {
    let (page, next) = describe_page(emulator, &json!({})).await;
    assert!(
        next.is_none(),
        "the default page carries every authorization these cases create, got {next:?}"
    );
    page
}

/// The authorized pair of `authorization`, as the service reports it.
fn pair_of(authorization: &Value) -> (String, String) {
    let account = authorization["AuthorizedAccountId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("the authorization must carry its account, got {authorization}")
        })
        .to_string();
    let region = authorization["AuthorizedAwsRegion"]
        .as_str()
        .unwrap_or_else(|| panic!("the authorization must carry its region, got {authorization}"))
        .to_string();
    (account, region)
}

/// `key` of `authorization` as a timestamp, which awsJson1_1 renders as epoch seconds.
fn time_of(authorization: &Value, key: &str, case: &str) -> f64 {
    authorization[key].as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: expected {key} to be a timestamp, got {authorization}")
    })
}

/// A table of authorized pairs spanning what the rule generalises over: which account
/// is authorized, how long and what shape the authorized Region string takes, and
/// whether the request carries tags.
fn pair_cases() -> Vec<(&'static str, &'static str, String, Value)> {
    let longest_region = "r".repeat(AWS_REGION_MAX_LENGTH);
    vec![
        (
            "one account, a commercial region",
            ACCOUNT_A,
            "us-east-1".to_string(),
            Value::Null,
        ),
        (
            "another account, another region",
            ACCOUNT_B,
            "eu-west-2".to_string(),
            json!([{ "Key": "team", "Value": "platform" }]),
        ),
        (
            "a third account, a three-part region",
            ACCOUNT_C,
            "ap-southeast-1".to_string(),
            json!([
                { "Key": "team", "Value": "security" },
                { "Key": "env", "Value": "prod" },
            ]),
        ),
        (
            "the same account in a second region",
            ACCOUNT_A,
            "eu-central-1".to_string(),
            Value::Null,
        ),
        // The published bounds of `AwsRegion`'s 1..64 length, both inside.
        (
            "a one-character region, the shortest the model allows",
            ACCOUNT_B,
            "a".to_string(),
            Value::Null,
        ),
        (
            "a sixty-four-character region, the longest the model allows",
            ACCOUNT_C,
            longest_region,
            json!([{ "Key": "edge", "Value": "bound" }]),
        ),
    ]
}

/// The put request for `account` and `region`, carrying `tags` when the case has any.
fn put_request(account: &str, region: &str, tags: &Value) -> Value {
    let mut request = json!({
        "AuthorizedAccountId": account,
        "AuthorizedAwsRegion": region,
    });
    if !tags.is_null() {
        request["Tags"] = tags.clone();
    }
    request
}

#[tokio::test]
async fn an_authorization_is_read_back_with_the_pair_it_was_granted_for() {
    for signing_region in ["us-east-1", "eu-west-1"] {
        let emulator = Emulator::in_region(signing_region);
        let mut expected_pairs: Vec<(String, String)> = Vec::new();

        for (case, account, region, tags) in pair_cases() {
            let case = format!("{signing_region}: {case}");
            let created = emulator
                .ok(
                    "PutAggregationAuthorization",
                    &put_request(account, &region, &tags),
                )
                .await;
            let created = put_authorization(&created, &case);

            assert_eq!(
                pair_of(&created),
                (account.to_string(), region.clone()),
                "case {case:?}: expected the authorization to carry the requested pair, got \
                 {created}"
            );
            // The ARN is BUILT FROM the authorized pair, in the account and Region the
            // request was signed for; nothing about it is minted.
            let expected_arn = emulator_server::emulator_arn(
                "config",
                signing_region,
                &format!("aggregation-authorization/{account}/{region}"),
            );
            assert_eq!(
                created["AggregationAuthorizationArn"].as_str(),
                Some(expected_arn.as_str()),
                "case {case:?}: expected the ARN {expected_arn:?}, got {created}"
            );
            assert!(
                time_of(&created, "CreationTime", &case) > 0.0,
                "case {case:?}: expected a wall-clock creation time, got {created}"
            );
            // Tags are metadata on the authorization, not part of it: the modelled
            // `AggregationAuthorization` carries no `Tags` member, so none is reported.
            assert!(
                created.get("Tags").is_none(),
                "case {case:?}: the reported authorization models no Tags member, got {created}"
            );

            expected_pairs.push((account.to_string(), region.clone()));
            expected_pairs.sort();

            // The describe reports exactly what the put returned, and every pair
            // granted so far, in key order: account first, then Region.
            let reported = all_authorizations(&emulator).await;
            assert_eq!(
                reported.iter().map(pair_of).collect::<Vec<_>>(),
                expected_pairs,
                "case {case:?}: expected every granted pair once in key order, got {reported:?}"
            );
            let read_back = reported
                .iter()
                .find(|authorization| {
                    pair_of(authorization) == (account.to_string(), region.clone())
                })
                .unwrap_or_else(|| panic!("case {case:?}: the granted pair must be reported"));
            assert_eq!(
                read_back, &created,
                "case {case:?}: expected the describe to report what the put returned, got \
                 {read_back}"
            );
        }
    }
}

#[tokio::test]
async fn a_repeat_put_of_the_same_pair_is_idempotent() {
    // Rows differ along what the SECOND request carries for the same pair: nothing,
    // the same tags, different tags, more tags, no tags at all.
    let cases: [(&str, Value, Value); 5] = [
        ("no tags either time", Value::Null, Value::Null),
        (
            "the same tags again",
            json!([{ "Key": "team", "Value": "platform" }]),
            json!([{ "Key": "team", "Value": "platform" }]),
        ),
        (
            "different tag values",
            json!([{ "Key": "team", "Value": "platform" }]),
            json!([{ "Key": "team", "Value": "security" }]),
        ),
        (
            "more tags than at creation",
            json!([{ "Key": "team", "Value": "platform" }]),
            json!([
                { "Key": "team", "Value": "platform" },
                { "Key": "env", "Value": "prod" },
            ]),
        ),
        (
            "tags dropped on the repeat",
            json!([{ "Key": "team", "Value": "platform" }]),
            Value::Null,
        ),
    ];

    for (case, first_tags, second_tags) in cases {
        let emulator = Emulator::new();
        let request = put_request(ACCOUNT_A, "us-west-2", &first_tags);
        let created = emulator.ok("PutAggregationAuthorization", &request).await;
        let created = put_authorization(&created, case);

        let repeat = put_request(ACCOUNT_A, "us-west-2", &second_tags);
        let again = emulator.ok("PutAggregationAuthorization", &repeat).await;
        let again = put_authorization(&again, case);

        assert_eq!(
            again, created,
            "case {case:?}: expected the repeat put to return the authorization the pair \
             already holds, got {again}"
        );
        let reported = all_authorizations(&emulator).await;
        assert_eq!(
            reported,
            vec![created.clone()],
            "case {case:?}: expected the repeat put to create no duplicate, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_pair_differing_in_either_half_is_a_different_authorization() {
    // Rows differ along WHICH half of the pair changes, and each row is compared with
    // the pair it differs from.
    let base = (ACCOUNT_A, "us-east-1");
    let cases: [(&str, &str, &str); 3] = [
        ("the same account in another region", ACCOUNT_A, "us-west-2"),
        ("another account in the same region", ACCOUNT_B, "us-east-1"),
        ("another account in another region", ACCOUNT_C, "eu-west-3"),
    ];

    let emulator = Emulator::new();
    let first = emulator
        .ok(
            "PutAggregationAuthorization",
            &put_request(base.0, base.1, &Value::Null),
        )
        .await;
    let first = put_authorization(&first, "the base pair");

    for (index, (case, account, region)) in cases.iter().enumerate() {
        let granted = emulator
            .ok(
                "PutAggregationAuthorization",
                &put_request(account, region, &Value::Null),
            )
            .await;
        let granted = put_authorization(&granted, case);
        assert_ne!(
            granted["AggregationAuthorizationArn"], first["AggregationAuthorizationArn"],
            "case {case:?}: expected an ARN of its own, got {granted}"
        );

        let reported = all_authorizations(&emulator).await;
        assert_eq!(
            reported.len(),
            index + 2,
            "case {case:?}: expected the base pair plus {} more, got {reported:?}",
            index + 1
        );
    }
}

#[tokio::test]
async fn describe_reports_the_authorizations_in_state_a_page_at_a_time() {
    let emulator = Emulator::new();
    // Granted out of key order, so the ordering under test is the store's and not the
    // order the requests arrived in.
    let granted: [(&str, &str); 5] = [
        (ACCOUNT_C, "us-east-1"),
        (ACCOUNT_A, "us-west-2"),
        (ACCOUNT_B, "eu-west-1"),
        (ACCOUNT_A, "ap-south-1"),
        (ACCOUNT_B, "ca-central-1"),
    ];
    for (account, region) in granted {
        emulator
            .ok(
                "PutAggregationAuthorization",
                &put_request(account, region, &Value::Null),
            )
            .await;
    }
    let mut expected: Vec<(String, String)> = granted
        .iter()
        .map(|(account, region)| ((*account).to_string(), (*region).to_string()))
        .collect();
    expected.sort();

    // Rows differ along the page size asked for: the modelled `Limit` default of 0
    // ("the default is maximum"), each small size, exactly what is in state, one past
    // it, and the published maximum.
    let cases: [(&str, i64, &[usize]); 6] = [
        ("the default page size", 0, &[5]),
        ("one per page", 1, &[1, 1, 1, 1, 1]),
        ("two per page", 2, &[2, 2, 1]),
        ("exactly what is in state", 5, &[5]),
        ("more than is in state", 6, &[5]),
        ("the published maximum", LIMIT_MAX, &[5]),
    ];
    for (case, limit, expected_pages) in cases {
        let mut next: Option<String> = None;
        let mut seen: Vec<(String, String)> = Vec::new();
        for (index, expected_len) in expected_pages.iter().enumerate() {
            let mut request = json!({ "Limit": limit });
            if let Some(token) = next.as_deref() {
                request["NextToken"] = json!(token);
            }
            let (page, token) = describe_page(&emulator, &request).await;
            assert_eq!(
                page.len(),
                *expected_len,
                "case {case:?}: expected {expected_len} authorizations on page {index}, got \
                 {page:?}"
            );
            seen.extend(page.iter().map(pair_of));
            next = token;
            let last_page = index + 1 == expected_pages.len();
            assert_eq!(
                next.is_none(),
                last_page,
                "case {case:?}: page {index} carried NextToken {next:?}"
            );
        }
        assert_eq!(
            seen, expected,
            "case {case:?}: expected every authorization exactly once in key order, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_describe_outside_the_modelled_paging_contract_is_refused() {
    let emulator = Emulator::new();
    // One granted pair, so every refusal below can be shown to leave the store as it
    // was for the next caller.
    emulator
        .ok(
            "PutAggregationAuthorization",
            &put_request(ACCOUNT_A, "us-east-1", &Value::Null),
        )
        .await;
    let before = all_authorizations(&emulator).await;

    let region_past_the_maximum = "r".repeat(AWS_REGION_MAX_LENGTH + 1);
    // Rows differ along WHAT is wrong. For the token: not base64 at all, base64 of
    // text carrying no key separator, base64 of a key whose halves are outside the
    // modelled constraints on an authorized pair (`AccountId`'s `^\d{12}$` pattern,
    // `AwsRegion`'s 1..64 length), and a token a sibling paged read of this service
    // issues. For the page size: a `Limit` past each end of the published 0..100 range.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a token this operation never issued",
            json!({ "NextToken": "not-a-token-!!" }),
            "InvalidNextTokenException",
        ),
        (
            "base64 of empty text",
            json!({ "NextToken": base64("") }),
            "InvalidNextTokenException",
        ),
        (
            "base64 of text carrying no key separator",
            json!({ "NextToken": base64(ACCOUNT_A) }),
            "InvalidNextTokenException",
        ),
        (
            "base64 of a key whose account id is eleven digits",
            json!({ "NextToken": base64("11112222333/us-east-1") }),
            "InvalidNextTokenException",
        ),
        (
            "base64 of a key whose account id is not digits",
            json!({ "NextToken": base64("abcdefghijkl/us-east-1") }),
            "InvalidNextTokenException",
        ),
        (
            "base64 of a key whose region is empty",
            json!({ "NextToken": base64(&format!("{ACCOUNT_A}/")) }),
            "InvalidNextTokenException",
        ),
        (
            "base64 of a key whose region is one past the modelled maximum",
            json!({ "NextToken": base64(&format!("{ACCOUNT_A}/{region_past_the_maximum}")) }),
            "InvalidNextTokenException",
        ),
        (
            "a token DescribeConfigRules would issue",
            json!({ "NextToken": base64("some-config-rule") }),
            "InvalidNextTokenException",
        ),
        (
            "one past the maximum Limit",
            json!({ "Limit": LIMIT_MAX + 1 }),
            "ValidationException",
        ),
        (
            "far past the maximum Limit",
            json!({ "Limit": 10_000 }),
            "ValidationException",
        ),
        (
            "one below the minimum Limit",
            json!({ "Limit": -1 }),
            "ValidationException",
        ),
    ];
    for (case, request, expected_code) in cases {
        let (status, code) = emulator
            .refused("DescribeAggregationAuthorizations", &request)
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected_code),
            "case {case:?}: expected HTTP 400 {expected_code}, got {status} {code}"
        );
        let after = all_authorizations(&emulator).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused page must leave the store alone, got {after:?}"
        );
    }
}

/// Grant every pair in `pairs`, in the order given.
async fn grant_all(emulator: &Emulator, pairs: &[(&str, &str)]) {
    for (account, region) in pairs {
        emulator
            .ok(
                "PutAggregationAuthorization",
                &put_request(account, region, &Value::Null),
            )
            .await;
    }
}

/// Revoke every pair in `pairs`, in the order given.
async fn revoke_all(emulator: &Emulator, pairs: &[(&str, &str)]) {
    for (account, region) in pairs {
        emulator
            .ok(
                "DeleteAggregationAuthorization",
                &json!({
                    "AuthorizedAccountId": account,
                    "AuthorizedAwsRegion": region,
                }),
            )
            .await;
    }
}

/// Every authorization a full paginated walk at `limit` reports, in the order the
/// pages carried them.
async fn walk_pages(emulator: &Emulator, limit: usize, case: &str) -> Vec<Value> {
    let mut seen: Vec<Value> = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut request = json!({ "Limit": limit });
        if let Some(token) = next.as_deref() {
            request["NextToken"] = json!(token);
        }
        let (page, token) = describe_page(emulator, &request).await;
        if token.is_some() && limit > 0 {
            assert_eq!(
                page.len(),
                limit,
                "case {case:?}: a page a NextToken follows must be full, got {page:?}"
            );
        }
        seen.extend(page);
        match token {
            Some(token) => next = Some(token),
            None => break,
        }
    }
    seen
}

/// An authorized pair as a table row names it: the authorized account id and the
/// authorized Region.
type PairRow = (&'static str, &'static str);

/// One row of [`a_page_token_resumes_in_key_order_across_grants_and_revocations`]: the
/// case name, the pairs granted between the pages, and the pairs revoked between them.
type MutationRow = (&'static str, &'static [PairRow], &'static [PairRow]);

#[tokio::test]
async fn a_page_token_resumes_in_key_order_across_grants_and_revocations() {
    /// The page size every case walks at, small enough that two pages remain after the
    /// first one.
    const PAGE: usize = 2;
    /// Account ids below and above every seeded one, so a grant between pages can land
    /// on either side of the key the first page's token resumes at.
    const ACCOUNT_LOWEST: &str = "000000000001";
    /// See [`ACCOUNT_LOWEST`].
    const ACCOUNT_HIGHEST: &str = "999999999999";

    // Granted out of key order, so what is under test is the store's ordering.
    let seeded: [PairRow; 6] = [
        (ACCOUNT_B, "us-east-1"),
        (ACCOUNT_A, "us-east-1"),
        (ACCOUNT_C, "us-east-1"),
        (ACCOUNT_A, "us-west-2"),
        (ACCOUNT_B, "eu-west-1"),
        (ACCOUNT_C, "ap-south-1"),
    ];
    // Rows differ along WHAT changes between the first page and the rest of the walk,
    // and WHERE the change falls relative to the key the token resumes at: nothing,
    // a revocation before it, of it, and after it, a grant before and after it, and a
    // grant and a revocation at once.
    let cases: [MutationRow; 7] = [
        ("nothing changes between the pages", &[], &[]),
        (
            "a pair before the resume key is revoked",
            &[],
            &[(ACCOUNT_A, "us-east-1")],
        ),
        (
            "the pair the token resumes at is revoked",
            &[],
            &[(ACCOUNT_B, "eu-west-1")],
        ),
        (
            "a pair after the resume key is revoked",
            &[],
            &[(ACCOUNT_C, "us-east-1")],
        ),
        (
            "a pair before the resume key is granted",
            &[(ACCOUNT_LOWEST, "us-east-1")],
            &[],
        ),
        (
            "a pair after the resume key is granted",
            &[(ACCOUNT_HIGHEST, "us-east-1")],
            &[],
        ),
        (
            "a grant and a revocation at once",
            &[(ACCOUNT_HIGHEST, "eu-west-1")],
            &[(ACCOUNT_B, "us-east-1")],
        ),
    ];

    for (case, granted, revoked) in cases {
        let emulator = Emulator::new();
        grant_all(&emulator, &seeded).await;

        // Every account id is twelve digits, so sorting the pairs orders them exactly
        // as their keys (account, separator, Region) order.
        let mut seeded_order: Vec<(String, String)> = seeded
            .iter()
            .map(|(account, region)| ((*account).to_string(), (*region).to_string()))
            .collect();
        seeded_order.sort();

        let (first_page, token) = describe_page(&emulator, &json!({ "Limit": PAGE })).await;
        let first_pairs: Vec<(String, String)> = first_page.iter().map(pair_of).collect();
        assert_eq!(
            first_pairs,
            seeded_order[..PAGE],
            "case {case:?}: expected the first page in key order, got {first_pairs:?}"
        );
        let token = token
            .unwrap_or_else(|| panic!("case {case:?}: a NextToken must follow a full page"));

        grant_all(&emulator, granted).await;
        revoke_all(&emulator, revoked).await;

        // The token holds the KEY the next page resumes at, so the rest of the walk
        // reports exactly the authorizations state holds at or after that key once the
        // grants and revocations above have landed.
        let resume_key = key_of(&seeded_order[PAGE]);
        let mut remaining: Vec<(String, String)> = seeded
            .iter()
            .chain(granted.iter())
            .copied()
            .filter(|pair| !revoked.contains(pair))
            .map(|(account, region)| (account.to_string(), region.to_string()))
            .collect();
        remaining.sort();
        remaining.dedup();
        let expected: Vec<(String, String)> = remaining
            .into_iter()
            .filter(|pair| key_of(pair) >= resume_key)
            .collect();

        let mut seen: Vec<(String, String)> = Vec::new();
        let mut next = Some(token);
        while let Some(token) = next {
            let request = json!({ "Limit": PAGE, "NextToken": token });
            let (page, token_after) = describe_page(&emulator, &request).await;
            assert!(
                page.len() <= PAGE,
                "case {case:?}: expected at most {PAGE} per page, got {page:?}"
            );
            assert!(
                !page.is_empty() || token_after.is_none(),
                "case {case:?}: an empty page must end the walk, got NextToken \
                 {token_after:?}"
            );
            seen.extend(page.iter().map(pair_of));
            next = token_after;
        }
        assert_eq!(
            seen, expected,
            "case {case:?}: expected the walk to resume at {resume_key:?} and report every \
             authorization at or after it, got {seen:?}"
        );

        // Across the whole walk every reported key is strictly greater than the last,
        // so nothing is reported twice and the ordering holds across the mutations.
        let walked: Vec<(String, String)> = first_pairs.into_iter().chain(seen).collect();
        assert!(
            walked
                .windows(2)
                .all(|pairs| key_of(&pairs[0]) < key_of(&pairs[1])),
            "case {case:?}: expected strictly increasing keys over the walk, got {walked:?}"
        );
    }
}

#[tokio::test]
async fn describing_authorizations_reports_state_without_changing_it() {
    // Rows differ along how many authorizations the account holds and the page size, so
    // the read is checked both when it pages and when it does not.
    let cases: [(&str, usize, usize); 5] = [
        ("an empty store", 0, 0),
        ("a single authorization", 1, 0),
        ("several on one page", 5, 0),
        ("several over three pages", 5, 2),
        ("several one page at a time", 5, 1),
    ];
    for (seed_case, count, limit) in cases {
        let case = format!("{seed_case}, Limit {limit}");
        let emulator = Emulator::new();
        let pairs: Vec<(String, String)> = (0..count)
            .map(|index| (format!("{index:012}"), "us-east-1".to_string()))
            .collect();
        for (account, region) in &pairs {
            emulator
                .ok(
                    "PutAggregationAuthorization",
                    &put_request(account, region, &Value::Null),
                )
                .await;
        }

        let first = walk_pages(&emulator, limit, &case).await;
        let again = walk_pages(&emulator, limit, &case).await;
        assert_eq!(
            again, first,
            "case {case:?}: a second read must report the same authorizations, got {again:?}"
        );
        assert_eq!(
            first.iter().map(pair_of).collect::<Vec<_>>(),
            pairs,
            "case {case:?}: expected every granted pair in key order, got {first:?}"
        );
        // A paged walk leaves the unpaged read reporting the same authorizations, so
        // paging consumed nothing.
        let unpaged = all_authorizations(&emulator).await;
        assert_eq!(
            unpaged, first,
            "case {case:?}: the default page must report what the walk did, got {unpaged:?}"
        );
    }
}

#[tokio::test]
async fn a_revoked_authorization_stops_being_reported() {
    // Rows differ along WHICH pair of the three granted is revoked, and the last row
    // revokes a pair that was never authorized at all — which the model gives no
    // not-found error for.
    let granted: [(&str, &str); 3] = [
        (ACCOUNT_A, "us-east-1"),
        (ACCOUNT_A, "us-west-2"),
        (ACCOUNT_B, "eu-west-1"),
    ];
    let cases: [(&str, &str, &str); 4] = [
        ("the first pair", ACCOUNT_A, "us-east-1"),
        ("a second region of the same account", ACCOUNT_A, "us-west-2"),
        ("another account", ACCOUNT_B, "eu-west-1"),
        ("a pair that was never authorized", ACCOUNT_C, "ap-south-1"),
    ];

    for (case, account, region) in cases {
        let emulator = Emulator::new();
        for (granted_account, granted_region) in granted {
            emulator
                .ok(
                    "PutAggregationAuthorization",
                    &put_request(granted_account, granted_region, &Value::Null),
                )
                .await;
        }

        emulator
            .ok(
                "DeleteAggregationAuthorization",
                &json!({
                    "AuthorizedAccountId": account,
                    "AuthorizedAwsRegion": region,
                }),
            )
            .await;

        let mut expected: Vec<(String, String)> = granted
            .iter()
            .filter(|(granted_account, granted_region)| {
                (*granted_account, *granted_region) != (account, region)
            })
            .map(|(granted_account, granted_region)| {
                ((*granted_account).to_string(), (*granted_region).to_string())
            })
            .collect();
        expected.sort();
        let reported = all_authorizations(&emulator).await;
        assert_eq!(
            reported.iter().map(pair_of).collect::<Vec<_>>(),
            expected,
            "case {case:?}: expected exactly the pairs that were not revoked, got {reported:?}"
        );

        // A revoked pair can be granted again, and comes back as a NEW authorization.
        let regranted = emulator
            .ok(
                "PutAggregationAuthorization",
                &put_request(account, region, &Value::Null),
            )
            .await;
        let regranted = put_authorization(&regranted, case);
        assert_eq!(
            pair_of(&regranted),
            (account.to_string(), region.to_string()),
            "case {case:?}: expected the pair to be grantable again, got {regranted}"
        );
        assert_eq!(
            all_authorizations(&emulator).await.len(),
            expected.len() + 1,
            "case {case:?}: expected the regranted pair beside the surviving ones"
        );
    }
}

#[tokio::test]
async fn the_authorization_store_is_capped_at_this_regions_bound() {
    // The bound this emulator holds aggregation authorizations under, read from the
    // constant the handler enforces so the test cannot drift from it; the cases below
    // sit either side of it.
    const BOUND: usize = service_com_amazonaws_configservice_starlingdoveservice::state::AGGREGATION_AUTHORIZATIONS_QUOTA;

    let emulator = Emulator::new();
    for index in 0..BOUND {
        let account = format!("{index:012}");
        emulator
            .ok(
                "PutAggregationAuthorization",
                &put_request(&account, "us-east-1", &Value::Null),
            )
            .await;
    }

    // A pair already authorized is idempotent, so it is still accepted at the bound.
    let repeat = emulator
        .ok(
            "PutAggregationAuthorization",
            &put_request(&format!("{:012}", 0), "us-east-1", &Value::Null),
        )
        .await;
    assert!(
        put_authorization(&repeat, "a repeat at the bound")
            .get("AggregationAuthorizationArn")
            .is_some(),
        "a repeat of an authorized pair consumes nothing and is never refused, got {repeat}"
    );

    // Rows differ along WHICH half of the new pair is new: a new account, and a new
    // Region for an account that already holds one.
    let refused: [(&str, String, &str); 2] = [
        ("a new account", format!("{BOUND:012}"), "us-east-1"),
        (
            "a new region for an authorized account",
            format!("{:012}", 0),
            "eu-west-1",
        ),
    ];
    for (case, account, region) in refused {
        let (status, code) = emulator
            .refused(
                "PutAggregationAuthorization",
                &put_request(&account, region, &Value::Null),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?}: expected HTTP 400 InvalidParameterValueException past the bound, \
             got {status} {code}"
        );
    }

    // Revoking one authorization makes room for exactly one more.
    emulator
        .ok(
            "DeleteAggregationAuthorization",
            &json!({
                "AuthorizedAccountId": format!("{:012}", 0),
                "AuthorizedAwsRegion": "us-east-1",
            }),
        )
        .await;
    let granted = emulator
        .ok(
            "PutAggregationAuthorization",
            &put_request(&format!("{BOUND:012}"), "us-east-1", &Value::Null),
        )
        .await;
    assert_eq!(
        pair_of(&put_authorization(&granted, "after a revocation")),
        (format!("{BOUND:012}"), "us-east-1".to_string()),
        "a revocation makes room for a new pair, got {granted}"
    );
}

#[tokio::test]
async fn a_put_outside_the_modelled_constraints_is_refused_and_persists_nothing() {
    let too_long_region = "r".repeat(AWS_REGION_MAX_LENGTH + 1);
    // Rows differ along WHICH member is outside its modelled constraint and HOW:
    // `AuthorizedAccountId`'s `^\d{12}$` pattern (too short, too long, non-digits,
    // separators, empty) and `AwsRegion`'s 1..64 length, each side of the bound.
    let cases: Vec<(&str, Value)> = vec![
        (
            "an account id of eleven digits",
            put_request("11112222333", "us-east-1", &Value::Null),
        ),
        (
            "an account id of thirteen digits",
            put_request("1111222233334", "us-east-1", &Value::Null),
        ),
        (
            "an account id that is not digits at all",
            put_request("not-an-account", "us-east-1", &Value::Null),
        ),
        (
            "an account id with a separator",
            put_request("1111-2222-333", "us-east-1", &Value::Null),
        ),
        (
            "an empty account id",
            put_request("", "us-east-1", &Value::Null),
        ),
        ("an empty region", put_request(ACCOUNT_A, "", &Value::Null)),
        (
            "a region one character past the modelled maximum",
            put_request(ACCOUNT_A, &too_long_region, &Value::Null),
        ),
        (
            "no authorized account id at all",
            json!({ "AuthorizedAwsRegion": "us-east-1" }),
        ),
        (
            "no authorized region at all",
            json!({ "AuthorizedAccountId": ACCOUNT_A }),
        ),
    ];

    let emulator = Emulator::new();
    for (case, request) in cases {
        // The two operations take the SAME pair of members under the same modelled
        // constraints, so both refuse every case the same way.
        for operation in [
            "PutAggregationAuthorization",
            "DeleteAggregationAuthorization",
        ] {
            let (status, code) = emulator.refused(operation, &request).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ValidationException"),
                "case {operation}/{case:?}: expected HTTP 400 ValidationException, got \
                 {status} {code}"
            );
        }
    }

    let reported = all_authorizations(&emulator).await;
    assert!(
        reported.is_empty(),
        "expected a refused put to persist nothing, got {reported:?}"
    );
}
