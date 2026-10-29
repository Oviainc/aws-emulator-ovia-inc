// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for WHICH POSITIONS of the identifier space `ListRules`
//! accepts a page cursor at.
//!
//! The rule asserted here: a cursor resumes a walk only at a position this endpoint
//! really handed out, so the positions it honours are exactly the ones `CreateRule` has
//! minted so far; every other value the `RuleIdentifier` pattern admits — one ABOVE the
//! last position minted, and one BELOW the first, including the lowest value the pattern
//! admits, which no mint can ever occupy — is refused with the modelled
//! `ValidationException`, HTTP 400 and `Reason=INVALID_PAGE_TOKEN`, never answered with a
//! page that silently restarts or truncates the walk.
//!
//! The tables span the dimension the rule generalises over — WHERE in the identifier
//! ordering a cursor's position falls: the lowest and the highest value the pattern
//! admits, every position the store minted, and the positions one step below and one
//! step above each of those — crossed with how many positions the store has minted at
//! all (none, one, several) and with the Region the request is signed for.
//!
//! Every case drives the service through the real router, exactly as a signed SDK
//! request would, and states its own case so a failure names the input.

use crate::list_rules_page_token_authenticity::{
    assert_refused, create_rules_in, page_in, REGIONS,
};
use crate::retention_rule_lifecycle::{router, RESOURCE_TYPES};
use service_com_amazonaws_rbin_amazonrecyclebin::state::encode_page_token;

/// The characters `RuleIdentifier` admits, in ascending BYTE order — which is the order
/// a page cursor's position is compared in.
/// Source: smithy_query(query="shape", shape="RuleIdentifier") — pattern
/// `^[0-9a-zA-Z]{11}$`.
const IDENTIFIER_DIGITS: &[u8; 62] =
    b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// `RuleIdentifier` is exactly 11 of those characters. Same source.
const IDENTIFIER_LEN: usize = 11;

/// How many rules the store has minted, per row of the tables: nothing at all (a
/// brand-new store), a single position, and several.
const MINTED_COUNTS: [usize; 3] = [0, 1, 3];

/// The rules a resume table needs so there is a position before, between and after the
/// ones the store minted.
const MINTED_RULES: usize = 3;

/// The lowest value the identifier pattern admits: [`IDENTIFIER_LEN`] copies of the
/// first digit of [`IDENTIFIER_DIGITS`]. Nothing sorts below it, so a cursor naming it
/// could only ever exclude nothing.
fn lowest_identifier() -> String {
    std::iter::repeat_n(IDENTIFIER_DIGITS[0] as char, IDENTIFIER_LEN).collect()
}

/// The highest value the identifier pattern admits — [`IDENTIFIER_LEN`] copies of the
/// last digit of [`IDENTIFIER_DIGITS`]. Everything sorts below it, so a cursor naming it
/// could only ever exclude every rule.
fn highest_identifier() -> String {
    std::iter::repeat_n(IDENTIFIER_DIGITS[IDENTIFIER_DIGITS.len() - 1] as char, IDENTIFIER_LEN)
        .collect()
}

/// The value one position away from `identifier` in the identifier ordering: the next
/// one up when `up`, the previous one otherwise. `None` at the end of the space the
/// pattern admits, which has no neighbour on that side.
fn step(identifier: &str, up: bool) -> Option<String> {
    let last = IDENTIFIER_DIGITS.len() - 1;
    let mut digits: Vec<u8> = identifier.bytes().collect();
    for index in (0..digits.len()).rev() {
        let position = IDENTIFIER_DIGITS
            .iter()
            .position(|digit| *digit == digits[index])?;
        let stepped = if up {
            Some(position + 1).filter(|next| *next <= last)
        } else {
            position.checked_sub(1)
        };
        match stepped {
            Some(next) => {
                digits[index] = IDENTIFIER_DIGITS[next];
                return String::from_utf8(digits).ok();
            }
            // This digit is already at the end of the alphabet the step runs toward: it
            // rolls over and the step carries into the digit to its left.
            None => digits[index] = IDENTIFIER_DIGITS[if up { 0 } else { last }],
        }
    }
    None
}

/// Whether `candidate` is a value the identifier pattern admits, so a row of the tables
/// really exercises a position the model allows a cursor to name.
fn is_identifier_shaped(candidate: &str) -> bool {
    candidate.len() == IDENTIFIER_LEN
        && candidate
            .bytes()
            .all(|byte| IDENTIFIER_DIGITS.contains(&byte))
}

#[tokio::test]
async fn list_rules_refuses_every_position_it_has_not_minted() {
    for (row, region) in REGIONS.iter().enumerate() {
        let resource_type = RESOURCE_TYPES[row % RESOURCE_TYPES.len()];
        for minted in MINTED_COUNTS {
            let router = router();
            let setup = format!("{minted} rules retaining {resource_type} in {region}");
            let created = create_rules_in(&router, region, resource_type, minted, &setup).await;

            // Where a cursor's position can fall relative to the ones the store minted:
            // at the bottom of the space (the lowest value the pattern admits — the one
            // no mint can occupy), at its top, and one step below or above each minted
            // position.
            let mut candidates: Vec<(String, String)> = vec![
                (
                    "the lowest position the identifier pattern admits".to_string(),
                    lowest_identifier(),
                ),
                (
                    "the highest position the identifier pattern admits".to_string(),
                    highest_identifier(),
                ),
            ];
            for identifier in &created {
                if let Some(below) = step(identifier, false) {
                    candidates.push((format!("one position below the minted {identifier:?}"), below));
                }
                if let Some(above) = step(identifier, true) {
                    candidates.push((format!("one position above the minted {identifier:?}"), above));
                }
            }

            for (case, identifier) in candidates {
                // A neighbour of one minted position can be another minted position;
                // those are what the resume table below covers.
                if created.contains(&identifier) {
                    continue;
                }
                let case = format!("{case}, with {setup}");
                assert!(
                    is_identifier_shaped(&identifier),
                    "case {case:?}: the row must name a value the identifier pattern \
                     admits, got {identifier:?}"
                );
                assert_refused(
                    &router,
                    region,
                    resource_type,
                    &encode_page_token(region, &identifier),
                    &case,
                )
                .await;
            }

            // Every refusal above left the store alone: the Region's own walk still
            // reports each rule it holds, in creation order, in one page.
            let case = format!("the whole walk, with {setup}");
            let (reported, token) = page_in(&router, region, resource_type, None, None, &case).await;
            assert_eq!(
                reported, created,
                "case {case:?}: the walk must report every rule of the Region, got {reported:?}"
            );
            assert_eq!(
                token, None,
                "case {case:?}: a page holding every rule hands back no cursor, got {token:?}"
            );
        }
    }
}

#[tokio::test]
async fn every_position_the_endpoint_minted_resumes_the_walk_after_it() {
    // The other half of the rule: refusing the positions it never handed out must not
    // cost it one it did — a cursor at any minted position still resumes right after it.
    for (row, region) in REGIONS.iter().enumerate() {
        let resource_type = RESOURCE_TYPES[row % RESOURCE_TYPES.len()];
        let router = router();
        let setup = format!("{MINTED_RULES} rules retaining {resource_type} in {region}");
        let created = create_rules_in(&router, region, resource_type, MINTED_RULES, &setup).await;

        for (nth, identifier) in created.iter().enumerate() {
            let case = format!("a cursor at the minted position #{nth} ({identifier:?}), {setup}");
            let (reported, _token) = page_in(
                &router,
                region,
                resource_type,
                Some(MINTED_RULES),
                Some(&encode_page_token(region, identifier)),
                &case,
            )
            .await;
            assert_eq!(
                reported,
                created[nth + 1..].to_vec(),
                "case {case:?}: the walk must resume right after the cursor, got {reported:?}"
            );
        }
    }
}
