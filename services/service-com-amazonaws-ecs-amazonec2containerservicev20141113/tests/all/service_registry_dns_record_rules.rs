// SPDX-License-Identifier: CC0-1.0

//! What a `serviceRegistries` entry has to name, decided by the Cloud Map service's own DNS
//! record type together with the task definition's `networkMode`.
//!
//! The rule under test, asserted over a table that spans the class rather than over one
//! request — the rows differ in record type (`SRV`, `A`, `AAAA`, a service that publishes no
//! DNS record at all), in network mode (`awsvpc`, `bridge`, `host`) and in the form of the
//! triplet (`port` only, the complete `containerName`+`containerPort` pair, both, a half
//! pair, neither, a container the revision does not declare, a port it does not publish):
//!
//! * `ServiceRegistry$containerName`: "The container name value ... It's already specified
//!   in the task definition" — a name none of the revision's `containerDefinitions` carries
//!   is refused whatever the record type;
//! * an address-only record (`A`, `AAAA`) resolves to an address its owner holds alone, so a
//!   revision in the `bridge` or `host` network mode — whose tasks share the container
//!   instance's address — cannot be registered in one;
//! * "If the task definition that your service task specifies uses the `bridge` or `host`
//!   network mode, you must specify a `containerName` and `containerPort` combination from
//!   the task definition";
//! * "If the task definition ... uses the `awsvpc` network mode and a type SRV DNS record is
//!   used, you must specify either a `containerName` and `containerPort` combination or a
//!   `port` value. However, you can't specify both" — an `SRV` record publishes exactly one
//!   port, so the entry says which once, and never twice or not at all;
//! * a pair the entry names has to be one the revision declares.
//!
//! Every refusal is a 400 `InvalidParameterException` that leaves NO ECS service behind, and
//! every acceptance is read back through `DescribeServices`, so the stored triplet is the
//! one the request named. `UpdateService` and `CreateTaskSet` attach the same registries and
//! are held to the same rule.
//!
//! Every case drives the emulator through the shared wire harness in [`crate::ecs_wire`],
//! the same front door a client uses, and the Cloud Map registries are created through CLOUD
//! MAP's own API on the one shared engine — so nothing reaches into a state struct and a
//! refusal that is not computed from persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    answered, awsvpc_network_configuration, cloud_map, cluster_with_capacity,
    emulator_with_cloud_map, list, ok, refusal, string,
};

/// The cluster every service below is created in.
const CLUSTER: &str = "registryrules";

/// The one container every revision declares, and the port it publishes.
const CONTAINER: &str = "app";
const CONTAINER_PORT: i64 = 80;

/// A container name NO revision here declares, for the rows about an unresolvable
/// `containerName`.
const MISSING_CONTAINER: &str = "not-a-container";

/// A container port no revision here publishes, for the row about an unresolvable
/// `containerPort`.
const UNPUBLISHED_PORT: i64 = 9999;

/// The `port` an entry publishes when it names one directly instead of a container pair.
const REGISTRY_PORT: i64 = 8080;

/// A VPC id of the shape `CreatePrivateDnsNamespace$Vpc` takes. Never resolved: Cloud Map
/// stores the id it is given.
const VPC: &str = "vpc-0123456789abcdef0";

/// The modelled error every refusal in this suite carries, with HTTP 400.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The refusal an `SRV` registry that names its port zero times or twice earns.
fn srv_port_source_message(registry_arn: &str) -> String {
    format!(
        "When specifying a value for serviceRegistries that is configured to use a type \
         'SRV' DNS record, you must also enter a value for either 'port' or the \
         'containerName' and 'containerPort' combination, but not both. Registry: \
         {registry_arn}"
    )
}

/// The refusal an address-only record earns against a revision on the host's or the
/// bridge's network. `record_type` is the one the Cloud Map service publishes.
fn address_record_message(record_type: &str, registry_arn: &str) -> String {
    format!(
        "The specified serviceRegistries value is configured to use a type '{record_type}' \
         DNS record, which is not supported when specifying 'host' or 'bridge' for \
         networkMode. Change to 'SRV' record, which is the supported DNS record type. \
         Registry: {registry_arn}"
    )
}

/// The refusal a `containerName` the revision does not declare earns.
fn container_not_valid_message(container_name: &str, registry_arn: &str) -> String {
    format!(
        "The container {container_name} is not valid. The value for 'containerName' must \
         already be specified in the task definition. Registry: {registry_arn}"
    )
}

/// The refusal a revision on the host's or the bridge's network earns when the entry names
/// no complete container pair.
fn host_network_pair_message(registry_arn: &str) -> String {
    format!(
        "When specifying 'host' or 'bridge' for networkMode, values for 'containerName' and \
         'containerPort' must be specified from the task definition. Registry: \
         {registry_arn}"
    )
}

/// The refusal a `containerPort` the named container does not publish earns.
fn unpublished_port_message(container_name: &str, container_port: i64) -> String {
    format!(
        "The container {container_name} did not have a container port {container_port} \
         defined."
    )
}

/// Register one revision in `network_mode` whose single container publishes
/// [`CONTAINER_PORT`], and return its family (which is what a service names it by).
async fn register_revision(router: &MultiServiceRouter, network_mode: &str, case: &str) -> String {
    let family = format!("registryrules-{network_mode}");
    ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "networkMode": network_mode,
            "containerDefinitions": [{
                "name": CONTAINER,
                "image": "public.ecr.aws/nginx/nginx:latest",
                "memory": 128,
                "portMappings": [{
                    "containerPort": CONTAINER_PORT,
                    "hostPort": CONTAINER_PORT,
                    "protocol": "tcp",
                }],
            }],
        }),
        case,
    )
    .await;
    family
}

/// Create a Cloud Map service in `namespace_id` publishing `record_type`, or no DNS record
/// at all when `record_type` is `None`, and return its ARN — the `registryArn` an ECS
/// service registry names.
async fn cloud_map_registry(
    router: &MultiServiceRouter,
    namespace_id: &str,
    name: &str,
    record_type: Option<&str>,
    case: &str,
) -> String {
    let mut body = json!({ "Name": name, "NamespaceId": namespace_id });
    if let Some(record_type) = record_type {
        // `DnsRecord$Type` is the model's `RecordType` enum (A, AAAA, CNAME, SRV).
        body["DnsConfig"] = json!({ "DnsRecords": [{ "Type": record_type, "TTL": 60 }] });
    }
    let created = cloud_map(router, "CreateService", body, case).await;
    string(&created["Service"], "Arn", case)
}

/// The namespace id a `Create*Namespace` call's operation targets.
async fn namespace_id(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    case: &str,
) -> String {
    let created = cloud_map(router, operation, body, case).await;
    let operation_id = string(&created, "OperationId", case);
    let filed = cloud_map(
        router,
        "GetOperation",
        json!({ "OperationId": operation_id }),
        case,
    )
    .await;
    string(&filed["Operation"]["Targets"], "NAMESPACE", case)
}

/// The registries and revisions every row below is built from.
struct Fixture {
    router: MultiServiceRouter,
    /// `registryArn` of a Cloud Map service publishing an `SRV` record.
    srv: String,
    /// `registryArn` of one publishing an `A` record.
    a: String,
    /// `registryArn` of one publishing an `AAAA` record.
    aaaa: String,
    /// `registryArn` of an HTTP-only service, which publishes no DNS record at all.
    http: String,
}

impl Fixture {
    async fn build() -> Self {
        let case = "fixture";
        let router = emulator_with_cloud_map();
        cluster_with_capacity(&router, CLUSTER, case).await;
        for mode in ["awsvpc", "bridge", "host"] {
            register_revision(&router, mode, case).await;
        }
        // A DnsConfig asks Cloud Map for Route 53 records, so it needs a namespace that
        // holds a hosted zone; the record-less service goes in an HTTP namespace.
        let dns_namespace = namespace_id(
            &router,
            "CreatePrivateDnsNamespace",
            json!({ "Name": "registryrules.example.com", "Vpc": VPC }),
            case,
        )
        .await;
        let http_namespace = namespace_id(
            &router,
            "CreateHttpNamespace",
            json!({ "Name": "registryrules-http" }),
            case,
        )
        .await;
        let srv =
            cloud_map_registry(&router, &dns_namespace, "srvreg", Some("SRV"), case).await;
        let a = cloud_map_registry(&router, &dns_namespace, "areg", Some("A"), case).await;
        let aaaa =
            cloud_map_registry(&router, &dns_namespace, "aaaareg", Some("AAAA"), case).await;
        let http = cloud_map_registry(&router, &http_namespace, "httpreg", None, case).await;
        Self {
            router,
            srv,
            a,
            aaaa,
            http,
        }
    }
}

/// One row: a revision's network mode, the registry entry a create names, and whether the
/// create is answered or refused with a particular sentence.
struct Row {
    case: &'static str,
    network_mode: &'static str,
    /// The `serviceRegistries` entry, `registryArn` included.
    registry: Value,
    /// `None` when the create must be ANSWERED; the refusal's message otherwise.
    refused_with: Option<String>,
}

/// A `CreateService` body for `row`, in the cluster the fixture holds.
fn create_request(row: &Row, name: &str) -> Value {
    let mut request = json!({
        "cluster": CLUSTER,
        "serviceName": name,
        "taskDefinition": format!("registryrules-{}", row.network_mode),
        "desiredCount": 1,
        "launchType": "EC2",
        "serviceRegistries": [row.registry.clone()],
    });
    if row.network_mode == "awsvpc" {
        request["networkConfiguration"] = awsvpc_network_configuration();
    }
    request
}

#[tokio::test]
async fn a_service_registry_is_held_to_its_record_type_and_the_revisions_network_mode() {
    let fixture = Fixture::build().await;
    let router = &fixture.router;
    let (srv, a, aaaa, http) = (
        fixture.srv.as_str(),
        fixture.a.as_str(),
        fixture.aaaa.as_str(),
        fixture.http.as_str(),
    );

    let rows = vec![
        // An SRV record publishes ONE port, named exactly once.
        Row {
            case: "an SRV registry naming neither a port nor a container pair",
            network_mode: "awsvpc",
            registry: json!({ "registryArn": srv }),
            refused_with: Some(srv_port_source_message(srv)),
        },
        Row {
            case: "an SRV registry naming a port",
            network_mode: "awsvpc",
            registry: json!({ "registryArn": srv, "port": REGISTRY_PORT }),
            refused_with: None,
        },
        Row {
            case: "an SRV registry naming a complete container pair",
            network_mode: "awsvpc",
            registry: json!({
                "registryArn": srv,
                "containerName": CONTAINER,
                "containerPort": CONTAINER_PORT,
            }),
            refused_with: None,
        },
        Row {
            case: "an SRV registry naming both a port and a container pair",
            network_mode: "awsvpc",
            registry: json!({
                "registryArn": srv,
                "containerName": CONTAINER,
                "containerPort": CONTAINER_PORT,
                "port": REGISTRY_PORT,
            }),
            refused_with: Some(srv_port_source_message(srv)),
        },
        Row {
            case: "an SRV registry naming a containerName with no containerPort",
            network_mode: "awsvpc",
            registry: json!({ "registryArn": srv, "containerName": CONTAINER }),
            refused_with: Some(srv_port_source_message(srv)),
        },
        Row {
            case: "an SRV registry naming a containerPort with no containerName",
            network_mode: "awsvpc",
            registry: json!({ "registryArn": srv, "containerPort": CONTAINER_PORT }),
            refused_with: Some(srv_port_source_message(srv)),
        },
        // An address-only record carries no port, so an awsvpc task — which owns the
        // address of its own interface — needs nothing more than the registry.
        Row {
            case: "an A registry against an awsvpc revision",
            network_mode: "awsvpc",
            registry: json!({ "registryArn": a }),
            refused_with: None,
        },
        Row {
            case: "an AAAA registry against an awsvpc revision naming a container pair",
            network_mode: "awsvpc",
            registry: json!({
                "registryArn": aaaa,
                "containerName": CONTAINER,
                "containerPort": CONTAINER_PORT,
            }),
            refused_with: None,
        },
        // ... and cannot be used at all by a revision that shares the instance's address.
        Row {
            case: "an A registry against a bridge revision",
            network_mode: "bridge",
            registry: json!({
                "registryArn": a,
                "containerName": CONTAINER,
                "containerPort": CONTAINER_PORT,
            }),
            refused_with: Some(address_record_message("A", a)),
        },
        Row {
            case: "an AAAA registry against a host revision",
            network_mode: "host",
            registry: json!({
                "registryArn": aaaa,
                "containerName": CONTAINER,
                "containerPort": CONTAINER_PORT,
            }),
            refused_with: Some(address_record_message("AAAA", aaaa)),
        },
        // An SRV record is the one that works for those modes, with the pair named.
        Row {
            case: "an SRV registry against a bridge revision naming the pair",
            network_mode: "bridge",
            registry: json!({
                "registryArn": srv,
                "containerName": CONTAINER,
                "containerPort": CONTAINER_PORT,
            }),
            refused_with: None,
        },
        Row {
            case: "an SRV registry against a host revision naming the pair",
            network_mode: "host",
            registry: json!({
                "registryArn": srv,
                "containerName": CONTAINER,
                "containerPort": CONTAINER_PORT,
            }),
            refused_with: None,
        },
        Row {
            case: "an SRV registry against a bridge revision naming only a port",
            network_mode: "bridge",
            registry: json!({ "registryArn": srv, "port": REGISTRY_PORT }),
            refused_with: Some(host_network_pair_message(srv)),
        },
        // The containerName is resolved against the revision whatever the record type.
        Row {
            case: "an SRV registry naming a container the awsvpc revision does not declare",
            network_mode: "awsvpc",
            registry: json!({
                "registryArn": srv,
                "containerName": MISSING_CONTAINER,
                "containerPort": CONTAINER_PORT,
            }),
            refused_with: Some(container_not_valid_message(MISSING_CONTAINER, srv)),
        },
        Row {
            case: "an A registry naming a container the bridge revision does not declare",
            network_mode: "bridge",
            registry: json!({
                "registryArn": a,
                "containerName": MISSING_CONTAINER,
                "containerPort": CONTAINER_PORT,
            }),
            refused_with: Some(container_not_valid_message(MISSING_CONTAINER, a)),
        },
        // A pair the entry names has to be one the revision declares.
        Row {
            case: "an SRV registry naming a port the container does not publish",
            network_mode: "bridge",
            registry: json!({
                "registryArn": srv,
                "containerName": CONTAINER,
                "containerPort": UNPUBLISHED_PORT,
            }),
            refused_with: Some(unpublished_port_message(CONTAINER, UNPUBLISHED_PORT)),
        },
        // A registry that publishes no DNS record at all is held to no record-type rule.
        Row {
            case: "an HTTP-only registry against an awsvpc revision",
            network_mode: "awsvpc",
            registry: json!({ "registryArn": http }),
            refused_with: None,
        },
    ];

    let mut accepted = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let name = format!("regsvc{index}");
        let request = create_request(row, &name);
        match row.refused_with.as_deref() {
            Some(expected) => {
                let (code, message) =
                    refusal(router, "CreateService", request.clone(), case).await;
                assert_eq!(
                    (code.as_str(), message.as_str()),
                    (INVALID_PARAMETER, expected),
                    "case {case:?}: expected the create to be refused with \
                     {INVALID_PARAMETER} {expected:?}, got ({code:?}, {message:?})"
                );
            }
            None => {
                let body = ok(router, "CreateService", request.clone(), case).await;
                let stored = &body["service"]["serviceRegistries"];
                assert_eq!(
                    stored,
                    &json!([row.registry.clone()]),
                    "case {case:?}: expected the create to store the registry it was \
                     given, got {stored}"
                );
                accepted.push((name, row.registry.clone()));
            }
        }
    }

    // Every REFUSED create left no ECS service behind: the cluster holds exactly the
    // services the accepted rows created, read back through the listing.
    let listed = ok(
        router,
        "ListServices",
        json!({ "cluster": CLUSTER }),
        "listing",
    )
    .await;
    let mut listed_names: Vec<String> = list(&listed, "serviceArns", "listing")
        .into_iter()
        .map(|arn| {
            arn.as_str()
                .unwrap_or_else(|| panic!("expected a service ARN, got {arn}"))
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    listed_names.sort();
    let mut expected_names: Vec<String> =
        accepted.iter().map(|(name, _)| name.clone()).collect();
    expected_names.sort();
    assert_eq!(
        listed_names, expected_names,
        "a refused create stores nothing: expected the cluster to hold {expected_names:?}, \
         got {listed_names:?}"
    );

    // And every ACCEPTED create is readable back with the triplet it named.
    for (name, registry) in &accepted {
        let case = name.as_str();
        let body = ok(
            router,
            "DescribeServices",
            json!({ "cluster": CLUSTER, "services": [name] }),
            case,
        )
        .await;
        let described = list(&body, "services", case);
        let found = described
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: expected the service to be described"));
        assert_eq!(
            &found["serviceRegistries"],
            &json!([registry.clone()]),
            "case {case:?}: expected DescribeServices to report the stored registry, got \
             {found}"
        );
    }
}

#[tokio::test]
async fn update_service_holds_an_attached_registry_to_the_same_rule() {
    let fixture = Fixture::build().await;
    let router = &fixture.router;
    let (srv, a) = (fixture.srv.as_str(), fixture.a.as_str());

    // A bridge service whose SRV registry names the pair is created, so what the update
    // changes is only the registry list.
    let accepted = json!({
        "registryArn": srv,
        "containerName": CONTAINER,
        "containerPort": CONTAINER_PORT,
    });
    let created = ok(
        router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": "updateregsvc",
            "taskDefinition": "registryrules-bridge",
            "desiredCount": 1,
            "launchType": "EC2",
            "serviceRegistries": [accepted.clone()],
        }),
        "setup",
    )
    .await;
    assert_eq!(
        &created["service"]["serviceRegistries"],
        &json!([accepted.clone()]),
        "setup: expected the create to store the registry, got {created}"
    );

    let rows: Vec<(&str, Value, String)> = vec![
        (
            "an A record attached to a bridge service",
            json!([{
                "registryArn": a,
                "containerName": CONTAINER,
                "containerPort": CONTAINER_PORT,
            }]),
            address_record_message("A", a),
        ),
        (
            "an SRV registry naming no port source",
            json!([{ "registryArn": srv }]),
            host_network_pair_message(srv),
        ),
        (
            "an SRV registry naming a container the revision does not declare",
            json!([{
                "registryArn": srv,
                "containerName": MISSING_CONTAINER,
                "containerPort": CONTAINER_PORT,
            }]),
            container_not_valid_message(MISSING_CONTAINER, srv),
        ),
    ];

    for (case, registries, expected) in &rows {
        let (code, message) = refusal(
            router,
            "UpdateService",
            json!({
                "cluster": CLUSTER,
                "service": "updateregsvc",
                "serviceRegistries": registries.clone(),
            }),
            case,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (INVALID_PARAMETER, expected.as_str()),
            "case {case:?}: expected the update to be refused with {INVALID_PARAMETER} \
             {expected:?}, got ({code:?}, {message:?})"
        );
    }

    // A refused update changed nothing: the service still holds the registry it was
    // created with.
    let described = ok(
        router,
        "DescribeServices",
        json!({ "cluster": CLUSTER, "services": ["updateregsvc"] }),
        "readback",
    )
    .await;
    let services = list(&described, "services", "readback");
    let found = services
        .first()
        .unwrap_or_else(|| panic!("readback: expected the service to be described"));
    assert_eq!(
        &found["serviceRegistries"],
        &json!([accepted]),
        "a refused update must change nothing, got {found}"
    );
}

#[tokio::test]
async fn create_task_set_holds_its_registries_to_the_same_rule() {
    let fixture = Fixture::build().await;
    let router = &fixture.router;
    let (srv, a) = (fixture.srv.as_str(), fixture.a.as_str());

    // A task set belongs to a service its own scheduler does not place, so the service
    // deploys through the EXTERNAL controller and carries no task definition itself.
    ok(
        router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": "tasksetregsvc",
            "deploymentController": { "type": "EXTERNAL" },
            "desiredCount": 1,
        }),
        "setup",
    )
    .await;

    let accepted = json!({
        "registryArn": srv,
        "containerName": CONTAINER,
        "containerPort": CONTAINER_PORT,
    });
    let rows: Vec<(&str, Value, Option<String>)> = vec![
        (
            "an SRV registry naming the pair",
            accepted.clone(),
            None,
        ),
        (
            "an A record on a bridge revision",
            json!({
                "registryArn": a,
                "containerName": CONTAINER,
                "containerPort": CONTAINER_PORT,
            }),
            Some(address_record_message("A", a)),
        ),
        (
            "an SRV registry naming no port source",
            json!({ "registryArn": srv }),
            Some(host_network_pair_message(srv)),
        ),
    ];

    for (index, (case, registry, refused_with)) in rows.iter().enumerate() {
        let request = json!({
            "cluster": CLUSTER,
            "service": "tasksetregsvc",
            "taskDefinition": "registryrules-bridge",
            "externalId": format!("taskset{index}"),
            "launchType": "EC2",
            "serviceRegistries": [registry.clone()],
        });
        match refused_with.as_deref() {
            Some(expected) => {
                let (code, message) =
                    refusal(router, "CreateTaskSet", request, case).await;
                assert_eq!(
                    (code.as_str(), message.as_str()),
                    (INVALID_PARAMETER, expected),
                    "case {case:?}: expected the task set to be refused with \
                     {INVALID_PARAMETER} {expected:?}, got ({code:?}, {message:?})"
                );
            }
            None => {
                let (status, body) = answered(router, "CreateTaskSet", request).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the task set to be created, got {status} with \
                     {body}"
                );
                assert_eq!(
                    &body["taskSet"]["serviceRegistries"],
                    &json!([registry.clone()]),
                    "case {case:?}: expected the task set to store the registry it was \
                     given, got {body}"
                );
            }
        }
    }

    // Only the accepted row created a task set: the service holds exactly one.
    let described = ok(
        router,
        "DescribeTaskSets",
        json!({ "cluster": CLUSTER, "service": "tasksetregsvc" }),
        "readback",
    )
    .await;
    let task_sets = list(&described, "taskSets", "readback");
    assert_eq!(
        task_sets.len(),
        1,
        "a refused CreateTaskSet stores nothing: expected one task set, got {described}"
    );
    assert_eq!(
        &task_sets[0]["serviceRegistries"],
        &json!([accepted]),
        "the stored task set carries the registry the accepted row named, got {described}"
    );
}

/// The published bound on how many registries one service may carry, from this crate's
/// statement of the rule `ServiceRegistry`'s own doc gives ("Each service may be associated
/// with one service registry. Multiple service registries for each service are not
/// supported.").
const MAX_SERVICE_REGISTRIES: usize =
    service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::MAX_SERVICE_REGISTRIES;

/// The sentence a list longer than the bound is refused with.
const MULTIPLE_REGISTRIES_MESSAGE: &str =
    service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::MULTIPLE_SERVICE_REGISTRIES_MESSAGE;

#[tokio::test]
async fn a_service_may_carry_at_most_the_published_number_of_registries() {
    let fixture = Fixture::build().await;
    let router = &fixture.router;

    // One entry the rule accepts, which every row below is built out of.
    let entry = json!({
        "registryArn": fixture.srv.as_str(),
        "containerName": CONTAINER,
        "containerPort": CONTAINER_PORT,
    });

    // The rows walk the COUNT across the bound: the bound itself, one past it, and well
    // past it — so the refusal is asserted at the boundary and not only at a big list.
    let counts: [usize; 4] = [
        MAX_SERVICE_REGISTRIES,
        MAX_SERVICE_REGISTRIES + 1,
        MAX_SERVICE_REGISTRIES + 2,
        MAX_SERVICE_REGISTRIES + 8,
    ];

    // A service the update and the task-set rows act on, created inside the bound.
    ok(
        router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": "countregsvc",
            "taskDefinition": "registryrules-bridge",
            "desiredCount": 1,
            "launchType": "EC2",
            "serviceRegistries": [entry.clone()],
        }),
        "setup",
    )
    .await;
    ok(
        router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": "counttasksetsvc",
            "deploymentController": { "type": "EXTERNAL" },
            "desiredCount": 1,
        }),
        "setup",
    )
    .await;

    for (index, count) in counts.into_iter().enumerate() {
        let registries: Vec<Value> = (0..count).map(|_| entry.clone()).collect();
        let within = count <= MAX_SERVICE_REGISTRIES;
        let requests: [(&str, Value); 3] = [
            (
                "CreateService",
                json!({
                    "cluster": CLUSTER,
                    "serviceName": format!("countsvc{index}"),
                    "taskDefinition": "registryrules-bridge",
                    "desiredCount": 1,
                    "launchType": "EC2",
                    "serviceRegistries": registries.clone(),
                }),
            ),
            (
                "UpdateService",
                json!({
                    "cluster": CLUSTER,
                    "service": "countregsvc",
                    "serviceRegistries": registries.clone(),
                }),
            ),
            (
                "CreateTaskSet",
                json!({
                    "cluster": CLUSTER,
                    "service": "counttasksetsvc",
                    "taskDefinition": "registryrules-bridge",
                    "externalId": format!("countset{index}"),
                    "launchType": "EC2",
                    "serviceRegistries": registries.clone(),
                }),
            ),
        ];

        for (operation, request) in requests {
            let case = format!("{count} registries on {operation}");
            if within {
                let (status, body) = answered(router, operation, request).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: a list inside the bound is answered, got {status} with \
                     {body}"
                );
            } else {
                let (code, message) = refusal(router, operation, request, &case).await;
                assert_eq!(
                    (code.as_str(), message.as_str()),
                    (INVALID_PARAMETER, MULTIPLE_REGISTRIES_MESSAGE),
                    "case {case:?}: expected {INVALID_PARAMETER} \
                     {MULTIPLE_REGISTRIES_MESSAGE:?}, got ({code:?}, {message:?})"
                );
            }
        }
    }
}
