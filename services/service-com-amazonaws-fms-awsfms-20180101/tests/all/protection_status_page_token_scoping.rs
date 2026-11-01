// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the page-token rule `GetProtectionStatus` shares with
//! every other paginated read of this service whose page set is SELECTED by the
//! request.
//!
//! The rule: such a read's `NextToken` names a page boundary only under the
//! WHOLE selection the request that minted it named, so a token minted under one
//! selection is refused under another rather than resumed from as an arbitrary
//! key — a boundary of one selection may be absent from the next, or sort past
//! items the next does hold, so resuming from it there would silently drop them
//! and report a short page, or an empty one, as a complete page a caller cannot
//! tell apart from "nothing was recorded". A refused call reports nothing and
//! leaves the page as it was, and the token a selection does mint still walks
//! that selection's whole page.
//!
//! Here the selection is FOUR request members at once — the `PolicyId` whose
//! attacks are reported, the `MemberAccountId` whose attacked resources are
//! reported, and the `StartTime` / `EndTime` bounds of the queried period —
//! because all four narrow the very iterator the boundary is minted from. A
//! boundary carrying only part of that selection is therefore no boundary of any
//! of these pages either, and neither is the bare attack key nor a boundary of
//! the OTHER collection this service narrows by the same policy
//! (`ListComplianceStatus`).
//!
//! Asserted over a TABLE whose rows are eight selections of ONE policy's attack
//! inventory. They differ in whether a member account is named and in which one
//! (two accounts holding attacks, one holding a single attack, and one the policy
//! applies to that holds none — whose page is EMPTY, the answer a caller must be
//! able to tell apart from a truncated page), in which period bounds are present
//! (neither, the upper one only, both) and in how many attacks the page holds
//! (none, one, two, three, all five). Two rows select DIFFERENT ways and hold the
//! same attacks, so the stamp is what tells their tokens apart rather than the
//! page's contents.
//! [`the_selections_this_suite_serves_are_pages_that_interleave`] pins those
//! preconditions, so the refusal rows below cannot quietly stop covering the case
//! where resuming from a foreign boundary would drop attacks.
//!
//! The attack inventory is seeded through the store, the way the sibling
//! `protection_status` suite seeds it: Shield Advanced detects attacks and this
//! emulator stands in for no detector, so the only honest source of an attack is
//! one somebody put in the store.
//!
//! Every test drives the service the way a client does — an awsJson1_1 `POST /`
//! through the wire harness in [`crate::policy_lifecycle`] — so the assertions
//! are about the WIRE response.

use std::sync::Arc;

use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::state;
use smithy_server_runtime::MultiServiceRouter;

// The wire harness, the store seam, the policy writer and the attack seeder live
// in one place each for this test target, so no two suites can drift in how a
// request is signed, the store is seeded or an attack is recorded.
use crate::administrator_gated_operations::{associate_admin, with_store};
use crate::policy_lifecycle::{call, emulator_with_engine, error_code};
use crate::protection_status::{
    at, put_scoped_policy, seed_attack, stamp, Attack, BASE_SECS, SCOPED_ACCOUNTS,
};

/// The attacks seeded against the policy under test, deliberately NOT inserted
/// in chronological order, so the order the pages below serve is the store's
/// doing and not the seeding's.
///
/// Two accounts hold two attacks each, a third holds one ongoing attack, and the
/// fourth account the policy applies to holds none. The accounts INTERLEAVE in
/// the chronological order the keys carry: the earliest two attacks belong to
/// one account and the latest two to another, so a boundary of the later
/// account's page sorts ABOVE everything the earlier one holds.
fn attacks() -> Vec<Attack> {
    vec![
        Attack {
            attack_id: "first-222",
            account: SCOPED_ACCOUNTS[1],
            start_secs: 3_600,
            start_millis: 0,
            duration_secs: Some(60),
            vectors: &["SYN_FLOOD"],
        },
        Attack {
            attack_id: "early-111",
            account: SCOPED_ACCOUNTS[0],
            start_secs: 0,
            start_millis: 0,
            duration_secs: Some(60),
            vectors: &["UDP_TRAFFIC"],
        },
        Attack {
            attack_id: "second-222",
            account: SCOPED_ACCOUNTS[1],
            start_secs: 7_200,
            start_millis: 0,
            duration_secs: Some(60),
            vectors: &["REQUEST_FLOOD", "UDP_REFLECTION"],
        },
        Attack {
            attack_id: "ongoing-333",
            account: SCOPED_ACCOUNTS[2],
            start_secs: 1_800,
            start_millis: 0,
            duration_secs: None,
            vectors: &["HTTP_REFLECTION"],
        },
        Attack {
            attack_id: "later-111",
            account: SCOPED_ACCOUNTS[0],
            start_secs: 600,
            start_millis: 500,
            duration_secs: Some(60),
            vectors: &[],
        },
    ]
}

/// One selection of that inventory: the four values a request selects its page
/// by, and the attack ids the page holds in the chronological order it serves
/// them.
///
/// Each row's `ids` follow from the two documented filters: `MemberAccountId`
/// selects the attacks on THAT account's resources, and an attack is in the
/// queried period when it OVERLAPS it (it started no later than `EndTime` and
/// had not already ended at `StartTime`, with an ongoing attack never excluded
/// by the lower bound and an absent bound constraining that side not at all).
struct Selection {
    label: &'static str,
    account: Option<&'static str>,
    start_secs: Option<u64>,
    end_secs: Option<u64>,
    ids: &'static [&'static str],
}

impl Selection {
    /// This selection's `StartTime` as the request's modelled `TimeStamp` — the
    /// value the page's stamp is built from.
    fn start(&self) -> Option<DateTime> {
        self.start_secs.map(bound)
    }

    /// This selection's `EndTime`, likewise.
    fn end(&self) -> Option<DateTime> {
        self.end_secs.map(bound)
    }

    /// The `NextToken` this selection's page mints for the boundary `key`.
    fn token(&self, policy_id: &str, key: &str) -> String {
        state::encode_shield_attack_page_token(
            policy_id,
            self.account,
            self.start(),
            self.end(),
            key,
        )
    }
}

/// The selections the rule is asserted over.
fn selections() -> Vec<Selection> {
    vec![
        Selection {
            label: "account 111, whole history",
            account: Some(SCOPED_ACCOUNTS[0]),
            start_secs: None,
            end_secs: None,
            ids: &["early-111", "later-111"],
        },
        Selection {
            label: "account 222, whole history",
            account: Some(SCOPED_ACCOUNTS[1]),
            start_secs: None,
            end_secs: None,
            ids: &["first-222", "second-222"],
        },
        Selection {
            label: "account 333, whole history",
            account: Some(SCOPED_ACCOUNTS[2]),
            start_secs: None,
            end_secs: None,
            ids: &["ongoing-333"],
        },
        Selection {
            // The policy applies to this account, so the page is EMPTY rather
            // than the modelled not-found.
            label: "account 444, whole history",
            account: Some(SCOPED_ACCOUNTS[3]),
            start_secs: None,
            end_secs: None,
            ids: &[],
        },
        Selection {
            label: "every account, whole history",
            account: None,
            start_secs: None,
            end_secs: None,
            ids: &[
                "early-111",
                "later-111",
                "ongoing-333",
                "first-222",
                "second-222",
            ],
        },
        Selection {
            // The two earliest attacks had ended before this period began; the
            // ongoing one has not ended at all.
            label: "every account, from 1800 to 7200",
            account: None,
            start_secs: Some(1_800),
            end_secs: Some(7_200),
            ids: &["ongoing-333", "first-222", "second-222"],
        },
        Selection {
            // The SAME attacks as "account 111, whole history", selected another
            // way: both of that account's attacks started no later than this
            // upper bound.
            label: "account 111, up to 601",
            account: Some(SCOPED_ACCOUNTS[0]),
            start_secs: None,
            end_secs: Some(601),
            ids: &["early-111", "later-111"],
        },
        Selection {
            // An account that HAS attacks, none of them in this period.
            label: "account 222, up to 601",
            account: Some(SCOPED_ACCOUNTS[1]),
            start_secs: None,
            end_secs: Some(601),
            ids: &[],
        },
    ]
}

/// `BASE_SECS` plus `offset_secs` as the modelled `TimeStamp` a request bound
/// carries — the same instant [`stamp`] writes on the wire.
fn bound(offset_secs: u64) -> DateTime {
    DateTime::from_secs(
        i64::try_from(BASE_SECS + offset_secs).expect("the seeded instants are well inside i64"),
    )
}

/// The store key the attack named `attack_id` is held under, built through the
/// service's own key writer so this suite cannot disagree with it about the
/// form.
fn attack_key(policy_id: &str, attack_id: &str) -> String {
    let attacks = attacks();
    let attack = attacks
        .iter()
        .find(|attack| attack.attack_id == attack_id)
        .unwrap_or_else(|| panic!("case {attack_id}: this suite seeds no such attack"));
    state::shield_attack_key(
        policy_id,
        at(attack.start_secs, attack.start_millis),
        attack.attack_id,
    )
}

/// A fresh emulator with an administrator associated, one Shield Advanced policy
/// scoped to [`SCOPED_ACCOUNTS`] and the attack table seeded against it.
async fn fixture() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>, String) {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;
    let policy_id = put_scoped_policy(
        &router,
        "protection-status-page-token-scope",
        "SHIELD_ADVANCED",
        Some(json!({ "ACCOUNT": SCOPED_ACCOUNTS })),
        None,
    )
    .await;
    with_store(&engine, |store| {
        for attack in &attacks() {
            seed_attack(store, &policy_id, attack);
        }
    });
    (router, engine, policy_id)
}

/// One `GetProtectionStatus` request for a selection, with the optional page
/// bound and token.
fn request(
    policy_id: &str,
    selection: &Selection,
    max_results: Option<usize>,
    token: Option<&str>,
) -> Value {
    let mut request = json!({ "PolicyId": policy_id });
    if let Some(account) = selection.account {
        request["MemberAccountId"] = json!(account);
    }
    if let Some(start) = selection.start_secs {
        request["StartTime"] = stamp(start);
    }
    if let Some(end) = selection.end_secs {
        request["EndTime"] = stamp(end);
    }
    if let Some(max_results) = max_results {
        request["MaxResults"] = json!(max_results);
    }
    if let Some(token) = token {
        request["NextToken"] = json!(token);
    }
    request
}

/// The attack ids one response reports, in the order it served them — empty when
/// it carries no `Data` document at all, which is what a refusal looks like.
fn served(case: &str, body: &Value) -> Vec<String> {
    let Some(data) = body.get("Data").and_then(Value::as_str) else {
        return Vec::new();
    };
    match serde_json::from_str::<Value>(data) {
        Ok(Value::Array(entries)) => entries
            .iter()
            .map(|entry| {
                entry["AttackId"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case}: every reported attack carries its AttackId, got {entry}")
                    })
                    .to_string()
            })
            .collect(),
        other => panic!("case {case}: Data must be a JSON array, got {data:?} ({other:?})"),
    }
}

/// The `NextToken` a response carries, if any.
fn next_token(body: &Value) -> Option<String> {
    body.get("NextToken")
        .filter(|token| !token.is_null())
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Every attack one selection reports, walking the tokens it mints at page size
/// `max_results`: each page is served, bounded by the size asked for, and the
/// walk ends at a page carrying no token.
async fn walk(
    router: &MultiServiceRouter,
    policy_id: &str,
    selection: &Selection,
    max_results: Option<usize>,
) -> Vec<String> {
    let case = format!("{} / MaxResults {max_results:?}", selection.label);
    let mut reported: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // A bound no chain of this fixture can need, so a token that never clears
    // fails here instead of looping forever.
    for round in 0..32 {
        let (status, body) = call(
            router,
            "GetProtectionStatus",
            &request(policy_id, selection, max_results, token.as_deref()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case} / page {round}: expected 200, got {status} with {body}"
        );
        let page = served(&case, &body);
        if let Some(size) = max_results {
            assert!(
                page.len() <= size,
                "case {case} / page {round}: a page must not exceed the requested size, got \
                 {page:?}"
            );
        }
        reported.extend(page);
        match next_token(&body) {
            Some(next) => token = Some(next),
            None => return reported,
        }
    }
    panic!("case {case}: the NextToken chain did not end, reported {reported:?}");
}

/// The token the first page of size one mints for a selection, asserting that
/// page is that selection's first attack — `None` for a selection whose page
/// holds one attack or none, which mints no token at all.
async fn first_token(
    router: &MultiServiceRouter,
    policy_id: &str,
    selection: &Selection,
) -> Option<String> {
    let case = format!("{} / first page", selection.label);
    let (status, body) = call(
        router,
        "GetProtectionStatus",
        &request(policy_id, selection, Some(1), None),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: the first page must be served, got {status} with {body}"
    );
    assert_eq!(
        served(&case, &body),
        selection
            .ids
            .iter()
            .take(1)
            .map(|id| (*id).to_string())
            .collect::<Vec<String>>(),
        "case {case}: expected the selection's first attack, got {body}"
    );
    let token = next_token(&body);
    assert_eq!(
        token.is_some(),
        selection.ids.len() > 1,
        "case {case}: a token must be minted exactly while attacks remain after the page, got \
         {token:?} for {:?}",
        selection.ids
    );
    token
}

/// A token that names no boundary of `target`'s page is refused with the
/// operation's modelled `InvalidInputException`, reports nothing, and leaves that
/// page as it was.
async fn refuse(
    router: &MultiServiceRouter,
    policy_id: &str,
    target: &Selection,
    token: &str,
    case: &str,
) {
    let (status, body) = call(
        router,
        "GetProtectionStatus",
        &request(policy_id, target, None, Some(token)),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidInputException"),
        "case {case}: a token naming no boundary of the selected page must be refused, got \
         {status} with {body}"
    );
    assert!(
        body.get("Data").is_none_or(Value::is_null) && next_token(&body).is_none(),
        "case {case}: a refused call must report nothing, got {body}"
    );
    let after = walk(router, policy_id, target, None).await;
    assert_eq!(
        after,
        target.ids.to_vec(),
        "case {case}: a refused token must leave the selected page as it was, got {after:?}"
    );
}

/// The preconditions every row below reads against: each selection reports
/// exactly the attacks its row names, the selections that hold nothing answer an
/// EMPTY page, and the pages INTERLEAVE in the chronological key order they are
/// served in — so resuming one selection from another's boundary would drop
/// attacks rather than merely reorder them.
#[tokio::test]
async fn the_selections_this_suite_serves_are_pages_that_interleave() {
    let (router, _engine, policy_id) = fixture().await;

    for selection in &selections() {
        let reported = walk(&router, &policy_id, selection, None).await;
        assert_eq!(
            reported,
            selection.ids.to_vec(),
            "case {}: expected exactly that selection's attacks, got {reported:?}",
            selection.label
        );
    }

    // For at least one ordered pair, the boundary one selection mints sorts
    // ABOVE attacks another selection holds, so resuming the other from it would
    // silently drop them.
    let mut dropped = 0;
    for minter in &selections() {
        let Some(boundary) = minter.ids.get(1).map(|id| attack_key(&policy_id, id)) else {
            continue;
        };
        for target in &selections() {
            if target.label == minter.label {
                continue;
            }
            dropped += target
                .ids
                .iter()
                .filter(|id| attack_key(&policy_id, id) < boundary)
                .count();
        }
    }
    assert!(
        dropped > 0,
        "this suite needs selections that interleave, so a foreign boundary would drop attacks; \
         got none over {} selections",
        selections().len()
    );
}

/// RULE: a `NextToken` of this read names a page boundary only under the whole
/// selection — policy, member account, `StartTime`, `EndTime` — that minted it.
/// Replayed under another selection, sent as the bare attack key, carrying only
/// part of a selection, or minted by the other listing narrowed by the same
/// policy, it is refused with the modelled `InvalidInputException`; under its own
/// selection it resumes the page.
#[tokio::test]
async fn a_token_is_a_boundary_only_under_the_selection_that_minted_it() {
    let (router, _engine, policy_id) = fixture().await;
    let selections = selections();

    for minter in &selections {
        let Some(token) = first_token(&router, &policy_id, minter).await else {
            continue;
        };

        // Under ANOTHER selection the same token names no boundary — including
        // the selections whose pages are empty, where accepting it would be
        // indistinguishable from "no attack was recorded", and the one holding
        // the very same attacks, where only the stamp tells the two apart.
        for target in &selections {
            if target.label == minter.label {
                continue;
            }
            let case = format!(
                "token minted under {} replayed under {}",
                minter.label, target.label
            );
            refuse(&router, &policy_id, target, &token, &case).await;
        }

        // Under the selection that minted it, it resumes exactly after the page
        // it was minted for — so the refusals above are not refusing every
        // token.
        let case = format!("token minted under {} replayed under itself", minter.label);
        let (status, body) = call(
            &router,
            "GetProtectionStatus",
            &request(&policy_id, minter, None, Some(&token)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: the minting selection's own token must resume its page, got {status} \
             with {body}"
        );
        assert_eq!(
            served(&case, &body),
            minter.ids[1..].to_vec(),
            "case {case}: expected the attacks after the first, got {body}"
        );
    }

    // A boundary carrying only PART of what selects the page is no boundary of
    // it, and neither is the bare key — the unstamped form, which names no
    // selection at all.
    for target in &selections {
        let key = attack_key(&policy_id, "early-111");
        let mut partial = vec![
            ("the bare attack key, carrying no selection", hex::encode(&key)),
            (
                "only the policy, the way the other policy-narrowed listing stamps its page",
                state::encode_scoped_page_token(&policy_id, &key),
            ),
        ];
        if target.account.is_some() {
            partial.push((
                "the selection with no member account",
                state::encode_shield_attack_page_token(
                    &policy_id,
                    None,
                    target.start(),
                    target.end(),
                    &key,
                ),
            ));
        }
        if target.start_secs.is_some() {
            partial.push((
                "the selection with no StartTime",
                state::encode_shield_attack_page_token(
                    &policy_id,
                    target.account,
                    None,
                    target.end(),
                    &key,
                ),
            ));
        }
        if target.end_secs.is_some() {
            partial.push((
                "the selection with no EndTime",
                state::encode_shield_attack_page_token(
                    &policy_id,
                    target.account,
                    target.start(),
                    None,
                    &key,
                ),
            ));
        }
        for (origin, token) in partial {
            let case = format!("{origin}, sent under {}", target.label);
            refuse(&router, &policy_id, target, &token, &case).await;
        }
    }

    // A key of ANOTHER selection, stamped with the TARGET's selection, is still
    // only a lower bound in the page's own chronological order: the stamp
    // decides which page a token belongs to, the key decides where in it the
    // page resumes.
    for target in &selections {
        for other in &selections {
            if other.label == target.label {
                continue;
            }
            let Some(key) = other.ids.first().map(|id| attack_key(&policy_id, id)) else {
                continue;
            };
            let case = format!(
                "a key of {} stamped with {}",
                other.label, target.label
            );
            let (status, body) = call(
                &router,
                "GetProtectionStatus",
                &request(&policy_id, target, None, Some(&target.token(&policy_id, &key))),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: a boundary stamped with this selection is served, got {status} with \
                 {body}"
            );
            let expected: Vec<String> = target
                .ids
                .iter()
                .filter(|id| attack_key(&policy_id, id) >= key)
                .map(|id| (*id).to_string())
                .collect();
            assert_eq!(
                served(&case, &body),
                expected,
                "case {case}: expected the page to resume at the first attack at or after that \
                 key, got {body}"
            );
        }
    }

    // The token the OTHER collection this service narrows by the same policy
    // really mints: a boundary of the accounts that policy applies to, which is
    // no boundary of its attacks however well it decodes.
    let (status, body) = call(
        &router,
        "ListComplianceStatus",
        &json!({ "PolicyId": policy_id, "MaxResults": 1 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "ListComplianceStatus must answer 200 so this suite has its token, got {status} with \
         {body}"
    );
    let compliance_token = next_token(&body).unwrap_or_else(|| {
        panic!("this suite needs a policy of more than one account to mint that token, got {body}")
    });
    for target in &selections {
        let case = format!(
            "a token minted by ListComplianceStatus for the same policy, sent under {}",
            target.label
        );
        refuse(&router, &policy_id, target, &compliance_token, &case).await;
    }
}

/// RULE: the token a selection mints decodes through the same stamp, so walking
/// the chain reports every attack of THAT selection exactly once, in the page's
/// own chronological order, at every page size.
#[tokio::test]
async fn the_token_a_selection_mints_walks_that_selections_whole_page() {
    let (router, _engine, policy_id) = fixture().await;

    // Page sizes differing in whether a page holds one attack, some of them, or
    // the whole selection at the largest page this service serves.
    for max_results in [1usize, 2, state::MAX_PAGE_SIZE] {
        for selection in &selections() {
            let walked = walk(&router, &policy_id, selection, Some(max_results)).await;
            assert_eq!(
                walked,
                selection.ids.to_vec(),
                "case {} / MaxResults {max_results}: walking the minted tokens must report every \
                 attack of that selection exactly once, in the page's order, got {walked:?}",
                selection.label
            );
        }
    }
}

/// RULE, the other side of it: what selects the page is the INSTANT each bound
/// names, not the way the caller spelled it, so the same period written in
/// another valid `TimeStamp` format is the same selection and resumes from its
/// token.
#[tokio::test]
async fn the_same_instant_in_another_timestamp_format_is_the_same_selection() {
    let (router, _engine, policy_id) = fixture().await;
    // The row with both bounds present, so both are re-spelled below.
    let selection = selections()
        .into_iter()
        .find(|selection| selection.start_secs.is_some() && selection.end_secs.is_some())
        .expect("this suite holds a selection naming both bounds");
    let token = first_token(&router, &policy_id, &selection)
        .await
        .expect("that selection's page leaves attacks behind, so it mints a token");

    // The same instants with an explicit zero sub-second part: a number is the
    // documented default for these members ("Unix time in seconds"), and a
    // fractional one names the very same instant.
    let mut replayed = request(&policy_id, &selection, None, Some(&token));
    for (member, offset) in [
        ("StartTime", selection.start_secs),
        ("EndTime", selection.end_secs),
    ] {
        let offset = offset.expect("both bounds are present in this row");
        replayed[member] = json!(format!("{}.000", BASE_SECS + offset)
            .parse::<f64>()
            .expect("a Unix-seconds instant parses as a number"));
    }
    let (status, body) = call(&router, "GetProtectionStatus", &replayed).await;
    assert_eq!(
        status, 200,
        "the same period in another TimeStamp format must resume the page, got {status} with \
         {body}"
    );
    assert_eq!(
        served("the same period re-spelled", &body),
        selection.ids[1..].to_vec(),
        "expected the attacks after the first, got {body}"
    );
}
