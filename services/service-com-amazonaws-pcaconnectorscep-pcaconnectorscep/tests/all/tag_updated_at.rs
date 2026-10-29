// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the `UpdatedAt` a tagging write leaves behind.
//!
//! The rule under test: a call that changes what this service has stored for a connector or a
//! challenge sets that record's `UpdatedAt` to the instant of the change, so the next reader
//! reports when the resource last changed; a call that leaves the record exactly as it was —
//! a refused one, a merge of entries the record already carried, or a plain read — leaves
//! `UpdatedAt` where it was, and `CreatedAt` never moves at all.
//!
//! `UpdatedAt` is "the date and time that the connector was updated" (smithy_query shape
//! Connector, member UpdatedAt) and "the date and time that the challenge was updated"
//! (shape Challenge, member UpdatedAt), and `TagResource` writes the tags onto the record
//! itself, so a successful tagging is one of those updates.
//!
//! Every case drives the service the way a client does: one HTTP request per call, through
//! the same `MultiServiceRouter` the emulator boots. The rows differ along what the rule
//! generalises over: the resource kind, whether the merge adds keys, overwrites values, does
//! both, or writes nothing new, how many entries it carries, and whether the call is accepted
//! or refused. Each record's timestamps are first moved into the past through the service's
//! own state — the same kind of clock move the harness makes for a pending transition — so
//! "moved to now" and "left alone" are separated by a wide, deterministic margin instead of
//! by however many microseconds a case happens to take.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{self, label, new_router, Answer};
use service_com_amazonaws_pcaconnectorscep_pcaconnectorscep as scep;

/// A region that is NOT the emulator default, so an ARN built from the request's own SigV4
/// credential scope is distinguishable from a hardcoded one.
const REGION: &str = "us-west-2";

/// A private-CA ARN matching `CertificateAuthorityArn`'s own pattern.
const CA_ARN: &str = "arn:aws:acm-pca:us-west-2:000000000000:certificate-authority/11111111-2222-3333-4444-555555555555";

/// How far into the past a record's stored timestamps are moved before the call under test,
/// so a timestamp that moved to "now" and one that stayed put are far apart.
const BACKDATE: Duration = Duration::from_secs(600);

/// How much of [`BACKDATE`] a moved timestamp must have crossed to count as "moved to now"
/// rather than merely nudged. Generous in both directions: the point is the order of
/// magnitude, not the microseconds a case takes.
const MOVED_BY_AT_LEAST: f64 = BACKDATE.as_secs() as f64 / 2.0;

/// The per-resource tag ceiling, named from the service's own constant so a row that sits on
/// the bound cannot drift off it.
const MAX_TAGS_PER_RESOURCE: usize = scep::state::MAX_TAGS_PER_RESOURCE;

/// The tag pairs of a row, in the order the request sends them.
type Tags = Vec<(String, String)>;

fn pairs(entries: &[(&str, &str)]) -> Tags {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

fn tags_member(tags: &Tags) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    )
}

/// Epoch seconds, as the wire reports a timestamp of this `restJson1` service.
fn epoch(instant: SystemTime) -> f64 {
    instant
        .duration_since(UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs_f64()
}

/// Which of this service's two taggable resources a row tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Connector,
    Challenge,
}

/// What the call under test does to the record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// Accepted, and the record ends up different: `UpdatedAt` moves to the instant of the
    /// write.
    Changed,
    /// Accepted, but every entry it writes is one the record already carried, so the record is
    /// unchanged and `UpdatedAt` stays.
    Unchanged,
    /// Refused, so nothing is written and `UpdatedAt` stays.
    Refused,
}

/// One row: what the create stored, what the `TagResource` call sends, and what that does to
/// the record.
#[derive(Debug)]
struct Case {
    name: String,
    initial: Tags,
    add: Tags,
    outcome: Outcome,
}

/// The rows.
fn cases() -> Vec<Case> {
    let mut cases = vec![
        Case {
            name: "the first tag on a resource created without any".to_string(),
            initial: Tags::new(),
            add: pairs(&[("customer", "Example Corp")]),
            outcome: Outcome::Changed,
        },
        Case {
            name: "a second key beside the one the create stored".to_string(),
            initial: pairs(&[("environment", "production")]),
            add: pairs(&[("owner", "platform")]),
            outcome: Outcome::Changed,
        },
        Case {
            name: "a new value for a key the resource already carries".to_string(),
            initial: pairs(&[("environment", "staging"), ("owner", "platform")]),
            add: pairs(&[("environment", "production")]),
            outcome: Outcome::Changed,
        },
        Case {
            name: "one key added and one overwritten in the same call".to_string(),
            initial: pairs(&[("stage", "one"), ("zone", "a")]),
            add: pairs(&[("zone", "b"), ("app", "checkout")]),
            outcome: Outcome::Changed,
        },
        Case {
            name: "an empty value replacing a non-empty one".to_string(),
            initial: pairs(&[("owner", "platform")]),
            add: pairs(&[("owner", "")]),
            outcome: Outcome::Changed,
        },
        Case {
            name: "several keys at once, in letters from outside ASCII".to_string(),
            initial: pairs(&[("ascii", "yes")]),
            add: pairs(&[("équipe", "sécurité"), ("都市", "東京"), ("owner", "platform")]),
            outcome: Outcome::Changed,
        },
        Case {
            name: "exactly the entries the resource already carries".to_string(),
            initial: pairs(&[("environment", "production"), ("owner", "platform")]),
            add: pairs(&[("environment", "production"), ("owner", "platform")]),
            outcome: Outcome::Unchanged,
        },
        Case {
            name: "one of the entries the resource already carries".to_string(),
            initial: pairs(&[("environment", "production"), ("owner", "platform")]),
            add: pairs(&[("owner", "platform")]),
            outcome: Outcome::Unchanged,
        },
        Case {
            name: "an empty Tags map".to_string(),
            initial: pairs(&[("keep", "me")]),
            add: Tags::new(),
            outcome: Outcome::Unchanged,
        },
        Case {
            name: "an empty value the resource already carries".to_string(),
            initial: pairs(&[("owner", "")]),
            add: pairs(&[("owner", "")]),
            outcome: Outcome::Unchanged,
        },
        Case {
            name: "a key in the reserved namespace".to_string(),
            initial: pairs(&[("environment", "production")]),
            add: pairs(&[("aws:created-by", "me")]),
            outcome: Outcome::Refused,
        },
        Case {
            name: "a character outside the tag alphabet in the value".to_string(),
            initial: pairs(&[("keep", "me")]),
            add: pairs(&[("payload", "<not-a-tag-value>")]),
            outcome: Outcome::Refused,
        },
        Case {
            name: "an empty key".to_string(),
            initial: Tags::new(),
            add: pairs(&[("", "empty-key")]),
            outcome: Outcome::Refused,
        },
    ];

    // The per-resource ceiling from both sides: a new key over a full resource writes nothing,
    // while an overwrite with the value it already carries is accepted and changes nothing.
    let filled: Tags = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("key-{index:03}"), format!("value-{index}")))
        .collect();
    cases.push(Case {
        name: "a new key on a resource already at the per-resource ceiling".to_string(),
        initial: filled.clone(),
        add: pairs(&[("one-too-many", "x")]),
        outcome: Outcome::Refused,
    });
    cases.push(Case {
        name: "an overwrite with the stored value on a resource at the ceiling".to_string(),
        initial: filled.clone(),
        add: vec![("key-000".to_string(), "value-0".to_string())],
        outcome: Outcome::Unchanged,
    });
    cases.push(Case {
        name: "an overwrite with a new value on a resource at the ceiling".to_string(),
        initial: filled,
        add: vec![("key-000".to_string(), "rewritten".to_string())],
        outcome: Outcome::Changed,
    });
    cases
}

async fn create_connector(router: &MultiServiceRouter, tags: &Tags) -> Answer {
    let mut request = json!({ "CertificateAuthorityArn": CA_ARN });
    request["Tags"] = tags_member(tags);
    harness::call(router, REGION, "POST", "/connectors", Some(request)).await
}

async fn tag_resource(router: &MultiServiceRouter, arn: &str, tags: &Tags) -> Answer {
    harness::call(
        router,
        REGION,
        "POST",
        &format!("/tags/{}", label(arn)),
        Some(json!({ "Tags": tags_member(tags) })),
    )
    .await
}

async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Answer {
    harness::call(router, REGION, "GET", &format!("/tags/{}", label(arn)), None).await
}

/// An `ACTIVE` connector carrying `tags`, with the transition the clock owed it already made
/// due and settled, so its timestamps are not about to be moved by anything but the call
/// under test.
async fn active_connector(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    tags: &Tags,
) -> String {
    let created = create_connector(router, tags).await;
    assert_eq!(
        created.status, 202,
        "expected 202 from CreateConnector, got {} / {:?}",
        created.status, created.body
    );
    let arn = created.text("ConnectorArn");
    harness::release_pending(engine, &arn);
    let read = read_connector(router, &arn).await;
    assert_eq!(
        read.connector("Status").as_str(),
        Some("ACTIVE"),
        "expected the connector to settle to ACTIVE, got {:?}",
        read.body
    );
    arn
}

/// The challenge a connector mints, carrying `tags`.
async fn challenge_of(router: &MultiServiceRouter, connector_arn: &str, tags: &Tags) -> String {
    let created = harness::call(
        router,
        REGION,
        "POST",
        "/challenges",
        Some(json!({ "ConnectorArn": connector_arn, "Tags": tags_member(tags) })),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "expected 202 from CreateChallenge, got {} / {:?}",
        created.status, created.body
    );
    created.body["Challenge"]["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a challenge Arn in {:?}", created.body))
        .to_string()
}

async fn read_connector(router: &MultiServiceRouter, arn: &str) -> Answer {
    harness::call(
        router,
        REGION,
        "GET",
        &format!("/connectors/{}", label(arn)),
        None,
    )
    .await
}

async fn read_challenge_metadata(router: &MultiServiceRouter, arn: &str) -> Answer {
    harness::call(
        router,
        REGION,
        "GET",
        &format!("/challengeMetadata/{}", label(arn)),
        None,
    )
    .await
}

/// The `CreatedAt` and `UpdatedAt` the single read of `arn` reports, taken through whichever
/// reader its kind has.
async fn timestamps(router: &MultiServiceRouter, kind: Kind, arn: &str) -> (f64, f64) {
    let (answer, member) = match kind {
        Kind::Connector => (read_connector(router, arn).await, "Connector"),
        Kind::Challenge => (
            read_challenge_metadata(router, arn).await,
            "ChallengeMetadata",
        ),
    };
    assert_eq!(
        answer.status, 200,
        "expected 200 reading {arn}, got {} / {:?}",
        answer.status, answer.body
    );
    let timestamp = |name: &str| {
        answer.body[member][name].as_f64().unwrap_or_else(|| {
            panic!(
                "expected a numeric {name} in {:?}",
                answer.body[member].clone()
            )
        })
    };
    (timestamp("CreatedAt"), timestamp("UpdatedAt"))
}

/// The same two timestamps as a LISTING reports for `arn`, so a case checks the single read
/// and the listing agree about when the record last changed.
async fn listed_timestamps(
    router: &MultiServiceRouter,
    kind: Kind,
    arn: &str,
    connector_arn: &str,
) -> (f64, f64) {
    let (answer, items) = match kind {
        Kind::Connector => (
            harness::call(router, REGION, "GET", "/connectors", None).await,
            "Connectors",
        ),
        Kind::Challenge => (
            harness::call(
                router,
                REGION,
                "GET",
                &format!("/challengeMetadata?ConnectorArn={}", label(connector_arn)),
                None,
            )
            .await,
            "Challenges",
        ),
    };
    assert_eq!(
        answer.status, 200,
        "expected 200 from the listing of {arn}, got {} / {:?}",
        answer.status, answer.body
    );
    let entry = answer.body[items]
        .as_array()
        .unwrap_or_else(|| panic!("expected a {items} list in {:?}", answer.body))
        .iter()
        .find(|entry| entry["Arn"].as_str() == Some(arn))
        .unwrap_or_else(|| panic!("expected {arn} in {:?}", answer.body))
        .clone();
    let timestamp = |name: &str| {
        entry[name]
            .as_f64()
            .unwrap_or_else(|| panic!("expected a numeric {name} in {entry:?}"))
    };
    (timestamp("CreatedAt"), timestamp("UpdatedAt"))
}

/// Move the stored timestamps of the connector or challenge `arn` names [`BACKDATE`] into the
/// past, through the service's own state, and answer the instant they were moved to.
///
/// Strictly a clock move, like the harness's move of a pending transition's due instant: it
/// rewrites the two timestamps and nothing else — no tags, no status — so the handlers still
/// compute everything else from this same record.
fn backdate(engine: &engine::InMemoryEngine, arn: &str) -> f64 {
    let when = SystemTime::now() - BACKDATE;
    let shared = scep::state::service_state(engine).expect("the service's own state");
    let mut store = shared.lock();
    match scep::state::connector_arn_of_challenge(arn) {
        Some(connector_arn) => {
            let challenge = store
                .connectors
                .get_mut(connector_arn)
                .and_then(|connector| connector.challenges.get_mut(arn))
                .unwrap_or_else(|| panic!("expected a stored challenge under {arn}"));
            challenge.created_at = when;
            challenge.updated_at = when;
        }
        None => {
            let connector = store
                .connectors
                .get_mut(arn)
                .unwrap_or_else(|| panic!("expected a stored connector under {arn}"));
            connector.created_at = when;
            connector.updated_at = when;
        }
    }
    epoch(when)
}

/// The tags a row's resource carries once the call under test has been answered.
fn expected_tags(case: &Case) -> BTreeMap<String, String> {
    let mut tags: BTreeMap<String, String> = case.initial.iter().cloned().collect();
    if case.outcome != Outcome::Refused {
        for (key, value) in &case.add {
            tags.insert(key.clone(), value.clone());
        }
    }
    tags
}

#[tokio::test]
async fn tagging_moves_updated_at_exactly_when_it_changes_the_record() {
    for kind in [Kind::Connector, Kind::Challenge] {
        for case in cases() {
            let case_name = format!("{} on a {kind:?}", case.name);
            let (router, engine) = new_router();

            let (connector_arn, arn) = match kind {
                Kind::Connector => {
                    let arn = active_connector(&router, &engine, &case.initial).await;
                    (arn.clone(), arn)
                }
                Kind::Challenge => {
                    let connector = active_connector(&router, &engine, &Tags::new()).await;
                    let challenge = challenge_of(&router, &connector, &case.initial).await;
                    (connector, challenge)
                }
            };

            // Both stored timestamps are moved well into the past, so the call under test is
            // the only thing that could have put either of them near "now".
            let backdated = backdate(&engine, &arn);
            let (created_before, updated_before) = timestamps(&router, kind, &arn).await;
            assert!(
                (updated_before - backdated).abs() < 1.0 && (created_before - backdated).abs() < 1.0,
                "case {case_name:?}: expected the backdated timestamps {backdated} to be what \
                 the record reports, got CreatedAt {created_before} / UpdatedAt {updated_before}"
            );

            let tagged = tag_resource(&router, &arn, &case.add).await;
            let expected_status = if case.outcome == Outcome::Refused {
                400
            } else {
                204
            };
            assert_eq!(
                tagged.status, expected_status,
                "case {case_name:?}: expected {expected_status}, got {} / {:?}",
                tagged.status, tagged.body
            );
            let answered_at = epoch(SystemTime::now());

            let (created_after, updated_after) = timestamps(&router, kind, &arn).await;
            assert_eq!(
                created_after, created_before,
                "case {case_name:?}: CreatedAt must never move, got {created_after} after \
                 {created_before}"
            );
            match case.outcome {
                Outcome::Changed => {
                    assert!(
                        updated_after - updated_before >= MOVED_BY_AT_LEAST
                            && updated_after <= answered_at + 1.0,
                        "case {case_name:?}: a tagging that changes the record must report the \
                         instant of the change as UpdatedAt; expected something near \
                         {answered_at}, got {updated_after} (it was {updated_before})"
                    );
                }
                Outcome::Unchanged | Outcome::Refused => {
                    assert_eq!(
                        updated_after, updated_before,
                        "case {case_name:?}: a call that leaves the record as it was must leave \
                         UpdatedAt alone; expected {updated_before}, got {updated_after}"
                    );
                }
            }

            // The listing reports the same record as the single read does.
            let (created_listed, updated_listed) =
                listed_timestamps(&router, kind, &arn, &connector_arn).await;
            assert_eq!(
                (created_listed, updated_listed),
                (created_after, updated_after),
                "case {case_name:?}: the listing must report the same timestamps as the single \
                 read, got {created_listed} / {updated_listed}"
            );

            // The tags the row expects really are what the record carries, so the timestamp
            // above is the timestamp of THAT write.
            let listed = list_tags(&router, &arn).await;
            let expected = expected_tags(&case);
            let got: BTreeMap<String, String> = listed
                .tags()
                .iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect();
            assert_eq!(
                got, expected,
                "case {case_name:?}: expected the resource to carry {expected:?}, got {got:?}"
            );

            // Reading is not changing: every read above, and these, leave UpdatedAt where the
            // last write put it.
            let (created_reread, updated_reread) = timestamps(&router, kind, &arn).await;
            assert_eq!(
                (created_reread, updated_reread),
                (created_after, updated_after),
                "case {case_name:?}: reads must not move the timestamps, got {created_reread} / \
                 {updated_reread}"
            );
        }
    }
}

#[tokio::test]
async fn tagging_one_resource_leaves_every_other_record_untouched() {
    let (router, engine) = new_router();

    let first = active_connector(&router, &engine, &pairs(&[("which", "first")])).await;
    let second = active_connector(&router, &engine, &pairs(&[("which", "second")])).await;
    let challenge = challenge_of(&router, &first, &pairs(&[("which", "challenge")])).await;

    for arn in [&first, &second, &challenge] {
        backdate(&engine, arn);
    }
    let before_first = timestamps(&router, Kind::Connector, &first).await.1;
    let before_second = timestamps(&router, Kind::Connector, &second).await.1;
    let before_challenge = timestamps(&router, Kind::Challenge, &challenge).await.1;

    // Tagging the challenge changes the challenge, not the connector that owns it.
    let tagged = tag_resource(&router, &challenge, &pairs(&[("owner", "platform")])).await;
    assert_eq!(
        tagged.status, 204,
        "expected 204 tagging the challenge, got {} / {:?}",
        tagged.status, tagged.body
    );
    let after_challenge = timestamps(&router, Kind::Challenge, &challenge).await.1;
    assert!(
        after_challenge - before_challenge >= MOVED_BY_AT_LEAST,
        "case \"tagged challenge\": expected UpdatedAt to move from {before_challenge}, got \
         {after_challenge}"
    );
    assert_eq!(
        timestamps(&router, Kind::Connector, &first).await.1,
        before_first,
        "case \"connector owning the tagged challenge\": expected UpdatedAt to stay at \
         {before_first}"
    );

    // Tagging the connector changes the connector, not the challenge it owns and not another
    // connector.
    let tagged = tag_resource(&router, &first, &pairs(&[("owner", "platform")])).await;
    assert_eq!(
        tagged.status, 204,
        "expected 204 tagging the connector, got {} / {:?}",
        tagged.status, tagged.body
    );
    let after_first = timestamps(&router, Kind::Connector, &first).await.1;
    assert!(
        after_first - before_first >= MOVED_BY_AT_LEAST,
        "case \"tagged connector\": expected UpdatedAt to move from {before_first}, got \
         {after_first}"
    );
    assert_eq!(
        timestamps(&router, Kind::Challenge, &challenge).await.1,
        after_challenge,
        "case \"challenge of the tagged connector\": expected UpdatedAt to stay at \
         {after_challenge}"
    );
    assert_eq!(
        timestamps(&router, Kind::Connector, &second).await.1,
        before_second,
        "case \"another connector\": expected UpdatedAt to stay at {before_second}"
    );
}
