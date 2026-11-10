// SPDX-License-Identifier: CC0-1.0

//! WHEN an association is stamped and what the clock owes it scheduled: at the instant of
//! the WRITE, after every cross-service hop the request awaited.
//!
//! `AssociateUser` resolves the EC2 instance its request names against the service that
//! OWNS instances, over the in-process callback — twice, once in the hoisted gate and once
//! in the handler itself — and this service's store is released across each of those hops.
//! The rule under test is the one every write of this crate obeys: the clock is read under
//! the lock that writes, so
//!
//! * `AssociationDate` reports the instant the association was stored, never an earlier
//!   reading;
//! * the `ASSOCIATING` -> `ASSOCIATED` transition the stored association owes the clock is
//!   measured from that same instant, so the whole of that span is still ahead of the
//!   caller when the answer arrives and the transient status the span exists for is
//!   observable;
//! * every transition the clock had already made due by then is materialised by this
//!   request, including one that fell due while its hop was in flight.
//!
//! The rows of the first test span what the rule generalises over: how long the hop takes,
//! from an answer at once to one that takes more than twice the span the association owes.
//! The second test places a transition the clock owes INSIDE the window the hops open.
//!
//! EC2 is mounted beside this service on ONE engine, with the instance question it answers
//! paced by the test, and every call goes through the router's `dispatch`, so each request
//! is deserialized, constraint-checked, handled and serialized exactly as a client's would
//! be.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use serde_json::json;
use smithy_server_runtime::{
    http, BoxFuture, EmbeddedEmulator, ErasedOperation, ErasedServiceDefinition,
    MultiServiceRouter, OperationContext, Service,
};

use service_com_amazonaws_ec2_amazonec2 as ec2;
use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::state::iso8601;
use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState as ServiceState;
use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::EmulatedService;

// ONE wire harness per service: the dispatch plumbing, the account's directory fleet and
// network, the reply reader and the user-association helpers come from the sibling suites
// rather than a second copy here.
use crate::identity_provider_subscriptions::{
    as_str, create_directories, create_network, list_providers, Harness, Reply,
    ACCOUNT_SECURITY_GROUPS, ACCOUNT_SUBNETS, ACCOUNT_VPC, PAST_THE_OWED_TRANSITION, REGION,
};
use crate::instance_fleet_resolution::{associate, list_associations, register_provider};
use crate::pending_transition_comes_with_the_record::{owed, summary_status};

/// The directory the reused `instance_fleet_resolution` helpers register and associate
/// through, which the harness below therefore puts in the account's directory fleet.
const DIRECTORY: &str = "d-0123456789";

/// The AMI a launch names. Nothing here asserts anything about the image.
const AMI: &str = "ami-00112233445566778";

/// The status a fresh association carries, and the one the span it owes brings it to.
/// Spelled out here rather than read from the production constants, so a change to them
/// fails a test instead of silently changing what this suite accepts.
const STATUS_ASSOCIATING: &str = "ASSOCIATING";
const STATUS_ASSOCIATED: &str = "ASSOCIATED";

/// How long the service that owns instances takes to answer the instance question, as the
/// test paces it.
///
/// Shared with the EC2 service mounted in the router, so a test can pace the hops of the
/// one call it measures and clear the pace again for the reads that follow it.
#[derive(Clone, Debug, Default)]
struct HopPace(Arc<AtomicU64>);

impl HopPace {
    fn set(&self, delay: Duration) {
        self.0.store(
            u64::try_from(delay.as_millis()).expect("a pace in whole milliseconds"),
            Ordering::SeqCst,
        );
    }

    fn delay(&self) -> Duration {
        Duration::from_millis(self.0.load(Ordering::SeqCst))
    }
}

/// The routing key of the ONE EC2 operation this service's instance resolution calls — it
/// asks `DescribeInstances` about one id (`state::ec2_fleet`).
const DESCRIBE_INSTANCES: &str = "DescribeInstances";

/// EC2's own `DescribeInstances`, answered once the paced delay has elapsed.
///
/// The ANSWER is EC2's: the request is handed to the operation EC2's own routing table
/// names, so what this service resolves is what the owner of instances reports. The delay
/// is the only thing added, and it stands for the time a cross-service hop takes.
struct PacedDescribeInstances {
    pace: HopPace,
}

impl ErasedOperation for PacedDescribeInstances {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        let delay = self.pace.delay();
        Box::pin(async move {
            tokio::time::sleep(delay).await;
            let operation = svc
                .definition()
                .routing_table()
                .get(DESCRIBE_INSTANCES)
                .expect("EC2 models DescribeInstances");
            operation.run(svc, req, ctx).await
        })
    }
}

/// Amazon EC2, mounted so the instance question this service asks it takes the time the
/// test paces. Everything else — which requests it claims, its protocol, its operations
/// and every answer — is EC2's own.
struct PacedEc2 {
    inner: ec2::EmulatedService<engine::InMemoryEngine>,
    paced: PacedDescribeInstances,
}

impl Service for PacedEc2 {
    fn definition(&self) -> &dyn ErasedServiceDefinition {
        self.inner.definition()
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        self.inner.can_handle(parts)
    }

    fn override_operation(&self, req: &RtRequest) -> Option<&dyn ErasedOperation> {
        let key = self
            .definition()
            .wire_protocol(req)
            .protocol()
            .routing_key(req)
            .ok()?;
        (key == DESCRIBE_INSTANCES).then_some(&self.paced as &dyn ErasedOperation)
    }
}

/// This service and the owner of instances over ONE engine, with the instance hop paced,
/// plus an EC2 client over the router's own in-process callback — the very seam this
/// service's instance resolution reaches EC2 through.
struct PacedFleet {
    h: Harness,
    ec2_client: ::aws_sdk_ec2::Client,
    pace: HopPace,
}

impl PacedFleet {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let pace = HopPace::default();
        let mut router = MultiServiceRouter::new();
        // The tests sign nothing; authentication is the framework's concern, not this
        // service's.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(PacedEc2 {
            inner: ec2::EmulatedService::new(Arc::clone(&engine)),
            paced: PacedDescribeInstances { pace: pace.clone() },
        });
        // `RegisterIdentityProvider` resolves a `DirectoryId` against the account's
        // directories, and the `Settings` of that registration against what the account has
        // in Amazon EC2, so both are in place before the registration is made.
        create_directories(&engine, &[DIRECTORY]);
        create_network(
            &engine,
            ACCOUNT_VPC,
            ACCOUNT_SUBNETS,
            ACCOUNT_SECURITY_GROUPS,
        );
        let callback = EmbeddedEmulator::from_router(router.clone());
        let config = ::aws_sdk_ec2::Config::builder()
            .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
            .region(::aws_sdk_ec2::config::Region::new(REGION.to_string()))
            .endpoint_url(format!("https://ec2.{REGION}.amazonaws.com"))
            .credentials_provider(callback.aws_credentials_provider())
            .http_client(callback.http_client())
            .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
            .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
            .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                ::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled(),
            )
            .build();
        Self {
            h: Harness { router, engine },
            ec2_client: ::aws_sdk_ec2::Client::from_conf(config),
            pace,
        }
    }

    /// Launch one instance through EC2 itself and answer the id it minted — an instance of
    /// the account that this service's own fleet seam does not hold, so resolving it is the
    /// hop this suite paces.
    async fn launch(&self) -> String {
        let launched = self
            .ec2_client
            .run_instances()
            .image_id(AMI)
            .min_count(1)
            .max_count(1)
            .send()
            .await
            .unwrap_or_else(|error| panic!("setup: EC2 must launch an instance: {error:?}"));
        launched
            .instances()
            .iter()
            .find_map(|instance| instance.instance_id().map(str::to_string))
            .unwrap_or_else(|| panic!("setup: EC2 must report the id of the instance it launched"))
    }

    /// Settle whatever the clock owes, so the only transition a later request has to
    /// materialise is the one the test staged.
    async fn settle(&self, case: &str) {
        tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
        list_providers(&self.h, &json!({})).await.ok(case);
    }
}

/// The instant the record `arn` names owes the clock its transition at, read off the record
/// itself (through the same trait the writers schedule by) rather than computed from a span
/// this suite knows.
fn due_at(h: &Harness, arn: &str, case: &str) -> SystemTime {
    let owed_now = owed(h);
    owed_now
        .iter()
        .find(|(held, _)| held == arn)
        .map(|(_, due)| *due)
        .unwrap_or_else(|| panic!("case {case:?}: {arn} must owe a transition, got {owed_now:?}"))
}

/// The `Status` a `ListUserAssociations` reply reports for the association `arn` names,
/// through the sibling suite's reader of a list reply.
fn association_status(reply: &Reply, arn: &str, case: &str) -> String {
    summary_status(reply, "InstanceUserSummaries", "InstanceUserArn", arn, case)
}

/// The status the store holds for the association `arn` names.
///
/// Read from the store rather than over the wire because any request of this service would
/// run the sweep itself, which is the very thing the second test is asserting about.
fn stored_status(h: &Harness, arn: &str, case: &str) -> String {
    let store = h.engine.state::<ServiceState>();
    let held = store.lock();
    held.instance_users
        .get(arn)
        .map(|record| record.status.clone())
        .unwrap_or_else(|| panic!("case {case:?}: the store must hold the association {arn}"))
}

/// The user each row's association is made for.
const USERNAME: &str = "write.time.user";

/// One hop duration to drive the rule on.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    hop: Duration,
}

/// The rows differ along the one thing the rule generalises over: how long the hop the
/// request awaits takes, relative to the span the association it stores owes the clock.
const ROWS: &[Row] = &[
    Row {
        case: "an instance question answered at once",
        hop: Duration::ZERO,
    },
    Row {
        case: "a hop well inside the span the association owes",
        hop: Duration::from_millis(150),
    },
    Row {
        case: "a hop longer than that span",
        hop: Duration::from_millis(600),
    },
    Row {
        case: "a hop of more than twice that span",
        hop: Duration::from_millis(1200),
    },
];

/// RULE: however long the hop before the write takes, the association is stamped with the
/// instant it was STORED and the transition it owes the clock is measured from that same
/// instant — so `AssociationDate` is no earlier than the hops the request awaited and no
/// later than the answer, the whole span is still ahead of the caller when the answer
/// arrives, and the transient status the span exists for is what a read straight back
/// reports.
#[tokio::test]
async fn the_association_is_stamped_and_scheduled_at_the_instant_it_is_stored() {
    for row in ROWS {
        let case = row.case;
        // A harness per row, so the pace one row drives cannot reach another's records.
        let fleet = PacedFleet::new();
        register_provider(&fleet.h).await;
        let instance = fleet.launch().await;

        // Only the association's own hops are paced: the launch and the registration are
        // the setup this row needs, not what it measures.
        fleet.pace.set(row.hop);
        let started = SystemTime::now();
        let reply = associate(&fleet.h, USERNAME, &instance).await;
        let answered = SystemTime::now();
        fleet.pace.set(Duration::ZERO);

        let body = reply.ok(case);
        let summary = &body["InstanceUserSummary"];
        let arn = as_str(summary, "InstanceUserArn", case).to_string();
        let status = as_str(summary, "Status", case);
        assert_eq!(
            status, STATUS_ASSOCIATING,
            "case {case:?}: a fresh association is answered {STATUS_ASSOCIATING}, got {status}"
        );

        // THE STAMP: the instant of the write, which is after BOTH hops the request awaited
        // — the hoisted gate's and the handler's own — and no later than the answer. The
        // member carries whole seconds, so the bounds are compared in the form the record
        // stores.
        let association_date = as_str(summary, "AssociationDate", case);
        let after_both_hops = iso8601(started + row.hop * 2);
        assert!(
            association_date >= after_both_hops.as_str(),
            "case {case:?}: expected an AssociationDate at or after {after_both_hops}, the \
             instant both of the request's hops had elapsed, got {association_date}"
        );
        let by_the_answer = iso8601(answered);
        assert!(
            association_date <= by_the_answer.as_str(),
            "case {case:?}: expected an AssociationDate no later than {by_the_answer}, when \
             the answer arrived, got {association_date}"
        );

        // THE SCHEDULE: measured from the write too, so the whole span is still ahead of the
        // caller when the answer arrives.
        let due = due_at(&fleet.h, &arn, case);
        assert!(
            due > answered,
            "case {case:?}: expected the transition the association owes to be still ahead \
             when the answer arrived at {answered:?}, got {due:?}"
        );

        // Which is why the transient status is observable at all: a read straight back
        // reports it, and the transition is still owed rather than lost.
        let listed = list_associations(&fleet.h, &instance).await;
        let reported = association_status(&listed, &arn, case);
        assert_eq!(
            reported, STATUS_ASSOCIATING,
            "case {case:?}: expected {STATUS_ASSOCIATING} on a read straight back, got \
             {reported}"
        );
        tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
        let listed = list_associations(&fleet.h, &instance).await;
        let reported = association_status(&listed, &arn, case);
        assert_eq!(
            reported, STATUS_ASSOCIATED,
            "case {case:?}: expected {STATUS_ASSOCIATED} once the span had elapsed, got \
             {reported}"
        );
    }
}

/// How long after the staged association the paced `AssociateUser` is fired, and how long
/// each of that request's hops takes.
///
/// Together they place the staged association's own transition INSIDE the window the hops
/// open: the request starts a stagger after the staged association was stored, its handler
/// runs one hop later and its write a second hop after that, so a transition due a span
/// after the staging falls due between the two. The test checks that placement against the
/// due instant the service itself recorded, so a machine too slow to hit the window fails
/// loudly instead of passing for the wrong reason.
const STAGGER: Duration = Duration::from_millis(100);
const PACED_HOP: Duration = Duration::from_millis(300);

/// The two users of the second test: the one whose association is left owing a transition,
/// and the one the paced request associates.
const STAGED_USERNAME: &str = "staged.user";
const WRITTEN_USERNAME: &str = "written.user";

/// RULE, the other half: the sweep a writing request runs is run against the instant it
/// writes, so a transition the clock made due while that request's hop was in flight is
/// materialised by it — a request that writes to the store leaves nothing the clock already
/// owed unapplied.
#[tokio::test]
async fn a_transition_the_clock_made_due_during_the_hop_is_materialised_by_the_write() {
    let case = "a transition that falls due while the hop is in flight";
    let fleet = PacedFleet::new();
    register_provider(&fleet.h).await;
    let instance = fleet.launch().await;
    // The registration owes the clock a transition of its own: it is settled first, so the
    // only transition the paced request below has to materialise is the staged one.
    fleet.settle(case).await;

    // The staged association, stored while the hop is instant, so the instant it falls due
    // is decided by its own span alone.
    let staged = associate(&fleet.h, STAGED_USERNAME, &instance).await;
    let staged_arn = as_str(
        &staged.ok(case)["InstanceUserSummary"],
        "InstanceUserArn",
        case,
    )
    .to_string();
    let staged_due = due_at(&fleet.h, &staged_arn, case);

    // No request runs in the stagger, so nothing sweeps: the staged transition falls due
    // with the store untouched.
    tokio::time::sleep(STAGGER).await;
    fleet.pace.set(PACED_HOP);
    let started = SystemTime::now();
    let written = associate(&fleet.h, WRITTEN_USERNAME, &instance).await;
    fleet.pace.set(Duration::ZERO);
    let written_arn = as_str(
        &written.ok(case)["InstanceUserSummary"],
        "InstanceUserArn",
        case,
    )
    .to_string();

    // The placement this test needs: the write cannot have happened before both of the
    // request's hops had elapsed, and the staged transition fell due before that.
    assert!(
        started + PACED_HOP * 2 > staged_due,
        "case {case:?}: the staged transition must fall due before the write — it was due \
         {staged_due:?} and the request started at {started:?}"
    );

    // THE RULE: the request swept at the instant it wrote, so the transition the clock had
    // made due while its hop was in flight is materialised.
    let staged_status = stored_status(&fleet.h, &staged_arn, case);
    assert_eq!(
        staged_status, STATUS_ASSOCIATED,
        "case {case:?}: expected the staged association to be {STATUS_ASSOCIATED}, \
         materialised by the request that wrote after it fell due, got {staged_status}"
    );

    // And the association that request stored is the fresh one, with its own span still
    // ahead of it rather than already elapsed.
    let written_status = stored_status(&fleet.h, &written_arn, case);
    assert_eq!(
        written_status, STATUS_ASSOCIATING,
        "case {case:?}: expected the association just stored to be {STATUS_ASSOCIATING}, got \
         {written_status}"
    );
    let written_due = due_at(&fleet.h, &written_arn, case);
    assert!(
        written_due > staged_due,
        "case {case:?}: expected the stored association's transition to be scheduled from \
         its own write, after the staged one's {staged_due:?}, got {written_due:?}"
    );
}
