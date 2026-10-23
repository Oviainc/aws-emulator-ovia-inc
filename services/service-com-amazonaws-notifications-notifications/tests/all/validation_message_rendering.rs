// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a `ValidationException` the boundary validator raised,
//! driven over HTTP through the same multi-service router the emulator boots in
//! production.
//!
//! The rule under test: a numeric-bound (`@range`) violation names the member that
//! failed and the bound it broke and NEVER renders the number that was submitted, so
//! two different rejected values at one member produce the SAME message; every other
//! violation class keeps the framework's wording, which does quote the submitted value
//! (and `Value null at '<member>'` for an absent one).
//!
//! The table spans every operation of this service that binds a `maxResults` with a
//! modelled `@range`, so the rows differ in the bounds themselves (`3..3`, `1..100`,
//! `1..1000`), in whether the operation is implemented or still answers the scaffold's
//! 501, and in whether it carries required members beside the page size. Each row is
//! exercised at both bounds, one step outside each bound, and far outside each bound.
//! Catalog source: `smithy_query(query="operation",operation="ListNotificationHubs,
//! ListNotificationConfigurations,ListChannels,ListEventRules",
//! model_path="services/service-com-amazonaws-notifications-notifications/smithy-model/com-amazonaws-notifications-notifications.json")`,
//! whose `input.maxResults` carries `{"httpQuery":"maxResults","range":{...}}` for each.

use serde_json::Value;

// The HTTP harness this suite drives the service through, shared with every other
// suite in this crate.
use crate::harness::{error_code, Emulator};

/// The member every row's page size is bound to, which is also the path the validating
/// walk reports a violation of it at.
const PAGE_SIZE_MEMBER: &str = "maxResults";

/// A `NotificationConfigurationArn` satisfying the pattern the model declares for it
/// (`^arn:[a-z-]{3,10}:notifications::[0-9]{12}:configuration/[a-z0-9]{27}$`), so a row
/// that requires one is refused for its page size alone and not for the ARN too.
const CONFIGURATION_ARN: &str =
    "arn:aws:notifications::123456789012:configuration/abcdefghij0123456789abcdefg";

/// One operation that binds a `maxResults` with a modelled `@range`.
struct RangedPage {
    case: &'static str,
    /// The `@http` URI the model binds the operation to.
    path: &'static str,
    /// Whether the operation also requires a `notificationConfigurationArn` query
    /// member, which the request then carries as [`CONFIGURATION_ARN`].
    requires_configuration_arn: bool,
    /// The `@range` the model declares on `maxResults`.
    min: i64,
    max: i64,
}

/// Catalog source: the `maxResults` member of each operation's input shape (see the
/// module header's `smithy_query` call).
const RANGED_PAGES: &[RangedPage] = &[
    RangedPage {
        case: "ListNotificationHubs — a range pinned to a single permitted value",
        path: "/notification-hubs",
        requires_configuration_arn: false,
        min: 3,
        max: 3,
    },
    RangedPage {
        case: "ListNotificationConfigurations — a hundred-wide range, implemented",
        path: "/notification-configurations",
        requires_configuration_arn: false,
        min: 1,
        max: 100,
    },
    RangedPage {
        case: "ListChannels — a hundred-wide range beside a required member",
        path: "/channels",
        requires_configuration_arn: true,
        min: 1,
        max: 100,
    },
    RangedPage {
        case: "ListEventRules — a thousand-wide range beside a required member",
        path: "/event-rules",
        requires_configuration_arn: true,
        min: 1,
        max: 1000,
    },
];

impl RangedPage {
    /// The listing request for one page size.
    fn uri(&self, requested: i64) -> String {
        if self.requires_configuration_arn {
            format!(
                "{}?notificationConfigurationArn={CONFIGURATION_ARN}&{PAGE_SIZE_MEMBER}={requested}",
                self.path
            )
        } else {
            format!("{}?{PAGE_SIZE_MEMBER}={requested}", self.path)
        }
    }

    /// Page sizes below the declared minimum: one step under it, two steps under it,
    /// and far under it (zero or negative, which no bound admits).
    fn below_minimum(&self) -> Vec<i64> {
        vec![self.min - 1, self.min - 2, self.min - 1_000]
    }

    /// Page sizes above the declared maximum: one step over it, two steps over it, and
    /// far over it.
    fn above_maximum(&self) -> Vec<i64> {
        vec![self.max + 1, self.max + 2, self.max + 10_000]
    }
}

/// The `message` member of an error body, or `None` for a body that carries none (a
/// successful response, or the scaffold's own 501 envelope).
fn stated_message(response: &Value) -> Option<&str> {
    response["message"].as_str()
}

/// The `message` member of an error body, which an error response must carry.
fn message(case: &str, response: &Value) -> String {
    stated_message(response)
        .unwrap_or_else(|| panic!("case {case:?}: an error body carries a message, got {response:?}"))
        .to_string()
}

/// The whole message a single-clause refusal of a page size BELOW its range states.
fn below_minimum_message(min: i64) -> String {
    format!(
        "1 validation error detected: Value at '{PAGE_SIZE_MEMBER}' failed to satisfy \
         constraint: Member must have value greater than or equal to {min}"
    )
}

/// The whole message a single-clause refusal of a page size ABOVE its range states.
fn above_maximum_message(max: i64) -> String {
    format!(
        "1 validation error detected: Value at '{PAGE_SIZE_MEMBER}' failed to satisfy \
         constraint: Member must have value less than or equal to {max}"
    )
}

/// RULE: a page size outside the `@range` the model declares is refused with the
/// modelled `ValidationException` (HTTP 400) whose message names the member and the
/// bound it broke — and nothing of the number submitted, so every rejected value past
/// one bound reads identically.
#[tokio::test]
async fn a_page_size_outside_its_range_is_refused_naming_the_bound_and_not_the_value() {
    let emulator = Emulator::new();

    for page in RANGED_PAGES {
        for (direction, requested, expected) in page
            .below_minimum()
            .into_iter()
            .map(|requested| ("below the minimum", requested, below_minimum_message(page.min)))
            .chain(
                page.above_maximum()
                    .into_iter()
                    .map(|requested| ("above the maximum", requested, above_maximum_message(page.max))),
            )
        {
            let case = format!("{} / {direction} / maxResults={requested}", page.case);
            let (status, refused) = emulator.send("GET", &page.uri(requested), None).await;
            assert_eq!(
                status, 400,
                "case {case:?}: a page size outside its range is refused with the modelled 400, got {refused:?}"
            );
            assert!(
                error_code(&refused).contains("ValidationException"),
                "case {case:?}: that refusal is a ValidationException, got {refused:?}"
            );
            let stated = message(&case, &refused);
            assert_eq!(
                stated, expected,
                "case {case:?}: expected message {expected:?}, got {stated:?}"
            );
            assert!(
                !stated.contains(&format!("'{requested}'")),
                "case {case:?}: the message must not render the submitted value, got {stated:?}"
            );
        }
    }
}

/// RULE: each published bound of the range is itself permitted, so the refusal above is
/// the bound being broken and not the member being present. A bound that the operation
/// implements answers 200; one whose operation is still a scaffold answers its own 501 —
/// either way the request is not refused as invalid.
#[tokio::test]
async fn each_published_bound_of_a_page_size_is_accepted() {
    let emulator = Emulator::new();

    for page in RANGED_PAGES {
        for requested in [page.min, page.max] {
            let case = format!("{} / at the bound / maxResults={requested}", page.case);
            let (status, response) = emulator.send("GET", &page.uri(requested), None).await;
            assert_ne!(
                status, 400,
                "case {case:?}: a page size AT a published bound is not a validation failure, got {response:?}"
            );
            assert!(
                stated_message(&response)
                    .is_none_or(|stated| !stated.contains(PAGE_SIZE_MEMBER)),
                "case {case:?}: nothing is stated about the page size, got {response:?}"
            );
        }
    }
}

/// RULE (the positive control of the listing the rule was found on): the one page size
/// `ListNotificationHubs` permits is served, and the required `notificationHubs` member
/// is reported.
#[tokio::test]
async fn the_one_permitted_page_size_of_the_hub_listing_is_served() {
    let emulator = Emulator::new();
    let page = &RANGED_PAGES[0];

    for requested in [page.min, page.max] {
        let case = format!("the hub listing at maxResults={requested}");
        let (status, listed) = emulator.send("GET", &page.uri(requested), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the permitted page size is served, got {listed:?}"
        );
        assert!(
            listed["notificationHubs"].is_array(),
            "case {case:?}: the listing reports the required notificationHubs member, got {listed:?}"
        );
    }
}

/// RULE: only the numeric-bound clause withholds the submitted value. Every other
/// violation class keeps the framework's wording — a present-but-invalid value is quoted
/// back as `Value '<value>' at '<member>'`, and an absent required member reads
/// `Value null at '<member>'`.
#[tokio::test]
async fn other_violation_classes_still_quote_the_value_they_refused() {
    let emulator = Emulator::new();

    // A `nextToken` that satisfies no `@pattern`: the clause quotes it back. The
    // asterisks are what the modelled `^[\w+-/=]+$` refuses — its `+-/` is a range over
    // `+,-.` and `/`, so `*` sits just below it — and they survive the query string as
    // sent, so the value the message echoes is comparable with the value submitted.
    let offending = "not*a*token";
    let (status, refused) = emulator
        .send("GET", &format!("/notification-hubs?nextToken={offending}"), None)
        .await;
    let case = "a nextToken outside its pattern";
    assert_eq!(
        status, 400,
        "case {case:?}: a value outside its pattern is refused with 400, got {refused:?}"
    );
    let stated = message(case, &refused);
    let expected = format!(
        "1 validation error detected: Value '{offending}' at 'nextToken' failed to satisfy \
         constraint: Member must satisfy regular expression pattern:"
    );
    assert!(
        stated.starts_with(&expected),
        "case {case:?}: expected a message starting {expected:?}, got {stated:?}"
    );

    // An absent required member: there is no submitted value to state. Asserted on
    // `RegisterNotificationHub`, whose `notificationHubRegion` the model marks
    // `@required` in the request body.
    let case = "an absent required member";
    let (status, refused) = emulator
        .send("POST", "/notification-hubs", Some(serde_json::json!({})))
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: an absent required member is refused with 400, got {refused:?}"
    );
    let stated = message(case, &refused);
    assert_eq!(
        stated,
        "1 validation error detected: Value null at 'notificationHubRegion' failed to \
         satisfy constraint: Member must not be null",
        "case {case:?}: the absent-member form is unchanged, got {stated:?}"
    );

    // Both classes in one request: one envelope, each clause worded by its own class.
    let case = "a page size out of range beside a token outside its pattern";
    let (status, refused) = emulator
        .send(
            "GET",
            &format!("/notification-hubs?maxResults=7&nextToken={offending}"),
            None,
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: both violations are refused with 400, got {refused:?}"
    );
    let stated = message(case, &refused);
    assert!(
        stated.starts_with("2 validation errors detected: "),
        "case {case:?}: two violations make one two-clause envelope, got {stated:?}"
    );
    assert!(
        stated.contains(
            "Value at 'maxResults' failed to satisfy constraint: Member must have value less \
             than or equal to 3"
        ),
        "case {case:?}: the range clause withholds the submitted value, got {stated:?}"
    );
    assert!(
        stated.contains(&format!("Value '{offending}' at 'nextToken'")),
        "case {case:?}: the pattern clause quotes the submitted value, got {stated:?}"
    );
    assert!(
        !stated.contains("'7'"),
        "case {case:?}: the rejected page size is nowhere in the message, got {stated:?}"
    );
}

/// RULE: a listing the service does hold state for keeps serving after a refusal — a
/// refused request changes nothing. Asserted on the one list operation whose page size
/// the configuration ARN does not gate.
#[tokio::test]
async fn a_refused_page_size_changes_nothing_the_listing_reports() {
    let emulator = Emulator::new();
    let page = &RANGED_PAGES[0];

    let (_, before) = emulator.send("GET", page.path, None).await;
    let (status, _) = emulator.send("GET", &page.uri(page.max + 1), None).await;
    assert_eq!(status, 400, "the refused page size is refused");
    let (status, after) = emulator.send("GET", page.path, None).await;
    assert_eq!(status, 200, "a listing after a refusal answers 200, got {after:?}");
    assert_eq!(
        before, after,
        "a refused request must not change what the next listing reports, got {after:?}"
    );
}
