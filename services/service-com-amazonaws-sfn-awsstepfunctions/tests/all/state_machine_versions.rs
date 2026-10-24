// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListStateMachineVersions`, driven over this service's own
//! wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`, so what is asserted is what a signed AWS SDK request
//! observes.
//!
//! The rules under test:
//!
//! 1. the listing reports EXACTLY the version records of the state machine its
//!    unqualified ARN names — one item per version, carrying that version's own
//!    qualified ARN (`<stateMachineArn>:<version>`) and the creation time the
//!    publish gave it — and nothing of any other state machine, whatever the number
//!    of versions and whichever call (`CreateStateMachine` or `UpdateStateMachine`)
//!    published them;
//! 2. "The results are sorted in descending order of the version creation time",
//!    and `maxResults`/`nextToken` cut that one order into pages whose
//!    concatenation is the order itself, with a `nextToken` present exactly while
//!    more results remain;
//! 3. the ARN is judged BEFORE the lookup: text that is not a well-formed state
//!    machine ARN of this service in the endpoint's region is `InvalidArn`, while a
//!    QUALIFIED ARN — a Distributed Map label, a version or an alias — is
//!    `ValidationException`, the labeled one carrying the modelled
//!    `API_DOES_NOT_SUPPORT_LABELED_ARNS` reason;
//! 4. the operation REPORTS: listing changes nothing a later caller sees, and the
//!    only state it reflects is what the clock already owed (a deleted state machine
//!    keeps no versions).
//!
//! The harness (`Emulator`, `create_machine`, `string`, `error_code`) is the one
//! `state_machine_lifecycle` already builds; `tests/all.rs` is this binary's crate
//! root, so it is reached through `crate::state_machine_lifecycle`.

use serde_json::{json, Value};

use crate::state_machine_lifecycle::{
    error_code, string, Emulator, ACCOUNT, HELLO_DEFINITION, PAST_THE_DELETE_DELAY, REGION,
    ROLE_ARN,
};

/// A second definition, so an update that publishes changes something.
const GOODBYE_DEFINITION: &str =
    r#"{"StartAt":"Bye","States":{"Bye":{"Type":"Pass","Result":{"msg":"bye"},"End":true}}}"#;

/// Create a state machine, publishing version 1, and return its unqualified ARN.
async fn create_published(emulator: &Emulator, name: &str) -> String {
    let created = emulator
        .ok(
            "CreateStateMachine",
            &json!({
                "name": name,
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
                "publish": true,
            }),
        )
        .await;
    string(&created, "stateMachineArn")
}

/// Update a state machine, publishing the next version, and return that version's
/// ARN.
async fn update_published(emulator: &Emulator, arn: &str, definition: &str) -> String {
    let updated = emulator
        .ok(
            "UpdateStateMachine",
            &json!({ "stateMachineArn": arn, "definition": definition, "publish": true }),
        )
        .await;
    string(&updated, "stateMachineVersionArn")
}

/// A state machine with `versions` published versions, and its unqualified ARN.
///
/// Version 1 comes from the create and every later one from an update that
/// publishes in the same action, so the rows below span both of the calls that
/// publish.
///
/// `pub(crate)`, not private: `tests/all/state_machine_aliases.rs` points aliases at
/// the versions this builds rather than carrying a second copy of the publish path.
pub(crate) async fn machine_with_versions(
    emulator: &Emulator,
    name: &str,
    versions: u32,
) -> String {
    if versions == 0 {
        return crate::state_machine_lifecycle::create_machine(emulator, name, HELLO_DEFINITION)
            .await;
    }
    let arn = create_published(emulator, name).await;
    for number in 2..=versions {
        // Alternate the definition so each version is a distinct snapshot.
        let definition = if number % 2 == 0 {
            GOODBYE_DEFINITION
        } else {
            HELLO_DEFINITION
        };
        update_published(emulator, &arn, definition).await;
    }
    arn
}

/// `ListStateMachineVersions` on `arn`, which must succeed, with the request members
/// `extra` carries merged in.
async fn list(emulator: &Emulator, arn: &str, extra: Value) -> Value {
    let Value::Object(extra) = extra else {
        panic!("expected an object of members, got {extra:?}")
    };
    let mut request = json!({ "stateMachineArn": arn });
    for (member, value) in extra {
        request[member] = value;
    }
    emulator.ok("ListStateMachineVersions", &request).await
}

/// The `stateMachineVersionArn` of every item of a listing body, in the order the
/// response carries them.
fn listed_arns(body: &Value) -> Vec<String> {
    body.get("stateMachineVersions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a stateMachineVersions list in {body:?}"))
        .iter()
        .map(|item| string(item, "stateMachineVersionArn"))
        .collect()
}

/// The `creationDate` of every item of a listing body, as the wire carries it.
fn listed_dates(body: &Value) -> Vec<f64> {
    body.get("stateMachineVersions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a stateMachineVersions list in {body:?}"))
        .iter()
        .map(|item| {
            item.get("creationDate")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| panic!("expected a creationDate in {item:?}"))
        })
        .collect()
}

/// Walk every page of the listing of `arn` at `max_results`, returning the version
/// ARNs in page order and asserting that a `nextToken` is present exactly while
/// more results remain.
async fn walk_pages(emulator: &Emulator, case: &str, arn: &str, max_results: i64) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // Bounded so a token that never advances fails the test instead of hanging.
    for page in 0..64 {
        let mut extra = json!({ "maxResults": max_results });
        if let Some(token) = token.as_deref() {
            extra["nextToken"] = json!(token);
        }
        let body = list(emulator, arn, extra).await;
        let arns = listed_arns(&body);
        if max_results > 0 {
            assert!(
                arns.len() <= usize::try_from(max_results).expect("a positive page size"),
                "case {case:?}: page {page} carries {} items, past maxResults {max_results}",
                arns.len()
            );
        }
        seen.extend(arns);
        token = body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            return seen;
        }
    }
    panic!("case {case:?}: the listing of {arn} never ran out of pages, got {seen:?}")
}

// ---------------------------------------------------------------------------
// What the listing reports
// ---------------------------------------------------------------------------

/// The listing reports one item per published version, newest first, each carrying
/// the version's own qualified ARN and the creation time `DescribeStateMachine`
/// reports for that same version — over a table that varies the NUMBER of versions,
/// which is what the rule generalises over.
#[tokio::test]
async fn the_listing_reports_every_published_version_newest_first() {
    let emulator = Emulator::new();

    // Rows: no version at all, one, and several — spanning a single page and,
    // against the default page size of 100, a count that still fits one page.
    for versions in [0_u32, 1, 2, 3, 7] {
        let case = format!("{versions} published version(s)");
        let arn = machine_with_versions(&emulator, &format!("versioned-{versions}"), versions).await;

        let body = list(&emulator, &arn, json!({})).await;
        let listed = listed_arns(&body);
        // Descending by creation time; versions published inside one clock tick
        // share it, so the total order is the version number, descending.
        let expected: Vec<String> = (1..=versions)
            .rev()
            .map(|number| format!("{arn}:{number}"))
            .collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the versions newest first, got {listed:?}"
        );
        assert!(
            body.get("nextToken").is_none_or(Value::is_null),
            "case {case:?}: one page holds every version, so no nextToken is owed, got {body:?}"
        );

        // The creation times are non-increasing, and each is the one the version's
        // own record reports through the implemented reader.
        let dates = listed_dates(&body);
        for window in dates.windows(2) {
            assert!(
                window[0] >= window[1],
                "case {case:?}: creation times must descend, got {dates:?}"
            );
        }
        for (item, version_arn) in dates.iter().zip(listed.iter()) {
            let described = emulator
                .ok(
                    "DescribeStateMachine",
                    &json!({ "stateMachineArn": version_arn }),
                )
                .await;
            let described_date = described
                .get("creationDate")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| panic!("expected a creationDate in {described:?}"));
            assert_eq!(
                *item, described_date,
                "case {case:?}: {version_arn} must list the creation date its record carries \
                 ({described_date}), got {item}"
            );
        }
    }
}

/// A version belongs to ONE state machine: the listing of each machine names only
/// its own versions, including when one machine's name is a prefix of another's
/// (which is what a prefix scan over version ARNs could get wrong).
#[tokio::test]
async fn versions_are_scoped_to_their_own_state_machine() {
    let emulator = Emulator::new();

    // Rows differ in how the names relate: one is a strict prefix of the next, one
    // shares no prefix at all, and one has no versions to report.
    let cases: Vec<(&str, u32)> = vec![("alpha", 2), ("alpha-beta", 3), ("gamma", 1), ("delta", 0)];
    let mut arns: Vec<(&str, u32, String)> = Vec::new();
    for (name, versions) in cases {
        let arn = machine_with_versions(&emulator, name, versions).await;
        arns.push((name, versions, arn));
    }

    for (name, versions, arn) in &arns {
        let listed = listed_arns(&list(&emulator, arn, json!({})).await);
        let expected: Vec<String> = (1..=*versions)
            .rev()
            .map(|number| format!("{arn}:{number}"))
            .collect();
        assert_eq!(
            &listed, &expected,
            "case {name:?}: expected only its own {versions} version(s), got {listed:?}"
        );
    }
}

/// A well-formed ARN that names no state machine is a state machine with no
/// versions: this operation models no resource-absent error at all, so it answers an
/// empty list rather than an error.
#[tokio::test]
async fn an_absent_state_machine_lists_no_versions() {
    let emulator = Emulator::new();
    machine_with_versions(&emulator, "present", 2).await;

    // Rows vary along why nothing is there: never created, created without publish,
    // and removed once the deletion the clock owed had happened.
    let never = format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:never-created");
    let unpublished = machine_with_versions(&emulator, "unpublished", 0).await;
    let deleted = machine_with_versions(&emulator, "deleted-later", 2).await;
    assert_eq!(
        listed_arns(&list(&emulator, &deleted, json!({})).await).len(),
        2,
        "the machine about to be deleted must hold two versions first"
    );
    emulator
        .ok("DeleteStateMachine", &json!({ "stateMachineArn": deleted }))
        .await;
    std::thread::sleep(PAST_THE_DELETE_DELAY);

    for (case, arn) in [
        ("a state machine that was never created", &never),
        ("a state machine created without publish", &unpublished),
        ("a state machine the clock has since removed", &deleted),
    ] {
        let body = list(&emulator, arn, json!({})).await;
        assert_eq!(
            listed_arns(&body),
            Vec::<String>::new(),
            "case {case:?}: expected an empty listing, got {body:?}"
        );
        assert!(
            body.get("nextToken").is_none_or(Value::is_null),
            "case {case:?}: an empty listing owes no nextToken, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// `maxResults` and `nextToken` cut ONE order into pages: walking every page at any
/// page size yields exactly the single-page listing, and each page is no longer than
/// `maxResults`.
#[tokio::test]
async fn paging_reassembles_the_one_descending_order() {
    let emulator = Emulator::new();
    let arn = machine_with_versions(&emulator, "paged", 9).await;
    let whole = listed_arns(&list(&emulator, &arn, json!({ "maxResults": 1000 })).await);
    assert_eq!(whole.len(), 9, "expected nine versions, got {whole:?}");

    // Rows span the page size: the modelled minimum that still means a size (1), a
    // size that does not divide the total, one equal to the total, one past it, the
    // modelled maximum (1000), and 0 — "a value of 0 uses the default" (100).
    for max_results in [1_i64, 2, 4, 9, 10, 1000, 0] {
        let case = format!("maxResults {max_results}");
        let walked = walk_pages(&emulator, &case, &arn, max_results).await;
        assert_eq!(
            walked, whole,
            "case {case:?}: paging must reassemble the one order, got {walked:?}"
        );
    }

    // A token naming an offset past the end is a token this service issued: it names
    // the empty tail, not an error.
    let past_the_end = list(&emulator, &arn, json!({ "nextToken": "9" })).await;
    assert_eq!(
        listed_arns(&past_the_end),
        Vec::<String>::new(),
        "a token at the end of the order lists nothing, got {past_the_end:?}"
    );
}

/// A `nextToken` this service never issued is the modelled `InvalidToken` (HTTP
/// 400), over a table of token forms.
#[tokio::test]
async fn a_token_this_service_never_issued_is_invalid_token() {
    let emulator = Emulator::new();
    let arn = machine_with_versions(&emulator, "tokens", 3).await;

    // Rows vary along the form of the text: not a number at all, a negative offset,
    // a float, a number padded on either side, a non-decimal radix, and a number
    // that overflows the offset.
    for token in [
        "not-a-token",
        "-1",
        "1.5",
        " 1",
        "1 ",
        "0x1",
        "99999999999999999999999999",
    ] {
        let (status, code) = emulator
            .err(
                "ListStateMachineVersions",
                &json!({ "stateMachineArn": &arn, "nextToken": token }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidToken"),
            "case {token:?}: expected 400 InvalidToken, got {status} {code}"
        );
    }
}

// ---------------------------------------------------------------------------
// The ARN is judged before the lookup
// ---------------------------------------------------------------------------

/// Text that is not a well-formed state machine ARN of this service in the
/// endpoint's region is the modelled `InvalidArn`, whose message names the first
/// component that failed inside the service's `Invalid Arn: '...'` envelope.
#[tokio::test]
async fn a_malformed_or_foreign_arn_is_invalid_arn() {
    let emulator = Emulator::new();
    let arn = machine_with_versions(&emulator, "arn-rule", 1).await;
    let prefix = format!("arn:aws:states:{REGION}:{ACCOUNT}");

    // Rows vary along WHICH component of the ARN is wrong, including an ARN of
    // another region — the check is against the ENDPOINT's region, and these are
    // other AWS regions from the published region list.
    let cases: Vec<(&str, String, String)> = vec![
        (
            "text that is not an ARN",
            "not-a-valid-arn".to_string(),
            "Invalid ARN prefix: not-a-valid-arn".to_string(),
        ),
        (
            "a bare state machine name",
            "arn-rule".to_string(),
            "Invalid ARN prefix: arn-rule".to_string(),
        ),
        (
            "an ARN with no resource field",
            prefix.clone(),
            format!("Invalid ARN: expected 6 colon-delimited fields in {prefix}"),
        ),
        (
            "an empty partition",
            format!("arn::states:{REGION}:{ACCOUNT}:stateMachine:arn-rule"),
            format!("Invalid ARN partition: arn::states:{REGION}:{ACCOUNT}:stateMachine:arn-rule"),
        ),
        (
            "another service's ARN",
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:arn-rule"),
            "Invalid ARN service: lambda, expected states".to_string(),
        ),
        (
            "an execution ARN",
            format!("{prefix}:execution:arn-rule:run-1"),
            "Invalid ARN resource type: execution, expected stateMachine".to_string(),
        ),
        (
            "an activity ARN",
            format!("{prefix}:activity:an-activity"),
            "Invalid ARN resource type: activity, expected stateMachine".to_string(),
        ),
        (
            "a state machine ARN with no name",
            format!("{prefix}:stateMachine:"),
            format!("Invalid ARN stateMachine name: {prefix}:stateMachine:"),
        ),
        (
            "an ARN of another region",
            format!("arn:aws:states:eu-west-1:{ACCOUNT}:stateMachine:arn-rule"),
            format!(
                "Expected the ARN arn:aws:states:eu-west-1:{ACCOUNT}:stateMachine:arn-rule to be \
                 within region ({REGION})."
            ),
        ),
        (
            "an ARN of another region in another partition",
            format!("arn:aws-us-gov:states:us-gov-west-1:{ACCOUNT}:stateMachine:arn-rule"),
            format!(
                "Expected the ARN arn:aws-us-gov:states:us-gov-west-1:{ACCOUNT}:stateMachine:\
                 arn-rule to be within region ({REGION})."
            ),
        ),
    ];

    for (case, value, diagnosis) in &cases {
        let (status, body) = emulator
            .call(
                "ListStateMachineVersions",
                &json!({ "stateMachineArn": value }),
            )
            .await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidArn"),
            "case {case:?}: expected 400 InvalidArn, got {status} {code} with body {body:?}"
        );
        assert_eq!(
            body.get("message").and_then(Value::as_str),
            Some(format!("Invalid Arn: '{diagnosis}'").as_str()),
            "case {case:?}: unexpected message in {body:?}"
        );
    }

    // None of those refusals disturbed the state machine that does exist.
    let listed = listed_arns(&list(&emulator, &arn, json!({})).await);
    assert_eq!(
        listed,
        vec![format!("{arn}:1")],
        "the existing listing must be unchanged, got {listed:?}"
    );
}

/// A QUALIFIED state machine ARN names a Distributed Map state, a version or an
/// alias — none of which has versions of its own — so it is the modelled
/// `ValidationException`, carrying `API_DOES_NOT_SUPPORT_LABELED_ARNS` for the
/// labeled form.
#[tokio::test]
async fn a_qualified_arn_is_a_validation_exception() {
    let emulator = Emulator::new();
    let arn = machine_with_versions(&emulator, "qualified", 2).await;
    let absent = format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:never-created");

    // (case, qualified ARN, the modelled reason the refusal must carry). Rows vary
    // along the qualifier's class and form, and along whether the state machine
    // behind it exists at all — the ARN is judged before anything is looked up.
    let mut cases: Vec<(String, String, Option<&str>)> = Vec::new();
    for base in [&arn, &absent] {
        for label in ["mapStateLabel", "Map", "inner-map_1", "0"] {
            cases.push((
                format!("a labeled ARN '{base}/{label}'"),
                format!("{base}/{label}"),
                Some("API_DOES_NOT_SUPPORT_LABELED_ARNS"),
            ));
        }
        for qualifier in ["1", "2", "99", "PROD", "live-alias"] {
            cases.push((
                format!("a qualified ARN '{base}:{qualifier}'"),
                format!("{base}:{qualifier}"),
                None,
            ));
        }
    }

    for (case, value, reason) in &cases {
        let (status, body) = emulator
            .call(
                "ListStateMachineVersions",
                &json!({ "stateMachineArn": value }),
            )
            .await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {code} with body \
             {body:?}"
        );
        assert_eq!(
            body.get("reason").and_then(Value::as_str),
            *reason,
            "case {case:?}: expected reason {reason:?}, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The listing reports; it does not mutate
// ---------------------------------------------------------------------------

/// Listing changes nothing a later caller sees: repeated listings — paged and
/// whole — answer the same thing, and the versions are still readable afterwards.
#[tokio::test]
async fn listing_does_not_change_what_a_later_caller_sees() {
    let emulator = Emulator::new();
    let arn = machine_with_versions(&emulator, "reporting", 4).await;

    let first = list(&emulator, &arn, json!({})).await;
    for round in 0..3 {
        let paged = walk_pages(&emulator, &format!("round {round}"), &arn, 2).await;
        let again = list(&emulator, &arn, json!({})).await;
        assert_eq!(
            again, first,
            "round {round}: a listing must answer the same thing, got {again:?}"
        );
        assert_eq!(
            paged,
            listed_arns(&first),
            "round {round}: paging must answer the same order, got {paged:?}"
        );
    }

    // And publishing one more version is the only thing that changes the listing.
    let published = update_published(&emulator, &arn, GOODBYE_DEFINITION).await;
    let listed = listed_arns(&list(&emulator, &arn, json!({})).await);
    assert_eq!(
        listed.first().map(String::as_str),
        Some(published.as_str()),
        "the newest version must head the listing, got {listed:?}"
    );
    assert_eq!(
        listed.len(),
        5,
        "the publish must add exactly one version, got {listed:?}"
    );
}
