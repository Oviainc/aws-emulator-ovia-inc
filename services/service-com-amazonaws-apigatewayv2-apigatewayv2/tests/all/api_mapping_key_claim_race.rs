// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the base-path claim an ApiGatewayV2 `UpdateApiMapping`
//! makes, reached CONCURRENTLY.
//!
//! Every test drives the real `restJson1` request path — `MultiServiceRouter::dispatch`
//! over a signed `http::Request`, from several tasks of a multi-threaded runtime — so
//! what is asserted is the wire behaviour concurrent clients see.
//!
//! The rule asserted here, over a TABLE rather than over one pair of requests: **one
//! base path of one custom domain name is held by at most one api mapping at any
//! instant**, so of any number of `UpdateApiMapping` calls that would move DIFFERENT
//! mappings of the same domain name onto the same base path at the same time, at most one
//! is applied (HTTP 200, the response reporting the record a later `GetApiMapping`
//! reports) and every other is refused HTTP 409 `ConflictException` with the base-path
//! conflict message, leaving the refused caller's stored `ApiMappingKey` exactly what it
//! was. Exactly one is applied when the base path was free; none is when a mapping that
//! does not race already holds it.
//!
//! The rows vary what the rule generalises over: how many callers race (a pair and
//! several), the form of the contested base path (one segment, several segments, the root
//! of the domain, every documented punctuation character), and whether the contested base
//! path was already occupied before the race.

use std::sync::Arc;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    API_MAPPING_KEY_CONFLICT, API_MAPPING_KEY_PUNCTUATION,
};

use crate::api_lifecycle::{as_str, emulator, error_code};
use crate::api_mapping_lifecycle::{
    create_api_mapping, get_api_mapping, seed_api, seed_domain_name, OTHER_STAGE, STAGE,
};
use crate::api_mapping_update::{body, update_api_mapping};

/// One row of the claim-race table.
struct RaceCase {
    label: &'static str,
    /// How many mappings of one domain name are patched onto [`RaceCase::contested`] at
    /// the same instant, each from its own task.
    racers: usize,
    /// The base path every racer claims. The empty string is the root of the domain —
    /// the base path an absent `ApiMappingKey` also names.
    contested: String,
    /// Whether a mapping that does NOT race already occupies the contested base path, in
    /// which case every racer is a second claim on it and none may be applied.
    pre_held: bool,
}

impl RaceCase {
    /// How many of this row's racers the rule lets through: the contested base path takes
    /// one holder, so a race for a free path has one winner and a race for one already
    /// held has none.
    fn expected_winners(&self) -> usize {
        usize::from(!self.pre_held)
    }
}

/// The rows span the racer counts and the base-path forms the rule generalises over. The
/// punctuation row builds its key from the crate's own documented class rather than from
/// a copy written here, so it is a row about the class.
fn race_cases() -> Vec<RaceCase> {
    vec![
        RaceCase {
            label: "a pair of racers, one segment",
            racers: 2,
            contested: "shared".to_string(),
            pre_held: false,
        },
        RaceCase {
            label: "five racers, one segment",
            racers: 5,
            contested: "shared".to_string(),
            pre_held: false,
        },
        RaceCase {
            label: "five racers, several segments",
            racers: 5,
            contested: "orders/v1/items".to_string(),
            pre_held: false,
        },
        RaceCase {
            label: "three racers, the root of the domain",
            racers: 3,
            contested: String::new(),
            pre_held: false,
        },
        RaceCase {
            label: "four racers, every documented punctuation character",
            racers: 4,
            contested: format!("a{API_MAPPING_KEY_PUNCTUATION}z"),
            pre_held: false,
        },
        RaceCase {
            label: "six racers for a base path a non-racing mapping already holds",
            racers: 6,
            contested: "taken".to_string(),
            pre_held: true,
        },
    ]
}

/// The mapping one racer owns: the identifier its patch addresses and the base path it
/// was created on, which a refused patch must leave untouched.
#[derive(Clone)]
struct Racer {
    api_mapping_id: String,
    created_key: String,
}

/// Create `racers` mappings of `domain`, each on a base path of its own, and answer them
/// in racer order.
async fn seed_racers(
    emulator: &MultiServiceRouter,
    domain: &str,
    api_id: &str,
    case: &str,
    racers: usize,
) -> Vec<Racer> {
    let mut seeded = Vec::with_capacity(racers);
    for racer in 0..racers {
        let created_key = format!("racer-{racer}");
        // The stage alternates, so no racer's record is a copy of another's and a patch
        // that carries no `Stage` is seen to keep the one the mapping already routes on.
        let stage = if racer % 2 == 0 { STAGE } else { OTHER_STAGE };
        let (status, created) = create_api_mapping(
            emulator,
            domain,
            &body(api_id, Some(&created_key), Some(stage)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?} racer {racer}: expected 200 from CreateApiMapping, got {status} {created}"
        );
        seeded.push(Racer {
            api_mapping_id: as_str(&created, "apiMappingId"),
            created_key,
        });
    }
    seeded
}

/// RULE, reached concurrently: at most one of N simultaneous `UpdateApiMapping` calls
/// claiming one base path of one domain name is applied; every other is refused the
/// modelled 409 `ConflictException` and keeps the base path it already held.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_claims_of_one_base_path_leave_at_most_one_holder() {
    let emulator = Arc::new(emulator());
    // One api, holding both stages every racer routes to: the race is about the base path
    // the mappings claim, not about what they route to.
    let api_id = seed_api(&emulator, "racing-api", &[STAGE, OTHER_STAGE]).await;

    for (row, case) in race_cases().into_iter().enumerate() {
        let label = case.label;
        // A domain name of its own per row, so one row's mappings are never a holder in
        // another's race.
        let domain = format!("race-{row}.example.com");
        seed_domain_name(&emulator, &domain).await;
        let racers = seed_racers(&emulator, &domain, &api_id, label, case.racers).await;

        if case.pre_held {
            let (status, holder) = create_api_mapping(
                &emulator,
                &domain,
                &body(&api_id, Some(&case.contested), Some(STAGE)),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected the non-racing holder to be created, got {status} {holder}"
            );
        }

        // Every racer patches ITS OWN mapping onto the contested base path at the same
        // time. The patch carries no `Stage`, so nothing but the base path is claimed.
        let mut pending = Vec::with_capacity(case.racers);
        for (racer, seeded) in racers.iter().enumerate() {
            let emulator = Arc::clone(&emulator);
            let domain = domain.clone();
            let api_mapping_id = seeded.api_mapping_id.clone();
            let request = body(&api_id, Some(&case.contested), None);
            pending.push(tokio::spawn(async move {
                let answer = update_api_mapping(&emulator, &domain, &api_mapping_id, &request).await;
                (racer, answer)
            }));
        }

        let mut winners: Vec<usize> = Vec::new();
        for task in pending {
            let (racer, (status, answer)) = task.await.expect("the racing patch answered");
            match status {
                200 => {
                    assert_eq!(
                        as_str(&answer, "apiMappingId"),
                        racers[racer].api_mapping_id,
                        "case {label:?} racer {racer}: a winner reports the mapping it \
                         addressed, got {answer}"
                    );
                    assert_eq!(
                        as_str(&answer, "apiMappingKey"),
                        case.contested,
                        "case {label:?} racer {racer}: a winner reports the contested base \
                         path, got {answer}"
                    );
                    winners.push(racer);
                }
                409 => {
                    assert_eq!(
                        error_code(&answer),
                        "ConflictException",
                        "case {label:?} racer {racer}: expected ConflictException, got {answer}"
                    );
                    assert_eq!(
                        as_str(&answer, "message"),
                        API_MAPPING_KEY_CONFLICT,
                        "case {label:?} racer {racer}: expected the base-path conflict \
                         message, got {answer}"
                    );
                }
                status => panic!(
                    "case {label:?} racer {racer}: expected 200 or 409, got {status} {answer}"
                ),
            }
        }

        assert_eq!(
            winners.len(),
            case.expected_winners(),
            "case {label:?}: expected {} of {} racers to be applied, got {winners:?}",
            case.expected_winners(),
            case.racers
        );

        // What the store holds after the race: the winner — when the rule allowed one —
        // on the contested base path, and every refused racer still on the base path it
        // was created with.
        let mut holders_of_contested = 0usize;
        for (racer, seeded) in racers.iter().enumerate() {
            let (status, read) = get_api_mapping(&emulator, &domain, &seeded.api_mapping_id).await;
            assert_eq!(
                status, 200,
                "case {label:?} racer {racer}: expected GetApiMapping to report the \
                 mapping, got {status} {read}"
            );
            let stored_key = as_str(&read, "apiMappingKey");
            if winners.contains(&racer) {
                assert_eq!(
                    stored_key, case.contested,
                    "case {label:?} racer {racer}: expected the winner stored on the \
                     contested base path, got {read}"
                );
                holders_of_contested += 1;
            } else {
                assert_eq!(
                    stored_key, seeded.created_key,
                    "case {label:?} racer {racer}: expected a refused patch to have left \
                     the base path it was created on, got {read}"
                );
            }
        }
        assert_eq!(
            holders_of_contested,
            case.expected_winners(),
            "case {label:?}: expected {} racer(s) holding the contested base path after \
             the race, got {holders_of_contested}",
            case.expected_winners()
        );
    }
}

/// RULE, reached twice over: a base path a racer WON is no longer free, so a later claim
/// on it — concurrent or not — is refused exactly as the losers of the race were. This
/// pins that the winner's claim is persisted rather than only reported.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_base_path_a_race_settled_on_is_held_against_later_claims() {
    let emulator = Arc::new(emulator());
    let api_id = seed_api(&emulator, "settled-api", &[STAGE, OTHER_STAGE]).await;
    let domain = "settled.example.com";
    seed_domain_name(&emulator, domain).await;
    let contested = "settled";
    let racers = seed_racers(&emulator, domain, &api_id, "the settled race", 3).await;

    let mut pending = Vec::with_capacity(racers.len());
    for (racer, seeded) in racers.iter().enumerate() {
        let emulator = Arc::clone(&emulator);
        let api_mapping_id = seeded.api_mapping_id.clone();
        let request = body(&api_id, Some(contested), None);
        pending.push(tokio::spawn(async move {
            let answer = update_api_mapping(&emulator, domain, &api_mapping_id, &request).await;
            (racer, answer)
        }));
    }

    let mut winner: Option<usize> = None;
    for task in pending {
        let (racer, (status, answer)) = task.await.expect("the racing patch answered");
        if status == 200 {
            assert!(
                winner.is_none(),
                "racer {racer}: a second racer was applied for one base path, got {answer}"
            );
            winner = Some(racer);
        }
    }
    let winner = winner.expect("the race for a free base path has one winner");

    // Every racer that lost, tried again in its own time: the base path is held now, so
    // the refusal is the same one the race gave it.
    for (racer, seeded) in racers.iter().enumerate() {
        if racer == winner {
            continue;
        }
        let (status, refused) = update_api_mapping(
            &emulator,
            domain,
            &seeded.api_mapping_id,
            &body(&api_id, Some(contested), None),
        )
        .await;
        assert_eq!(
            status, 409,
            "racer {racer} retrying after the race: expected 409, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "racer {racer} retrying after the race: expected ConflictException, got {refused}"
        );
    }

    // And the winner may claim the base path it already holds: that is no second claim.
    let (status, again) = update_api_mapping(
        &emulator,
        domain,
        &racers[winner].api_mapping_id,
        &body(&api_id, Some(contested), None),
    )
    .await;
    assert_eq!(
        status, 200,
        "the winner re-claiming the base path it holds: expected 200, got {status} {again}"
    );
    assert_eq!(
        as_str(&again, "apiMappingKey"),
        contested,
        "the winner re-claiming the base path it holds: expected it kept, got {again}"
    );

    let (status, read): (u16, Value) =
        get_api_mapping(&emulator, domain, &racers[winner].api_mapping_id).await;
    assert_eq!(
        status, 200,
        "expected GetApiMapping to report the winner, got {status} {read}"
    );
    assert_eq!(
        read, again,
        "expected GetApiMapping to report what the winner was told, got {read}"
    );
}
