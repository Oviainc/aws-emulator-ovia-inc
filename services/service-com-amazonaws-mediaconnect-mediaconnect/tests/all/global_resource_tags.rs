// SPDX-License-Identifier: CC0-1.0

//! `TagGlobalResource` end to end over HTTP, against every global resource the operation
//! documents: router inputs, router outputs and router network interfaces.
//!
//! The subject is the rule: a tag write MERGES the pairs it names into the stored tag map
//! of the one global resource its `ResourceArn` addresses — a key it names is set or
//! overwritten, a key it does not name keeps the value it had — and the resource's own
//! implemented reader afterwards reports that merged map. An ARN that addresses no global
//! resource this service holds is the modelled 404, never a 501 and never a success.
//!
//! The cases vary what those rules generalise over: which of the three global resource
//! kinds carries the tags, how many pairs one request names (none, one, several, up to the
//! tags-per-resource bound), whether a key is new or overwrites, the form of the key and of
//! the value (ASCII, non-ASCII, empty, URI-significant, at the documented length bounds and
//! one character past them), and the ways an ARN can address nothing.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case drives the same request path a MediaConnect client reaches: routing, restJson1
//! deserialization, constraint validation, handler, serialization.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};
// The account id, the percent-encoder, the length-bound filler, the tag-map builder and
// the reply triple the REGIONAL tagging suite owns: one definition of each in this test
// binary, so the two suites cannot drift apart.
use crate::resource_tags::{filler, pct, tag_map, Reply, ACCOUNT};
use crate::router_input_restart::union;
// The interface ARN no create minted, and the create that mints a real one: a STANDARD
// router input / output configuration names a `NetworkInterfaceArn` the create resolves
// against the store, so a subject of either kind mints its interface first, through the one
// helper the router-output suite owns.
use crate::router_network_interfaces::{public_configuration, UNKNOWN_ARN};
use crate::router_outputs::minted_network_interface;

/// Which global resource a subject is — "the API supports the following global resources:
/// router inputs, router outputs and router network interfaces"
/// (`smithy_query(query="operation",operation="TagGlobalResource")`). Each one has a
/// different implemented reader, which is the only thing the kind decides here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GlobalKind {
    Input,
    Output,
    NetworkInterface,
}

/// Every kind, so a rule is asserted over the whole class the operation covers.
pub(crate) const EVERY_KIND: [GlobalKind; 3] = [
    GlobalKind::Input,
    GlobalKind::Output,
    GlobalKind::NetworkInterface,
];

/// `POST /tags/global/{ResourceArn}` with the pairs in `tags`.
///
/// `pub(crate)` because the global tag READING suite drives the same write, to assert that
/// the report answers what the write merged.
pub(crate) async fn tag_global(emulator: &Emulator, resource_arn: &str, tags: Value) -> Reply {
    emulator
        .send(
            "POST",
            &format!("/tags/global/{}", pct(resource_arn)),
            Some(json!({ "tags": tags })),
        )
        .await
}

/// `POST /tags/{ResourceArn}` — the REGIONAL tag write, for the cases that pin which
/// operation reaches which resource.
///
/// `pub(crate)` because the tag-removal suite tags its REGIONAL subject through this same
/// write (`tests/all/tag_removal_without_tag_keys.rs`), so both suites put tags on a
/// resource with one request shape.
pub(crate) async fn tag_regional(emulator: &Emulator, resource_arn: &str, tags: Value) -> Reply {
    emulator
        .send(
            "POST",
            &format!("/tags/{}", pct(resource_arn)),
            Some(json!({ "tags": tags })),
        )
        .await
}

/// Create one global resource of `kind`, carrying the `tags` member when `tags` names one
/// and no such member at all when it is `None`, and return its minted ARN.
///
/// `pub(crate)` because the GLOBAL tag READING suite creates the same three subjects: one
/// definition of each create in this test binary, so a rule asserted about a kind is
/// asserted about the same request both suites send.
pub(crate) async fn create_subject(
    emulator: &Emulator,
    kind: GlobalKind,
    name: &str,
    tags: Option<Value>,
) -> String {
    let (uri, mut body, pointer) = match kind {
        GlobalKind::Input => (
            "/v1/routerInput",
            json!({
                "name": name,
                "configuration": union(
                    "Standard",
                    "standard",
                    json!({
                        "networkInterfaceArn":
                            minted_network_interface(emulator, &format!("{name}-input-nic"))
                                .await,
                        "protocol": "RTP",
                        "protocolConfiguration": union("Rtp", "rtp", json!({ "port": 5000 })),
                    }),
                ),
                "maximumBitrate": 20_000_000i64,
                "routingScope": "REGIONAL",
                "tier": "INPUT_20",
            }),
            "routerInput",
        ),
        GlobalKind::Output => (
            "/v1/routerOutput",
            json!({
                "name": name,
                "configuration": union(
                    "Standard",
                    "standard",
                    json!({
                        "networkInterfaceArn":
                            minted_network_interface(emulator, &format!("{name}-output-nic"))
                                .await,
                        "protocolConfiguration": union(
                            "Rtp",
                            "rtp",
                            json!({ "destinationAddress": "198.51.100.10", "destinationPort": 5100 }),
                        ),
                    }),
                ),
                "maximumBitrate": 20_000_000i64,
                "routingScope": "REGIONAL",
                "tier": "OUTPUT_20",
            }),
            "routerOutput",
        ),
        GlobalKind::NetworkInterface => (
            "/v1/routerNetworkInterface",
            json!({
                "name": name,
                "configuration": public_configuration(1),
            }),
            "routerNetworkInterface",
        ),
    };
    if let Some(tags) = tags {
        body["tags"] = tags;
    }
    let (status, code, created) = emulator.send("POST", uri, Some(body)).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "creating the {kind:?} fixture {name:?}: expected 201, got {status} {code:?}: {created}"
    );
    created[pointer]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The resource `arn` addresses, as the IMPLEMENTED reader for its kind reports it —
/// GetRouterInput, GetRouterOutput and BatchGetRouterNetworkInterface respectively, each of
/// which computes its answer from the stored record.
///
/// `pub(crate)` because the global UNTAGGING suite reads the same three records back, so
/// there is one reader per kind in this test binary rather than one per suite.
pub(crate) async fn read_back(
    emulator: &Emulator,
    case: &str,
    kind: GlobalKind,
    arn: &str,
) -> Value {
    let uri = match kind {
        GlobalKind::Input => format!("/v1/routerInput/{arn}"),
        GlobalKind::Output => format!("/v1/routerOutput/{arn}"),
        GlobalKind::NetworkInterface => {
            format!("/v1/routerNetworkInterfaces?arns={arn}")
        }
    };
    let (status, code, body) = emulator.send("GET", &uri, None).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: reading the {kind:?} {arn} back expected 200, got {status} {code:?}: \
         {body}"
    );
    match kind {
        GlobalKind::Input => body["routerInput"].clone(),
        GlobalKind::Output => body["routerOutput"].clone(),
        GlobalKind::NetworkInterface => {
            // The batch reader answers a LIST, so the entry read here is asserted to be
            // the interface the request named rather than whatever came first.
            let reported = body["routerNetworkInterfaces"][0].clone();
            assert_eq!(
                reported["arn"].as_str(),
                Some(arn),
                "case {case:?}: the batch read must report the interface the request named, \
                 got {reported}"
            );
            reported
        }
    }
}

/// The `Tags` the reader reports for a resource.
fn reported_tags(case: &str, resource: &Value) -> BTreeMap<String, String> {
    let tags = resource["tags"].as_object().unwrap_or_else(|| {
        panic!("case {case:?}: a global resource reports a Tags map, got {resource}")
    });
    tags.iter()
        .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
        .collect()
}

/// The tags the implemented reader reports for `arn`.
async fn tags_of(emulator: &Emulator, case: &str, kind: GlobalKind, arn: &str) -> BTreeMap<String, String> {
    let resource = read_back(emulator, case, kind, arn).await;
    reported_tags(case, &resource)
}

/// A timestamp a reader reports, in seconds since the epoch (this emulator's codec renders
/// every timestamp in restJson1's default epoch-seconds form).
///
/// `pub(crate)` because the global UNTAGGING suite asserts the same `UpdatedAt` rule.
pub(crate) fn timestamp(case: &str, member: &str, resource: &Value) -> f64 {
    resource[member].as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: {member} must be reported as a timestamp, got {resource}")
    })
}

/// One subject of each kind, each created with its own starting tags, plus the starting
/// tags each one carries.
async fn subjects(emulator: &Emulator) -> Vec<(GlobalKind, String, BTreeMap<String, String>)> {
    let mut created = Vec::new();
    for (index, kind) in EVERY_KIND.into_iter().enumerate() {
        let initial = BTreeMap::from([
            ("env".to_string(), "prod".to_string()),
            ("team".to_string(), format!("video-{index}")),
        ]);
        let tags = tag_map(&initial.iter().map(|(k, v)| (k.clone(), v.clone())).collect::<Vec<_>>());
        let arn = create_subject(emulator, kind, &format!("tagged-{index}"), Some(tags)).await;
        created.push((kind, arn, initial));
    }
    created
}

/// The tag writes one subject is driven through. Each row differs from the others along
/// what the merge rule generalises over: how many pairs the request carries, whether a key
/// is new or overwrites one the resource already has, and the form of the key and value.
fn merge_rows() -> Vec<(&'static str, Vec<(String, String)>)> {
    vec![
        ("no pairs at all", vec![]),
        ("one new key", vec![("owner".into(), "ops".into())]),
        (
            "overwrite a key the resource already carries",
            vec![("env".into(), "staging".into())],
        ),
        (
            "two pairs at once: one overwrite, one new key",
            vec![
                ("env".into(), "prod".into()),
                ("cost-center".into(), "cc-42".into()),
            ],
        ),
        (
            "non-ASCII key and value",
            vec![("проект".into(), "видео-поток".into())],
        ),
        ("an empty value", vec![("blank".into(), String::new())]),
        (
            "key and value at the documented maximum character lengths",
            vec![(
                filler(mc::MAX_TAG_KEY_LENGTH, 3),
                filler(mc::MAX_TAG_VALUE_LENGTH, 7),
            )],
        ),
        (
            "URI-significant characters in the key and the value",
            vec![("path/key".into(), "a:b c&d=e".into())],
        ),
        (
            "re-stating a key with the value it already has",
            vec![("owner".into(), "ops".into())],
        ),
        (
            "three pairs, all new",
            vec![
                ("a".into(), "1".into()),
                ("b".into(), "2".into()),
                ("c".into(), "3".into()),
            ],
        ),
    ]
}

/// A tag write merges the pairs it names into the stored tags of the ONE global resource it
/// addresses, whichever of the three kinds that is, and the resource's implemented reader
/// reports exactly that merged map. The expected map is maintained by the rule itself
/// (insert the named keys, touch nothing else), so each row asserts the rule rather than a
/// recorded answer — and every other subject is re-read each round, so a write that reached
/// the wrong resource, or a read that changed one, fails here.
#[tokio::test]
async fn tagging_a_global_resource_merges_the_named_keys_into_its_stored_tags() {
    let emulator = Emulator::new();
    let created = subjects(&emulator).await;

    let mut expected: BTreeMap<String, BTreeMap<String, String>> = created
        .iter()
        .map(|(_, arn, initial)| (arn.clone(), initial.clone()))
        .collect();

    for (kind, arn, _) in &created {
        for (row, pairs) in merge_rows() {
            let case = format!("{kind:?} / {row}");
            let (status, code, body) = tag_global(&emulator, arn, tag_map(&pairs)).await;
            assert_eq!(
                (status, code.as_deref()),
                (204, None),
                "case {case:?}: TagGlobalResource expected 204 with no error, got {status} \
                 {code:?}: {body}"
            );

            let want = expected
                .get_mut(arn)
                .expect("every subject has an expected tag map");
            for (key, value) in pairs {
                want.insert(key, value);
            }

            // Every subject, not just the one written to: the write addressed one resource,
            // and reading a resource changes nothing a later caller sees.
            for (other_kind, other_arn, _) in &created {
                let reported = tags_of(&emulator, &case, *other_kind, other_arn).await;
                let want = expected
                    .get(other_arn)
                    .expect("every subject has an expected tag map");
                assert_eq!(
                    &reported, want,
                    "case {case:?}: {other_kind:?} {other_arn} must report its own merged tag \
                     map, got {reported:?}"
                );
            }
        }
    }
}

/// A `ResourceArn` that addresses no GLOBAL resource this service holds is refused with the
/// modelled `NotFoundException` (404) — never a 501 and never a success. The rows are the
/// ways an ARN can address nothing: each of the three global ARN forms with an id no create
/// minted, a REGIONAL resource this service really holds (a flow, which the regional tag
/// operations reach instead), another service's ARN, and a string that is no ARN at all.
#[tokio::test]
async fn tagging_a_global_arn_that_addresses_nothing_is_not_found() {
    let emulator = Emulator::new();

    // A flow this service really holds: it is a regional resource, so the GLOBAL tag write
    // does not reach it.
    let (status, code, created) = emulator
        .create_flow(json!({
            "name": "regional-flow",
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
        }))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "creating the fixture flow expected 201, got {status} {code:?}: {created}"
    );
    let flow_arn = created["flow"]["flowArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let rows = [
        (
            "a router input ARN no create minted",
            format!("arn:aws:mediaconnect:{REGION}:{ACCOUNT}:routerInput:ffffffffffff"),
        ),
        (
            "a router output ARN no create minted",
            format!("arn:aws:mediaconnect:{REGION}:{ACCOUNT}:routerOutput:eeeeeeeeeeee"),
        ),
        (
            "a router network interface ARN no create minted",
            UNKNOWN_ARN.to_string(),
        ),
        (
            "a flow ARN this service does hold: a flow is not a global resource",
            flow_arn.clone(),
        ),
        (
            "another service's ARN",
            format!("arn:aws:medialive:{REGION}:{ACCOUNT}:channel:1234567"),
        ),
        ("a string that is no ARN at all", "not-an-arn".to_string()),
    ];

    for (case, arn) in rows {
        let (status, code, body) = tag_global(&emulator, &arn, json!({ "k": "v" })).await;
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("NotFoundException")),
            "case {case:?}: expected 404 NotFoundException, got {status} {code:?}: {body}"
        );
    }

    // The other half of the split, pinned so it cannot drift: the REGIONAL tag write does
    // not reach a global resource either.
    let router_input = create_subject(
        &emulator,
        GlobalKind::Input,
        "not-regional",
        Some(json!({})),
    )
    .await;
    let (status, code, body) = tag_regional(&emulator, &router_input, json!({ "k": "v" })).await;
    assert_eq!(
        (status, code.as_deref()),
        (404, Some("NotFoundException")),
        "TagResource does not reach a router input, got {status} {code:?}: {body}"
    );
    // And the global write on that same ARN is served.
    let (status, code, body) = tag_global(&emulator, &router_input, json!({ "k": "v" })).await;
    assert_eq!(
        (status, code.as_deref()),
        (204, None),
        "TagGlobalResource reaches a router input, got {status} {code:?}: {body}"
    );

    // Precedence, pinned so it cannot drift: a pair that breaks the tag-pair bound is
    // refused on the pair (400) rather than on the ARN (404), because the request is
    // checked before any resource is resolved.
    let unknown = format!("arn:aws:mediaconnect:{REGION}:{ACCOUNT}:routerInput:ffffffffffff");
    let (status, code, body) = tag_global(
        &emulator,
        &unknown,
        tag_map(&[(filler(mc::MAX_TAG_KEY_LENGTH + 1, 9), "v".to_string())]),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "an over-long key on an unknown ARN is refused on the pair, got {status} {code:?}: {body}"
    );
}

/// A global resource the clock has already taken away is a resource no tag write reaches:
/// once DeleteRouterInput's removal has come due, the ARN addresses nothing and the write
/// is the modelled 404 — while the input is still on its way out it is still there.
#[tokio::test]
async fn tagging_a_router_input_the_clock_removed_is_not_found() {
    let emulator = Emulator::new();
    let arn = create_subject(&emulator, GlobalKind::Input, "doomed", Some(json!({}))).await;

    let (status, code, body) = emulator
        .send("DELETE", &format!("/v1/routerInput/{arn}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "deleting the fixture router input expected 200, got {status} {code:?}: {body}"
    );

    // Still on its way out: the record exists, so the write is served.
    let (status, code, body) = tag_global(&emulator, &arn, json!({ "while": "deleting" })).await;
    assert_eq!(
        (status, code.as_deref()),
        (204, None),
        "tagging an input that is still being deleted expected 204, got {status} {code:?}: {body}"
    );

    // Past the span the removal was owed at, the record is gone and so is its tag map.
    tokio::time::sleep(mc::TRANSITION_SPAN * 2).await;
    let (status, code, body) = tag_global(&emulator, &arn, json!({ "after": "removal" })).await;
    assert_eq!(
        (status, code.as_deref()),
        (404, Some("NotFoundException")),
        "tagging a removed router input expected 404 NotFoundException, got {status} {code:?}: \
         {body}"
    );
}

/// One row of the bounds table: the pairs a tag write carries, and the status and error code
/// the service answers.
struct BoundCase {
    case: &'static str,
    pairs: Vec<(String, String)>,
    status: u16,
    code: Option<&'static str>,
}

/// A tag pair outside the bounds the model's own member documentation states — "tag keys can
/// have a maximum character length of 128 characters, and tag values can have a maximum
/// length of 256 characters" — is refused with `BadRequestException` (400) and stores
/// nothing, and a pair AT either bound is stored. The lengths count characters, so a
/// 256-character multi-byte value is within the bound. Asserted on each global resource
/// kind, because the rule is the service's, not one resource's.
#[tokio::test]
async fn tag_pairs_outside_the_documented_bounds_are_refused() {
    let emulator = Emulator::new();

    for kind in EVERY_KIND {
        let arn = create_subject(&emulator, kind, "bounded", Some(json!({}))).await;
        let rows = vec![
            BoundCase {
                case: "a key at the documented maximum length",
                pairs: vec![(filler(mc::MAX_TAG_KEY_LENGTH, 1), "v".into())],
                status: 204,
                code: None,
            },
            BoundCase {
                case: "a key one character past the maximum length",
                pairs: vec![(filler(mc::MAX_TAG_KEY_LENGTH + 1, 1), "v".into())],
                status: 400,
                code: Some("BadRequestException"),
            },
            BoundCase {
                case: "a value at the documented maximum length",
                pairs: vec![("at-value-bound".into(), filler(mc::MAX_TAG_VALUE_LENGTH, 2))],
                status: 204,
                code: None,
            },
            BoundCase {
                case: "a value one character past the maximum length",
                pairs: vec![(
                    "past-value-bound".into(),
                    filler(mc::MAX_TAG_VALUE_LENGTH + 1, 2),
                )],
                status: 400,
                code: Some("BadRequestException"),
            },
            BoundCase {
                case: "a value of 256 multi-byte characters — the bound counts characters",
                pairs: vec![("multibyte-value".into(), "é".repeat(mc::MAX_TAG_VALUE_LENGTH))],
                status: 204,
                code: None,
            },
            BoundCase {
                case: "an empty key names no tag",
                pairs: vec![(String::new(), "v".into())],
                status: 400,
                code: Some("BadRequestException"),
            },
            BoundCase {
                case: "one good pair beside an over-long key: neither is stored",
                pairs: vec![
                    ("would-be-stored".into(), "v".into()),
                    (filler(mc::MAX_TAG_KEY_LENGTH + 1, 5), "v".into()),
                ],
                status: 400,
                code: Some("BadRequestException"),
            },
        ];

        // The expected map grows only by the rows the rule accepts, so a refused row is
        // also asserted to have stored nothing.
        let mut expected: BTreeMap<String, String> = BTreeMap::new();
        for BoundCase {
            case: row,
            pairs,
            status: want_status,
            code: want_code,
        } in rows
        {
            let case = format!("{kind:?} / {row}");
            let (status, code, body) = tag_global(&emulator, &arn, tag_map(&pairs)).await;
            assert_eq!(
                (status, code.as_deref()),
                (want_status, want_code),
                "case {case:?}: expected {want_status} {want_code:?}, got {status} {code:?}: \
                 {body}"
            );
            if want_status == 204 {
                for (key, value) in pairs {
                    expected.insert(key, value);
                }
            }
            let reported = tags_of(&emulator, &case, kind, &arn).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: only accepted pairs are stored, got {reported:?}"
            );
        }
    }
}

/// The tags-per-resource bound applies to the MERGED set a write would leave on the
/// resource: a write that fills it is accepted, one that would add a key past it is refused
/// with `BadRequestException` (400) and stores nothing, and a write that only re-states keys
/// the resource already carries is accepted at the bound.
#[tokio::test]
async fn a_global_resource_keeps_at_most_the_bounded_number_of_tags() {
    const LIMIT: usize = mc::MAX_TAGS_PER_RESOURCE;

    let emulator = Emulator::new();
    let arn = create_subject(
        &emulator,
        GlobalKind::Output,
        "full-of-tags",
        Some(json!({})),
    )
    .await;

    let at_bound: Vec<(String, String)> = (0..LIMIT)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect();
    let (status, code, body) = tag_global(&emulator, &arn, tag_map(&at_bound)).await;
    assert_eq!(
        (status, code.as_deref()),
        (204, None),
        "a write that fills the bound ({LIMIT}) expected 204, got {status} {code:?}: {body}"
    );
    let expected: BTreeMap<String, String> = at_bound.iter().cloned().collect();
    let reported = tags_of(
        &emulator,
        "at the bound",
        GlobalKind::Output,
        &arn,
    )
    .await;
    assert_eq!(
        reported, expected,
        "at the bound the resource carries every written pair, got {} keys",
        reported.len()
    );

    // Re-stating a key it already carries does not grow the set, so it is accepted.
    let (status, code, body) = tag_global(&emulator, &arn, json!({ "key-0": "rewritten" })).await;
    assert_eq!(
        (status, code.as_deref()),
        (204, None),
        "overwriting a key at the bound expected 204, got {status} {code:?}: {body}"
    );

    // One key past the bound is refused, and the refusal stores neither pair it carried.
    let (status, code, body) = tag_global(
        &emulator,
        &arn,
        json!({ "one-too-many": "v", "another-too-many": "v" }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "a write past the bound ({LIMIT}) expected 400 BadRequestException, got {status} \
         {code:?}: {body}"
    );
    let mut still_expected = expected.clone();
    still_expected.insert("key-0".to_string(), "rewritten".to_string());
    let reported = tags_of(
        &emulator,
        "past the bound",
        GlobalKind::Output,
        &arn,
    )
    .await;
    assert_eq!(
        reported, still_expected,
        "a refused write stores nothing, got {} keys",
        reported.len()
    );
}

/// The tags are a member of the global resource, so a write that CHANGES them changes the
/// record and its `UpdatedAt` moves with it — and a write that leaves every pair as it was
/// changed nothing, so `UpdatedAt` stays where it is. Nothing else the create stored moves
/// either way.
#[tokio::test]
async fn a_tag_write_moves_updated_at_only_when_it_changes_the_tags() {
    let emulator = Emulator::new();

    let mut created = Vec::new();
    for (index, kind) in EVERY_KIND.into_iter().enumerate() {
        let arn =
            create_subject(&emulator, kind, &format!("stamped-{index}"), Some(json!({}))).await;
        let case = format!("{kind:?} / created");
        let resource = read_back(&emulator, &case, kind, &arn).await;
        let created_at = timestamp(&case, "createdAt", &resource);
        let updated_at = timestamp(&case, "updatedAt", &resource);
        assert_eq!(
            created_at, updated_at,
            "case {case:?}: a created resource's UpdatedAt is its CreatedAt, got {updated_at} \
             and {created_at}"
        );
        created.push((kind, arn, resource));
    }

    // Enough for the clock to have moved past the instant the creates stamped.
    tokio::time::sleep(mc::TRANSITION_SPAN * 2).await;

    let mut stamped = Vec::new();
    for (kind, arn, at_create) in &created {
        let case = format!("{kind:?} / a write that changes the tags");
        let (status, code, body) = tag_global(&emulator, arn, json!({ "env": "prod" })).await;
        assert_eq!(
            (status, code.as_deref()),
            (204, None),
            "case {case:?}: expected 204, got {status} {code:?}: {body}"
        );
        let resource = read_back(&emulator, &case, *kind, arn).await;
        let moved = timestamp(&case, "updatedAt", &resource);
        let at_create_stamp = timestamp(&case, "updatedAt", at_create);
        assert!(
            moved > at_create_stamp,
            "case {case:?}: a write that changed the tags moves UpdatedAt, got {moved} and \
             {at_create_stamp}"
        );
        assert_eq!(
            resource["name"], at_create["name"],
            "case {case:?}: a tag write leaves the Name as the create stored it, got {:?}",
            resource["name"]
        );
        assert_eq!(
            resource["createdAt"], at_create["createdAt"],
            "case {case:?}: a tag write leaves CreatedAt where it was, got {:?}",
            resource["createdAt"]
        );
        stamped.push((*kind, arn.clone(), moved));
    }

    tokio::time::sleep(mc::TRANSITION_SPAN * 2).await;

    for (kind, arn, moved) in &stamped {
        let case = format!("{kind:?} / a write that changes nothing");
        let (status, code, body) = tag_global(&emulator, arn, json!({ "env": "prod" })).await;
        assert_eq!(
            (status, code.as_deref()),
            (204, None),
            "case {case:?}: expected 204, got {status} {code:?}: {body}"
        );
        let resource = read_back(&emulator, &case, *kind, arn).await;
        let still = timestamp(&case, "updatedAt", &resource);
        assert_eq!(
            still, *moved,
            "case {case:?}: re-stating the pairs it already carries leaves UpdatedAt where it \
             was, got {still} and {moved}"
        );
    }
}
