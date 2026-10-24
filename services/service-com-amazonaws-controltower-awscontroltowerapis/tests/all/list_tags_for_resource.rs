// SPDX-License-Identifier: CC0-1.0

//! `ListTagsForResource` on its own, end to end over HTTP.
//!
//! The merge behaviour of `TagResource` and what a read reports after it lives in
//! [`crate::resource_tags`]; this suite pins the three things that are the READ's own,
//! each over a TABLE of inputs rather than one case:
//!
//! * which `resourceArn` values the HANDLER answers for and which the front door refuses
//!   first: every value inside `Arn`'s published `@length` bounds and matching its
//!   `@pattern` reaches the handler, which answers 200 for an ARN this service minted and
//!   the modelled `ResourceNotFoundException` (404) for one it did not — including a value
//!   sitting exactly ON each bound — while a value outside those bounds never reaches it;
//! * a resource is named by its WHOLE ARN: a value that differs from a minted ARN in any
//!   single character, or carries anything before or after it, names no resource and is
//!   404 — it does not inherit the real resource's tags;
//! * reporting does not depend on the resource's status: the tags reported before a
//!   resource's clock-owed transition are the tags reported after it, and reading never
//!   makes that transition due earlier or later.
//!
//! Every bound written out by hand names its source: `resourceArn` targets `Arn`,
//! `@length(min 20, max 2048)` with `@pattern ^arn:aws[0-9a-zA-Z_\-:\/]+$`, bound as
//! `@http GET /tags/{resourceArn}` with a 200 success status and the errors
//! `InternalServerException` (500), `ResourceNotFoundException` (404) and
//! `ValidationException` (400) — from
//! `smithy_query(query="operation", operation="ListTagsForResource")` on this crate's
//! `smithy-model/`.

use crate::front_door_validation::{arn_of_length, FRONT_DOOR_CODE};
use crate::landing_zone_lifecycle::{emulator, past_the_settle_span};
use crate::resource_tags::{apply_tags, read_tags, reported_tags, resources, tags};

/// `arn` with the last character of its resource id replaced by a DIFFERENT one, so the
/// value is still an `Arn` of the same length and shape but names nothing: the smallest
/// possible miss.
///
/// The substituted character is always `'0'` or `'1'`, both of which `Arn`'s
/// `@pattern ^arn:aws[0-9a-zA-Z_\-:\/]+$` allows in the part after the literal prefix, so
/// the mutant stays a value the front door passes and the handler is what refuses it.
fn one_character_off(arn: &str) -> String {
    let mut characters: Vec<char> = arn.chars().collect();
    let last = characters
        .last()
        .copied()
        .expect("a minted ARN is not empty");
    *characters
        .last_mut()
        .expect("a minted ARN is not empty") = if last == '0' { '1' } else { '0' };
    characters.into_iter().collect()
}

/// RULE: inside `Arn`'s published bounds the HANDLER decides — 200 for an ARN this
/// service minted, 404 `ResourceNotFoundException` for one it did not — including on each
/// bound itself.
#[tokio::test]
async fn an_arn_inside_the_modeled_bounds_is_answered_by_the_handler() {
    let router = emulator();
    let created = resources(&router).await;

    // Every row satisfies @length(20..2048) and @pattern ^arn:aws[0-9a-zA-Z_\-:\/]+$, so
    // the front door passes it and what comes back is this operation's own answer.
    let mut cases: Vec<(String, String, u16, Option<&str>)> = Vec::new();
    for resource in &created {
        cases.push((
            format!("the minted ARN of {}", resource.case),
            resource.arn.clone(),
            200,
            None,
        ));
    }
    cases.extend([
        (
            "an ARN exactly at the length min of 20".to_string(),
            arn_of_length(20),
            404,
            Some("ResourceNotFoundException"),
        ),
        (
            "an ARN exactly at the length max of 2048".to_string(),
            arn_of_length(2048),
            404,
            Some("ResourceNotFoundException"),
        ),
        (
            "an ARN midway through the length range".to_string(),
            arn_of_length(512),
            404,
            Some("ResourceNotFoundException"),
        ),
        (
            "a well-formed enabled-control ARN nothing minted".to_string(),
            "arn:aws:controltower:us-west-2:000000000000:enabledcontrol/absent".to_string(),
            404,
            Some("ResourceNotFoundException"),
        ),
        (
            "an ARN of another AWS service".to_string(),
            "arn:aws:organizations::000000000000:ou/o-emulatororg/ou-abcd-9z".to_string(),
            404,
            Some("ResourceNotFoundException"),
        ),
    ]);

    for (case, arn, status, error_code) in cases {
        let read = read_tags(&router, &arn).await;
        assert_eq!(
            (read.status, read.error_code.as_deref()),
            (status, error_code),
            "case {case:?}: expected {:?}, got {:?} body {:?}",
            (status, error_code),
            (read.status, read.error_code.as_deref()),
            read.body
        );
    }
}

/// RULE (the other side of it): a `resourceArn` OUTSIDE `Arn`'s bounds never reaches the
/// handler — it is the front door's 400, not a 404 about a resource.
#[tokio::test]
async fn an_arn_outside_the_modeled_bounds_never_reaches_the_handler() {
    let router = emulator();
    let _ = resources(&router).await;

    let cases: Vec<(&str, String)> = vec![
        (
            "one character below the length min of 20",
            arn_of_length(19),
        ),
        (
            "one character above the length max of 2048",
            arn_of_length(2049),
        ),
        (
            "long enough, but holding a character @pattern does not allow",
            "arn:aws.not.a.legal.arn.value".to_string(),
        ),
        (
            "long enough, but not starting with the pattern's literal prefix",
            "brn:awsnotanarnatallxxxxx".to_string(),
        ),
    ];

    for (case, arn) in cases {
        let refused = read_tags(&router, &arn).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (400, Some(FRONT_DOOR_CODE)),
            "case {case:?}: expected 400 {FRONT_DOOR_CODE}, got {:?} body {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );
    }
}

/// RULE: a resource is named by its WHOLE ARN. A value that differs from a minted ARN in
/// one character, or carries anything before or after it, names no resource — so it is
/// 404 and inherits none of the real resource's tags.
#[tokio::test]
async fn only_the_exact_minted_arn_names_the_resource() {
    let router = emulator();
    let created = resources(&router).await;

    let applied = tags(&[("Team", "platform"), ("Env", "prod")]);
    for resource in &created {
        let answer = apply_tags(&router, &resource.arn, &applied).await;
        assert_eq!(
            answer.status, 204,
            "case {:?}: expected the tagging to be accepted with 204, got {} body {:?}",
            resource.case, answer.status, answer.body
        );
    }

    for resource in &created {
        // Each mutant stays inside @length(20..2048) and matches @pattern, so the handler
        // is what answers for it.
        let mutants: Vec<(&str, String)> = vec![
            ("one character of the resource id differs", one_character_off(&resource.arn)),
            ("a character appended", format!("{}x", resource.arn)),
            ("a path segment appended", format!("{}/child", resource.arn)),
            (
                "the final character dropped",
                resource.arn[..resource.arn.len() - 1].to_string(),
            ),
            (
                "a colon-separated suffix appended",
                format!("{}:extra", resource.arn),
            ),
        ];

        for (mutant_case, arn) in mutants {
            assert_ne!(
                arn, resource.arn,
                "case {:?}/{mutant_case:?}: the mutant must differ from the minted ARN",
                resource.case
            );
            let read = read_tags(&router, &arn).await;
            assert_eq!(
                (read.status, read.error_code.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {:?}/{mutant_case:?}: expected 404 ResourceNotFoundException — a \
                 near-miss ARN names no resource and inherits no tags — got {:?} body {:?}",
                resource.case,
                (read.status, read.error_code.as_deref()),
                read.body
            );
        }

        // And the resource itself still reports exactly what was applied to it.
        let read = read_tags(&router, &resource.arn).await;
        let mut expected = resource.created_with.clone();
        expected.extend(applied.clone());
        assert_eq!(
            (read.status, reported_tags(&read)),
            (200, expected.clone()),
            "case {:?}: expected 200 and the tag set {expected:?}, got {} body {:?}",
            resource.case,
            read.status,
            read.body
        );
    }
}

/// RULE: reporting does not depend on the resource's status. A resource this service
/// returns in a non-terminal status reports the same tags once the clock has made its
/// transition due, and the reads in between change nothing a later caller sees.
#[tokio::test]
async fn the_reported_tags_do_not_change_as_the_clock_settles_the_resource() {
    let router = emulator();
    let created = resources(&router).await;

    let applied = tags(&[("Owner", "platform"), ("Stage", "settling")]);
    let mut expected = Vec::new();
    for resource in &created {
        let answer = apply_tags(&router, &resource.arn, &applied).await;
        assert_eq!(
            answer.status, 204,
            "case {:?}: expected the tagging to be accepted with 204, got {} body {:?}",
            resource.case, answer.status, answer.body
        );
        let mut want = resource.created_with.clone();
        want.extend(applied.clone());
        expected.push(want);
    }

    // While every resource is still in the status its creating call left it in.
    for (index, resource) in created.iter().enumerate() {
        let read = read_tags(&router, &resource.arn).await;
        assert_eq!(
            (read.status, reported_tags(&read)),
            (200, expected[index].clone()),
            "case {:?}, before the transition is due: expected 200 and {:?}, got {} body \
             {:?}",
            resource.case,
            expected[index],
            read.status,
            read.body
        );
    }

    // And once the clock owes each resource its terminal status.
    tokio::time::sleep(past_the_settle_span()).await;
    for (index, resource) in created.iter().enumerate() {
        let read = read_tags(&router, &resource.arn).await;
        assert_eq!(
            (read.status, reported_tags(&read)),
            (200, expected[index].clone()),
            "case {:?}, after the transition became due: expected 200 and the same tag set \
             {:?}, got {} body {:?}",
            resource.case,
            expected[index],
            read.status,
            read.body
        );
    }
}
