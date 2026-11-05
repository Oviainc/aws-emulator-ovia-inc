// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UntagResource`, read back through `ListTagsForResource` and the
//! persisted store.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the
//! `MultiServiceRouter`, reusing the harness of `gateway_lifecycle`, so what is asserted
//! is the wire behaviour, and the only way a later request can see what an earlier one
//! wrote is the shared store.
//!
//! The rule under test: `UntagResource` removes from the resource its `ResourceARN` names
//! exactly the tags whose key the request lists — whatever their value — leaves every
//! other tag of that resource in place, treats a key the resource does not carry as
//! nothing to remove (so the call succeeds and is idempotent), and echoes the ARN the
//! store holds the resource under; an ARN with no record behind it is refused as the
//! resource type it names.
//!
//! The table below spans that class rather than one recorded request: rows differing in
//! how many keys are removed (none, one, several, all), whether a listed key is present,
//! absent or repeated, the character forms a modelled `TagKey` admits (letters, spaces,
//! digits and `_ . : / = + - @`), tag-key case, and the tag's value (including an empty
//! one) — and each row is run against BOTH resource types this service tags, a gateway
//! and a hypervisor.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::gateway_lifecycle::{
    build_engine, build_router, call, create, never_created_arn, persisted_client_tags, Reply,
    REGION,
};

/// An ARN of this service naming `resource_type`/`id`, in the region the harness signs
/// for.
fn arn_of(resource_type: &str, id: &str) -> String {
    format!(
        "arn:aws:backup-gateway:{REGION}:{}:{resource_type}/{id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

fn tag_map<'a>(
    pairs: impl IntoIterator<Item = &'a (&'a str, &'a str)>,
) -> BTreeMap<String, String> {
    pairs
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// Import one hypervisor and hand back the ARN the store minted for it — the second
/// resource type this service's `ResourceARN` member accepts.
async fn import_hypervisor(router: &MultiServiceRouter, name: &str, host: &str) -> String {
    let reply = call(
        router,
        "ImportHypervisorConfiguration",
        &json!({ "Name": name, "Host": host }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {name:?}: ImportHypervisorConfiguration must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply
        .str_at("/HypervisorArn")
        .unwrap_or_else(|| panic!("no HypervisorArn in {:?}", reply.body))
        .to_string()
}

/// Seed one resource of `kind` and hand back its ARN.
async fn seed_resource(router: &MultiServiceRouter, kind: &str, nonce: usize) -> String {
    match kind {
        "gateway" => create(router, &format!("gw-untag-{nonce}")).await,
        "hypervisor" => {
            import_hypervisor(
                router,
                &format!("hv-untag-{nonce}"),
                // One host, one hypervisor — so every row needs its own.
                &format!("10.0.{}.{}", nonce / 256, nonce % 256),
            )
            .await
        }
        other => panic!("unknown resource kind {other:?}"),
    }
}

/// Put `tags` on `arn` through `TagResource`, the only way a client writes tags.
async fn tag(router: &MultiServiceRouter, case: &str, arn: &str, tags: &[(&str, &str)]) {
    if tags.is_empty() {
        return;
    }
    let reply = call(
        router,
        "TagResource",
        &json!({
            "ResourceARN": arn,
            "Tags": tags
                .iter()
                .map(|(key, value)| json!({ "Key": key, "Value": value }))
                .collect::<Vec<Value>>(),
        }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: TagResource must answer 200, got {} {:?}",
        reply.status, reply.body
    );
}

async fn untag(router: &MultiServiceRouter, arn: &str, keys: &[&str]) -> Reply {
    call(
        router,
        "UntagResource",
        &json!({ "ResourceARN": arn, "TagKeys": keys }),
    )
    .await
}

async fn listed_tags(router: &MultiServiceRouter, arn: &str) -> BTreeMap<String, String> {
    let reply = call(router, "ListTagsForResource", &json!({ "ResourceArn": arn })).await;
    assert_eq!(
        reply.status, 200,
        "ListTagsForResource must answer 200 for {arn}, got {} {:?}",
        reply.status, reply.body
    );
    reply.tags()
}

/// One row of the removal table: the tags the resource starts with, the keys the request
/// lists, and the tags it must be left carrying.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    initial: &'static [(&'static str, &'static str)],
    remove: &'static [&'static str],
    expected: &'static [(&'static str, &'static str)],
}

/// The removal rows. The key forms come from the modelled `TagKey`:
/// `@length(min: 1, max: 128)` and `@pattern("^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$")`
/// (`smithy_query(query="shape",shape="TagKey")`), so letters of any script, separators
/// (spaces) and digits are all legal key characters beside `_ . : / = + - @`.
const ROWS: &[Row] = &[
    Row {
        case: "one key of several",
        initial: &[("env", "test"), ("owner", "qa"), ("team", "core")],
        remove: &["owner"],
        expected: &[("env", "test"), ("team", "core")],
    },
    Row {
        case: "several keys at once",
        initial: &[("env", "test"), ("owner", "qa"), ("team", "core")],
        remove: &["env", "team"],
        expected: &[("owner", "qa")],
    },
    Row {
        case: "every key the resource carries",
        initial: &[("env", "test"), ("owner", "qa"), ("team", "core")],
        remove: &["env", "owner", "team"],
        expected: &[],
    },
    Row {
        case: "no keys at all",
        initial: &[("env", "test"), ("owner", "qa")],
        remove: &[],
        expected: &[("env", "test"), ("owner", "qa")],
    },
    Row {
        case: "a key the resource never carried",
        initial: &[("env", "test")],
        remove: &["owner"],
        expected: &[("env", "test")],
    },
    Row {
        case: "a key the resource never carried, on an untagged resource",
        initial: &[],
        remove: &["env"],
        expected: &[],
    },
    Row {
        case: "a present key beside an absent one",
        initial: &[("env", "test"), ("team", "core")],
        remove: &["env", "never-set"],
        expected: &[("team", "core")],
    },
    Row {
        case: "the same key listed twice in one request",
        initial: &[("env", "test"), ("team", "core")],
        remove: &["env", "env"],
        expected: &[("team", "core")],
    },
    Row {
        case: "keys carrying separators and non-latin letters",
        initial: &[
            ("cost center", "ops"),
            ("Ключ", "значение"),
            ("path/to:thing=1+2-3@x", "kept"),
        ],
        remove: &["cost center", "Ключ"],
        expected: &[("path/to:thing=1+2-3@x", "kept")],
    },
    Row {
        case: "tag keys are case sensitive",
        initial: &[("Env", "upper"), ("env", "lower")],
        remove: &["env"],
        expected: &[("Env", "upper")],
    },
    Row {
        // The non-empty value carries no space: the modelled `Tag` doc states "Spaces are
        // not allowed in tag values", so a value with one is never stored to remove a key
        // beside. A KEY may carry spaces, which the row above covers.
        case: "a key is removed whatever its value, including an empty one",
        initial: &[("empty", ""), ("full", "a-non-empty_value:1")],
        remove: &["empty"],
        expected: &[("full", "a-non-empty_value:1")],
    },
    Row {
        case: "a 128-character key, the modelled TagKey maximum",
        initial: &[(
            "kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk\
             kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk",
            "max",
        )],
        remove: &[
            "kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk\
             kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk",
        ],
        expected: &[],
    },
    Row {
        case: "a one-character key, the modelled TagKey minimum",
        initial: &[("k", "min"), ("K", "other")],
        remove: &["k"],
        expected: &[("K", "other")],
    },
];

/// The rule: a removal drops exactly the listed keys and nothing else, succeeds for a key
/// the resource does not carry, and echoes the ARN the store holds — on either resource
/// type this service tags.
#[tokio::test]
async fn untag_resource_removes_exactly_the_listed_keys() {
    let engine = build_engine();
    let router = build_router(&engine);

    for (kind_index, kind) in ["gateway", "hypervisor"].into_iter().enumerate() {
        for (index, row) in ROWS.iter().enumerate() {
            let case = format!("{kind} / {}", row.case);
            let arn = seed_resource(&router, kind, kind_index * ROWS.len() + index + 1).await;
            tag(&router, &case, &arn, row.initial).await;

            let reply = untag(&router, &arn, row.remove).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: UntagResource must answer 200, got {} {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.str_at("/ResourceARN"),
                Some(arn.as_str()),
                "case {case:?}: UntagResource must echo the stored ARN, got {:?}",
                reply.body
            );

            let expected = tag_map(row.expected);
            let listed = listed_tags(&router, &arn).await;
            assert_eq!(
                listed, expected,
                "case {case:?}: after removing {:?} the resource must carry {expected:?}, \
                 ListTagsForResource reported {listed:?}",
                row.remove
            );
            let stored = persisted_client_tags(&engine, &arn);
            assert_eq!(
                stored, expected,
                "case {case:?}: after removing {:?} the STORE must hold {expected:?}, it \
                 holds {stored:?}",
                row.remove
            );
        }
    }
}

/// Removing is idempotent: repeating the same request neither fails nor changes the tag
/// set again, and a removed key only comes back if it is tagged again.
#[tokio::test]
async fn untag_resource_is_idempotent_and_a_removed_key_can_be_set_again() {
    let engine = build_engine();
    let router = build_router(&engine);

    for kind in ["gateway", "hypervisor"] {
        let arn = seed_resource(&router, kind, 9000 + usize::from(kind == "hypervisor")).await;
        tag(&router, kind, &arn, &[("env", "test"), ("team", "core")]).await;

        for attempt in 1..=3 {
            let reply = untag(&router, &arn, &["env"]).await;
            assert_eq!(
                reply.status, 200,
                "case {kind} attempt {attempt}: a repeated UntagResource must answer 200, \
                 got {} {:?}",
                reply.status, reply.body
            );
            let listed = listed_tags(&router, &arn).await;
            assert_eq!(
                listed,
                tag_map(&[("team", "core")]),
                "case {kind} attempt {attempt}: the tag set must be unchanged by a repeat, \
                 got {listed:?}"
            );
        }

        tag(&router, kind, &arn, &[("env", "again")]).await;
        let listed = listed_tags(&router, &arn).await;
        assert_eq!(
            listed,
            tag_map(&[("env", "again"), ("team", "core")]),
            "case {kind}: a removed key must be settable again, got {listed:?}"
        );
    }
}

/// A `ResourceARN` with no record behind it is refused as the resource type it names —
/// the same way `TagResource` and `ListTagsForResource` refuse it — and nothing is
/// written.
#[tokio::test]
async fn untag_resource_refuses_an_arn_with_no_record() {
    let engine = build_engine();
    let router = build_router(&engine);
    // A real, tagged gateway exists, so a refusal below means "this ARN", not "the store
    // is empty".
    let live = create(&router, "gw-live").await;
    tag(&router, "live", &live, &[("env", "test")]).await;

    // (case, the ARN, expected status, expected ErrorCode member) — every resource type
    // this service names is answered 400 with the `ErrorCode` naming that type; an ARN of
    // a type this service does not name takes the 404 the shape's `@httpError` declares
    // and no `ErrorCode`.
    let rows: [(&str, String, u16, Option<&str>); 4] = [
        (
            "a gateway that was never created",
            never_created_arn(),
            400,
            Some("GatewayNotFound"),
        ),
        (
            "a hypervisor that was never imported",
            arn_of("hypervisor", "hv-1111111111111111"),
            400,
            Some("HypervisorNotFound"),
        ),
        (
            "a virtual machine",
            arn_of("vm", "vm-2222222222222222"),
            400,
            Some("VirtualMachineNotFound"),
        ),
        (
            "a resource type this service does not name",
            arn_of("appliance", "ap-3333333333333333"),
            404,
            None,
        ),
    ];

    for (case, arn, status, error_code) in &rows {
        let refused = untag(&router, arn, &["env"]).await;
        assert_eq!(
            (
                refused.status,
                refused.code.as_deref(),
                refused.str_at("/ErrorCode")
            ),
            (*status, Some("ResourceNotFoundException"), *error_code),
            "case {case:?}: UntagResource on an ARN with no record must answer {status} \
             ResourceNotFoundException (ErrorCode {error_code:?}), got {} {:?}",
            refused.status,
            refused.body
        );
    }

    let listed = listed_tags(&router, &live).await;
    assert_eq!(
        listed,
        tag_map(&[("env", "test")]),
        "the live gateway's tags must be untouched by the refusals, got {listed:?}"
    );
}
