// SPDX-License-Identifier: CC0-1.0

//! Who OWNS an output's members, end to end over the wire: the provenance the service
//! sets on an output, and the uniqueness of an output's name within its feed.
//!
//! Every case here drives the real request path — the router picks the service from the
//! SigV4 credential scope, the framework deserializes and validates the body, the
//! handler runs, and the response is serialized as `restJson1` — so what these tests
//! assert is what a client sees.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * an output's `fromAssociation` is SET BY THE SERVICE when the output enters the feed
//!   — true for one `AssociateFeed` appended, false for one a create or an update
//!   introduced — so `UpdateFeed` keeps the stored value whether the request echoes it
//!   or omits it, and a request supplying a DIFFERENT value is refused with the modelled
//!   `ValidationException` (HTTP 400) without writing anything;
//! * an output's `name` IDENTIFIES it within its feed, so every operation that supplies
//!   a feed's output list — `CreateFeed` and `UpdateFeed`, which replace it, and
//!   `AssociateFeed`, which appends to it — refuses a list naming one output twice with
//!   the modelled `ConflictException` (HTTP 409) and stores nothing;
//! * a feed's output list is BOUNDED at `MAX_OUTPUTS_PER_FEED`, and every operation that
//!   SETS the list holds that bound: a list at the bound is accepted and one past it is
//!   refused with the modelled `ServiceQuotaExceededException` (HTTP 402), so no single
//!   request can put a stored feed past it.

use crate::harness::{call, output_config, router};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::{
    duplicate_output_name_message, from_association_not_changeable_message,
    too_many_outputs_message, MAX_OUTPUTS_PER_FEED,
};
use smithy_server_runtime::MultiServiceRouter;

/// One output body, carrying `fromAssociation` only when `from_association` is `Some`
/// (`UpdateOutput$fromAssociation` is optional: the caller echoes the value it read).
fn output(name: &str, kind: &str, from_association: Option<bool>) -> Value {
    let mut output = json!({
        "name": name,
        "outputConfig": output_config(kind),
        "status": "ENABLED"
    });
    if let Some(value) = from_association {
        output
            .as_object_mut()
            .expect("an output body is an object")
            .insert("fromAssociation".to_string(), json!(value));
    }
    output
}

/// `GET /v1/feed/{id}`, asserting it is served.
async fn get_feed(router: &MultiServiceRouter, id: &str, label: &str) -> Value {
    let (status, feed) = call(router, "GET", &format!("/v1/feed/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {label:?}: GetFeed answers 200, got {status} with {feed}"
    );
    feed
}

/// The `fromAssociation` each output of `feed` reports, keyed by output name.
fn provenance(feed: &Value) -> Vec<(String, Value)> {
    feed["outputs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|output| {
            (
                output["name"].as_str().unwrap_or_default().to_string(),
                output["fromAssociation"].clone(),
            )
        })
        .collect()
}

/// A feed holding one output of EACH provenance: `crop1`, which `CreateFeed` introduced
/// (false), and `subs1`, which `AssociateFeed` appended (true). Returns its id.
async fn feed_of_both_provenances(router: &MultiServiceRouter, label: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/v1/feed",
        Some(json!({
            "name": "provenance-feed",
            "outputs": [output("crop1", "cropping", None)]
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {label:?}: CreateFeed answers 202, got {status} with {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: CreateFeed returns an id, got {created}"))
        .to_string();

    let (status, associated) = call(
        router,
        "POST",
        &format!("/v1/feed/{id}/associate"),
        Some(json!({
            "associatedResourceName": "source-media",
            "outputs": [output("subs1", "subtitling", None)]
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label:?}: AssociateFeed answers 200, got {status} with {associated}"
    );

    let feed = get_feed(router, &id, label).await;
    assert_eq!(
        provenance(&feed),
        vec![
            ("crop1".to_string(), json!(false)),
            ("subs1".to_string(), json!(true)),
        ],
        "case {label:?}: the fixture holds one output of each provenance, got {:?}",
        feed["outputs"]
    );
    id
}

/// One row of the provenance table: the replacement list a PUT carries, and the
/// provenance the feed must report afterwards, output by output in list order.
struct KeptCase {
    label: &'static str,
    outputs: Value,
    expected: Vec<(&'static str, bool)>,
}

fn kept(label: &'static str, outputs: Value, expected: Vec<(&'static str, bool)>) -> KeptCase {
    KeptCase {
        label,
        outputs,
        expected,
    }
}

/// UpdateFeed keeps the provenance the service set: the stored value survives a request
/// that echoes it and one that omits it, and an output the update INTRODUCES is false.
#[tokio::test]
async fn update_feed_keeps_the_service_set_output_provenance() {
    // The rows differ along what the rule generalises over: which stored outputs the
    // replacement list carries, in which order, whether each one echoes the member or
    // omits it, the config it carries, and whether the output is new to the feed.
    let rows = vec![
        kept(
            "both stored outputs, the member omitted on both",
            json!([
                output("crop1", "cropping", None),
                output("subs1", "subtitling", None)
            ]),
            vec![("crop1", false), ("subs1", true)],
        ),
        kept(
            "both stored outputs, each echoing the value it was read with",
            json!([
                output("crop1", "cropping", Some(false)),
                output("subs1", "subtitling", Some(true))
            ]),
            vec![("crop1", false), ("subs1", true)],
        ),
        kept(
            "the association-made output alone, echoing true",
            json!([output("subs1", "subtitling", Some(true))]),
            vec![("subs1", true)],
        ),
        kept(
            "the association-made output alone, omitting the member",
            json!([output("subs1", "subtitling", None)]),
            vec![("subs1", true)],
        ),
        kept(
            "the stored outputs reordered and reconfigured",
            json!([
                output("subs1", "cropping", None),
                output("crop1", "contextualMetadata", Some(false))
            ]),
            vec![("subs1", true), ("crop1", false)],
        ),
        kept(
            "one output the update introduces, beside a stored one",
            json!([
                output("fresh1", "clipping", None),
                output("subs1", "subtitling", Some(true))
            ]),
            vec![("fresh1", false), ("subs1", true)],
        ),
        kept(
            "only outputs the update introduces",
            json!([
                output("fresh1", "clipping", None),
                output("fresh2", "cropping", Some(false))
            ]),
            vec![("fresh1", false), ("fresh2", false)],
        ),
    ];

    for KeptCase {
        label,
        outputs,
        expected,
    } in rows
    {
        let router = router();
        let id = feed_of_both_provenances(&router, label).await;

        let (status, updated) = call(
            &router,
            "PUT",
            &format!("/v1/feed/{id}"),
            Some(json!({"name": "provenance-feed", "outputs": outputs})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: UpdateFeed answers 200, got {status} with {updated}"
        );
        let expected: Vec<(String, Value)> = expected
            .into_iter()
            .map(|(name, from_association)| (name.to_string(), json!(from_association)))
            .collect();
        assert_eq!(
            provenance(&updated),
            expected,
            "case {label:?}: the update reports the provenance the service holds, got {:?}",
            updated["outputs"]
        );

        // The provenance is in STORAGE, not only in the response.
        let fetched = get_feed(&router, &id, label).await;
        assert_eq!(
            provenance(&fetched),
            expected,
            "case {label:?}: GetFeed reports the provenance the service holds, got {:?}",
            fetched["outputs"]
        );
    }
}

/// A request that supplies a `fromAssociation` the service does not hold for the output
/// of that name is asking to rewrite a service-set fact: refused (HTTP 400), nothing
/// written.
#[tokio::test]
async fn update_feed_refuses_a_request_that_changes_output_provenance() {
    // The rows differ in which output the changed value sits on (one the create made,
    // one the association made, one the feed has never held), its position in the list,
    // and the direction of the change (false -> true and true -> false).
    let rows: Vec<(&str, Value, usize, bool)> = vec![
        (
            "true on the output the create made",
            json!([
                output("crop1", "cropping", Some(true)),
                output("subs1", "subtitling", Some(true))
            ]),
            1,
            true,
        ),
        (
            "false on the output the association made",
            json!([
                output("crop1", "cropping", None),
                output("subs1", "subtitling", Some(false))
            ]),
            2,
            false,
        ),
        (
            "true on an output the feed has never held",
            json!([output("fresh1", "clipping", Some(true))]),
            1,
            true,
        ),
        (
            "true on the third output of the list",
            json!([
                output("crop1", "cropping", None),
                output("subs1", "subtitling", Some(true)),
                output("fresh1", "contextualMetadata", Some(true))
            ]),
            3,
            true,
        ),
    ];

    for (label, outputs, position, supplied) in rows {
        let router = router();
        let id = feed_of_both_provenances(&router, label).await;
        let before = get_feed(&router, &id, label).await;

        let (status, body) = call(
            &router,
            "PUT",
            &format!("/v1/feed/{id}"),
            Some(json!({"name": "renamed-feed", "outputs": outputs})),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: a request that changes fromAssociation is refused with 400, \
             got {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ValidationException"),
            "case {label:?}: that 400 is a ValidationException, got {body}"
        );
        assert_eq!(
            body["message"],
            json!(from_association_not_changeable_message(position, supplied)),
            "case {label:?}: the refusal names the member and the value supplied, got {:?}",
            body["message"]
        );

        let after = get_feed(&router, &id, label).await;
        assert_eq!(
            after["name"], before["name"],
            "case {label:?}: the refused update renamed nothing, got {:?}",
            after["name"]
        );
        assert_eq!(
            after["outputs"], before["outputs"],
            "case {label:?}: the refused update left the stored outputs as they were, got {:?}",
            after["outputs"]
        );
    }
}

/// Which operation carries the output list under test — the three that supply one.
#[derive(Debug, Clone, Copy)]
enum ListCarrier {
    /// `CreateFeed`: the list the new feed is minted with (HTTP 202 when accepted).
    Create,
    /// `UpdateFeed`: the list that replaces the stored one (HTTP 200 when accepted).
    Update,
    /// `AssociateFeed`: the list appended to the stored one (HTTP 200 when accepted).
    Associate,
}

impl ListCarrier {
    /// The status this operation answers when it accepts the list.
    fn accepted_status(self) -> u16 {
        match self {
            Self::Create => 202,
            Self::Update | Self::Associate => 200,
        }
    }
}

/// Send `outputs` to `carrier` on a feed that already holds one output named `held1`,
/// and return the response plus that feed's id.
async fn carry_outputs(
    router: &MultiServiceRouter,
    carrier: ListCarrier,
    outputs: Value,
    label: &str,
) -> (u16, Value, String) {
    let (status, created) = call(
        router,
        "POST",
        "/v1/feed",
        Some(json!({
            "name": "uniqueness-feed",
            "outputs": [output("held1", "cropping", None)]
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {label:?}: the fixture feed is created, got {status} with {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: CreateFeed returns an id, got {created}"))
        .to_string();

    let (status, body) = match carrier {
        ListCarrier::Create => {
            call(
                router,
                "POST",
                "/v1/feed",
                Some(json!({"name": "minted-feed", "outputs": outputs})),
            )
            .await
        }
        ListCarrier::Update => {
            call(
                router,
                "PUT",
                &format!("/v1/feed/{id}"),
                Some(json!({"name": "uniqueness-feed", "outputs": outputs})),
            )
            .await
        }
        ListCarrier::Associate => {
            call(
                router,
                "POST",
                &format!("/v1/feed/{id}/associate"),
                Some(json!({
                    "associatedResourceName": "source-media",
                    "outputs": outputs
                })),
            )
            .await
        }
    };
    (status, body, id)
}

/// An output name identifies the output within its feed, so no operation that supplies
/// an output list accepts a list naming one output twice.
#[tokio::test]
async fn an_output_list_naming_one_output_twice_is_refused() {
    // The rows differ in where the repeat is and how the repeated entries differ: side
    // by side, with another output between them, three copies of one name, and (the
    // control row) a list whose names are all distinct. `None` is the control.
    let rows: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "two outputs of one name, side by side",
            json!([
                output("dup1", "cropping", None),
                output("dup1", "clipping", None)
            ]),
            Some("dup1"),
        ),
        (
            "two outputs of one name, with another between them",
            json!([
                output("edge1", "cropping", None),
                output("mid1", "clipping", None),
                output("edge1", "subtitling", None)
            ]),
            Some("edge1"),
        ),
        (
            "three outputs of one name",
            json!([
                output("thrice1", "cropping", None),
                output("thrice1", "clipping", None),
                output("thrice1", "subtitling", None)
            ]),
            Some("thrice1"),
        ),
        (
            "one name repeated after a first, distinct pair",
            json!([
                output("one1", "cropping", None),
                output("two1", "clipping", None),
                output("two1", "contextualMetadata", None)
            ]),
            Some("two1"),
        ),
        (
            "names that differ only in case are distinct names",
            json!([
                output("Case1", "cropping", None),
                output("case1", "clipping", None)
            ]),
            None,
        ),
        (
            "every name distinct",
            json!([
                output("first1", "cropping", None),
                output("second1", "clipping", None)
            ]),
            None,
        ),
    ];

    for (row, outputs, duplicate) in rows {
        for carrier in [
            ListCarrier::Create,
            ListCarrier::Update,
            ListCarrier::Associate,
        ] {
            let label = format!("{carrier:?}: {row}");
            let router = router();
            let (status, body, id) =
                carry_outputs(&router, carrier, outputs.clone(), &label).await;

            let Some(duplicate) = duplicate else {
                assert_eq!(
                    status,
                    carrier.accepted_status(),
                    "case {label:?}: distinct names are accepted with HTTP {}, got {status} \
                     with {body}",
                    carrier.accepted_status()
                );
                continue;
            };

            assert_eq!(
                status, 409,
                "case {label:?}: a repeated output name is refused with 409, got {status} \
                 with {body}"
            );
            assert_eq!(
                body["__type"],
                json!("ConflictException"),
                "case {label:?}: that 409 is a ConflictException, got {body}"
            );
            assert_eq!(
                body["message"],
                json!(duplicate_output_name_message(duplicate)),
                "case {label:?}: the refusal names the repeated name, got {:?}",
                body["message"]
            );

            // Nothing was stored: no feed was minted, and the fixture feed still holds
            // exactly the output it was created with.
            let (status, listed) = call(&router, "GET", "/v1/feeds", None).await;
            assert_eq!(
                (status, listed["feeds"].as_array().map(Vec::len)),
                (200, Some(1)),
                "case {label:?}: the refused call minted no feed, got {listed}"
            );
            let feed = get_feed(&router, &id, &label).await;
            assert_eq!(
                provenance(&feed),
                vec![("held1".to_string(), json!(false))],
                "case {label:?}: the refused call left the stored outputs as they were, \
                 got {:?}",
                feed["outputs"]
            );
        }
    }
}

/// A feed's output list is BOUNDED, and the operations that SET the list hold that
/// bound: a list at the bound is accepted, one past it is refused (HTTP 402) and the
/// store is left as it was.
#[tokio::test]
async fn an_output_list_past_the_feed_output_quota_is_refused() {
    // Each published bound and one past it: just under, exactly at, one over, and well
    // over. (`AssociateFeed`'s own bound is on the APPEND — stored plus appended —
    // which `feed_association.rs` covers; here the request IS the whole list.)
    for carrier in [ListCarrier::Create, ListCarrier::Update] {
        for count in [
            MAX_OUTPUTS_PER_FEED - 1,
            MAX_OUTPUTS_PER_FEED,
            MAX_OUTPUTS_PER_FEED + 1,
            MAX_OUTPUTS_PER_FEED + 5,
        ] {
            let label = format!("{carrier:?}: a list of {count} outputs");
            let router = router();
            let outputs: Vec<Value> = (0..count)
                .map(|index| output(&format!("out{index}"), "cropping", None))
                .collect();
            let (status, body, id) =
                carry_outputs(&router, carrier, json!(outputs), &label).await;

            if count <= MAX_OUTPUTS_PER_FEED {
                assert_eq!(
                    status,
                    carrier.accepted_status(),
                    "case {label:?}: a list within the bound of {MAX_OUTPUTS_PER_FEED} is \
                     accepted with HTTP {}, got {status} with {body}",
                    carrier.accepted_status()
                );
                continue;
            }

            assert_eq!(
                status, 402,
                "case {label:?}: a list past the bound of {MAX_OUTPUTS_PER_FEED} is refused \
                 with 402, got {status} with {body}"
            );
            assert_eq!(
                body["__type"],
                json!("ServiceQuotaExceededException"),
                "case {label:?}: that 402 is a ServiceQuotaExceededException, got {body}"
            );
            assert_eq!(
                body["message"],
                json!(too_many_outputs_message()),
                "case {label:?}: the refusal states the bound, got {:?}",
                body["message"]
            );

            let (status, listed) = call(&router, "GET", "/v1/feeds", None).await;
            assert_eq!(
                (status, listed["feeds"].as_array().map(Vec::len)),
                (200, Some(1)),
                "case {label:?}: the refused call minted no feed, got {listed}"
            );
            let feed = get_feed(&router, &id, &label).await;
            assert_eq!(
                provenance(&feed),
                vec![("held1".to_string(), json!(false))],
                "case {label:?}: the refused call left the stored outputs as they were, \
                 got {:?}",
                feed["outputs"]
            );
        }
    }
}
