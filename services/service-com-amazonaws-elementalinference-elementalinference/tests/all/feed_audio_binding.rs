// SPDX-License-Identifier: CC0-1.0

//! Which audio input of a feed each of its outputs consumes, end to end over the wire.
//!
//! Every case here drives the real request path — the router picks the service from the
//! SigV4 credential scope, the framework deserializes and validates the body, the
//! handler runs, and the response is serialized as `restJson1` — so what these tests
//! assert is what a client sees.
//!
//! THE RULE under test: a feed EXPOSES media inputs and each of its outputs consumes one
//! of them, so
//!
//! * `GetFeed` reports the inputs the feed exposes (`inputs.audio[*].name`), which is
//!   how a client learns what it may bind an output to and where it pushes media; a feed
//!   created with no explicit input exposes the service's default audio input, minted
//!   with the feed, so the member is reported from the moment the feed exists;
//! * every output's configuration reports the audio input that output consumes
//!   (`audioInput`), whichever member of the `OutputConfig` union the configuration is,
//!   on the create's own answer, on every read and on every write — so a
//!   read-modify-write round trip through `UpdateFeed` (a full-replacement PUT) cannot
//!   drop the binding;
//! * a request that names no audio input is bound to the feed's default input, a request
//!   that names one of the feed's inputs keeps it, and one that names an input the feed
//!   does not expose is REFUSED with the modelled `ValidationException` (HTTP 400) and
//!   writes nothing — the same rule a dictionary or fixture reference that resolves to
//!   nothing earns.
//!
//! Each rule is asserted over a TABLE whose rows differ along what it generalises over:
//! which member of the union the configuration is, whether the binding is supplied or
//! left to the service, which operation writes the output (`CreateFeed`, `UpdateFeed`,
//! `AssociateFeed`), and the form of a name that resolves to no input.

use crate::harness::{call, output_config, past_the_transition_span, router};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::cannot_find_audio_input_message;
use smithy_server_runtime::MultiServiceRouter;

/// Every member of the `OutputConfig` union
/// (`smithy_query(query="shape",shape="OutputConfig")`: cropping, clipping, subtitling,
/// contextualMetadata). The audio binding is a property of an output whichever feature
/// it is for, so every rule below is asserted on all four.
const CONFIG_KINDS: [&str; 4] = ["cropping", "clipping", "subtitling", "contextualMetadata"];

/// `POST /v1/feed` with one output named `output_name` carrying `config`.
async fn create_feed(
    router: &MultiServiceRouter,
    name: &str,
    output_name: &str,
    config: &Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        "/v1/feed",
        Some(json!({
            "name": name,
            "outputs": [{"name": output_name, "outputConfig": config, "status": "ENABLED"}]
        })),
    )
    .await
}

/// `PUT /v1/feed/{id}` replacing the feed's payload with one output carrying `config`.
async fn update_feed(
    router: &MultiServiceRouter,
    id: &str,
    output_name: &str,
    config: &Value,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/v1/feed/{id}"),
        Some(json!({
            "name": "audio-bound-feed",
            "outputs": [{"name": output_name, "outputConfig": config, "status": "ENABLED"}]
        })),
    )
    .await
}

/// `POST /v1/feed/{id}/associate` appending one output carrying `config`.
async fn associate_feed(
    router: &MultiServiceRouter,
    id: &str,
    resource: &str,
    output_name: &str,
    config: &Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/v1/feed/{id}/associate"),
        Some(json!({
            "associatedResourceName": resource,
            "outputs": [{"name": output_name, "outputConfig": config, "status": "ENABLED"}]
        })),
    )
    .await
}

/// `GET /v1/feed/{id}`, asserting it is served.
async fn get_feed(router: &MultiServiceRouter, id: &str, case: &str) -> Value {
    let (status, feed) = call(router, "GET", &format!("/v1/feed/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetFeed answers 200, got {status} with {feed}"
    );
    feed
}

/// The `outputConfig` of `kind` with `audio_input` named, or left to the service when it
/// is `None`.
fn config_bound_to(kind: &str, audio_input: Option<&str>) -> Value {
    let mut config = output_config(kind);
    if let Some(audio_input) = audio_input {
        config[kind]
            .as_object_mut()
            .unwrap_or_else(|| panic!("the {kind} config is an object"))
            .insert("audioInput".to_string(), json!(audio_input));
    }
    config
}

/// The audio input the single member of `config` reports, or `None` when it reports
/// none.
fn binding_of(config: &Value) -> Option<String> {
    let members = config
        .as_object()
        .unwrap_or_else(|| panic!("an outputConfig is a union object, got {config}"));
    assert_eq!(
        members.len(),
        1,
        "an outputConfig carries exactly one member, got {config}"
    );
    members
        .values()
        .next()
        .and_then(|settings| settings.get("audioInput"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The names of the audio inputs a feed reports it exposes
/// (`GetFeedResponse$inputs.audio[*].name`).
fn exposed_audio_inputs(feed: &Value, case: &str) -> Vec<String> {
    let audio = feed["inputs"]["audio"].as_array().unwrap_or_else(|| {
        panic!("case {case:?}: the feed reports the inputs it exposes, got {feed}")
    });
    audio
        .iter()
        .map(|input| {
            let members = input
                .as_object()
                .unwrap_or_else(|| panic!("case {case:?}: an audio input is an object: {input}"));
            assert_eq!(
                members.keys().collect::<Vec<&String>>(),
                vec!["name"],
                "case {case:?}: an audio input reports its name, got {input}"
            );
            let name = members["name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: an input name is a string: {input}"))
                .to_string();
            assert!(
                !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                "case {case:?}: an input name satisfies the modelled ResourceName pattern \
                 ^[a-zA-Z0-9]([a-zA-Z0-9-_]{{0,126}}[a-zA-Z0-9])?$, got {name:?}"
            );
            name
        })
        .collect()
}

/// The audio input every output of `feed` reports, asserting each one is an input the
/// feed exposes.
fn bindings_of(feed: &Value, exposed: &[String], case: &str) -> Vec<String> {
    let outputs = feed["outputs"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the feed reports its outputs, got {feed}"));
    assert!(
        !outputs.is_empty(),
        "case {case:?}: a feed holds at least one output, got {feed}"
    );
    outputs
        .iter()
        .map(|output| {
            let binding = binding_of(&output["outputConfig"]).unwrap_or_else(|| {
                panic!(
                    "case {case:?}: output {:?} reports the audio input it consumes, got {}",
                    output["name"], output["outputConfig"]
                )
            });
            assert!(
                exposed.contains(&binding),
                "case {case:?}: output {:?} is bound to an input the feed exposes ({exposed:?}), \
                 got {binding:?}",
                output["name"]
            );
            binding
        })
        .collect()
}

/// A feed reports the inputs it exposes, and an output that names none is bound to the
/// feed's default input — on the create's own answer and on every read, for every member
/// of the `OutputConfig` union.
#[tokio::test]
async fn a_feed_exposes_its_inputs_and_binds_an_unbound_output_to_the_default_one() {
    for kind in CONFIG_KINDS {
        let case = format!("a {kind} output naming no audio input");
        let router = router();
        let (status, created) =
            create_feed(&router, "audio-bound-feed", "out1", &output_config(kind)).await;
        assert_eq!(
            status, 202,
            "case {case:?}: CreateFeed answers 202, got {status} with {created}"
        );
        let id = created["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create reports an id, got {created}"))
            .to_string();
        let accepted_binding = binding_of(&created["outputs"][0]["outputConfig"]).unwrap_or_else(
            || {
                panic!(
                    "case {case:?}: the create reports the audio input the output consumes, got {}",
                    created["outputs"][0]["outputConfig"]
                )
            },
        );
        // `CreateFeedResponse` declares no `inputs` member, so only a read of the feed
        // reports the input set.
        assert_eq!(
            created.get("inputs"),
            None,
            "case {case:?}: the create reports no input set, got {created}"
        );

        let feed = get_feed(&router, &id, &case).await;
        let exposed = exposed_audio_inputs(&feed, &case);
        assert_eq!(
            exposed.len(),
            1,
            "case {case:?}: a feed created with no explicit input exposes exactly one, got \
             {exposed:?}"
        );
        assert_eq!(
            bindings_of(&feed, &exposed, &case),
            vec![exposed[0].clone()],
            "case {case:?}: the output is bound to the feed's default input {:?}",
            exposed[0]
        );
        assert_eq!(
            accepted_binding, exposed[0],
            "case {case:?}: the create reported the same binding the read does, got \
             {accepted_binding:?} then {:?}",
            exposed[0]
        );
    }
}

/// An audio input the request NAMES is the one the output keeps, whichever operation
/// writes the output: the name is discovered from the feed itself, so what is asserted is
/// that a supplied binding survives the write rather than any particular token.
#[tokio::test]
async fn a_supplied_audio_input_is_the_one_every_write_path_stores() {
    for kind in CONFIG_KINDS {
        let case = format!("a {kind} output naming the feed's own audio input");
        let router = router();

        // What this endpoint exposes, learned the way a client learns it: from a read.
        let (status, probe) =
            create_feed(&router, "audio-probe-feed", "out1", &output_config(kind)).await;
        assert_eq!(
            status, 202,
            "case {case:?}: the probe feed is created, got {status} with {probe}"
        );
        let probe_id = probe["id"].as_str().unwrap_or_default().to_string();
        let exposed = exposed_audio_inputs(&get_feed(&router, &probe_id, &case).await, &case);
        let named = exposed[0].clone();
        let bound = config_bound_to(kind, Some(&named));

        // CreateFeed with the binding stated.
        let (status, created) = create_feed(&router, "audio-bound-feed", "out1", &bound).await;
        assert_eq!(
            status, 202,
            "case {case:?}: CreateFeed answers 202, got {status} with {created}"
        );
        let id = created["id"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            binding_of(&created["outputs"][0]["outputConfig"]),
            Some(named.clone()),
            "case {case:?}: the create keeps the supplied binding, got {}",
            created["outputs"][0]["outputConfig"]
        );
        let feed = get_feed(&router, &id, &case).await;
        assert_eq!(
            bindings_of(&feed, &exposed_audio_inputs(&feed, &case), &case),
            vec![named.clone()],
            "case {case:?}: the stored output keeps the supplied binding, got {feed}"
        );

        // UpdateFeed: a full-replacement PUT of the config just read keeps it too.
        let read_back = feed["outputs"][0]["outputConfig"].clone();
        let (status, updated) = update_feed(&router, &id, "out1", &read_back).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateFeed answers 200, got {status} with {updated}"
        );
        assert_eq!(
            binding_of(&updated["outputs"][0]["outputConfig"]),
            Some(named.clone()),
            "case {case:?}: the round trip keeps the binding, got {}",
            updated["outputs"][0]["outputConfig"]
        );

        // AssociateFeed appends an output carrying the binding; the append runs on a
        // settled feed, so the reconfiguration the PUT started is waited out first.
        past_the_transition_span();
        let (status, claimed) =
            associate_feed(&router, &id, "source-media", "out2", &bound).await;
        assert_eq!(
            status, 200,
            "case {case:?}: AssociateFeed answers 200, got {status} with {claimed}"
        );
        let feed = get_feed(&router, &id, &case).await;
        let exposed = exposed_audio_inputs(&feed, &case);
        assert_eq!(
            bindings_of(&feed, &exposed, &case),
            vec![named.clone(), named.clone()],
            "case {case:?}: the appended output carries the binding too, got {feed}"
        );
    }
}

/// An audio input the feed does not expose names nothing it has to offer: every write
/// path refuses it with the modelled `ValidationException` (HTTP 400) and stores nothing.
#[tokio::test]
async fn an_audio_input_the_feed_does_not_expose_is_refused() {
    let router = router();
    let (status, probe) = create_feed(
        &router,
        "audio-probe-feed",
        "out1",
        &output_config("cropping"),
    )
    .await;
    assert_eq!(
        status, 202,
        "the probe feed is created, got {status} with {probe}"
    );
    let probe_id = probe["id"].as_str().unwrap_or_default().to_string();
    let exposed = exposed_audio_inputs(&get_feed(&router, &probe_id, "probe").await, "probe");
    let named = exposed[0].clone();

    // The rows differ along the form of a name that resolves to no input of the feed: a
    // one-character name, the exposed name with a suffix, the exposed name in another
    // case (names are matched exactly), another plausible input name, and the exposed
    // name with its separators changed.
    let unknown: Vec<String> = [
        "a".to_string(),
        format!("{named}-2"),
        named.to_uppercase(),
        "second-audio".to_string(),
        named.replace('-', "_"),
    ]
    .into_iter()
    // A row that happens to spell the exposed name is no longer a name the feed does
    // not expose, so it is not one of this rule's inputs.
    .filter(|candidate| *candidate != named)
    .collect();
    assert!(
        unknown.len() >= 4,
        "the table spans several forms of an unexposed name, got {unknown:?}"
    );

    for kind in CONFIG_KINDS {
        for name in &unknown {
            let case = format!("a {kind} output naming the audio input {name:?}");
            let bound = config_bound_to(kind, Some(name));

            // CreateFeed: refused, and no feed is minted for it.
            let (status, refused) = create_feed(&router, "refused-feed", "out1", &bound).await;
            assert_eq!(
                status, 400,
                "case {case:?}: CreateFeed refuses it with the modelled 400, got {status} with \
                 {refused}"
            );
            assert_eq!(
                refused["__type"],
                json!("ValidationException"),
                "case {case:?}: that 400 is a ValidationException, got {refused}"
            );
            assert_eq!(
                refused["message"],
                json!(cannot_find_audio_input_message(name)),
                "case {case:?}: the refusal names the audio input the request supplied, got {}",
                refused["message"]
            );
            let (status, listed) = call(&router, "GET", "/v1/feeds?maxResults=100", None).await;
            assert_eq!(
                (status, listed["feeds"].as_array().map(Vec::len)),
                (200, Some(1)),
                "case {case:?}: the refused create minted no feed, got {status} with {listed}"
            );

            // UpdateFeed and AssociateFeed: refused on a stored feed, which keeps the
            // output it already holds.
            let before = get_feed(&router, &probe_id, &case).await;
            for (path, (status, refused)) in [
                (
                    "UpdateFeed",
                    update_feed(&router, &probe_id, "out1", &bound).await,
                ),
                (
                    "AssociateFeed",
                    associate_feed(&router, &probe_id, "source-media", "out2", &bound).await,
                ),
            ] {
                assert_eq!(
                    status, 400,
                    "case {case:?}: {path} refuses it with the modelled 400, got {status} with \
                     {refused}"
                );
                assert_eq!(
                    refused["__type"],
                    json!("ValidationException"),
                    "case {case:?}: {path}'s 400 is a ValidationException, got {refused}"
                );
                assert_eq!(
                    refused["message"],
                    json!(cannot_find_audio_input_message(name)),
                    "case {case:?}: {path} names the audio input the request supplied, got {}",
                    refused["message"]
                );
            }
            let after = get_feed(&router, &probe_id, &case).await;
            assert_eq!(
                after, before,
                "case {case:?}: the refused writes left the feed as it was, got {after}"
            );
        }
    }
}

/// The inputs a feed exposes are the feed's own, for its whole life: every later call
/// reports the same set, and every output it then holds is still bound to one of them.
#[tokio::test]
async fn the_inputs_a_feed_exposes_outlive_every_write_to_it() {
    for kind in CONFIG_KINDS {
        let case = format!("a feed whose outputs are {kind} configs");
        let router = router();
        let (status, created) =
            create_feed(&router, "long-lived-feed", "out1", &output_config(kind)).await;
        assert_eq!(
            status, 202,
            "case {case:?}: CreateFeed answers 202, got {status} with {created}"
        );
        let id = created["id"].as_str().unwrap_or_default().to_string();
        let exposed = exposed_audio_inputs(&get_feed(&router, &id, &case).await, &case);

        // Each step: what the feed is put through, after which its input set must be
        // unchanged and its outputs still bound inside it.
        let (status, updated) = update_feed(&router, &id, "out1", &output_config(kind)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateFeed answers 200, got {status} with {updated}"
        );
        past_the_transition_span();
        let (status, claimed) = associate_feed(
            &router,
            &id,
            "source-media",
            "out2",
            &output_config("contextualMetadata"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: AssociateFeed answers 200, got {status} with {claimed}"
        );
        let held = get_feed(&router, &id, &case).await;
        assert_eq!(
            exposed_audio_inputs(&held, &case),
            exposed,
            "case {case:?}: a claimed feed exposes the same inputs, got {held}"
        );
        bindings_of(&held, &exposed, &case);

        let (status, released) = call(
            &router,
            "POST",
            &format!("/v1/feed/{id}/disassociate"),
            Some(json!({"associatedResourceName": "source-media"})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DisassociateFeed answers 200, got {status} with {released}"
        );
        let archived = get_feed(&router, &id, &case).await;
        assert_eq!(
            exposed_audio_inputs(&archived, &case),
            exposed,
            "case {case:?}: a released feed exposes the same inputs, got {archived}"
        );
        bindings_of(&archived, &exposed, &case);
    }
}
