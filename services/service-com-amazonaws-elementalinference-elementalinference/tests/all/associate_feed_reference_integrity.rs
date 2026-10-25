// SPDX-License-Identifier: CC0-1.0

//! What an APPEND may leave behind in a feed's output list, end to end over the wire.
//!
//! `AssociateFeed` is the operation that APPENDS to the stored list ("You can add more
//! outputs to the existing outputs. New outputs will be appended"), so it is a writer of
//! the references a stored feed carries, exactly as the two operations that set the whole
//! list are.
//!
//! THE RULE under test, a property of the feed the call would leave behind rather than of
//! the operation that carried the list: an output NAMES another resource through its
//! config — a dictionary (`SubtitlingConfig$dictionary`) or a fixture of the data source
//! (`DataSourceConfiguration$fixtureId`) — and the service maintains that relation in both
//! directions (`GetDictionary$references`, and `DeleteDictionary`'s refusal while a feed
//! references the dictionary), so an identifier that resolves to no record is refused with
//! the modelled `ValidationException` (HTTP 400) and the feed is left exactly as it was:
//! its outputs, its status and the resource that holds it all unchanged, and no resolving
//! output of the same list stored either.
//!
//! The cases here are a TABLE whose rows differ along what the rule generalises over for
//! an APPEND: which kind of reference resolves to nothing, WHERE in the appended list the
//! unresolvable one sits (alone, first, in the middle, last), how many outputs the append
//! carries, whether the feed already has a resource claiming it, and whether the call is a
//! dry run — so an implementation that checked only the first output of a list, only an
//! unclaimed feed, or only the real call would answer rows alike that the rule says must
//! differ. The closing case drives the same lists with every reference resolving, which is
//! what shows the refusals above are a RESOLUTION of each named id and not a blanket
//! refusal of the shape of the request.

use crate::harness::{
    call, clipping_output, enabled_output, held_fixture_id, message_of, router, subtitling_output,
    REGION,
};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::{
    cannot_find_dictionary_message, cannot_find_fixture_message,
};
use smithy_server_runtime::MultiServiceRouter;

/// The resource name every association below claims the feed for. A repeat association
/// names the SAME resource, so the feed's status precondition admits it and the appended
/// list is what each case is about.
const RESOURCE: &str = "source-media";

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
            "associatedResourceName": RESOURCE,
            "outputs": outputs,
            "dryRun": dry_run,
        })),
    )
    .await
}

/// `GET /v1/feed/{id}`, asserting the read itself succeeded.
async fn get_feed(router: &MultiServiceRouter, id: &str, label: &str) -> Value {
    let (status, feed) = call(router, "GET", &format!("/v1/feed/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {label:?}: GetFeed({id}) answers 200, got {status} with {feed}"
    );
    feed
}

/// `GET /v1/dictionary/{id}`, asserting the read itself succeeded.
async fn get_dictionary(router: &MultiServiceRouter, id: &str, label: &str) -> Value {
    let (status, dictionary) = call(router, "GET", &format!("/v1/dictionary/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {label:?}: GetDictionary({id}) answers 200, got {status} with {dictionary}"
    );
    dictionary
}

/// Create one feed holding a single cropping output — whose config names no other
/// resource, so the base feed carries no reference of its own — and return its id.
async fn base_feed(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/v1/feed",
        Some(json!({"name": name, "outputs": [enabled_output("crop1", "cropping")]})),
    )
    .await;
    assert_eq!(
        status, 202,
        "CreateFeed({name}) answers 202, got {status} with {created}"
    );
    created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("a created feed carries an id, got {created}"))
        .to_string()
}

/// Create one dictionary at the endpoint under test and return its id: an id a subtitling
/// output can name and have resolve.
async fn resolving_dictionary(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/v1/dictionary",
        Some(json!({"name": name, "language": "eng"})),
    )
    .await;
    assert_eq!(
        status, 202,
        "CreateDictionary({name}) in {REGION} answers 202, got {status} with {created}"
    );
    created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("a created dictionary carries an id, got {created}"))
        .to_string()
}

/// The `outputs` list of a feed response.
fn outputs_of(feed: &Value) -> Vec<Value> {
    feed["outputs"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("a feed response carries an outputs array, got {feed}"))
}

/// The names of a feed's outputs, in stored order.
fn output_names(feed: &Value) -> Vec<String> {
    outputs_of(feed)
        .iter()
        .map(|output| {
            output["name"]
                .as_str()
                .unwrap_or_else(|| panic!("a stored output carries a name, got {output}"))
                .to_string()
        })
        .collect()
}

/// One row of the table: an append whose list carries exactly one unresolvable reference.
struct AppendCase {
    label: String,
    /// The outputs the append carries, in request order.
    outputs: Vec<Value>,
    /// The identifier that resolves to nothing, which the refusal has to name.
    unresolvable: String,
    /// The refusal the rule earns for that identifier.
    expected: String,
    /// Whether a resource already claims the feed when the append arrives.
    already_claimed: bool,
    dry_run: bool,
}

/// Every row's unresolvable reference is refused and the feed is left exactly as it was.
#[tokio::test]
async fn an_append_naming_an_unresolvable_reference_is_refused_and_writes_nothing() {
    let router = router();
    // The resolving references a row pairs its unresolvable one with: a dictionary of this
    // endpoint, and a fixture the data source really holds.
    let dictionary = resolving_dictionary(&router, "liveeventdict").await;
    let fixture = held_fixture_id();

    // Dictionary and fixture ids that satisfy their modelled constraints (`DictionaryId`:
    // len 1..19, `^[a-zA-Z0-9]+$`; `FixtureId`: len 1..128), so the boundary validator
    // admits them and the handler's resolution is what refuses them.
    let unknown_dictionary = "doesnotexist1234567";
    let unknown_fixture = "no-such-fixture";

    let cases: Vec<AppendCase> = vec![
        AppendCase {
            label: "the only appended output names an unknown dictionary".to_string(),
            outputs: vec![subtitling_output("subs1", Some(unknown_dictionary))],
            unresolvable: unknown_dictionary.to_string(),
            expected: cannot_find_dictionary_message(unknown_dictionary),
            already_claimed: false,
            dry_run: false,
        },
        AppendCase {
            label: "the only appended output names an unknown fixture".to_string(),
            outputs: vec![clipping_output("clip1", unknown_fixture)],
            unresolvable: unknown_fixture.to_string(),
            expected: cannot_find_fixture_message(unknown_fixture),
            already_claimed: false,
            dry_run: false,
        },
        AppendCase {
            label: "the unknown dictionary is the FIRST of two, the second resolving"
                .to_string(),
            outputs: vec![
                subtitling_output("subs1", Some(unknown_dictionary)),
                subtitling_output("subs2", Some(&dictionary)),
            ],
            unresolvable: unknown_dictionary.to_string(),
            expected: cannot_find_dictionary_message(unknown_dictionary),
            already_claimed: false,
            dry_run: false,
        },
        AppendCase {
            label: "the unknown fixture is in the MIDDLE of three".to_string(),
            outputs: vec![
                subtitling_output("subs1", Some(&dictionary)),
                clipping_output("clip1", unknown_fixture),
                enabled_output("meta1", "contextualMetadata"),
            ],
            unresolvable: unknown_fixture.to_string(),
            expected: cannot_find_fixture_message(unknown_fixture),
            already_claimed: false,
            dry_run: false,
        },
        AppendCase {
            label: "the unknown dictionary is the LAST of three".to_string(),
            outputs: vec![
                clipping_output("clip1", &fixture),
                enabled_output("meta1", "contextualMetadata"),
                subtitling_output("subs1", Some(unknown_dictionary)),
            ],
            unresolvable: unknown_dictionary.to_string(),
            expected: cannot_find_dictionary_message(unknown_dictionary),
            already_claimed: false,
            dry_run: false,
        },
        AppendCase {
            label: "a feed a resource already holds takes no unknown fixture".to_string(),
            outputs: vec![clipping_output("clip9", unknown_fixture)],
            unresolvable: unknown_fixture.to_string(),
            expected: cannot_find_fixture_message(unknown_fixture),
            already_claimed: true,
            dry_run: false,
        },
        AppendCase {
            label: "a feed a resource already holds takes no unknown dictionary".to_string(),
            outputs: vec![subtitling_output("subs9", Some(unknown_dictionary))],
            unresolvable: unknown_dictionary.to_string(),
            expected: cannot_find_dictionary_message(unknown_dictionary),
            already_claimed: true,
            dry_run: false,
        },
        AppendCase {
            label: "a DRY RUN of an append naming an unknown dictionary".to_string(),
            outputs: vec![subtitling_output("subs1", Some(unknown_dictionary))],
            unresolvable: unknown_dictionary.to_string(),
            expected: cannot_find_dictionary_message(unknown_dictionary),
            already_claimed: false,
            dry_run: true,
        },
        AppendCase {
            label: "a DRY RUN of an append naming an unknown fixture".to_string(),
            outputs: vec![clipping_output("clip1", unknown_fixture)],
            unresolvable: unknown_fixture.to_string(),
            expected: cannot_find_fixture_message(unknown_fixture),
            already_claimed: false,
            dry_run: true,
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let label = case.label.as_str();
        let id = base_feed(&router, &format!("appendbase{index}")).await;
        if case.already_claimed {
            // The claim this feed arrives with, taken by an association whose own
            // references resolve.
            let (status, claimed) = associate(
                &router,
                &id,
                json!([subtitling_output("held1", Some(&dictionary))]),
                false,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {label:?}: the first association answers 200, got {status} with {claimed}"
            );
        }
        let before = get_feed(&router, &id, label).await;

        let (status, body) = associate(
            &router,
            &id,
            Value::Array(case.outputs.clone()),
            case.dry_run,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ValidationException"),
            "case {label:?}: expected ValidationException, got {body}"
        );
        let message = message_of(&body).to_string();
        assert_eq!(
            message, case.expected,
            "case {label:?}: expected {:?}, got {message:?}",
            case.expected
        );
        assert!(
            message.contains(case.unresolvable.as_str()),
            "case {label:?}: the refusal names the identifier it could not resolve \
             ({:?}), got {message:?}",
            case.unresolvable
        );

        // The feed is exactly as it was: the refused append stored none of its outputs —
        // not the unresolvable one and not a resolving sibling of the same list — and it
        // moved neither the status nor the resource that holds the feed.
        let after = get_feed(&router, &id, label).await;
        assert_eq!(
            outputs_of(&after),
            outputs_of(&before),
            "case {label:?}: the refused append left the outputs as they were, got {after}"
        );
        assert_eq!(
            after["status"], before["status"],
            "case {label:?}: the refused append left the status as it was, got {after}"
        );
        assert_eq!(
            after["association"], before["association"],
            "case {label:?}: the refused append left the association as it was, got {after}"
        );

        // The relation's other direction agrees: the resolving dictionary a row paired its
        // unresolvable reference with does not report this feed, because the output naming
        // it was never stored. A row whose feed arrived already claimed holds a stored
        // subtitling output that DOES name it, so that one is expected to report it.
        let references = get_dictionary(&router, &dictionary, label).await["references"].clone();
        let reported = references
            .as_array()
            .is_some_and(|ids| ids.contains(&json!(id)));
        assert_eq!(
            reported, case.already_claimed,
            "case {label:?}: the dictionary reports feed {id} only for the output the feed \
             really holds, got {references}"
        );
    }
}

/// The same lists with every reference RESOLVING are appended, read back, and reported by
/// the dictionary they name: the refusals above resolve each named id rather than refusing
/// the shape of the request.
#[tokio::test]
async fn an_append_whose_references_all_resolve_is_stored_and_read_back() {
    let router = router();
    let dictionary = resolving_dictionary(&router, "resolvingdict").await;
    let fixture = held_fixture_id();

    // The same shapes of list as the refused rows, with each unresolvable id replaced by
    // one that resolves: one reference, both kinds at once, and a list of three.
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "one subtitling output naming a dictionary",
            json!([subtitling_output("subs1", Some(&dictionary))]),
            vec!["crop1", "subs1"],
        ),
        (
            "a dictionary and a fixture in one list",
            json!([
                subtitling_output("subs1", Some(&dictionary)),
                clipping_output("clip1", &fixture),
            ]),
            vec!["crop1", "subs1", "clip1"],
        ),
        (
            "three outputs, two of them carrying a reference",
            json!([
                clipping_output("clip1", &fixture),
                enabled_output("meta1", "contextualMetadata"),
                subtitling_output("subs1", Some(&dictionary)),
            ]),
            vec!["crop1", "clip1", "meta1", "subs1"],
        ),
    ];

    for (index, (label, outputs, expected_names)) in rows.into_iter().enumerate() {
        let id = base_feed(&router, &format!("resolvingbase{index}")).await;

        let (status, claimed) = associate(&router, &id, outputs, false).await;
        assert_eq!(
            status, 200,
            "case {label:?}: an append whose references resolve answers 200, got {status} \
             with {claimed}"
        );

        let feed = get_feed(&router, &id, label).await;
        assert_eq!(
            output_names(&feed),
            expected_names
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<String>>(),
            "case {label:?}: the appended outputs join the stored list in request order, \
             got {feed}"
        );
        assert_eq!(
            feed["association"]["associatedResourceName"],
            json!(RESOURCE),
            "case {label:?}: the accepted association reports the resource that holds the \
             feed, got {feed}"
        );
        assert_eq!(
            feed["status"],
            json!("ACTIVE"),
            "case {label:?}: a claimed feed reports ACTIVE, got {feed}"
        );
        // Every output this call appended entered the feed through the association.
        for output in outputs_of(&feed) {
            let name = output["name"].as_str().unwrap_or("<unnamed>");
            let appended = name != "crop1";
            assert_eq!(
                output["fromAssociation"],
                json!(appended),
                "case {label:?}: output {name:?} reports whether it arrived with the \
                 association, got {output}"
            );
        }

        // The relation's other direction: the dictionary the appended output names reports
        // this feed.
        let references = get_dictionary(&router, &dictionary, label).await["references"].clone();
        assert!(
            references
                .as_array()
                .is_some_and(|ids| ids.contains(&json!(id))),
            "case {label:?}: the dictionary reports the feed that names it, got {references}"
        );
    }
}
