// SPDX-License-Identifier: CC0-1.0

//! Which tag keys a CALLER may name, over every operation that writes or removes
//! a tag.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `networkmonitor` SDK client sees. The harness
//! (`router`/`call`/…) is the one `monitor_and_probe` owns, and the tagging
//! requests are the ones `resource_tags` owns.
//!
//! THE RULE: the `aws:` tag-key namespace belongs to Amazon Web Services' own
//! tags, so a key in it is not a caller's to create, change or remove — a
//! request naming one is refused with HTTP 400 `ValidationException` and leaves
//! the stored tags exactly as they were, whichever operation carried the key and
//! wherever in the request it sat. The prefix is reserved in any mixture of
//! cases, and a key that merely CONTAINS it, or carries it without the colon, is
//! an ordinary key the service stores.
//!
//! The refusal is stated the way this service states every rule it authors
//! itself (see `authored_refusal_messages`): AWS's constraint-violation
//! construction, naming the member path the caller sent the key at — an entry of
//! a tag map (`tags.aws:owner`, `probe.tags.aws:owner`,
//! `probes.1.member.probeTags.aws:owner`) or an element of a key list
//! (`tagKeys.1.member`).
//!
//! The table spans the class the rule covers: every operation that assigns tags
//! (`CreateMonitor`'s `tags` and each nested probe's `probeTags`, `CreateProbe`'s
//! `tags` and `probe.tags`, `TagResource` on both resource kinds the model's
//! `resourceArn` references) and the one that removes them (`UntagResource`),
//! several reserved keys (the bare prefix, the reported `aws:createdBy`, a
//! service-qualified key the issue did not cite, upper- and mixed-case spellings
//! of the prefix), a reserved key beside an ordinary one, and a reserved key on
//! an ARN nothing stores.

use serde_json::{json, Value};

use super::monitor_and_probe::{
    create_monitor, create_probe, error_code, error_message, get_monitor, probe_json, probes_of,
    router, source_arn, string_at, ACCOUNT, REGION,
};
use super::resource_tags::{fixture, list_tags, tag_map, tag_resource, untag_resource, Target};

/// The tag-key prefix Amazon Web Services reserves for the tags it applies
/// itself. From AWS's tagging rules, which reserve it across every service: a
/// caller can neither create a tag whose key carries it nor change or remove
/// one.
const RESERVED_PREFIX: &str = "aws:";

/// The envelope AWS wraps a single constraint clause in.
const ONE_ERROR: &str = "1 validation error detected: ";

/// The requirement the clause states for this rule.
const TAG_KEY_REQUIREMENT: &str = "Member must not begin with the reserved prefix aws:";

/// The whole message a refused tag key is stated with.
fn one_error(value: &str, member_path: &str) -> String {
    format!(
        "{ONE_ERROR}Value '{value}' at '{member_path}' failed to satisfy constraint: \
         {TAG_KEY_REQUIREMENT}"
    )
}

/// What a row sends, since each operation carries a tag key in a different
/// place.
enum Request {
    /// `TagResource` on the fixture's monitor or probe.
    Tag { target: Target, tags: Value },
    /// `TagResource` on an ARN this service never minted.
    TagUnknown { tags: Value },
    /// `UntagResource` on the fixture's monitor or probe.
    Untag {
        target: Target,
        keys: &'static [&'static str],
    },
    /// `CreateMonitor` with the named body.
    Create { body: String },
    /// `CreateProbe` on the fixture's monitor with the named body.
    Probe { body: String },
}

/// One row: what it exercises, the request it makes, the key the clause must
/// echo, and the member path it must name.
struct Case {
    case: &'static str,
    request: Request,
    key: &'static str,
    member_path: String,
}

/// A monitor ARN this service never minted, for the row that pins the order the
/// request is judged in.
fn unknown_arn() -> String {
    format!("arn:aws:networkmonitor:{REGION}:{ACCOUNT}:monitor/nothing-stores-this")
}

/// Send one row's request.
async fn send(
    router: &smithy_server_runtime::MultiServiceRouter,
    monitor_arn: &str,
    probe_arn: &str,
    monitor_name: &str,
    request: &Request,
) -> (u16, Value) {
    match request {
        Request::Tag { target, tags } => {
            let arn = match target {
                Target::Monitor => monitor_arn,
                Target::Probe => probe_arn,
            };
            tag_resource(router, arn, tags).await
        }
        Request::TagUnknown { tags } => tag_resource(router, &unknown_arn(), tags).await,
        Request::Untag { target, keys } => {
            let arn = match target {
                Target::Monitor => monitor_arn,
                Target::Probe => probe_arn,
            };
            untag_resource(router, arn, keys).await
        }
        Request::Create { body } => create_monitor(router, body).await,
        Request::Probe { body } => create_probe(router, monitor_name, body).await,
    }
}

/// The tags `ListTagsForResource` reports for one ARN.
async fn stored_tags(
    router: &smithy_server_runtime::MultiServiceRouter,
    resource_arn: &str,
) -> Value {
    let (status, listed) = list_tags(router, resource_arn).await;
    assert_eq!(
        status, 200,
        "expected ListTagsForResource 200 for {resource_arn}, got {status} {listed:?}"
    );
    listed.get("tags").cloned().unwrap_or(Value::Null)
}

/// A key in AWS's reserved namespace is refused wherever a request names one,
/// with the clause naming the member the caller sent it at — and nothing is
/// written, added or removed.
#[tokio::test]
async fn a_reserved_tag_key_is_refused_wherever_a_request_names_one() {
    let router = router();
    let monitor_name = "reserved-host";
    let monitor_tags = json!({"team": "net"});
    let probe_tags = json!({"probe": "one"});
    let (monitor_arn, probe_arn) =
        fixture(&router, monitor_name, &monitor_tags, &probe_tags).await;

    let cases: Vec<Case> = vec![
        Case {
            case: "the reported case: TagResource on a monitor",
            request: Request::Tag {
                target: Target::Monitor,
                tags: json!({"aws:createdBy": "someone"}),
            },
            key: "aws:createdBy",
            member_path: "tags.aws:createdBy".to_string(),
        },
        Case {
            case: "TagResource on a probe, the bare reserved prefix as a key",
            request: Request::Tag {
                target: Target::Probe,
                tags: json!({"aws:": "someone"}),
            },
            key: "aws:",
            member_path: "tags.aws:".to_string(),
        },
        Case {
            case: "TagResource with a service-qualified reserved key beside an ordinary one",
            request: Request::Tag {
                target: Target::Monitor,
                tags: json!({"owner": "net", "aws:cloudformation:stack-name": "stack"}),
            },
            key: "aws:cloudformation:stack-name",
            member_path: "tags.aws:cloudformation:stack-name".to_string(),
        },
        Case {
            case: "TagResource with the prefix upper-cased",
            request: Request::Tag {
                target: Target::Monitor,
                tags: json!({"AWS:owner": "someone"}),
            },
            key: "AWS:owner",
            member_path: "tags.AWS:owner".to_string(),
        },
        Case {
            case: "TagResource with the prefix in mixed case",
            request: Request::Tag {
                target: Target::Probe,
                tags: json!({"Aws:Owner": "someone"}),
            },
            key: "Aws:Owner",
            member_path: "tags.Aws:Owner".to_string(),
        },
        Case {
            case: "TagResource on an ARN nothing stores: the request is judged first",
            request: Request::TagUnknown {
                tags: json!({"aws:owner": "someone"}),
            },
            key: "aws:owner",
            member_path: "tags.aws:owner".to_string(),
        },
        Case {
            case: "UntagResource naming a reserved key",
            request: Request::Untag {
                target: Target::Monitor,
                keys: &["aws:createdBy"],
            },
            key: "aws:createdBy",
            member_path: "tagKeys.1.member".to_string(),
        },
        Case {
            case: "UntagResource naming a reserved key after a stored one",
            request: Request::Untag {
                target: Target::Probe,
                keys: &["probe", "aws:cloudformation:stack-name"],
            },
            key: "aws:cloudformation:stack-name",
            member_path: "tagKeys.2.member".to_string(),
        },
        Case {
            case: "CreateMonitor's own tags",
            request: Request::Create {
                body: json!({"monitorName": "reserved-create", "tags": {"aws:owner": "me"}})
                    .to_string(),
            },
            key: "aws:owner",
            member_path: "tags.aws:owner".to_string(),
        },
        Case {
            case: "CreateMonitor's first nested probe's probeTags",
            request: Request::Create {
                body: json!({
                    "monitorName": "reserved-nested-first",
                    "probes": [{
                        "sourceArn": source_arn(),
                        "destination": "10.0.0.1",
                        "protocol": "ICMP",
                        "probeTags": tag_map(&[("aws:owner", "me")]),
                    }],
                })
                .to_string(),
            },
            key: "aws:owner",
            member_path: "probes.1.member.probeTags.aws:owner".to_string(),
        },
        Case {
            case: "CreateMonitor's second nested probe's probeTags",
            request: Request::Create {
                body: json!({
                    "monitorName": "reserved-nested-second",
                    "probes": [
                        {
                            "sourceArn": source_arn(),
                            "destination": "10.0.0.1",
                            "protocol": "ICMP",
                            "probeTags": tag_map(&[("team", "net")]),
                        },
                        {
                            "sourceArn": source_arn(),
                            "destination": "10.0.0.2",
                            "protocol": "ICMP",
                            "probeTags": tag_map(&[("AWS:managedBy", "me")]),
                        },
                    ],
                })
                .to_string(),
            },
            key: "AWS:managedBy",
            member_path: "probes.2.member.probeTags.AWS:managedBy".to_string(),
        },
        Case {
            case: "CreateProbe's top-level tags",
            request: Request::Probe {
                body: format!(
                    r#"{{"probe":{},"tags":{{"aws:createdBy":"me"}}}}"#,
                    probe_json("10.0.0.5", "ICMP", "")
                ),
            },
            key: "aws:createdBy",
            member_path: "tags.aws:createdBy".to_string(),
        },
        Case {
            case: "CreateProbe's probe.tags",
            request: Request::Probe {
                body: format!(
                    r#"{{"probe":{}}}"#,
                    probe_json("10.0.0.6", "ICMP", r#","tags":{"aws:owner":"me"}"#)
                ),
            },
            key: "aws:owner",
            member_path: "probe.tags.aws:owner".to_string(),
        },
    ];

    for row in &cases {
        let case = row.case;
        let (status, refused) = send(
            &router,
            &monitor_arn,
            &probe_arn,
            monitor_name,
            &row.request,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused:?}"
        );
        let expected = one_error(row.key, &row.member_path);
        let message = error_message(&refused);
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );
        assert!(
            !message.contains('"'),
            "case {case:?}: a refusal must carry no Rust Debug quoting, got {message:?}"
        );
    }

    // Nothing any refused row carried was written: the fixture's resources carry
    // exactly the tags they were created with, so no ordinary key that travelled
    // beside a reserved one was stored and no named key was removed.
    let listed_monitor = stored_tags(&router, &monitor_arn).await;
    assert_eq!(
        listed_monitor, monitor_tags,
        "expected the monitor's tags to be untouched, got {listed_monitor:?}"
    );
    let listed_probe = stored_tags(&router, &probe_arn).await;
    assert_eq!(
        listed_probe, probe_tags,
        "expected the probe's tags to be untouched, got {listed_probe:?}"
    );
    // The same stored maps a read of the resource reports.
    let (status, got) = get_monitor(&router, monitor_name).await;
    assert_eq!(
        status, 200,
        "expected GetMonitor 200 for the fixture, got {status} {got:?}"
    );
    assert_eq!(
        got.get("tags").cloned().unwrap_or(Value::Null),
        monitor_tags,
        "expected GetMonitor to report the untouched monitor tags, got {got:?}"
    );
    let probes = probes_of(&got);
    assert_eq!(
        probes.len(),
        1,
        "a refused CreateProbe must attach no probe, got {got:?}"
    );
    assert_eq!(
        probes[0].get("tags").cloned().unwrap_or(Value::Null),
        probe_tags,
        "expected GetMonitor to report the untouched probe tags, got {got:?}"
    );
    // A refused create stores no monitor at all.
    for name in [
        "reserved-create",
        "reserved-nested-first",
        "reserved-nested-second",
    ] {
        let (status, missing) = get_monitor(&router, name).await;
        assert_eq!(
            status, 404,
            "a refused CreateMonitor must store no monitor {name}, got {status} {missing:?}"
        );
    }
}

/// A key that is NOT in the reserved namespace is stored, reported and
/// removable: the rule is about the namespace, not about refusing keys that
/// mention it.
///
/// The rows differ in HOW each key comes close to the prefix without carrying
/// it: the prefix's letters with no colon, with a different separator, as a
/// longer word, upper-cased without the colon, and the prefix appearing inside
/// the key rather than at its start.
#[tokio::test]
async fn a_key_outside_the_reserved_namespace_is_stored_reported_and_removable() {
    // (case, key)
    let allowed: &[(&str, &str)] = &[
        ("the prefix's letters without its colon", "aws"),
        ("the prefix's letters with another separator", "aws-owner"),
        ("the prefix's letters inside a longer word", "awsome"),
        ("the prefix upper-cased without its colon", "AWSOME"),
        ("the reserved text inside the key, not at its start", "team:aws:region"),
        ("a key whose first character precedes the prefix", "-aws:owner"),
    ];

    let router = router();
    let monitor_name = "allowed-host";
    let (monitor_arn, probe_arn) =
        fixture(&router, monitor_name, &json!({}), &json!({})).await;

    for (case, key) in allowed {
        assert!(
            !key.to_ascii_lowercase().starts_with(RESERVED_PREFIX),
            "case {case:?}: the row must name a key outside the reserved namespace, got {key:?}"
        );
        let pair = tag_map(&[(*key, "kept")]);
        for (target, arn) in [
            (Target::Monitor, monitor_arn.as_str()),
            (Target::Probe, probe_arn.as_str()),
        ] {
            let (status, written) = tag_resource(&router, arn, &pair).await;
            assert_eq!(
                status, 204,
                "case {case:?} on {target:?}: expected TagResource 204, got {status} {written:?}"
            );
            let listed = stored_tags(&router, arn).await;
            assert_eq!(
                listed, pair,
                "case {case:?} on {target:?}: expected the key to be stored, got {listed:?}"
            );
            let (status, removed) = untag_resource(&router, arn, &[key]).await;
            assert_eq!(
                status, 204,
                "case {case:?} on {target:?}: expected UntagResource 204, got {status} {removed:?}"
            );
            let listed = stored_tags(&router, arn).await;
            assert_eq!(
                listed,
                json!({}),
                "case {case:?} on {target:?}: expected the key to be removed, got {listed:?}"
            );
        }
    }

    // The same keys travel on the create paths, and read back off the stored
    // records.
    for (index, (case, key)) in allowed.iter().enumerate() {
        let name = format!("allowed-create-{index}");
        let pair = tag_map(&[(*key, "kept")]);
        let body = json!({
            "monitorName": name,
            "tags": pair,
            "probes": [{
                "sourceArn": source_arn(),
                "destination": "10.0.0.1",
                "protocol": "ICMP",
                "probeTags": pair,
            }],
        })
        .to_string();
        let (status, created) = create_monitor(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateMonitor 200, got {status} {created:?}"
        );
        let (status, got) = get_monitor(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetMonitor 200, got {status} {got:?}"
        );
        assert_eq!(
            got.get("tags").cloned().unwrap_or(Value::Null),
            pair,
            "case {case:?}: expected the created monitor to carry the key, got {got:?}"
        );
        let probes = probes_of(&got);
        assert_eq!(
            probes.len(),
            1,
            "case {case:?}: expected the nested probe to be attached, got {got:?}"
        );
        assert_eq!(
            probes[0].get("tags").cloned().unwrap_or(Value::Null),
            pair,
            "case {case:?}: expected the nested probe to carry the key, got {got:?}"
        );
        // And on `CreateProbe`, where the key can ride on either member.
        let probe_body = format!(
            r#"{{"probe":{},"tags":{pair}}}"#,
            probe_json("10.0.0.9", "ICMP", "")
        );
        let (status, probe) = create_probe(&router, &name, &probe_body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateProbe 200, got {status} {probe:?}"
        );
        assert_eq!(
            probe.get("tags").cloned().unwrap_or(Value::Null),
            pair,
            "case {case:?}: expected the minted probe to carry the key, got {probe:?}"
        );
        assert!(
            !string_at(&probe, "probeId").is_empty(),
            "case {case:?}: expected the minted probe to carry an id, got {probe:?}"
        );
    }
}
