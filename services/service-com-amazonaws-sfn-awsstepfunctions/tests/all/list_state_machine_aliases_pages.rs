// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how `ListStateMachineAliases` PAGES one state machine's
//! aliases, driven over this service's own wire protocol (`awsJson1_0`, `POST /`
//! with `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`, so what is asserted is what a signed AWS SDK request
//! observes.
//!
//! What the listing reports, and the order it reports it in, is
//! `tests/all/execution_alias_routing.rs` and
//! `tests/all/alias_order_and_redrive_context.rs`. The rules this suite pins are the
//! ones a client meets when the listing does not fit in one answer, and the ones that
//! make it a REPORT:
//!
//! 1. `maxResults` is an upper limit on a page and `0` means the documented default,
//!    so the pages of a listing concatenate to the whole listing, in one order, with
//!    a `nextToken` issued exactly while items remain and none on the last page;
//! 2. a `nextToken` the service issued names a position in that one order, so
//!    resuming with it yields exactly the suffix after the page that issued it,
//!    whatever page width the next call asks for — and a token this service never
//!    issued is the modelled `InvalidToken` (HTTP 400), while a token outside the
//!    member's modelled length never reaches the operation at all;
//! 3. listing and paging change nothing a later caller sees: every alias still
//!    describes as it did, and the listing of a state machine whose deletion has
//!    begun is that state machine's own modelled refusal — `StateMachineDeleting`
//!    while the record stands, `StateMachineDoesNotExist` once the clock has removed
//!    it, because an alias dies with the state machine it belongs to.
//!
//! The harness (`Emulator`, `string`, `PAST_THE_DELETE_DELAY`) is the one
//! `state_machine_lifecycle` builds, the published version comes from
//! `state_machine_versions`' `machine_with_versions` and the `(status, code, message)`
//! reader from `error_messages`' `failure`; `tests/all.rs` is this binary's crate
//! root, so each is reached through `crate::`.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::{
    invalid_token_message, resource_error_message, DEFAULT_PAGE_SIZE,
    MAX_ALIASES_PER_STATE_MACHINE, STATE_MACHINE_DELETING, STATE_MACHINE_DOES_NOT_EXIST,
};

use crate::error_messages::failure;
use crate::state_machine_lifecycle::{string, Emulator, PAST_THE_DELETE_DELAY};
use crate::state_machine_versions::machine_with_versions;

/// One row of the bad-token table: the case, the token sent, and the HTTP status,
/// error code and message the refusal must carry (`None` where the message is the
/// validation envelope `tests/all/error_messages.rs` pins).
type TokenCase<'a> = (&'a str, String, u16, &'a str, Option<String>);

/// The most calls any walk below may take: a whole state machine's quota of aliases
/// read one at a time, and one more. A service that issued a token which never
/// advanced would otherwise be walked for ever rather than reported.
const MAX_PAGES: usize = MAX_ALIASES_PER_STATE_MACHINE + 1;

/// A state machine with one published version, and that version's ARN.
async fn machine_with_a_version(emulator: &Emulator, name: &str) -> (String, String) {
    let machine = machine_with_versions(emulator, name, 1).await;
    let version = format!("{machine}:1");
    (machine, version)
}

/// Create `count` aliases routing all of the traffic to `version_arn`, and return
/// their ARNs in the order they were created.
async fn aliases(emulator: &Emulator, version_arn: &str, count: usize) -> Vec<String> {
    let mut created = Vec::with_capacity(count);
    for index in 0..count {
        let created_alias = emulator
            .ok(
                "CreateStateMachineAlias",
                &json!({
                    "name": format!("page-{index:03}"),
                    "routingConfiguration": [
                        { "stateMachineVersionArn": version_arn, "weight": 100 },
                    ],
                }),
            )
            .await;
        created.push(string(&created_alias, "stateMachineAliasArn"));
    }
    created
}

/// ONE `ListStateMachineAliases` call: the alias ARNs of the page it answered, in the
/// order the page carries them, and the token it issued.
async fn page(
    emulator: &Emulator,
    state_machine_arn: &str,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (Vec<String>, Option<String>) {
    let mut request = json!({ "stateMachineArn": state_machine_arn });
    if let Some(max_results) = max_results {
        request["maxResults"] = json!(max_results);
    }
    if let Some(next_token) = next_token {
        request["nextToken"] = json!(next_token);
    }
    let listed = emulator.ok("ListStateMachineAliases", &request).await;
    let items = listed
        .get("stateMachineAliases")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected an alias list in {listed:?}"))
        .iter()
        .map(|item| string(item, "stateMachineAliasArn"))
        .collect();
    let next = listed
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (items, next)
}

/// Walk the whole listing of `state_machine_arn` at `max_results` per call, following
/// the tokens it issues, and return the alias ARNs in the order the pages carry them
/// beside how many pages it took.
async fn walk(
    emulator: &Emulator,
    state_machine_arn: &str,
    max_results: Option<i64>,
) -> (Vec<String>, usize) {
    let mut reported: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for pages in 1..=MAX_PAGES {
        let (items, next) = page(emulator, state_machine_arn, max_results, token.as_deref()).await;
        reported.extend(items);
        match next {
            Some(next) => token = Some(next),
            None => return (reported, pages),
        }
    }
    panic!("a limit of {max_results:?} took more than {MAX_PAGES} calls, reporting {reported:?}")
}

/// `DescribeStateMachineAlias` on `arn`, which must succeed.
async fn describe_alias(emulator: &Emulator, arn: &str) -> Value {
    emulator
        .ok(
            "DescribeStateMachineAlias",
            &json!({ "stateMachineAliasArn": arn }),
        )
        .await
}

// ---------------------------------------------------------------------------
// Rule 1: `maxResults` is an upper limit, and the pages are one listing
// ---------------------------------------------------------------------------

/// The pages of a listing concatenate to the whole listing and each holds at most the
/// limit asked for, over a table whose rows differ along the NUMBER of aliases and
/// along the limit: none asked for, a limit of one, a limit that divides the total, a
/// limit that does not, a limit past the total, and the limit at each published bound
/// of its modelled range (`0..1000`, where `0` means the default).
///
/// `MAX_ALIASES_PER_STATE_MACHINE` aliases is the most one state machine can hold, and
/// it is `DEFAULT_PAGE_SIZE`, so the two quota rows are what shows the default page
/// carries a whole state machine's aliases rather than truncating them.
#[tokio::test]
async fn max_results_is_an_upper_limit_and_the_pages_concatenate_to_the_listing() {
    // (case, aliases created, the `maxResults` the calls ask for).
    let cases: Vec<(&str, usize, Option<i64>)> = vec![
        ("no alias at all", 0, None),
        ("one alias and no limit asked for", 1, None),
        ("fewer aliases than one page", 3, None),
        ("a limit of one", 5, Some(1)),
        ("a limit that divides the total", 4, Some(2)),
        ("a limit that does not divide the total", 5, Some(2)),
        ("a limit past the total", 3, Some(10)),
        (
            "the limit at its published minimum, which means the default",
            3,
            Some(0),
        ),
        ("the limit at its published maximum", 3, Some(1000)),
        (
            "a whole state machine's quota of aliases on default pages",
            MAX_ALIASES_PER_STATE_MACHINE,
            None,
        ),
        (
            "a whole state machine's quota cut into pages of seven",
            MAX_ALIASES_PER_STATE_MACHINE,
            Some(7),
        ),
    ];

    for (case, count, max_results) in &cases {
        let emulator = Emulator::new();
        let (machine, version) = machine_with_a_version(&emulator, "alias-paging").await;
        let created = aliases(&emulator, &version, *count).await;

        // A limit of `0`, like an absent one, means the documented default.
        let limit = match max_results {
            Some(asked) if *asked > 0 => usize::try_from(*asked).expect("a positive limit"),
            _ => DEFAULT_PAGE_SIZE,
        };

        let mut reported: Vec<String> = Vec::new();
        let mut pages = 0usize;
        let mut token: Option<String> = None;
        loop {
            assert!(
                pages < MAX_PAGES,
                "case {case:?}: a listing of {} took more than {MAX_PAGES} calls, reporting \
                 {reported:?}",
                created.len()
            );
            let (items, next) = page(&emulator, &machine, *max_results, token.as_deref()).await;
            pages += 1;
            // Every page is as full as the limit and the remainder allow, so the
            // caller makes exactly as many calls as there are pages.
            let remaining = created.len() - reported.len();
            assert_eq!(
                items.len(),
                remaining.min(limit),
                "case {case:?}: page {pages} of a listing of {} at a limit of {limit} must hold \
                 {} aliases, got {items:?}",
                created.len(),
                remaining.min(limit)
            );
            reported.extend(items);
            match next {
                Some(next) => {
                    assert!(
                        reported.len() < created.len(),
                        "case {case:?}: a token is issued only while aliases remain, got \
                         {next:?} after reporting all {} of them",
                        created.len()
                    );
                    token = Some(next);
                }
                None => {
                    assert_eq!(
                        reported.len(),
                        created.len(),
                        "case {case:?}: the last page issues no token, so the listing must be \
                         complete, got {reported:?} of {created:?}"
                    );
                    break;
                }
            }
        }

        assert_eq!(
            reported, created,
            "case {case:?}: the pages concatenate to the one listing, got {reported:?}"
        );
        let expected_pages = created.len().div_ceil(limit).max(1);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: a listing of {} at a limit of {limit} takes {expected_pages} \
             call(s), took {pages}",
            created.len()
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: a token names a position in that one order
// ---------------------------------------------------------------------------

/// A token the service issued resumes the listing exactly after the page that issued
/// it, over every cut of a listing and every width the rest is read at — so a token is
/// a position in the one order rather than a page of a fixed size.
#[tokio::test]
async fn a_token_resumes_the_listing_after_the_page_that_issued_it() {
    let emulator = Emulator::new();
    let (machine, version) = machine_with_a_version(&emulator, "alias-tokens").await;
    let created = aliases(&emulator, &version, 6).await;

    for cut in 1..created.len() {
        let width = i64::try_from(cut).expect("a page width");
        let (first, token) = page(&emulator, &machine, Some(width), None).await;
        assert_eq!(
            first,
            created[..cut],
            "cut {cut}: the first page is the first {cut} of {created:?}, got {first:?}"
        );
        let remaining = created.len() - cut;
        let token = token.unwrap_or_else(|| {
            panic!("cut {cut}: {remaining} aliases remain, so a token is issued")
        });

        for rest_width in [1i64, 2, 3, 1000] {
            let mut rest: Vec<String> = Vec::new();
            let mut cursor = Some(token.clone());
            let mut calls = 0usize;
            while let Some(current) = cursor {
                assert!(
                    calls < MAX_PAGES,
                    "cut {cut} read on at {rest_width}: more than {MAX_PAGES} calls, reporting \
                     {rest:?}"
                );
                calls += 1;
                let (items, next) =
                    page(&emulator, &machine, Some(rest_width), Some(&current)).await;
                rest.extend(items);
                cursor = next;
            }
            assert_eq!(
                rest,
                created[cut..],
                "cut {cut} read on at {rest_width}: expected {:?}, got {rest:?}",
                &created[cut..]
            );
        }
    }
}

/// A `nextToken` this service never issued is the modelled `InvalidToken` (HTTP 400)
/// stating the service's own condition, and a token outside the member's modelled
/// length (`1..1024`) is refused by the boundary before the operation sees it — over a
/// table spanning the forms a client can send, including the length bound and one
/// character past it.
#[tokio::test]
async fn a_token_this_service_never_issued_is_refused() {
    let emulator = Emulator::new();
    let (machine, version) = machine_with_a_version(&emulator, "alias-bad-tokens").await;
    let created = aliases(&emulator, &version, 2).await;

    let cases: Vec<TokenCase<'_>> = vec![
        (
            "text that names no position",
            "page-two".to_string(),
            400,
            "InvalidToken",
            Some(invalid_token_message()),
        ),
        (
            "a negative position",
            "-1".to_string(),
            400,
            "InvalidToken",
            Some(invalid_token_message()),
        ),
        (
            "a fractional position",
            "1.5".to_string(),
            400,
            "InvalidToken",
            Some(invalid_token_message()),
        ),
        (
            "a position with surrounding space",
            " 1 ".to_string(),
            400,
            "InvalidToken",
            Some(invalid_token_message()),
        ),
        (
            "another list operation's token form",
            "eyJvZmZzZXQiOjF9".to_string(),
            400,
            "InvalidToken",
            Some(invalid_token_message()),
        ),
        (
            "digits at the modelled length maximum, which name no position",
            "9".repeat(1024),
            400,
            "InvalidToken",
            Some(invalid_token_message()),
        ),
        (
            "a token one character past the modelled length maximum",
            "9".repeat(1025),
            400,
            "ValidationException",
            None,
        ),
        (
            "an empty token, under the modelled length minimum",
            String::new(),
            400,
            "ValidationException",
            None,
        ),
    ];

    for (case, token, status, code, message) in &cases {
        let (actual_status, actual_code, actual_message) = failure(
            &emulator,
            case,
            "ListStateMachineAliases",
            &json!({ "stateMachineArn": &machine, "nextToken": token }),
        )
        .await;
        assert_eq!(
            (actual_status, actual_code.as_str()),
            (*status, *code),
            "case {case:?}: expected {status} {code}, got {actual_status} {actual_code} with \
             message {actual_message:?}"
        );
        if let Some(expected) = message {
            assert_eq!(
                &actual_message, expected,
                "case {case:?}: expected the message {expected:?}, got {actual_message:?}"
            );
        }
        // A refused token leaves the listing itself standing.
        let (listed, next) = page(&emulator, &machine, None, None).await;
        assert_eq!(
            (&listed, &next),
            (&created, &None),
            "case {case:?}: the refusal changes no record, got {listed:?} with {next:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the listing is a report, and an alias dies with its state machine
// ---------------------------------------------------------------------------

/// Paging the aliases changes nothing a later caller sees — each alias describes
/// exactly as it did and the listing repeats — and once the state machine's deletion
/// has begun the listing is that state machine's own modelled refusal:
/// `StateMachineDeleting` while the record stands, and `StateMachineDoesNotExist`
/// once the clock has removed it with its aliases.
#[tokio::test]
async fn paging_changes_nothing_and_the_aliases_die_with_the_state_machine() {
    let emulator = Emulator::new();
    let (machine, version) = machine_with_a_version(&emulator, "alias-report").await;
    let created = aliases(&emulator, &version, 4).await;

    let mut before: Vec<Value> = Vec::with_capacity(created.len());
    for arn in &created {
        before.push(describe_alias(&emulator, arn).await);
    }

    // Every width, twice over, so no walk consumes what the next one reads.
    for round in 1..=2 {
        for width in [None, Some(1), Some(2), Some(3), Some(1000)] {
            let (reported, _) = walk(&emulator, &machine, width).await;
            assert_eq!(
                reported, created,
                "round {round} at a limit of {width:?}: expected {created:?}, got {reported:?}"
            );
        }
    }

    let mut after: Vec<Value> = Vec::with_capacity(created.len());
    for arn in &created {
        after.push(describe_alias(&emulator, arn).await);
    }
    assert_eq!(
        after, before,
        "paging reports the alias records, it does not change them"
    );

    // The deletion the clock owes: the record is still there, so the listing is the
    // modelled `StateMachineDeleting`.
    emulator
        .ok("DeleteStateMachine", &json!({ "stateMachineArn": &machine }))
        .await;
    let (status, code, message) = failure(
        &emulator,
        "a state machine whose deletion has begun",
        "ListStateMachineAliases",
        &json!({ "stateMachineArn": &machine }),
    )
    .await;
    assert_eq!(
        (status, code.as_str(), message.as_str()),
        (
            400,
            "StateMachineDeleting",
            resource_error_message(STATE_MACHINE_DELETING, &machine).as_str()
        ),
        "a deleting state machine refuses the listing, got {status} {code} {message:?}"
    );

    // Once it has become due, the state machine and its aliases are gone.
    std::thread::sleep(PAST_THE_DELETE_DELAY);
    let (status, code, message) = failure(
        &emulator,
        "a state machine the clock has removed",
        "ListStateMachineAliases",
        &json!({ "stateMachineArn": &machine }),
    )
    .await;
    assert_eq!(
        (status, code.as_str(), message.as_str()),
        (
            400,
            "StateMachineDoesNotExist",
            resource_error_message(STATE_MACHINE_DOES_NOT_EXIST, &machine).as_str()
        ),
        "a removed state machine has no aliases to list, got {status} {code} {message:?}"
    );
}
