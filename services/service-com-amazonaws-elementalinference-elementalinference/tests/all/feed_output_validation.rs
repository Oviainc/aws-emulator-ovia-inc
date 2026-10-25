// SPDX-License-Identifier: CC0-1.0

//! The content checks a feed's OUTPUT LIST is held to, end to end over the wire.
//!
//! Every case here drives the real request path — the router picks the service from the
//! SigV4 credential scope, the framework deserializes and validates the body, the handler
//! runs, and the response is serialized as `restJson1` — so what these tests assert is
//! what a client sees.
//!
//! THE RULES under test, each a property of the feed the request would leave behind
//! rather than of the operation that carried the list:
//!
//! * a feed HOLDS at least one output ("The key contents of the feed is an array of
//!   outputs"; `CreateFeedRequest$outputs`: "You must specify at least one output"), so an
//!   operation that SETS the whole list — `CreateFeed` and `UpdateFeed` — refuses an empty
//!   one with the modelled `ValidationException` (HTTP 400) and stores nothing.
//!   `AssociateFeed` is not one of them: it appends to the stored list, so a call that
//!   appends nothing cannot empty a feed;
//! * an output NAMES another resource through its config — a dictionary
//!   (`SubtitlingConfig$dictionary`) or a fixture of the data source
//!   (`DataSourceConfiguration$fixtureId`) — and the service keeps that relation in both
//!   directions (`GetDictionary$references`, and `DeleteDictionary`'s refusal while a feed
//!   references the dictionary), so an id that resolves to no record is refused (HTTP 400)
//!   by every operation that writes outputs into a feed, and one that resolves is stored
//!   and read back;
//! * an output's `name` identifies it within its feed, so a list naming one output twice
//!   is refused with the modelled `ConflictException` (HTTP 409) by those same operations.
//!
//! Each case is a TABLE whose rows differ along what the rule generalises over — the
//! optional members the request carries, how many outputs the stored feed holds and
//! whether a resource holds it, the kind of reference and the form and length of the
//! identifier, and which operation carried the list — so an implementation that enforced
//! a rule on one operation, or for one shape of input, would answer rows alike that the
//! rule says must differ.

use crate::harness::{
    call, call_in_region, held_fixture_id, message_of, output_config, past_the_transition_span,
    router, subtitling_output, REGION,
};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::{
    cannot_find_dictionary_message, cannot_find_fixture_message, duplicate_output_name_message,
    MISSING_OUTPUTS_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

/// The operations that write a feed's outputs, as the wire reaches them: the two that SET
/// the whole list and the one that appends to it (`OPERATIONS.md`).
const WRITERS: [&str; 4] = ["create", "update", "associate", "associate-dry-run"];

/// An IAM role ARN in the account this emulator serves, satisfying `IamRoleArn`
/// (len 32..255, `^arn:aws[a-z\-]*:iam::[0-9]{12}:role/.+$`): a role this service may be
/// asked to pass, so a request carrying it is not refused for the role.
fn own_account_role() -> String {
    format!(
        "arn:aws:iam::{}:role/elemental-inference-access",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// `POST /v1/feed`.
async fn create(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", "/v1/feed", Some(body)).await
}

/// `PUT /v1/feed/{id}` with `outputs` as the replacement list, keeping the feed's name.
async fn update(
    router: &MultiServiceRouter,
    id: &str,
    name: &str,
    outputs: Value,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/v1/feed/{id}"),
        Some(json!({"name": name, "outputs": outputs})),
    )
    .await
}

/// `POST /v1/feed/{id}/associate` with `outputs` to append.
async fn associate(
    router: &MultiServiceRouter,
    id: &str,
    outputs: Value,
    dry_run: bool,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/v1/feed/{id}/associate"),
        Some(json!({
            "associatedResourceName": "source-media",
            "outputs": outputs,
            "dryRun": dry_run,
        })),
    )
    .await
}

/// `GET /v1/feed/{id}`, asserting the read itself succeeded.
async fn get_feed(router: &MultiServiceRouter, label: &str, id: &str) -> Value {
    let (status, feed) = call(router, "GET", &format!("/v1/feed/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {label:?}: GetFeed({id}) answers 200, got {status} with {feed}"
    );
    feed
}

/// Create a feed named `name` holding `outputs`, and return its id.
async fn feed_holding(router: &MultiServiceRouter, name: &str, outputs: Value) -> String {
    let (status, created) = create(router, json!({"name": name, "outputs": outputs})).await;
    assert_eq!(
        status, 202,
        "CreateFeed({name}) with a list the rules admit answers 202, got {status} with \
         {created}"
    );
    created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("a created feed carries an id, got {created}"))
        .to_string()
}

/// Create a dictionary through `region`'s endpoint and return its id.
async fn dictionary_in(router: &MultiServiceRouter, region: &str, name: &str) -> String {
    let (status, created) = call_in_region(
        router,
        region,
        "POST",
        "/v1/dictionary",
        Some(json!({"name": name, "language": "eng"})),
    )
    .await;
    assert_eq!(
        status, 202,
        "CreateDictionary({name}) in {region} answers 202, got {status} with {created}"
    );
    created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("a created dictionary carries an id, got {created}"))
        .to_string()
}

/// The ids `ListFeeds` reports for this endpoint, sorted.
async fn feed_ids(router: &MultiServiceRouter) -> Vec<String> {
    let (status, page) = call(router, "GET", "/v1/feeds?maxResults=100", None).await;
    assert_eq!(
        status, 200,
        "ListFeeds answers 200, got {status} with {page}"
    );
    let mut ids: Vec<String> = page["feeds"]
        .as_array()
        .unwrap_or_else(|| panic!("ListFeeds reports a feeds array, got {page}"))
        .iter()
        .map(|summary| {
            summary["id"]
                .as_str()
                .expect("a feed summary carries an id")
                .to_string()
        })
        .collect();
    ids.sort();
    ids
}

/// The `outputs` list of a feed response.
fn outputs_of(feed: &Value) -> Vec<Value> {
    feed["outputs"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("a feed response carries an outputs array, got {feed}"))
}

/// The message of a refusal, after asserting its status and its error code.
fn refusal(label: &str, answer: (u16, Value), status: u16, kind: &str) -> String {
    let (actual, body) = answer;
    assert_eq!(
        actual, status,
        "case {label:?}: expected HTTP {status}, got {actual} with {body}"
    );
    assert_eq!(
        body["__type"],
        json!(kind),
        "case {label:?}: expected {kind}, got {body}"
    );
    message_of(&body).to_string()
}

/// One output named `name` whose clipping config names `fixture_id` as its data source
/// (`DataSourceConfiguration$fixtureId`).
fn clipping_output(name: &str, fixture_id: &str) -> Value {
    json!({
        "name": name,
        "status": "ENABLED",
        "outputConfig": {"clipping": {
            "callbackMetadata": "season opener",
            "dataSourceConfiguration": {"fixtureId": fixture_id}
        }},
    })
}

/// `CreateFeed` with an empty `outputs` is refused whatever else the request carries, and
/// mints nothing.
#[tokio::test]
async fn create_feed_refuses_an_output_list_with_no_outputs() {
    let router = router();

    // Rows differ in the OPTIONAL members the request carries, each of which is judged by
    // a rule of its own: a request that satisfies every other rule is still refused for
    // the list alone.
    let rows: Vec<(&str, Value)> = vec![
        (
            "the required members alone",
            json!({"name": "no-outputs-feed", "outputs": []}),
        ),
        (
            "with tags the service accepts",
            json!({"name": "tagged-no-outputs", "outputs": [], "tags": {"team": "video"}}),
        ),
        (
            "with an access role in this account",
            json!({
                "name": "roled-no-outputs",
                "outputs": [],
                "accessRoleArn": own_account_role(),
            }),
        ),
    ];

    for (label, body) in rows {
        let message = refusal(label, create(&router, body).await, 400, "ValidationException");
        assert_eq!(
            message, MISSING_OUTPUTS_MESSAGE,
            "case {label:?}: expected {MISSING_OUTPUTS_MESSAGE:?}, got {message:?}"
        );
        assert_eq!(
            feed_ids(&router).await,
            Vec::<String>::new(),
            "case {label:?}: a refused create mints no feed"
        );
    }
}

/// `UpdateFeed` with an empty `outputs` is refused whatever the stored feed holds, and
/// leaves it exactly as it was.
#[tokio::test]
async fn update_feed_refuses_an_empty_output_list_and_changes_nothing() {
    // Rows differ in the stored feed the PUT would empty: how many outputs it holds, and
    // whether a resource holds the feed.
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "a feed holding one output",
            json!([{"name": "crop1", "outputConfig": output_config("cropping"),
                    "status": "ENABLED"}]),
            false,
        ),
        (
            "a feed holding three outputs",
            json!([
                {"name": "crop1", "outputConfig": output_config("cropping"), "status": "ENABLED"},
                {"name": "clip1", "outputConfig": output_config("clipping"), "status": "ENABLED"},
                {"name": "meta1", "outputConfig": output_config("contextualMetadata"),
                 "status": "DISABLED"},
            ]),
            false,
        ),
        (
            "a feed a resource holds",
            json!([{"name": "subs1", "outputConfig": output_config("subtitling"),
                    "status": "ENABLED"}]),
            true,
        ),
    ];

    for (label, held, associated) in rows {
        let router = router();
        let id = feed_holding(&router, "feed-to-empty", held).await;
        if associated {
            let (status, claimed) = associate(&router, &id, json!([]), false).await;
            assert_eq!(
                status, 200,
                "case {label:?}: the association answers 200, got {status} with {claimed}"
            );
        }
        let before = get_feed(&router, label, &id).await;

        let message = refusal(
            label,
            update(&router, &id, "feed-to-empty", json!([])).await,
            400,
            "ValidationException",
        );
        assert_eq!(
            message, MISSING_OUTPUTS_MESSAGE,
            "case {label:?}: expected {MISSING_OUTPUTS_MESSAGE:?}, got {message:?}"
        );

        // A refused PUT writes nothing — not the outputs, and not the status a real
        // reconfiguration would have moved the feed into.
        let after = get_feed(&router, label, &id).await;
        assert_eq!(
            outputs_of(&after),
            outputs_of(&before),
            "case {label:?}: the refused update left the outputs as they were"
        );
        assert_eq!(
            after["status"], before["status"],
            "case {label:?}: the refused update left the status as it was, got {after}"
        );
    }
}

/// One row of the reference table: an output naming a resource that resolves to nothing,
/// and the refusal it earns.
struct ReferenceCase {
    label: String,
    /// The identifier the output names, which the refusal has to report.
    id: String,
    output: Value,
    expected: String,
}

/// Every operation that writes a feed's outputs refuses an output whose reference
/// resolves to nothing, and stores nothing.
#[tokio::test]
async fn an_output_reference_that_resolves_to_nothing_is_refused_by_every_writer() {
    let router = router();
    // A dictionary of ANOTHER Region: this service is regional, so its id names no
    // dictionary at the endpoint under test even though the store holds the record.
    let other_region_dictionary = dictionary_in(&router, "eu-west-1", "otherregiondict").await;
    let held_fixture = held_fixture_id();
    // A near miss on a held fixture's id: the same length and form, one character apart.
    let near_miss = {
        let mut bytes = held_fixture.clone().into_bytes();
        bytes[0] = if bytes[0] == b'z' { b'y' } else { b'z' };
        String::from_utf8(bytes).expect("ASCII stays UTF-8")
    };

    // Dictionary ids that satisfy `DictionaryId` (len 1..19, `^[a-zA-Z0-9]+$`), so the
    // boundary admits them and the handler is what refuses them: they differ in length,
    // in alphabet, and in whether a record with that id exists in ANOTHER Region.
    let unknown_dictionaries = [
        "doesnotexist1234567".to_string(),
        "a".to_string(),
        "Zz9".to_string(),
        other_region_dictionary,
    ];
    // Fixture ids that satisfy `FixtureId` (len 1..128), differing in form and length:
    // free text, a single digit, the modelled maximum, and a near miss on the form the
    // data source's own ids take.
    let unknown_fixtures = [
        "no-such-fixture".to_string(),
        "0".to_string(),
        "a".repeat(128),
        near_miss,
    ];

    let mut cases: Vec<ReferenceCase> = Vec::new();
    for id in unknown_dictionaries {
        cases.push(ReferenceCase {
            label: format!("a subtitling output naming dictionary {id:?}"),
            output: subtitling_output("subs1", Some(&id)),
            expected: cannot_find_dictionary_message(&id),
            id,
        });
    }
    for id in unknown_fixtures {
        cases.push(ReferenceCase {
            label: format!("a clipping output naming fixture {id:?}"),
            output: clipping_output("clip1", &id),
            expected: cannot_find_fixture_message(&id),
            id,
        });
    }

    let mut messages: Vec<String> = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        for writer in WRITERS {
            let label = format!("{} through {writer}", case.label);
            // The feed the writers that need one operate on, holding an output whose
            // references all resolve.
            let name = format!("base-{index}-{}", writer.replace('-', ""));
            let id = feed_holding(
                &router,
                &name,
                json!([{"name": "crop1", "outputConfig": output_config("cropping"),
                        "status": "ENABLED"}]),
            )
            .await;
            let before = get_feed(&router, &label, &id).await;
            let listed = feed_ids(&router).await;

            let answer = match writer {
                "create" => {
                    create(
                        &router,
                        json!({"name": format!("ref-{index}"), "outputs": [case.output.clone()]}),
                    )
                    .await
                }
                "update" => update(&router, &id, &name, json!([case.output.clone()])).await,
                "associate" => associate(&router, &id, json!([case.output.clone()]), false).await,
                "associate-dry-run" => {
                    associate(&router, &id, json!([case.output.clone()]), true).await
                }
                other => panic!("no such writer: {other}"),
            };

            let message = refusal(&label, answer, 400, "ValidationException");
            assert_eq!(
                message, case.expected,
                "case {label:?}: expected {:?}, got {message:?}",
                case.expected
            );
            assert!(
                message.contains(case.id.as_str()),
                "case {label:?}: the refusal names the identifier it could not resolve, got \
                 {message:?}"
            );

            // Nothing was stored: no feed was minted, and the feed the writer named holds
            // the outputs and the status it held before.
            assert_eq!(
                feed_ids(&router).await,
                listed,
                "case {label:?}: a refused writer mints no feed"
            );
            let after = get_feed(&router, &label, &id).await;
            assert_eq!(
                outputs_of(&after),
                outputs_of(&before),
                "case {label:?}: the refused writer left the outputs as they were"
            );
            assert_eq!(
                after["status"], before["status"],
                "case {label:?}: the refused writer left the status as it was, got {after}"
            );
            messages.push(message);
        }
    }

    // The rule is about ONE value the request supplied, so the refusal is built from the
    // request: no two identifiers earn the same sentence.
    let mut distinct = messages.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        cases.len(),
        "each identifier earns its own message, got {messages:?}"
    );
}

/// References that DO resolve are stored by each writer and reported by a read back —
/// and the dictionary's own view of the relation agrees.
#[tokio::test]
async fn an_output_reference_that_resolves_is_stored_and_read_back() {
    let router = router();
    // A dictionary of the endpoint under test, and a fixture the data source holds. A
    // client learns a `fixtureId` from `SearchFixtures`; `GetFixture` answering for it is
    // what makes it one the catalog really holds.
    let dictionary = dictionary_in(&router, REGION, "liveeventdict").await;
    let fixture = held_fixture_id();
    let (status, found) = call(&router, "GET", &format!("/v1/fixtures/{fixture}"), None).await;
    assert_eq!(
        status, 200,
        "the data source holds fixture {fixture}, got {status} with {found}"
    );
    past_the_transition_span();

    // CreateFeed: both kinds of reference at once.
    let (status, created) = create(
        &router,
        json!({
            "name": "referencing-feed",
            "outputs": [
                subtitling_output("subs1", Some(&dictionary)),
                clipping_output("clip1", &fixture),
            ],
        }),
    )
    .await;
    assert_eq!(
        status, 202,
        "a create whose references resolve answers 202, got {status} with {created}"
    );
    let id = created["id"].as_str().expect("the feed carries an id");

    let feed = get_feed(&router, "create", id).await;
    let stored = outputs_of(&feed);
    assert_eq!(
        stored[0]["outputConfig"]["subtitling"]["dictionary"],
        json!(dictionary),
        "case \"create\": the stored output reports the dictionary it names, got {feed}"
    );
    assert_eq!(
        stored[1]["outputConfig"]["clipping"]["dataSourceConfiguration"]["fixtureId"],
        json!(fixture),
        "case \"create\": the stored output reports the fixture it names, got {feed}"
    );

    // The relation's other direction: the dictionary reports the referencing feed.
    let (status, dictionary_read) =
        call(&router, "GET", &format!("/v1/dictionary/{dictionary}"), None).await;
    assert_eq!(
        status, 200,
        "GetDictionary answers 200, got {status} with {dictionary_read}"
    );
    assert_eq!(
        dictionary_read["references"],
        json!([id]),
        "case \"create\": the dictionary reports the feed that names it, got \
         {dictionary_read}"
    );

    // UpdateFeed: a replacement list whose references resolve is stored.
    let (status, updated) = update(
        &router,
        id,
        "referencing-feed",
        json!([clipping_output("clip2", &fixture)]),
    )
    .await;
    assert_eq!(
        status, 200,
        "an update whose references resolve answers 200, got {status} with {updated}"
    );
    past_the_transition_span();
    let feed = get_feed(&router, "update", id).await;
    assert_eq!(
        outputs_of(&feed)
            .iter()
            .map(|output| output["name"].clone())
            .collect::<Vec<Value>>(),
        vec![json!("clip2")],
        "case \"update\": the replacement list is what the feed holds, got {feed}"
    );

    // AssociateFeed: an appended output whose reference resolves is appended.
    let (status, claimed) = associate(
        &router,
        id,
        json!([subtitling_output("subs2", Some(&dictionary))]),
        false,
    )
    .await;
    assert_eq!(
        status, 200,
        "an append whose references resolve answers 200, got {status} with {claimed}"
    );
    let feed = get_feed(&router, "associate", id).await;
    assert_eq!(
        outputs_of(&feed)
            .iter()
            .map(|output| output["name"].clone())
            .collect::<Vec<Value>>(),
        vec![json!("clip2"), json!("subs2")],
        "case \"associate\": the appended output joins the stored list, got {feed}"
    );
}

/// Every operation that writes a feed's outputs refuses a list that names one output
/// twice: an output's name identifies it within its feed.
#[tokio::test]
async fn a_list_naming_one_output_twice_is_refused_by_every_writer() {
    // Rows differ in HOW the name repeats: twice over with the same definition, twice
    // with different configs, and as the last of a longer list.
    let rows: Vec<(&str, Value)> = vec![
        (
            "the same output twice",
            json!([
                {"name": "dup1", "outputConfig": output_config("cropping"), "status": "ENABLED"},
                {"name": "dup1", "outputConfig": output_config("cropping"), "status": "ENABLED"},
            ]),
        ),
        (
            "one name over two different configs",
            json!([
                {"name": "dup1", "outputConfig": output_config("subtitling"), "status": "ENABLED"},
                {"name": "dup1", "outputConfig": output_config("clipping"), "status": "DISABLED"},
            ]),
        ),
        (
            "the repeat is the last of three",
            json!([
                {"name": "dup1", "outputConfig": output_config("cropping"), "status": "ENABLED"},
                {"name": "other1", "outputConfig": output_config("clipping"), "status": "ENABLED"},
                {"name": "dup1", "outputConfig": output_config("contextualMetadata"),
                 "status": "ENABLED"},
            ]),
        ),
    ];

    for (row, outputs) in rows {
        for writer in WRITERS {
            let label = format!("{row} through {writer}");
            let router = router();
            let id = feed_holding(
                &router,
                "duplicate-target",
                json!([{"name": "crop1", "outputConfig": output_config("cropping"),
                        "status": "ENABLED"}]),
            )
            .await;

            let answer = match writer {
                "create" => {
                    create(
                        &router,
                        json!({"name": "duplicate-feed", "outputs": outputs.clone()}),
                    )
                    .await
                }
                "update" => update(&router, &id, "duplicate-target", outputs.clone()).await,
                "associate" => associate(&router, &id, outputs.clone(), false).await,
                "associate-dry-run" => associate(&router, &id, outputs.clone(), true).await,
                other => panic!("no such writer: {other}"),
            };

            let message = refusal(&label, answer, 409, "ConflictException");
            let expected = duplicate_output_name_message("dup1");
            assert_eq!(
                message, expected,
                "case {label:?}: expected {expected:?}, got {message:?}"
            );
            let feed = get_feed(&router, &label, &id).await;
            assert_eq!(
                outputs_of(&feed).len(),
                1,
                "case {label:?}: the refused writer left the stored list alone, got {feed}"
            );
        }
    }
}
