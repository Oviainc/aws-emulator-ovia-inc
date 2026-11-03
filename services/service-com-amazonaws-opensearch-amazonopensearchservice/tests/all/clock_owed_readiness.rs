// SPDX-License-Identifier: CC0-1.0

//! The readiness this control plane publishes for the resources it mints, driven over the
//! real wire path: a `restJson1` request through the multi-service router into the mounted
//! service, so what is asserted is what a polling client sees.
//!
//! ONE rule, over every resource kind this service answers in a non-terminal status:
//!
//! > a resource a create answers in a non-terminal status reaches its terminal status once
//! > the ONE span the clock owes it has passed, and every read materialises what the clock
//! > already owed — so the state a client was told to wait for IS observable on a read of
//! > the resource, and stays observable on every later read, while a caller acting inside
//! > that span still observes the transient status (the span is never zero, which is what
//! > keeps the in-flight state and the state errors that go with it reachable).
//!
//! The rows span the kinds, not one recorded call: the DOMAIN itself
//! (`DomainStatus$Created`/`Processing`/`DomainProcessingStatus`), a PACKAGE
//! (`PackageDetails$PackageStatus`), a domain-package ASSOCIATION
//! (`DomainPackageDetails$DomainPackageStatus`) and a VPC ENDPOINT (`VpcEndpoint$Status`) —
//! each read back through the operation a client polls, and each asserted in the same three
//! positions: the create's own answer, a read taken inside the span, and the read taken
//! after it.
//!
//! The chain a client builds out of them is asserted too, in the order a client has to take
//! it (a domain, then a package on it, then the association, then an endpoint for it, then
//! the service-software read): every one of those waits observes its terminal status, and
//! the service-software options a quiescent poll compares agree across repeated reads.
//!
//! The control-plane harness comes from [`super::domain_lifecycle`]; this suite adds no
//! second copy of it.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    DEPLOYMENT_STATUS_COMPLETED, DOMAIN_PACKAGE_STATUS_ACTIVE, DOMAIN_PACKAGE_STATUS_ASSOCIATING,
    PACKAGE_STATUS_AVAILABLE, PACKAGE_STATUS_COPYING, PACKAGE_TYPE_TXT_DICTIONARY,
    PROCESSING_STATUS_ACTIVE, PROCESSING_STATUS_CREATING, VPC_ENDPOINT_STATUS_ACTIVE,
    VPC_ENDPOINT_STATUS_CREATING,
};

use super::domain_lifecycle::{Answer, Emulator, PREFIX, SETTLE_WAIT};
use super::package_associations::create_package;

/// One resource kind this rule covers: the name a failure reports it by, the status a
/// create answers for it, and the status the clock owes it.
struct Kind {
    case: &'static str,
    transient: &'static str,
    terminal: &'static str,
}

/// The kinds, one row per resource this control plane answers in a non-terminal status —
/// the four the readiness chain of a client is built out of.
///
/// The status values are this service's own statements of the modelled ones
/// (`DomainProcessingStatusType`, `PackageStatus`, `DomainPackageStatus`,
/// `VpcEndpointStatus`), imported rather than spelled here so a row cannot drift from the
/// value the handlers report.
fn kinds() -> Vec<Kind> {
    vec![
        Kind {
            case: "a domain",
            transient: PROCESSING_STATUS_CREATING,
            terminal: PROCESSING_STATUS_ACTIVE,
        },
        Kind {
            case: "a package",
            transient: PACKAGE_STATUS_COPYING,
            terminal: PACKAGE_STATUS_AVAILABLE,
        },
        Kind {
            case: "a domain-package association",
            transient: DOMAIN_PACKAGE_STATUS_ASSOCIATING,
            terminal: DOMAIN_PACKAGE_STATUS_ACTIVE,
        },
        Kind {
            case: "a VPC endpoint",
            transient: VPC_ENDPOINT_STATUS_CREATING,
            terminal: VPC_ENDPOINT_STATUS_ACTIVE,
        },
    ]
}

/// `DescribeDomain`'s view of the domain's readiness: the processing status it publishes.
async fn domain_status(emulator: &Emulator, domain: &str) -> Option<String> {
    let answer = emulator
        .call("GET", &format!("{PREFIX}/opensearch/domain/{domain}"), None)
        .await;
    assert_eq!(
        answer.status, 200,
        "case DescribeDomain({domain:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["DomainStatus"]["DomainProcessingStatus"]
        .as_str()
        .map(str::to_string)
}

/// Whether `DescribeDomain` reports the domain as created with nothing in flight — the
/// `Created && !Processing` pair a readiness poll waits for.
async fn domain_is_settled(emulator: &Emulator, domain: &str) -> bool {
    let answer = emulator
        .call("GET", &format!("{PREFIX}/opensearch/domain/{domain}"), None)
        .await;
    let status = &answer.body["DomainStatus"];
    status["Created"].as_bool() == Some(true) && status["Processing"].as_bool() == Some(false)
}

/// `DescribePackages`'s view of one package's readiness.
async fn package_status(emulator: &Emulator, package_id: &str) -> Option<String> {
    let answer = emulator
        .call(
            "POST",
            &format!("{PREFIX}/packages/describe"),
            Some(json!({ "Filters": [{ "Name": "PackageID", "Value": [package_id] }] })),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case DescribePackages({package_id:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["PackageDetailsList"][0]["PackageStatus"]
        .as_str()
        .map(str::to_string)
}

/// `ListPackagesForDomain`'s view of one association's readiness.
async fn association_status(
    emulator: &Emulator,
    domain: &str,
    package_id: &str,
) -> Option<String> {
    let answer = emulator
        .call(
            "GET",
            &format!("{PREFIX}/domain/{domain}/packages"),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case ListPackagesForDomain({domain:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["DomainPackageDetailsList"]
        .as_array()?
        .iter()
        .find(|details| details["PackageID"] == json!(package_id))
        .and_then(|details| details["DomainPackageStatus"].as_str())
        .map(str::to_string)
}

/// `DescribeVpcEndpoints`'s view of one endpoint's readiness.
async fn endpoint_status(emulator: &Emulator, endpoint_id: &str) -> Option<String> {
    let answer = emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/vpcEndpoints/describe"),
            Some(json!({ "VpcEndpointIds": [endpoint_id] })),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case DescribeVpcEndpoints({endpoint_id:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["VpcEndpoints"][0]["Status"]
        .as_str()
        .map(str::to_string)
}

/// The `ServiceSoftwareOptions` a reader publishes for the domain — what a client's
/// quiescent poll compares across reads.
async fn service_software(emulator: &Emulator, domain: &str) -> Value {
    let answer = emulator
        .call("GET", &format!("{PREFIX}/opensearch/domain/{domain}"), None)
        .await;
    answer.body["DomainStatus"]["ServiceSoftwareOptions"].clone()
}

/// The domain ARN the store holds, which `CreateVpcEndpoint` names its domain by.
fn domain_arn(emulator: &Emulator, domain: &str) -> String {
    let store = emulator.store();
    let guard = store.lock();
    guard
        .domains
        .get(domain)
        .unwrap_or_else(|| panic!("expected the store to hold domain {domain}"))
        .arn
        .clone()
}

/// A required string member of an answer, with the case named on failure.
fn string_at(answer: &Answer, pointer: &[&str], case: &str) -> String {
    let mut value = &answer.body;
    for step in pointer {
        value = &value[*step];
    }
    value
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a string at {pointer:?}, got {:?}",
                answer.body
            )
        })
        .to_string()
}

/// RULE: every resource this control plane answers in a non-terminal status reaches its
/// terminal status once the span the clock owes it has passed, and every read materialises
/// what was already owed — so a client that polls observes the state it waited for, and a
/// caller acting inside the span still observes the transient one.
#[tokio::test]
async fn every_minted_resource_reaches_the_status_the_clock_owes_it() {
    let emulator = Emulator::new();
    let table = kinds();
    let domain = "readiness-chain";

    // === a domain ===
    let kind = &table[0];
    let created = emulator
        .create(json!({
            "DomainName": domain,
            "ClusterConfig": { "InstanceType": "t3.small.search", "InstanceCount": 1 },
            "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 10 },
            // Provisioned inside a VPC, which is what gives the domain a VPC endpoint to
            // create — the fourth kind of the table.
            "VPCOptions": {
                "SubnetIds": ["subnet-0123456789abcdef0"],
                "SecurityGroupIds": ["sg-0123456789abcdef0"],
            },
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "case {}: expected 200 from CreateDomain, got {} with {:?}",
        kind.case, created.status, created.body
    );
    let answered = string_at(
        &created,
        &["DomainStatus", "DomainProcessingStatus"],
        kind.case,
    );
    assert_eq!(
        answered, kind.transient,
        "case {}: the create answers the transient status, expected {:?}, got {answered:?}",
        kind.case, kind.transient
    );
    // Inside the span: the read a client takes straight after the create still reports the
    // transient status, which is what keeps the in-flight state reachable.
    let inside = domain_status(&emulator, domain).await;
    assert_eq!(
        inside.as_deref(),
        Some(kind.transient),
        "case {}: a read inside the span reports the transient status, got {inside:?}",
        kind.case
    );

    tokio::time::sleep(SETTLE_WAIT).await;

    // After the span: the status a client polls for is observable, and stays so.
    for probe in 1..=2 {
        let settled = domain_status(&emulator, domain).await;
        assert_eq!(
            settled.as_deref(),
            Some(kind.terminal),
            "case {} (read {probe}): expected {:?} once the span has passed, got {settled:?}",
            kind.case,
            kind.terminal
        );
        assert!(
            domain_is_settled(&emulator, domain).await,
            "case {} (read {probe}): expected Created && !Processing once the span has passed",
            kind.case
        );
    }

    // === a package ===
    let kind = &table[1];
    let package_id = create_package(&emulator, "readiness-dictionary", PACKAGE_TYPE_TXT_DICTIONARY)
        .await;
    let inside = package_status(&emulator, &package_id).await;
    assert_eq!(
        inside.as_deref(),
        Some(kind.transient),
        "case {}: a read inside the span reports the transient status, got {inside:?}",
        kind.case
    );

    tokio::time::sleep(SETTLE_WAIT).await;

    for probe in 1..=2 {
        let settled = package_status(&emulator, &package_id).await;
        assert_eq!(
            settled.as_deref(),
            Some(kind.terminal),
            "case {} (read {probe}): expected {:?} once the span has passed, got {settled:?}",
            kind.case,
            kind.terminal
        );
    }

    // === a domain-package association ===
    let kind = &table[2];
    let associated = emulator
        .call(
            "POST",
            &format!("{PREFIX}/packages/associateMultiple"),
            Some(json!({
                "DomainName": domain,
                "PackageList": [{ "PackageID": package_id }],
            })),
        )
        .await;
    assert_eq!(
        associated.status, 200,
        "case {}: expected 200 from AssociatePackages, got {} with {:?}",
        kind.case, associated.status, associated.body
    );
    let answered = associated.body["DomainPackageDetailsList"][0]["DomainPackageStatus"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        answered, kind.transient,
        "case {}: the call answers the transient status, expected {:?}, got {answered:?}",
        kind.case, kind.transient
    );
    let inside = association_status(&emulator, domain, &package_id).await;
    assert_eq!(
        inside.as_deref(),
        Some(kind.transient),
        "case {}: a read inside the span reports the transient status, got {inside:?}",
        kind.case
    );

    // === a VPC endpoint ===
    let kind = &table[3];
    let arn = domain_arn(&emulator, domain);
    let endpoint = emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/vpcEndpoints"),
            Some(json!({
                "DomainArn": arn,
                "VpcOptions": { "SubnetIds": ["subnet-0123456789abcdef0"] },
            })),
        )
        .await;
    assert_eq!(
        endpoint.status, 200,
        "case {}: expected 200 from CreateVpcEndpoint, got {} with {:?}",
        kind.case, endpoint.status, endpoint.body
    );
    let endpoint_id = string_at(&endpoint, &["VpcEndpoint", "VpcEndpointId"], kind.case);
    let answered = string_at(&endpoint, &["VpcEndpoint", "Status"], kind.case);
    assert_eq!(
        answered, kind.transient,
        "case {}: the create answers the transient status, expected {:?}, got {answered:?}",
        kind.case, kind.transient
    );
    let inside = endpoint_status(&emulator, &endpoint_id).await;
    assert_eq!(
        inside.as_deref(),
        Some(kind.transient),
        "case {}: a read inside the span reports the transient status, got {inside:?}",
        kind.case
    );

    tokio::time::sleep(SETTLE_WAIT).await;

    // The association and the endpoint were started inside the same span, so one wait
    // settles both: each reports the status the clock owed it, on every read.
    for probe in 1..=2 {
        let settled = association_status(&emulator, domain, &package_id).await;
        assert_eq!(
            settled.as_deref(),
            Some(table[2].terminal),
            "case {} (read {probe}): expected {:?} once the span has passed, got {settled:?}",
            table[2].case,
            table[2].terminal
        );
        let settled = endpoint_status(&emulator, &endpoint_id).await;
        assert_eq!(
            settled.as_deref(),
            Some(table[3].terminal),
            "case {} (read {probe}): expected {:?} once the span has passed, got {settled:?}",
            table[3].case,
            table[3].terminal
        );
    }

    // === what a client's LAST wait of the chain reads ===
    // The service-software options a quiescent poll compares: no deployment is in flight on
    // a domain nothing was deployed to, and three agreeing reads are what the poll needs.
    let mut reads = Vec::new();
    for _ in 0..3 {
        reads.push(service_software(&emulator, domain).await);
    }
    for (probe, read) in reads.iter().enumerate() {
        assert_eq!(
            read["UpdateStatus"].as_str(),
            Some(DEPLOYMENT_STATUS_COMPLETED),
            "case the service software (read {probe}): expected {DEPLOYMENT_STATUS_COMPLETED:?}, \
             got {read:?}"
        );
        assert_eq!(
            read["UpdateAvailable"].as_bool(),
            Some(false),
            "case the service software (read {probe}): expected no update available, got {read:?}"
        );
    }
    assert!(
        reads.windows(2).all(|pair| pair[0] == pair[1]),
        "case the service software: expected agreeing reads, got {reads:?}"
    );
}
