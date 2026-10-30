// SPDX-License-Identifier: CC0-1.0

//! The ordering invariant every instant this service PERSISTS satisfies.
//!
//! RULE: an operation that records a change stamps the instant it persists from the
//! store's own instant accessor, so every recorded instant is strictly later than every
//! instant the store has already handed out — two changes recorded inside the same
//! millisecond never collapse onto the same stamp, and a stamp a write persists is never
//! earlier than one an earlier write persisted. The accessor exists because
//! `creationTime`/`updateTime` are millisecond-resolution on the wire
//! (`smithy_query(query="shape",shape="EmailContact")` carries them as
//! `@timestampFormat("date-time")`), which would otherwise order two changes made inside
//! one millisecond wrongly or not at all.
//!
//! Three operations of this service record an instant: `CreateEmailContact` (the
//! contact's creation and update instants), `SendActivationCode` (the issue instant the
//! activation code's documented 12-hour lifetime runs from) and `ActivateEmailContact`
//! (the update instant of the status change it makes). Every case below drives them
//! through the harness `email_contacts.rs` builds — the production
//! `MultiServiceRouter`, restJson1 serialization and the modelled-constraint walk —
//! back to back, so the whole sequence lands inside a millisecond or two and the
//! ordering is a property of the stamps rather than of the wall clock.

use serde_json::Value;
use service_com_amazonaws_notificationscontacts_notificationscontacts::state::{
    self, MAX_EMAIL_CONTACTS, STATUS_ACTIVE,
};

use crate::email_contacts::{
    activate, as_millis, create_ok, emulator, get_ok, issued_code, send_code, Emulator,
};

/// The issue instant the store holds for the activation token of `arn`.
///
/// `SendActivationCode` has an empty output and no response of this service carries the
/// instant, so the store is the only place it can be read — the same door
/// `activation_code_bounds.rs` reads the token through.
fn issued_at(em: &Emulator, arn: &str) -> i64 {
    state::service_state(&*em.engine)
        .expect("the emulator's own engine must expose this service's store")
        .lock()
        .contact(arn)
        .and_then(|contact| {
            contact
                .activation_token
                .as_ref()
                .map(|token| token.issued_at_millis)
        })
        .unwrap_or_else(|| panic!("{arn} must hold an activation token after a send"))
}

/// Assert that `recorded` — instants in the order the calls persisted them — rises
/// strictly.
fn assert_strictly_increasing(case: &str, recorded: &[(String, i64)]) {
    assert!(
        recorded.len() >= 2,
        "case {case:?}: the sequence must record at least two instants to order, got {recorded:?}"
    );
    for pair in recorded.windows(2) {
        let (earlier_label, earlier) = &pair[0];
        let (later_label, later) = &pair[1];
        assert!(
            later > earlier,
            "case {case:?}: {later_label} was recorded after {earlier_label}, so it must be \
             strictly later; got {later} against {earlier} (whole sequence: {recorded:?})"
        );
    }
}

/// One row of the table below: a write sequence, described by what it writes.
struct Row {
    label: &'static str,
    /// Further contacts created after the subject and before the first send. Each
    /// records its own creation instant, so a send that follows has to be stamped past
    /// all of them.
    bystanders: usize,
    /// How many activation codes are sent for the subject; each send mints a code and
    /// records the instant that code's lifetime runs from.
    sends: usize,
    /// Whether the subject is activated at the end with the last code sent, which
    /// records the update instant of the status change.
    activates: bool,
}

/// Drive `row`'s write sequence on a fresh emulator and return every instant it
/// recorded, in the order the calls recorded them.
async fn recorded_instants(row: &Row) -> Vec<(String, i64)> {
    let case = row.label;
    let em = emulator();
    let mut recorded: Vec<(String, i64)> = Vec::new();

    let arn = create_ok(&em, "subject", "subject@example.com").await;
    let subject = get_ok(&em, &arn).await;
    let created = as_millis(&subject, "creationTime");
    assert_eq!(
        as_millis(&subject, "updateTime"),
        created,
        "case {case:?}: a contact nothing has changed since its creation carries one \
         instant for both members, got {subject:?}"
    );
    recorded.push(("the subject's creationTime".to_string(), created));

    for n in 0..row.bystanders {
        let name = format!("bystander-{n}");
        let other = create_ok(&em, &name, &format!("{name}@example.com")).await;
        let instant = as_millis(&get_ok(&em, &other).await, "creationTime");
        recorded.push((format!("{name}'s creationTime"), instant));
    }

    for n in 0..row.sends {
        let (status, body) = send_code(&em, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: send {} must answer 200, got {status} / {body:?}",
            n + 1
        );
        recorded.push((
            format!("the issue instant of the code send {} minted", n + 1),
            issued_at(&em, &arn),
        ));
    }

    if row.activates {
        let code = issued_code(&em, &arn)
            .unwrap_or_else(|| panic!("case {case:?}: the send must persist a code to activate"));
        let (status, body) = activate(&em, &arn, &code).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the issued code must activate the contact, got {status} / {body:?}"
        );
        let activated = get_ok(&em, &arn).await;
        assert_eq!(
            activated.get("status").and_then(Value::as_str),
            Some(STATUS_ACTIVE),
            "case {case:?}: the activated contact must read back {STATUS_ACTIVE}, got {activated:?}"
        );
        recorded.push((
            "the subject's updateTime after its activation".to_string(),
            as_millis(&activated, "updateTime"),
        ));
    }

    recorded
}

#[tokio::test]
async fn every_recorded_instant_is_strictly_later_than_the_ones_before_it() {
    // Rows differ along what the rule generalises over: WHICH writes record an instant,
    // how many of them there are, and in which order they run. Every row's calls run
    // back to back, so each one exercises several writes inside the same millisecond —
    // the case the store's accessor exists for. The bystander counts stay inside
    // MAX_EMAIL_CONTACTS, the quota a create is refused past.
    let rows = [
        Row {
            label: "one send straight after the create",
            bystanders: 0,
            sends: 1,
            activates: false,
        },
        Row {
            label: "one send, then the activation",
            bystanders: 0,
            sends: 1,
            activates: true,
        },
        Row {
            label: "two sends, the second superseding the first",
            bystanders: 0,
            sends: 2,
            activates: false,
        },
        Row {
            label: "five sends, then the activation",
            bystanders: 0,
            sends: 5,
            activates: true,
        },
        Row {
            label: "a send behind one other contact's create",
            bystanders: 1,
            sends: 1,
            activates: true,
        },
        Row {
            label: "a send behind a burst of creates",
            bystanders: MAX_EMAIL_CONTACTS - 2,
            sends: 1,
            activates: false,
        },
        Row {
            label: "sends and an activation behind a full store of creates",
            bystanders: MAX_EMAIL_CONTACTS - 1,
            sends: 3,
            activates: true,
        },
    ];

    for row in &rows {
        assert!(
            row.bystanders < MAX_EMAIL_CONTACTS,
            "case {:?}: the row must stay inside the {MAX_EMAIL_CONTACTS}-contact quota",
            row.label
        );
        let recorded = recorded_instants(row).await;
        assert_strictly_increasing(row.label, &recorded);
    }
}

/// The seed the randomized sequences are drawn from. Fixed, so a failing case replays
/// exactly, and named in every assertion message below.
const SEED: u64 = 0x2545_f491_4f6c_dd1d;

/// How many randomized sequences run — bounded, so the suite stays quick and writes
/// nothing to disk.
const SEQUENCE_CASES: u64 = 8;

/// How many operations each randomized sequence drives.
const STEPS_PER_SEQUENCE: u64 = 14;

/// SplitMix64, so the step choices below are reproducible from [`SEED`] alone without a
/// generator dependency.
fn next_u64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

#[tokio::test]
async fn a_randomized_write_sequence_still_records_strictly_increasing_instants() {
    // The invariant is an ORDERING one, so it is checked over generated sequences as
    // well as the table above: whichever writes a caller interleaves, and in whatever
    // order, the instants the store persisted rise strictly.
    for case in 0..SEQUENCE_CASES {
        let label = format!("seed {SEED:#x}, sequence {case}");
        let mut rng = SEED ^ case;
        let em = emulator();
        // Each contact, with whether it has been activated: a send is refused for an
        // active contact and an activation of one records nothing, so neither is driven
        // here — their own suites own those refusals.
        let mut contacts: Vec<(String, bool)> = Vec::new();
        let mut recorded: Vec<(String, i64)> = Vec::new();

        for step in 0..=STEPS_PER_SEQUENCE {
            // The first step always creates, so the later steps have a contact to work
            // on; after that the operation is drawn.
            let roll = if step == 0 { 0 } else { next_u64(&mut rng) % 3 };
            let pick = next_u64(&mut rng) as usize;
            match roll {
                0 if contacts.len() < MAX_EMAIL_CONTACTS => {
                    let name = format!("seq{case}-{step}");
                    let arn = create_ok(&em, &name, &format!("{name}@example.com")).await;
                    let instant = as_millis(&get_ok(&em, &arn).await, "creationTime");
                    contacts.push((arn, false));
                    recorded.push((format!("step {step}: {name}'s creationTime"), instant));
                }
                1 | 2 if contacts.iter().any(|(_, active)| !active) => {
                    let inactive: Vec<usize> = contacts
                        .iter()
                        .enumerate()
                        .filter(|(_, (_, active))| !active)
                        .map(|(index, _)| index)
                        .collect();
                    let index = inactive[pick % inactive.len()];
                    let arn = contacts[index].0.clone();
                    let (status, body) = send_code(&em, &arn).await;
                    assert_eq!(
                        status, 200,
                        "case {label:?} step {step}: a send for an inactive contact must \
                         answer 200, got {status} / {body:?}"
                    );
                    recorded.push((
                        format!("step {step}: the issue instant of the code minted for {arn}"),
                        issued_at(&em, &arn),
                    ));

                    if roll == 2 {
                        let code = issued_code(&em, &arn).unwrap_or_else(|| {
                            panic!("case {label:?} step {step}: the send must persist a code")
                        });
                        let (status, body) = activate(&em, &arn, &code).await;
                        assert_eq!(
                            status, 200,
                            "case {label:?} step {step}: the issued code must activate the \
                             contact, got {status} / {body:?}"
                        );
                        contacts[index].1 = true;
                        recorded.push((
                            format!("step {step}: {arn}'s updateTime after its activation"),
                            as_millis(&get_ok(&em, &arn).await, "updateTime"),
                        ));
                    }
                }
                // Nothing to do: the store is at its quota, or every contact is already
                // active. The sequence keeps going.
                _ => {}
            }
        }

        assert_strictly_increasing(&label, &recorded);
    }
}
