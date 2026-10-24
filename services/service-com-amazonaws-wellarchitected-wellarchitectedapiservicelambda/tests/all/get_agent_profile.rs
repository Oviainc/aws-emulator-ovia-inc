// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `GetAgentProfile` — the read that reports the agent
//! profile a `profileArn` resolves to (`GET /api/v1/agent-profiles/{profileArn}`,
//! `smithy_query(query="operation", operation="GetAgentProfile")`).
//!
//! The round trip of ONE profile and the error envelopes of an unresolvable or
//! malformed ARN are pinned by `super::agent_goals`. What this suite adds is the two
//! rules that only show up with SEVERAL profiles in the store, each over a table of
//! rows rather than one request:
//!
//! 1. RESOLUTION. A read is answered from the profile its `profileArn` identifies and
//!    from no other: with profiles stored that differ along every member a profile
//!    carries — name, pillar set, aggregation scope count, tags, `deletionProtection` —
//!    each ARN reports its own stored members and its own DERIVED members
//!    (`eligibleFor…`, `fieldErrors`), never a neighbour's.
//! 2. PURE READ. `GetAgentProfile` is `@readonly`: a read — and a read that misses —
//!    changes nothing a later caller sees. Repeated reads of every stored profile are
//!    identical to the first, a 404 read of an absent ARN adds no profile, and the
//!    stored profile set is exactly what the creates put there.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client lives in `super::workload_harness`; the agent-profile
//! request builders and the profile projection check live in `super::agent_goals`.

use serde_json::{json, Value};

use super::agent_goals::{
    absent_profile_arn, assert_profile_projection, create_profile, get_profile, member,
    profile_body, PILLARS,
};
use super::workload_harness::Client;

/// One row of the profile table: the case name (also the profile's `name`, which the
/// modelled `^[a-zA-Z0-9_-]+$` accepts), how many aggregation scopes it names, its
/// pillar set, whether it sends `deletionProtection` explicitly, and how many tags it
/// carries.
struct Row {
    case: &'static str,
    aggregation: usize,
    pillars: &'static [&'static str],
    deletion_protection: Option<bool>,
    tags: usize,
}

/// The profiles every test here spans. The rows differ along what the read reports:
/// the pillar set (one, several, and the whole published catalog — the modelled
/// `Pillars` `@length(1, 5)` maximum), the number of aggregation scopes (none, which is
/// the modelled `@length(min: 0)` floor, through several, which is what the derived
/// eligibility members key on), whether `deletionProtection` is sent or left to its
/// modelled `default: true`, and how many tags the profile carries.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no-scope-one-pillar-default-protection",
            aggregation: 0,
            pillars: &["SECURITY"],
            deletion_protection: None,
            tags: 0,
        },
        Row {
            case: "one-scope-two-pillars-protection-off",
            aggregation: 1,
            pillars: &["COST_OPTIMIZATION", "PERFORMANCE"],
            deletion_protection: Some(false),
            tags: 1,
        },
        Row {
            case: "three-scopes-every-pillar-protection-on",
            aggregation: 3,
            pillars: &PILLARS,
            deletion_protection: Some(true),
            tags: 3,
        },
        Row {
            case: "no-scope-pillar-the-issue-did-not-cite",
            aggregation: 0,
            pillars: &["OPERATIONAL_EXCELLENCE", "RESILIENCE"],
            deletion_protection: Some(false),
            tags: 2,
        },
    ]
}

impl Row {
    /// The `CreateAgentProfile` body this row sends.
    fn body(&self) -> Value {
        let mut body = profile_body(self.case, self.pillars, self.aggregation);
        if let Some(protection) = self.deletion_protection {
            body["deletionProtection"] = json!(protection);
        }
        if self.tags > 0 {
            body["tags"] = (0..self.tags)
                .map(|index| json!({ "key": format!("key-{index}"), "value": self.case }))
                .collect::<Vec<Value>>()
                .into();
        }
        body
    }

    /// What `deletionProtection` must read back as: the value sent, or the modelled
    /// `default: true` when the request sent none.
    fn expected_protection(&self) -> bool {
        self.deletion_protection.unwrap_or(true)
    }

    /// A profile is eligible for recommendation generation exactly when it names a scope
    /// of Amazon Web Services resources to analyse.
    fn eligible(&self) -> bool {
        self.aggregation > 0
    }
}

/// Create every row and answer `(row, arn, sent body)`, with the create itself asserted
/// and the minted ARNs checked to be distinct — distinct ARNs are what makes rule 1 a
/// statement about resolution at all. Each row travels WITH its own ARN and body, so no
/// assertion below depends on the order the table was created in.
async fn seed(client: &Client) -> Vec<(Row, String, Value)> {
    let mut seeded: Vec<(Row, String, Value)> = Vec::new();
    for row in rows() {
        let body = row.body();
        let created = create_profile(client, body.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected CreateAgentProfile 200, got {created:?}",
            row.case
        );
        let arn = member(&created, "arn").to_string();
        assert!(
            seeded.iter().all(|(_, stored, _)| stored != &arn),
            "case {:?}: every create must mint its own ARN, got a repeat of {arn:?}",
            row.case
        );
        seeded.push((row, arn, body));
    }
    seeded
}

/// Rule 1: each stored profile is reported by its OWN ARN. Every member the row sent
/// comes back as sent, every member the service owns is that profile's own, and the
/// derived members answer for that profile's scope — while every other stored ARN
/// answers its own profile instead of this one.
#[tokio::test]
async fn a_read_is_answered_from_the_profile_its_arn_identifies() {
    let client = Client::new();
    let seeded = seed(&client).await;

    for (row, arn, body) in &seeded {
        let case = row.case;
        let fetched = get_profile(&client, arn).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected GetAgentProfile 200, got {fetched:?}"
        );

        // The stored, service-owned and derived members, against the body THIS row sent.
        assert_profile_projection(
            &fetched,
            body,
            case,
            "GetAgentProfile",
            row.expected_protection(),
            row.eligible(),
        );
        assert_eq!(
            fetched.body.get("arn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: the reported arn must be the one asked for, got {fetched:?}"
        );
        // Nothing has modified a profile that was only created, so the `lastModified…`
        // members are absent rather than echoing the create.
        for absent in ["lastModifiedBy", "lastModifiedAt"] {
            assert!(
                fetched.body.get(absent).is_none_or(Value::is_null),
                "case {case:?}: {absent} must be absent on an unmodified profile, \
                 got {fetched:?}"
            );
        }

        // The read resolved THIS profile and no other: every other stored ARN answers a
        // different body, and the members that identify a profile are ITS own.
        for (other_row, other_arn, other_body) in &seeded {
            if other_arn == arn {
                continue;
            }
            let other_case = other_row.case;
            let other = get_profile(&client, other_arn).await;
            assert_ne!(
                other.body, fetched.body,
                "case {case:?}: {other_case:?}'s ARN must not answer this profile, \
                 got {other:?}"
            );
            assert_eq!(
                other.body.get("arn").and_then(Value::as_str),
                Some(other_arn.as_str()),
                "case {case:?}: {other_case:?}'s ARN must report itself, got {other:?}"
            );
            for identifying in ["name", "pillars", "aggregationConfiguration"] {
                assert_eq!(
                    other.body.get(identifying),
                    other_body.get(identifying),
                    "case {case:?}: {other_case:?}'s ARN must answer its OWN \
                     {identifying}, got {other:?}"
                );
            }
        }
    }
}

/// Rule 2: the read is pure. Reading every stored profile repeatedly, and missing on
/// well-formed ARNs no profile carries, leaves the store exactly as the creates left
/// it — the same bodies and the same profile set.
#[tokio::test]
async fn a_read_changes_nothing_a_later_caller_sees() {
    let client = Client::new();
    let seeded = seed(&client).await;

    let mut before: Vec<Value> = Vec::new();
    for (row, arn, _) in &seeded {
        let answer = get_profile(&client, arn).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected GetAgentProfile 200, got {answer:?}",
            row.case
        );
        before.push(answer.body);
    }

    // Rows that differ along the form of the resource part of an ARN no profile
    // carries: a hex id, a name with the separators the modelled pattern allows, and the
    // shortest resource part that pattern accepts.
    let misses = [
        ("absent-hex-resource", absent_profile_arn(&"7".repeat(32))),
        ("absent-name-resource", absent_profile_arn("some_profile-9")),
        ("absent-single-character", absent_profile_arn("x")),
    ];
    for (case, arn) in &misses {
        assert!(
            seeded.iter().all(|(_, stored, _)| stored != arn),
            "case {case:?}: the row must not name a stored profile"
        );
        let answer = get_profile(&client, arn).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "ResourceId"),
            arn.as_str(),
            "case {case:?}: ResourceId must be the ARN as sent, got {answer:?}"
        );
    }

    // Three more reads of each profile, after the misses: every body is still the one
    // the first read answered.
    for _ in 0..3 {
        for ((row, arn, _), expected) in seeded.iter().zip(&before) {
            let case = row.case;
            let again = get_profile(&client, arn).await;
            assert_eq!(
                again.status, 200,
                "case {case:?}: expected GetAgentProfile 200 on a repeat read, \
                 got {again:?}"
            );
            assert_eq!(
                &again.body, expected,
                "case {case:?}: a repeat read must report the identical profile, \
                 got {again:?}"
            );
        }
    }

    // And the store holds exactly the profiles the creates put there: no read minted,
    // dropped or re-keyed one.
    let stored: Vec<String> = {
        let store = client.store();
        let guard = store.lock();
        let mut arns: Vec<String> = guard.agent_profiles.keys().cloned().collect();
        arns.sort();
        arns
    };
    let mut expected: Vec<String> = seeded.iter().map(|(_, arn, _)| arn.clone()).collect();
    expected.sort();
    assert_eq!(
        stored, expected,
        "case \"store-after-reads\": expected exactly the created profiles, got {stored:?}"
    );
}
