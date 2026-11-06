// SPDX-License-Identifier: CC0-1.0

//! What every operation that resolves a `landingZoneIdentifier` answers when the value
//! names no landing zone this account holds, end to end over HTTP.
//!
//! Driven through the same [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter)
//! the production emulator dispatches with, so what these tests exercise is the wire
//! behaviour a client sees — status codes, error codes and message bodies — not the
//! handlers' Rust signatures.
//!
//! The rule under test: a `landingZoneIdentifier` of the ARN form this service reports for
//! a landing zone, naming none the account holds, is the modelled `ResourceNotFoundException`
//! (404) reporting that the MANAGEMENT ACCOUNT and the REGION OF THE REQUEST hold no landing
//! zone identifier value — so the two variable parts of the message are the account this
//! emulator presents and the Region the request was signed for, and the identifier the
//! caller sent appears in it nowhere.
//!
//! Three sides of it are asserted, each over a TABLE whose rows differ in what the rule
//! generalises over — which operation resolves the member, which well-formed landing zone
//! ARN is sent (its partition, Region, account and resource id, including an id at the
//! published length), which Region the request is signed for, and whether the account holds
//! a landing zone of its own:
//!
//! * every one of those operations answers the one wording, in an account holding no landing
//!   zone and in one holding another;
//! * the Region named is the REQUEST's, so the same identifier answers differently when
//!   signed for another Region, and the account named is the emulator's own;
//! * a miss changes nothing: the landing zone the account does hold still resolves after
//!   them.
//!
//! Every status and error code written out by hand names its source:
//! `smithy_query(query="operation", operation="GetLandingZone,UpdateLandingZone,
//! ResetLandingZone,DeleteLandingZone")` on this crate's `smithy-model/` — all four model
//! `ResourceNotFoundException` (404) and `ValidationException` (400), `GetLandingZone` and
//! `DeleteLandingZone` take `landingZoneIdentifier` alone, `UpdateLandingZone` takes a
//! required `version` as well, and `landingZoneIdentifier` is a plain `String` with no
//! pattern of its own. The landing zone ARN form is this service's
//! (`arn:<partition>:controltower:<region>:<account>:landingzone/<id>`), as
//! `tests/all/landing_zone_identifier.rs` pins it. The emulator and the harness are the
//! sibling suites'.

use crate::landing_zone_lifecycle::{call_in_region, emulator, set_up_emulator, Answer};

/// One operation that resolves a `landingZoneIdentifier`, with the other members its input
/// requires.
#[derive(Clone, Copy)]
struct Resolver {
    path: &'static str,
    /// The members the request carries besides `landingZoneIdentifier`, as JSON object
    /// entries.
    extra: &'static [(&'static str, &'static str)],
}

/// Every operation under the rule: the read, the update, the re-deployment and the
/// decommissioning.
const RESOLVERS: &[Resolver] = &[
    Resolver {
        path: "/get-landingzone",
        extra: &[],
    },
    Resolver {
        // `version` is required, and the catalog check on it is a refusal about the REQUEST,
        // so a version the catalog offers is sent to reach the identifier's own lookup.
        path: "/update-landingzone",
        extra: &[("version", "3.3")],
    },
    Resolver {
        path: "/reset-landingzone",
        extra: &[],
    },
    Resolver {
        path: "/delete-landingzone",
        extra: &[],
    },
];

/// The landing zone ARNs no account in these tests holds: rows differing in the partition,
/// the Region and account segments, and the resource id — including one of the 16-character
/// form this service mints and one carrying the other characters the segment admits.
fn unheld_identifiers() -> Vec<(&'static str, String)> {
    vec![
        (
            "a landing zone ARN of the signed Region",
            "arn:aws:controltower:us-west-2:000000000000:landingzone/AAAAAAAAAAAAAAAA".to_string(),
        ),
        (
            "a landing zone ARN of another Region",
            "arn:aws:controltower:eu-west-1:000000000000:landingzone/BBBBBBBBBBBBBBBB".to_string(),
        ),
        (
            "a landing zone ARN of another partition and account",
            "arn:aws-us-gov:controltower:us-gov-west-1:210987654321:landingzone/CCCCCCCCCCCCCCCC"
                .to_string(),
        ),
        (
            "a landing zone ARN whose id carries a dash and an underscore",
            "arn:aws:controltower:ap-south-1:000000000000:landingzone/zone_1-of-2".to_string(),
        ),
    ]
}

/// The Regions a row's request is signed for: the credential scope's Region is what the
/// message names.
const SIGNING_REGIONS: &[&str] = &["us-west-2", "eu-central-1", "ap-southeast-2"];

/// AWS Control Tower's wording for a `landingZoneIdentifier` that resolves to nothing: fixed
/// text around two substitutions, the management account and the Region of the request.
fn expected_message(region: &str) -> String {
    format!(
        "AWS Control Tower could not find a landing zone identifier value for the management \
         account: {}, in Region {region}. To continue, provide a valid resource ARN or \
         landingZoneIdentifier string for your landing zone.",
        engine::emulator_account_id()
    )
}

/// The request body one resolver is driven with for `identifier`.
fn body(resolver: Resolver, identifier: &str) -> serde_json::Value {
    let mut body = serde_json::json!({ "landingZoneIdentifier": identifier });
    for (member, value) in resolver.extra {
        body[*member] = serde_json::json!(value);
    }
    body
}

/// Assert `answer` is the modelled miss: the 404 every one of these operations declares, its
/// error code, AWS's wording for the Region the request was signed for, and no mention of
/// the identifier that was sent.
fn assert_missing(case: &str, identifier: &str, region: &str, answer: &Answer) {
    assert_eq!(
        (answer.status, answer.error_code.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case {case:?}: expected 404 ResourceNotFoundException, got {:?} body {:?}",
        (answer.status, answer.error_code.as_deref()),
        answer.body
    );
    let expected = expected_message(region);
    assert_eq!(
        answer.string("message"),
        expected,
        "case {case:?}: expected the message {expected:?}, got {:?}",
        answer.string("message")
    );
    assert!(
        !answer.string("message").contains(identifier),
        "case {case:?}: the message must name the account and Region rather than the \
         identifier sent, got {:?}",
        answer.string("message")
    );
}

/// RULE: a landing zone ARN naming none the account holds is the modelled 404 reporting the
/// management account and the Region of the request — from every operation that resolves the
/// member, whether or not the account holds a landing zone of its own.
#[tokio::test]
async fn an_unheld_landing_zone_identifier_names_the_account_and_region() {
    for (account_state, set_up) in [
        ("an account holding no landing zone", false),
        ("an account holding another landing zone", true),
    ] {
        let router = if set_up {
            set_up_emulator().await
        } else {
            emulator()
        };

        for resolver in RESOLVERS {
            for (identifier_case, identifier) in unheld_identifiers() {
                for region in SIGNING_REGIONS {
                    let answer = call_in_region(
                        &router,
                        region,
                        resolver.path,
                        body(*resolver, &identifier),
                    )
                    .await;
                    assert_missing(
                        &format!(
                            "{account_state}: {} with {identifier_case} signed for {region}",
                            resolver.path
                        ),
                        &identifier,
                        region,
                        &answer,
                    );
                }
            }
        }
    }
}

/// RULE (its variable parts): the message is built from the REQUEST, so one identifier
/// answers one Region's wording when signed for that Region and another's when signed for
/// another — and the account it names is the one this emulator presents, never a segment of
/// the identifier.
#[tokio::test]
async fn the_region_named_is_the_requests_own() {
    let router = emulator();
    // An identifier whose own Region and account segments are NOT the ones the message must
    // name, so a message echoing the identifier cannot pass.
    let identifier = "arn:aws-us-gov:controltower:us-gov-east-1:210987654321:landingzone/ELSEWHERE";

    let mut answered: Vec<(&str, String)> = Vec::new();
    for region in SIGNING_REGIONS {
        let answer = call_in_region(
            &router,
            region,
            "/get-landingzone",
            serde_json::json!({ "landingZoneIdentifier": identifier }),
        )
        .await;
        assert_missing(
            &format!("one identifier signed for {region}"),
            identifier,
            region,
            &answer,
        );
        answered.push((region, answer.string("message")));
    }

    for (region, message) in &answered {
        assert!(
            message.contains(&format!("in Region {region}")),
            "signed for {region}: expected the message to name that Region, got {message:?}"
        );
        assert!(
            message.contains(engine::emulator_account_id()),
            "signed for {region}: expected the message to name the management account \
             {:?}, got {message:?}",
            engine::emulator_account_id()
        );
        assert_eq!(
            answered
                .iter()
                .filter(|(_, other)| other == message)
                .count(),
            1,
            "signed for {region}: each Region must answer its own wording, got {answered:?}"
        );
    }
}

/// RULE (the reporting side of it): a miss resolves nothing and changes nothing, so the
/// landing zone the account does hold still reports itself after a run of them.
#[tokio::test]
async fn a_miss_leaves_the_landing_zone_the_account_holds() {
    let router = set_up_emulator().await;
    let held = call_in_region(
        &router,
        "us-west-2",
        "/list-landingzones",
        serde_json::json!({}),
    )
    .await;
    let arn = held
        .get("landingZones")
        .as_array()
        .and_then(|zones| zones.first())
        .and_then(|zone| zone.get("arn"))
        .and_then(|arn| arn.as_str())
        .expect("the set-up account holds one landing zone")
        .to_string();

    for (identifier_case, identifier) in unheld_identifiers() {
        let answer = call_in_region(
            &router,
            "us-west-2",
            "/get-landingzone",
            serde_json::json!({ "landingZoneIdentifier": identifier }),
        )
        .await;
        assert_missing(
            &format!("before the held read: {identifier_case}"),
            &identifier,
            "us-west-2",
            &answer,
        );
    }

    let read = call_in_region(
        &router,
        "us-west-2",
        "/get-landingzone",
        serde_json::json!({ "landingZoneIdentifier": arn }),
    )
    .await;
    assert_eq!(
        (read.status, read.get("landingZone").get("arn")),
        (200, Some(&serde_json::json!(arn))),
        "after the misses: expected the held landing zone to still resolve, got {} body {:?}",
        read.status,
        read.body
    );
}
