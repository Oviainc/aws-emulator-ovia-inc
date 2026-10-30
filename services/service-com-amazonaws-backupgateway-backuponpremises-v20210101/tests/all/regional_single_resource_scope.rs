// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the REGION scope of every operation of this service that resolves
//! a SINGLE ARN: the gateway reads and writes, the hypervisor reads and writes, the
//! virtual-machine read and the three tagging operations.
//!
//! Every case drives real `POST /` awsJson1_0 requests through the `MultiServiceRouter` —
//! the harness of `gateway_lifecycle` — so what is asserted is the wire behaviour, and the
//! region a request carries is the one its SigV4 credential scope names, exactly as a
//! client addressing a regional endpoint would.
//!
//! The rule under test: every resource of this service is regional (its region is a segment
//! of the ARN minted for it), so a request resolves only the resources of the Region it was
//! itself addressed to — an ARN of another Region names no resource that endpoint holds, no
//! more than one of another account would, so it is refused exactly as an ARN that names no
//! record at all is, and nothing is read, written or removed. The sibling
//! `regional_listing_scope` pins the same rule for the three listings, and the two sides
//! cannot disagree: a record one Region's listing omits is a record its reads, its writes
//! and its deletes do not resolve either.
//!
//! The tables below span the class the rule generalises over: EVERY operation that resolves
//! one ARN, over all three resource kinds a tagging operation can name, crossed with
//! Regions differing in whether they hold records of their own, in continent prefix, in
//! sharing the owning Region's prefix, and in being the Region an unsigned request falls
//! back to. The host a configuration test resolves a hypervisor by, which is not an ARN at
//! all, is a row of its own.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{service_state, VirtualMachineInventory};

use crate::gateway_lifecycle::{
    build_engine, build_router, call_in_region, create_body, issued_activation_key_for, Reply,
    REGION,
};
use crate::gateway_server_association::message_of;

/// The region whose records every cross-region call below names: the one the sibling suites
/// sign for.
const OWNER: &str = REGION;

/// A second region the fixture seeds records in, so a refusal is never about a region that
/// holds nothing at all.
const NEIGHBOUR: &str = "eu-central-1";

/// The `Host` each region's hypervisor is configured for — inside the modelled
/// `@length(min: 3, max: 128)` (`smithy_query(query="shape",shape="Host")`). One host per
/// region, so a host resolves a configuration in exactly one of them.
const OWNER_HOST: &str = "198.51.100.73";
const NEIGHBOUR_HOST: &str = "198.51.100.74";

/// The client tag every seeded record carries, so an untagging call has a key to remove.
const TAG_KEY: &str = "env";

/// Every region a call below is signed for. Region names are only ARN segments to this
/// service, so what matters is that they differ; these are spelled as AWS spells them.
const REGIONS: [&str; 5] = [OWNER, NEIGHBOUR, "ap-southeast-2", "us-west-1", "us-east-1"];

/// The regions that did NOT mint the records under test, and why each is a row: the rule
/// generalises over the region a request is addressed to, so the rows differ in what that
/// region holds and in how its name stands to the owning region's.
const OTHER_REGIONS: [(&str, &str); 4] = [
    (
        "another continent's region, holding records of its own",
        NEIGHBOUR,
    ),
    ("a region holding no record at all", "ap-southeast-2"),
    (
        "the region beside the owner's, sharing its prefix",
        "us-west-1",
    ),
    (
        "the region an unsigned request falls back to",
        "us-east-1",
    ),
];

/// The resources of one region, as that region's endpoint minted them.
#[derive(Debug, Clone)]
struct Arns {
    gateway: String,
    hypervisor: String,
    machine: String,
    /// The host the hypervisor above is configured for — what a configuration test names
    /// instead of an ARN.
    host: String,
}

/// One operation under test, and the request body that names the resources of `arns`.
///
/// Every other modelled constraint on the body is satisfied, so the only thing left for a
/// handler to decide is whether it resolves the ARN at all.
struct Case {
    /// The resource kind the ARN member names, for the assertion text.
    resource: &'static str,
    operation: &'static str,
    body: fn(&Arns) -> Value,
}

/// An `IamRoleArn` of this account, inside the modelled pattern
/// `^arn:(aws|aws-cn|aws-us-gov):iam::([0-9]+):role/(\S+)$` with
/// `@length(min: 20, max: 2048)` (`smithy_query(query="shape",shape="IamRoleArn")`).
fn role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/backup-gateway-property-mappings",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// One `VmwareToAwsTagMapping`, every member inside its modelled bounds
/// (`smithy_query(query="shape",shape="VmwareToAwsTagMapping")`).
fn property_mappings() -> Value {
    json!([{
        "VmwareCategory": "Department",
        "VmwareTagName": "Finance",
        "AwsTagKey": "Department",
        "AwsTagValue": "Finance",
    }])
}

/// One bandwidth rate-limit interval, every member inside its modelled bounds:
/// `StartHourOfDay`/`EndHourOfDay` `@range(min: 0, max: 23)`,
/// `StartMinuteOfHour`/`EndMinuteOfHour` 0..59, `DaysOfWeek` ordinals 0..6 and
/// `AverageUploadRateLimitInBitsPerSec` 51200..8000000000000
/// (`smithy_query(query="shape",shape="BandwidthRateLimitInterval")`).
fn rate_limit_intervals() -> Value {
    json!([{
        "StartHourOfDay": 1,
        "StartMinuteOfHour": 0,
        "EndHourOfDay": 5,
        "EndMinuteOfHour": 59,
        "DaysOfWeek": [1, 3],
        "AverageUploadRateLimitInBitsPerSec": 51_200,
    }])
}

/// Every operation of this service that resolves a single ARN.
///
/// `TagResource` and `UntagResource` spell the member `ResourceARN` and
/// `ListTagsForResource` spells it `ResourceArn`, as this service's model does; the three
/// take any resource of this service in that one member, so each appears once per resource
/// kind.
fn cases() -> Vec<Case> {
    vec![
        Case {
            resource: "gateway",
            operation: "GetGateway",
            body: |arns| json!({ "GatewayArn": arns.gateway }),
        },
        Case {
            resource: "gateway",
            operation: "DeleteGateway",
            body: |arns| json!({ "GatewayArn": arns.gateway }),
        },
        Case {
            resource: "gateway",
            operation: "UpdateGatewayInformation",
            body: |arns| {
                json!({ "GatewayArn": arns.gateway, "GatewayDisplayName": "renamed-elsewhere" })
            },
        },
        Case {
            resource: "gateway",
            operation: "UpdateGatewaySoftwareNow",
            body: |arns| json!({ "GatewayArn": arns.gateway }),
        },
        Case {
            resource: "gateway",
            operation: "PutMaintenanceStartTime",
            body: |arns| {
                json!({ "GatewayArn": arns.gateway, "HourOfDay": 3, "MinuteOfHour": 30 })
            },
        },
        Case {
            resource: "gateway",
            operation: "PutBandwidthRateLimitSchedule",
            body: |arns| {
                json!({
                    "GatewayArn": arns.gateway,
                    "BandwidthRateLimitIntervals": rate_limit_intervals(),
                })
            },
        },
        Case {
            resource: "gateway",
            operation: "GetBandwidthRateLimitSchedule",
            body: |arns| json!({ "GatewayArn": arns.gateway }),
        },
        Case {
            resource: "gateway",
            operation: "TestHypervisorConfiguration",
            body: |arns| json!({ "GatewayArn": arns.gateway, "Host": arns.host }),
        },
        Case {
            resource: "gateway",
            operation: "AssociateGatewayToServer",
            body: |arns| {
                json!({ "GatewayArn": arns.gateway, "ServerArn": arns.hypervisor })
            },
        },
        Case {
            resource: "gateway",
            operation: "DisassociateGatewayFromServer",
            body: |arns| json!({ "GatewayArn": arns.gateway }),
        },
        Case {
            resource: "gateway",
            operation: "TagResource",
            body: |arns| {
                json!({
                    "ResourceARN": arns.gateway,
                    "Tags": [{ "Key": TAG_KEY, "Value": "written-elsewhere" }],
                })
            },
        },
        Case {
            resource: "gateway",
            operation: "UntagResource",
            body: |arns| json!({ "ResourceARN": arns.gateway, "TagKeys": [TAG_KEY] }),
        },
        Case {
            resource: "gateway",
            operation: "ListTagsForResource",
            body: |arns| json!({ "ResourceArn": arns.gateway }),
        },
        Case {
            resource: "hypervisor",
            operation: "GetHypervisor",
            body: |arns| json!({ "HypervisorArn": arns.hypervisor }),
        },
        Case {
            resource: "hypervisor",
            operation: "UpdateHypervisor",
            body: |arns| json!({ "HypervisorArn": arns.hypervisor, "Name": "renamed" }),
        },
        Case {
            resource: "hypervisor",
            operation: "DeleteHypervisor",
            body: |arns| json!({ "HypervisorArn": arns.hypervisor }),
        },
        Case {
            resource: "hypervisor",
            operation: "StartVirtualMachinesMetadataSync",
            body: |arns| json!({ "HypervisorArn": arns.hypervisor }),
        },
        Case {
            resource: "hypervisor",
            operation: "PutHypervisorPropertyMappings",
            body: |arns| {
                json!({
                    "HypervisorArn": arns.hypervisor,
                    "IamRoleArn": role_arn(),
                    "VmwareToAwsTagMappings": property_mappings(),
                })
            },
        },
        Case {
            resource: "hypervisor",
            operation: "GetHypervisorPropertyMappings",
            body: |arns| json!({ "HypervisorArn": arns.hypervisor }),
        },
        Case {
            resource: "hypervisor",
            operation: "TagResource",
            body: |arns| {
                json!({
                    "ResourceARN": arns.hypervisor,
                    "Tags": [{ "Key": TAG_KEY, "Value": "written-elsewhere" }],
                })
            },
        },
        Case {
            resource: "hypervisor",
            operation: "UntagResource",
            body: |arns| json!({ "ResourceARN": arns.hypervisor, "TagKeys": [TAG_KEY] }),
        },
        Case {
            resource: "hypervisor",
            operation: "ListTagsForResource",
            body: |arns| json!({ "ResourceArn": arns.hypervisor }),
        },
        Case {
            resource: "virtual machine",
            operation: "GetVirtualMachine",
            body: |arns| json!({ "ResourceArn": arns.machine }),
        },
        Case {
            resource: "virtual machine",
            operation: "TagResource",
            body: |arns| {
                json!({
                    "ResourceARN": arns.machine,
                    "Tags": [{ "Key": TAG_KEY, "Value": "written-elsewhere" }],
                })
            },
        },
        Case {
            resource: "virtual machine",
            operation: "UntagResource",
            body: |arns| json!({ "ResourceARN": arns.machine, "TagKeys": [TAG_KEY] }),
        },
        Case {
            resource: "virtual machine",
            operation: "ListTagsForResource",
            body: |arns| json!({ "ResourceArn": arns.machine }),
        },
    ]
}

/// An ARN of this service in `region` naming `resource_type`/`id`, spelled out here rather
/// than taken from the crate so what is asserted does not depend on the code that mints
/// one: `arn:aws:backup-gateway:<region>:<account>:<type>/<id>`
/// (`smithy_query(query="shape",shape="GatewayArn")`).
fn arn_in(region: &str, resource_type: &str, id: &str) -> String {
    format!(
        "arn:aws:backup-gateway:{region}:{}:{resource_type}/{id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// ARNs of `region` that NOTHING was ever created under — the baseline every cross-region
/// answer is compared with: well formed enough to reach a handler, so the only thing left
/// for it to refuse is that the ARN names no record.
fn absent_arns(region: &str) -> Arns {
    Arns {
        gateway: arn_in(region, "gateway", "bgw-0000000000000911"),
        hypervisor: arn_in(region, "hypervisor", "hv-0000000000000911"),
        machine: arn_in(region, "vm", "vm-0000000000000911"),
        host: OWNER_HOST.to_string(),
    }
}

/// What one refusal said, reduced to the parts a client branches on — the same four the
/// sibling `missing_resource_refusal_is_one_answer` compares.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Refusal {
    status: u16,
    /// The awsJson error discriminator, namespace stripped.
    shape: Option<String>,
    /// The `ErrorCode` member of the modelled error shape, absent for a refusal that
    /// states none.
    error_code: Option<String>,
    message: String,
}

fn refusal_of(reply: &Reply) -> Refusal {
    Refusal {
        status: reply.status,
        shape: reply.code.clone(),
        error_code: reply.str_at("/ErrorCode").map(str::to_string),
        message: message_of(reply),
    }
}

/// The baseline refusal as it reads for the ARNs that were really asked for.
///
/// This service's sentence for an ARN it holds no record under names the ARN that was asked
/// for, so the comparison substitutes each absent ARN for the record's own rather than
/// expecting the baseline's; a refusal whose sentence names no ARN is left as it stands.
fn as_if_asked_for(baseline: &Refusal, absent: &Arns, asked: &Arns) -> Refusal {
    let mut message = baseline.message.clone();
    for (from, to) in [
        (&absent.gateway, &asked.gateway),
        (&absent.hypervisor, &asked.hypervisor),
        (&absent.machine, &asked.machine),
    ] {
        message = message.replace(from.as_str(), to.as_str());
    }
    Refusal {
        message,
        ..baseline.clone()
    }
}

/// One store, holding a full set of records in `OWNER` and another in `NEIGHBOUR`.
struct Fixture {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
    owner: Arns,
    neighbour: Arns,
}

/// One call that must be SERVED, and its reply.
async fn served(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> Reply {
    let reply = call_in_region(router, region, operation, body).await;
    assert_eq!(
        reply.status, 200,
        "seeding {region}: {operation} must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply
}

/// Everything one region holds in a fixture: a gateway, a hypervisor it is associated with,
/// the property mappings a metadata sync needs, one virtual machine of that hypervisor's
/// inventory, and a client tag on each of the three.
async fn seed_region(
    engine: &Arc<engine::InMemoryEngine>,
    router: &MultiServiceRouter,
    region: &str,
    host: &str,
) -> Arns {
    let activation_key = issued_activation_key_for(router, region).await;
    let created = served(
        router,
        region,
        "CreateGateway",
        &create_body(&format!("gw-{region}"), &activation_key, &BTreeMap::new()),
    )
    .await;
    let gateway = created.gateway_arn().to_string();

    let imported = served(
        router,
        region,
        "ImportHypervisorConfiguration",
        &json!({ "Name": format!("hv-{region}"), "Host": host }),
    )
    .await;
    let hypervisor = imported
        .str_at("/HypervisorArn")
        .unwrap_or_else(|| panic!("no HypervisorArn in {:?}", imported.body))
        .to_string();

    served(
        router,
        region,
        "AssociateGatewayToServer",
        &json!({ "GatewayArn": gateway, "ServerArn": hypervisor }),
    )
    .await;
    served(
        router,
        region,
        "PutHypervisorPropertyMappings",
        &json!({
            "HypervisorArn": hypervisor,
            "IamRoleArn": role_arn(),
            "VmwareToAwsTagMappings": property_mappings(),
        }),
    )
    .await;

    // A virtual machine is not minted by any operation of this model: it reaches the
    // service as part of the inventory of the hypervisor it runs on — in that hypervisor's
    // own region — which is the channel `record_virtual_machine` stands for.
    let machine = {
        let state = service_state(&**engine).expect("the in-memory engine must expose this state");
        let mut store = state.lock();
        store
            .record_virtual_machine(
                region,
                &hypervisor,
                VirtualMachineInventory {
                    name: format!("vm-{region}"),
                    host_name: format!("vm-{region}"),
                    path: "/Datacenter/vm/0".to_string(),
                    last_backup_date: None,
                    vmware_tags: Vec::new(),
                },
                SystemTime::now(),
            )
            .unwrap_or_else(|| {
                panic!("the store must record a machine on the hypervisor imported in {region}")
            })
            .arn
    };

    for arn in [&gateway, &hypervisor, &machine] {
        served(
            router,
            region,
            "TagResource",
            &json!({ "ResourceARN": arn, "Tags": [{ "Key": TAG_KEY, "Value": "seeded" }] }),
        )
        .await;
    }

    Arns {
        gateway,
        hypervisor,
        machine,
        host: host.to_string(),
    }
}

async fn fixture() -> Fixture {
    let engine = build_engine();
    let router = build_router(&engine);
    let owner = seed_region(&engine, &router, OWNER, OWNER_HOST).await;
    let neighbour = seed_region(&engine, &router, NEIGHBOUR, NEIGHBOUR_HOST).await;
    Fixture {
        engine,
        router,
        owner,
        neighbour,
    }
}

/// Every persisted fact a call under test could change, read out of the STORE rather than
/// off the wire: the record itself, so a member no response reports — a triggered appliance
/// update, a started metadata sync — is part of the comparison too, and the counts, so
/// nothing was added or removed anywhere.
#[derive(Debug, PartialEq, Eq)]
struct Persisted {
    gateways: usize,
    hypervisors: usize,
    machines: usize,
    records: Vec<(String, Option<String>)>,
}

fn persisted(fixture: &Fixture) -> Persisted {
    let state =
        service_state(&*fixture.engine).expect("the in-memory engine must expose this state");
    let store = state.lock();
    let mut records = Vec::new();
    for arns in [&fixture.owner, &fixture.neighbour] {
        records.push((
            arns.gateway.clone(),
            store.gateway(&arns.gateway).map(|record| format!("{record:?}")),
        ));
        records.push((
            arns.hypervisor.clone(),
            store
                .hypervisor(&arns.hypervisor)
                .map(|record| format!("{record:?}")),
        ));
        records.push((
            arns.machine.clone(),
            store
                .virtual_machine(&arns.machine)
                .map(|record| format!("{record:?}")),
        ));
    }
    Persisted {
        gateways: store.gateway_count(),
        hypervisors: store.hypervisor_count(),
        machines: store.virtual_machine_count(),
        records,
    }
}

/// Whether the listing `operation` reports `arn` when it is addressed to `region`.
async fn listing_reports(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    list_member: &str,
    arn_member: &str,
    arn: &str,
) -> bool {
    let reply = call_in_region(router, region, operation, &json!({})).await;
    assert_eq!(
        reply.status, 200,
        "case {operation} in {region}: a listing must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply
        .body
        .get(list_member)
        .and_then(Value::as_array)
        .is_some_and(|records| {
            records
                .iter()
                .any(|record| record.get(arn_member).and_then(Value::as_str) == Some(arn))
        })
}

/// The rule: a request addressed to one Region resolves only that Region's records, so a
/// call naming another Region's resource is refused exactly as one naming no record is, and
/// changes nothing — while the Region that owns the resource serves the very same request.
#[tokio::test]
async fn a_single_resource_call_resolves_only_the_records_of_the_region_it_was_addressed_to() {
    for case in cases() {
        let fixture = fixture().await;
        let before = persisted(&fixture);

        for (region_case, region) in OTHER_REGIONS {
            let absent = absent_arns(region);

            // The baseline: the same operation, signed for the same region, naming
            // resources that region never had. It has to be a refusal, or the comparison
            // below would be pinning a success.
            let baseline = refusal_of(
                &call_in_region(&fixture.router, region, case.operation, &(case.body)(&absent))
                    .await,
            );
            assert!(
                baseline.status >= 400,
                "case {} in {region} ({region_case}): an ARN naming no record must be refused, \
                 got {baseline:?}",
                case.operation
            );

            let crossed = refusal_of(
                &call_in_region(
                    &fixture.router,
                    region,
                    case.operation,
                    &(case.body)(&fixture.owner),
                )
                .await,
            );
            assert_eq!(
                crossed,
                as_if_asked_for(&baseline, &absent, &fixture.owner),
                "case {} on a {} of {OWNER} signed for {region} ({region_case}): the call must \
                 be refused exactly as one naming no record is, got {crossed:?}",
                case.operation,
                case.resource
            );
        }

        let after = persisted(&fixture);
        assert_eq!(
            after, before,
            "case {}: a call another region's endpoint refused must leave every record as it \
             was, got {after:?} against {before:?}",
            case.operation
        );

        // The owning region serves the very same request: what the refusals above answered
        // is the region the call was addressed to, not the request itself.
        let owned = call_in_region(
            &fixture.router,
            OWNER,
            case.operation,
            &(case.body)(&fixture.owner),
        )
        .await;
        assert_eq!(
            owned.status, 200,
            "case {}: the region that owns the {} must serve it, got {} {:?}",
            case.operation, case.resource, owned.status, owned.body
        );
    }
}

/// The same rule where the resource is resolved by HOST and not by an ARN: a configuration
/// test reaches the hypervisor records of the Region it was addressed to, so a host whose
/// only configuration lives in another Region is one this endpoint does not have — refused
/// as the missing hypervisor it is, with nothing written.
#[tokio::test]
async fn a_configuration_test_resolves_the_hosts_of_its_own_region() {
    let fixture = fixture().await;
    let before = persisted(&fixture);

    // (case, the region the test is addressed to, that region's own gateway, the host,
    //  whether this endpoint has a configuration for it)
    let rows: [(&str, &str, &Arns, &str, bool); 4] = [
        (
            "the owner's own host, from the owner",
            OWNER,
            &fixture.owner,
            OWNER_HOST,
            true,
        ),
        (
            "a host configured only in the neighbour, from the owner",
            OWNER,
            &fixture.owner,
            NEIGHBOUR_HOST,
            false,
        ),
        (
            "the neighbour's own host, from the neighbour",
            NEIGHBOUR,
            &fixture.neighbour,
            NEIGHBOUR_HOST,
            true,
        ),
        (
            "a host configured only in the owner, from the neighbour",
            NEIGHBOUR,
            &fixture.neighbour,
            OWNER_HOST,
            false,
        ),
    ];

    for (case, region, arns, host, configured) in rows {
        let reply = call_in_region(
            &fixture.router,
            region,
            "TestHypervisorConfiguration",
            &json!({ "GatewayArn": arns.gateway, "Host": host }),
        )
        .await;
        if configured {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: a host this region holds a configuration for must be served, \
                 got {} {:?}",
                reply.status, reply.body
            );
        } else {
            let message = message_of(&reply);
            assert_eq!(
                (
                    reply.status,
                    reply.code.as_deref(),
                    reply.str_at("/ErrorCode"),
                    message.as_str(),
                ),
                (
                    400,
                    Some("ResourceNotFoundException"),
                    Some("HypervisorNotFound"),
                    "The hypervisor is not found.",
                ),
                "case {case:?}: a host configured only in another region must be refused as \
                 the missing hypervisor it is, got {} {:?}",
                reply.status,
                reply.body
            );
        }
    }

    let after = persisted(&fixture);
    assert_eq!(
        after, before,
        "case configuration test: testing a configuration writes nothing, whichever region it \
         was addressed to, got {after:?} against {before:?}"
    );
}

/// The two sides of the scope agree: a record is resolvable by a single-resource read in
/// exactly the Regions whose listing reports it — never in one and not the other, which is
/// what would let a caller read, or destroy, a record it cannot even enumerate.
#[tokio::test]
async fn a_read_resolves_a_record_in_exactly_the_regions_whose_listing_reports_it() {
    let fixture = fixture().await;

    // (the read, the member it names the ARN in, the listing, the member the listing holds
    //  its records in, the member a record names its ARN in, the ARN of a seeded set)
    type Which = fn(&Arns) -> &String;
    let kinds: [(&str, &str, &str, &str, &str, Which); 3] = [
        (
            "GetGateway",
            "GatewayArn",
            "ListGateways",
            "Gateways",
            "GatewayArn",
            |arns| &arns.gateway,
        ),
        (
            "GetHypervisor",
            "HypervisorArn",
            "ListHypervisors",
            "Hypervisors",
            "HypervisorArn",
            |arns| &arns.hypervisor,
        ),
        (
            "GetVirtualMachine",
            "ResourceArn",
            "ListVirtualMachines",
            "VirtualMachines",
            "ResourceArn",
            |arns| &arns.machine,
        ),
    ];

    for (owner_region, arns) in [(OWNER, &fixture.owner), (NEIGHBOUR, &fixture.neighbour)] {
        for (read, arn_member, listing, list_member, listed_arn_member, which) in kinds {
            let arn = which(arns);
            for region in REGIONS {
                let reports = listing_reports(
                    &fixture.router,
                    region,
                    listing,
                    list_member,
                    listed_arn_member,
                    arn,
                )
                .await;
                let reply = call_in_region(
                    &fixture.router,
                    region,
                    read,
                    &json!({ arn_member: arn }),
                )
                .await;
                let resolves = reply.status == 200;

                assert_eq!(
                    resolves, reports,
                    "case {read}/{listing} of a record of {owner_region} addressed to {region}: \
                     a read must resolve exactly what the listing reports, got read {} and \
                     listed {reports}",
                    reply.status
                );
                assert_eq!(
                    resolves,
                    region == owner_region,
                    "case {read} of a record of {owner_region} addressed to {region}: only the \
                     region that minted a record resolves it, got {} {:?}",
                    reply.status,
                    reply.body
                );
            }
        }
    }
}
