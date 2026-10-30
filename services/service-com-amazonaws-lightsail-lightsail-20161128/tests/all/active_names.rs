// SPDX-License-Identifier: CC0-1.0

//! `GetActiveNames` — the flat, paged list of the names the account's resources
//! hold — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The `Authorization` header carries only a credential SCOPE, which
//! is what names the Region a request is made in.
//!
//! The rule under test: this operation reports the NAME of every resource the
//! account holds that a request made in that Region addresses — one flat,
//! de-duplicated list in the store's own name order, carried one page at a time
//! with `nextPageToken` naming the first name of the page after it and absent on
//! the last page — and nothing for a resource a delete removed. A resource whose
//! identity includes a Region (a bucket, a container service, a load balancer and
//! each SSL/TLS certificate it holds, a contact method) is named in THAT Region
//! alone; every other kind is served by name alone, so it is named whichever
//! Region the listing is made in.
//!
//! The rows span what the rule generalises over: the KIND of resource (every
//! named kind this service stores, both the kinds the caller names and the ones
//! whose name the service mints), the Region the create was made in against the
//! Region the listing is made in, whether the name has since been deleted, and
//! how many names the account holds (none, one, exactly a page, one past a page,
//! several pages and a remainder).

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::state;

use crate::instance_lifecycle::Emulator;
use crate::paging::expected_page_sizes;

/// How many names one `GetActiveNames` page carries — the handler's OWN
/// constant, so the paging rows keep straddling the page boundary whatever it is
/// set to.
const PAGE_SIZE: usize = state::ACTIVE_NAME_PAGE_SIZE;

/// The Region every case that does not say otherwise is made in: the one the
/// domain, distribution and certificate APIs are served from, so a single
/// account can hold a resource of every kind at once.
const REGION: &str = "us-east-1";

/// A second Region, for the cases about a resource whose identity includes one.
const OTHER_REGION: &str = "ap-northeast-1";

/// A walk over the pages of one listing cannot need more than this: anything
/// past it means the walk is not advancing.
const MAX_PAGES: usize = 20;

/// How long a case waits for a transition the clock owes.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// Driving the listing
// ---------------------------------------------------------------------------

/// One `GetActiveNames` page, resuming from `page_token` when one is given, made
/// in `region`.
async fn page(emulator: &Emulator, region: &str, page_token: Option<&str>) -> Value {
    let body = match page_token {
        Some(token) => json!({"pageToken": token}),
        None => json!({}),
    };
    emulator
        .ok_at(None, Some(region), "GetActiveNames", body)
        .await
}

/// The names one page reports, in the order it reports them.
fn reported(case: &str, body: &Value) -> Vec<String> {
    body["activeNames"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an activeNames array, got {body:?}"))
        .iter()
        .map(|name| {
            name.as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: every active name is a string, got {body:?}")
                })
                .to_string()
        })
        .collect()
}

/// Walk every page of this listing in `region`, answering the names in the order
/// reported and the size of each page.
///
/// Checks the paging contract as it goes: no page carries more than
/// [`PAGE_SIZE`] names, a `nextPageToken` names the FIRST name of the page after
/// it, a page that is not the last one is full, and the walk terminates because
/// the token is absent rather than because it ran out of patience.
/// `paging::walk_pages` walks records addressed BY a `name` member; this
/// listing's entries are the names themselves, so the walk lives here.
async fn walk(emulator: &Emulator, case: &str, region: &str) -> (Vec<String>, Vec<usize>) {
    let mut names: Vec<String> = Vec::new();
    let mut page_sizes = Vec::new();
    let mut token: Option<String> = None;
    for page_index in 0..MAX_PAGES {
        let body = page(emulator, region, token.as_deref()).await;
        let reported = reported(case, &body);
        assert!(
            reported.len() <= PAGE_SIZE,
            "case {case}: page {page_index} carries more than the {PAGE_SIZE} of a page, got \
             {reported:?}"
        );
        if let Some(token) = &token {
            assert_eq!(
                reported.first().map(String::as_str),
                Some(token.as_str()),
                "case {case}: a page token names the first name of the page after it, got \
                 {reported:?}"
            );
        }
        page_sizes.push(reported.len());
        names.extend(reported.iter().cloned());
        match body["nextPageToken"].as_str() {
            Some(next) => {
                assert_eq!(
                    reported.len(),
                    PAGE_SIZE,
                    "case {case}: a page that is not the last one is full, got {reported:?}"
                );
                token = Some(next.to_string());
            }
            None => return (names, page_sizes),
        }
    }
    panic!("case {case}: the walk did not terminate within {MAX_PAGES} pages");
}

/// The names a listing in `region` reports, as a set.
async fn active_names(emulator: &Emulator, case: &str, region: &str) -> BTreeSet<String> {
    walk(emulator, case, region).await.0.into_iter().collect()
}

/// The names of `expected`, in the order a listing must report them: the set's
/// own name order.
fn in_name_order(expected: &BTreeSet<String>) -> Vec<String> {
    expected.iter().cloned().collect()
}

// ---------------------------------------------------------------------------
// The kinds of resource an account can hold
// ---------------------------------------------------------------------------

/// A kind of Lightsail resource whose name the CALLER chooses.
///
/// The rows differ along what the rule generalises over: the kind of resource,
/// the operation that creates it, whether its identity includes the Region, and
/// whether this service has a delete that removes it.
#[derive(Clone, Copy, Debug)]
enum Kind {
    Instance,
    Disk,
    DiskSnapshot,
    InstanceSnapshot,
    Domain,
    Database,
    DatabaseSnapshot,
    StaticIp,
    Alarm,
    Distribution,
    Certificate,
    Bucket,
    ContainerService,
    LoadBalancer,
    LoadBalancerCertificate,
    /// A SECOND certificate on the same load balancer, added by a call of its
    /// own: a load balancer holds several, and each one is a named resource.
    AddedLoadBalancerCertificate,
}

/// Every kind, dependencies first: the instance a snapshot is taken of, an alarm
/// monitors and a distribution originates at, the disk a disk snapshot is taken
/// of, and the database a database snapshot is taken of are each created by an
/// earlier row.
const KINDS: &[Kind] = &[
    Kind::Instance,
    Kind::Disk,
    Kind::Database,
    Kind::DiskSnapshot,
    Kind::InstanceSnapshot,
    Kind::DatabaseSnapshot,
    Kind::Domain,
    Kind::StaticIp,
    Kind::Alarm,
    Kind::Distribution,
    Kind::Certificate,
    Kind::Bucket,
    Kind::ContainerService,
    Kind::LoadBalancer,
    Kind::LoadBalancerCertificate,
    Kind::AddedLoadBalancerCertificate,
];

impl Kind {
    /// The name this suite creates this kind's resource under. Each satisfies
    /// that resource's own modelled name constraint — a DNS zone is the one kind
    /// named by a dotted `DomainName`.
    fn name(self) -> &'static str {
        match self {
            Kind::Instance => "active-instance",
            Kind::Disk => "active-disk",
            Kind::DiskSnapshot => "active-disk-snapshot",
            Kind::InstanceSnapshot => "active-instance-snapshot",
            Kind::Domain => "active-zone.example",
            Kind::Database => "active-database",
            Kind::DatabaseSnapshot => "active-database-snapshot",
            Kind::StaticIp => "active-static-ip",
            Kind::Alarm => "active-alarm",
            Kind::Distribution => "active-distribution",
            Kind::Certificate => "active-certificate",
            Kind::Bucket => "active-bucket",
            Kind::ContainerService => "active-containers",
            Kind::LoadBalancer => "active-balancer",
            Kind::LoadBalancerCertificate => "active-balancer-cert",
            Kind::AddedLoadBalancerCertificate => "active-balancer-cert-2",
        }
    }

    /// Whether this kind's identity includes the Region it was created in — the
    /// kinds whose own listings report "all of your … in the Amazon Web Services
    /// Region where the request is made", plus the SSL/TLS certificates a load
    /// balancer holds, which live on that load balancer's record.
    fn is_regional(self) -> bool {
        matches!(
            self,
            Kind::Bucket
                | Kind::ContainerService
                | Kind::LoadBalancer
                | Kind::LoadBalancerCertificate
                | Kind::AddedLoadBalancerCertificate
        )
    }

    /// The create this kind's resource is made with, and the HTTP status that
    /// create answers. `None` for the load balancer's certificate: the load
    /// balancer's own create is what asks for it, so no call of its own creates
    /// it.
    ///
    /// Every name in the request — this kind's own and the ones it refers to —
    /// comes from `name_of`, so one request table serves a case that holds one
    /// resource per kind and a case that holds a differently named one per
    /// Region.
    fn create_request(
        self,
        name_of: impl Fn(Kind) -> String,
    ) -> Option<(&'static str, Value, u16)> {
        let name = name_of(self);
        Some(match self {
            Kind::Instance => (
                "CreateInstances",
                json!({
                    "instanceNames": [name],
                    "availabilityZone": "us-east-1a",
                    "blueprintId": "amazon_linux_2023",
                    "bundleId": "micro_3_0",
                }),
                200,
            ),
            Kind::Disk => (
                "CreateDisk",
                json!({"diskName": name, "availabilityZone": "us-east-1a", "sizeInGb": 8}),
                200,
            ),
            Kind::DiskSnapshot => (
                "CreateDiskSnapshot",
                json!({"diskName": name_of(Kind::Disk), "diskSnapshotName": name}),
                200,
            ),
            Kind::InstanceSnapshot => (
                "CreateInstanceSnapshot",
                json!({
                    "instanceName": name_of(Kind::Instance),
                    "instanceSnapshotName": name,
                }),
                200,
            ),
            Kind::Domain => ("CreateDomain", json!({"domainName": name}), 200),
            Kind::Database => (
                "CreateRelationalDatabase",
                json!({
                    "relationalDatabaseName": name,
                    "availabilityZone": "us-east-1a",
                    "relationalDatabaseBlueprintId": "mysql_8_4",
                    "relationalDatabaseBundleId": "micro_1_0",
                    "masterDatabaseName": "appdb",
                    "masterUsername": "dbadmin",
                }),
                200,
            ),
            Kind::DatabaseSnapshot => (
                "CreateRelationalDatabaseSnapshot",
                json!({
                    "relationalDatabaseName": name_of(Kind::Database),
                    "relationalDatabaseSnapshotName": name,
                }),
                200,
            ),
            Kind::StaticIp => ("AllocateStaticIp", json!({"staticIpName": name}), 200),
            Kind::Alarm => (
                "PutAlarm",
                json!({
                    "alarmName": name,
                    "monitoredResourceName": name_of(Kind::Instance),
                    "metricName": "CPUUtilization",
                    "comparisonOperator": "GreaterThanThreshold",
                    "threshold": 80,
                    "evaluationPeriods": 2,
                }),
                200,
            ),
            Kind::Distribution => (
                "CreateDistribution",
                json!({
                    "distributionName": name,
                    "origin": {"name": name_of(Kind::Instance)},
                    "defaultCacheBehavior": {"behavior": "cache"},
                    "bundleId": "small_1_0",
                }),
                200,
            ),
            Kind::Certificate => (
                "CreateCertificate",
                json!({"certificateName": name, "domainName": "active-cert.example"}),
                200,
            ),
            Kind::Bucket => (
                "CreateBucket",
                json!({"bucketName": name, "bundleId": "small_1_0"}),
                200,
            ),
            // `CreateContainerService` answers the modelled 201 for the
            // resource it created.
            Kind::ContainerService => (
                "CreateContainerService",
                json!({"serviceName": name, "power": "nano", "scale": 2}),
                201,
            ),
            // The load balancer is created holding the certificate the next
            // kind is about, so one call creates two named resources.
            Kind::LoadBalancer => (
                "CreateLoadBalancer",
                json!({
                    "loadBalancerName": name,
                    "instancePort": 80,
                    "certificateName": name_of(Kind::LoadBalancerCertificate),
                    "certificateDomainName": "active-balancer.example",
                }),
                200,
            ),
            Kind::LoadBalancerCertificate => return None,
            // The second certificate is asked for on its own, once the load
            // balancer exists.
            Kind::AddedLoadBalancerCertificate => (
                "CreateLoadBalancerTlsCertificate",
                json!({
                    "loadBalancerName": name_of(Kind::LoadBalancer),
                    "certificateName": name,
                    "certificateDomainName": "second.active-balancer.example",
                }),
                200,
            ),
        })
    }

    /// Create this kind's resource in `region`, with every name in the request
    /// resolved through `name_of`.
    async fn create(self, emulator: &Emulator, region: &str, name_of: impl Fn(Kind) -> String) {
        let Some((operation, body, want_status)) = self.create_request(&name_of) else {
            return;
        };
        let (status, answered) = emulator.call_at(None, Some(region), operation, body).await;
        assert_eq!(
            status, want_status,
            "case creating {self:?} named {} in {region}: expected HTTP {want_status} from \
             {operation}, got {status} {answered:?}",
            name_of(self)
        );
    }

    /// The delete that removes this kind's resource, or `None` for a kind whose
    /// delete this service has not implemented yet (the instance snapshot, the
    /// database snapshot, the certificate, the container service and the static
    /// IP, each still a 501 scaffold with an issue of its own).
    fn delete_request(self) -> Option<(&'static str, Value)> {
        let name = self.name();
        Some(match self {
            Kind::Instance => ("DeleteInstance", json!({"instanceName": name})),
            Kind::Distribution => ("DeleteDistribution", json!({"distributionName": name})),
            Kind::Disk => ("DeleteDisk", json!({"diskName": name})),
            Kind::DiskSnapshot => ("DeleteDiskSnapshot", json!({"diskSnapshotName": name})),
            Kind::Domain => ("DeleteDomain", json!({"domainName": name})),
            // A delete that skips the final snapshot removes exactly one name;
            // one that takes it would add the snapshot's.
            Kind::Database => (
                "DeleteRelationalDatabase",
                json!({"relationalDatabaseName": name, "skipFinalSnapshot": true}),
            ),
            Kind::Alarm => ("DeleteAlarm", json!({"alarmName": name})),
            Kind::Bucket => ("DeleteBucket", json!({"bucketName": name})),
            Kind::LoadBalancerCertificate | Kind::AddedLoadBalancerCertificate => (
                "DeleteLoadBalancerTlsCertificate",
                json!({
                    "loadBalancerName": Kind::LoadBalancer.name(),
                    "certificateName": name,
                }),
            ),
            Kind::LoadBalancer => ("DeleteLoadBalancer", json!({"loadBalancerName": name})),
            _ => return None,
        })
    }
}

/// Wait until the block storage disk `name` reports `available` — the one state
/// its documented delete accepts ("The disk must be in the available state"), and
/// a transition the clock owes rather than one a caller can ask for.
async fn wait_for_available_disk(emulator: &Emulator, name: &str) {
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    loop {
        let disk = emulator
            .ok_at(None, Some(REGION), "GetDisk", json!({"diskName": name}))
            .await["disk"]
            .clone();
        if disk["state"].as_str() == Some("available") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for {name}: expected the disk to become available within \
             {SETTLE_TIMEOUT:?}, got {disk:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ---------------------------------------------------------------------------
// Every kind of resource the account holds
// ---------------------------------------------------------------------------

/// RULE: every resource the account holds contributes its name, whatever kind it
/// is — and the names come back in the store's own name order, full pages until
/// the remainder.
#[tokio::test]
async fn every_kind_of_resource_the_account_holds_contributes_its_name() {
    let emulator = Emulator::new();
    let mut expected: BTreeSet<String> = BTreeSet::new();
    for kind in KINDS {
        kind.create(&emulator, REGION, own_name).await;
        expected.insert(kind.name().to_string());
    }

    let case = "every kind of resource";
    let (names, page_sizes) = walk(&emulator, case, REGION).await;
    assert_eq!(
        names,
        in_name_order(&expected),
        "case {case}: expected every held name in name order, got {names:?}"
    );
    assert_eq!(
        page_sizes,
        expected_page_sizes(expected.len(), PAGE_SIZE),
        "case {case}: expected full pages until the remainder, got {page_sizes:?}"
    );

    // Each name is reported once: a page boundary neither repeats nor skips one.
    let distinct: BTreeSet<&String> = names.iter().collect();
    assert_eq!(
        distinct.len(),
        names.len(),
        "case {case}: expected every name once, got {names:?}"
    );

    // This is a READ: it reports the same names however often it is called.
    let (again, again_sizes) = walk(&emulator, "every kind of resource, again", REGION).await;
    assert_eq!(
        again, names,
        "case {case}: a read must not change what a later caller sees, got {again:?}"
    );
    assert_eq!(
        again_sizes, page_sizes,
        "case {case}: expected the same paging, got {again_sizes:?}"
    );
}

/// RULE: an account holding no resource reports an empty LIST and no token —
/// whatever page token the request carries, because a token names a name the
/// account does not hold.
#[tokio::test]
async fn an_account_holding_nothing_reports_an_empty_list_and_no_token() {
    let emulator = Emulator::new();
    for token in [None, Some("anything"), Some("zzz-past-the-end")] {
        let case = format!("an empty account at {token:?}");
        let body = page(&emulator, REGION, token).await;
        assert_eq!(
            reported(&case, &body),
            Vec::<String>::new(),
            "case {case}: expected an empty activeNames array, got {body:?}"
        );
        assert!(
            body["nextPageToken"].is_null(),
            "case {case}: a last page carries no nextPageToken, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The Region a resource belongs to
// ---------------------------------------------------------------------------

/// RULE: a resource whose identity includes a Region is named in that Region
/// alone, while a resource served by name alone is named whichever Region the
/// listing is made in.
#[tokio::test]
async fn a_regional_resource_is_named_in_its_own_region_alone() {
    let emulator = Emulator::new();

    // One of each kind in each of two Regions, named apart by Region so a
    // listing that ignored the Region would report twice as many names.
    let mut here: BTreeSet<String> = BTreeSet::new();
    let mut there: BTreeSet<String> = BTreeSet::new();
    let mut anywhere: BTreeSet<String> = BTreeSet::new();
    for kind in KINDS {
        for (region, held) in [(REGION, &mut here), (OTHER_REGION, &mut there)] {
            // The two Regions cannot hold the same name under a kind keyed by
            // name alone, so only the regional kinds are created in both.
            if region != REGION && !kind.is_regional() {
                continue;
            }
            // A regional kind's name carries the Region it was created in, so
            // the two creates are two resources rather than one.
            let created = regional_name(*kind, region);
            kind.create(&emulator, region, |kind| regional_name(kind, region))
                .await;
            if kind.is_regional() {
                held.insert(created);
            } else {
                anywhere.insert(created);
            }
        }
    }

    let reported_here = active_names(&emulator, "a listing in this Region", REGION).await;
    let reported_there = active_names(&emulator, "a listing in the other Region", OTHER_REGION).await;

    let want_here: BTreeSet<String> = here.union(&anywhere).cloned().collect();
    let want_there: BTreeSet<String> = there.union(&anywhere).cloned().collect();
    assert_eq!(
        reported_here, want_here,
        "case a listing in {REGION}: expected this Region's regional resources and every \
         name-keyed one, got {reported_here:?}"
    );
    assert_eq!(
        reported_there, want_there,
        "case a listing in {OTHER_REGION}: expected that Region's regional resources and every \
         name-keyed one, got {reported_there:?}"
    );

    // Neither Region reports the other's regional names.
    for name in &there {
        assert!(
            !reported_here.contains(name),
            "case a listing in {REGION}: expected nothing of {OTHER_REGION}, got {name}"
        );
    }
    for name in &here {
        assert!(
            !reported_there.contains(name),
            "case a listing in {OTHER_REGION}: expected nothing of {REGION}, got {name}"
        );
    }
}

/// The name this suite creates `kind`'s resource under when one account holds
/// one resource per kind: the kind's own name.
fn own_name(kind: Kind) -> String {
    kind.name().to_string()
}

/// The name this suite creates `kind`'s resource under in `region`: a regional
/// kind carries the Region, because the account can hold one of those in each
/// Region at once.
fn regional_name(kind: Kind, region: &str) -> String {
    if kind.is_regional() {
        format!("{}-{region}", kind.name())
    } else {
        kind.name().to_string()
    }
}

// ---------------------------------------------------------------------------
// A name the service mints
// ---------------------------------------------------------------------------

/// RULE: the rule is about the resources the account HOLDS, not about who chose
/// their names — so a record whose name the service minted is an active name
/// too, and a contact method's, whose identity includes a Region, in that Region
/// alone.
#[tokio::test]
async fn a_record_whose_name_the_service_mints_is_an_active_name() {
    let emulator = Emulator::new();
    let source_disk = "minted-source-disk";
    let snapshot = "minted-source-disk-snapshot";
    emulator
        .ok_at(
            None,
            Some(REGION),
            "CreateDisk",
            json!({"diskName": source_disk, "availabilityZone": "us-east-1a", "sizeInGb": 8}),
        )
        .await;
    emulator
        .ok_at(
            None,
            Some(REGION),
            "CreateDiskSnapshot",
            json!({"diskName": source_disk, "diskSnapshotName": snapshot}),
        )
        .await;

    // The export snapshot record: the service mints its name, so it is read back
    // from that record's own reader.
    emulator
        .ok_at(
            None,
            Some(REGION),
            "ExportSnapshot",
            json!({"sourceSnapshotName": snapshot}),
        )
        .await;
    let export = minted_record_name(
        &emulator,
        "GetExportSnapshotRecords",
        "exportSnapshotRecords",
    )
    .await;
    // A stack is created from an export the clock has settled into `Succeeded`.
    wait_for_succeeded_export(&emulator, &export).await;
    emulator
        .ok_at(
            None,
            Some(REGION),
            "CreateCloudFormationStack",
            json!({"instances": [{
                "sourceName": export,
                "instanceType": "t2.micro",
                "portInfoSource": "DEFAULT",
                "availabilityZone": "us-east-1a",
            }]}),
        )
        .await;
    let stack = minted_record_name(
        &emulator,
        "GetCloudFormationStackRecords",
        "cloudFormationStackRecords",
    )
    .await;

    // The contact method: named by the service as well, and held under the
    // Region it was created in.
    emulator
        .ok_at(
            None,
            Some(REGION),
            "CreateContactMethod",
            json!({"protocol": "Email", "contactEndpoint": "someone@example.com"}),
        )
        .await;
    let contact = minted_record_name(&emulator, "GetContactMethods", "contactMethods").await;

    let account_wide: BTreeSet<String> = [
        source_disk.to_string(),
        snapshot.to_string(),
        export.clone(),
        stack.clone(),
    ]
    .into_iter()
    .collect();
    let mut want_here = account_wide.clone();
    want_here.insert(contact.clone());

    let case = "a minted name";
    let here = active_names(&emulator, case, REGION).await;
    assert_eq!(
        here, want_here,
        "case {case}: expected the minted record names beside the caller-named ones, got {here:?}"
    );

    let elsewhere = active_names(&emulator, "a minted name in another Region", OTHER_REGION).await;
    assert_eq!(
        elsewhere, account_wide,
        "case {case}: expected the contact method's name in {REGION} alone, got {elsewhere:?}"
    );
}

/// The ONE record `member` of `operation`'s answer holds, by the name the service
/// minted for it.
async fn minted_record_name(emulator: &Emulator, operation: &str, member: &str) -> String {
    let body = emulator
        .ok_at(None, Some(REGION), operation, json!({}))
        .await;
    let records = body[member]
        .as_array()
        .unwrap_or_else(|| panic!("case reading {operation}: expected a {member} array, got {body:?}"));
    assert_eq!(
        records.len(),
        1,
        "case reading {operation}: expected exactly the one created record, got {records:?}"
    );
    records[0]["name"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case reading {operation}: expected the record to carry a name, got {records:?}")
        })
        .to_string()
}

/// Wait until the export snapshot record `name` reports `Succeeded` — the state
/// a CloudFormation stack can be created from, and one the clock owes.
async fn wait_for_succeeded_export(emulator: &Emulator, name: &str) {
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    loop {
        let body = emulator
            .ok_at(None, Some(REGION), "GetExportSnapshotRecords", json!({}))
            .await;
        let record = body["exportSnapshotRecords"]
            .as_array()
            .and_then(|records| {
                records
                    .iter()
                    .find(|record| record["name"].as_str() == Some(name))
                    .cloned()
            })
            .unwrap_or_else(|| panic!("case waiting for {name}: expected the record, got {body:?}"));
        if record["state"].as_str() == Some("Succeeded") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for {name}: expected the export to succeed within {SETTLE_TIMEOUT:?}, \
             got {record:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ---------------------------------------------------------------------------
// A name a delete released
// ---------------------------------------------------------------------------

/// RULE: these are the ACTIVE names — a resource a delete removed holds its name
/// no longer, and no other resource's name moves with it.
#[tokio::test]
async fn a_deleted_resources_name_is_no_longer_active() {
    let emulator = Emulator::new();
    let mut expected: BTreeSet<String> = BTreeSet::new();
    for kind in KINDS {
        kind.create(&emulator, REGION, own_name).await;
        expected.insert(kind.name().to_string());
    }

    // Each certificate the load balancer holds is deleted BEFORE the load
    // balancer, so each delete is observed on its own: deleting the load
    // balancer takes the certificates it holds with it. The ALARM goes before
    // the instance it watches for the same reason: an alarm's monitored
    // resource is one the account holds, so removing that instance releases the
    // alarm with it. The DISTRIBUTION goes before the instance it pulls its
    // content from, for the other reason: a resource a distribution of the
    // account names is in use by it, so the delete of that resource is refused
    // while the distribution still pulls from it.
    let deletable = [
        Kind::AddedLoadBalancerCertificate,
        Kind::LoadBalancerCertificate,
        Kind::LoadBalancer,
        Kind::Distribution,
        Kind::Alarm,
        Kind::Instance,
        Kind::Disk,
        Kind::DiskSnapshot,
        Kind::Domain,
        Kind::Database,
        Kind::Bucket,
    ];
    for kind in deletable {
        let case = format!("deleting {kind:?}");
        let (operation, body) = kind
            .delete_request()
            .unwrap_or_else(|| panic!("case {case}: this kind has a delete"));
        // The one kind whose delete has a documented state precondition.
        if matches!(kind, Kind::Disk) {
            wait_for_available_disk(&emulator, kind.name()).await;
        }
        let (status, answered) = emulator
            .call_at(None, Some(REGION), operation, body)
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected {operation} to be served, got {status} {answered:?}"
        );

        assert!(
            expected.remove(kind.name()),
            "case {case}: expected the name to have been active before the delete"
        );
        let left = active_names(&emulator, &case, REGION).await;
        assert_eq!(
            left, expected,
            "case {case}: expected exactly the deleted name gone, got {left:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Paging
// ---------------------------------------------------------------------------

/// One account's worth of names: how many it holds, and the prefix that tells
/// this row's resources apart.
#[derive(Debug, Clone, Copy)]
struct PageRow {
    case: &'static str,
    prefix: &'static str,
    count: usize,
}

/// The counts straddle every page boundary the rule can turn on.
const PAGE_ROWS: &[PageRow] = &[
    PageRow {
        case: "an account holding one name",
        prefix: "solo",
        count: 1,
    },
    PageRow {
        case: "exactly one page",
        prefix: "full",
        count: PAGE_SIZE,
    },
    PageRow {
        case: "one past a page",
        prefix: "spill",
        count: PAGE_SIZE + 1,
    },
    PageRow {
        case: "several pages and a remainder",
        prefix: "wide",
        count: 2 * PAGE_SIZE + 3,
    },
];

/// The name of this row's resource `index`. Zero-padded, so name order is index
/// order however many a row holds.
fn page_row_name(row: &PageRow, index: usize) -> String {
    format!("{}-name-{index:02}", row.prefix)
}

/// RULE: the names are carried one page at a time, in the store's own name
/// order, with `nextPageToken` NAMING the first name of the page after it — so
/// resuming from any held name reports that name onward.
#[tokio::test]
async fn the_names_are_paged_in_name_order_and_every_name_resumes_the_walk() {
    for row in PAGE_ROWS {
        let emulator = Emulator::new();
        let case = row.case;
        // Allocated in REVERSE name order, so a listing that merely echoed the
        // order the resources were created in would fail.
        let mut all: Vec<String> = Vec::new();
        for index in (0..row.count).rev() {
            let name = page_row_name(row, index);
            emulator
                .ok_at(
                    None,
                    Some(REGION),
                    "AllocateStaticIp",
                    json!({"staticIpName": name}),
                )
                .await;
            all.push(name);
        }
        all.sort();

        let (names, page_sizes) = walk(&emulator, case, REGION).await;
        assert_eq!(
            names, all,
            "case {case}: expected every held name in name order, got {names:?}"
        );
        assert_eq!(
            page_sizes,
            expected_page_sizes(row.count, PAGE_SIZE),
            "case {case}: expected full pages until the remainder, got {page_sizes:?}"
        );

        // A page token NAMES a name rather than carrying an index, so resuming
        // from any held name reports that name onward.
        for (index, token) in all.iter().enumerate() {
            let body = page(&emulator, REGION, Some(token)).await;
            let resumed = reported(case, &body);
            let want: Vec<String> = all[index..].iter().take(PAGE_SIZE).cloned().collect();
            assert_eq!(
                resumed, want,
                "case {case}: resuming from {token} must report that name onward, got {resumed:?}"
            );
            let remaining = all.len() - index;
            assert_eq!(
                body["nextPageToken"].as_str(),
                (remaining > PAGE_SIZE).then(|| all[index + PAGE_SIZE].as_str()),
                "case {case}: expected the next token to name the first name of the page after \
                 it, got {body:?}"
            );
        }
    }
}

/// RULE: the listing answers the STORE, so a resource created after a listing
/// joins the next one in its name's place, and one of another kind joins it just
/// the same.
#[tokio::test]
async fn a_resource_created_later_joins_the_listing_in_name_order() {
    let emulator = Emulator::new();
    for name in ["name-a", "name-c"] {
        emulator
            .ok_at(
                None,
                Some(REGION),
                "AllocateStaticIp",
                json!({"staticIpName": name}),
            )
            .await;
    }
    let case = "before the later creates";
    let before = walk(&emulator, case, REGION).await.0;
    assert_eq!(
        before,
        vec!["name-a".to_string(), "name-c".to_string()],
        "case {case}: expected the two held names, got {before:?}"
    );

    // A second static IP, and a resource of a different kind entirely: both are
    // names the account holds, so both join the one flat list in name order.
    emulator
        .ok_at(
            None,
            Some(REGION),
            "AllocateStaticIp",
            json!({"staticIpName": "name-b"}),
        )
        .await;
    emulator
        .ok_at(
            None,
            Some(REGION),
            "CreateDisk",
            json!({"diskName": "name-d", "availabilityZone": "us-east-1a", "sizeInGb": 8}),
        )
        .await;
    let case = "after the later creates";
    let after = walk(&emulator, case, REGION).await.0;
    assert_eq!(
        after,
        vec![
            "name-a".to_string(),
            "name-b".to_string(),
            "name-c".to_string(),
            "name-d".to_string(),
        ],
        "case {case}: expected the new names in name order, got {after:?}"
    );
}
