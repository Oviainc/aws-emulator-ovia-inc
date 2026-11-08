// SPDX-License-Identifier: CC0-1.0

//! `ListStatements`: the statements of ONE interactive session, oldest first, in pages.
//!
//! RULE: a paginated read of a resource's children reports every child the store holds of
//! the named parent — nothing of any other parent — in the order the operation's own
//! sequence puts them, projected exactly as the single-child read projects it, one page at
//! a time with a `NextToken` that resumes the walk and no token on the last page; a parent
//! the store does not hold is refused with the operation's modelled not-found error. For
//! this operation the sequence is the one the session itself mints: a session actions one
//! statement at a time, in the order it took them, and the `Id` it gives each counts up in
//! that same order, so the listing is ordered by that `Id` ascending — the order of
//! execution — and not by the order the store happens to hold the records in.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rows differ along what the rule generalises over: how many statements one session
//! holds (none, one, each side of a page boundary, two pages and more, and the full number
//! a session keeps), the order the store holds them in against the order their ids were
//! minted in, the continuation token the walk is resumed from (including tokens this
//! service never issued), and whether the session is one this endpoint holds.
//!
//! The hold order and the due instants the rows turn on are written THROUGH THE STORE
//! rather than slept for, so a row's expected order is written down rather than inferred
//! from how fast the clock ticked between two runs.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::ops::list_statements::PAGE_SIZE;
use service_com_amazonaws_glue_awsglue::state::{
    messages, ComAmazonawsGlueAwsglueState, StatementEntry, FIRST_STATEMENT_ID,
    MAX_STATEMENTS_PER_SESSION,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_runs::{already_past, far_future};
use crate::statements::{get, ready_session, reach_due_instant, run, set_statement_due};
use crate::wire::{call, call_in_region, emulator_on, error_code, error_message, ok, REGION};

/// The `StatementState` values this suite observes, of the modelled enum (`WAITING`,
/// `RUNNING`, `AVAILABLE`, `CANCELLING`, `CANCELLED`, `ERROR`, from
/// `smithy_query shape=StatementState`).
const STATE_WAITING: &str = "WAITING";
const STATE_ERROR: &str = "ERROR";

/// An emulator plus the engine it runs on, so a suite can write the instants and the hold
/// order the rows turn on instead of waiting for them.
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (emulator_on(engine.clone()), engine)
}

/// The code a seeded statement carries, so every row can tell one statement from another.
fn code_for(id: i32) -> String {
    format!("print({id})")
}

/// Give the session `session` one statement per entry of `ids`, HELD in exactly the order
/// `ids` names them — written through the store, so a row fixes the hold order rather than
/// taking the order a run would have appended them in. Every statement is held in flight
/// for the whole row, so two listings of it report the same record.
fn seed_statements(engine: &Arc<engine::InMemoryEngine>, session: &str, ids: &[i32]) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    let entry = store
        .session_mut(REGION, session)
        .unwrap_or_else(|| panic!("the store must hold session {session:?}"));
    let now = SystemTime::now();
    entry.statements = ids
        .iter()
        .map(|id| {
            let mut statement = StatementEntry::run(*id, &code_for(*id), now);
            statement.settles_at = Some(far_future());
            statement
        })
        .collect();
    entry.next_statement_id = ids.iter().copied().max().unwrap_or(FIRST_STATEMENT_ID) + 1;
}

/// The `Statements` of one answer, which the operation reports even when the session holds
/// none.
fn listed_statements(page: &Value) -> Vec<Value> {
    page.get("Statements")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("ListStatements must answer a Statements list, got {page}"))
        .clone()
}

/// The `Id` of every statement one answer reports, in the order it reported them.
fn listed_ids(page: &Value) -> Vec<i64> {
    listed_statements(page)
        .iter()
        .map(|statement| {
            statement["Id"]
                .as_i64()
                .unwrap_or_else(|| panic!("a Statement carries an Id, got {statement}"))
        })
        .collect()
}

/// The token one answer carries, if it carries one.
fn token_of(page: &Value) -> Option<String> {
    page.get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// One `ListStatements` call for `session`, resumed from `token` when there is one.
async fn list(router: &MultiServiceRouter, session: &str, token: Option<&str>) -> Value {
    let mut request = json!({ "SessionId": session });
    if let Some(cursor) = token {
        request["NextToken"] = json!(cursor);
    }
    ok(router, "ListStatements", &request).await
}

/// Walk every page the listing hands out for `session`, following its own token, and answer
/// the ids in the order they were served plus how many pages it took.
async fn walk(
    router: &MultiServiceRouter,
    case: &str,
    session: &str,
    stored: usize,
) -> (Vec<i64>, usize) {
    let mut served: Vec<i64> = Vec::new();
    let mut pages = 0usize;
    let mut token: Option<String> = None;
    loop {
        let page = list(router, session, token.as_deref()).await;
        pages += 1;
        let ids = listed_ids(&page);
        assert!(
            ids.len() <= PAGE_SIZE,
            "case {case:?}: a page holds at most {PAGE_SIZE} statements, got {} of them",
            ids.len()
        );
        served.extend(ids);
        token = token_of(&page);
        if token.is_none() {
            break;
        }
        assert!(
            served.len() <= stored,
            "case {case:?}: the walk must terminate, got {served:?} for {stored} statements"
        );
    }
    (served, pages)
}

/// The ids `ids` names, in the order the rule puts them: the minted id ascending.
fn oldest_first(ids: &[i32]) -> Vec<i64> {
    let mut expected: Vec<i64> = ids.iter().map(|id| i64::from(*id)).collect();
    expected.sort_unstable();
    expected
}

/// The ids a session of `count` statements holds, as its runs would mint them.
fn minted_ids(count: usize) -> Vec<i32> {
    (0..count)
        .map(|index| FIRST_STATEMENT_ID + index as i32)
        .collect()
}

// ---------------------------------------------------------------------------
// What a run records is what the listing reports
// ---------------------------------------------------------------------------

/// The listing reports every statement `RunStatement` recorded on the session it names,
/// oldest first, each element the SAME projection `GetStatement` serves for it.
///
/// The rows differ along how many statements one session holds, from none up.
#[tokio::test]
async fn runs_are_listed_oldest_first_with_the_single_statement_projection() {
    // (case, how many statements the session holds)
    let rows: &[(&str, usize)] = &[
        ("a session nothing has run on", 0),
        ("one statement", 1),
        ("two statements", 2),
        ("five statements", 5),
    ];

    for (case, count) in rows {
        let (router, engine) = emulator_with_engine();
        let session = "listed-session";
        ready_session(&router, &engine, session).await;

        let mut minted = Vec::new();
        for index in 0..*count {
            minted.push(run(&router, session, &format!("print({index})")).await);
            // The session takes the next statement once the clock has finished this one.
            set_statement_due(&engine, session, already_past());
        }

        let page = list(&router, session, None).await;
        assert_eq!(
            listed_ids(&page),
            minted,
            "case {case:?}: expected the ids the runs minted, oldest first, got {page}"
        );
        assert!(
            token_of(&page).is_none(),
            "case {case:?}: a listing that reached the end carries no token, got {page}"
        );

        // Element for element the SAME statement the single-statement read reports: one
        // record, two readers.
        for (index, listed) in listed_statements(&page).iter().enumerate() {
            let id = minted[index];
            let read = get(&router, session, id).await;
            assert_eq!(
                listed, &read,
                "case {case:?}: element {index} must be the statement GetStatement reports \
                 for {id}, got {listed} and {read}"
            );
            assert_eq!(
                listed["Code"].as_str(),
                Some(format!("print({index})").as_str()),
                "case {case:?}: element {index} must carry its own code, got {listed}"
            );
        }
    }
}

/// The order is the minted `Id` ascending WHATEVER order the store holds the records in: a
/// session whose statements are held out of that order is still listed oldest first.
#[tokio::test]
async fn the_listing_order_is_the_minted_id_order_however_the_store_holds_them() {
    // (case, the ids the store holds, in the order it holds them)
    let rows: &[(&str, &[i32])] = &[
        ("the hold order is the minted order", &[1, 2, 3, 4, 5]),
        ("the hold order is reversed", &[5, 4, 3, 2, 1]),
        ("the hold order is scrambled", &[3, 1, 5, 2, 4]),
        (
            // Ids whose decimal spellings order differently as text than as numbers, so a
            // listing ordered by the id as a NUMBER is told apart from one ordered by its
            // digits.
            "ids either side of a digit boundary",
            &[9, 10, 2, 11, 1, 20, 3],
        ),
    ];

    for (case, held) in rows {
        let (router, engine) = emulator_with_engine();
        let session = "held-out-of-order";
        ready_session(&router, &engine, session).await;
        seed_statements(&engine, session, held);

        let page = list(&router, session, None).await;
        assert_eq!(
            listed_ids(&page),
            oldest_first(held),
            "case {case:?}: expected the minted ids ascending, got {page}"
        );
        // Every id still addresses the code it was seeded with, so the order was changed
        // and the records were not.
        for statement in listed_statements(&page) {
            let id = statement["Id"].as_i64().unwrap_or_default();
            assert_eq!(
                statement["Code"].as_str(),
                Some(code_for(id as i32).as_str()),
                "case {case:?}: statement {id} must still carry its own code, got {statement}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Paging
// ---------------------------------------------------------------------------

/// Following the service's own token walks every statement of the session exactly once, in
/// order, in pages no larger than the one page size this operation serves — and the last
/// page carries no token.
///
/// The rows span the page boundary: fewer than one page, exactly one, one more than one,
/// two pages exactly, more than two, and the full number of statements a session keeps.
#[tokio::test]
async fn following_the_token_walks_every_statement_exactly_once() {
    // (case, how many statements the session holds)
    let rows: &[(&str, usize)] = &[
        ("one statement", 1),
        ("one short of a page", PAGE_SIZE - 1),
        ("exactly one page", PAGE_SIZE),
        ("one more than a page", PAGE_SIZE + 1),
        ("exactly two pages", 2 * PAGE_SIZE),
        ("more than two pages", 2 * PAGE_SIZE + 3),
        (
            // The number of statements one session keeps, which is the most a walk can ever
            // have to cover.
            "every statement a session keeps",
            MAX_STATEMENTS_PER_SESSION,
        ),
    ];

    for (case, count) in rows {
        let (router, engine) = emulator_with_engine();
        let session = "paged-session";
        ready_session(&router, &engine, session).await;
        // Held in reverse, so paging is asserted over the minted order and not over the
        // order the store holds the records in.
        let mut held = minted_ids(*count);
        held.reverse();
        seed_statements(&engine, session, &held);

        let (served, pages) = walk(&router, case, session, *count).await;
        assert_eq!(
            served,
            oldest_first(&held),
            "case {case:?}: expected every stored statement once, oldest first, got {served:?}"
        );
        let pages_expected = count.div_ceil(PAGE_SIZE).max(1);
        assert_eq!(
            pages, pages_expected,
            "case {case:?}: expected {pages_expected} page(s) for {count} statements, got {pages}"
        );
    }
}

/// A token names a POSITION in the listing's order: the one the service hands out resumes
/// the walk strictly after the page it was handed out with, and one it never issued names a
/// place in that same order rather than being refused.
#[tokio::test]
async fn a_token_names_a_position_in_the_order() {
    let (router, engine) = emulator_with_engine();

    // A session holding more than one page, so the service issues a token of its own.
    let paged = "resumed-session";
    ready_session(&router, &engine, paged).await;
    let held = minted_ids(PAGE_SIZE + 3);
    seed_statements(&engine, paged, &held);
    let full = oldest_first(&held);

    let page = list(&router, paged, None).await;
    let token = token_of(&page).unwrap_or_else(|| {
        panic!("a listing that left statements unreported must carry a token, got {page}")
    });
    assert_eq!(
        listed_ids(&page),
        full[..PAGE_SIZE].to_vec(),
        "case \"the first page\": expected the first page of the order, got {page}"
    );
    // The token the service issued resumes strictly after that page's last statement: the
    // page it serves starts where the previous one stopped and overlaps it nowhere.
    let resumed = list(&router, paged, Some(&token)).await;
    assert_eq!(
        listed_ids(&resumed),
        full[PAGE_SIZE..].to_vec(),
        "case \"resumed from the issued token\": expected the statements after the first \
         page, got {resumed}"
    );
    assert!(
        token_of(&resumed).is_none(),
        "case \"resumed from the issued token\": the last page carries no token, got {resumed}"
    );
    // Resuming from the same token twice serves the same page: a token is a position, not a
    // consumed one-shot.
    let resumed_again = list(&router, paged, Some(&token)).await;
    assert_eq!(
        resumed, resumed_again,
        "case \"the same token twice\": a token must name the same position on every call, \
         got {resumed} and {resumed_again}"
    );

    // A session holding fewer statements than one page, so every answer below is a SUFFIX
    // of the whole order and a row is checked without knowing how a token is spelled.
    let short = "unissued-tokens";
    ready_session(&router, &engine, short).await;
    let short_held = minted_ids(5);
    seed_statements(&engine, short, &short_held);
    let short_full = oldest_first(&short_held);

    // (case, a token this service never issued)
    let unissued: &[(&str, &str)] = &[
        // "len 0..400000" (`OrchestrationToken`): the empty string is inside the modelled
        // length of the token, so it reaches the handler rather than being refused.
        ("the empty token", ""),
        ("a token that is not a cursor at all", "not-a-cursor"),
        ("a token spelled with one digit", "1"),
        ("a token above every cursor", "zzzzzzzzzz"),
    ];
    for (case, unissued_token) in unissued {
        let page = list(&router, short, Some(unissued_token)).await;
        let served = listed_ids(&page);
        assert!(
            short_full.ends_with(&served),
            "case {case:?}: a token must name a position in the order, so the answer is a \
             tail of {short_full:?}, got {served:?}"
        );
        assert!(
            token_of(&page).is_none(),
            "case {case:?}: a page that reached the end carries no token, got {page}"
        );
    }
}

// ---------------------------------------------------------------------------
// Only this session's statements, only this endpoint's session
// ---------------------------------------------------------------------------

/// The listing reports only the statements of the session it names, and a session is
/// listable only through the endpoint that created it.
#[tokio::test]
async fn the_listing_reports_only_the_statements_of_the_session_it_names() {
    let (router, engine) = emulator_with_engine();

    // (case, the session, the ids it holds)
    let rows: &[(&str, &str, &[i32])] = &[
        ("the session with three statements", "left", &[1, 2, 3]),
        ("the session with one statement", "right", &[1]),
        ("the session nothing has run on", "untouched", &[]),
    ];

    for (_, session, held) in rows {
        ready_session(&router, &engine, session).await;
        seed_statements(&engine, session, held);
    }

    for (case, session, held) in rows {
        let page = list(&router, session, None).await;
        assert_eq!(
            listed_ids(&page),
            oldest_first(held),
            "case {case:?}: expected only this session's statements, got {page}"
        );
        for statement in listed_statements(&page) {
            let id = statement["Id"].as_i64().unwrap_or_default();
            assert_eq!(
                statement["Code"].as_str(),
                Some(code_for(id as i32).as_str()),
                "case {case:?}: every listed statement is this session's own, got {statement}"
            );
        }
    }

    // The same session id through another Region's endpoint holds nothing: a session is
    // served only by the endpoint that created it.
    for region in ["eu-west-1", "us-west-2"] {
        let (status, body) = call_in_region(
            &router,
            region,
            "ListStatements",
            &json!({ "SessionId": "left" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {region:?}: a session is reachable only through its own endpoint, got \
             {status} with {body}"
        );
        assert_eq!(
            error_message(&body),
            messages::session_id_not_found("left"),
            "case {region:?}: expected the statement surface's session miss, got {body}"
        );
    }
}

/// A session this endpoint does not hold is a miss, whatever else the request carries —
/// never an empty listing.
#[tokio::test]
async fn a_session_the_store_does_not_hold_is_not_found() {
    let (router, engine) = emulator_with_engine();
    ready_session(&router, &engine, "held").await;
    seed_statements(&engine, "held", &[1, 2]);

    // A session id is NOT folded, so an id differing only in case names another session —
    // and a token on the request does not change the refusal.
    let rows: &[(&str, &str, Value)] = &[
        ("an id nothing created", "absent", json!({})),
        ("a prefix of a stored id", "hel", json!({})),
        ("a stored id in another case", "HELD", json!({})),
        (
            "an id nothing created, with a token",
            "absent",
            json!({ "NextToken": "0000000001" }),
        ),
        (
            "an id nothing created, with a request origin",
            "absent",
            json!({ "RequestOrigin": "glue-studio" }),
        ),
    ];

    for (case, session, extra) in rows {
        let mut request = json!({ "SessionId": session });
        if let Some(members) = extra.as_object() {
            for (member, value) in members {
                request[member] = value.clone();
            }
        }
        let (status, body) = call(&router, "ListStatements", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {case:?}: expected an EntityNotFoundException, got {} in {body}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            messages::session_id_not_found(session),
            "case {case:?}: the refusal must name the SessionId the request addressed, got {body}"
        );
    }

    // The session the store DOES hold is served, so the refusal is about the id and not
    // about the operation.
    let page = list(&router, "held", None).await;
    assert_eq!(
        listed_ids(&page),
        vec![1, 2],
        "case \"the held session\": expected its two statements, got {page}"
    );
}

// ---------------------------------------------------------------------------
// The state the clock owes, and nothing else
// ---------------------------------------------------------------------------

/// The listing reports the state the CLOCK owes each statement and changes nothing else:
/// two listings of statements whose terminal state is not yet due agree, the state the
/// clock owed is reported once it is due and keeps being reported, and a session still
/// actioning a statement is still busy after it has been listed.
#[tokio::test]
async fn listing_statements_reports_the_state_the_clock_owes_and_changes_nothing_else() {
    let (router, engine) = emulator_with_engine();
    let session = "settling";
    ready_session(&router, &engine, session).await;
    let in_flight = run(&router, session, "spark.sql('SELECT 1').show()").await;
    // The statement stays in flight for the whole span, whatever a caller reads.
    set_statement_due(&engine, session, far_future());

    let first = list(&router, session, None).await;
    let second = list(&router, session, None).await;
    assert_eq!(
        first, second,
        "case \"nothing due\": two listings must report the same statements, got {first} and \
         {second}"
    );
    for statement in listed_statements(&first) {
        assert_eq!(
            statement["State"].as_str(),
            Some(STATE_WAITING),
            "case \"nothing due\": a statement whose span has not elapsed is still \
             {STATE_WAITING}, got {statement}"
        );
    }

    // Listing a session's statements does not finish the one in flight: the session is
    // still busy, so a further run is still refused.
    let (status, body) = call(
        &router,
        "RunStatement",
        &json!({ "SessionId": session, "Code": "print('after the listing')" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"nothing due\": a listing must not free a busy session, got {status} with {body}"
    );
    assert_eq!(
        error_message(&body),
        messages::session_busy_with_statement(session, in_flight as i32),
        "case \"nothing due\": the refusal must still name the statement in flight, got {body}"
    );

    // Once the clock owes the terminal state, the listing reports it — and keeps reporting
    // the same thing, so the listing materialises what was due rather than moving a
    // statement because it was read.
    reach_due_instant(&engine, session);
    let settled = list(&router, session, None).await;
    let again = list(&router, session, None).await;
    assert_eq!(
        settled, again,
        "case \"the terminal state fell due\": two listings of settled statements must \
         agree, got {settled} and {again}"
    );
    for statement in listed_statements(&settled) {
        assert_eq!(
            statement["State"].as_str(),
            Some(STATE_ERROR),
            "case \"the terminal state fell due\": expected the state the clock owed, got \
             {statement}"
        );
        assert!(
            statement["CompletedOn"].as_i64().unwrap_or_default()
                > statement["StartedOn"].as_i64().unwrap_or_default(),
            "case \"the terminal state fell due\": a finished statement completed after it \
             started, got {statement}"
        );
    }
    // And the single-statement read agrees with the listing, both before and after.
    let read = get(&router, session, in_flight).await;
    assert_eq!(
        listed_statements(&settled).first(),
        Some(&read),
        "case \"the terminal state fell due\": the listing and the single read must report \
         one record, got {settled} and {read}"
    );
}
