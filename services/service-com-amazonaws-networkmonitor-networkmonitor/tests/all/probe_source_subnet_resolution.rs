// SPDX-License-Identifier: CC0-1.0

//! A probe's `sourceArn` is resolved against the subnets the ACCOUNT really
//! holds.
//!
//! The rule under test, one level above the cited example: a member that names
//! ANOTHER service's resource is resolved against that service, so a well-formed
//! identifier it holds no such resource for is refused with the modelled
//! `ValidationException` (HTTP 400) and nothing is stored, while the facts that
//! service reports about a resolved resource are read OFF its answer rather than
//! invented. For CloudWatch Network Monitor: `ProbeInput$sourceArn` is "The ARN
//! of the subnet"
//! (`smithy_query(query="doc",shape="ProbeInput",member="sourceArn")`) and
//! `Probe$vpcId` is "The ID of the source VPC subnet", so every operation that
//! TAKES a `sourceArn` — `CreateProbe` and `CreateMonitor`'s inline `probes`
//! list — resolves the subnet the ARN names against Amazon EC2, which owns the
//! account's subnets, refuses a source EC2 holds no subnet for, and reports the
//! VPC EC2 holds that subnet in. `UpdateProbeInput` binds no `sourceArn`
//! (`smithy_query(query="operation",operation="UpdateProbe")`), so an update
//! names no subnet to resolve and is refused on nothing.
//!
//! Every case drives real HTTP through a router carrying BOTH services on ONE
//! shared engine, and every subnet a case names is created through the Amazon
//! EC2 API itself ([`subnet_in`]) — so each value asserted against is one that
//! service reported rather than one this suite made up.
//!
//! The rows differ along what the rule generalises over: whether the account
//! holds the named subnet (never created, created then deleted, one character
//! off a held id), which published EC2 id form the absent id takes (8 and 17
//! characters), which VPC the held subnet sits in, how the ARN is otherwise
//! spelled (the partition and the region, which are not part of a subnet's
//! identity), which write path takes the source (`CreateProbe`, `CreateMonitor`'s
//! inline list), how long that list is and which position in it names the absent
//! subnet. The last case is an emulator with NO Amazon EC2 mounted, where
//! nothing is known about any subnet and so nothing is refused.

use std::sync::Arc;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use super::monitor_and_probe::{
    create_monitor, create_probe, error_code, error_message, get_monitor, probes_of, string_at,
    ACCOUNT, REGION,
};

/// The Amazon EC2 API version a client sends in an ec2Query body.
const EC2_API_VERSION: &str = "2016-11-15";

/// A router carrying this service AND Amazon EC2, which owns the account's VPC
/// subnets, on ONE shared engine — the arrangement the rule needs, since a
/// handler reaches EC2 through the in-process callback into this same emulator.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These cases exercise behaviour, not signing: every request below is
    // unsigned, exactly as the sibling suites drive this service.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_networkmonitor_networkmonitor::EmulatedService::new(engine.clone()),
    );
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        engine,
    ));
    router
}

/// Percent-encode the characters that are significant in an
/// `application/x-www-form-urlencoded` body.
fn form_value(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '&' => "%26".to_string(),
            '=' => "%3D".to_string(),
            '+' => "%2B".to_string(),
            '%' => "%25".to_string(),
            ' ' => "%20".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// Drive one ec2Query action as a client signing for [`REGION`]: `POST /` with
/// the parameters in the form body. Answers the HTTP status and the XML text.
async fn call_ec2(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={EC2_API_VERSION}");
    for (name, value) in params {
        body.push_str(&format!("&{name}={}", form_value(value)));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://ec2.{REGION}.amazonaws.com/"))
        .header("host", format!("ec2.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        // The SigV4 credential scope is what routes the request to `ec2` and
        // what names the Region the subnet is created in.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}/ec2/aws4_request, \
                 SignedHeaders=host, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("the request builds");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Drive one ec2Query action that must succeed and answer its XML body.
async fn ec2_ok(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> String {
    let (status, xml) = call_ec2(router, action, params).await;
    assert_eq!(
        status, 200,
        "setup: EC2 {action} must succeed, got {status} {xml}"
    );
    xml
}

/// The text of the first `<name>…</name>` element of an XML body.
fn element(xml: &str, name: &str) -> String {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = xml
        .find(&open)
        .unwrap_or_else(|| panic!("no <{name}> in {xml}"))
        + open.len();
    let end = xml[start..]
        .find(&close)
        .unwrap_or_else(|| panic!("no </{name}> in {xml}"))
        + start;
    xml[start..end].to_string()
}

/// One VPC of the account, created through Amazon EC2 itself.
async fn vpc_with(router: &MultiServiceRouter, cidr_block: &str) -> String {
    element(
        &ec2_ok(router, "CreateVpc", &[("CidrBlock", cidr_block)]).await,
        "vpcId",
    )
}

/// One subnet of the account, as Amazon EC2 reported it when this suite created
/// it: its id, the ARN that service minted for it, and the VPC it put it in.
#[derive(Debug, Clone)]
struct Subnet {
    subnet_id: String,
    subnet_arn: String,
    vpc_id: String,
}

/// Create one subnet in `vpc_id` through Amazon EC2 itself and answer what that
/// service reported for it.
async fn subnet_in(
    router: &MultiServiceRouter,
    vpc_id: &str,
    cidr_block: &str,
    zone: &str,
) -> Subnet {
    let xml = ec2_ok(
        router,
        "CreateSubnet",
        &[
            ("VpcId", vpc_id),
            ("CidrBlock", cidr_block),
            ("AvailabilityZone", zone),
        ],
    )
    .await;
    Subnet {
        subnet_id: element(&xml, "subnetId"),
        subnet_arn: element(&xml, "subnetArn"),
        vpc_id: element(&xml, "vpcId"),
    }
}

/// The ARN of the subnet `subnet_id` names, spelled with `partition` and
/// `region` rather than with the ones the account's own ARN carries.
///
/// A subnet's identity is its ID, so these are other spellings of ONE subnet's
/// ARN (the form the service requires of a `sourceArn`:
/// `arn:<partition>:ec2:<region>:<account>:subnet/<subnetId>`).
fn arn_spelled(partition: &str, region: &str, subnet_id: &str) -> String {
    format!("arn:{partition}:ec2:{region}:{ACCOUNT}:subnet/{subnet_id}")
}

/// One probe body sourced from `source_arn`, with a destination of its own so no
/// two probes of a case are the same request twice.
fn probe_from(source_arn: &str, destination_host: usize) -> String {
    format!(
        r#"{{"sourceArn":"{source_arn}","destination":"203.0.113.{destination_host}",
             "protocol":"ICMP","packetSize":56}}"#
    )
}

/// Create the monitor `name` holding no probes, for a case that adds them one at
/// a time.
async fn monitor_named(router: &MultiServiceRouter, case: &str, name: &str) {
    let body = format!(r#"{{"monitorName":"{name}","aggregationPeriod":30}}"#);
    let (status, created) = create_monitor(router, &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: setup CreateMonitor must succeed, got {status} {created:?}"
    );
}

/// What this service says for a `sourceArn` naming no subnet of the account: one
/// sentence naming the subnet by the id the ARN carried.
fn absent_subnet_message(subnet_id: &str) -> String {
    format!("The specified subnet {subnet_id} does not exist or could not be found.")
}

/// Assert `body` is the refusal of a source subnet the account does not hold.
fn assert_absent_refusal(case: &str, status: u16, body: &Value, subnet_id: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 for a source the account holds no subnet for, \
         got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {body:?}"
    );
    let expected = absent_subnet_message(subnet_id);
    assert_eq!(
        error_message(body),
        expected,
        "case {case:?}: expected the refusal to say {expected:?}, got {body:?}"
    );
}

/// The `vpcId` a probe body reports, or `"<none>"` when it reports none.
fn vpc_of(probe: &Value) -> String {
    probe
        .get("vpcId")
        .and_then(Value::as_str)
        .unwrap_or("<none>")
        .to_string()
}

/// A `sourceArn` naming a subnet the account HOLDS is accepted, and the probe
/// reports the VPC Amazon EC2 holds that subnet in — whichever way the ARN is
/// otherwise spelled.
#[tokio::test]
async fn a_held_subnet_is_accepted_and_reports_the_vpc_it_sits_in() {
    let router = emulator();
    let first_vpc = vpc_with(&router, "10.80.0.0/16").await;
    let second_vpc = vpc_with(&router, "10.90.0.0/16").await;
    let first = subnet_in(&router, &first_vpc, "10.80.1.0/24", "us-west-2a").await;
    let second = subnet_in(&router, &first_vpc, "10.80.2.0/24", "us-west-2b").await;
    let other = subnet_in(&router, &second_vpc, "10.90.1.0/24", "us-west-2a").await;

    // (case, the subnet the row names, the sourceArn it spells it with)
    let cases: Vec<(&str, &Subnet, String)> = vec![
        (
            "the ARN Amazon EC2 minted for the subnet",
            &first,
            first.subnet_arn.clone(),
        ),
        (
            "a second subnet of the same VPC",
            &second,
            second.subnet_arn.clone(),
        ),
        (
            "a subnet of another VPC",
            &other,
            other.subnet_arn.clone(),
        ),
        (
            "another partition in the ARN",
            &first,
            arn_spelled("aws-us-gov", REGION, &first.subnet_id),
        ),
        (
            "another region in the ARN",
            &other,
            arn_spelled("aws", "eu-west-1", &other.subnet_id),
        ),
        (
            "an empty region segment",
            &second,
            arn_spelled("aws", "", &second.subnet_id),
        ),
    ];

    for (index, (case, subnet, source_arn)) in cases.iter().enumerate() {
        let name = format!("held-{index}");
        monitor_named(&router, case, &name).await;
        let body = format!(r#"{{"probe":{}}}"#, probe_from(source_arn, index + 1));
        let (status, created) = create_probe(&router, &name, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a probe sourced from a held subnet must be accepted, \
             got {status} {created:?}"
        );
        assert_eq!(
            string_at(&created, "sourceArn"),
            *source_arn,
            "case {case:?}: expected the stored sourceArn {source_arn:?}, got {created:?}"
        );
        assert_eq!(
            vpc_of(&created),
            subnet.vpc_id,
            "case {case:?}: expected the VPC EC2 holds the subnet in ({:?}), got {created:?}",
            subnet.vpc_id
        );

        // Read back through the monitor: what the create answered is what a
        // later reader reports.
        let (status, got) = get_monitor(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetMonitor 200, got {status} {got:?}"
        );
        let probes = probes_of(&got);
        assert_eq!(
            probes.len(),
            1,
            "case {case:?}: expected the monitor to hold the one probe, got {got:?}"
        );
        assert_eq!(
            vpc_of(&probes[0]),
            subnet.vpc_id,
            "case {case:?}: expected the read-back probe to report VPC {:?}, got {got:?}",
            subnet.vpc_id
        );
        assert_eq!(
            string_at(&probes[0], "sourceArn"),
            *source_arn,
            "case {case:?}: expected the read-back probe to keep its sourceArn, got {got:?}"
        );
    }
}

/// A `sourceArn` whose subnet Amazon EC2 holds none for is refused, and attaches
/// nothing — on `CreateProbe` and on `CreateMonitor`'s inline list alike.
#[tokio::test]
async fn a_source_the_account_holds_no_subnet_for_is_refused() {
    let router = emulator();
    let vpc = vpc_with(&router, "10.70.0.0/16").await;
    let held = subnet_in(&router, &vpc, "10.70.1.0/24", "us-west-2a").await;
    // A subnet the account held and gave back, through Amazon EC2 itself: from
    // then on its id names no subnet of the account.
    let released = subnet_in(&router, &vpc, "10.70.2.0/24", "us-west-2b").await;
    ec2_ok(
        &router,
        "DeleteSubnet",
        &[("SubnetId", released.subnet_id.as_str())],
    )
    .await;

    // A held id with the low digits of its identifier changed: as well-formed as the held
    // one, and above everything the store's ONE id mint has issued, so the account holds no
    // subnet under it however many ids a fresh account's ambient resources spent.
    let mut neighbour = held.subnet_id.clone();
    neighbour.truncate(neighbour.len() - 3);
    neighbour.push_str("fff");
    assert_ne!(
        neighbour, held.subnet_id,
        "the neighbouring id must differ from the held one"
    );

    // (case, the sourceArn the row sends, the subnet id its refusal names)
    let cases: Vec<(&str, String, String)> = vec![
        (
            // The cited case: a well-formed subnet ARN of this account whose
            // subnet was never created anywhere.
            "a 17-character id the account holds no subnet for",
            arn_spelled("aws", REGION, "subnet-00000000000000000"),
            "subnet-00000000000000000".to_string(),
        ),
        (
            // The other id length AWS publishes for an EC2 resource.
            "an 8-character id the account holds no subnet for",
            arn_spelled("aws", REGION, "subnet-0a1b2c3d"),
            "subnet-0a1b2c3d".to_string(),
        ),
        (
            "a held subnet's id with its low digits changed",
            arn_spelled("aws", REGION, &neighbour),
            neighbour.clone(),
        ),
        (
            "the id of a subnet the account gave back",
            released.subnet_arn.clone(),
            released.subnet_id.clone(),
        ),
        (
            "a subnet of another account",
            format!("arn:aws:ec2:{REGION}:111122223333:subnet/{}", held.subnet_id),
            held.subnet_id.clone(),
        ),
        (
            // A resource the account really holds that is no subnet at all.
            "the VPC the held subnet sits in",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:vpc/{vpc}"),
            vpc.clone(),
        ),
    ];

    for (index, (case, source_arn, named)) in cases.iter().enumerate() {
        // Through CreateProbe, onto a monitor that must stay empty.
        let name = format!("absent-{index}");
        monitor_named(&router, case, &name).await;
        let body = format!(r#"{{"probe":{}}}"#, probe_from(source_arn, index + 1));
        let (status, refused) = create_probe(&router, &name, &body).await;
        assert_absent_refusal(case, status, &refused, named);
        let (status, got) = get_monitor(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetMonitor 200, got {status} {got:?}"
        );
        assert!(
            probes_of(&got).is_empty(),
            "case {case:?}: the refused create must attach no probe, got {got:?}"
        );

        // And through CreateMonitor's inline list, which must create nothing.
        let inline = format!("inline-absent-{index}");
        let body = format!(
            r#"{{"monitorName":"{inline}","aggregationPeriod":30,"probes":[{}]}}"#,
            probe_from(source_arn, index + 1)
        );
        let (status, refused) = create_monitor(&router, &body).await;
        assert_absent_refusal(case, status, &refused, named);
        let (status, missing) = get_monitor(&router, &inline).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused create must store no monitor, got {status} {missing:?}"
        );
        assert_eq!(
            error_code(&missing),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {missing:?}"
        );
    }

    // The control: the subnet the account DOES hold is still accepted, so the
    // rows above are refused for the subnet they name and not for the shape of
    // the request.
    monitor_named(&router, "the held subnet", "absent-control").await;
    let body = format!(r#"{{"probe":{}}}"#, probe_from(&held.subnet_arn, 90));
    let (status, created) = create_probe(&router, "absent-control", &body).await;
    assert_eq!(
        status, 200,
        "the held subnet must still be accepted, got {status} {created:?}"
    );
    assert_eq!(
        vpc_of(&created),
        held.vpc_id,
        "the accepted probe must report the VPC Amazon EC2 holds its subnet in, got {created:?}"
    );
}

/// `CreateMonitor` resolves EVERY entry of its inline `probes` list, so one
/// entry naming a subnet Amazon EC2 holds none for refuses the whole create and
/// stores no monitor — wherever in the list it sits.
#[tokio::test]
async fn create_monitor_resolves_every_entry_of_its_inline_list() {
    let router = emulator();
    let vpc = vpc_with(&router, "10.60.0.0/16").await;
    let mut held: Vec<Subnet> = Vec::new();
    for position in 1..=4 {
        held.push(
            subnet_in(
                &router,
                &vpc,
                &format!("10.60.{position}.0/24"),
                "us-west-2a",
            )
            .await,
        );
    }
    // A well-formed id of this account that no subnet carries.
    let absent_id = "subnet-0fedcba987654321f";

    // (case, the list as positions: `Some(index)` is a held subnet, `None` the
    // absent id, in request order; whether the create is accepted)
    let cases: &[(&str, &[Option<usize>], bool)] = &[
        ("one held subnet", &[Some(0)], true),
        ("four held subnets", &[Some(0), Some(1), Some(2), Some(3)], true),
        ("the only entry absent", &[None], false),
        ("the first of two absent", &[None, Some(0)], false),
        ("the second of three absent", &[Some(0), None, Some(1)], false),
        (
            "the last of four absent",
            &[Some(0), Some(1), Some(2), None],
            false,
        ),
    ];

    for (index, (case, list, accepted)) in cases.iter().enumerate() {
        let name = format!("inline-{index}");
        let bodies: Vec<String> = list
            .iter()
            .enumerate()
            .map(|(position, entry)| {
                let arn = match entry {
                    Some(subnet) => held[*subnet].subnet_arn.clone(),
                    None => arn_spelled("aws", REGION, absent_id),
                };
                probe_from(&arn, position + 1)
            })
            .collect();
        let body = format!(
            r#"{{"monitorName":"{name}","aggregationPeriod":30,"probes":[{}]}}"#,
            bodies.join(",")
        );
        let (status, answered) = create_monitor(&router, &body).await;

        if !*accepted {
            assert_absent_refusal(case, status, &answered, absent_id);
            let (status, missing) = get_monitor(&router, &name).await;
            assert_eq!(
                status, 404,
                "case {case:?}: a refused create must store no monitor, got {status} {missing:?}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateMonitor 200, got {status} {answered:?}"
        );
        let (status, got) = get_monitor(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetMonitor 200, got {status} {got:?}"
        );
        let probes = probes_of(&got);
        assert_eq!(
            probes.len(),
            list.len(),
            "case {case:?}: expected {} stored probes, got {got:?}",
            list.len()
        );
        // Every stored probe reports the VPC its own source subnet sits in.
        for probe in &probes {
            let source = string_at(probe, "sourceArn");
            let subnet = held
                .iter()
                .find(|subnet| subnet.subnet_arn == source)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the stored sourceArn {source:?} must be one asked for")
                });
            assert_eq!(
                vpc_of(probe),
                subnet.vpc_id,
                "case {case:?}: expected the probe of {:?} to report VPC {:?}, got {probe:?}",
                subnet.subnet_id,
                subnet.vpc_id
            );
        }
    }
}

/// `UpdateProbe` binds no `sourceArn`, so it names no subnet to resolve: an
/// update applies to a probe whose source subnet the account has since given
/// back, and leaves that probe's source and VPC exactly as the create resolved
/// them.
#[tokio::test]
async fn an_update_names_no_source_subnet_and_resolves_none() {
    let router = emulator();
    let vpc = vpc_with(&router, "10.50.0.0/16").await;
    let subnet = subnet_in(&router, &vpc, "10.50.1.0/24", "us-west-2a").await;
    monitor_named(&router, "the update path", "updated").await;
    let body = format!(r#"{{"probe":{}}}"#, probe_from(&subnet.subnet_arn, 1));
    let (status, created) = create_probe(&router, "updated", &body).await;
    assert_eq!(
        status, 200,
        "setup: the probe must be created, got {status} {created:?}"
    );
    let probe_id = string_at(&created, "probeId");

    // (case, whether the account still holds the source subnet)
    for (case, holds_subnet) in [("while the subnet is held", true), ("once it is gone", false)] {
        if !holds_subnet {
            ec2_ok(
                &router,
                "DeleteSubnet",
                &[("SubnetId", subnet.subnet_id.as_str())],
            )
            .await;
        }
        let destination = if holds_subnet {
            "203.0.113.20"
        } else {
            "203.0.113.21"
        };
        let (status, updated) = super::monitor_and_probe::call(
            &router,
            "PATCH",
            &format!("/monitors/updated/probes/{probe_id}"),
            Some(&format!(r#"{{"destination":"{destination}"}}"#)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the update names no subnet, so it must be accepted, \
             got {status} {updated:?}"
        );
        assert_eq!(
            string_at(&updated, "destination"),
            destination,
            "case {case:?}: expected the update to store the destination, got {updated:?}"
        );
        assert_eq!(
            string_at(&updated, "sourceArn"),
            subnet.subnet_arn,
            "case {case:?}: expected the probe to keep its sourceArn, got {updated:?}"
        );
        assert_eq!(
            vpc_of(&updated),
            subnet.vpc_id,
            "case {case:?}: expected the probe to keep the VPC the create resolved \
             ({:?}), got {updated:?}",
            subnet.vpc_id
        );
    }
}

/// With NO Amazon EC2 to ask, nothing is known about the subnet a `sourceArn`
/// names, so the probe is refused on nothing — and it reports no `vpcId`,
/// because a VPC is reported only when EC2 answered one.
#[tokio::test]
async fn with_no_ec2_to_ask_a_source_is_refused_on_nothing() {
    let router = super::monitor_and_probe::router();
    monitor_named(&router, "no EC2 to ask", "no-inventory").await;

    // Both published EC2 id forms, neither of which any subnet of this emulator
    // carries, because there is no EC2 mounted on it at all.
    for (index, subnet_id) in ["subnet-00000000000000000", "subnet-0a1b2c3d"]
        .iter()
        .enumerate()
    {
        let case = format!("a {} character id", subnet_id.len() - "subnet-".len());
        let arn = arn_spelled("aws", REGION, subnet_id);
        let body = format!(r#"{{"probe":{}}}"#, probe_from(&arn, index + 1));
        let (status, created) = create_probe(&router, "no-inventory", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: with nothing to resolve against the probe must be accepted, \
             got {status} {created:?}"
        );
        assert_eq!(
            vpc_of(&created),
            "<none>",
            "case {case:?}: an unresolved source must report no vpcId, got {created:?}"
        );
    }
}
