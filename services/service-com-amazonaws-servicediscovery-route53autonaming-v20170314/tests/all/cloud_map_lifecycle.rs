// SPDX-License-Identifier: CC0-1.0

//! The Cloud Map control-plane rules this slice adds, over the wire: where a
//! `DnsConfig` may be attached, what a namespace `Name` must satisfy, how a name
//! already owned by a completed create is refused, how an empty `Description` is
//! normalised, which messages are fixed text, the in-use preconditions on the two
//! deletes, who may carry a custom health status, and how `ListNamespaces` validates
//! its filters and pages.
//!
//! Every request goes through `MultiServiceRouter::dispatch`, i.e. the same path the
//! emulator serves in production (`awsJson1_1`: `POST /` with `X-Amz-Target`), and the
//! harness is the one `cloud_map_core` already builds.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::cloud_map_core::{call, emulator, failure, namespace, ok, service};

/// The `@pattern` the model declares on `CreateHttpNamespace$Name`
/// (`smithy_query(query="operation",operation="CreateHttpNamespace")`).
const HTTP_NAME_PATTERN: &str = "^(?!arn:)[!-~]{1,1024}$";
/// The repetition bound inside [`HTTP_NAME_PATTERN`], which is also the member's
/// `@length` maximum.
const HTTP_NAME_MAX: usize = 1024;
/// The `@pattern` the model declares on `CreatePrivateDnsNamespace$Name`
/// (`smithy_query(query="operation",operation="CreatePrivateDnsNamespace")`).
const PRIVATE_NAME_PATTERN: &str = "^(?!arn:)[!-~]{1,253}$";
/// The repetition bound inside [`PRIVATE_NAME_PATTERN`].
const PRIVATE_NAME_MAX: usize = 253;

/// A VPC id of the shape `CreatePrivateDnsNamespace$Vpc` takes (`ResourceId`, 0..64).
const VPC: &str = "vpc-0123456789abcdef0";

/// The wire error code and message of a failure body.
///
/// The code is the shape name `__type` carries, and the message is read under either
/// casing: a handler-returned error renders the modelled `Message` member, while a
/// refusal the framework makes before the handler runs renders a lowercase `message`.
fn error_of(answer: &Value) -> (String, String) {
    let code = answer["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    let message = answer
        .get("Message")
        .or_else(|| answer.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    (code, message)
}

/// Run a namespace create to completion and return `(operation id, namespace id)`.
///
/// Both namespace creates are asynchronous and report only an `OperationId`, so the
/// namespace id is read where Cloud Map puts it: that operation's `Targets` map.
async fn created_namespace(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (String, String) {
    let created = ok(router, operation, body.clone()).await;
    let operation_id = created["OperationId"]
        .as_str()
        .unwrap_or_else(|| panic!("{operation} with {body} returns an OperationId, got {created}"))
        .to_string();
    let filed = ok(router, "GetOperation", json!({ "OperationId": operation_id })).await;
    let namespace_id = filed["Operation"]["Targets"]["NAMESPACE"]
        .as_str()
        .unwrap_or_else(|| panic!("the CREATE_NAMESPACE operation targets a namespace, got {filed}"))
        .to_string();
    (operation_id, namespace_id)
}

/// A private DNS namespace, which is the one namespace kind that holds a hosted zone.
async fn private_namespace(router: &MultiServiceRouter, name: &str) -> String {
    created_namespace(
        router,
        "CreatePrivateDnsNamespace",
        json!({ "Name": name, "Vpc": VPC }),
    )
    .await
    .1
}

/// A `DnsConfig` asks Cloud Map to create Route 53 records in the parent namespace's
/// hosted zone, so it is accepted exactly where there is a hosted zone to hold them.
/// An HTTP namespace has none, so the call is refused with 400 `InvalidInput` and no
/// service is created; a DNS namespace stores the records and reports `Type=DNS_HTTP`.
#[tokio::test]
async fn a_dns_config_is_refused_against_a_namespace_that_holds_no_hosted_zone() {
    let router = emulator();
    let (_, http_id) = namespace(&router, "dnscfg-http-ns").await;
    let dns_id = private_namespace(&router, "dnscfg-dns-ns.example.com").await;

    // `Type` values are the model's `RecordType` enum (A, AAAA, CNAME, SRV) and
    // `RoutingPolicy` its `RoutingPolicy` enum (MULTIVALUE, WEIGHTED), so the rows
    // differ in record type, record count and routing policy — not just in value.
    let configs: Vec<(&str, Value)> = vec![
        (
            "one A record",
            json!({ "DnsRecords": [{ "Type": "A", "TTL": 60 }] }),
        ),
        (
            "an AAAA record with a zero TTL",
            json!({ "DnsRecords": [{ "Type": "AAAA", "TTL": 0 }] }),
        ),
        (
            "a CNAME under a weighted routing policy",
            json!({ "RoutingPolicy": "WEIGHTED",
                    "DnsRecords": [{ "Type": "CNAME", "TTL": 300 }] }),
        ),
        (
            "two records at once",
            json!({ "DnsRecords": [{ "Type": "A", "TTL": 10 },
                                   { "Type": "SRV", "TTL": 10 }] }),
        ),
    ];

    let mut index = 0;
    for (config_case, dns_config) in &configs {
        // The namespace may be named directly or through the (deprecated)
        // `DnsConfig.NamespaceId`; the rule holds either way.
        for named_by in ["NamespaceId", "DnsConfig.NamespaceId"] {
            for (namespace_case, namespace_id, holds_hosted_zone) in [
                ("an HTTP namespace", &http_id, false),
                ("a private DNS namespace", &dns_id, true),
            ] {
                index += 1;
                let mut config = dns_config.clone();
                let mut body = json!({ "Name": format!("dnscfg-svc-{index}") });
                if named_by == "NamespaceId" {
                    body["NamespaceId"] = json!(namespace_id);
                } else {
                    config["NamespaceId"] = json!(namespace_id);
                }
                body["DnsConfig"] = config;
                let case = format!("{config_case} in {namespace_case} named by {named_by}");

                let (status, answer) = call(&router, "CreateService", body.clone()).await;
                if holds_hosted_zone {
                    assert_eq!(
                        (status, &answer["Service"]["Type"]),
                        (200, &json!("DNS_HTTP")),
                        "case {case:?} from {body}: a namespace with a hosted zone registers the \
                         records, got {answer}"
                    );
                    assert_eq!(
                        answer["Service"]["DnsConfig"]["DnsRecords"],
                        body["DnsConfig"]["DnsRecords"],
                        "case {case:?}: the stored DnsConfig is reported back, got {answer}"
                    );
                } else {
                    let (code, message) = error_of(&answer);
                    assert_eq!(
                        (status, code.as_str()),
                        (400, "InvalidInput"),
                        "case {case:?} from {body}: expected 400 InvalidInput, got {answer}"
                    );
                    assert!(
                        message.contains("DnsConfig"),
                        "case {case:?}: the refusal names the member it refused, got {answer}"
                    );
                }
            }
        }
    }

    // Nothing was created by any refused call: the HTTP namespace holds no service.
    let read = ok(&router, "GetNamespace", json!({ "Id": http_id })).await;
    assert_eq!(
        read["Namespace"]["ServiceCount"],
        json!(0),
        "a refused CreateService must store nothing, got {read}"
    );
}

/// A namespace `Name` must satisfy the `@pattern` its member declares: no `arn:`
/// prefix, between one and the member's maximum characters, and every character
/// printable ASCII. A violation is refused with 400 `InvalidInput` naming the member
/// and the constraint, and nothing is created.
///
/// Only the two members whose pattern carries the `(?!arn:)` lookahead are rows here:
/// `NamespaceNamePublic`'s pattern is a plain label pattern, which the boundary
/// validator compiles and applies itself (`public_dns_namespace.rs` pins it).
#[tokio::test]
async fn a_namespace_name_must_satisfy_the_pattern_the_model_declares() {
    let router = emulator();

    // (operation, the member's pattern, its maximum length)
    let operations: Vec<(&str, &str, usize)> = vec![
        ("CreateHttpNamespace", HTTP_NAME_PATTERN, HTTP_NAME_MAX),
        (
            "CreatePrivateDnsNamespace",
            PRIVATE_NAME_PATTERN,
            PRIVATE_NAME_MAX,
        ),
    ];

    for (tag, (operation, pattern, max_len)) in operations.into_iter().enumerate() {
        // Accepted names must differ between the two operations, because a name a
        // completed create already owns is refused as a duplicate rather than
        // re-created — so each row carries the operation's index.
        let unique = char::from(b'a' + u8::try_from(tag).unwrap_or(0));
        // (case, name, accepted) — the published bound and one past it are both rows.
        let cases: Vec<(&str, String, bool)> = vec![
            ("a dotted DNS name", format!("example-{unique}.internal"), true),
            ("one character", unique.to_string(), true),
            (
                "underscores and dashes, which the pattern allows",
                format!("_my-ns.sub_domain-{unique}"),
                true,
            ),
            ("at the length bound", "a".repeat(max_len), true),
            ("one past the length bound", "a".repeat(max_len + 1), false),
            ("empty, which the pattern's {1,N} refuses", String::new(), false),
            (
                "a space, which is below the printable range",
                "not a valid dns name!!".to_string(),
                false,
            ),
            ("a tab", "ns\tname".to_string(), false),
            (
                "a non-ASCII character",
                "n\u{e4}mespace.example".to_string(),
                false,
            ),
            (
                "an ARN, which the lookahead refuses",
                "arn:aws:servicediscovery:us-west-2:123456789012:namespace/ns-1".to_string(),
                false,
            ),
        ];

        for (case, name, accepted) in cases {
            let mut body = json!({ "Name": name });
            if operation == "CreatePrivateDnsNamespace" {
                body["Vpc"] = json!(VPC);
            }
            let (status, answer) = call(&router, operation, body).await;
            let shown = format!("{name:.40}");
            if accepted {
                assert_eq!(
                    status, 200,
                    "case {case:?} ({shown}, {} chars) on {operation}: a name satisfying the \
                     pattern is accepted, got {answer}",
                    name.chars().count()
                );
                assert!(
                    answer["OperationId"].is_string(),
                    "case {case:?} on {operation}: an accepted create files an operation, got \
                     {answer}"
                );
                continue;
            }
            let (code, message) = error_of(&answer);
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidInput"),
                "case {case:?} ({shown}, {} chars) on {operation}: expected 400 InvalidInput, got \
                 {answer}",
                name.chars().count()
            );
            assert!(
                message.contains("at 'name'") && message.contains("failed to satisfy constraint"),
                "case {case:?} on {operation}: the refusal names the member and its constraint, \
                 got {answer}"
            );
            // A name inside the length bound is refused by the PATTERN, so the clause
            // states the pattern the model declares.
            if name.chars().count() <= max_len {
                assert!(
                    message.contains(pattern),
                    "case {case:?} on {operation}: the clause states the pattern {pattern}, got \
                     {answer}"
                );
            }
        }
    }

    // Every refused create stored nothing, so the account holds only the accepted ones.
    let listed = ok(&router, "ListNamespaces", json!({})).await;
    let names: Vec<&str> = listed["Namespaces"]
        .as_array()
        .expect("Namespaces is a list")
        .iter()
        .map(|summary| summary["Name"].as_str().unwrap_or_default())
        .collect();
    assert!(
        names.iter().all(|name| !name.contains(' ')),
        "no refused name may have been stored, got {names:?}"
    );
}

/// This model declares exactly one error for a bad request — `InvalidInput`, "one or
/// more specified values aren't valid", which every operation lists — and no
/// `ValidationException` shape at all, so EVERY refusal of a bad value carries that
/// code: the ones a handler makes and the ones the centralized walk makes before a
/// handler runs, whichever constraint class they come from.
#[tokio::test]
async fn every_refusal_of_a_bad_value_carries_this_service_s_own_code() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "refusal-ns").await;

    // One row per constraint class the walk and the handlers between them refuse.
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "a required member the request omits",
            "CreateService",
            json!({ "NamespaceId": namespace_id }),
        ),
        (
            // `GetNamespaceRequest$Id` targets `Arn`, length 0..255.
            "a value past a declared length maximum",
            "GetNamespace",
            json!({ "Id": "n".repeat(256) }),
        ),
        (
            // `ListNamespacesRequest$MaxResults` has range 1..100.
            "a value below a declared range minimum",
            "ListNamespaces",
            json!({ "MaxResults": 0 }),
        ),
        (
            "a value above a declared range maximum",
            "ListNamespaces",
            json!({ "MaxResults": 101 }),
        ),
        (
            // `CustomHealthStatus` is an enum of HEALTHY and UNHEALTHY.
            "a value outside a declared enum",
            "UpdateInstanceCustomHealthStatus",
            json!({ "ServiceId": "srv-any", "InstanceId": "i-1", "Status": "DEGRADED" }),
        ),
        (
            // Refused by the handler rather than the walk: the same code.
            "a combination of members that cannot hold together",
            "CreateService",
            json!({ "Name": "two-checks-svc", "NamespaceId": namespace_id,
                    "HealthCheckConfig": { "Type": "HTTP" },
                    "HealthCheckCustomConfig": { "FailureThreshold": 1 } }),
        ),
    ];

    for (case, operation, body) in cases {
        let (status, answer) = call(&router, operation, body.clone()).await;
        let (code, message) = error_of(&answer);
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidInput"),
            "case {case:?} on {operation} with {body}: expected 400 InvalidInput, got {answer}"
        );
        assert!(
            !message.is_empty(),
            "case {case:?} on {operation}: a refusal says what was wrong, got {answer}"
        );
    }
}

/// A `Name` already owned by a create that has COMPLETED is a duplicate request, not a
/// bad value: it is refused with `DuplicateRequest` naming that operation. Deleting
/// the namespace frees the name and the identical call then succeeds.
#[tokio::test]
async fn a_name_already_owned_by_a_completed_create_is_a_duplicate_request() {
    let router = emulator();

    // (operation, the name it creates, the extra members that operation requires)
    let cases: Vec<(&str, &str, Value)> = vec![
        ("CreateHttpNamespace", "dup-http-ns", json!({})),
        (
            "CreatePrivateDnsNamespace",
            "dup-private-ns.example",
            json!({ "Vpc": VPC }),
        ),
        // The third namespace create follows the same rule; its `Name` is a dotted DNS
        // name because `NamespaceNamePublic`'s pattern is a label pattern.
        (
            "CreatePublicDnsNamespace",
            "dup-public-ns.example.com",
            json!({}),
        ),
    ];

    for (operation, name, extra) in cases {
        let mut body = json!({ "Name": name });
        if let Value::Object(members) = extra {
            for (key, value) in members {
                body[key.as_str()] = value;
            }
        }

        let (first_operation, namespace_id) =
            created_namespace(&router, operation, body.clone()).await;

        let (status, answer) = call(&router, operation, body.clone()).await;
        let (code, _) = error_of(&answer);
        assert_eq!(
            (status, code.as_str()),
            (400, "DuplicateRequest"),
            "case {operation} on the taken name {name:?}: expected 400 DuplicateRequest, got \
             {answer}"
        );
        assert_eq!(
            answer["DuplicateOperationId"],
            json!(first_operation),
            "case {operation}: the refusal names the create that owns the name, got {answer}"
        );

        // A second namespace was NOT created: the name still resolves to the first.
        let read = ok(&router, "GetNamespace", json!({ "Id": namespace_id })).await;
        assert_eq!(
            read["Namespace"]["Name"],
            json!(name),
            "case {operation}: the stored namespace is untouched, got {read}"
        );

        // Deleting it frees the name, so the identical call succeeds.
        ok(&router, "DeleteNamespace", json!({ "Id": namespace_id })).await;
        let (_, recreated) = created_namespace(&router, operation, body.clone()).await;
        assert_ne!(
            recreated, namespace_id,
            "case {operation}: the freed name is created afresh, not replayed"
        );
    }
}

/// `Description` has no default in the model, so a resource created with
/// `Description=""` is indistinguishable on a read from one created without the member:
/// the read omits it. Any non-empty text round-trips unchanged.
#[tokio::test]
async fn an_empty_description_is_omitted_and_a_non_empty_one_round_trips() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "desc-ns").await;

    // (case, what the create sends, what the read must report)
    let descriptions: Vec<(&str, Option<String>, Option<String>)> = vec![
        ("no Description at all", None, None),
        ("an empty string", Some(String::new()), None),
        (
            "a single space, which is not empty",
            Some(" ".to_string()),
            Some(" ".to_string()),
        ),
        ("one character", Some("x".to_string()), Some("x".to_string())),
        (
            "a sentence",
            Some("the checkout service".to_string()),
            Some("the checkout service".to_string()),
        ),
        // `ResourceDescription` is length 0..1024, so the maximum is a row too.
        (
            "at the length bound",
            Some("d".repeat(1024)),
            Some("d".repeat(1024)),
        ),
    ];

    for (index, (case, sent, expected)) in descriptions.into_iter().enumerate() {
        // Every creator that accepts a `Description`: both namespace creates and the
        // service create, each read back through its own Get.
        let targets: Vec<(&str, Value, &str, &str)> = vec![
            (
                "CreateHttpNamespace",
                json!({ "Name": format!("desc-http-{index}") }),
                "GetNamespace",
                "Namespace",
            ),
            (
                "CreatePrivateDnsNamespace",
                json!({ "Name": format!("desc-private-{index}.example"), "Vpc": VPC }),
                "GetNamespace",
                "Namespace",
            ),
            (
                "CreateService",
                json!({ "Name": format!("desc-svc-{index}"), "NamespaceId": namespace_id }),
                "GetService",
                "Service",
            ),
        ];

        for (operation, mut body, reader, member) in targets {
            if let Some(description) = &sent {
                body["Description"] = json!(description);
            }
            let id = if operation == "CreateService" {
                ok(&router, operation, body.clone()).await["Service"]["Id"]
                    .as_str()
                    .expect("CreateService returns the service it stored")
                    .to_string()
            } else {
                created_namespace(&router, operation, body.clone()).await.1
            };

            let read = ok(&router, reader, json!({ "Id": id })).await;
            let reported = read[member].get("Description").cloned();
            match &expected {
                None => assert!(
                    reported.is_none(),
                    "case {case:?} on {operation}: the read must omit Description, got {read}"
                ),
                Some(text) => assert_eq!(
                    reported,
                    Some(json!(text)),
                    "case {case:?} on {operation}: the read must report what was stored, got \
                     {read}"
                ),
            }
        }
    }
}

/// Cloud Map's not-found and conflict messages carry no prose: `NamespaceNotFound`
/// carries no `Message` on a namespace READ, while a call that NAMES a namespace to act
/// on — an update, a delete, or a create in it — reports the namespace reference the
/// request named and nothing else, and `ServiceAlreadyExists` carries one constant string
/// naming neither resource.
#[tokio::test]
async fn a_not_found_or_conflict_body_echoes_no_request_value() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "msg-ns").await;

    // Every operation that can answer `NamespaceNotFound`, an id or name that reaches it
    // — the ids differ in shape so no row can pass by matching one of them — and the
    // `Message` that operation answers with (`None` for no `Message` member at all).
    let not_found: Vec<(&str, Value, Option<&str>)> = vec![
        ("GetNamespace", json!({ "Id": "ns-sim2realbeha" }), None),
        ("GetNamespace", json!({ "Id": "ns-0123456789abcdef" }), None),
        (
            "DeleteNamespace",
            json!({ "Id": "ns-nosuchnamespace" }),
            Some("ns-nosuchnamespace"),
        ),
        (
            "DeleteNamespace",
            json!({ "Id": "arn:aws:servicediscovery:us-west-2:000000000000:namespace/ns-gone" }),
            Some("arn:aws:servicediscovery:us-west-2:000000000000:namespace/ns-gone"),
        ),
        (
            "UpdateHttpNamespace",
            json!({ "Id": "ns-nosuchnamespace", "Namespace": { "Description": "x" } }),
            Some("ns-nosuchnamespace"),
        ),
        (
            "CreateService",
            json!({ "Name": "orphan-svc", "NamespaceId": "ns-nosuchnamespace" }),
            Some("ns-nosuchnamespace"),
        ),
        (
            "DiscoverInstances",
            json!({ "NamespaceName": "no-such-namespace", "ServiceName": "any" }),
            None,
        ),
    ];
    for (operation, body, expected_message) in not_found {
        let (status, answer) = call(&router, operation, body.clone()).await;
        let (code, message) = error_of(&answer);
        assert_eq!(
            (status, code.as_str()),
            (400, "NamespaceNotFound"),
            "case {operation} with {body}: expected 400 NamespaceNotFound, got {answer}"
        );
        match expected_message {
            None => assert!(
                answer.get("Message").is_none() && answer.get("message").is_none(),
                "case {operation} with {body}: NamespaceNotFound carries no Message, got {answer}"
            ),
            Some(expected) => assert_eq!(
                message, expected,
                "case {operation} with {body}: expected the Message {expected:?}, got {answer}"
            ),
        }
    }

    // The conflict message is one constant, whatever the service and namespace are
    // called — including a second namespace, so the text cannot be a stored echo.
    let other_namespace = private_namespace(&router, "msg-other-ns.example").await;
    for (case, parent, name) in [
        ("a short name", &namespace_id, "a"),
        ("a dotted name", &namespace_id, "svc.with.dots"),
        ("an underscored SRV-style name", &namespace_id, "_svc._tcp"),
        ("the same name in another namespace", &other_namespace, "a"),
    ] {
        let body = json!({ "Name": name, "NamespaceId": parent });
        ok(&router, "CreateService", body.clone()).await;
        let (status, answer) = call(&router, "CreateService", body.clone()).await;
        let (code, message) = error_of(&answer);
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (400, "ServiceAlreadyExists", "Service already exists."),
            "case {case:?} with {body}: the conflict is the constant message, got {answer}"
        );
    }
}

/// A resource that still contains other resources cannot be deleted: a namespace that
/// holds a service, and a service that holds an instance, are refused with
/// `ResourceInUse`. The delete succeeds once the children are gone.
#[tokio::test]
async fn a_resource_that_still_has_children_is_refused_with_resource_in_use() {
    let router = emulator();

    // The child count varies across rows: the refusal is about having children at all,
    // not about having exactly one.
    for (case, services, instances) in [
        ("one service holding one instance", 1_usize, 1_usize),
        ("one service holding three instances", 1, 3),
        ("three services, none holding an instance", 3, 0),
    ] {
        let namespace_name = format!("inuse-ns-{services}-{instances}");
        let (_, namespace_id) = namespace(&router, &namespace_name).await;
        let mut service_ids = Vec::new();
        for index in 0..services {
            let service_id =
                service(&router, &namespace_id, &format!("inuse-svc-{index}")).await;
            for instance in 0..instances {
                ok(
                    &router,
                    "RegisterInstance",
                    json!({ "ServiceId": service_id, "InstanceId": format!("i-{instance}"),
                            "Attributes": { "AWS_INSTANCE_IPV4": "10.0.0.1" } }),
                )
                .await;
            }
            service_ids.push(service_id);
        }

        // The namespace is in use while any service names it.
        let (status, code) = failure(
            &router,
            "DeleteNamespace",
            json!({ "Id": namespace_id }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ResourceInUse"),
            "case {case:?}: a namespace holding {services} service(s) is in use"
        );

        for service_id in &service_ids {
            if instances > 0 {
                let (status, code) =
                    failure(&router, "DeleteService", json!({ "Id": service_id })).await;
                assert_eq!(
                    (status, code.as_str()),
                    (400, "ResourceInUse"),
                    "case {case:?}: a service holding {instances} instance(s) is in use"
                );
                for instance in 0..instances {
                    ok(
                        &router,
                        "DeregisterInstance",
                        json!({ "ServiceId": service_id, "InstanceId": format!("i-{instance}") }),
                    )
                    .await;
                }
            }
            ok(&router, "DeleteService", json!({ "Id": service_id })).await;
            let (status, code) = failure(&router, "GetService", json!({ "Id": service_id })).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ServiceNotFound"),
                "case {case:?}: a deleted service is gone from the store"
            );
        }

        // Drained, the namespace deletes and files the operation that records it.
        let deleted = ok(&router, "DeleteNamespace", json!({ "Id": namespace_id })).await;
        let operation_id = deleted["OperationId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: DeleteNamespace files an operation"))
            .to_string();
        let filed = ok(
            &router,
            "GetOperation",
            json!({ "OperationId": operation_id }),
        )
        .await;
        assert_eq!(
            (
                &filed["Operation"]["Type"],
                &filed["Operation"]["Targets"]["NAMESPACE"]
            ),
            (&json!("DELETE_NAMESPACE"), &json!(namespace_id)),
            "case {case:?}: the filed operation records the delete, got {filed}"
        );
        let (status, code) = failure(&router, "GetNamespace", json!({ "Id": namespace_id })).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NamespaceNotFound"),
            "case {case:?}: a deleted namespace is gone from the store"
        );
    }
}

/// `UpdateInstanceCustomHealthStatus` changes the status "only for custom health
/// checks, which you define using `HealthCheckCustomConfig`", and the refusal says which
/// registry is missing the service: one that configures NO health checking is in no
/// health registry and answers `ServiceNotFound`, while one checked by Route 53 is
/// health-checked but not by a check a client may set and answers
/// `CustomHealthNotFound`. A service that does configure a custom health check accepts
/// the call, and both readers answer from the one record it writes (the wait before the
/// submitted status is reported is pinned by `custom_health_status.rs`).
#[tokio::test]
async fn a_custom_health_status_is_accepted_only_where_a_custom_health_check_is_configured()
{
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "health-ns").await;

    // (case, the health configuration the service was created with, the code a submit
    // is refused with — `None` when it is accepted)
    let configurations: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "no health configuration at all",
            json!({}),
            Some("ServiceNotFound"),
        ),
        (
            "a Route 53 health check, which this call may not change",
            json!({ "HealthCheckConfig": { "Type": "HTTP", "ResourcePath": "/ping" } }),
            Some("CustomHealthNotFound"),
        ),
        (
            "a custom health check",
            json!({ "HealthCheckCustomConfig": { "FailureThreshold": 1 } }),
            None,
        ),
        (
            "a custom health check with no failure threshold",
            json!({ "HealthCheckCustomConfig": {} }),
            None,
        ),
    ];

    for (index, (case, health, refused_with)) in configurations.into_iter().enumerate() {
        let accepted = refused_with.is_none();
        let service_name = format!("health-svc-{index}");
        let mut body = json!({ "Name": service_name, "NamespaceId": namespace_id });
        if let Value::Object(members) = health {
            for (key, value) in members {
                body[key.as_str()] = value;
            }
        }
        let service_id = ok(&router, "CreateService", body.clone()).await["Service"]["Id"]
            .as_str()
            .expect("CreateService returns the service it stored")
            .to_string();
        ok(
            &router,
            "RegisterInstance",
            json!({ "ServiceId": service_id, "InstanceId": "i-1",
                    "Attributes": { "AWS_INSTANCE_IPV4": "10.0.0.1" } }),
        )
        .await;

        // The initial status the register call stored, read through the operation that
        // exists for it: `HEALTHY` where a custom health check is configured (the
        // documented default for `AWS_INIT_HEALTH_STATUS`), `UNKNOWN` where nothing has
        // reported on the instance.
        let initial = ok(
            &router,
            "GetInstancesHealthStatus",
            json!({ "ServiceId": service_id }),
        )
        .await;
        assert_eq!(
            initial["Status"]["i-1"],
            json!(if accepted { "HEALTHY" } else { "UNKNOWN" }),
            "case {case:?}: the registered instance's initial status, got {initial}"
        );
        let (status, code) = failure(
            &router,
            "GetInstancesHealthStatus",
            json!({ "ServiceId": service_id, "Instances": ["i-never"] }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InstanceNotFound"),
            "case {case:?}: a named instance the service never registered is not found"
        );

        // Both `CustomHealthStatus` values, so the accepted arm is exercised in each
        // direction rather than only into one state.
        for status_value in ["UNHEALTHY", "HEALTHY"] {
            let request = json!({ "ServiceId": service_id, "InstanceId": "i-1",
                                  "Status": status_value });
            let (status, answer) =
                call(&router, "UpdateInstanceCustomHealthStatus", request.clone()).await;
            if let Some(expected_code) = refused_with {
                let (code, _) = error_of(&answer);
                assert_eq!(
                    (status, code.as_str()),
                    (400, expected_code),
                    "case {case:?} with {request}: expected 400 {expected_code}, got {answer}"
                );
                continue;
            }
            assert_eq!(
                status, 200,
                "case {case:?} with {request}: a custom-health service accepts the call, got \
                 {answer}"
            );

            // The submit is stored rather than applied at once — the status it asks for
            // is due later, which `custom_health_status.rs` pins — so what this proves
            // is that both readers answer from the ONE stored record, whichever status
            // that record currently reports.
            // `ALL`, so the comparison holds whichever status the record reports: the
            // view a request that names no status gets leaves out an instance the
            // service has stopped routing traffic to.
            let found = ok(
                &router,
                "DiscoverInstances",
                json!({ "NamespaceName": "health-ns", "ServiceName": service_name,
                        "HealthStatus": "ALL" }),
            )
            .await;
            let discovered = found["Instances"][0]["HealthStatus"].clone();
            let health = ok(
                &router,
                "GetInstancesHealthStatus",
                json!({ "ServiceId": service_id }),
            )
            .await;
            assert_eq!(
                health["Status"]["i-1"], discovered,
                "case {case:?}: both readers answer from the one stored record, got {health} / \
                 {found}"
            );
            // `HealthStatus`, from the model's enum of that name.
            assert!(
                ["HEALTHY", "UNHEALTHY", "UNKNOWN"]
                    .contains(&discovered.as_str().unwrap_or_default()),
                "case {case:?}: a read reports a modelled HealthStatus, got {found}"
            );
        }

        // An instance the service never registered is not found — but only once the
        // service itself is in the custom-health registry: the registry check comes
        // first, so a service this call may not address is refused whatever instance it
        // names.
        let (status, code) = failure(
            &router,
            "UpdateInstanceCustomHealthStatus",
            json!({ "ServiceId": service_id, "InstanceId": "i-never", "Status": "HEALTHY" }),
        )
        .await;
        let expected = (400, refused_with.unwrap_or("InstanceNotFound"));
        assert_eq!(
            (status, code.as_str()),
            expected,
            "case {case:?}: an unregistered instance answers {expected:?}"
        );
    }

    // An id nothing created is refused the same way, so the registry lookup is not a
    // special case for services that exist.
    let (status, code) = failure(
        &router,
        "UpdateInstanceCustomHealthStatus",
        json!({ "ServiceId": "srv-nosuchservice", "InstanceId": "i-1", "Status": "HEALTHY" }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ServiceNotFound"),
        "a service id nothing created answers ServiceNotFound"
    );
}

/// `ListNamespaces` accepts only the `Name`/`Condition` pairs the `NamespaceFilter`
/// documentation states — `EQ` for TYPE, NAME, HTTP_NAME and RESOURCE_OWNER,
/// `BEGINS_WITH` for TYPE, NAME and HTTP_NAME, each with exactly one value — and
/// refuses everything else with 400 `InvalidInput`.
#[tokio::test]
async fn list_namespaces_refuses_every_filter_combination_it_does_not_support() {
    let router = emulator();
    namespace(&router, "filter-http-ns").await;
    private_namespace(&router, "filter-dns-ns.example").await;

    // `NamespaceFilterName` × `FilterCondition`, both from the model
    // (smithy_query shape="NamespaceFilterName,FilterCondition"), so every published
    // pair is a row — supported or not.
    let names = ["TYPE", "NAME", "HTTP_NAME", "RESOURCE_OWNER"];
    let conditions = ["EQ", "BEGINS_WITH", "IN", "BETWEEN"];
    for name in names {
        for condition in conditions {
            let value = match name {
                "TYPE" => "HTTP",
                "RESOURCE_OWNER" => "SELF",
                _ => "filter-http-ns",
            };
            let supported = match condition {
                "EQ" => true,
                "BEGINS_WITH" => name != "RESOURCE_OWNER",
                _ => false,
            };
            let body = json!({ "Filters": [{ "Name": name, "Values": [value],
                                             "Condition": condition }] });
            let (status, answer) = call(&router, "ListNamespaces", body.clone()).await;
            if supported {
                assert_eq!(
                    status, 200,
                    "case {name}+{condition}: a supported combination lists, got {answer}"
                );
            } else {
                let (code, _) = error_of(&answer);
                assert_eq!(
                    (status, code.as_str()),
                    (400, "InvalidInput"),
                    "case {name}+{condition}: an unsupported combination is refused, got {answer}"
                );
            }
        }
    }

    // The remaining filter rules: a condition takes exactly one value, an omitted
    // condition means `EQ`, and TYPE/RESOURCE_OWNER accept only their documented values.
    let rules: Vec<(&str, Value, bool)> = vec![
        (
            "an omitted Condition, which means EQ",
            json!([{ "Name": "NAME", "Values": ["filter-http-ns"] }]),
            true,
        ),
        (
            "two values under EQ",
            json!([{ "Name": "NAME", "Values": ["a", "b"], "Condition": "EQ" }]),
            false,
        ),
        (
            "no values at all",
            json!([{ "Name": "NAME", "Values": [] }]),
            false,
        ),
        (
            "a TYPE value outside NamespaceType",
            json!([{ "Name": "TYPE", "Values": ["DNS"] }]),
            false,
        ),
        (
            "a RESOURCE_OWNER value outside SELF/OTHER_ACCOUNTS",
            json!([{ "Name": "RESOURCE_OWNER", "Values": ["ME"] }]),
            false,
        ),
        (
            "two filters, both supported",
            json!([{ "Name": "TYPE", "Values": ["HTTP"] },
                   { "Name": "NAME", "Values": ["filter-http-ns"] }]),
            true,
        ),
        (
            "two filters, one unsupported",
            json!([{ "Name": "TYPE", "Values": ["HTTP"] },
                   { "Name": "NAME", "Values": ["x"], "Condition": "BETWEEN" }]),
            false,
        ),
    ];
    for (case, filters, accepted) in rules {
        let body = json!({ "Filters": filters });
        let (status, answer) = call(&router, "ListNamespaces", body.clone()).await;
        if accepted {
            assert_eq!(status, 200, "case {case:?}: expected a list, got {answer}");
        } else {
            let (code, _) = error_of(&answer);
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidInput"),
                "case {case:?}: expected 400 InvalidInput, got {answer}"
            );
        }
    }
}

/// `ListNamespaces` answers from what was stored: each filter keeps the namespaces
/// whose member it matches, and the page size and `NextToken` walk the whole store.
#[tokio::test]
async fn list_namespaces_reports_the_namespaces_each_filter_matches() {
    let router = emulator();
    namespace(&router, "list-http-one").await;
    namespace(&router, "list-http-two").await;
    private_namespace(&router, "list-private.example").await;

    let names = |listed: &Value| -> Vec<String> {
        listed["Namespaces"]
            .as_array()
            .expect("Namespaces is a list")
            .iter()
            .map(|summary| summary["Name"].as_str().unwrap_or_default().to_string())
            .collect()
    };

    // (case, the filter, the names it must report)
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "no filter at all",
            json!({}),
            vec!["list-http-one", "list-http-two", "list-private.example"],
        ),
        (
            "TYPE EQ HTTP",
            json!({ "Filters": [{ "Name": "TYPE", "Values": ["HTTP"] }] }),
            vec!["list-http-one", "list-http-two"],
        ),
        (
            "TYPE EQ DNS_PRIVATE",
            json!({ "Filters": [{ "Name": "TYPE", "Values": ["DNS_PRIVATE"] }] }),
            vec!["list-private.example"],
        ),
        (
            "TYPE EQ DNS_PUBLIC, which nothing created",
            json!({ "Filters": [{ "Name": "TYPE", "Values": ["DNS_PUBLIC"] }] }),
            vec![],
        ),
        (
            "NAME EQ one namespace",
            json!({ "Filters": [{ "Name": "NAME", "Values": ["list-http-two"] }] }),
            vec!["list-http-two"],
        ),
        (
            "NAME BEGINS_WITH a shared prefix",
            json!({ "Filters": [{ "Name": "NAME", "Values": ["list-http"],
                                  "Condition": "BEGINS_WITH" }] }),
            vec!["list-http-one", "list-http-two"],
        ),
        (
            "HTTP_NAME EQ, which reads Properties.HttpProperties.HttpName",
            json!({ "Filters": [{ "Name": "HTTP_NAME", "Values": ["list-private.example"] }] }),
            vec!["list-private.example"],
        ),
        (
            "RESOURCE_OWNER EQ SELF, which this account created",
            json!({ "Filters": [{ "Name": "RESOURCE_OWNER", "Values": ["SELF"] }] }),
            vec!["list-http-one", "list-http-two", "list-private.example"],
        ),
        (
            "RESOURCE_OWNER EQ OTHER_ACCOUNTS, which shared nothing in",
            json!({ "Filters": [{ "Name": "RESOURCE_OWNER", "Values": ["OTHER_ACCOUNTS"] }] }),
            vec![],
        ),
        (
            "TYPE and NAME together, which a namespace must match both of",
            json!({ "Filters": [{ "Name": "TYPE", "Values": ["HTTP"] },
                                { "Name": "NAME", "Values": ["list-private.example"] }] }),
            vec![],
        ),
    ];
    for (case, body, expected) in cases {
        let listed = ok(&router, "ListNamespaces", body.clone()).await;
        let mut found = names(&listed);
        found.sort();
        assert_eq!(
            found, expected,
            "case {case:?} with {body}: expected {expected:?}, got {listed}"
        );
    }

    // `MaxResults` caps the page and `NextToken` resumes after it, so every stored
    // namespace is reached exactly once however small the page is.
    for page_size in [1_i64, 2, 3, 100] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..10 {
            let mut body = json!({ "MaxResults": page_size });
            if let Some(next) = &token {
                body["NextToken"] = json!(next);
            }
            let page = ok(&router, "ListNamespaces", body.clone()).await;
            let page_names = names(&page);
            assert!(
                page_names.len() <= usize::try_from(page_size).unwrap_or(usize::MAX),
                "case MaxResults={page_size}: a page may not exceed the cap, got {page}"
            );
            walked.extend(page_names);
            match page["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        walked.sort();
        assert_eq!(
            walked,
            vec![
                "list-http-one".to_string(),
                "list-http-two".to_string(),
                "list-private.example".to_string()
            ],
            "case MaxResults={page_size}: paging reaches every namespace exactly once"
        );
    }

    // A token naming no stored namespace, or one this operation never issued, is
    // refused rather than silently restarting the list from the beginning.
    let (status, code) = failure(
        &router,
        "ListNamespaces",
        json!({ "NextToken": "ns-nosuchnamespace" }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidInput"),
        "an unresolvable NextToken is refused"
    );

    // The summary a list reports agrees with the read of the same namespace.
    let listed = ok(
        &router,
        "ListNamespaces",
        json!({ "Filters": [{ "Name": "NAME", "Values": ["list-private.example"] }] }),
    )
    .await;
    let summary = &listed["Namespaces"][0];
    let read = ok(
        &router,
        "GetNamespace",
        json!({ "Id": summary["Id"].as_str().unwrap_or_default() }),
    )
    .await;
    for member in ["Id", "Arn", "Name", "Type", "ServiceCount", "Properties"] {
        assert_eq!(
            summary[member], read["Namespace"][member],
            "member {member:?} must agree between the list and the read, got {listed} / {read}"
        );
    }
}
