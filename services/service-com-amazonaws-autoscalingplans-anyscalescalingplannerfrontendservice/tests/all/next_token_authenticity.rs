// SPDX-License-Identifier: CC0-1.0

//! A `NextToken` is opaque server state: each paged read accepts ONLY the tokens it
//! handed out itself, and answers every other string with the modelled
//! `InvalidNextTokenException` (HTTP 400) instead of serving a page.
//!
//! The rule pinned here: decoding a cursor is the exact INVERSE of minting one, so a
//! token is accepted only when it is the very string this read would have produced for
//! the position it carries — same namespace, same spelling, and a position the minting
//! side can actually produce. A string that merely LOOKS like a token (a well-formed
//! token whose position is empty, a re-cased spelling of a real one, the sibling read's
//! token) is not a token this service minted, so it is refused rather than silently
//! restarting the walk at the beginning — which would make a client paging in a loop
//! re-read the first page forever.
//!
//! Rows differ along what that rule generalises over: which paged read is walked
//! (`DescribeScalingPlans` over plans, `DescribeScalingPlanResources` over the resources
//! one plan covers), and the SHAPE of the string offered as a token — the position it
//! carries, its hex spelling, its namespace, its separator, and whether it is hex or
//! text at all.
//!
//! The harness is reused from [`crate::scaling_plan_lifecycle`]; nothing here builds a
//! second one.

use serde_json::{json, Value};
use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::state as plan_state;

use crate::scaling_plan_lifecycle::{instruction, stack_source, Emulator};

/// The plan whose covered resources the resource read walks.
const PLAN: &str = "plan-a";

/// The `ServiceNamespace` and `ScalableDimension` the walked resources carry
/// (`smithy_query(query="shape",shape="ServiceNamespace,ScalableDimension")`).
const NAMESPACE: &str = "autoscaling";
const DIMENSION: &str = "autoscaling:autoScalingGroup:DesiredCapacity";

/// The documented maximum length of a `ScalingPlanName`: the shape is
/// `(string, len 1..128)` (`smithy_query(query="shape",shape="ScalingPlanName")`), so the
/// longest position a plans cursor can carry is 128 characters.
const PLAN_NAME_MAX: usize = 128;

/// One paged read: the operation, the base request that walks it, the member its page is
/// reported in, and the namespace its own cursors carry (the `CURSOR_KIND` of its
/// handler).
struct PagedRead {
    operation: &'static str,
    collection: &'static str,
    kind: &'static str,
    base: Value,
}

/// The two paged reads this service serves — the only two callers of the cursor helper.
fn paged_reads() -> Vec<PagedRead> {
    vec![
        PagedRead {
            operation: "DescribeScalingPlans",
            collection: "ScalingPlans",
            kind: "plans",
            base: json!({}),
        },
        PagedRead {
            operation: "DescribeScalingPlanResources",
            collection: "ScalingPlanResources",
            kind: "plan-resources",
            base: json!({ "ScalingPlanName": PLAN, "ScalingPlanVersion": 1 }),
        },
    ]
}

/// One scaling instruction covering the Auto Scaling group named by `letter`.
fn asg_instruction(letter: char) -> Value {
    instruction(
        NAMESPACE,
        &format!("autoScalingGroup/asg-{letter}"),
        DIMENSION,
        1,
        10,
        "ASGAverageCPUUtilization",
        40.0,
    )
}

/// Three plans, the first of which covers three resources — so both paged reads have
/// more than one page to walk and therefore mint a cursor.
async fn seeded() -> Emulator {
    let emulator = Emulator::new();
    for (index, (name, letters)) in [
        (PLAN, &['a', 'b', 'c'][..]),
        ("plan-b", &['d'][..]),
        ("plan-c", &['e'][..]),
    ]
    .into_iter()
    .enumerate()
    {
        let instructions: Vec<Value> = letters.iter().copied().map(asg_instruction).collect();
        emulator
            .ok(
                "CreateScalingPlan",
                json!({
                    "ScalingPlanName": name,
                    "ApplicationSource": stack_source(index + 1),
                    "ScalingInstructions": instructions,
                }),
            )
            .await;
    }
    emulator.make_all_due();
    emulator
}

/// One page of `read`: the entries it reports and the token it hands back.
async fn page(
    emulator: &Emulator,
    read: &PagedRead,
    max_results: Option<usize>,
    token: Option<&str>,
) -> (Vec<Value>, Option<String>) {
    let mut request = read.base.clone();
    if let Some(max_results) = max_results {
        request["MaxResults"] = json!(max_results);
    }
    if let Some(token) = token {
        request["NextToken"] = json!(token);
    }
    let body = emulator.ok(read.operation, request).await;
    let entries = body[read.collection]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {}: no {} in {body:?}", read.operation, read.collection));
    let next_token = body
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (entries, next_token)
}

/// `minted` with its first lower-case hex letter written UPPER case — a spelling
/// `hex::decode` accepts and the minting side never emits.
///
/// `None` for a token written entirely in hex DIGITS, which has only the one spelling:
/// there is no re-cased variant of it to probe.
fn recased(minted: &str) -> Option<String> {
    let at = minted
        .char_indices()
        .find(|(_, character)| character.is_ascii_lowercase())
        .map(|(index, _)| index)?;
    let mut spelled = minted.to_string();
    spelled.replace_range(at..=at, &minted[at..=at].to_uppercase());
    Some(spelled)
}

/// The strings that are NOT tokens `read` minted, labelled by the shape each one has.
///
/// `minted` is a real token of this read; `sibling` a real token of the other paged read.
fn forged_tokens(read: &PagedRead, minted: &str, sibling: &str) -> Vec<(String, String)> {
    let kind = read.kind;
    let mut rows = vec![
        // The position no minting site can produce: a plans cursor carries a
        // `ScalingPlanName` (modelled non-empty) and a resources cursor a resource
        // identity key (which always carries its separators).
        (
            "a well-formed token carrying an empty position".to_string(),
            plan_state::encode_cursor(kind, ""),
        ),
        (
            "the namespace with no separator after it".to_string(),
            hex::encode(kind),
        ),
        (
            "a position with no namespace before it".to_string(),
            hex::encode(format!(":{PLAN}")),
        ),
        (
            "a bare position, neither namespace nor separator".to_string(),
            hex::encode(PLAN),
        ),
        (
            "a separator other than the one the namespace is written with".to_string(),
            hex::encode(format!("{kind}|{PLAN}")),
        ),
        (
            "the namespace of a token the sibling read mints".to_string(),
            sibling.to_string(),
        ),
        (
            "a minted token with an odd number of hex digits".to_string(),
            format!("{minted}a"),
        ),
        (
            "hex that is not UTF-8".to_string(),
            hex::encode([0xffu8, 0xfe]),
        ),
        (
            "text that is not hex at all".to_string(),
            "garbage".to_string(),
        ),
        ("the empty string".to_string(), String::new()),
    ];
    // Re-casing the hex of a minted token makes it a token nobody minted. Both spellings
    // are probed only where they EXIST: a token written in hex digits alone has just the
    // one spelling, so there would be nothing to refuse.
    let upper = minted.to_uppercase();
    if upper != minted {
        rows.push((
            "the upper-cased spelling of a minted token".to_string(),
            upper,
        ));
    }
    if let Some(spelled) = recased(minted) {
        rows.push((
            "a mixed-case spelling of a minted token".to_string(),
            spelled,
        ));
    }
    rows
}

#[tokio::test]
async fn a_paged_read_refuses_every_token_it_did_not_mint() {
    let emulator = seeded().await;
    let reads = paged_reads();

    // What each read reports unpaged, and the token each hands out mid-walk. Both are
    // read off the service rather than written down here: the tokens under test are
    // real ones, so a refusal cannot be mistaken for the service refusing its own.
    let mut whole: Vec<Vec<Value>> = Vec::new();
    let mut minted: Vec<String> = Vec::new();
    for read in &reads {
        let (entries, token) = page(&emulator, read, None, None).await;
        assert!(
            entries.len() > 1 && token.is_none(),
            "case {}: the unpaged read must report the whole collection at once, got {} \
             entries with {token:?}",
            read.operation,
            entries.len()
        );
        let (first, token) = page(&emulator, read, Some(1), None).await;
        let token = token.unwrap_or_else(|| {
            panic!(
                "case {}: a one-entry page must hand back a cursor, got {first:?}",
                read.operation
            )
        });
        whole.push(entries);
        minted.push(token);
    }

    for (index, read) in reads.iter().enumerate() {
        let own = minted[index].clone();
        let sibling = minted[(index + 1) % minted.len()].clone();

        // The token this read minted IS accepted, and resumes strictly after the entry
        // the first page stopped at.
        let (resumed, _) = page(&emulator, read, Some(1), Some(&own)).await;
        let expected: Vec<Value> = whole[index].iter().skip(1).take(1).cloned().collect();
        assert_eq!(
            resumed, expected,
            "case {} NextToken=<minted>: the walk must resume after the first entry, got \
             {resumed:?}",
            read.operation
        );

        // Everything else is refused — with the modelled 400, never with a page.
        for (label, token) in forged_tokens(read, &own, &sibling) {
            let mut request = read.base.clone();
            request["NextToken"] = json!(token);
            let (status, parsed) = emulator.call(read.operation, &request).await;
            assert_eq!(
                (
                    status,
                    crate::scaling_plan_lifecycle::error_code(&parsed)
                ),
                (400, "InvalidNextTokenException"),
                "case {} NextToken={token:?} ({label}): expected a 400 \
                 InvalidNextTokenException, got {status} with {parsed}",
                read.operation
            );
        }
    }
}

#[tokio::test]
async fn a_refused_token_does_not_restart_the_walk() {
    let emulator = seeded().await;
    for read in paged_reads() {
        // The forged token the recorded instance offers: a well-formed token whose
        // position is empty. Before the fix it resumed "after the empty position", i.e.
        // at the very beginning, so the caller got the full first page with HTTP 200.
        let forged = plan_state::encode_cursor(read.kind, "");
        let (unpaged, _) = page(&emulator, &read, None, None).await;
        let mut request = read.base.clone();
        request["MaxResults"] = json!(50);
        request["NextToken"] = json!(forged);
        let (status, parsed) = emulator.call(read.operation, &request).await;
        assert_eq!(
            (status, crate::scaling_plan_lifecycle::error_code(&parsed)),
            (400, "InvalidNextTokenException"),
            "case {} NextToken={forged:?}: a token this service never minted must be \
             refused with the modelled error, not served a page, got {status} with {parsed}",
            read.operation
        );
        assert!(
            parsed.get(read.collection).is_none(),
            "case {} NextToken={forged:?}: the refusal carries no page, got {parsed}",
            read.operation
        );
        // The refusal changed nothing: the collection reads back exactly as before.
        let (again, _) = page(&emulator, &read, None, None).await;
        assert_eq!(
            again, unpaged,
            "case {}: a refused token leaves the collection untouched, got {again:?}",
            read.operation
        );
    }
}

#[test]
fn the_cursor_helper_decodes_exactly_what_it_mints() {
    // The helper is namespace-agnostic, so the rule is pinned on the two namespaces this
    // service walks AND on one that is neither.
    let kinds = ["plans", "plan-resources", "forecasts"];
    // Positions spanning what a cursor can carry: a one-character name (the lower bound
    // of `ScalingPlanName`, `len 1..128`), an ordinary name, a name at that upper bound,
    // a resource identity key with its escaped separators, a position holding the
    // namespace separator itself, and non-ASCII text.
    let positions = [
        "a".to_string(),
        "plan-a".to_string(),
        "n".repeat(PLAN_NAME_MAX),
        format!("{NAMESPACE}/autoScalingGroup\\/asg-a/{DIMENSION}"),
        "a:b".to_string(),
        "üñî-plan".to_string(),
    ];

    for kind in kinds {
        for position in &positions {
            let token = plan_state::encode_cursor(kind, position);
            assert_eq!(
                plan_state::decode_cursor(kind, &token).as_deref(),
                Some(position.as_str()),
                "case ({kind:?}, {position:?}): a minted token must decode to its own \
                 position, got {:?}",
                plan_state::decode_cursor(kind, &token)
            );

            // Re-spelling the hex of a minted token makes it a token nobody minted.
            let upper = token.to_uppercase();
            if upper != token {
                assert_eq!(
                    plan_state::decode_cursor(kind, &upper),
                    None,
                    "case ({kind:?}, {position:?}): the upper-cased spelling is not a \
                     minted token, got {:?}",
                    plan_state::decode_cursor(kind, &upper)
                );
            }

            // A token minted under another namespace does not decode under this one.
            for other in kinds.iter().filter(|other| **other != kind) {
                let foreign = plan_state::encode_cursor(other, position);
                assert_eq!(
                    plan_state::decode_cursor(kind, &foreign),
                    None,
                    "case ({kind:?}, {position:?}): a {other:?} token is not a {kind:?} \
                     token, got {:?}",
                    plan_state::decode_cursor(kind, &foreign)
                );
            }
        }

        // The one position minting can never produce: empty. No caller of
        // `encode_cursor` passes an empty value, so no token carrying one was minted.
        assert_eq!(
            plan_state::decode_cursor(kind, &plan_state::encode_cursor(kind, "")),
            None,
            "case {kind:?}: an empty position is not a position this service mints, got \
             {:?}",
            plan_state::decode_cursor(kind, &plan_state::encode_cursor(kind, ""))
        );
    }
}
