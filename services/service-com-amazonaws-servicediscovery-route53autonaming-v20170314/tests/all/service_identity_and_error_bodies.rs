// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for three rules about what Cloud Map answers with, over the wire:
//!
//! 1. `DiscoverInstances` filters on CUSTOM attributes only, so a `QueryParameters` or
//!    `OptionalParameters` key in the service's own reserved attribute namespace is
//!    refused with `InvalidInput` naming the member and the keys, never treated as a
//!    filter.
//! 2. A service `Id` is a function of the pair that names the service — its namespace
//!    and its `Name` — so the same pair always reads back the same `Id`, including after
//!    a `DeleteService` freed the name, while a different pair gets a different `Id`.
//! 3. A refusal's `Message` is per-operation and built from the request or deliberately
//!    empty: `UpdateService` refusing a record-type change reports BOTH whole
//!    record-type sets; a namespace UPDATE's and a create-in-a-namespace's
//!    `NamespaceNotFound` is the namespace reference the request named, where the
//!    namespace READ carries no message at all; and the same holds across the operation,
//!    tagging, instance-registry and conflict refusals.
//!
//! Every request goes through `MultiServiceRouter::dispatch`, i.e. the same path the
//! emulator serves in production (`awsJson1_1`: `POST /` with `X-Amz-Target`), using the
//! harness `cloud_map_core` already owns.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloud_map_core::{call, dns_namespace, emulator, namespace, ok, service};

/// The attribute keys the service defines itself, from
/// `smithy_query(query="doc",shape="RegisterInstanceRequest",member="Attributes")`,
/// which lists exactly these keys and then "Custom attributes" for everything else.
const RESERVED_ATTRIBUTE_KEYS: &[&str] = &[
    "AWS_ALIAS_DNS_NAME",
    "AWS_EC2_INSTANCE_ID",
    "AWS_INIT_HEALTH_STATUS",
    "AWS_INSTANCE_CNAME",
    "AWS_INSTANCE_IPV4",
    "AWS_INSTANCE_IPV6",
    "AWS_INSTANCE_PORT",
];

/// The two members `DiscoverInstances` filters with.
const FILTER_MEMBERS: [&str; 2] = ["QueryParameters", "OptionalParameters"];

/// The wire status, error code and `Message` of one refused call.
///
/// The code is the shape name `__type` carries; the message is read under either casing,
/// because a handler-returned error renders the modelled `Message` member while a
/// framework refusal renders a lowercase `message`.
async fn refusal(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (u16, String, Option<String>) {
    let (status, json) = call(router, operation, body).await;
    assert_ne!(status, 200, "{operation} must fail, got {json}");
    let code = json["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    let message = json
        .get("Message")
        .or_else(|| json.get("message"))
        .and_then(Value::as_str)
        .map(str::to_string);
    (status, code, message)
}

/// The refusal AWS words for reserved keys in one filter member.
fn reserved_message(member: &str, keys: &[&str]) -> String {
    format!(
        "Request {member} contains invalid attribute: [{}]. [aws_] prefix is reserved for AWS \
         attributes.",
        keys.join(", ")
    )
}

/// A filter key in the reserved namespace is a bad request on either filter member,
/// whatever its case and however many of them the map carries, while a key outside that
/// namespace is an ordinary custom-attribute filter.
#[tokio::test]
async fn a_reserved_attribute_key_is_refused_on_either_filter_member() {
    let router = emulator();
    let namespace_name = "reserved-filters-ns";
    let (_, namespace_id) = namespace(&router, namespace_name).await;
    let service_name = "reserved-filters-svc";
    let service_id = service(&router, &namespace_id, service_name).await;
    for (instance_id, zone) in [("inst-a", "z1"), ("inst-b", "z2")] {
        ok(
            &router,
            "RegisterInstance",
            json!({
                "ServiceId": service_id,
                "InstanceId": instance_id,
                "Attributes": {
                    "AWS_INSTANCE_IPV4": "192.0.2.10",
                    "tier": "web",
                    "zone": zone,
                },
            }),
        )
        .await;
    }

    // (case, the filter map, the keys the refusal must name — empty for an accepted map)
    let mut cases: Vec<(String, Value, Vec<&str>)> = vec![
        (
            "the reserved prefix with nothing behind it".to_string(),
            json!({ "AWS_": "x" }),
            vec!["AWS_"],
        ),
        (
            "a reserved key in lower case".to_string(),
            json!({ "aws_instance_ipv4": "192.0.2.10" }),
            vec!["aws_instance_ipv4"],
        ),
        (
            "a reserved key in mixed case".to_string(),
            json!({ "Aws_Alias_Dns_Name": "lb.example.com" }),
            vec!["Aws_Alias_Dns_Name"],
        ),
        (
            "two reserved keys at once".to_string(),
            json!({ "AWS_INSTANCE_PORT": "8080", "AWS_INSTANCE_IPV4": "192.0.2.10" }),
            vec!["AWS_INSTANCE_IPV4", "AWS_INSTANCE_PORT"],
        ),
        (
            "one reserved key beside a custom one".to_string(),
            json!({ "tier": "web", "AWS_INSTANCE_IPV4": "192.0.2.10" }),
            vec!["AWS_INSTANCE_IPV4"],
        ),
        // Keys that only look reserved: the namespace is the `aws_` PREFIX, so a key
        // that merely starts with those letters is a custom attribute.
        (
            "a custom key matching an instance attribute".to_string(),
            json!({ "zone": "z1" }),
            Vec::new(),
        ),
        (
            "a custom key that starts with aws but not aws_".to_string(),
            json!({ "awsome": "yes" }),
            Vec::new(),
        ),
        (
            "a custom key that carries the prefix inside it".to_string(),
            json!({ "my_aws_key": "yes" }),
            Vec::new(),
        ),
        (
            "a custom key that is the prefix letters alone".to_string(),
            json!({ "AWS": "yes" }),
            Vec::new(),
        ),
    ];
    // Every key of the published reserved catalog, so no row passes by naming one of
    // them in particular.
    for key in RESERVED_ATTRIBUTE_KEYS {
        cases.push((
            format!("the published reserved key {key}"),
            json!({ *key: "x" }),
            vec![key],
        ));
    }

    for (case, filters, reserved) in cases {
        for member in FILTER_MEMBERS {
            let mut body = json!({
                "NamespaceName": namespace_name,
                "ServiceName": service_name,
                "HealthStatus": "ALL",
            });
            body[member] = filters.clone();

            if reserved.is_empty() {
                let (status, answer) = call(&router, "DiscoverInstances", body.clone()).await;
                assert_eq!(
                    status, 200,
                    "case {case:?} on {member}: a custom-attribute filter is accepted, got \
                     {status} {answer}"
                );
                continue;
            }

            let (status, code, message) = refusal(&router, "DiscoverInstances", body).await;
            let expected = reserved_message(member, &reserved);
            assert_eq!(
                (status, code.as_str(), message.as_deref()),
                (400, "InvalidInput", Some(expected.as_str())),
                "case {case:?} on {member}: expected 400 InvalidInput {expected:?}, got {status} \
                 {code} {message:?}"
            );
        }
    }

    // The refused calls changed nothing: the instances are still discoverable, and a
    // custom filter still selects by value.
    let found = ok(
        &router,
        "DiscoverInstances",
        json!({
            "NamespaceName": namespace_name,
            "ServiceName": service_name,
            "HealthStatus": "ALL",
            "QueryParameters": { "zone": "z2" },
        }),
    )
    .await;
    assert_eq!(
        found["Instances"],
        json!([{
            "InstanceId": "inst-b",
            "NamespaceName": namespace_name,
            "ServiceName": service_name,
            "HealthStatus": "UNKNOWN",
            "Attributes": {
                "AWS_INSTANCE_IPV4": "192.0.2.10",
                "tier": "web",
                "zone": "z2",
            },
        }]),
        "a custom filter still selects by value, got {found}"
    );
}

/// A service `Id` is derived from the namespace it lives in and its `Name`: the same pair
/// always reads back the same `Id` — including when the name was freed by a delete and
/// created again — and the same name in another namespace gets a different one.
#[tokio::test]
async fn a_service_id_is_derived_from_its_namespace_and_name() {
    let router = emulator();
    let (_, first_namespace) = namespace(&router, "derived-ids-ns-a").await;
    let (_, second_namespace) = namespace(&router, "derived-ids-ns-b").await;

    // The names differ along what the rule generalises over: length and the punctuation
    // `ServiceName` admits.
    let names = [
        "a",
        "derived-service",
        "svc.with.dots",
        "_svc._tcp",
        "Service_With_Mixed-Case.9",
        &"n".repeat(60),
    ];

    let mut seen: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for name in names {
        let first = service(&router, &first_namespace, name).await;

        // The id has the published shape: `srv-` and 16 lowercase alphanumerics.
        let body = first
            .strip_prefix("srv-")
            .unwrap_or_else(|| panic!("case {name:?}: a service id starts with srv-, got {first}"));
        assert!(
            body.len() == 16 && body.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
            "case {name:?}: expected srv- and 16 lowercase alphanumerics, got {first:?}"
        );

        // It is the id the service reads back under, and the read is stable.
        for round in 0..2 {
            let read = ok(&router, "GetService", json!({ "Id": first })).await["Service"].clone();
            assert_eq!(
                (read["Id"].clone(), read["Name"].clone()),
                (json!(first), json!(name)),
                "case {name:?} read {round}: the stored service answers under its derived id, got \
                 {read}"
            );
        }

        // Freeing the name and creating it again answers with the same id.
        ok(&router, "DeleteService", json!({ "Id": first })).await;
        let again = service(&router, &first_namespace, name).await;
        assert_eq!(
            again, first,
            "case {name:?}: the re-created service must reuse id {first:?}, got {again:?}"
        );

        // The same name under another namespace is another service, so another id.
        let elsewhere = service(&router, &second_namespace, name).await;
        assert_ne!(
            elsewhere, first,
            "case {name:?}: the same name in another namespace must get its own id, got \
             {elsewhere:?}"
        );

        for (id, owner) in [(first.clone(), "a"), (elsewhere.clone(), "b")] {
            if let Some(previous) = seen.insert(id.clone(), format!("{name:?} in namespace {owner}")) {
                panic!("id {id:?} was minted for {previous} and again for {name:?}");
            }
        }
    }
}

/// `UpdateService` refusing a record-type change reports both whole record-type sets:
/// the ones the stored service carries and the ones the request named.
#[tokio::test]
async fn a_refused_record_type_change_reports_both_record_type_sets() {
    let router = emulator();
    let namespace_id = dns_namespace(&router, "record-type-message").await;

    /// One `DnsRecord` object.
    fn record(record_type: &str, ttl: i64) -> Value {
        json!({ "Type": record_type, "TTL": ttl })
    }

    // (case, the record types the service is created with, the types the change names)
    let cases: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        ("one type swapped for another", vec!["A"], vec!["CNAME"]),
        ("a type added beside the stored one", vec!["A"], vec!["A", "SRV"]),
        (
            "one of two stored types swapped",
            vec!["A", "AAAA"],
            vec!["A", "SRV"],
        ),
        (
            "both stored types replaced",
            vec!["A", "AAAA"],
            vec!["CNAME", "SRV"],
        ),
        ("a type the service never carried", vec!["SRV"], vec!["AAAA"]),
    ];

    for (index, (case, stored, requested)) in cases.into_iter().enumerate() {
        let created = ok(
            &router,
            "CreateService",
            json!({
                "Name": format!("record-type-{index}"),
                "NamespaceId": namespace_id,
                "DnsConfig": {
                    "DnsRecords": stored.iter().map(|t| record(t, 60)).collect::<Vec<Value>>(),
                },
            }),
        )
        .await;
        let service_id = created["Service"]["Id"]
            .as_str()
            .expect("CreateService returns the service it stored")
            .to_string();

        let (status, code, message) = refusal(
            &router,
            "UpdateService",
            json!({
                "Id": service_id,
                "Service": {
                    "DnsConfig": {
                        "DnsRecords": requested
                            .iter()
                            .map(|t| record(t, 60))
                            .collect::<Vec<Value>>(),
                    },
                },
            }),
        )
        .await;
        let expected = format!(
            "The record types in the request must be the same as those present in the existing \
             service. Records in the existing service are [{}], but the records in the request \
             are [{}]. The record types of a service cannot be changed.",
            stored.join(", "),
            requested.join(", ")
        );
        assert_eq!(
            (status, code.as_str(), message.as_deref()),
            (400, "InvalidInput", Some(expected.as_str())),
            "case {case:?}: expected 400 InvalidInput {expected:?}, got {status} {code} {message:?}"
        );

        // Nothing was written, and restating the stored types is still accepted.
        let read = ok(&router, "GetService", json!({ "Id": service_id })).await["Service"].clone();
        assert_eq!(
            read["DnsConfig"]["DnsRecords"],
            json!(stored.iter().map(|t| record(t, 60)).collect::<Vec<Value>>()),
            "case {case:?}: a refused change stores nothing, got {read}"
        );
        ok(
            &router,
            "UpdateService",
            json!({
                "Id": service_id,
                "Service": {
                    "DnsConfig": {
                        "DnsRecords": stored.iter().map(|t| record(t, 120)).collect::<Vec<Value>>(),
                    },
                },
            }),
        )
        .await;
    }
}

/// A namespace UPDATE that cannot resolve its reference answers `NamespaceNotFound`
/// whose whole body is the reference the request named, while the namespace READ answers
/// the same class with no message at all.
#[tokio::test]
async fn a_namespace_update_echoes_the_reference_it_could_not_resolve() {
    let router = emulator();

    // (the update, the change body it requires)
    let updates: [(&str, Value); 3] = [
        ("UpdateHttpNamespace", json!({ "Description": "x" })),
        (
            "UpdatePrivateDnsNamespace",
            json!({ "Properties": { "DnsProperties": { "SOA": { "TTL": 30 } } } }),
        ),
        ("UpdatePublicDnsNamespace", json!({ "Description": "x" })),
    ];

    // References that resolve to no namespace: the shapes differ — an id, an ARN, a
    // name, and the empty string `Arn`'s `length 0..255` admits — so no row can pass by
    // matching one of them.
    let references = [
        "ns-nosuchnamespace",
        "ns-0123456789abcdef",
        "arn:aws:servicediscovery:us-west-2:000000000000:namespace/ns-gone",
        "never-created.example.com",
        "",
    ];

    for reference in references {
        for (operation, change) in &updates {
            let (status, code, message) = refusal(
                &router,
                operation,
                json!({ "Id": reference, "Namespace": change }),
            )
            .await;
            assert_eq!(
                (code.as_str(), message.as_deref()),
                ("NamespaceNotFound", Some(reference)),
                "case {reference:?} on {operation}: the body is the requested reference, got \
                 {code} {message:?}"
            );

            // The READ of the same reference reports the class alone — the asymmetry
            // that tells the two errors apart — at the same status as the update.
            let (read_status, read_code, read_message) =
                refusal(&router, "GetNamespace", json!({ "Id": reference })).await;
            assert_eq!(
                (read_status, read_code.as_str(), read_message.as_deref()),
                (status, "NamespaceNotFound", None),
                "case {reference:?}: the read carries no message, got {read_status} {read_code} \
                 {read_message:?}"
            );
        }
    }
}

/// The same per-operation rule across the rest of the service's refusals: the reads
/// report the class alone, a call that NAMES a resource to act on echoes the reference
/// or the pair it was asked about, and the two in-use conflicts are fixed sentences
/// naming neither resource nor count.
#[tokio::test]
async fn each_refusal_carries_the_body_of_the_operation_that_raised_it() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "bodies-ns").await;
    let service_id = service(&router, &namespace_id, "bodies-svc").await;
    ok(
        &router,
        "RegisterInstance",
        json!({
            "ServiceId": service_id,
            "InstanceId": "i-0",
            "Attributes": { "stage": "prod" },
        }),
    )
    .await;
    let service_arn = ok(&router, "GetService", json!({ "Id": service_id })).await["Service"]["Arn"]
        .as_str()
        .expect("GetService reports the service's Arn")
        .to_string();
    let missing_arn = format!(
        "arn:aws:servicediscovery:{}:{}:namespace/ns-nothinghere",
        crate::cloud_map_core::REGION,
        engine::EMULATOR_ACCOUNT_ID
    );

    // (case, operation, request, expected class, the Message — `None` for no member)
    let cases: Vec<(&str, &str, Value, &str, Option<String>)> = vec![
        (
            "an operation id nothing filed",
            "GetOperation",
            json!({ "OperationId": "nosuchoperation" }),
            "OperationNotFound",
            None,
        ),
        (
            "an operation id of the width this service mints",
            "GetOperation",
            json!({ "OperationId": "a".repeat(32) }),
            "OperationNotFound",
            None,
        ),
        (
            "a namespace read of a namespace nothing created",
            "GetNamespace",
            json!({ "Id": "ns-nothinghere" }),
            "NamespaceNotFound",
            None,
        ),
        (
            "a tag write against a resource nothing minted",
            "TagResource",
            json!({
                "ResourceARN": missing_arn,
                "Tags": [{ "Key": "stage", "Value": "prod" }],
            }),
            "ResourceNotFoundException",
            None,
        ),
        (
            "a tag read against the same resource",
            "ListTagsForResource",
            json!({ "ResourceARN": missing_arn }),
            "ResourceNotFoundException",
            None,
        ),
        (
            "a create in a namespace nothing created",
            "CreateService",
            json!({ "Name": "orphan-svc", "NamespaceId": "ns-nothinghere" }),
            "NamespaceNotFound",
            Some("ns-nothinghere".to_string()),
        ),
        (
            "a create naming that namespace by ARN",
            "CreateService",
            json!({ "Name": "orphan-svc", "NamespaceId": missing_arn }),
            "NamespaceNotFound",
            Some(missing_arn.clone()),
        ),
        (
            "a delete of a namespace nothing created",
            "DeleteNamespace",
            json!({ "Id": "ns-nothinghere" }),
            "NamespaceNotFound",
            Some("ns-nothinghere".to_string()),
        ),
        (
            "a namespace that still holds a service",
            "DeleteNamespace",
            json!({ "Id": namespace_id }),
            "ResourceInUse",
            Some(
                "Namespace has associated services; delete the services before deleting the \
                 namespace"
                    .to_string(),
            ),
        ),
        (
            "a service that still holds an instance",
            "DeleteService",
            json!({ "Id": service_id }),
            "ResourceInUse",
            Some(
                "Service contains registered instances; delete the instances before deleting \
                 the service"
                    .to_string(),
            ),
        ),
        (
            "an instance read of an instance nothing registered",
            "GetInstance",
            json!({ "ServiceId": service_id, "InstanceId": "i-never" }),
            "InstanceNotFound",
            Some("i-never".to_string()),
        ),
        (
            "a deregister of that same instance",
            "DeregisterInstance",
            json!({ "ServiceId": service_id, "InstanceId": "i-never" }),
            "InstanceNotFound",
            Some(format!(
                "instanceId i-never doesn't exists for service {service_arn}"
            )),
        ),
    ];

    for (case, operation, body, expected_code, expected_message) in cases {
        let (_, code, message) = refusal(&router, operation, body).await;
        assert_eq!(
            (code.as_str(), message.as_deref()),
            (expected_code, expected_message.as_deref()),
            "case {case:?} on {operation}: expected {expected_code} {expected_message:?}, got \
             {code} {message:?}"
        );
    }

    // Nothing the refused calls named was removed: the service and its instance are
    // still there to be read.
    let read = ok(&router, "GetService", json!({ "Id": service_id })).await["Service"].clone();
    assert_eq!(
        read["InstanceCount"],
        json!(1),
        "the refused deletes removed nothing, got {read}"
    );
}
