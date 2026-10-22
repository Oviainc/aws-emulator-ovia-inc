// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the account input policy: `PutPolicy`, `GetPolicy` and
//! `DeletePolicy`.
//!
//! The rule these cases pin: an account carries AT MOST ONE policy, and that policy is
//! exactly the set of per-input-kind decisions the last `PutPolicy` stated — `PutPolicy`
//! replaces it rather than merging into it, `GetPolicy` reports it unchanged and reports
//! NO `Policy` member while the account carries none (before any `PutPolicy`, and after
//! `DeletePolicy`), and `DeletePolicy` drops it.
//!
//! One policy is refused rather than stored: a policy that leaves NO input location
//! allowed. A location the policy does not name carries no decision and is allowed, so
//! that is exactly the policy stating `DISALLOWED` for every location the `Policy` shape
//! models — AWS answers `BadRequestException` (400) and stores nothing.
//!
//! The wire harness is the one `job_lifecycle` already builds: a single shared engine
//! behind the real multi-service router, so every case here asserts on the bytes a
//! client would see.

use serde_json::{json, Map, Value};
use service_com_amazonaws_mediaconvert_mediaconvert::ops::types::INPUT_POLICY_VALUES;

use crate::job_lifecycle::{Emulator, Reply};

/// The policy's URI — the `http` binding all three policy operations share
/// (`smithy_query(query="operation",operation="GetPolicy,PutPolicy,DeletePolicy")`).
const POLICY_URI: &str = "/2017-08-29/policy";

/// The members of the modelled `Policy`, under the `jsonName` each goes on the wire with
/// (`smithy_query(query="shape",shape="Policy")`): `httpInputs`, `httpsInputs`,
/// `s3Inputs`. One input kind per member.
const INPUT_KINDS: [&str; 3] = ["httpInputs", "httpsInputs", "s3Inputs"];

/// Whether a stated policy leaves NO input location allowed.
///
/// An input location the policy does not name carries no decision — it is allowed, which
/// is what an account with no policy at all allows — so a policy allows nothing only when
/// it states `DISALLOWED` for every one of the three locations in [`INPUT_KINDS`]. That is
/// the policy AWS refuses: "Policies require at least one input location to be set to
/// ALLOWED."
fn allows_no_input(policy: &Value) -> bool {
    INPUT_KINDS
        .iter()
        .all(|kind| policy.get(kind).and_then(Value::as_str) == Some("DISALLOWED"))
}

async fn put_policy(emulator: &Emulator, policy: &Value) -> Reply {
    emulator
        .send("PUT", POLICY_URI, Some(json!({ "policy": policy })))
        .await
}

async fn get_policy(emulator: &Emulator) -> Reply {
    emulator.send("GET", POLICY_URI, None).await
}

async fn delete_policy(emulator: &Emulator) -> Reply {
    emulator.send("DELETE", POLICY_URI, None).await
}

/// Every policy a caller can state: each of the three input kinds either carries one of
/// the modelled `InputPolicy` values or is left out entirely.
///
/// Drawn from the generated `INPUT_POLICY_VALUES` — the model's own value set
/// (`smithy_query(query="shape",shape="InputPolicy")`) — rather than a hand-typed list,
/// so the table cannot drift from the model. Three choices per member gives 3^3 = 27
/// rows, the empty policy included.
fn every_policy() -> Vec<Value> {
    let choices: Vec<Option<&str>> = std::iter::once(None)
        .chain(INPUT_POLICY_VALUES.iter().copied().map(Some))
        .collect();
    let mut rows = Vec::new();
    for http in &choices {
        for https in &choices {
            for s3 in &choices {
                let mut policy = Map::new();
                for (kind, choice) in INPUT_KINDS.iter().zip([http, https, s3]) {
                    if let Some(value) = choice {
                        policy.insert((*kind).to_string(), Value::String((*value).to_string()));
                    }
                }
                rows.push(Value::Object(policy));
            }
        }
    }
    rows
}

/// A policy is stored whole: what `PutPolicy` states is what `GetPolicy` reports, and an
/// input kind the request leaves out is absent afterwards rather than carried over from
/// the policy that was there before — unless the policy allows no input location at all,
/// which is refused and stores nothing.
///
/// The rows span the class the rule generalises over — which input kinds the policy
/// names, and which modelled decision each carries — and run against ONE account in
/// sequence, so every row also replaces a different predecessor. Each row is asserted
/// against the rule itself ([`allows_no_input`]), not against a hand-listed verdict.
#[tokio::test]
async fn a_put_policy_is_the_whole_policy_the_account_reads_back() {
    let emulator = Emulator::new();
    let rows = every_policy();
    assert_eq!(
        rows.len(),
        27,
        "the table must span every choice of every input kind, got {} rows from {INPUT_POLICY_VALUES:?}",
        rows.len()
    );

    let mut previous = Value::Null;
    for policy in &rows {
        if allows_no_input(policy) {
            // The one row in the space that allows nothing: refused, and the policy that
            // was already stored is still the one the account reads back.
            let refused = put_policy(&emulator, policy).await;
            assert_eq!(
                refused.status, 400,
                "case {policy}: a policy allowing no input location must be refused with HTTP \
                 400, got {} with body {}",
                refused.status, refused.body
            );
            let after = get_policy(&emulator).await;
            assert_eq!(
                after.at("/policy"),
                &previous,
                "case {policy}: a refused PutPolicy must leave {previous} stored, got {}",
                after.at("/policy")
            );
            continue;
        }

        let put = put_policy(&emulator, policy).await;
        assert_eq!(
            put.status, 200,
            "case {policy}: expected HTTP 200 from PutPolicy, got {} with body {}",
            put.status, put.body
        );
        assert_eq!(
            put.at("/policy"),
            policy,
            "case {policy}: PutPolicy must answer the policy it stored, got {}",
            put.at("/policy")
        );

        let got = get_policy(&emulator).await;
        assert_eq!(
            got.status, 200,
            "case {policy}: expected HTTP 200 from GetPolicy, got {} with body {}",
            got.status, got.body
        );
        assert_eq!(
            got.at("/policy"),
            policy,
            "case {policy}: GetPolicy must report the policy that was put, got {}",
            got.at("/policy")
        );

        // Member by member, so a kind the row leaves out is shown to be absent rather
        // than inherited from `previous` or invented by the service.
        for kind in INPUT_KINDS {
            let actual = got.at(&format!("/policy/{kind}"));
            match policy.get(kind) {
                Some(expected) => assert_eq!(
                    actual, expected,
                    "case {policy}: {kind} must read back as it was put, got {actual}"
                ),
                None => assert!(
                    actual.is_null(),
                    "case {policy}: {kind} was not stated, so it must be absent and not \
                     carried over from {previous}, got {actual}"
                ),
            }
        }

        previous = policy.clone();
    }
}

/// An account reports no policy until one is put, and none again once it is deleted.
///
/// The rows differ in how much the deleted policy stated — every input kind, one of
/// them, none of them — because what is dropped is the whole record either way. Nothing
/// identifies the policy, so a delete with none stored leaves the same state and answers
/// the same 200.
#[tokio::test]
async fn deleting_the_policy_leaves_the_account_carrying_none() {
    let emulator = Emulator::new();

    let before = get_policy(&emulator).await;
    assert_eq!(
        before.status, 200,
        "an account with no policy must still answer HTTP 200, got {} with body {}",
        before.status, before.body
    );
    assert!(
        before.at("/policy").is_null(),
        "case never put: an account that has never put a policy carries none, got {}",
        before.at("/policy")
    );

    let rows = [
        json!({ "httpInputs": "DISALLOWED", "httpsInputs": "ALLOWED", "s3Inputs": "DISALLOWED" }),
        json!({ "s3Inputs": "ALLOWED" }),
        json!({}),
    ];

    for policy in &rows {
        let put = put_policy(&emulator, policy).await;
        assert_eq!(
            put.status, 200,
            "case {policy}: expected HTTP 200 from PutPolicy, got {} with body {}",
            put.status, put.body
        );
        let stored = get_policy(&emulator).await;
        assert_eq!(
            stored.at("/policy"),
            policy,
            "case {policy}: the policy must be in place before it is deleted, got {}",
            stored.at("/policy")
        );

        for attempt in 1..=2 {
            let deleted = delete_policy(&emulator).await;
            assert_eq!(
                deleted.status, 200,
                "case {policy} delete #{attempt}: expected HTTP 200 from DeletePolicy, got {} \
                 with body {}",
                deleted.status, deleted.body
            );
            let after = get_policy(&emulator).await;
            assert_eq!(
                after.status, 200,
                "case {policy} delete #{attempt}: expected HTTP 200 from GetPolicy, got {} with \
                 body {}",
                after.status, after.body
            );
            assert!(
                after.at("/policy").is_null(),
                "case {policy} delete #{attempt}: the account carries no policy after a delete, \
                 got {}",
                after.at("/policy")
            );
        }
    }
}

/// Reading the policy changes nothing a later caller sees: repeated `GetPolicy` calls
/// answer the same policy, and repeated calls with none stored keep answering none
/// rather than materialising one.
#[tokio::test]
async fn reading_the_policy_never_changes_what_a_later_caller_sees() {
    let emulator = Emulator::new();
    let policy = json!({ "httpInputs": "DISALLOWED", "s3Inputs": "ALLOWED" });

    let put = put_policy(&emulator, &policy).await;
    assert_eq!(
        put.status, 200,
        "expected HTTP 200 from PutPolicy, got {} with body {}",
        put.status, put.body
    );

    for read in 1..=4 {
        let got = get_policy(&emulator).await;
        assert_eq!(
            got.at("/policy"),
            &policy,
            "case read #{read} of a stored policy: every read answers the stored policy, got {}",
            got.at("/policy")
        );
    }

    let deleted = delete_policy(&emulator).await;
    assert_eq!(
        deleted.status, 200,
        "expected HTTP 200 from DeletePolicy, got {} with body {}",
        deleted.status, deleted.body
    );

    for read in 1..=4 {
        let got = get_policy(&emulator).await;
        assert!(
            got.at("/policy").is_null(),
            "case read #{read} after the delete: every read answers no policy, got {}",
            got.at("/policy")
        );
    }
}

/// A decision outside `InputPolicy`'s modelled value set is refused at the boundary, and
/// the refused request leaves the stored policy exactly as it was.
///
/// The rows are values no `InputPolicy` models: a lowercase spelling of a modelled one, a
/// word that is not in the set at all, and the empty string.
#[tokio::test]
async fn a_decision_outside_the_modelled_value_set_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    let stored = json!({ "httpInputs": "ALLOWED", "httpsInputs": "DISALLOWED" });
    let put = put_policy(&emulator, &stored).await;
    assert_eq!(
        put.status, 200,
        "expected HTTP 200 from PutPolicy, got {} with body {}",
        put.status, put.body
    );

    for bad in ["allowed", "MAYBE", ""] {
        let refused = put_policy(&emulator, &json!({ "httpInputs": bad })).await;
        assert_eq!(
            refused.status, 400,
            "case {bad:?}: a value outside {INPUT_POLICY_VALUES:?} must be refused with HTTP \
             400, got {} with body {}",
            refused.status, refused.body
        );

        let got = get_policy(&emulator).await;
        assert_eq!(
            got.at("/policy"),
            &stored,
            "case {bad:?}: a refused PutPolicy must leave the stored policy alone, got {}",
            got.at("/policy")
        );
    }
}

/// A policy that allows NO input location is refused with `BadRequestException` (400),
/// stores nothing, and the refusal names the constraint it broke; a policy that leaves at
/// least one location allowed is stored.
///
/// The refused rows differ in the ORDER the three decisions arrive in and in what was
/// stored before them, so the refusal cannot be keyed to one request body or to one
/// predecessor. The accepted rows differ in WHICH location is the allowed one, in how it
/// is allowed (stated `ALLOWED`, or left out and so carrying no decision) and in how many
/// locations the policy names at all: every nearest neighbour of the refused policy, each
/// of which AWS stores.
#[tokio::test]
async fn a_policy_that_allows_no_input_location_is_refused_and_stores_nothing() {
    // The predecessors a refusal must leave alone: none at all, one location, and a
    // policy naming all three.
    let predecessors = [
        ("no policy stored", Value::Null),
        ("one location stored", json!({ "s3Inputs": "ALLOWED" })),
        (
            "all three locations stored",
            json!({
                "httpInputs": "DISALLOWED",
                "httpsInputs": "ALLOWED",
                "s3Inputs": "DISALLOWED"
            }),
        ),
    ];
    // The same all-DISALLOWED policy, with its members arriving in different orders: a
    // JSON object's member order is not part of the policy.
    let refused_bodies = [
        json!({ "s3Inputs": "DISALLOWED", "httpInputs": "DISALLOWED", "httpsInputs": "DISALLOWED" }),
        json!({ "httpsInputs": "DISALLOWED", "s3Inputs": "DISALLOWED", "httpInputs": "DISALLOWED" }),
    ];

    for (label, predecessor) in &predecessors {
        for refused_body in &refused_bodies {
            let emulator = Emulator::new();
            if !predecessor.is_null() {
                let seeded = put_policy(&emulator, predecessor).await;
                assert_eq!(
                    seeded.status, 200,
                    "case {label} / {refused_body}: the predecessor policy must be stored, got \
                     {} with body {}",
                    seeded.status, seeded.body
                );
            }

            assert!(
                allows_no_input(refused_body),
                "case {label} / {refused_body}: this row must be an all-DISALLOWED policy"
            );
            let refused = put_policy(&emulator, refused_body).await;
            assert_eq!(
                refused.status, 400,
                "case {label} / {refused_body}: expected HTTP 400, got {} with body {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_type.as_deref(),
                Some("BadRequestException"),
                "case {label} / {refused_body}: expected BadRequestException, got {:?} with body \
                 {}",
                refused.error_type,
                refused.body
            );
            // The message rule AWS states is the constraint itself, so the message must
            // name it: the locations, in their modelled member spelling, and that at
            // least one of them has to be ALLOWED. The recorded prose is only an example.
            let message = refused.string("/message");
            for member in ["S3Inputs", "HttpInputs", "HttpsInputs", "ALLOWED"] {
                assert!(
                    message.contains(member),
                    "case {label} / {refused_body}: the refusal must name {member:?}, got \
                     {message:?}"
                );
            }

            // Nothing was stored: the account still reads back exactly its predecessor.
            let after = get_policy(&emulator).await;
            assert_eq!(
                after.status, 200,
                "case {label} / {refused_body}: expected HTTP 200 from GetPolicy, got {} with \
                 body {}",
                after.status, after.body
            );
            assert_eq!(
                after.at("/policy"),
                predecessor,
                "case {label} / {refused_body}: the refused put must leave {predecessor} \
                 stored, got {}",
                after.at("/policy")
            );
        }
    }

    // The nearest neighbours of the refused policy: one location allowed, by being stated
    // ALLOWED or by being left out, with the rest DISALLOWED. Every one is stored.
    let mut accepted: Vec<Value> = Vec::new();
    for allowed in INPUT_KINDS {
        for state_it in [true, false] {
            let mut policy = Map::new();
            for kind in INPUT_KINDS {
                if kind == allowed {
                    if state_it {
                        policy.insert(kind.to_string(), json!("ALLOWED"));
                    }
                } else {
                    policy.insert(kind.to_string(), json!("DISALLOWED"));
                }
            }
            accepted.push(Value::Object(policy));
        }
    }
    assert_eq!(
        accepted.len(),
        6,
        "one row per input kind per way of allowing it, got {}",
        accepted.len()
    );

    for policy in &accepted {
        let emulator = Emulator::new();
        assert!(
            !allows_no_input(policy),
            "case {policy}: this row must leave an input location allowed"
        );
        let put = put_policy(&emulator, policy).await;
        assert_eq!(
            put.status, 200,
            "case {policy}: a policy leaving one location allowed must be stored, got {} with \
             body {}",
            put.status, put.body
        );
        let got = get_policy(&emulator).await;
        assert_eq!(
            got.at("/policy"),
            policy,
            "case {policy}: GetPolicy must report the policy that was put, got {}",
            got.at("/policy")
        );
    }
}
