// SPDX-License-Identifier: CC0-1.0

//! The requests Amazon ECS REFUSES, and the modelled error each refusal carries, driven
//! end to end over the shared wire harness in [`crate::ecs_wire`].
//!
//! Three rules, one per suite below:
//!
//! * A registration identifies its machine with the instance identity document AND the
//!   signature of that document the same instance metadata publishes; a document
//!   presented without that signature vouches for nothing, so it is refused and no
//!   container instance is registered for it.
//! * A Fargate task definition's `cpu` and `memory` are legal only TOGETHER: each task
//!   size Fargate offers pairs one `cpu` with the `memory` values it supports (the
//!   `RegisterTaskDefinition$memory` doc's own list), so a pair outside that catalog is
//!   refused — and the catalog is what a legal pair is checked against, not one example
//!   of it.
//! * A request this service refuses before any handler runs is answered with an error the
//!   service MODELS: its model declares no validation error shape, so a value outside a
//!   declared constraint is an `InvalidParameterException`.

use serde_json::{json, Value};

use crate::ecs_wire::{
    create_cluster, identity_document, identity_document_signature, list, ok, refusal, service,
    string, ACCOUNT, SIGNED_REGION,
};

/// The cluster every request below names.
const CLUSTER: &str = "demo";

/// The modelled error a refused ECS request carries: "The specified parameter isn't
/// valid. Review the available parameters for the API request."
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The modelled error a refused registration of a task definition carries: "These errors
/// are usually caused by a client action."
const CLIENT_EXCEPTION: &str = "ClientException";

/// What ECS says when the identity document and the signature presented with it are not
/// the pair that vouches for one EC2 instance.
const IDENTITY_NOT_VALID: &str =
    "The identity document and identity document signature were not valid.";

/// A minimal, valid container definition, so every refusal below is reached by the member
/// under test rather than by an unrunnable container.
fn container() -> Value {
    json!({ "name": "app", "image": "public.ecr.aws/nginx/nginx:latest", "memory": 128 })
}

/// The refusal a Fargate `cpu`/`memory` pair outside the catalog carries, for the values
/// as the request stated them.
fn no_configuration(cpu: &str, memory: &str) -> String {
    format!(
        "No Fargate configuration exists for given values: {cpu} CPU, {memory} memory. \
         See the Amazon ECS documentation for the valid values."
    )
}

#[tokio::test]
async fn a_registration_whose_identity_is_not_vouched_for_is_refused() {
    let router = service();
    create_cluster(&router, CLUSTER, "identity").await;

    let instance_id = "i-0abcdef1234567890";
    let document = identity_document(instance_id);
    let signature = identity_document_signature();

    // Rows: the case, the identity document presented, and the signature presented beside
    // it. They vary along what the rule generalises over: whether a document was presented
    // at all, whether a signature came with it, whether that signature is of a signature's
    // form and length, and whether the document describes a machine of THIS account in
    // THIS Region. The recorded divergence (a registration that vouches for nothing) is
    // the first row.
    let rows: [(&str, Value, Option<String>); 10] = [
        ("no identity document at all", Value::Null, None),
        (
            "a signature with no document",
            Value::Null,
            Some(signature.clone()),
        ),
        ("no signature at all", json!(document), None),
        ("an empty signature", json!(document), Some(String::new())),
        (
            "a blank signature",
            json!(document),
            Some("   ".to_string()),
        ),
        (
            "a signature that is a sentence",
            json!(document),
            Some("not a signature!".to_string()),
        ),
        (
            "a signature too short to be one",
            json!(document),
            Some("c2lnbmF0dXJl".to_string()),
        ),
        (
            "a signature carrying characters base64 never does",
            json!(document),
            Some(format!("{}*#@", signature.clone())),
        ),
        (
            "a document naming another account",
            json!(json!({
                "accountId": "123456789012",
                "instanceId": instance_id,
                "region": SIGNED_REGION,
            })
            .to_string()),
            Some(signature.clone()),
        ),
        (
            "a document naming another Region",
            json!(json!({
                "accountId": ACCOUNT,
                "instanceId": instance_id,
                "region": "eu-west-1",
            })
            .to_string()),
            Some(signature.clone()),
        ),
    ];

    for (case, presented, signature) in rows {
        let mut request = json!({ "cluster": CLUSTER });
        if !presented.is_null() {
            request["instanceIdentityDocument"] = presented;
        }
        if let Some(signature) = signature {
            request["instanceIdentityDocumentSignature"] = json!(signature);
        }
        let (code, message) = refusal(&router, "RegisterContainerInstance", request, case).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: expected an {INVALID_PARAMETER}, got {code:?} with {message:?}"
        );
        assert_eq!(
            message, IDENTITY_NOT_VALID,
            "case {case:?}: expected {IDENTITY_NOT_VALID:?}, got {message:?}"
        );

        // Nothing was registered for it: the cluster still holds no container instance,
        // so a refused registration cannot be counted against its capacity.
        let listed = ok(
            &router,
            "ListContainerInstances",
            json!({ "cluster": CLUSTER }),
            case,
        )
        .await;
        assert!(
            list(&listed, "containerInstanceArns", case).is_empty(),
            "case {case:?}: a refused registration must register nothing, got {listed}"
        );
    }

    // The document is still read BEFORE the signature: a document that leaves a required
    // member blank is reported on that member, whether or not a signature came with it.
    let (code, message) = refusal(
        &router,
        "RegisterContainerInstance",
        json!({
            "cluster": CLUSTER,
            "instanceIdentityDocument": json!({ "instanceId": "i-0123456789abcdef0" }).to_string(),
        }),
        "no accountId and no signature",
    )
    .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        (INVALID_PARAMETER, "accountId cannot be blank."),
        "case \"no accountId and no signature\": expected the blank member to be named, got \
         {code:?} with {message:?}"
    );

    // The pair REGISTERS: the instance reads back through `DescribeContainerInstances`
    // carrying the EC2 instance id the document named.
    let registered = ok(
        &router,
        "RegisterContainerInstance",
        json!({
            "cluster": CLUSTER,
            "instanceIdentityDocument": document,
            "instanceIdentityDocumentSignature": signature,
        }),
        "document and signature",
    )
    .await;
    let arn = string(
        &registered["containerInstance"],
        "containerInstanceArn",
        "document and signature",
    );
    let described = ok(
        &router,
        "DescribeContainerInstances",
        json!({ "cluster": CLUSTER, "containerInstances": [arn.clone()] }),
        "document and signature",
    )
    .await;
    let read = &list(&described, "containerInstances", "document and signature")[0];
    assert_eq!(
        string(read, "ec2InstanceId", "document and signature"),
        instance_id,
        "case \"document and signature\": the stored instance carries the document's \
         instanceId, got {read}"
    );
}

#[tokio::test]
async fn a_fargate_task_definition_takes_only_a_documented_cpu_memory_pair() {
    let router = service();

    // Rows: the case, `cpu`, `memory`, and whether Fargate has a configuration for the
    // pair. The catalog is the `RegisterTaskDefinition$memory` doc's list of legal pairs,
    // so the table walks EVERY size it names at both published bounds and one step past
    // them, in CPU units and MiB and in the vCPU/GB string forms the members document.
    // The recorded divergence (256 CPU with 4096 MiB) is one row.
    let rows: [(&str, &str, &str, bool); 27] = [
        ("256 at its lowest memory", "256", "512", true),
        ("256 at its highest memory", "256", "2048", true),
        ("256 with a memory it skips", "256", "1536", false),
        ("256 past its highest memory", "256", "4096", false),
        ("512 at its lowest memory", "512", "1024", true),
        ("512 at its highest memory", "512", "4096", true),
        ("512 below its lowest memory", "512", "512", false),
        ("512 past its highest memory", "512", "5120", false),
        ("1024 at its lowest memory", "1024", "2048", true),
        ("1024 at its highest memory", "1024", "8192", true),
        ("1024 past its highest memory", "1024", "9216", false),
        ("2048 at its lowest memory", "2048", "4096", true),
        ("2048 at its highest memory", "2048", "16384", true),
        ("2048 off its 1 GB increment", "2048", "4608", false),
        ("4096 at its lowest memory", "4096", "8192", true),
        ("4096 at its highest memory", "4096", "30720", true),
        ("4096 past its highest memory", "4096", "31744", false),
        ("8192 at its lowest memory", "8192", "16384", true),
        ("8192 on its 4 GB increment", "8192", "20480", true),
        ("8192 at its highest memory", "8192", "61440", true),
        ("8192 off its 4 GB increment", "8192", "18432", false),
        ("16384 at its lowest memory", "16384", "32768", true),
        ("16384 at its highest memory", "16384", "122880", true),
        ("16384 off its 8 GB increment", "16384", "36864", false),
        ("a cpu size Fargate does not offer", "128", "512", false),
        ("the same pair in vCPU and GB", "1 vCPU", "2GB", true),
        ("a cpu that names no number", "a lot", "512", false),
    ];

    for (index, (case, cpu, memory, supported)) in rows.into_iter().enumerate() {
        let family = format!("fargate{index}");
        let request = json!({
            "family": family,
            "requiresCompatibilities": ["FARGATE"],
            "networkMode": "awsvpc",
            "cpu": cpu,
            "memory": memory,
            "containerDefinitions": [container()],
        });
        if supported {
            // A legal pair registers, and the revision reads back through
            // `DescribeTaskDefinition` carrying the size it was registered with.
            let registered = ok(&router, "RegisterTaskDefinition", request, case).await;
            let revision = &registered["taskDefinition"];
            assert_eq!(
                (
                    string(revision, "cpu", case),
                    string(revision, "memory", case)
                ),
                (cpu.to_string(), memory.to_string()),
                "case {case:?}: expected the registered size back, got {revision}"
            );
            let described = ok(
                &router,
                "DescribeTaskDefinition",
                json!({ "taskDefinition": family }),
                case,
            )
            .await;
            let read = &described["taskDefinition"];
            assert_eq!(
                (string(read, "cpu", case), string(read, "memory", case)),
                (cpu.to_string(), memory.to_string()),
                "case {case:?}: the stored revision keeps its size, got {read}"
            );
            continue;
        }
        let (code, message) = refusal(&router, "RegisterTaskDefinition", request, case).await;
        assert_eq!(
            code, CLIENT_EXCEPTION,
            "case {case:?}: expected a {CLIENT_EXCEPTION}, got {code:?} with {message:?}"
        );
        assert!(
            message.starts_with("No Fargate configuration exists for given values:"),
            "case {case:?}: expected the no-configuration refusal, got {message:?}"
        );
        // Nothing was registered for it: the family holds no revision.
        let listed = ok(
            &router,
            "ListTaskDefinitions",
            json!({ "familyPrefix": family }),
            case,
        )
        .await;
        assert!(
            list(&listed, "taskDefinitionArns", case).is_empty(),
            "case {case:?}: a refused registration must store no revision, got {listed}"
        );
    }

    // The refusal states the values the request asked for, in the units they name.
    for (case, cpu, memory, stated_cpu, stated_memory) in [
        ("cpu units and MiB", "256", "4096", "256", "4096"),
        ("vCPU and GB", "1 vCPU", "1GB", "1024", "1024"),
        ("a cpu that names no number", "a lot", "4096", "a lot", "4096"),
        ("a negative cpu", "-256", "4096", "-256", "4096"),
        ("a cpu that is not a finite number", "inf vCPU", "4096", "inf vCPU", "4096"),
        (
            "a memory no task size reaches",
            "256",
            "99999GB",
            "256",
            "99999GB",
        ),
    ] {
        let (_, message) = refusal(
            &router,
            "RegisterTaskDefinition",
            json!({
                "family": format!("stated-{}", case.replace(' ', "-")),
                "requiresCompatibilities": ["FARGATE"],
                "networkMode": "awsvpc",
                "cpu": cpu,
                "memory": memory,
                "containerDefinitions": [container()],
            }),
            case,
        )
        .await;
        assert_eq!(
            message,
            no_configuration(stated_cpu, stated_memory),
            "case {case:?}: expected {:?}, got {message:?}",
            no_configuration(stated_cpu, stated_memory)
        );
    }

    // The catalog is FARGATE's: a revision that asks for no Fargate compatibility takes
    // any size, because nothing pairs its `cpu` with its `memory`.
    let ec2 = ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": "ec2-any-size",
            "requiresCompatibilities": ["EC2"],
            "cpu": "256",
            "memory": "4096",
            "containerDefinitions": [container()],
        }),
        "an EC2 revision with the refused pair",
    )
    .await;
    assert_eq!(
        string(
            &ec2["taskDefinition"],
            "memory",
            "an EC2 revision with the refused pair"
        ),
        "4096",
        "case \"an EC2 revision with the refused pair\": expected the revision to register, \
         got {ec2}"
    );

    // A Fargate revision still states the ONE requirement it fails first: the network
    // mode it must declare, then each task-level member it must define.
    for (case, request, expected) in [
        (
            "a Fargate revision in bridge network mode",
            json!({
                "family": "fargate-bridge",
                "requiresCompatibilities": ["FARGATE"],
                "networkMode": "bridge",
                "cpu": "256",
                "memory": "512",
                "containerDefinitions": [container()],
            }),
            "Fargate only supports network mode \u{2018}awsvpc\u{2019}.",
        ),
        (
            "a Fargate revision with no task-level cpu",
            json!({
                "family": "fargate-no-cpu",
                "requiresCompatibilities": ["FARGATE"],
                "networkMode": "awsvpc",
                "memory": "512",
                "containerDefinitions": [container()],
            }),
            "Fargate requires that 'cpu' be defined at the task level.",
        ),
        (
            "a Fargate revision with no task-level memory",
            json!({
                "family": "fargate-no-memory",
                "requiresCompatibilities": ["FARGATE"],
                "networkMode": "awsvpc",
                "cpu": "256",
                "containerDefinitions": [container()],
            }),
            "Fargate requires that 'memory' be defined at the task level.",
        ),
    ] {
        let (code, message) = refusal(&router, "RegisterTaskDefinition", request, case).await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (CLIENT_EXCEPTION, expected),
            "case {case:?}: expected {expected:?}, got {code:?} with {message:?}"
        );
    }
}

/// One container definition carrying one port mapping, built from the ports the row
/// declares: `None` leaves the member out, which is the "left blank" form.
fn container_with_ports(container_port: Option<i64>, host_port: Option<i64>) -> Value {
    let mut mapping = json!({});
    if let Some(container_port) = container_port {
        mapping["containerPort"] = json!(container_port);
    }
    if let Some(host_port) = host_port {
        mapping["hostPort"] = json!(host_port);
    }
    let mut definition = container();
    definition["portMappings"] = json!([mapping]);
    definition
}

/// One row of the port-mapping table: the case name, the network mode the registration
/// declares, the container port and the host port the one mapping carries (`None` leaves
/// the member out), and whether the registration is accepted.
type PortRow = (&'static str, &'static str, Option<i64>, Option<i64>, bool);

#[tokio::test]
async fn a_static_network_mode_takes_only_matching_host_and_container_ports() {
    let router = service();

    // `PortMapping$hostPort`: "If you use containers in a task with the awsvpc or host
    // network mode, the hostPort can either be left blank or set to the same value as the
    // containerPort" — those modes reserve the SAME port on the host, so any other host
    // port asks for a mapping they cannot make, while `bridge` binds a host port of its
    // own choosing and takes whichever one the caller asks for.
    //
    // Rows: the network mode, the container port, the host port, and whether the
    // registration is accepted. The recorded divergence (an `awsvpc` revision declaring a
    // host port of its own) is one row. A mapping that declares a host port and NO
    // container port is left out: which of the two Amazon ECS reports first for it is not
    // something this issue's recordings show.
    let rows: [PortRow; 10] = [
        ("awsvpc with a blank host port", "awsvpc", Some(8080), None, true),
        (
            "awsvpc with the matching host port",
            "awsvpc",
            Some(8080),
            Some(8080),
            true,
        ),
        (
            "awsvpc with another host port",
            "awsvpc",
            Some(8080),
            Some(80),
            false,
        ),
        (
            "awsvpc with a dynamic host port",
            "awsvpc",
            Some(8080),
            Some(0),
            false,
        ),
        ("host with a blank host port", "host", Some(443), None, true),
        (
            "host with the matching host port",
            "host",
            Some(443),
            Some(443),
            true,
        ),
        (
            "host with another host port",
            "host",
            Some(443),
            Some(8443),
            false,
        ),
        ("bridge with a blank host port", "bridge", Some(80), None, true),
        (
            "bridge with a host port of its own",
            "bridge",
            Some(80),
            Some(32768),
            true,
        ),
        (
            "bridge with a dynamic host port",
            "bridge",
            Some(80),
            Some(0),
            true,
        ),
    ];

    for (index, (case, network_mode, container_port, host_port, accepted)) in
        rows.into_iter().enumerate()
    {
        let family = format!("ports{index}");
        let request = json!({
            "family": family,
            "networkMode": network_mode,
            "containerDefinitions": [container_with_ports(container_port, host_port)],
        });
        if accepted {
            let registered = ok(&router, "RegisterTaskDefinition", request, case).await;
            let stored = &registered["taskDefinition"]["containerDefinitions"][0]
                ["portMappings"][0];
            assert_eq!(
                stored["containerPort"].as_i64(),
                container_port,
                "case {case:?}: the stored mapping keeps its container port, got {stored}"
            );
            continue;
        }
        let (code, message) = refusal(&router, "RegisterTaskDefinition", request, case).await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (
                CLIENT_EXCEPTION,
                format!(
                    "When networkMode={network_mode}, the host ports and container ports \
                     in port mappings must match."
                )
                .as_str()
            ),
            "case {case:?}: expected the port-mapping refusal, got {code:?} with {message:?}"
        );
        let listed = ok(
            &router,
            "ListTaskDefinitions",
            json!({ "familyPrefix": family }),
            case,
        )
        .await;
        assert!(
            list(&listed, "taskDefinitionArns", case).is_empty(),
            "case {case:?}: a refused registration must store no revision, got {listed}"
        );
    }
}

#[tokio::test]
async fn a_value_outside_a_declared_constraint_is_a_modelled_refusal() {
    let router = service();
    create_cluster(&router, CLUSTER, "constraints").await;

    // A cluster read that asks for a field a cluster cannot be described with is the one
    // such refusal ECS states in its own sentence.
    for (case, include) in [
        ("an unknown include field", json!(["NOT_A_VALID_INCLUDE_FIELD"])),
        ("an unknown field beside a known one", json!(["TAGS", "NOPE"])),
        ("a known field in the wrong case", json!(["tags"])),
    ] {
        let (code, message) = refusal(
            &router,
            "DescribeClusters",
            json!({ "clusters": [CLUSTER], "include": include }),
            case,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (INVALID_PARAMETER, "The cluster identifier is invalid"),
            "case {case:?}: expected the modelled cluster refusal, got {code:?} with \
             {message:?}"
        );
    }

    // Every other refusal the framework raises carries the same MODELLED code: this
    // service declares no validation error shape, so a client's dispatch on
    // `InvalidParameterException` catches them all.
    for (case, operation, request) in [
        (
            "an unknown include field on a task read",
            "DescribeTasks",
            json!({ "cluster": CLUSTER, "tasks": ["a".repeat(32)], "include": ["NOPE"] }),
        ),
        (
            "an unknown include field on a container-instance read",
            "DescribeContainerInstances",
            json!({
                "cluster": CLUSTER,
                "containerInstances": ["a".repeat(32)],
                "include": ["NOPE"],
            }),
        ),
        (
            "a required member left out",
            "DescribeTasks",
            json!({ "cluster": CLUSTER }),
        ),
        (
            "an unknown launch type",
            "RunTask",
            json!({ "cluster": CLUSTER, "taskDefinition": "web", "launchType": "NOPE" }),
        ),
    ] {
        let (code, message) = refusal(&router, operation, request, case).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: expected an {INVALID_PARAMETER}, got {code:?} with {message:?}"
        );
        assert!(
            !message.is_empty(),
            "case {case:?}: a refusal states what it refused, got {message:?}"
        );
    }

    // A field a cluster CAN be described with is served — every one of them — so the
    // refusal above is about the value and not about the member.
    let described = ok(
        &router,
        "DescribeClusters",
        json!({
            "clusters": [CLUSTER],
            "include": ["ATTACHMENTS", "CONFIGURATIONS", "SETTINGS", "STATISTICS", "TAGS"],
        }),
        "every known include field",
    )
    .await;
    assert_eq!(
        list(&described, "clusters", "every known include field").len(),
        1,
        "case \"every known include field\": expected the cluster described, got {described}"
    );
}
