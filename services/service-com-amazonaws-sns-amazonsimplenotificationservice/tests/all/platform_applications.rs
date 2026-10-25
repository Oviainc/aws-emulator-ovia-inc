// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListPlatformApplications`, the mobile-push application
//! listing.
//!
//! The rule under test: a platform-application listing REPORTS one entry per
//! application this account holds — each carrying the ARN its record is held under and
//! the attributes that record holds, with the write-only push credential material
//! withheld — ordered oldest first and paged at up to 100 applications, with a
//! `NextToken` returned only while applications remain and a token the listing did not
//! mint refused as bad input; the listing itself changes nothing.
//!
//! The rows below differ in the PLATFORM, the application NAME, the attribute SET each
//! application holds (none, one, several, credential-bearing) and the application
//! COUNT walked, which is what the rule generalises over. The records are seeded
//! straight into persisted state because the operation that registers one,
//! `CreatePlatformApplication`, is still a scaffold stub with an issue of its own
//! (`20261025T025317Z-sns-createplatformapplication-stub.md`) — the same way the SMS
//! sandbox suite seeds a status no implemented call writes yet. Every listing call
//! goes over the wire through the router the emulator serves in production, so nothing
//! but persisted state carries information between calls.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_sns_amazonsimplenotificationservice::state::{
    platform_application_arn, PlatformApplicationRecord, INVALID_NEXT_TOKEN_REFUSAL, PAGE_SIZE,
    PLATFORM_CREDENTIAL_ATTRIBUTE_NAMES,
};

use super::topics_subscriptions_publish::{text_of, Answer, Sns};

/// The region the test harness signs its calls for, and so the region the seeded ARNs
/// carry.
const REGION: &str = "us-east-1";

/// Every `<key>…</key>` / `<value>…</value>` pair inside `block`, in document order.
fn entries_of(block: &str) -> HashMap<String, String> {
    let mut found = HashMap::new();
    let mut rest = block;
    while let Some(start) = rest.find("<entry>") {
        rest = &rest[start + "<entry>".len()..];
        let Some(end) = rest.find("</entry>") else {
            break;
        };
        let entry = &rest[..end];
        found.insert(text_of(entry, "key"), text_of(entry, "value"));
        rest = &rest[end + "</entry>".len()..];
    }
    found
}

/// The body of the response's `<PlatformApplications>` element, or `None` when the
/// response carries no such member at all — a refused call serves no listing.
fn applications_block(body: &str) -> Option<&str> {
    if let Some(start) = body.find("<PlatformApplications>") {
        let rest = &body[start + "<PlatformApplications>".len()..];
        let end = rest.find("</PlatformApplications>")?;
        return Some(&rest[..end]);
    }
    body.contains("<PlatformApplications/>").then_some("")
}

/// The `(PlatformApplicationArn, Attributes)` pairs the listing carries, in document
/// order.
///
/// A list member's open tag carries the envelope's default namespace, so the opening
/// is matched up to its first `>` rather than as a bare `<member>`.
fn reported(answer: &Answer) -> Vec<(String, HashMap<String, String>)> {
    let block = applications_block(&answer.body)
        .unwrap_or_else(|| panic!("no PlatformApplications member in {}", answer.body));
    let mut found = Vec::new();
    let mut rest = block;
    while let Some(start) = rest.find("<member") {
        rest = &rest[start..];
        let Some(open) = rest.find('>') else { break };
        rest = &rest[open + 1..];
        let Some(end) = rest.find("</member>") else {
            break;
        };
        let member = &rest[..end];
        found.push((
            text_of(member, "PlatformApplicationArn"),
            entries_of(member),
        ));
        rest = &rest[end + "</member>".len()..];
    }
    found
}

/// The ARNs the listing reports, in document order.
fn reported_arns(answer: &Answer) -> Vec<String> {
    reported(answer).into_iter().map(|(arn, _)| arn).collect()
}

/// One listing call, with the optional paging parameter.
async fn list(sns: &Sns, params: &[(&str, &str)]) -> Answer {
    sns.call("ListPlatformApplications", params).await
}

/// Write one platform application into persisted state, `created_at` ordering it, and
/// answer its ARN.
fn seed(
    sns: &Sns,
    at: u64,
    platform: &str,
    name: &str,
    attributes: &[(&str, &str)],
) -> String {
    let arn = platform_application_arn(REGION, platform, name);
    let state = sns.state();
    let mut state = state.lock();
    state.platform_applications.insert(
        arn.clone(),
        PlatformApplicationRecord {
            arn: arn.clone(),
            platform: platform.to_string(),
            name: name.to_string(),
            attributes: attributes
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
            created_at: SystemTime::UNIX_EPOCH + Duration::from_secs(at + 1),
        },
    );
    arn
}

/// One row of the listing table: the case, the platform, the application name and the
/// attributes its record holds.
type ApplicationCase<'a> = (&'a str, &'a str, &'a str, &'a [(&'a str, &'a str)]);

/// The applications the listing table seeds.
///
/// The platforms are the ones `CreatePlatformApplicationInput$Platform` and the
/// `CreatePlatformApplication` doc name (ADM, APNS, APNS_SANDBOX, GCM, Baidu, MPNS,
/// WNS); the attribute names are from the
/// `SetPlatformApplicationAttributesInput$Attributes` doc
/// (`smithy_query(query="doc", shape="SetPlatformApplicationAttributesInput",
/// member="Attributes")`), which lists `PlatformCredential`, `PlatformPrincipal`, the
/// four `Event*` topics, the two feedback role ARNs, `SuccessFeedbackSampleRate` and
/// the two APNs token-based `ApplePlatform*` names.
fn application_cases() -> Vec<ApplicationCase<'static>> {
    vec![
        ("no attributes at all", "GCM", "push-gcm-bare", &[]),
        (
            "one event topic",
            "ADM",
            "push-adm-events",
            &[(
                "EventEndpointCreated",
                "arn:aws:sns:us-east-1:000000000000:endpoint-created",
            )],
        ),
        (
            "credential material only",
            "APNS",
            "push-apns-credentials",
            &[
                ("PlatformCredential", "-----BEGIN PRIVATE KEY-----"),
                ("PlatformPrincipal", "signing-key-id"),
            ],
        ),
        (
            "credential material beside reportable attributes",
            "APNS_SANDBOX",
            "push-apns-sandbox-mixed",
            &[
                ("PlatformCredential", "sandbox-signing-key"),
                ("PlatformPrincipal", "sandbox-signing-key-id"),
                ("ApplePlatformTeamID", "TEAM123456"),
                ("ApplePlatformBundleID", "com.example.app"),
                ("SuccessFeedbackSampleRate", "100"),
            ],
        ),
        (
            "the feedback roles and every event topic",
            "Baidu",
            "push-baidu-feedback",
            &[
                (
                    "EventEndpointCreated",
                    "arn:aws:sns:us-east-1:000000000000:created",
                ),
                (
                    "EventEndpointDeleted",
                    "arn:aws:sns:us-east-1:000000000000:deleted",
                ),
                (
                    "EventEndpointUpdated",
                    "arn:aws:sns:us-east-1:000000000000:updated",
                ),
                (
                    "EventDeliveryFailure",
                    "arn:aws:sns:us-east-1:000000000000:failed",
                ),
                (
                    "SuccessFeedbackRoleArn",
                    "arn:aws:iam::000000000000:role/sns-success",
                ),
                (
                    "FailureFeedbackRoleArn",
                    "arn:aws:iam::000000000000:role/sns-failure",
                ),
                ("SuccessFeedbackSampleRate", "0"),
            ],
        ),
        (
            "a name carrying the punctuation the name rule allows",
            "MPNS",
            "push.mpns_app-9",
            &[("SuccessFeedbackSampleRate", "50")],
        ),
        (
            "a value the envelope must escape",
            "WNS",
            "push-wns-escaped",
            &[("EventDeliveryFailure", "arn:aws:sns:us-east-1:000000000000:a&b")],
        ),
    ]
}

/// The attributes a record's listing entry must carry: every attribute it holds except
/// the write-only credential material.
fn expected_attributes(attributes: &[(&str, &str)]) -> HashMap<String, String> {
    attributes
        .iter()
        .filter(|(key, _)| !PLATFORM_CREDENTIAL_ATTRIBUTE_NAMES.contains(key))
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The listing reports one entry per application the account holds, each with the ARN
/// its record is held under and the attributes that record holds — credential material
/// withheld — oldest first, and reading it twice reports the same thing because a read
/// changes nothing.
#[tokio::test]
async fn the_listing_reports_every_application_with_the_attributes_it_holds() {
    let sns = Sns::new();

    let empty = list(&sns, &[]).await;
    assert_eq!(
        empty.status, 200,
        "an account holding no application: expected 200, got {} / {}",
        empty.status, empty.body
    );
    assert!(
        reported(&empty).is_empty(),
        "an account holding no application: expected no entries, got {:?}",
        reported_arns(&empty)
    );
    assert!(
        empty.element("NextToken").is_none(),
        "an account holding no application: expected no NextToken, got {}",
        empty.body
    );

    let cases = application_cases();
    let mut seeded: Vec<String> = Vec::new();
    for (at, (case, platform, name, attributes)) in cases.iter().enumerate() {
        let arn = seed(&sns, at as u64, platform, name, attributes);
        seeded.push(arn.clone());

        let answer = list(&sns, &[]).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} / {}",
            answer.status, answer.body
        );
        let entries = reported(&answer);
        assert_eq!(
            entries.len(),
            at + 1,
            "case {case:?}: expected {} entries, got {:?}",
            at + 1,
            reported_arns(&answer)
        );

        let held: Vec<&(String, HashMap<String, String>)> = entries
            .iter()
            .filter(|(reported_arn, _)| *reported_arn == arn)
            .collect();
        assert_eq!(
            held.len(),
            1,
            "case {case:?}: expected exactly one entry for {arn}, got {:?}",
            reported_arns(&answer)
        );
        let expected = expected_attributes(attributes);
        assert_eq!(
            held[0].1, expected,
            "case {case:?}: expected the attributes {expected:?}, got {:?}",
            held[0].1
        );
        for (key, value) in attributes.iter() {
            if !PLATFORM_CREDENTIAL_ATTRIBUTE_NAMES.contains(key) {
                continue;
            }
            assert!(
                !answer.body.contains(value),
                "case {case:?}: the listing must withhold the {key} value, got {}",
                answer.body
            );
        }
    }

    let first = list(&sns, &[]).await;
    assert_eq!(
        reported_arns(&first),
        seeded,
        "expected every seeded application once, oldest first, got {}",
        first.body
    );

    let second = list(&sns, &[]).await;
    assert_eq!(
        reported(&second),
        reported(&first),
        "a second read must report the same applications and attributes, got {}",
        second.body
    );
    assert_eq!(
        sns.state().lock().platform_applications.len(),
        seeded.len(),
        "a listing must not add or drop a record"
    );
}

/// The listing pages at up to [`PAGE_SIZE`] applications and the token walks the
/// pages: the pages concatenate to every application the account holds, oldest first,
/// each application once, and only the last page carries no token.
#[tokio::test]
async fn the_listing_pages_at_the_documented_page_size_and_the_token_walks_each_once() {
    let sns = Sns::new();

    // Two full pages and a partial third, so a page boundary falls both on and off the
    // end of the set.
    let count = PAGE_SIZE * 2 + PAGE_SIZE / 2;
    let seeded: Vec<String> = (0..count)
        .map(|at| seed(&sns, at as u64, "GCM", &format!("paged-{at:04}"), &[]))
        .collect();

    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut params: Vec<(&str, &str)> = Vec::new();
        if let Some(token) = token.as_deref() {
            params.push(("NextToken", token));
        }
        let answer = list(&sns, &params).await;
        assert_eq!(
            answer.status, 200,
            "page {pages}: expected 200, got {} / {}",
            answer.status, answer.body
        );
        let page = reported_arns(&answer);
        assert!(
            page.len() <= PAGE_SIZE,
            "page {pages}: expected at most {PAGE_SIZE} applications, got {}",
            page.len()
        );
        assert!(
            !page.is_empty(),
            "page {pages}: a page a token led to must not be empty"
        );
        walked.extend(page);
        pages += 1;
        token = answer.element("NextToken");
        if token.is_none() {
            break;
        }
        assert!(
            walked.len() < seeded.len(),
            "a NextToken was returned with every application already reported, got {} of {}",
            walked.len(),
            seeded.len()
        );
    }

    assert_eq!(
        walked, seeded,
        "expected every application once, oldest first, got {} of {}",
        walked.len(),
        seeded.len()
    );
    let expected_pages = seeded.len().div_ceil(PAGE_SIZE);
    assert_eq!(
        pages, expected_pages,
        "expected {expected_pages} pages, got {pages}"
    );
}

/// A `NextToken` the listing did not mint is refused as bad input — the modelled
/// `InvalidParameterException`, the service's sentence for the member — and the
/// account's applications are left alone.
#[tokio::test]
async fn a_next_token_the_listing_did_not_mint_is_refused() {
    let sns = Sns::new();
    let platforms = ["ADM", "APNS", "GCM"];
    for (at, platform) in platforms.iter().enumerate() {
        seed(&sns, at as u64, platform, &format!("held-{at}"), &[]);
    }

    let cases: &[(&str, &str)] = &[
        ("not a token at all", "bogus"),
        ("past the end of the set", "9999"),
        ("negative", "-1"),
        ("fractional", "1.5"),
        ("whitespace", " "),
        ("an ARN", "arn:aws:sns:us-east-1:000000000000:app/GCM/held-0"),
    ];
    for (case, token) in cases {
        let answer = list(&sns, &[("NextToken", token)]).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "InvalidParameter",
            "case {case:?}: expected the InvalidParameter code, got {}",
            answer.body
        );
        assert_eq!(
            answer.element("Message").as_deref(),
            Some(INVALID_NEXT_TOKEN_REFUSAL),
            "case {case:?}: expected the member's refusal sentence, got {}",
            answer.body
        );
        assert!(
            applications_block(&answer.body).is_none(),
            "case {case:?}: a refused request must serve no listing, got {}",
            answer.body
        );
    }

    let answer = list(&sns, &[]).await;
    assert_eq!(
        reported_arns(&answer).len(),
        platforms.len(),
        "the refused listings must leave the account's applications alone, got {}",
        answer.body
    );
}
