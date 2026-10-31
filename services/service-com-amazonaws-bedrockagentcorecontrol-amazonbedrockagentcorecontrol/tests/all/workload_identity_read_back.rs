// SPDX-License-Identifier: CC0-1.0

//! A generated-program end-to-end test of the workload-identity read path: whatever
//! sequence of creates, updates and deletes a caller issues, every later read reports the
//! record the LAST successful mutation of that name left, and nothing else.
//!
//! The invariant under test, one sentence: for each `name`, `GetWorkloadIdentity` answers
//! the modelled 200 carrying exactly the `allowedResourceOauth2ReturnUrls` of the last
//! accepted create or update of that name — in the order that request supplied, the whole
//! list and nothing from an earlier one — while the `name` is held, and the modelled 404
//! `ResourceNotFoundException` naming the resource type and the requested name once a
//! delete has removed it; a mutation of one name changes no other name's record, and an
//! identity's `workloadIdentityArn` and `createdTime` never move while it is held.
//!
//! The suites beside this one walk tables of single mutations. This one walks an
//! interleaved PROGRAM of them, drawn from a seeded generator, against a model of the
//! store kept in the test: every step predicts the answer from the model alone, and after
//! every step every name in the pool is read back and compared with the model. That is
//! what distinguishes a read wired to its own mutations from one serving a create-time
//! snapshot, which the program's interleavings would otherwise hide.

use std::collections::BTreeMap;

use serde_json::json;

use super::harness::{
    assert_not_found, carries_no_member, create_identity, delete_identity, error_code,
    expected_arn, expected_url_list, get_identity, instant, list_identities, listed_names,
    not_found_by_id, not_found_by_name, router, update_identity, MAX_RESULTS_MAX, NAME_MAX_LENGTH,
    NAME_MIN_LENGTH,
};

/// The seed the mutation program below is drawn from. Fixed, so a failure is reproducible;
/// it is named in every assertion message.
const PROGRAM_SEED: u64 = 0x5EED_1DEA_F00D_u64;

/// How many mutation steps the program walks. Bounded: the whole walk issues this many
/// mutations plus one read per pooled name after each of them, and writes nothing to disk.
const PROGRAM_STEPS: usize = 48;

/// The modelled success statuses of the three mutations and of the read
/// (`smithy_query operation=CreateWorkloadIdentity,UpdateWorkloadIdentity,
/// DeleteWorkloadIdentity,GetWorkloadIdentity`: `http.code`).
const CREATE_STATUS: u16 = 201;
const UPDATE_STATUS: u16 = 200;
const DELETE_STATUS: u16 = 204;
const READ_STATUS: u16 = 200;

/// The seeded generator the program is drawn from: SplitMix64, the mixing function
/// `rand`'s own `SeedableRng::seed_from_u64` uses to expand a seed.
///
/// Eight lines of arithmetic rather than a generator crate, so this test adds nothing to
/// this crate's `Cargo.toml` or to the workspace lockfile; the seed is a constant every
/// assertion message names, so a failing walk is replayed by reading it off the message.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in `0..bound` — every bound here is tiny, so the modulo bias cannot affect
    /// whether the invariant under test holds.
    fn below(&mut self, bound: usize) -> usize {
        (self.next_u64() % bound as u64) as usize
    }
}

/// What the model believes one held identity carries: the return-URL list the last
/// accepted mutation of it supplied, and the `createdTime` its first read reported (learnt
/// on that read, then held fixed, so an update that moved it would be caught).
struct Expected {
    urls: Option<Vec<&'static str>>,
    created_time: Option<f64>,
}

/// Which mutation a step issues.
#[derive(Debug)]
enum Step {
    Create,
    Update,
    Delete,
}

/// A return-URL list a step may supply, by the index the generator draws: none at all,
/// the empty list, and lists of one, two and three entries — including an order no sort
/// would produce and the scheme forms a `ResourceOauth2ReturnUrlListType` entry takes
/// (https, a custom scheme, loopback http). A function rather than a const table because
/// the rows differ in length.
fn url_list(choice: usize) -> Option<Vec<&'static str>> {
    match choice {
        0 => None,
        1 => Some(vec![]),
        2 => Some(vec!["https://example.com/oauth2/callback"]),
        3 => Some(vec![
            "https://example.com/zulu",
            "http://localhost:8080/return",
            "myapp://auth/return",
        ]),
        _ => Some(vec!["myapp://auth/return", "https://example.com/alpha"]),
    }
}

/// How many candidate lists [`url_list`] offers.
const URL_LIST_COUNT: usize = 5;

/// The names the program mutates: each character class `WorkloadIdentityNameType`'s
/// pattern admits (`^[A-Za-z0-9_.-]+$`) and both modelled length bounds (3..255), so the
/// program's refusals come from the handlers rather than from the boundary validator.
fn name_pool() -> Vec<String> {
    vec![
        "a".repeat(NAME_MIN_LENGTH),
        "payments.service_v1-east".to_string(),
        "20261030".to_string(),
        "MixedCase_Identity".to_string(),
        "z".repeat(NAME_MAX_LENGTH),
    ]
}

/// Walk a generated program of interleaved mutations, checking the model against the
/// service after every step.
#[tokio::test]
async fn every_read_reports_the_record_the_last_mutation_left() {
    let router = router();
    let names = name_pool();
    let mut rng = SplitMix64(PROGRAM_SEED);
    let mut model: BTreeMap<String, Expected> = BTreeMap::new();

    for index in 0..PROGRAM_STEPS {
        let name = names[rng.below(names.len())].clone();
        let urls = url_list(rng.below(URL_LIST_COUNT));
        let step = match rng.below(10) {
            0..=3 => Step::Create,
            4..=7 => Step::Update,
            _ => Step::Delete,
        };
        let case = format!(
            "seed {PROGRAM_SEED:#x} step {index} {step:?} name {name:?} urls {urls:?}"
        );

        match step {
            Step::Create => {
                let mut body = json!({"name": name});
                if let Some(urls) = &urls {
                    body["allowedResourceOauth2ReturnUrls"] = json!(urls);
                }
                let (status, answer) = create_identity(&router, &body).await;
                if model.contains_key(&name) {
                    // A name this account already holds is refused: `name` is "unique
                    // within your account", and the refusal must not overwrite the record
                    // the sweep below then re-checks.
                    assert_eq!(
                        (status, error_code(&answer).as_str()),
                        (400, "ValidationException"),
                        "case {case:?}: a name already held is refused, got {status} {} ({answer})",
                        error_code(&answer)
                    );
                } else {
                    assert_eq!(
                        status, CREATE_STATUS,
                        "case {case:?}: expected HTTP {CREATE_STATUS}, got {status} ({answer})"
                    );
                    assert_eq!(
                        answer["allowedResourceOauth2ReturnUrls"],
                        expected_url_list(urls.as_ref()),
                        "case {case:?}: the create reports the list it was given, got {}",
                        answer["allowedResourceOauth2ReturnUrls"]
                    );
                    model.insert(
                        name.clone(),
                        Expected {
                            urls: urls.clone(),
                            created_time: None,
                        },
                    );
                }
            }
            Step::Update => {
                let (status, answer) = update_identity(&router, &name, urls.as_ref()).await;
                match model.get_mut(&name) {
                    Some(held) => {
                        assert_eq!(
                            status, UPDATE_STATUS,
                            "case {case:?}: expected HTTP {UPDATE_STATUS}, got {status} ({answer})"
                        );
                        assert_eq!(
                            answer["allowedResourceOauth2ReturnUrls"],
                            expected_url_list(urls.as_ref()),
                            "case {case:?}: the update reports exactly the list it replaced \
                             with, got {}",
                            answer["allowedResourceOauth2ReturnUrls"]
                        );
                        held.urls = urls.clone();
                    }
                    None => assert_not_found(&case, status, &answer, &not_found_by_id(&name)),
                }
            }
            Step::Delete => {
                let (status, answer) = delete_identity(&router, &name).await;
                if model.remove(&name).is_some() {
                    assert_eq!(
                        status, DELETE_STATUS,
                        "case {case:?}: expected HTTP {DELETE_STATUS}, got {status} ({answer})"
                    );
                    assert!(
                        carries_no_member(&answer),
                        "case {case:?}: a successful delete carries no member, got {answer}"
                    );
                } else {
                    assert_not_found(&case, status, &answer, &not_found_by_name(&name));
                }
            }
        }

        // The sweep: every pooled name is read back and compared with the model, so a
        // mutation that wrote the wrong record, wrote nothing, or wrote through to another
        // identity is caught on the step that issued it.
        for probe in &names {
            let (status, read) = get_identity(&router, probe).await;
            let probe_case = format!("{case} / read {probe:?}");
            match model.get_mut(probe) {
                Some(held) => {
                    assert_eq!(
                        status, READ_STATUS,
                        "case {probe_case:?}: expected HTTP {READ_STATUS} for a held identity, \
                         got {status} ({read})"
                    );
                    assert_eq!(
                        read["allowedResourceOauth2ReturnUrls"],
                        expected_url_list(held.urls.as_ref()),
                        "case {probe_case:?}: expected the list the last mutation left ({:?}), \
                         got {}",
                        held.urls,
                        read["allowedResourceOauth2ReturnUrls"]
                    );
                    assert_eq!(
                        read["workloadIdentityArn"],
                        json!(expected_arn(probe)),
                        "case {probe_case:?}: a held identity keeps its ARN, got {}",
                        read["workloadIdentityArn"]
                    );
                    let created = instant(&probe_case, &read, "createdTime");
                    match held.created_time {
                        None => held.created_time = Some(created),
                        Some(first) => assert_eq!(
                            created, first,
                            "case {probe_case:?}: no mutation moves createdTime, got {created} \
                             for {first}"
                        ),
                    }
                    assert!(
                        instant(&probe_case, &read, "lastUpdatedTime") >= created,
                        "case {probe_case:?}: expected lastUpdatedTime at or after createdTime \
                         {created}, got {}",
                        instant(&probe_case, &read, "lastUpdatedTime")
                    );
                }
                None => assert_not_found(&probe_case, status, &read, &not_found_by_id(probe)),
            }
        }
    }

    // The program ended with a store the listing reports exactly: every name the model
    // still holds, and none a delete removed. The pool is smaller than the modelled
    // `maxResults` maximum, so one page holds it.
    let (status, page) = list_identities(&router, Some(MAX_RESULTS_MAX), None).await;
    let expected: Vec<String> = model.keys().cloned().collect();
    assert_eq!(
        (status, listed_names(&page)),
        (200, expected.clone()),
        "case \"the listing after the whole program (seed {PROGRAM_SEED:#x})\": expected \
         {expected:?}, got {status} ({page})"
    );

    // The program exercised all three mutations and both outcomes of a read: a walk that
    // never deleted anything, or never left a name missing, would pass the sweep above
    // without testing what this file is about.
    assert!(
        !expected.is_empty() && expected.len() < names.len(),
        "case \"the program's coverage (seed {PROGRAM_SEED:#x})\": expected the walk to end \
         with some names held and some removed, got {expected:?} of {names:?}"
    );
}
