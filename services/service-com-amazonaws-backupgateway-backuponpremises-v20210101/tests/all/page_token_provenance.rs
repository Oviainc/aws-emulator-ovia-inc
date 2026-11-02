// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the PROVENANCE of a `NextToken` on every paginated listing of
//! this service: `ListGateways`, `ListHypervisors` and `ListVirtualMachines`.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the `MultiServiceRouter`
//! — the harness of `gateway_lifecycle` — so what is asserted is the wire behaviour.
//!
//! The rule under test: a listing resumes only a `NextToken` this service really ISSUED
//! for that listing — a page boundary it minted, in the region the listing reports — and
//! refuses every other string with HTTP 400 `ValidationException`, never by restarting
//! the listing from the top. Being well FORMED is not being issued: an ARN of the right
//! shape that no record was ever stored under would bound the range at a key nothing
//! sorts below, so accepting it hands back page one plus a fresh token and a client that
//! mangled a token loops over page one for ever.
//!
//! The tables below span that class rather than one forged string: all three listings,
//! each with tokens differing along every segment of the ARN a boundary encodes
//! (partition, region, account, resource id) and along the SPELLING of a minted id
//! (unpadded, over-padded, another prefix, a sequence past every one minted), plus a
//! token one region's listing issued replayed against another region's, and — on the
//! other side of the rule — a token whose boundary record has since been deleted, which
//! still resumes.

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

/// A second region to seed, so a token can be replayed against a listing that did not
/// issue it. Its name sorts BELOW the harness's `us-west-2`, which is what makes a
/// cross-region boundary able to include a whole listing.
const OTHER_REGION: &str = "eu-central-1";

/// One paginated listing of this service.
#[derive(Debug, Clone, Copy)]
struct Listing {
    operation: &'static str,
    /// The output member the records arrive in.
    list_member: &'static str,
    /// The member of a record carrying its ARN.
    arn_member: &'static str,
    /// The ARN resource type of the records it lists — the `gateway` of
    /// `arn:aws:backup-gateway:<region>:<account>:gateway/<id>`.
    resource_type: &'static str,
    /// The id prefix this service mints a record of that type under, as seen in the ARNs
    /// it hands out: asserted against a minted ARN in every test below, so this table
    /// cannot drift from what the service mints.
    id_prefix: &'static str,
    /// How many hex digits the minted id of that type carries behind its prefix — the
    /// eight of `hypervisor/hype-00000000`, the sixteen of
    /// `gateway/bgw-0000000000000000`. Part of the identifier form of the RESOURCE, so it
    /// sits beside the prefix and is asserted against a minted ARN in every test below.
    id_digits: usize,
}

const LISTINGS: [Listing; 3] = [
    Listing {
        operation: "ListGateways",
        list_member: "Gateways",
        arn_member: "GatewayArn",
        resource_type: "gateway",
        id_prefix: "bgw-",
        id_digits: 16,
    },
    Listing {
        operation: "ListHypervisors",
        list_member: "Hypervisors",
        arn_member: "HypervisorArn",
        resource_type: "hypervisor",
        id_prefix: "hype-",
        id_digits: 8,
    },
    Listing {
        operation: "ListVirtualMachines",
        list_member: "VirtualMachines",
        arn_member: "ResourceArn",
        resource_type: "vm",
        id_prefix: "vm-",
        id_digits: 16,
    },
];

/// The ARNs one region's records are stored under, by resource kind — in the order the
/// seed created them.
#[derive(Debug, Default, Clone)]
struct Seeded {
    gateways: Vec<String>,
    hypervisors: Vec<String>,
    machines: Vec<String>,
}

impl Seeded {
    /// The ARNs of the records `listing` reports.
    fn of(&self, listing: &Listing) -> &[String] {
        match listing.operation {
            "ListGateways" => &self.gateways,
            "ListHypervisors" => &self.hypervisors,
            _ => &self.machines,
        }
    }
}

/// An ARN of this service, spelled out segment by segment rather than taken from the
/// crate, so a forged token is independent of the code that mints a real one. The form is
/// the modelled `ServerArn` pattern's:
/// `^arn:(aws|aws-cn|aws-us-gov):backup-gateway(:[a-zA-Z-0-9]+){3}\/[a-zA-Z-0-9]+$`
/// (`smithy_query(query="shape",shape="ServerArn")`).
fn arn(partition: &str, region: &str, account: &str, resource_type: &str, id: &str) -> String {
    format!("arn:{partition}:backup-gateway:{region}:{account}:{resource_type}/{id}")
}

/// A minted id as this service renders one: the prefix of its resource type followed by a
/// sequence in that resource's own hex width, the form every ARN the service hands out
/// carries (asserted below).
fn minted_id(listing: &Listing, sequence: u64) -> String {
    format!(
        "{}{sequence:0width$x}",
        listing.id_prefix,
        width = listing.id_digits
    )
}

/// Seed `region` with `count` gateways, `count` hypervisors and `count` virtual machines
/// — enough, at two or more, for every listing of that region to hand out a `NextToken`
/// at `MaxResults=1`, and the number of records is what fixes the sequences the service
/// mints in that region.
async fn seed(
    engine: &Arc<engine::InMemoryEngine>,
    router: &MultiServiceRouter,
    region: &str,
    count: usize,
) -> Seeded {
    let mut seeded = Seeded::default();
    for index in 0..count {
        let activation_key = issued_activation_key_for(router, region).await;
        let reply = call_in_region(
            router,
            region,
            "CreateGateway",
            &create_body(
                &format!("gw-{region}-{index}"),
                &activation_key,
                &BTreeMap::new(),
            ),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case seed {region}/gateway-{index}: CreateGateway must answer 200, got {} {:?}",
            reply.status, reply.body
        );
        seeded.gateways.push(reply.gateway_arn().to_string());
    }

    for index in 0..count {
        let reply = call_in_region(
            router,
            region,
            "ImportHypervisorConfiguration",
            &json!({
                "Name": format!("hv-{index}"),
                // A record stands for the appliance at its host, so each gets its own.
                "Host": format!("{region}-hv-{index}.example.internal"),
            }),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case seed {region}/hypervisor-{index}: ImportHypervisorConfiguration must \
             answer 200, got {} {:?}",
            reply.status, reply.body
        );
        seeded.hypervisors.push(
            reply
                .str_at("/HypervisorArn")
                .unwrap_or_else(|| panic!("no HypervisorArn in {:?}", reply.body))
                .to_string(),
        );
    }

    // A virtual machine is minted by no operation of this model: it reaches the service as
    // part of the inventory of the hypervisor it runs on, which is the channel
    // `record_virtual_machine` stands for — in that hypervisor's own region.
    let hypervisor_arn = seeded
        .hypervisors
        .first()
        .expect("the seed imported a hypervisor")
        .clone();
    let state = service_state(&**engine).expect("the in-memory engine must expose this state");
    let mut store = state.lock();
    for index in 0..count {
        let recorded = store
            .record_virtual_machine(
                region,
                &hypervisor_arn,
                VirtualMachineInventory {
                    name: format!("vm-{index}"),
                    host_name: format!("vm-{index}"),
                    path: format!("/Datacenter/vm/{region}/{index}"),
                    last_backup_date: None,
                    vmware_tags: Vec::new(),
                },
                SystemTime::now(),
            )
            .unwrap_or_else(|| {
                panic!("case seed {region}/machine-{index}: the store must record a machine")
            });
        seeded.machines.push(recorded.arn);
    }
    seeded
}

/// The ARNs a listing reply carries, read off the members `listing` names them in.
fn listed_arns(reply: &Reply, listing: &Listing) -> Vec<String> {
    reply
        .body
        .get(listing.list_member)
        .and_then(Value::as_array)
        .map(|records| {
            records
                .iter()
                .map(|record| {
                    record
                        .get(listing.arn_member)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn list(
    router: &MultiServiceRouter,
    region: &str,
    listing: &Listing,
    body: &Value,
) -> Reply {
    call_in_region(router, region, listing.operation, body).await
}

/// Assert that `listing` really mints ARNs of the form the forged tokens below are built
/// from: `<resource_type>/<id_prefix><id_digits hex digits>`.
fn assert_minted_form(listing: &Listing, region: &str, minted: &str) {
    let expected = format!(
        ":{region}:{}:{}/",
        engine::EMULATOR_ACCOUNT_ID, listing.resource_type
    );
    assert!(
        minted.starts_with("arn:aws:backup-gateway:") && minted.contains(&expected),
        "case {}: a minted ARN must name this service, the addressed region and the \
         resource type, got {minted:?}",
        listing.operation
    );
    let id = minted
        .rsplit('/')
        .next()
        .unwrap_or_else(|| panic!("case {}: {minted:?} carries no resource id", listing.operation));
    let digits = id.strip_prefix(listing.id_prefix).unwrap_or_else(|| {
        panic!(
            "case {}: a minted id must carry the {:?} prefix, got {id:?}",
            listing.operation, listing.id_prefix
        )
    });
    assert!(
        digits.len() == listing.id_digits && digits.chars().all(|c| c.is_ascii_hexdigit()),
        "case {}: a minted id must carry a {}-hex-digit sequence, got {id:?}",
        listing.operation,
        listing.id_digits
    );
}

/// The forged `NextToken`s for `listing`: strings that satisfy the modelled `NextToken`
/// constraints (len 1..1000, pattern `^.+$`, so each reaches the handler rather than the
/// boundary validator) and encode a boundary this service never issued for this listing.
///
/// The rows differ along every segment of the boundary ARN and along the spelling of a
/// minted id, which is what the rule generalises over; the second row is a boundary that
/// sorts BELOW every stored key, the shape that used to be answered with page one.
fn forged_tokens(listing: &Listing) -> Vec<(String, String)> {
    let account = engine::EMULATOR_ACCOUNT_ID;
    let resource_type = listing.resource_type;
    let prefix = listing.id_prefix;
    // A sequence no run of this test can reach, so the id is well formed and unminted.
    let unminted = minted_id(listing, u64::MAX);
    // The first sequence this service mints, which the seed has reached, and its digits.
    let minted = minted_id(listing, 0);
    let digits = minted
        .strip_prefix(prefix)
        .expect("a minted id carries its resource type's prefix")
        .to_string();
    vec![
        (
            "a minted spelling of a sequence past every one minted".to_string(),
            arn("aws", REGION, account, resource_type, &unminted),
        ),
        (
            "a well-formed ARN sorting below every stored key".to_string(),
            arn("aws", "a", "0", resource_type, "0"),
        ),
        (
            "a minted sequence in a region that sorts below the listing's".to_string(),
            arn("aws", OTHER_REGION, account, resource_type, &minted),
        ),
        (
            "a minted sequence of another account".to_string(),
            arn("aws", REGION, "999999999999", resource_type, &minted),
        ),
        (
            "a minted sequence in another partition".to_string(),
            arn("aws-cn", REGION, account, resource_type, &minted),
        ),
        (
            "a minted sequence spelled unpadded".to_string(),
            arn("aws", REGION, account, resource_type, &format!("{prefix}0")),
        ),
        (
            "a minted sequence with an extra padding digit".to_string(),
            arn(
                "aws",
                REGION,
                account,
                resource_type,
                &format!("{prefix}0{digits}"),
            ),
        ),
        (
            "a minted sequence under another id prefix".to_string(),
            arn(
                "aws",
                REGION,
                account,
                resource_type,
                &format!("zz-{digits}"),
            ),
        ),
    ]
    .into_iter()
    .map(|(case, boundary)| (format!("{case} ({boundary})"), hex::encode(boundary)))
    .collect()
}

/// A `NextToken` this service never ISSUED is refused with 400 `ValidationException` on
/// every listing — including one whose boundary is a well-formed ARN of the listing's own
/// resource type, which must not be answered with page one and a fresh token.
#[tokio::test]
async fn a_next_token_this_service_never_issued_is_refused() {
    let engine = build_engine();
    let router = build_router(&engine);
    let seeded = seed(&engine, &router, REGION, 2).await;

    for listing in LISTINGS {
        let records = seeded.of(&listing);
        assert!(
            records.len() >= 2,
            "case {}: the seed must store at least two records for a token to mean \
             anything, got {records:?}",
            listing.operation
        );
        assert_minted_form(&listing, REGION, &records[0]);

        // Page one, for the comparison every refusal below is judged against: a refused
        // listing must not be answered with these records.
        let page_one = list(&router, REGION, &listing, &json!({ "MaxResults": 1 })).await;
        let issued = page_one.next_token().unwrap_or_else(|| {
            panic!(
                "case {}: a truncated page must carry a NextToken, got {:?}",
                listing.operation, page_one.body
            )
        });
        let first_page_arns = listed_arns(&page_one, &listing);

        for (case, token) in forged_tokens(&listing) {
            let refused = list(
                &router,
                REGION,
                &listing,
                &json!({ "MaxResults": 1, "NextToken": token }),
            )
            .await;
            assert_eq!(
                (refused.status, refused.code.as_deref()),
                (400, Some("ValidationException")),
                "case {} / {case}: a NextToken this service never issued must be refused \
                 with 400 ValidationException, got {} {:?}",
                listing.operation,
                refused.status,
                refused.body
            );
            assert!(
                refused
                    .str_at("/Message")
                    .is_some_and(|stated| stated.contains("at 'nextToken'")),
                "case {} / {case}: the refusal must name the member it refused, at its \
                 validation path, got {:?}",
                listing.operation,
                refused.body
            );
            let served = listed_arns(&refused, &listing);
            assert!(
                served.is_empty(),
                "case {} / {case}: a refused listing must serve no records — least of all \
                 page one ({first_page_arns:?}) — got {served:?}",
                listing.operation
            );
            assert_eq!(
                refused.next_token(),
                None,
                "case {} / {case}: a refused listing must promise no successor, got {:?}",
                listing.operation,
                refused.body
            );
        }

        // The token this listing DID issue still resumes after its boundary, so the
        // refusals above are about provenance and not about the operation.
        let resumed = list(
            &router,
            REGION,
            &listing,
            &json!({ "MaxResults": 1, "NextToken": issued }),
        )
        .await;
        let mut expected = records.to_vec();
        expected.sort();
        assert_eq!(
            (resumed.status, listed_arns(&resumed, &listing)),
            (200, vec![expected[1].clone()]),
            "case {}: a token this listing issued must resume after the boundary it \
             encodes, got {} {:?}",
            listing.operation,
            resumed.status,
            resumed.body
        );
    }
}

/// A token one region's listing issued does not resume another region's listing of the
/// same resource: the records of another region are not part of this listing, so the
/// boundary it encodes is not one this listing ever handed out — and it must not be
/// answered with that region's page one.
#[tokio::test]
async fn a_token_one_regions_listing_issued_does_not_resume_anothers() {
    let engine = build_engine();
    let router = build_router(&engine);
    let here = seed(&engine, &router, REGION, 2).await;
    let there = seed(&engine, &router, OTHER_REGION, 2).await;

    for listing in LISTINGS {
        assert_minted_form(&listing, REGION, &here.of(&listing)[0]);
        assert_minted_form(&listing, OTHER_REGION, &there.of(&listing)[0]);

        // (the region whose listing issued the token, the region it is replayed against)
        for (issuing_region, replay_region) in [(REGION, OTHER_REGION), (OTHER_REGION, REGION)] {
            let page_one = list(
                &router,
                issuing_region,
                &listing,
                &json!({ "MaxResults": 1 }),
            )
            .await;
            let token = page_one.next_token().unwrap_or_else(|| {
                panic!(
                    "case {} in {issuing_region}: a truncated page must carry a NextToken, \
                     got {:?}",
                    listing.operation, page_one.body
                )
            });
            let replayed = list(
                &router,
                replay_region,
                &listing,
                &json!({ "MaxResults": 1, "NextToken": token }),
            )
            .await;
            assert_eq!(
                (replayed.status, replayed.code.as_deref()),
                (400, Some("ValidationException")),
                "case {}: a token {issuing_region} issued must not resume the \
                 {replay_region} listing, got {} {:?}",
                listing.operation,
                replayed.status,
                replayed.body
            );
            let served = listed_arns(&replayed, &listing);
            assert!(
                served.is_empty(),
                "case {}: a token {issuing_region} issued must serve no {replay_region} \
                 records, got {served:?}",
                listing.operation
            );
        }
    }
}

/// Which sequences a listing will resume is scoped to the REGION that minted them: one
/// store serves every region, so a boundary whose sequence another region minted — even
/// spelled with this listing's own region, account and partition — is one this listing
/// never handed out, and is refused rather than answered with a page.
///
/// The table gives the two regions DIFFERENT record counts and walks every sequence up to
/// one past the larger, so each row is accepted exactly when the region under test has
/// minted that sequence, and an accepted boundary hands back precisely the records of that
/// region sorting above it.
#[tokio::test]
async fn which_sequences_resume_is_scoped_to_the_region_that_minted_them() {
    let engine = build_engine();
    let router = build_router(&engine);
    // (region, how many records of each kind it holds)
    let counts: [(&str, usize); 2] = [(REGION, 3), (OTHER_REGION, 5)];
    let mut seeded = Vec::new();
    for (region, count) in counts {
        seeded.push((region, count, seed(&engine, &router, region, count).await));
    }
    let highest = counts
        .iter()
        .map(|(_, count)| *count)
        .max()
        .expect("the table holds a region");

    for listing in LISTINGS {
        for (region, count, records) in &seeded {
            let arns = records.of(&listing);
            assert_eq!(
                arns.len(),
                *count,
                "case {} in {region}: the seed must store {count} records, got {arns:?}",
                listing.operation
            );
            assert_minted_form(&listing, region, &arns[0]);

            for sequence in 0..=(highest as u64) {
                let boundary = arn(
                    "aws",
                    region,
                    engine::EMULATOR_ACCOUNT_ID,
                    listing.resource_type,
                    &minted_id(&listing, sequence),
                );
                let reply = list(
                    &router,
                    region,
                    &listing,
                    &json!({ "NextToken": hex::encode(&boundary) }),
                )
                .await;
                let minted_here = (sequence as usize) < *count;
                if minted_here {
                    // A boundary this region's own listing handed out: the page after it
                    // is this region's records sorting above it, and nothing else.
                    let expected: Vec<String> = arns[(sequence as usize + 1)..].to_vec();
                    assert_eq!(
                        (reply.status, listed_arns(&reply, &listing)),
                        (200, expected),
                        "case {} in {region} sequence {sequence} ({boundary}): a boundary \
                         this region minted must resume after it, got {} {:?}",
                        listing.operation,
                        reply.status,
                        reply.body
                    );
                } else {
                    assert_eq!(
                        (reply.status, reply.code.as_deref()),
                        (400, Some("ValidationException")),
                        "case {} in {region} sequence {sequence} ({boundary}): a sequence \
                         {region} never minted must be refused, got {} {:?}",
                        listing.operation,
                        reply.status,
                        reply.body
                    );
                    let served = listed_arns(&reply, &listing);
                    assert!(
                        served.is_empty(),
                        "case {} in {region} sequence {sequence}: a refused listing must \
                         serve no records, got {served:?}",
                        listing.operation
                    );
                }
            }
        }
    }

    // The table means nothing unless the two regions really minted different sequences:
    // the sequences only the larger region reached are the ones the smaller must refuse.
    assert!(
        counts[0].1 < counts[1].1,
        "case table: the two regions must hold different record counts, got {counts:?}"
    );
}

/// A token whose BOUNDARY RECORD has since been deleted still resumes the listing: a
/// boundary this service issued stays issued, so the page after it is the records sorting
/// above it — the deleted record simply is not among them.
#[tokio::test]
async fn a_token_whose_boundary_record_was_deleted_still_resumes() {
    let engine = build_engine();
    let router = build_router(&engine);
    let seeded = seed(&engine, &router, REGION, 2).await;
    let listing = LISTINGS[0];
    let mut gateways = seeded.gateways.clone();
    gateways.sort();

    let page_one = list(&router, REGION, &listing, &json!({ "MaxResults": 1 })).await;
    assert_eq!(
        listed_arns(&page_one, &listing),
        vec![gateways[0].clone()],
        "page one must hold the first gateway in ARN order, got {:?}",
        page_one.body
    );
    let token = page_one
        .next_token()
        .unwrap_or_else(|| panic!("a truncated page must carry a NextToken, got {:?}", page_one.body));

    // The boundary record itself is removed, which is exactly what a client holding a
    // token cannot know about.
    let deleted = call_in_region(
        &router,
        REGION,
        "DeleteGateway",
        &json!({ "GatewayArn": gateways[0] }),
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "case delete the boundary: DeleteGateway must answer 200, got {} {:?}",
        deleted.status, deleted.body
    );

    let resumed = list(
        &router,
        REGION,
        &listing,
        &json!({ "NextToken": token.clone() }),
    )
    .await;
    assert_eq!(
        (resumed.status, listed_arns(&resumed, &listing)),
        (200, gateways[1..].to_vec()),
        "case delete the boundary: the page after a deleted boundary must hold the \
         records sorting above it, got {} {:?}",
        resumed.status,
        resumed.body
    );
    assert_eq!(
        resumed.next_token(),
        None,
        "case delete the boundary: a final page must promise no successor, got {:?}",
        resumed.body
    );
}
