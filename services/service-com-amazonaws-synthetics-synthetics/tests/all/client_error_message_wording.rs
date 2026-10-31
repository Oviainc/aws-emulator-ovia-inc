// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS the two client errors it builds from the request, end to end
//! over the wire (the same `MultiServiceRouter` the emulator boots, through the harness in
//! `canary_lifecycle`).
//!
//! Two rules, each asserted over a SET of inputs rather than over one request:
//!
//! 1. **A canary lookup whose wording is recorded names the identifier the request
//!    supplied.** `AssociateResource`'s refusal is built from the request's own
//!    `ResourceArn`, so a caller associating several canaries can tell which one was
//!    unresolvable — the construction the same call already uses for an unresolvable
//!    `GroupIdentifier`. (The canary operations whose wording is not recorded keep the
//!    service's fixed, identifier-free sentence — `missing_resource_errors`,
//!    `associated_groups`.)
//!
//! 2. **A duplicate group name is reported single-quoted.** A group name "can include any
//!    Unicode characters", spaces among them, so the quotes are what delimit the name in
//!    the sentence and let a client read it back out.
//!
//! The third message this family covers — how a `@pattern` requirement is quoted — is a
//! property of the service's whole validation gate rather than of one operation, so it is
//! asserted where that gate's rules are: `server_side_validation.rs`.

use std::collections::BTreeSet;

use serde_json::json;

use crate::canary_lifecycle::{create_body, Answer, Synthetics};
use crate::group_membership::canary_not_found_message;

/// `GroupName` is modelled `length 1..64`, with no `@pattern`
/// (`smithy_query(query="shape",shape="GroupName")`), so a group name may be any Unicode
/// string inside that bound.
const MAX_GROUP_NAME_LEN: usize = 64;

/// `CanaryName` is modelled `length 1..255, pattern ^[0-9a-z_\-]+$`
/// (`smithy_query(query="shape",shape="CanaryName")`), which bounds the canary segment of
/// a `CanaryArn`.
const MAX_CANARY_NAME_LEN: usize = 255;

/// A canary ARN, written out from `CanaryArn`'s modelled pattern: the `Canary` shape
/// carries no ARN member, so the ARN a caller hands `AssociateResource` is one it composes.
fn canary_arn(region: &str, account: &str, name: &str) -> String {
    format!("arn:aws:synthetics:{region}:{account}:canary:{name}")
}

/// `Message` off a refusal, after asserting its status and error code.
fn refusal_message(case: &str, answer: &Answer, status: u16, code: &str) -> String {
    assert_eq!(
        answer.status, status,
        "case {case}: expected {status}, got {} ({:?})",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code.as_deref(),
        Some(code),
        "case {case}: expected x-amzn-errortype: {code}, got {:?}",
        answer.error_code
    );
    answer
        .at("Message")
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a Message on the refusal, got {:?}",
                answer.body
            )
        })
        .to_string()
}

/// `AssociateResource` answers a `ResourceArn` that resolves to no canary with the
/// missing-canary sentence built from that ARN — whatever the ARN names.
#[tokio::test]
async fn a_missing_canary_on_an_arn_addressed_association_names_the_requested_arn() {
    let region = "us-east-1";
    let account = engine::EMULATOR_ACCOUNT_ID;
    let aws = Synthetics::new(region);

    // A group to associate into, and a canary that DOES exist, so the rows below are
    // answered by a populated store rather than an empty one.
    let group = aws
        .call("POST", "/group", Some(json!({ "Name": "live-group" })))
        .await;
    assert_eq!(
        group.status, 200,
        "case create-group: expected 200, got {} ({:?})",
        group.status, group.body
    );
    let group_id = group.string_at("Group.Id");
    let live = aws.create(create_body("live-canary")).await;
    assert_eq!(
        live.status, 200,
        "case create-canary: expected 200, got {} ({:?})",
        live.status, live.body
    );

    // Every row is a legal `CanaryArn` no store holds, differing along what the rule
    // generalises over: the canary name's charset and length, the Region, and the account.
    let arns = [
        ("another-name", canary_arn(region, account, "no-such-canary")),
        ("single-character-name", canary_arn(region, account, "x")),
        ("underscored-name", canary_arn(region, account, "no_such_1")),
        (
            "max-length-name",
            canary_arn(region, account, &"c".repeat(MAX_CANARY_NAME_LEN)),
        ),
        // The live canary's name in a Region this endpoint is not: "You must run this
        // operation in the Region where the canary exists".
        ("another-region", canary_arn("eu-west-3", account, "live-canary")),
        (
            "another-account",
            canary_arn(region, "999999999999", "live-canary"),
        ),
    ];

    let mut sentences: BTreeSet<String> = BTreeSet::new();
    for (case, arn) in &arns {
        let answer = aws
            .call(
                "PATCH",
                &format!("/group/{group_id}/associate"),
                Some(json!({ "ResourceArn": arn })),
            )
            .await;
        let message = refusal_message(case, &answer, 404, "ResourceNotFoundException");
        assert_eq!(
            message,
            canary_not_found_message(arn),
            "case {case}: expected the missing-canary sentence naming the ARN as supplied, \
             got {message:?}"
        );
        // The ARN is the only thing the message varies by: it is the message's suffix, and
        // what precedes it is one sentence for every row.
        let sentence = message.strip_suffix(arn.as_str()).unwrap_or_else(|| {
            panic!("case {case}: expected the message to end with the requested ARN {arn:?}, got {message:?}")
        });
        sentences.insert(sentence.to_string());
    }
    assert_eq!(
        sentences.len(),
        1,
        "case one-sentence: expected one sentence around the echoed ARN across every row, \
         got {sentences:?}"
    );

    // The association that CAN be made still is: the refusals above changed nothing.
    let accepted = aws
        .call(
            "PATCH",
            &format!("/group/{group_id}/associate"),
            Some(json!({ "ResourceArn": canary_arn(region, account, "live-canary") })),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "case associate-live: expected 200, got {} ({:?})",
        accepted.status, accepted.body
    );
}

/// A second `CreateGroup` for a name already held is refused with the name SINGLE-QUOTED,
/// so the name can be read back out of the sentence whatever characters it contains.
#[tokio::test]
async fn a_duplicate_group_name_is_reported_single_quoted() {
    let aws = Synthetics::new("eu-central-1");

    // The classes the rule generalises over: charset (ASCII, digits, spaces, non-ASCII, an
    // apostrophe of the name's own), and length up to the modelled maximum.
    let names = [
        ("ascii-hyphenated", "prod-canaries".to_string()),
        ("single-character", "g".to_string()),
        ("digits-only", "2026".to_string()),
        ("with-spaces", "front door canaries".to_string()),
        ("non-ascii", "überwachung-gruppe".to_string()),
        ("with-an-apostrophe", "alice's canaries".to_string()),
        ("max-length", "g".repeat(MAX_GROUP_NAME_LEN)),
    ];

    let mut sentences: BTreeSet<(String, String)> = BTreeSet::new();
    for (case, name) in &names {
        let first = aws
            .call("POST", "/group", Some(json!({ "Name": name })))
            .await;
        assert_eq!(
            first.status, 200,
            "case {case}: expected the first create to succeed, got {} ({:?})",
            first.status, first.body
        );

        // The retry differs only in its tags, so what conflicts is the NAME.
        let duplicate = aws
            .call(
                "POST",
                "/group",
                Some(json!({ "Name": name, "Tags": { "retry": "yes" } })),
            )
            .await;
        let message = refusal_message(case, &duplicate, 409, "ConflictException");
        let expected = format!("Group with the name '{name}' already exists");
        assert_eq!(
            message, expected,
            "case {case}: expected Message {expected:?}, got {message:?}"
        );

        // The quotes are what make the name delimitable: a client can take it back out.
        let (before, rest) = message
            .split_once('\'')
            .unwrap_or_else(|| panic!("case {case}: expected a quoted name, got {message:?}"));
        let (quoted, after) = rest
            .rsplit_once('\'')
            .unwrap_or_else(|| panic!("case {case}: expected a closing quote, got {message:?}"));
        assert_eq!(
            quoted, name,
            "case {case}: expected the quoted part to be the requested name, got {quoted:?}"
        );
        sentences.insert((before.to_string(), after.to_string()));
    }
    assert_eq!(
        sentences.len(),
        1,
        "case one-sentence: expected one sentence around the quoted name across every row, \
         got {sentences:?}"
    );
}
