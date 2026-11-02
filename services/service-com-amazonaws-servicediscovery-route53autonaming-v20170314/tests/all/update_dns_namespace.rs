// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdatePrivateDnsNamespace` and `UpdatePublicDnsNamespace`.
//!
//! The rule under test: a DNS namespace update writes exactly the properties its change
//! carries — `Description` and `Properties.DnsProperties.SOA.TTL`, both optional on the
//! change shapes — onto the one stored namespace record of ITS namespace type, leaves
//! every member the change omits as it was, and answers with the `OperationId` of the
//! `UPDATE_NAMESPACE` operation it files; a reference no namespace answers to is
//! `NamespaceNotFound` and a namespace of another type `InvalidInput`, and
//! neither writes anything.
//!
//! Each assertion spans BOTH DNS namespace kinds, because the two operations are the
//! same rule over the two change shapes. Requests go over the wire through
//! `MultiServiceRouter::dispatch`, with the harness `cloud_map_core` already owns.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_servicediscovery_route53autonaming_v20170314 as cloudmap;
use smithy_server_runtime::MultiServiceRouter;

use crate::cloud_map_core::{call, emulator, failure, namespace, ok};

/// A VPC id of the shape `CreatePrivateDnsNamespace$Vpc` takes (`ResourceId`, 0..64).
const VPC: &str = "vpc-0123456789abcdef0";

/// `ResourceDescription`'s published maximum, from `smithy_query(query="shape",
/// shape="PrivateDnsNamespaceChange")`: `Description` targets `ResourceDescription`,
/// `length {min: 0, max: 1024}`.
const DESCRIPTION_MAX: usize = 1024;

/// The published `RecordTTL` range, from `smithy_query(query="shape", shape="SOAChange")`:
/// `TTL` targets `RecordTTL`, `range {min: 0, max: 2147483647}`.
const TTL_MIN: i64 = 0;
const TTL_MAX: i64 = 2147483647;

/// One DNS namespace kind: the `NamespaceType` it is stored as, the create that makes
/// one, and the update this suite exercises on it.
struct Kind {
    namespace_type: &'static str,
    create: &'static str,
    update: &'static str,
    /// A DNS name suffix the created namespaces carry, so the two kinds never collide in
    /// the name index.
    suffix: &'static str,
}

/// The two DNS namespace kinds, which `NAMESPACE_TYPE_VALUES` names beside `HTTP`.
const KINDS: [Kind; 2] = [
    Kind {
        namespace_type: "DNS_PRIVATE",
        create: "CreatePrivateDnsNamespace",
        update: "UpdatePrivateDnsNamespace",
        suffix: "private.example.com",
    },
    Kind {
        namespace_type: "DNS_PUBLIC",
        create: "CreatePublicDnsNamespace",
        update: "UpdatePublicDnsNamespace",
        suffix: "public.example.com",
    },
];

/// Create a DNS namespace of `kind` and return its id.
async fn dns_namespace(
    router: &MultiServiceRouter,
    kind: &Kind,
    name: &str,
    description: Option<&str>,
    soa_ttl: Option<i64>,
) -> String {
    let mut body = json!({ "Name": format!("{name}.{}", kind.suffix) });
    if kind.create == "CreatePrivateDnsNamespace" {
        // `Vpc` is required for a private DNS namespace: it is the VPC the hosted zone
        // is associated with.
        body["Vpc"] = json!(VPC);
    }
    if let Some(description) = description {
        body["Description"] = json!(description);
    }
    if let Some(ttl) = soa_ttl {
        body["Properties"] = json!({ "DnsProperties": { "SOA": { "TTL": ttl } } });
    }
    let created = ok(router, kind.create, body).await;
    let operation_id = created["OperationId"]
        .as_str()
        .unwrap_or_else(|| panic!("{} returns an OperationId, got {created}", kind.create))
        .to_string();
    let filed = ok(router, "GetOperation", json!({ "OperationId": operation_id })).await;
    filed["Operation"]["Targets"]["NAMESPACE"]
        .as_str()
        .unwrap_or_else(|| panic!("the CREATE_NAMESPACE operation targets a namespace, got {filed}"))
        .to_string()
}

/// `GetNamespace`'s view of one namespace.
async fn read(router: &MultiServiceRouter, reference: &str) -> Value {
    ok(router, "GetNamespace", json!({ "Id": reference })).await["Namespace"].clone()
}

/// The `Description` a namespace reads back, as JSON so an absent member is
/// distinguishable from an empty string.
async fn stored_description(router: &MultiServiceRouter, id: &str) -> Value {
    read(router, id).await["Description"].clone()
}

/// The `Properties.DnsProperties.SOA.TTL` a namespace reads back, as JSON.
async fn stored_ttl(router: &MultiServiceRouter, id: &str) -> Value {
    read(router, id).await["Properties"]["DnsProperties"]["SOA"]["TTL"].clone()
}

/// A change body carrying the members a case names, omitting the rest.
fn change(description: Option<&str>, soa_ttl: Option<i64>) -> Value {
    let mut namespace_change = json!({});
    if let Some(description) = description {
        namespace_change["Description"] = json!(description);
    }
    if let Some(ttl) = soa_ttl {
        namespace_change["Properties"] = json!({ "DnsProperties": { "SOA": { "TTL": ttl } } });
    }
    namespace_change
}

/// The rule for `Description`: the description a change carries is the description the
/// namespace holds afterwards, whatever that description is. The rows differ in length,
/// script, content and form; the empty one is the normalisation every resource
/// description follows (an empty `ResourceDescription` is stored as none, so the read
/// omits the member).
#[tokio::test]
async fn every_description_a_dns_namespace_update_carries_is_what_it_reads_back() {
    let router = emulator();

    let at_the_bound = "d".repeat(DESCRIPTION_MAX);
    let cases: Vec<(&str, String)> = vec![
        ("a short description", "the new description".to_string()),
        (
            "a description replacing the previous one",
            "a second new description".to_string(),
        ),
        ("the empty description", String::new()),
        ("a single character", "x".to_string()),
        (
            "punctuation and whitespace",
            "tier: gold / zone: us-west-2, owner = team-a".to_string(),
        ),
        ("non-ASCII text", "descripción de malla — 网格 🌐".to_string()),
        ("at the published 1024 bound", at_the_bound),
    ];

    for kind in &KINDS {
        let id = dns_namespace(&router, kind, "desc", Some("the original description"), None).await;
        for (case, description) in &cases {
            let case = format!("{} {case}", kind.update);
            let updated = ok(
                &router,
                kind.update,
                json!({ "Id": id, "Namespace": change(Some(description), None) }),
            )
            .await;
            assert!(
                updated["OperationId"]
                    .as_str()
                    .is_some_and(|id| !id.is_empty()),
                "case {case:?}: the update must return an OperationId, got {updated}"
            );

            let expected = if description.is_empty() {
                Value::Null
            } else {
                json!(description)
            };
            let actual = stored_description(&router, &id).await;
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the namespace to read back the description it was \
                 given, got {actual}"
            );
        }
    }
}

/// The rule for the SOA: the TTL a change carries is the TTL the namespace's hosted-zone
/// SOA holds afterwards. The rows span the published `RecordTTL` range, including both
/// bounds, and include a namespace created with no SOA at all (which the update gives
/// one).
#[tokio::test]
async fn every_soa_ttl_a_dns_namespace_update_carries_is_what_it_reads_back() {
    let router = emulator();

    let cases: Vec<(&str, i64)> = vec![
        ("at the published low bound", TTL_MIN),
        ("one above the low bound", 1),
        ("a minute", 60),
        ("the five-minute default shape", 300),
        ("a day", 86400),
        ("at the published high bound", TTL_MAX),
    ];

    for kind in &KINDS {
        // One namespace created WITH a SOA TTL and one created without, so the rule is
        // asserted both as a replacement and as a first value.
        for (seed_case, seed) in [("created with a SOA TTL", Some(900_i64)), ("created with none", None)] {
            let id = dns_namespace(
                &router,
                kind,
                if seed.is_some() { "soa-seeded" } else { "soa-bare" },
                None,
                seed,
            )
            .await;
            for (case, ttl) in &cases {
                let case = format!("{} {seed_case}, {case}", kind.update);
                ok(
                    &router,
                    kind.update,
                    json!({ "Id": id, "Namespace": change(None, Some(*ttl)) }),
                )
                .await;
                let actual = stored_ttl(&router, &id).await;
                assert_eq!(
                    actual,
                    json!(ttl),
                    "case {case:?}: expected the namespace to read back TTL {ttl}, got {actual}"
                );
            }
        }
    }
}

/// A TTL one past either published bound is refused by the modelled `@range`, and the
/// stored namespace is left exactly as it was.
#[tokio::test]
async fn a_soa_ttl_past_a_published_bound_is_refused_and_changes_nothing() {
    let router = emulator();

    for kind in &KINDS {
        let id = dns_namespace(&router, kind, "soa-bound", Some("kept"), Some(120)).await;
        for (case, ttl) in [
            ("one below the low bound", TTL_MIN - 1),
            ("one above the high bound", TTL_MAX + 1),
        ] {
            let case = format!("{} {case}", kind.update);
            let (status, json) = call(
                &router,
                kind.update,
                json!({ "Id": id, "Namespace": change(None, Some(ttl)) }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: TTL {ttl} is outside the declared range, got {status}: {json}"
            );

            let actual = stored_ttl(&router, &id).await;
            assert_eq!(
                actual,
                json!(120),
                "case {case:?}: a refused update must leave the stored TTL alone, got {actual}"
            );
        }

        // A description past its published bound is refused the same way.
        let (status, json) = call(
            &router,
            kind.update,
            json!({
                "Id": id,
                "Namespace": change(Some(&"d".repeat(DESCRIPTION_MAX + 1)), None),
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {:?}: a description past the 1024-character bound is a client error, got \
             {status}: {json}",
            kind.update
        );
        let actual = stored_description(&router, &id).await;
        assert_eq!(
            actual,
            json!("kept"),
            "case {:?}: a refused update must leave the stored description alone, got {actual}",
            kind.update
        );
    }
}

/// Both members of a DNS namespace change are optional, so a call updates ONLY what it
/// carries: the other stays exactly as it was, and a call carrying both writes both.
#[tokio::test]
async fn a_member_the_change_omits_is_left_as_it_was() {
    let router = emulator();

    // (case, the change, the description expected afterwards, the TTL expected afterwards)
    let cases: Vec<(&str, Value, Value, Value)> = vec![
        (
            "the description alone",
            change(Some("only the description"), None),
            json!("only the description"),
            json!(42),
        ),
        (
            "the SOA alone",
            change(None, Some(99)),
            json!("only the description"),
            json!(99),
        ),
        (
            "both at once",
            change(Some("both"), Some(7)),
            json!("both"),
            json!(7),
        ),
        (
            "neither (an empty change)",
            change(None, None),
            json!("both"),
            json!(7),
        ),
    ];

    for kind in &KINDS {
        let id = dns_namespace(&router, kind, "partial", Some("seeded"), Some(42)).await;
        for (case, namespace_change, description, ttl) in &cases {
            let case = format!("{} {case}", kind.update);
            ok(
                &router,
                kind.update,
                json!({ "Id": id, "Namespace": namespace_change }),
            )
            .await;
            let read_description = stored_description(&router, &id).await;
            assert_eq!(
                &read_description, description,
                "case {case:?}: expected description {description}, got {read_description}"
            );
            let read_ttl = stored_ttl(&router, &id).await;
            assert_eq!(
                &read_ttl, ttl,
                "case {case:?}: expected TTL {ttl}, got {read_ttl}"
            );
        }
    }
}

/// The update changes nothing else about the namespace, and no other namespace.
#[tokio::test]
async fn an_update_touches_only_the_named_namespace() {
    let router = emulator();

    for kind in &KINDS {
        let id = dns_namespace(&router, kind, "isolated-a", Some("a"), Some(30)).await;
        let other = dns_namespace(&router, kind, "isolated-b", Some("b"), Some(30)).await;

        let before = read(&router, &id).await;
        ok(
            &router,
            kind.update,
            json!({ "Id": id, "Namespace": change(Some("a, updated"), Some(45)) }),
        )
        .await;
        let after = read(&router, &id).await;

        for member in ["Id", "Arn", "Name", "Type", "CreateDate", "ServiceCount"] {
            let case = format!("{} {member}", kind.update);
            assert_eq!(
                after[member], before[member],
                "case {case:?}: an update changes only the described properties, got {after}"
            );
        }
        let hosted_zone = &after["Properties"]["DnsProperties"]["HostedZoneId"];
        assert_eq!(
            hosted_zone, &before["Properties"]["DnsProperties"]["HostedZoneId"],
            "case {:?}: the hosted zone is not changed by an update, got {hosted_zone}",
            kind.update
        );

        let untouched = read(&router, &other).await;
        assert_eq!(
            (
                untouched["Description"].clone(),
                untouched["Properties"]["DnsProperties"]["SOA"]["TTL"].clone()
            ),
            (json!("b"), json!(30)),
            "case {:?}: the other namespace must be untouched, got {untouched}",
            kind.update
        );
    }
}

/// The call files an `UPDATE_NAMESPACE` operation targeting the namespace, which starts
/// non-terminal and reaches `SUCCESS` once the clock owes it.
#[tokio::test]
async fn the_call_files_an_update_namespace_operation_for_the_namespace() {
    let router = emulator();

    for kind in &KINDS {
        let id = dns_namespace(&router, kind, "op", None, Some(60)).await;
        let updated = ok(
            &router,
            kind.update,
            json!({ "Id": id, "Namespace": change(Some("filed"), None) }),
        )
        .await;
        let operation_id = updated["OperationId"]
            .as_str()
            .unwrap_or_else(|| panic!("{} returns an OperationId, got {updated}", kind.update))
            .to_string();

        let immediate = ok(
            &router,
            "GetOperation",
            json!({ "OperationId": operation_id }),
        )
        .await;
        assert_eq!(
            immediate["Operation"]["Type"],
            json!("UPDATE_NAMESPACE"),
            "case {:?}: the filed operation is an UPDATE_NAMESPACE, got {immediate}",
            kind.update
        );
        assert_eq!(
            immediate["Operation"]["Targets"]["NAMESPACE"],
            json!(id),
            "case {:?}: the filed operation targets the namespace it updated, got {immediate}",
            kind.update
        );
        assert_eq!(
            immediate["Operation"]["Status"],
            json!("SUBMITTED"),
            "case {:?}: a caller polling straight away still sees the non-terminal status, got \
             {immediate}",
            kind.update
        );

        // Blocking is fine here: the test owns the runtime, and the service never sleeps
        // for this span — it stores the due instant and materialises it on read.
        std::thread::sleep(cloudmap::state::OPERATION_SUCCESS_SPAN + Duration::from_millis(50));
        let settled = ok(
            &router,
            "GetOperation",
            json!({ "OperationId": operation_id }),
        )
        .await;
        assert_eq!(
            settled["Operation"]["Status"],
            json!("SUCCESS"),
            "case {:?}: the clock owed this operation its terminal status, got {settled}",
            kind.update
        );
    }
}

/// `Id` is "the ID or Amazon Resource Name (ARN) of the namespace", so both forms reach
/// the one stored record.
#[tokio::test]
async fn a_namespace_is_addressable_by_id_and_by_arn() {
    let router = emulator();

    for kind in &KINDS {
        let id = dns_namespace(&router, kind, "ref", None, None).await;
        let arn = read(&router, &id).await["Arn"]
            .as_str()
            .expect("a stored namespace carries its ARN")
            .to_string();

        for (case, reference) in [("by id", id.clone()), ("by ARN", arn.clone())] {
            let case = format!("{} {case}", kind.update);
            let description = format!("addressed {case}");
            ok(
                &router,
                kind.update,
                json!({ "Id": reference, "Namespace": change(Some(&description), None) }),
            )
            .await;
            let actual = stored_description(&router, &id).await;
            assert_eq!(
                actual,
                json!(description),
                "case {case:?}: both reference forms must update the one stored namespace, got \
                 {actual}"
            );
        }
    }
}

/// A reference no stored namespace answers to is the modelled `NamespaceNotFound`, and
/// nothing is written for it.
#[tokio::test]
async fn an_unknown_namespace_reference_is_namespace_not_found() {
    let router = emulator();

    for kind in &KINDS {
        let id = dns_namespace(&router, kind, "known", Some("untouched"), Some(15)).await;
        let arn = read(&router, &id).await["Arn"]
            .as_str()
            .expect("a stored namespace carries its ARN")
            .to_string();

        let references: Vec<(&str, String)> = vec![
            ("an id that was never created", "ns-000000000000000".to_string()),
            (
                "an ARN of a namespace that was never created",
                arn.replace(&id, "ns-000000000000000"),
            ),
            (
                "a namespace name rather than its id",
                format!("known.{}", kind.suffix),
            ),
            ("the empty reference", String::new()),
        ];

        for (case, reference) in references {
            let case = format!("{} {case}", kind.update);
            let (status, code) = failure(
                &router,
                kind.update,
                json!({ "Id": reference, "Namespace": change(Some("never stored"), Some(1)) }),
            )
            .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "NamespaceNotFound"),
                "case {case:?}: expected 400 NamespaceNotFound, got {status} {code}"
            );
        }

        let description = stored_description(&router, &id).await;
        let ttl = stored_ttl(&router, &id).await;
        assert_eq!(
            (description.clone(), ttl.clone()),
            (json!("untouched"), json!(15)),
            "case {:?}: a refused update must write nothing, got {description} / {ttl}",
            kind.update
        );
    }
}

/// Each update applies to ONE namespace type: a namespace of another type that does
/// exist is a bad request (400 `InvalidInput`), not a missing namespace, and nothing is
/// written for it.
#[tokio::test]
async fn a_namespace_of_another_type_is_refused_with_invalid_input() {
    let router = emulator();

    // Every namespace type this service stores, each created through its own create.
    let http = namespace(&router, "type-http-ns").await.1;
    let private = dns_namespace(&router, &KINDS[0], "type", Some("seeded"), Some(25)).await;
    let public = dns_namespace(&router, &KINDS[1], "type", Some("seeded"), Some(25)).await;
    let stored: Vec<(&str, &String)> = vec![
        ("HTTP", &http),
        (KINDS[0].namespace_type, &private),
        (KINDS[1].namespace_type, &public),
    ];

    for kind in &KINDS {
        for (namespace_type, id) in &stored {
            if *namespace_type == kind.namespace_type {
                continue;
            }
            let case = format!("{} against a {namespace_type} namespace", kind.update);
            let (status, code) = failure(
                &router,
                kind.update,
                json!({ "Id": id, "Namespace": change(Some("never stored"), Some(1)) }),
            )
            .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidInput"),
                "case {case:?}: expected 400 InvalidInput, got {status} {code}"
            );
            let actual = stored_description(&router, id).await;
            assert_ne!(
                actual,
                json!("never stored"),
                "case {case:?}: a refused update must write nothing, got {actual}"
            );
        }

        // The namespace of this update's own type beside them still updates.
        let own = if kind.namespace_type == KINDS[0].namespace_type {
            &private
        } else {
            &public
        };
        ok(
            &router,
            kind.update,
            json!({ "Id": own, "Namespace": change(Some("updated"), None) }),
        )
        .await;
        let actual = stored_description(&router, own).await;
        assert_eq!(
            actual,
            json!("updated"),
            "case {:?}: the namespace of its own type must still accept an update, got {actual}",
            kind.update
        );
    }
}

/// `UpdaterRequestId` "allows failed requests to be retried without the risk of running
/// the operation twice": the same token with the same change replays the first
/// operation, while the same token asking for a DIFFERENT change is a new request.
#[tokio::test]
async fn a_retried_updater_request_id_files_one_operation() {
    let router = emulator();

    for kind in &KINDS {
        let id = dns_namespace(&router, kind, "token", None, None).await;
        let update = |token: &'static str, namespace_change: Value| {
            let router = &router;
            let id = id.clone();
            async move {
                ok(
                    router,
                    kind.update,
                    json!({
                        "Id": id,
                        "UpdaterRequestId": token,
                        "Namespace": namespace_change,
                    }),
                )
                .await
            }
        };

        let first = update("token-a", change(Some("one"), Some(10))).await;
        let retry = update("token-a", change(Some("one"), Some(10))).await;
        assert_eq!(
            retry["OperationId"], first["OperationId"],
            "case {:?}: a retry of the same change with the same token replays the first \
             operation, got {retry}",
            kind.update
        );

        let changed = update("token-a", change(Some("two"), Some(10))).await;
        assert_ne!(
            changed["OperationId"], first["OperationId"],
            "case {:?}: the same token asking for a DIFFERENT change is a new request, got \
             {changed}",
            kind.update
        );
        let actual = stored_description(&router, &id).await;
        assert_eq!(
            actual,
            json!("two"),
            "case {:?}: the second change must be stored rather than replayed away, got {actual}",
            kind.update
        );

        let other_token = update("token-b", change(Some("three"), None)).await;
        assert_ne!(
            other_token["OperationId"], changed["OperationId"],
            "case {:?}: a different token is a different request, got {other_token}",
            kind.update
        );
    }
}
