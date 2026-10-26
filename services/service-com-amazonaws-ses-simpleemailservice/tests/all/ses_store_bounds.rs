// SPDX-License-Identifier: CC0-1.0

//! The bounds on the caller-driven stores, and the quota window they share.
//!
//! Every request reaching this emulator can grow one of these stores, so each has
//! a ceiling: the identity list at `MAX_IDENTITIES`, the configuration sets at
//! `MAX_CONFIGURATION_SETS`, the send log at
//! `MAX_SENT_LOG_ENTRIES` with the log's retention applied on append, and the text
//! one log entry keeps at `MAX_LOGGED_TEXT`. What is asserted here is the RULE
//! each bound follows over a SET of sizes — at the ceiling, one past it, and well
//! inside it — reached through the same `state` API the handlers use, because the
//! identity ceiling is 10,000 entries and asserting it over the wire would say
//! nothing the store does not already decide.

use service_com_amazonaws_ses_simpleemailservice::state::{
    logged_text, ComAmazonawsSesSimpleemailserviceState, IdentityType, SentMessage,
    MAX_CONFIGURATION_SETS, MAX_IDENTITIES, MAX_LOGGED_TEXT, MAX_SENT_LOG_ENTRIES,
    SEND_LOG_RETENTION, SEND_QUOTA_WINDOW,
};
use std::time::{Duration, SystemTime};

/// One send-log entry with `recipients` recipients, accepted at `sent_at`.
fn message(index: usize, recipients: usize, sent_at: SystemTime) -> SentMessage {
    SentMessage {
        message_id: format!("id-{index}"),
        source: "sender@example.com".to_string(),
        destinations: (0..recipients)
            .map(|at| format!("rcpt-{at}@example.com"))
            .collect(),
        subject: None,
        body: None,
        configuration_set: None,
        tags: Vec::new(),
        sent_at,
    }
}

#[tokio::test]
async fn the_identity_list_stops_at_the_accounts_limit() {
    let mut state = ComAmazonawsSesSimpleemailserviceState::default();
    // Fill the list to exactly the ceiling, alternating the two identity classes
    // so the bound is not a property of one of them.
    for index in 0..MAX_IDENTITIES {
        let (name, identity_type) = if index % 2 == 0 {
            (
                format!("user-{index}@example.com"),
                IdentityType::EmailAddress,
            )
        } else {
            (format!("d{index}.example.com"), IdentityType::Domain)
        };
        assert!(
            state.verify_identity(&name, identity_type).is_some(),
            "case identity {index}: a verification inside the limit must be recorded"
        );
    }
    assert_eq!(
        state.identities.len(),
        MAX_IDENTITIES,
        "case full list: expected {MAX_IDENTITIES} identities, got {}",
        state.identities.len()
    );

    // One past the ceiling is refused, for either class, and records nothing.
    for (name, identity_type) in [
        ("one-too-many@example.com", IdentityType::EmailAddress),
        ("one-too-many.example.com", IdentityType::Domain),
    ] {
        assert!(
            state.verify_identity(name, identity_type).is_none(),
            "case {name:?}: a verification past the limit must be refused"
        );
        assert!(
            !state.identities.contains_key(name),
            "case {name:?}: a refused verification must record nothing"
        );
    }

    // Re-verifying an identity already in the list adds nothing, so it still
    // succeeds at the ceiling — and the record it returns is the stored one.
    let existing = "user-0@example.com";
    let before = state.identities.get(existing).cloned();
    let after = state
        .verify_identity(existing, IdentityType::EmailAddress)
        .cloned();
    assert!(
        after.is_some(),
        "case {existing:?}: re-verifying an existing identity must succeed at the limit"
    );
    assert_eq!(
        after.map(|identity| identity.verified_at),
        before.map(|identity| identity.verified_at),
        "case {existing:?}: re-verification must leave the stored record alone"
    );
    assert_eq!(
        state.identities.len(),
        MAX_IDENTITIES,
        "case re-verification at the limit: the list must not grow, got {}",
        state.identities.len()
    );
}

#[tokio::test]
async fn the_configuration_sets_stop_at_the_accounts_limit() {
    let mut state = ComAmazonawsSesSimpleemailserviceState::default();
    // Filled to exactly the ceiling, over names of differing form, since the
    // bound is on the COUNT and not on any one name.
    for index in 0..MAX_CONFIGURATION_SETS {
        let name = format!("config-set-{index}");
        let recorded = state.add_configuration_set(&name);
        assert!(
            recorded.is_some_and(|set| set.sending_enabled),
            "case configuration set {index}: a create inside the limit must be recorded, \
             sending enabled"
        );
    }
    assert_eq!(
        state.configuration_sets.len(),
        MAX_CONFIGURATION_SETS,
        "case full store: expected {MAX_CONFIGURATION_SETS} sets, got {}",
        state.configuration_sets.len()
    );

    // One past the ceiling is refused, and records nothing.
    for name in ["one-too-many", "ONE_TOO_MANY-2"] {
        assert!(
            state.add_configuration_set(name).is_none(),
            "case {name:?}: a create past the limit must be refused"
        );
        assert!(
            !state.configuration_sets.contains_key(name),
            "case {name:?}: a refused create must record nothing"
        );
    }

    // A name already in the store adds nothing, so it still succeeds at the
    // ceiling and leaves the stored record alone.
    let existing = "config-set-0";
    state
        .configuration_sets
        .get_mut(existing)
        .expect("the set was recorded above")
        .sending_enabled = false;
    assert_eq!(
        state
            .add_configuration_set(existing)
            .map(|set| set.sending_enabled),
        Some(false),
        "case {existing:?}: an existing set keeps its stored sending state"
    );
    assert_eq!(
        state.configuration_sets.len(),
        MAX_CONFIGURATION_SETS,
        "case existing name at the limit: the store must not grow, got {}",
        state.configuration_sets.len()
    );
}

#[tokio::test]
async fn the_send_log_keeps_the_newest_entries_up_to_its_backstop() {
    let mut state = ComAmazonawsSesSimpleemailserviceState::default();
    let now = SystemTime::now();
    // Sizes spanning the bound: well inside it, exactly at it, and past it.
    for appended in [1_usize, MAX_SENT_LOG_ENTRIES / 2, MAX_SENT_LOG_ENTRIES] {
        let mut state = ComAmazonawsSesSimpleemailserviceState::default();
        for index in 0..appended {
            state.log_sent_message(message(index, 1, now));
        }
        assert_eq!(
            state.sent_messages.len(),
            appended,
            "case {appended} appended: expected every entry kept, got {}",
            state.sent_messages.len()
        );
    }

    let appended = MAX_SENT_LOG_ENTRIES + 7;
    for index in 0..appended {
        state.log_sent_message(message(index, 1, now));
    }
    assert_eq!(
        state.sent_messages.len(),
        MAX_SENT_LOG_ENTRIES,
        "case {appended} appended: the log is bounded at {MAX_SENT_LOG_ENTRIES}, got {}",
        state.sent_messages.len()
    );
    // The entries dropped are the OLDEST: the log is the trailing record of what
    // was accepted, so the newest append is always in it.
    let kept: Vec<&str> = state
        .sent_messages
        .iter()
        .map(|logged| logged.message_id.as_str())
        .collect();
    assert_eq!(
        kept.first().copied(),
        Some(format!("id-{}", appended - MAX_SENT_LOG_ENTRIES).as_str()),
        "case {appended} appended: expected the oldest entries dropped, got {:?}",
        kept.first()
    );
    assert_eq!(
        kept.last().copied(),
        Some(format!("id-{}", appended - 1).as_str()),
        "case {appended} appended: the newest entry must be kept, got {:?}",
        kept.last()
    );
}

#[tokio::test]
async fn the_quota_window_drops_what_has_aged_out() {
    let now = SystemTime::now();
    // Each row is an age for one two-recipient message, on both sides of the
    // window and on its boundary.
    let rows: &[(&str, Duration, f64)] = &[
        ("just accepted", Duration::ZERO, 2.0),
        ("an hour old", Duration::from_secs(3_600), 2.0),
        (
            "a second inside the window",
            SEND_QUOTA_WINDOW - Duration::from_secs(1),
            2.0,
        ),
        ("exactly the window", SEND_QUOTA_WINDOW, 0.0),
        (
            "a second past the window",
            SEND_QUOTA_WINDOW + Duration::from_secs(1),
            0.0,
        ),
        ("a week old", Duration::from_secs(7 * 24 * 3_600), 0.0),
    ];
    for (case, age, expected) in rows {
        let mut state = ComAmazonawsSesSimpleemailserviceState::default();
        let sent_at = now
            .checked_sub(*age)
            .expect("the test clock stays after the epoch");
        state.log_sent_message(message(0, 2, sent_at));
        let counted = state.sent_last_24_hours(now);
        assert_eq!(
            counted, *expected,
            "case {case}: expected {expected} recipients counted, got {counted}"
        );
    }

    // An append also materialises the expiry the clock has already made due, so
    // the store does not keep what NO reader counts. That expiry is the log's
    // retention, which is longer than this quota window: an entry the quota has
    // stopped counting is still part of the sending statistics, so it stays in
    // the log until it leaves the retention too.
    let mut state = ComAmazonawsSesSimpleemailserviceState::default();
    state.log_sent_message(message(
        0,
        1,
        now.checked_sub(SEND_QUOTA_WINDOW + Duration::from_secs(60))
            .expect("the test clock stays after the epoch"),
    ));
    state.log_sent_message(message(1, 1, now));
    assert_eq!(
        state.sent_messages.len(),
        2,
        "case append past the quota window: the entry is kept for the longer report, got {:?}",
        state.sent_messages
    );
    assert_eq!(
        state.sent_last_24_hours(now),
        1.0,
        "case append past the quota window: only the entry inside the window counts"
    );

    // Past the retention, the append drops it: nothing reports it any more.
    let mut state = ComAmazonawsSesSimpleemailserviceState::default();
    state.log_sent_message(message(
        0,
        1,
        now.checked_sub(SEND_LOG_RETENTION + Duration::from_secs(60))
            .expect("the test clock stays after the epoch"),
    ));
    state.log_sent_message(message(1, 1, now));
    assert_eq!(
        state.sent_messages.len(),
        1,
        "case append past the retention: the aged-out entry is dropped, got {:?}",
        state.sent_messages
    );
    assert_eq!(
        state.sent_last_24_hours(now),
        1.0,
        "case append past the retention: only the entry inside the window counts"
    );
}

#[tokio::test]
async fn every_minted_id_is_fresh_opaque_and_ses_shaped() {
    let mut state = ComAmazonawsSesSimpleemailserviceState::default();
    // Minted in bulk, because what matters is that no two ids from one store
    // ever collide — the counter each id digests is what guarantees it.
    const MINTED: usize = 5_000;
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for index in 0..MINTED {
        let message_id = state.new_message_id();
        // `0100<12 hex>-<8-4-4-4-12 hex>-000000`, the shape SES answers
        // `SendEmail` with.
        assert!(
            message_id.starts_with("0100") && message_id.ends_with("-000000"),
            "case message id {index}: expected an SES-shaped id, got {message_id:?}"
        );
        let hex_only = message_id.replace(['-'], "");
        assert!(
            hex_only.chars().all(|c| c.is_ascii_hexdigit()),
            "case message id {index}: expected hex characters only, got {message_id:?}"
        );
        assert!(
            seen.insert(message_id.clone()),
            "case message id {index}: expected a fresh id, got {message_id:?} again"
        );

        let token = state.new_verification_token();
        // 44 characters, the length SES's own domain-verification token carries.
        assert_eq!(
            token.len(),
            44,
            "case token {index}: expected 44 characters, got {token:?}"
        );
        assert!(
            token.chars().all(|c| c.is_ascii_hexdigit()),
            "case token {index}: expected hex characters only, got {token:?}"
        );
        assert!(
            seen.insert(token.clone()),
            "case token {index}: expected a fresh token, got {token:?} again"
        );
    }
    assert_eq!(
        seen.len(),
        MINTED * 2,
        "case {MINTED} mints: every id must be distinct, got {} distinct",
        seen.len()
    );
}

#[tokio::test]
async fn logged_message_text_is_capped_without_splitting_a_character() {
    // Lengths spanning the cap, in both a single-byte and a multi-byte alphabet:
    // the cap is on CHARACTERS, so a 3-byte character must not be cut in half.
    let lengths = [
        0_usize,
        1,
        MAX_LOGGED_TEXT - 1,
        MAX_LOGGED_TEXT,
        MAX_LOGGED_TEXT + 1,
        MAX_LOGGED_TEXT * 3,
    ];
    for character in ['a', 'é', '✉'] {
        for length in lengths {
            let text: String = std::iter::repeat_n(character, length).collect();
            let kept = logged_text(Some(&text)).expect("a present text is kept");
            let expected = length.min(MAX_LOGGED_TEXT);
            assert_eq!(
                kept.chars().count(),
                expected,
                "case {character:?} x {length}: expected {expected} characters kept, got {}",
                kept.chars().count()
            );
            assert!(
                text.starts_with(&kept),
                "case {character:?} x {length}: the kept text must be a prefix of the original"
            );
            assert!(
                kept.chars().all(|c| c == character),
                "case {character:?} x {length}: a character must never be split"
            );
        }
    }
    assert!(
        logged_text(None).is_none(),
        "case absent text: expected None, got something"
    );
}
