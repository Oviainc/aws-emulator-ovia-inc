// SPDX-License-Identifier: CC0-1.0

//! `DeleteScraper`, end to end over HTTP: the deletion of one scraper is accepted only
//! while the stored record is `ACTIVE`, moves that record to `DELETING`, reports the
//! status it just wrote, and lets the clock drop the record — after which `ListScrapers`
//! no longer carries it.
//!
//! The store is seeded directly here, through the same engine the mounted service reads
//! ([`Amp::store`]) and with the same builder the listing suite uses
//! ([`crate::scraper_listing::scraper`]): seeding is how a scraper in an EXACT status
//! exists to delete — and it is the same record the handler reads, not a second copy of
//! it. A deletion of a scraper `CreateScraper` itself made is driven in
//! `crate::scraper_creation`.
//!
//! The deletion is read back through `ListScrapers`, which reports the stored record.

use crate::scraper_listing::{
    dataset_destination, eks_source, listed, scraper, seed, workspace_destination,
};
use crate::workspace_lifecycle::{text, wait_for_due_transition, Amp};
use service_com_amazonaws_amp_amazonprometheusservice::ops::types::{
    Source, VpcConfiguration, SCRAPER_STATUS_CODE_VALUES,
};
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    SCRAPER_NOT_FOUND_MESSAGE, SCRAPER_RESOURCE_TYPE, STATUS_ACTIVE, STATUS_DELETING,
};

/// A VPC source — the `Source` union's other variant, so the rule is exercised on a
/// stored record of either shape.
fn vpc_source() -> Source {
    Source::vpcConfiguration(VpcConfiguration {
        security_group_ids: vec!["sg-0123456789abcdef0".to_string()],
        subnet_ids: vec!["subnet-0123456789abcdef0".to_string()],
    })
}

/// `DELETE /scrapers/{scraperId}`, with the optional `clientToken` query member when one
/// is given.
async fn delete(
    amp: &Amp,
    region: &str,
    scraper_id: &str,
    client_token: Option<&str>,
) -> crate::workspace_lifecycle::Reply {
    let uri = match client_token {
        Some(token) => format!("/scrapers/{scraper_id}?clientToken={token}"),
        None => format!("/scrapers/{scraper_id}"),
    };
    amp.call("DELETE", &uri, region, "").await
}

/// The stored status of one scraper, or `None` once the store no longer holds it.
fn stored_status(amp: &Amp, scraper_id: &str) -> Option<String> {
    amp.store()
        .lock()
        .scrapers
        .get(scraper_id)
        .map(|record| record.status_code.clone())
}

/// Deleting an `ACTIVE` scraper is accepted (modelled 202), reports the id and the
/// `DELETING` status it wrote, and the clock then removes the record — whatever the shape
/// of the scraper being deleted. One row per shape the stored record can take.
#[tokio::test]
async fn deleting_an_active_scraper_moves_it_to_deleting_then_the_clock_removes_it() {
    let region = "us-west-2";
    let cases = vec![
        (
            "an EKS source writing to a workspace, with an alias",
            scraper(
                "s-11111111-1111-1111-1111-111111111111",
                Some("collector"),
                STATUS_ACTIVE,
                eks_source("metrics-cluster"),
                workspace_destination("ws-11111111-1111-1111-1111-111111111111"),
            ),
            Some("delete-token-1"),
        ),
        (
            "a VPC source writing to a CloudWatch dataset, with no alias",
            scraper(
                "s-22222222-2222-2222-2222-222222222222",
                None,
                STATUS_ACTIVE,
                vpc_source(),
                dataset_destination("fleet-metrics"),
            ),
            None,
        ),
        (
            // `ScraperId` is `len 1..64, pattern ^[0-9A-Za-z][-.0-9A-Z_a-z]*$`: an id at
            // the short end of that length is addressed the same way as a minted one.
            "a one-character scraperId",
            scraper(
                "s",
                Some("tiny"),
                STATUS_ACTIVE,
                eks_source("tiny-cluster"),
                workspace_destination("ws-22222222-2222-2222-2222-222222222222"),
            ),
            Some("delete-token-3"),
        ),
    ];

    for (case, seeded, client_token) in cases {
        let amp = Amp::new();
        seed(&amp, &seeded);
        let scraper_id = seeded.scraper_id.as_str();

        let reply = delete(&amp, region, scraper_id, client_token).await;
        assert_eq!(
            reply.status, 202,
            "case {case:?}: DeleteScraper is modelled 202, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["scraperId"]),
            scraper_id,
            "case {case:?}: the response must name the scraper deleted, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["status"]["statusCode"]),
            STATUS_DELETING,
            "case {case:?}: expected the status the call wrote, got {reply:?}"
        );
        assert_eq!(
            stored_status(&amp, scraper_id).as_deref(),
            Some(STATUS_DELETING),
            "case {case:?}: the stored record must have moved to {STATUS_DELETING}, got \
             {:?}",
            stored_status(&amp, scraper_id)
        );

        // While the removal is pending the scraper is still listed, in `DELETING` —
        // "This includes scrapers being created or deleted."
        let pending = amp.call("GET", "/scrapers", region, "").await;
        assert_eq!(
            listed(&pending.body),
            vec![scraper_id.to_string()],
            "case {case:?}: a scraper being deleted is still listed, got {pending:?}"
        );
        assert_eq!(
            text(&pending.body["scrapers"][0]["status"]["statusCode"]),
            STATUS_DELETING,
            "case {case:?}: the listing must report the stored status, got {pending:?}"
        );

        wait_for_due_transition();

        let settled = amp.call("GET", "/scrapers", region, "").await;
        assert_eq!(
            settled.status, 200,
            "case {case:?}: the listing must answer 200, got {settled:?}"
        );
        assert_eq!(
            listed(&settled.body),
            Vec::<String>::new(),
            "case {case:?}: the removal the clock owed must have dropped the scraper, \
             got {settled:?}"
        );
        assert_eq!(
            stored_status(&amp, scraper_id),
            None,
            "case {case:?}: the store must no longer hold the scraper, got {:?}",
            stored_status(&amp, scraper_id)
        );

        // The record is gone, so the very same call is now a not-found rather than a
        // second deletion.
        let repeated = delete(&amp, region, scraper_id, client_token).await;
        assert_eq!(
            repeated.status, 404,
            "case {case:?}: deleting a scraper the store no longer holds must answer \
             404, got {repeated:?}"
        );
    }
}

/// `ScraperStatusCode$DELETING` documents the rule — "Deletions are allowed only when
/// status is ACTIVE" — so every OTHER value of the modelled enum is refused with the
/// modelled `ConflictException` (409) and leaves the stored record where it was. The
/// catalog is the model's own value list
/// (`crate::ops::types::SCRAPER_STATUS_CODE_VALUES`), so a status this test never names
/// by hand is covered too.
#[tokio::test]
async fn a_delete_is_refused_for_every_status_but_active() {
    let region = "us-west-2";

    for status in SCRAPER_STATUS_CODE_VALUES {
        let amp = Amp::new();
        let seeded = scraper(
            "s-33333333-3333-3333-3333-333333333333",
            Some("statuses"),
            status,
            eks_source("status-cluster"),
            workspace_destination("ws-33333333-3333-3333-3333-333333333333"),
        );
        seed(&amp, &seeded);
        let scraper_id = seeded.scraper_id.as_str();

        let reply = delete(&amp, region, scraper_id, None).await;
        if *status == STATUS_ACTIVE {
            assert_eq!(
                reply.status, 202,
                "case {status:?}: an ACTIVE scraper is deletable, got {reply:?}"
            );
            assert_eq!(
                text(&reply.body["status"]["statusCode"]),
                STATUS_DELETING,
                "case {status:?}: expected the status the call wrote, got {reply:?}"
            );
            continue;
        }
        assert_eq!(
            reply.status, 409,
            "case {status:?}: a scraper that is not {STATUS_ACTIVE} must be refused with \
             409, got {reply:?}"
        );
        assert_eq!(
            reply.error_code(),
            "ConflictException",
            "case {status:?}: expected the modelled ConflictException, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["resourceId"]),
            scraper_id,
            "case {status:?}: the refusal must name the scraper, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["resourceType"]),
            SCRAPER_RESOURCE_TYPE,
            "case {status:?}: the refusal must name the resource type, got {reply:?}"
        );
        assert_eq!(
            stored_status(&amp, scraper_id).as_deref(),
            Some(*status),
            "case {status:?}: a refused delete must change nothing, got {:?}",
            stored_status(&amp, scraper_id)
        );
    }
}

/// A `scraperId` the store does not hold is the modelled `ResourceNotFoundException`
/// (404), whatever the id looks like — and a scraper the store DOES hold is untouched by
/// a call that names another.
#[tokio::test]
async fn a_scraper_the_store_does_not_hold_is_not_found() {
    let region = "us-west-2";
    let amp = Amp::new();
    let seeded = scraper(
        "s-44444444-4444-4444-4444-444444444444",
        Some("bystander"),
        STATUS_ACTIVE,
        eks_source("bystander-cluster"),
        workspace_destination("ws-44444444-4444-4444-4444-444444444444"),
    );
    seed(&amp, &seeded);

    let absent = [
        ("a minted-looking id nothing holds", "s-99999999-9999-9999-9999-999999999999"),
        ("a one-character id", "z"),
        ("an id of the maximum modelled length", &"s".repeat(64)),
        ("an id that differs only in case", "S-44444444-4444-4444-4444-444444444444"),
    ];

    for (case, scraper_id) in absent {
        let reply = delete(&amp, region, scraper_id, None).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: an absent scraper must answer 404, got {reply:?}"
        );
        assert_eq!(
            reply.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected the modelled ResourceNotFoundException, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["message"]),
            SCRAPER_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the service's not-found sentence, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["resourceId"]),
            scraper_id,
            "case {case:?}: the refusal must name the id asked for, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["resourceType"]),
            SCRAPER_RESOURCE_TYPE,
            "case {case:?}: the refusal must name the resource type, got {reply:?}"
        );
    }

    assert_eq!(
        stored_status(&amp, &seeded.scraper_id).as_deref(),
        Some(STATUS_ACTIVE),
        "a delete that named another scraper must leave this one {STATUS_ACTIVE}, got {:?}",
        stored_status(&amp, &seeded.scraper_id)
    );
}

/// `clientToken` is `@idempotencyToken` and the operation is `@idempotent`: replaying the
/// very call that started the pending deletion is that same call, so it is accepted again,
/// answers from the record that call wrote and does NOT push the owed removal out. A
/// repeat that is not that call — no token, or another token — is a second deletion of a
/// scraper that is no longer `ACTIVE`, which the status rule refuses.
#[tokio::test]
async fn replaying_the_delete_with_its_client_token_is_the_same_request() {
    let region = "us-west-2";
    let scraper_id = "s-55555555-5555-5555-5555-555555555555";
    let token = "idem-55555555";

    let cases: Vec<(&str, Option<&str>, u16)> = vec![
        ("the same clientToken", Some(token), 202),
        ("no clientToken at all", None, 409),
        ("a different clientToken", Some("idem-other"), 409),
    ];

    for (case, repeat_token, expected_status) in cases {
        let amp = Amp::new();
        seed(
            &amp,
            &scraper(
                scraper_id,
                Some("idempotent"),
                STATUS_ACTIVE,
                eks_source("idempotent-cluster"),
                workspace_destination("ws-55555555-5555-5555-5555-555555555555"),
            ),
        );

        let first = delete(&amp, region, scraper_id, Some(token)).await;
        assert_eq!(
            first.status, 202,
            "case {case:?}: the first delete must be accepted, got {first:?}"
        );
        let owed = amp
            .store()
            .lock()
            .scrapers
            .get(scraper_id)
            .and_then(|record| record.transition_due);
        assert!(
            owed.is_some(),
            "case {case:?}: the first delete must leave a removal owed, got {owed:?}"
        );

        let repeat = delete(&amp, region, scraper_id, repeat_token).await;
        assert_eq!(
            repeat.status, expected_status,
            "case {case:?}: expected status {expected_status}, got {repeat:?}"
        );
        if expected_status == 202 {
            assert_eq!(
                text(&repeat.body["status"]["statusCode"]),
                STATUS_DELETING,
                "case {case:?}: the replay must answer from the stored record, got {repeat:?}"
            );
        } else {
            assert_eq!(
                repeat.error_code(),
                "ConflictException",
                "case {case:?}: expected the modelled ConflictException, got {repeat:?}"
            );
        }
        let still_owed = amp
            .store()
            .lock()
            .scrapers
            .get(scraper_id)
            .and_then(|record| record.transition_due);
        assert_eq!(
            still_owed, owed,
            "case {case:?}: the repeat must not move the instant the removal is owed, \
             expected {owed:?}, got {still_owed:?}"
        );
    }
}

/// Deleting one scraper is a change to that record only: every other scraper the account
/// holds keeps its status and stays listed, and the one deleted leaves the others behind
/// when the clock drops it.
#[tokio::test]
async fn a_delete_touches_only_the_scraper_it_names() {
    let region = "us-west-2";
    let amp = Amp::new();
    let ids = [
        "s-aaaa1111-1111-1111-1111-111111111111",
        "s-bbbb2222-2222-2222-2222-222222222222",
        "s-cccc3333-3333-3333-3333-333333333333",
    ];
    for id in ids {
        seed(
            &amp,
            &scraper(
                id,
                Some("fleet"),
                STATUS_ACTIVE,
                eks_source("fleet-cluster"),
                workspace_destination("ws-66666666-6666-6666-6666-666666666666"),
            ),
        );
    }

    let reply = delete(&amp, region, ids[1], None).await;
    assert_eq!(
        reply.status, 202,
        "case {:?}: the delete must be accepted, got {reply:?}",
        ids[1]
    );
    for id in ids {
        let expected = if id == ids[1] {
            STATUS_DELETING
        } else {
            STATUS_ACTIVE
        };
        assert_eq!(
            stored_status(&amp, id).as_deref(),
            Some(expected),
            "case {id:?}: expected the stored status {expected:?}, got {:?}",
            stored_status(&amp, id)
        );
    }

    wait_for_due_transition();

    let settled = amp.call("GET", "/scrapers", region, "").await;
    assert_eq!(
        listed(&settled.body),
        vec![ids[0].to_string(), ids[2].to_string()],
        "only the deleted scraper may be dropped, got {settled:?}"
    );
}
