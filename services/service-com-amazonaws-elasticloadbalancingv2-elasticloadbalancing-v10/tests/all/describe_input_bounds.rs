// SPDX-License-Identifier: CC0-1.0

//! What the ELBv2 `Describe*` reads refuse on the REQUEST's own shape, before they look
//! anything up — over the wire, through the router.
//!
//! The rules these tables pin:
//!
//! * `DescribeLoadBalancers` describes at most 20 load balancers per call, counting the
//!   ones the request asks about through `LoadBalancerArns` and `Names` together
//!   (`LoadBalancerArns`' own documentation: "You can specify up to 20 load balancers in
//!   a single call" — `smithy_query(query="doc",shape="DescribeLoadBalancersInput",
//!   member="LoadBalancerArns")`; the model declares no length constraint for either
//!   list, so the bound is the service's own). Past it the answer is HTTP 400
//!   `ValidationError` naming the limit and the count the request carried.
//! * `DescribeTargetGroups` describes at most 20 target groups per call, and its three
//!   filters — `Names`, `TargetGroupArns` and `LoadBalancerArn` — select in three
//!   different ways, so a call carries at most one of them. Either way the answer is
//!   HTTP 400 `ValidationError`.
//! * Both are judged BEFORE any lookup, which is what the rows with identifiers that all
//!   exist pin: a request outside the shape the API accepts is never answered as a
//!   missing resource, and never with a report. An identifier nothing holds INSIDE the
//!   bound still answers the modelled not-found, which is what the rows carrying one
//!   absent identifier pin.
//!
//! Every row differs along what those rules generalise over: which member carries the
//! identifiers, how many it carries on each side of the bound, whether the identifiers
//! resolve, and which filters are combined. The wire harness (`emulator`, `call`,
//! `action`, `p`, `members`) is `all/provisioning_loop.rs`'s; nothing here restates it.

use crate::provisioning_loop::{account, action, call, emulator, members, p, REGION};
use crate::target_group_attributes_and_modification::unescaped;
use smithy_server_runtime::MultiServiceRouter;

/// The awsQuery error code this service answers a request whose values it cannot accept
/// with, for a rule its model declares no error shape for.
const VALIDATION_ERROR: &str = "ValidationError";

/// The `awsQueryError` code of the modelled `LoadBalancerNotFoundException`.
const LOAD_BALANCER_NOT_FOUND: &str = "LoadBalancerNotFound";

/// The `awsQueryError` code of the modelled `TargetGroupNotFoundException`.
const TARGET_GROUP_NOT_FOUND: &str = "TargetGroupNotFound";

/// How many resources one `Describe` call may ask about, as the handlers bound it.
const PER_CALL: usize = 20;

/// How many load balancers and target groups each table below creates: one more than the
/// bound, so a row one past the bound can be built entirely out of identifiers that
/// EXIST — which is what makes the refusal a statement about the request.
const POOL: usize = PER_CALL + 1;

/// What one row expects back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// The request is inside the shape the API accepts, and reports this many resources.
    Reports(usize),
    /// The request is outside the per-call bound; the refusal carries this count when the
    /// service's wording quotes one.
    PastBound(usize),
    /// The request is well shaped, and one identifier in it resolves to nothing.
    NotFound,
}

/// One row: the case, the indices the request names by ARN, the indices it names by name,
/// and what must come back.
type ListCase = (&'static str, Vec<usize>, Vec<usize>, Expect);

/// One row of the filter table: the case, the request's filter fields, and the awsQuery
/// error code the answer must carry (empty for a row the service serves).
type FilterCase = (&'static str, Vec<(String, String)>, &'static str);

/// Create a load balancer with two subnets and answer its ARN.
///
/// `all/provisioning_loop.rs`'s creator is private to that suite; this one exists because
/// the tables below need a POOL of load balancers rather than one, and it asserts only
/// what it needs: that the create was served and minted an ARN.
async fn create_load_balancer(router: &MultiServiceRouter, name: &str) -> String {
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", name));
    form.push(p("Subnets.member.1", "subnet-11111111"));
    form.push(p("Subnets.member.2", "subnet-22222222"));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case create {name}: expected 200, got {} body {}",
        reply.status, reply.body
    );
    reply
        .text("LoadBalancerArn")
        .unwrap_or_else(|| panic!("case create {name}: no LoadBalancerArn in {}", reply.body))
}

/// A load balancer ARN of the right shape that no load balancer in the store carries.
fn absent_load_balancer_arn(index: usize) -> String {
    format!(
        "arn:aws:elasticloadbalancing:{REGION}:{}:loadbalancer/app/absent-lb-{index}/0123456789abcdef",
        account()
    )
}

/// A target group ARN of the right shape that no target group in the store carries.
fn absent_target_group_arn(index: usize) -> String {
    format!(
        "arn:aws:elasticloadbalancing:{REGION}:{}:targetgroup/absent-tg-{index}/0123456789abcdef",
        account()
    )
}

/// The ARN at `index` of a created pool, or an ARN nothing holds for an index past it.
fn arn_at(pool: &[String], index: usize, absent: fn(usize) -> String) -> String {
    pool.get(index).cloned().unwrap_or_else(|| absent(index))
}

/// The name at `index` of a created pool, or a name nothing holds for an index past it.
fn name_at(names: &[String], index: usize, absent_prefix: &str) -> String {
    names
        .get(index)
        .cloned()
        .unwrap_or_else(|| format!("{absent_prefix}-{index}"))
}

/// The whole index range `0..count`, for a row that names a run of the pool.
fn ix(count: usize) -> Vec<usize> {
    (0..count).collect()
}

/// The indices `from..to`, for a row that names a slice of the pool.
fn span(from: usize, to: usize) -> Vec<usize> {
    (from..to).collect()
}

#[tokio::test]
async fn describe_load_balancers_bounds_the_load_balancers_one_call_asks_about() {
    let (router, _engine) = emulator();

    // A pool of 21 load balancers, so a request one past the bound can name only load
    // balancers that exist.
    let mut arns = Vec::new();
    let mut names = Vec::new();
    for index in 0..POOL {
        let name = format!("bound-lb-{index:02}");
        arns.push(create_load_balancer(&router, &name).await);
        names.push(name);
    }

    // (case, ARN indices, name indices, expectation)
    //
    // The rows walk the bound from either side, on each selector alone and on the two
    // together, with identifiers that exist and identifiers that do not: what decides the
    // refusal is how many load balancers the call asks about, not whether they are there.
    let rows: Vec<ListCase> = vec![
        ("one name", vec![], ix(1), Expect::Reports(1)),
        ("one ARN", ix(1), vec![], Expect::Reports(1)),
        (
            "twenty names, the published bound",
            vec![],
            ix(PER_CALL),
            Expect::Reports(PER_CALL),
        ),
        (
            "twenty ARNs, the published bound",
            ix(PER_CALL),
            vec![],
            Expect::Reports(PER_CALL),
        ),
        (
            "ten ARNs and ten names, together at the bound",
            ix(10),
            span(10, PER_CALL),
            Expect::Reports(PER_CALL),
        ),
        (
            "twenty-one names that all exist, one past the bound",
            vec![],
            ix(POOL),
            Expect::PastBound(POOL),
        ),
        (
            "twenty-one ARNs that all exist, one past the bound",
            ix(POOL),
            vec![],
            Expect::PastBound(POOL),
        ),
        (
            "eleven ARNs and ten names, together one past the bound",
            ix(11),
            span(11, POOL),
            Expect::PastBound(POOL),
        ),
        (
            "fifty names, most of which nothing holds",
            vec![],
            ix(50),
            Expect::PastBound(50),
        ),
        (
            "nineteen names that exist and one that does not, inside the bound",
            vec![],
            span(0, 19).into_iter().chain([POOL + 7]).collect(),
            Expect::NotFound,
        ),
        (
            "one ARN nothing holds, inside the bound",
            vec![POOL + 3],
            vec![],
            Expect::NotFound,
        ),
        ("no selector at all", vec![], vec![], Expect::Reports(POOL)),
    ];

    for (case, arn_indices, name_indices, expected) in rows {
        let mut form = action("DescribeLoadBalancers");
        for (position, index) in arn_indices.iter().enumerate() {
            form.push(p(
                &format!("LoadBalancerArns.member.{}", position + 1),
                &arn_at(&arns, *index, absent_load_balancer_arn),
            ));
        }
        for (position, index) in name_indices.iter().enumerate() {
            form.push(p(
                &format!("Names.member.{}", position + 1),
                &name_at(&names, *index, "absent-lb"),
            ));
        }
        // Every row asks for every load balancer it can report in one page, so the
        // reported count is the whole selection rather than a page of it.
        form.push(p("PageSize", "400"));
        let reply = call(&router, &form).await;

        match expected {
            Expect::Reports(count) => {
                assert_eq!(
                    reply.status, 200,
                    "case {case:?}: expected 200, got {} body {}",
                    reply.status, reply.body
                );
                let reported = members(&reply.body, "LoadBalancers").len();
                assert_eq!(
                    reported, count,
                    "case {case:?}: expected {count} load balancers reported, got {reported} body {}",
                    reply.body
                );
            }
            Expect::PastBound(asked_about) => {
                let expected_message = format!(
                    "Up to {PER_CALL} load balancers can be described at a time, but {asked_about} were specified"
                );
                assert_eq!(
                    (reply.status, reply.error_code().as_str()),
                    (400, VALIDATION_ERROR),
                    "case {case:?}: expected 400 {VALIDATION_ERROR}, got {} body {}",
                    reply.status,
                    reply.body
                );
                let message = reply.text("Message").map(|text| unescaped(&text));
                assert_eq!(
                    message.as_deref(),
                    Some(expected_message.as_str()),
                    "case {case:?}: expected the message {expected_message:?}, got {message:?}"
                );
            }
            Expect::NotFound => {
                assert_eq!(
                    (reply.status, reply.error_code().as_str()),
                    (400, LOAD_BALANCER_NOT_FOUND),
                    "case {case:?}: expected 400 {LOAD_BALANCER_NOT_FOUND}, got {} body {}",
                    reply.status,
                    reply.body
                );
            }
        }
    }
}

#[tokio::test]
async fn describe_target_groups_bounds_the_target_groups_one_call_asks_about() {
    let (router, _engine) = emulator();

    // The same pool, of target groups: 21 of them, so the row one past the bound names
    // only target groups that exist.
    let mut arns = Vec::new();
    let mut names = Vec::new();
    for index in 0..POOL {
        let name = format!("bound-tg-{index:02}");
        arns.push(crate::provisioning_loop::create_http_target_group(&router, &name).await);
        names.push(name);
    }

    // (case, ARN indices, name indices, expectation)
    //
    // Each row names ONE of the two selectors, because the filters are mutually exclusive
    // (the next test); the bound is walked from either side on each of them.
    let rows: Vec<ListCase> = vec![
        ("one ARN", ix(1), vec![], Expect::Reports(1)),
        ("one name", vec![], ix(1), Expect::Reports(1)),
        (
            "twenty ARNs, the bound",
            ix(PER_CALL),
            vec![],
            Expect::Reports(PER_CALL),
        ),
        (
            "twenty names, the bound",
            vec![],
            ix(PER_CALL),
            Expect::Reports(PER_CALL),
        ),
        (
            "twenty-one ARNs that all exist, one past the bound",
            ix(POOL),
            vec![],
            Expect::PastBound(POOL),
        ),
        (
            "twenty-one names that all exist, one past the bound",
            vec![],
            ix(POOL),
            Expect::PastBound(POOL),
        ),
        (
            "fifty ARNs, most of which nothing holds",
            ix(50),
            vec![],
            Expect::PastBound(50),
        ),
        (
            "nineteen ARNs that exist and one that does not, inside the bound",
            span(0, 19).into_iter().chain([POOL + 7]).collect(),
            vec![],
            Expect::NotFound,
        ),
        (
            "one name nothing holds, inside the bound",
            vec![],
            vec![POOL + 3],
            Expect::NotFound,
        ),
        ("no filter at all", vec![], vec![], Expect::Reports(POOL)),
    ];

    for (case, arn_indices, name_indices, expected) in rows {
        let mut form = action("DescribeTargetGroups");
        for (position, index) in arn_indices.iter().enumerate() {
            form.push(p(
                &format!("TargetGroupArns.member.{}", position + 1),
                &arn_at(&arns, *index, absent_target_group_arn),
            ));
        }
        for (position, index) in name_indices.iter().enumerate() {
            form.push(p(
                &format!("Names.member.{}", position + 1),
                &name_at(&names, *index, "absent-tg"),
            ));
        }
        form.push(p("PageSize", "400"));
        let reply = call(&router, &form).await;

        match expected {
            Expect::Reports(count) => {
                assert_eq!(
                    reply.status, 200,
                    "case {case:?}: expected 200, got {} body {}",
                    reply.status, reply.body
                );
                let reported = members(&reply.body, "TargetGroups").len();
                assert_eq!(
                    reported, count,
                    "case {case:?}: expected {count} target groups reported, got {reported} body {}",
                    reply.body
                );
            }
            // The service's wording for this bound quotes the limit and no count, so the
            // row's count decides only which side of the bound it is on.
            Expect::PastBound(_) => {
                let expected_message =
                    format!("You cannot describe more than '{PER_CALL}' target groups at a time");
                assert_eq!(
                    (reply.status, reply.error_code().as_str()),
                    (400, VALIDATION_ERROR),
                    "case {case:?}: expected 400 {VALIDATION_ERROR}, got {} body {}",
                    reply.status,
                    reply.body
                );
                let message = reply.text("Message").map(|text| unescaped(&text));
                assert_eq!(
                    message.as_deref(),
                    Some(expected_message.as_str()),
                    "case {case:?}: expected the message {expected_message:?}, got {message:?}"
                );
            }
            Expect::NotFound => {
                assert_eq!(
                    (reply.status, reply.error_code().as_str()),
                    (400, TARGET_GROUP_NOT_FOUND),
                    "case {case:?}: expected 400 {TARGET_GROUP_NOT_FOUND}, got {} body {}",
                    reply.status,
                    reply.body
                );
            }
        }
    }
}

#[tokio::test]
async fn describe_target_groups_refuses_more_than_one_filter() {
    let (router, _engine) = emulator();

    let load_balancer = create_load_balancer(&router, "filter-lb").await;
    let group = crate::provisioning_loop::create_http_target_group(&router, "filter-tg").await;
    let absent_load_balancer = absent_load_balancer_arn(1);

    // (case, the filter fields, the expected awsQuery code — empty for a row the service
    // serves)
    //
    // The rows walk every combination of the three filters: each one alone, each pair, and
    // all three. A pair is refused as surely as the three together, and the refusal is
    // about the request's shape, so it is answered even when a filter in it names a load
    // balancer nothing holds.
    let rows: Vec<FilterCase> = vec![
        (
            "target group ARNs alone",
            vec![p("TargetGroupArns.member.1", &group)],
            "",
        ),
        ("names alone", vec![p("Names.member.1", "filter-tg")], ""),
        (
            "a load balancer ARN alone",
            vec![p("LoadBalancerArn", &load_balancer)],
            "",
        ),
        (
            "names and target group ARNs",
            vec![
                p("Names.member.1", "filter-tg"),
                p("TargetGroupArns.member.1", &group),
            ],
            VALIDATION_ERROR,
        ),
        (
            "names and a load balancer ARN",
            vec![
                p("Names.member.1", "filter-tg"),
                p("LoadBalancerArn", &load_balancer),
            ],
            VALIDATION_ERROR,
        ),
        (
            "target group ARNs and a load balancer ARN",
            vec![
                p("TargetGroupArns.member.1", &group),
                p("LoadBalancerArn", &load_balancer),
            ],
            VALIDATION_ERROR,
        ),
        (
            "all three filters",
            vec![
                p("Names.member.1", "filter-tg"),
                p("TargetGroupArns.member.1", &group),
                p("LoadBalancerArn", &load_balancer),
            ],
            VALIDATION_ERROR,
        ),
        (
            "all three filters, where the load balancer ARN names nothing",
            vec![
                p("Names.member.1", "filter-tg"),
                p("TargetGroupArns.member.1", &group),
                p("LoadBalancerArn", &absent_load_balancer),
            ],
            VALIDATION_ERROR,
        ),
        (
            "one filter naming a load balancer nothing holds",
            vec![p("LoadBalancerArn", &absent_load_balancer)],
            LOAD_BALANCER_NOT_FOUND,
        ),
    ];

    let conflict = "Target group names, target group ARNs, and a load balancer ARN cannot be \
                    specified at the same time";

    for (case, fields, code) in rows {
        let mut form = action("DescribeTargetGroups");
        for field in &fields {
            form.push(field.clone());
        }
        let reply = call(&router, &form).await;

        if code.is_empty() {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: one filter is accepted, so expected 200, got {} body {}",
                reply.status, reply.body
            );
            continue;
        }
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, code),
            "case {case:?}: expected 400 {code}, got {} body {}",
            reply.status,
            reply.body
        );
        if code == VALIDATION_ERROR {
            let message = reply.text("Message").map(|text| unescaped(&text));
            assert_eq!(
                message.as_deref(),
                Some(conflict),
                "case {case:?}: expected the message {conflict:?}, got {message:?}"
            );
        }
    }

    // A refused read must change nothing: every target group is still reported, and the
    // one filter that was refused in combination still works on its own.
    let mut form = action("DescribeTargetGroups");
    form.push(p("Names.member.1", "filter-tg"));
    let after = call(&router, &form).await;
    assert_eq!(
        (after.status, members(&after.body, "TargetGroups").len()),
        (200, 1),
        "case after the refusals: the target group must still be reported, got {} body {}",
        after.status,
        after.body
    );
}
