// SPDX-License-Identifier: CC0-1.0

//! A deletion that has COMPLETED leaves NOTHING an operation can resolve.
//!
//! Both of this service's deletes are accepted with HTTP 202 and complete when the
//! removal falls due. From that instant the id names no resource of the Region at all,
//! so EVERY operation that takes it answers the modelled `ResourceNotFoundException`
//! (HTTP 404) — the delete that served the deletion included, since `@idempotent`
//! constrains the EFFECT of a repeat (one deletion, not two) and not whether a call
//! about a resource that no longer exists succeeds. A client can therefore read a 404 as
//! "already gone" and a success as "it was still there", and no two paths disagree about
//! what the Region holds in one instant.
//!
//! The suites below assert that over a table of inputs — both resource kinds, resources
//! of several shapes, with and without siblings in the Region, and however often the
//! delete is repeated — plus what the removal frees: the Region's quota, and nothing
//! else's record.

use crate::harness::{
    call, call_in_region, enabled_output, listed_ids, message_of, past_the_transition_span,
    percent_encode, router, tag_object, tags_path, REGION,
};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::{
    MAX_FEEDS, NOT_FOUND_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

/// One row of the completed-deletion table: which kind of resource is deleted, the body
/// it was created from, how many other resources of that kind the Region holds, and how
/// often the completed deletion is repeated.
struct DeleteCase {
    label: &'static str,
    kind: &'static str,
    body: Value,
    siblings: usize,
    repeats: usize,
}

/// Rows spanning what the rule generalises over: the resource kind (so both delete
/// operations), the shape of the resource (how many outputs, whether it carries tags, a
/// role, entries), whether the Region holds other resources of that kind, and the number
/// of times the completed deletion is repeated.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            label: "the only feed of the Region, repeated once",
            kind: "feed",
            body: json!({"name": "lone-feed", "outputs": [enabled_output("crop1", "cropping")]}),
            siblings: 0,
            repeats: 1,
        },
        DeleteCase {
            label: "a feed with three outputs, a role and tags, repeated three times",
            kind: "feed",
            body: json!({
                "name": "rich-feed",
                "accessRoleArn": "arn:aws:iam::000000000000:role/elemental-inference-feed",
                "outputs": [
                    enabled_output("crop1", "cropping"),
                    enabled_output("clip1", "clipping"),
                    enabled_output("meta1", "contextualMetadata"),
                ],
                "tags": {"team": "broadcast"},
            }),
            siblings: 2,
            repeats: 3,
        },
        DeleteCase {
            label: "a dictionary with no entries, repeated once",
            kind: "dictionary",
            body: json!({"name": "lone-dictionary", "language": "eng"}),
            siblings: 0,
            repeats: 1,
        },
        DeleteCase {
            label: "a dictionary with entries and tags, repeated twice",
            kind: "dictionary",
            body: json!({
                "name": "rich-dictionary",
                "language": "fra",
                "entries": "offside\nhat-trick\n",
                "tags": {"env": "test"},
            }),
            siblings: 3,
            repeats: 2,
        },
    ]
}

/// Create one resource of `kind` from `body` and return its id and ARN.
async fn create(router: &MultiServiceRouter, kind: &str, body: Value) -> (String, String) {
    let (status, created) = call(router, "POST", &format!("/v1/{kind}"), Some(body)).await;
    assert_eq!(
        status, 202,
        "creating a {kind} answers 202, got {status} with {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("a created {kind} carries an id, got {created}"))
        .to_string();
    let arn = created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("a created {kind} carries an arn, got {created}"))
        .to_string();
    (id, arn)
}

/// A create body for a sibling resource of `kind`, named `name`: the smallest resource of
/// that kind, since a sibling is only there to be left alone.
fn sibling_body(kind: &str, name: &str) -> Value {
    match kind {
        "feed" => json!({"name": name, "outputs": [enabled_output("crop1", "cropping")]}),
        "dictionary" => json!({"name": name, "language": "eng"}),
        other => panic!("no such resource kind: {other}"),
    }
}

/// The list route and the response member of `kind`, as the model binds them
/// (`OPERATIONS.md`, "Route": `GET /v1/feeds`, `GET /v1/dictionaries`).
fn list_route(kind: &str) -> (&'static str, &'static str) {
    match kind {
        "feed" => ("/v1/feeds", "feeds"),
        "dictionary" => ("/v1/dictionaries", "dictionaries"),
        other => panic!("no such resource kind: {other}"),
    }
}

/// The ids this Region's list operation reports for `kind`.
async fn reported_ids(router: &MultiServiceRouter, kind: &str) -> Vec<String> {
    let (path, member) = list_route(kind);
    listed_ids(router, REGION, path, member).await
}

/// A delete the service has already COMPLETED is REFUSED, with the modelled 404 and this
/// service's not-found sentence, however often it is repeated — and the deleted resource
/// is gone from every read and every list, while its siblings are untouched.
#[tokio::test]
async fn a_completed_deletion_is_refused_however_often_the_delete_is_repeated() {
    for case in delete_cases() {
        let DeleteCase {
            label,
            kind,
            body,
            siblings,
            repeats,
        } = case;
        let router = router();
        let mut sibling_ids = Vec::new();
        for index in 0..siblings {
            let (id, _) = create(&router, kind, sibling_body(kind, &format!("other{index}"))).await;
            sibling_ids.push(id);
        }
        let (id, _arn) = create(&router, kind, body).await;

        // The deletion is ACCEPTED, not completed: the transient status says so.
        let (status, accepted) = call(&router, "DELETE", &format!("/v1/{kind}/{id}"), None).await;
        assert_eq!(
            (status, &accepted["status"]),
            (202, &json!("DELETING")),
            "case {label:?}: the accepted delete answers 202/DELETING, got {status} with \
             {accepted}"
        );

        past_the_transition_span();
        for repeat in 0..repeats {
            let (status, repeated) =
                call(&router, "DELETE", &format!("/v1/{kind}/{id}"), None).await;
            assert_eq!(
                (status, &repeated["__type"]),
                (404, &json!("ResourceNotFoundException")),
                "case {label:?}: repeat {repeat} of the completed delete is a 404 \
                 ResourceNotFoundException, got {status} with {repeated}"
            );
            assert_eq!(
                message_of(&repeated),
                NOT_FOUND_MESSAGE,
                "case {label:?}: repeat {repeat} carries this service's not-found sentence, got \
                 {repeated}"
            );
        }

        // The read answers the same, in the same instant: the two paths cannot disagree.
        let (status, read) = call(&router, "GET", &format!("/v1/{kind}/{id}"), None).await;
        assert_eq!(
            (status, &read["__type"]),
            (404, &json!("ResourceNotFoundException")),
            "case {label:?}: reading the deleted {kind} is a 404, got {status} with {read}"
        );
        assert_eq!(
            message_of(&read),
            NOT_FOUND_MESSAGE,
            "case {label:?}: that refusal carries this service's not-found sentence, got {read}"
        );

        let reported = reported_ids(&router, kind).await;
        assert!(
            !reported.contains(&id),
            "case {label:?}: the deleted {kind} is not listed, got {reported:?}"
        );
        for sibling in &sibling_ids {
            assert!(
                reported.contains(sibling),
                "case {label:?}: the other {kind}s are still listed, got {reported:?}"
            );
        }
    }
}

/// One probe of the gone-resource table: a request about the deleted resource, and the
/// operation it goes to.
struct Probe {
    label: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
}

/// The requests that must answer as they do for an id the store holds nothing for, once
/// the resource's deletion has completed: the delete itself, a read, a mutation, a tag
/// write, a tag delete, and the resource's own sub-resources.
fn gone_probes(kind: &str, id: &str, arn: &str) -> Vec<Probe> {
    let mut probes = vec![
        Probe {
            label: "repeating the completed delete",
            method: "DELETE",
            path: format!("/v1/{kind}/{id}"),
            body: None,
        },
        Probe {
            label: "reading the resource",
            method: "GET",
            path: format!("/v1/{kind}/{id}"),
            body: None,
        },
        Probe {
            label: "tagging the resource by its ARN",
            method: "POST",
            path: tags_path(arn),
            body: Some(json!({"tags": tag_object(&[("env".to_string(), "prod".to_string())])})),
        },
        Probe {
            label: "untagging the resource by its ARN",
            method: "DELETE",
            path: format!("{}?tagKeys={}", tags_path(arn), percent_encode("env")),
            body: None,
        },
    ];
    match kind {
        "feed" => probes.extend([
            Probe {
                label: "overwriting the feed",
                method: "PUT",
                path: format!("/v1/feed/{id}"),
                body: Some(json!({
                    "name": "resurrected",
                    "outputs": [enabled_output("crop1", "cropping")],
                })),
            },
            Probe {
                label: "reading the feed's policy",
                method: "GET",
                path: format!("/v1/feed/{id}/policy"),
                body: None,
            },
            Probe {
                label: "attaching a policy to the feed",
                method: "PUT",
                path: format!("/v1/feed/{id}/policy"),
                body: Some(json!({"policy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}"})),
            },
        ]),
        "dictionary" => probes.extend([
            Probe {
                label: "updating the dictionary",
                method: "PATCH",
                path: format!("/v1/dictionary/{id}"),
                body: Some(json!({"name": "resurrected"})),
            },
            Probe {
                label: "exporting the dictionary's entries",
                method: "GET",
                path: format!("/v1/dictionary/{id}/entries/export"),
                body: None,
            },
        ]),
        other => panic!("no such resource kind: {other}"),
    }
    probes
}

/// Once the deletion has completed, EVERY operation that takes the id answers for it as
/// it does for an id the store holds nothing for — and so does another Region's endpoint,
/// which never held the resource at all.
#[tokio::test]
async fn a_completed_deletion_leaves_nothing_any_operation_resolves() {
    for kind in ["feed", "dictionary"] {
        let router = router();
        let (id, arn) = create(&router, kind, sibling_body(kind, "doomed")).await;
        let (status, _) = call(&router, "DELETE", &format!("/v1/{kind}/{id}"), None).await;
        assert_eq!(
            status, 202,
            "case {kind:?}: the delete is accepted, got {status}"
        );
        past_the_transition_span();

        for probe in gone_probes(kind, &id, &arn) {
            let (status, body) = call(&router, probe.method, &probe.path, probe.body.clone()).await;
            assert_eq!(
                (status, &body["__type"]),
                (404, &json!("ResourceNotFoundException")),
                "case {kind:?}/{}: {} is a 404 ResourceNotFoundException, got {status} with \
                 {body}",
                probe.label,
                probe.method
            );
            assert_eq!(
                message_of(&body),
                NOT_FOUND_MESSAGE,
                "case {kind:?}/{}: that refusal carries this service's not-found sentence, got \
                 {body}",
                probe.label
            );
        }

        let (status, elsewhere) = call_in_region(
            &router,
            "eu-west-1",
            "DELETE",
            &format!("/v1/{kind}/{id}"),
            None,
        )
        .await;
        assert_eq!(
            (status, &elsewhere["__type"]),
            (404, &json!("ResourceNotFoundException")),
            "case {kind:?}: another Region's endpoint holds no such resource, got {status} \
             with {elsewhere}"
        );
    }
}

/// A completed deletion holds none of the Region's quota and leaves no record behind: the
/// slot it occupied takes another feed, and the Region reports only live feeds.
#[tokio::test]
async fn a_completed_deletion_holds_no_quota() {
    let router = router();
    let mut ids = Vec::new();
    for index in 0..MAX_FEEDS {
        let (id, _) = create(&router, "feed", sibling_body("feed", &format!("f{index}"))).await;
        ids.push(id);
    }

    // The Region is full, so one more feed is refused with the modelled quota error.
    let (status, refused) = call(
        &router,
        "POST",
        "/v1/feed",
        Some(sibling_body("feed", "onetoomany")),
    )
    .await;
    assert_eq!(
        (status, &refused["__type"]),
        (402, &json!("ServiceQuotaExceededException")),
        "a create past the quota is refused, got {status} with {refused}"
    );

    let doomed = ids[0].clone();
    let (status, _) = call(&router, "DELETE", &format!("/v1/feed/{doomed}"), None).await;
    assert_eq!(status, 202, "the delete is accepted, got {status}");
    past_the_transition_span();

    // The deletion has completed, so the Region holds one feed fewer and takes another.
    let (status, created) = call(
        &router,
        "POST",
        "/v1/feed",
        Some(sibling_body("feed", "replacement")),
    )
    .await;
    assert_eq!(
        status, 202,
        "the slot a completed deletion freed takes another feed, got {status} with {created}"
    );
    let (status, repeated) = call(&router, "DELETE", &format!("/v1/feed/{doomed}"), None).await;
    assert_eq!(
        (status, &repeated["__type"]),
        (404, &json!("ResourceNotFoundException")),
        "the completed deletion left no record for the delete to report, got {status} with \
         {repeated}"
    );
    let reported = reported_ids(&router, "feed").await;
    assert_eq!(
        reported.len(),
        MAX_FEEDS,
        "the Region reports {MAX_FEEDS} feeds in {REGION}, got {}",
        reported.len()
    );
    assert!(
        !reported.contains(&doomed),
        "the deleted feed is not among them, got {reported:?}"
    );
}
