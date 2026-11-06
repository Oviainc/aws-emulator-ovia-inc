// SPDX-License-Identifier: CC0-1.0

//! The id a create names its new resource with has to be an id this service can address
//! the resource BY afterwards.
//!
//! The rule under test: a create that NAMES the resource it stores is accepted only when
//! that id is at least `state::MIN_ID_CHARS` character and every character is drawn from
//! the identifier alphabet (ASCII letters, digits, `_` and `-`); any other value is
//! refused with `UnprocessableEntityException` (HTTP 422,
//! `smithy_query(query="operation",operation="CreateChannel,CreateOriginEndpoint,CreateHarvestJob")`)
//! keyed by the `id` member and stating the requirement, and nothing is stored. The id is
//! the only handle the record has: it is the `{Id}` http label of that resource's own
//! reads (`GET /channels/{Id}`, `GET /origin_endpoints/{Id}`, `GET /harvest_jobs/{Id}`)
//! and the last segment of its ARN, so a record keyed by anything else would be listed
//! forever and reachable by nothing.
//!
//! The rule is the service's own, not a modelled constraint: the v1 model targets every
//! one of those `Id` members at the bare `__string` and publishes no `@pattern` or
//! `@length`, so the framework's constraint walk cannot refuse any of it. The requirement
//! sentence and the floor are read off the service rather than restated, so the suite
//! cannot drift from what it enforces; the accepted CHARACTER SET is written out here (its
//! source is the MediaPackage v2 model, which pins the same identifier as
//! `^[a-zA-Z0-9_-]+$`) so the oracle below is independent of the service's own set.
//!
//! The rows differ along what the rule generalises over: WHICH create carries the id (all
//! three of them), the id's length (the shortest there is, middling, long), which
//! character classes it draws on (digits, upper and lower case, each punctuation
//! character, every mixture), and — for the refusals — which character outside the
//! alphabet it carries (whitespace, each path and URI metacharacter, a non-ASCII letter)
//! as well as the empty string. The last suite is a property test over a seeded alphabet
//! mixing admitted and rejected characters.

use serde_json::{json, Value};
use service_com_amazonaws_mediapackage_mediapackage::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::{
    call, create_channel, emulator, error_code, SeededGenerator, ACCOUNT,
    CROSS_ACCOUNT_HARVEST_ROLE, REGION,
};

/// The floor the service publishes for an id, so the empty string is no id at all.
const MIN_CHARS: usize = state::MIN_ID_CHARS;

/// The channel every origin endpoint below packages. Its own id satisfies the rule, so the
/// fixture cannot be what a case is refused for.
const FIXTURE_CHANNEL: &str = "fixture-channel";

/// The origin endpoint every harvest job below harvests. It retains a startover window,
/// because that is the content a harvest exports.
const FIXTURE_ENDPOINT: &str = "fixture-endpoint";

/// The resources whose create NAMES the record it stores — the three ids that are each the
/// http label of their own resource's reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Created {
    Channel,
    OriginEndpoint,
    HarvestJob,
}

impl Created {
    /// Every kind the rule holds for.
    const ALL: &'static [Created] = &[Self::Channel, Self::OriginEndpoint, Self::HarvestJob];

    /// The collection URI the create posts to, and that the resource's reads hang off.
    fn collection(self) -> &'static str {
        match self {
            Self::Channel => "/channels",
            Self::OriginEndpoint => "/origin_endpoints",
            Self::HarvestJob => "/harvest_jobs",
        }
    }

    /// The member a listing of this collection reports its entries in.
    fn list_member(self) -> &'static str {
        match self {
            Self::Channel => "channels",
            Self::OriginEndpoint => "originEndpoints",
            Self::HarvestJob => "harvestJobs",
        }
    }

    /// The ARN segment this service mints an id of this kind into.
    fn arn_type(self) -> &'static str {
        match self {
            Self::Channel => "channels",
            Self::OriginEndpoint => "origin_endpoints",
            Self::HarvestJob => "harvest_jobs",
        }
    }

    /// Whether a read of this resource renders exactly what its create answered. A harvest
    /// job's status is moved by the CLOCK after it is submitted, so only its identity —
    /// its id and the ARN minted from it — is stable across reads.
    fn renders_create_response(self) -> bool {
        self != Self::HarvestJob
    }

    /// A create body for this kind naming `id`, with every other member a value the
    /// service accepts, so the id is the only thing under test.
    fn create_body(self, id: &str) -> Value {
        match self {
            Self::Channel => json!({ "id": id }),
            Self::OriginEndpoint => json!({
                "channelId": FIXTURE_CHANNEL,
                "id": id,
                "hlsPackage": {}
            }),
            Self::HarvestJob => json!({
                "id": id,
                "originEndpointId": FIXTURE_ENDPOINT,
                "startTime": "2026-01-01T00:00:00Z",
                "endTime": "2026-01-01T01:00:00Z",
                "s3Destination": {
                    "bucketName": "harvest-bucket",
                    "manifestKey": "index.m3u8",
                    "roleArn": CROSS_ACCOUNT_HARVEST_ROLE
                }
            }),
        }
    }
}

/// `arn:aws:mediapackage:<region>:<account>:<type>/<id>`, as this service mints it.
fn expected_arn(kind: Created, id: &str) -> String {
    format!(
        "arn:aws:mediapackage:{REGION}:{ACCOUNT}:{}/{id}",
        kind.arn_type()
    )
}

/// Ids a resource can be addressed by, so this service must store resources under them.
///
/// Source of the character set: the MediaPackage v2 model's identifier pattern
/// `^[a-zA-Z0-9_-]+$` (letters, digits, `_`, `-`). One row per class the rule generalises
/// over — including rows the issue did not cite — and never two rows that differ only by
/// being a different spelling of the same class.
fn accepted_ids() -> Vec<String> {
    vec![
        // The shortest id there is, in each character class.
        "a".repeat(MIN_CHARS),
        "7".to_string(),
        "Z".to_string(),
        // Each punctuation character of the alphabet, alone and doubled.
        "_".to_string(),
        "-".to_string(),
        "__".to_string(),
        "-_-".to_string(),
        // Mixed classes and lengths.
        "0123456789".to_string(),
        "Mixed_Case-99".to_string(),
        "live-eu_west_1-0".to_string(),
        "x".repeat(64),
        "L".repeat(256),
    ]
}

/// Ids this service cannot address a resource by, so it must refuse them.
///
/// The rows span both halves of the rule: the floor (the empty string) and the character
/// set — whitespace in every position, each character that would have to be
/// percent-encoded to survive the `{Id}` label or that splits the path outright, and a
/// non-ASCII letter.
fn refused_ids() -> Vec<String> {
    vec![
        String::new(),
        " ".to_string(),
        "   ".to_string(),
        "\t".to_string(),
        "\n".to_string(),
        " leading".to_string(),
        "trailing ".to_string(),
        "has a space".to_string(),
        "with/slash".to_string(),
        "/leading-slash".to_string(),
        "a/../b".to_string(),
        "with.dot".to_string(),
        "with:colon".to_string(),
        "with%20escape".to_string(),
        "with+plus".to_string(),
        "with*star".to_string(),
        "with?query".to_string(),
        "with#fragment".to_string(),
        "with&ampersand".to_string(),
        "with=equals".to_string(),
        "with@at".to_string(),
        "with,comma".to_string(),
        "with\\backslash".to_string(),
        "with\"quote".to_string(),
        "with'apostrophe".to_string(),
        "with{brace}".to_string(),
        "grüße".to_string(),
        "日本語".to_string(),
        format!("arn:aws:mediapackage:{REGION}:{ACCOUNT}:channels/smuggled"),
    ]
}

/// A short, readable name for a case: the kind, the id's length, and its first characters
/// with the invisible ones shown as escapes.
fn case_name(kind: Created, id: &str) -> String {
    format!(
        "{kind:?} with an id of {} characters: {:?}",
        id.chars().count(),
        id.chars().take(32).collect::<String>()
    )
}

/// Create the channel and the origin endpoint the endpoint and harvest-job cases hang off.
async fn seed_fixtures(router: &MultiServiceRouter) {
    create_channel(router, FIXTURE_CHANNEL).await;
    let (status, body) = call(
        router,
        "POST",
        "/origin_endpoints",
        Some(&json!({
            "channelId": FIXTURE_CHANNEL,
            "id": FIXTURE_ENDPOINT,
            "hlsPackage": {},
            // A harvest exports content the endpoint retains, so the fixture keeps a
            // startover window; `tests/all/harvest_startover.rs` is the suite for that rule.
            "startoverWindowSeconds": 3_600
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "seeding the fixture endpoint expected 200, got {status} {body:?}"
    );
}

/// The ids a listing of `kind` reports.
async fn listed_ids(router: &MultiServiceRouter, kind: Created) -> Vec<String> {
    let (status, body) = call(router, "GET", kind.collection(), None).await;
    assert_eq!(
        status, 200,
        "listing {kind:?} expected 200, got {status} {body:?}"
    );
    body.get(kind.list_member())
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("id").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Assert `refusal` is this service's 422, keyed by the `id` member and stating
/// `requirement`.
fn assert_refused(case: &str, status: u16, refusal: &Value, requirement: &str) {
    assert_eq!(
        status, 422,
        "case {case:?}: expected 422, got {status} {refusal:?}"
    );
    assert_eq!(
        error_code(refusal),
        "UnprocessableEntityException",
        "case {case:?}: expected UnprocessableEntityException, got {refusal:?}"
    );
    assert_eq!(
        refusal.pointer(&format!("/errors/{}", state::ID_MEMBER)),
        Some(&json!(requirement)),
        "case {case:?}: the refusal must be keyed by {:?} and state the requirement, got \
         {refusal:?}",
        state::ID_MEMBER
    );
    assert_eq!(
        refusal.get("message").and_then(Value::as_str),
        Some(format!("{} {requirement}", state::ID_MEMBER).as_str()),
        "case {case:?}: the refusal's sentence must name the member and the requirement, got \
         {refusal:?}"
    );
}

/// Create `id` and read it back through the resource's own `{Id}` label, asserting the
/// record is addressable by the id it was stored under.
async fn assert_created_and_addressable(
    router: &MultiServiceRouter,
    case: &str,
    kind: Created,
    id: &str,
) {
    let (status, created) = call(
        router,
        "POST",
        kind.collection(),
        Some(&kind.create_body(id)),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the create expected 200, got {status} {created:?}"
    );
    assert_eq!(
        created.get("id").and_then(Value::as_str),
        Some(id),
        "case {case:?}: the create must report the id it stored, got {created:?}"
    );
    assert_eq!(
        created.get("arn").and_then(Value::as_str),
        Some(expected_arn(kind, id).as_str()),
        "case {case:?}: the ARN's last segment must be the id, got {created:?}"
    );

    // The id is the `{Id}` label of this resource's own read, so the stored record is
    // reachable by it.
    let (status, described) = call(
        router,
        "GET",
        &format!("{}/{id}", kind.collection()),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the read of {id:?} expected 200, got {status} {described:?}"
    );
    assert_eq!(
        described.get("id").and_then(Value::as_str),
        Some(id),
        "case {case:?}: the read must render the stored record, got {described:?}"
    );
    if kind.renders_create_response() {
        assert_eq!(
            described, created,
            "case {case:?}: the read must render exactly what the create stored, got \
             {described:?}"
        );
    }
    let listed = listed_ids(router, kind).await;
    assert!(
        listed.iter().any(|entry| entry == id),
        "case {case:?}: the listing must report the stored id, got {listed:?}"
    );
}

/// Assert the create of `id` is refused with `requirement` and that nothing was stored:
/// the listing is exactly what it was before the attempt.
async fn assert_refused_and_not_stored(
    router: &MultiServiceRouter,
    case: &str,
    kind: Created,
    id: &str,
    requirement: &str,
) {
    let before = listed_ids(router, kind).await;
    let (status, refusal) = call(
        router,
        "POST",
        kind.collection(),
        Some(&kind.create_body(id)),
    )
    .await;
    assert_refused(case, status, &refusal, requirement);
    let after = listed_ids(router, kind).await;
    assert_eq!(
        after, before,
        "case {case:?}: a refused create must store nothing, got {after:?}"
    );
    assert!(
        !after.iter().any(|entry| entry == id),
        "case {case:?}: the refused id must not be listed, got {after:?}"
    );
}

#[tokio::test]
async fn an_id_the_service_can_address_is_stored_and_read_back_on_every_create() {
    let router = emulator();
    seed_fixtures(&router).await;

    for kind in Created::ALL {
        for id in accepted_ids() {
            // One id per kind, so a channel and an endpoint sharing an id is not what any
            // row depends on.
            assert_created_and_addressable(&router, &case_name(*kind, &id), *kind, &id).await;
        }
    }
}

#[tokio::test]
async fn an_id_the_service_could_not_address_is_refused_by_every_create() {
    let router = emulator();
    seed_fixtures(&router).await;
    let requirement = state::resource_id_requirement();

    for kind in Created::ALL {
        for id in refused_ids() {
            assert_refused_and_not_stored(&router, &case_name(*kind, &id), *kind, &id, &requirement)
                .await;
        }
    }
}

#[tokio::test]
async fn an_id_past_the_size_bound_is_refused_by_its_own_requirement() {
    let router = emulator();
    seed_fixtures(&router).await;

    // An id of acceptable characters but past the service's string bound is refused too —
    // by the SIZE requirement rather than by the alphabet one, so the caller is told which
    // rule the value broke. The bound and its sentence are the service's own.
    let max = state::MAX_STRING_CHARS;
    for kind in Created::ALL {
        for length in [max, max + 1, max * 2] {
            let id = "y".repeat(length);
            let case = case_name(*kind, &id);
            if length <= max {
                assert_created_and_addressable(&router, &case, *kind, &id).await;
            } else {
                let requirement = format!("may be at most {max} characters, got {length}");
                assert_refused_and_not_stored(&router, &case, *kind, &id, &requirement).await;
            }
        }
    }
}

/// The characters an id may carry, written out from the MediaPackage v2 identifier pattern
/// `^[a-zA-Z0-9_-]+$` — the property test's own oracle, independent of the service's set.
fn admitted_by_the_identifier_pattern(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-')
}

/// The alphabet the generated ids are drawn from: admitted characters mixed with
/// characters outside the set, so a generated id lands either side of the rule.
const ALPHABET: &[char] = &[
    'a', 'q', 'Z', '0', '9', '_', '-', ' ', '/', '.', ':', '%', '+', '*', '#', '?', '\\', 'é',
];

#[tokio::test]
async fn acceptance_follows_the_identifier_alphabet_on_generated_ids() {
    let router = emulator();
    seed_fixtures(&router).await;
    let requirement = state::resource_id_requirement();
    let mut already_created: Vec<(Created, String)> = Vec::new();

    // A bounded number of generated ids per seed, each judged against the published
    // identifier domain rather than against the service's own answer: the invariant is
    // that the service stores exactly the ids it could address afterwards. A failing case
    // reproduces from the seed named in its assertion message.
    for (index, seed) in [7u64, 61, 2_027].into_iter().enumerate() {
        let mut rng = SeededGenerator::new(seed);
        // One kind per seed, so every create the rule holds for is generated against.
        let kind = Created::ALL[index % Created::ALL.len()];

        for case_index in 0..24u32 {
            // Lengths either side of the floor: the empty string, one character, and
            // short ids where a single rejected character decides the case.
            let length = match case_index % 4 {
                0 => rng.next_in(2) - 1,
                1 => rng.next_in(3),
                _ => rng.next_in(16),
            };
            let id: String = (0..length)
                .map(|_| ALPHABET[rng.next_in(ALPHABET.len() as u64) - 1])
                .collect();
            let acceptable = id.chars().count() >= MIN_CHARS
                && id.chars().all(admitted_by_the_identifier_pattern);
            let case = format!(
                "seed {seed}, case {case_index}: {}, acceptable={acceptable}",
                case_name(kind, &id)
            );

            if acceptable {
                // An id this suite already created would be refused as a duplicate, which
                // is a different rule (`tests/all/channel_lifecycle.rs` covers it).
                if already_created
                    .iter()
                    .any(|(stored_kind, stored)| *stored_kind == kind && stored == &id)
                {
                    continue;
                }
                assert_created_and_addressable(&router, &case, kind, &id).await;
                already_created.push((kind, id));
            } else {
                let (status, refusal) =
                    call(&router, "POST", kind.collection(), Some(&kind.create_body(&id))).await;
                assert_refused(&case, status, &refusal, &requirement);
                let listed = listed_ids(&router, kind).await;
                assert!(
                    !listed.iter().any(|entry| entry == &id),
                    "case {case:?}: a refused create must store nothing, got {listed:?}"
                );
            }
        }
    }
}
