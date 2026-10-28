// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests that pin `AuthorizeDBSecurityGroupIngress` (and its twin
//! `RevokeDBSecurityGroupIngress`) when SEVERAL callers modify the ingress rules of one DB
//! security group at the same time.
//!
//! The sibling suite `db_security_group_ingress` asserts the rule one call at a time. The
//! axis here is the one that suite does not span: CONCURRENCY. The rule a concurrent caller
//! reads is the same rule — a DB security group of this account is VPC-backed, so a request
//! that would change its ingress rules is refused HTTP 400 `InvalidParameterValue` in the
//! one text this API version answers that with, and nothing is stored — and it holds
//! independently of how many callers ask at once, in what order they are served, and
//! whether they ask for the SAME rule or different ones. So every caller of a batch in
//! flight together reads one identical answer (there is no winner that is served
//! differently from the losers), and the group reads back afterwards holding exactly the
//! rules it held before: none.
//!
//! Every call goes over the wire through the same mounting helper and awsQuery form POST
//! the sibling suites use, on a multi-threaded runtime so the calls really do overlap.

use std::collections::BTreeSet;
use std::sync::Arc;

use service_com_amazonaws_rds_amazonrdsv19::state::VPC_DB_SECURITY_GROUP_NOT_MODIFIABLE;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, emulator, expect_ok, subtrees, text};

/// How many callers of one batch are in flight together.
const WIDTH: usize = 6;

/// One row of the concurrency table: which operation the batch calls, and which members
/// each of its calls carries beyond the group name.
struct Row {
    case: &'static str,
    action: &'static str,
    members: &'static [(&'static str, &'static str)],
}

/// The batches run concurrently. The rows span the axes the rule generalises over: the
/// operation (both of the two that modify a group's rules), and the form of authorization
/// each documents — "one of CIDR range, EC2SecurityGroupId for VPC, or
/// (EC2SecurityGroupOwnerId and either EC2SecurityGroupName or EC2SecurityGroupId for
/// non-VPC)" — including the duplicate-rule case where every concurrent caller of the batch
/// asks for the very same rule.
const ROWS: &[Row] = &[
    Row {
        case: "the same CIDR range from every caller",
        action: "AuthorizeDBSecurityGroupIngress",
        members: &[("CIDRIP", "203.0.113.0/24")],
    },
    Row {
        case: "the same host CIDR range from every caller",
        action: "AuthorizeDBSecurityGroupIngress",
        members: &[("CIDRIP", "10.1.2.3/32")],
    },
    Row {
        case: "the same EC2 security group id from every caller",
        action: "AuthorizeDBSecurityGroupIngress",
        members: &[("EC2SecurityGroupId", "sg-0123456789abcdef0")],
    },
    Row {
        case: "the same EC2 security group name and owner from every caller",
        action: "AuthorizeDBSecurityGroupIngress",
        members: &[
            ("EC2SecurityGroupName", "app-tier"),
            ("EC2SecurityGroupOwnerId", "000000000000"),
        ],
    },
    Row {
        case: "no form of authorization at all from every caller",
        action: "AuthorizeDBSecurityGroupIngress",
        members: &[],
    },
    Row {
        case: "the revoke of the same CIDR range from every caller",
        action: "RevokeDBSecurityGroupIngress",
        members: &[("CIDRIP", "203.0.113.0/24")],
    },
];

/// The group every call below names, in the spelling the create sent.
const GROUP: &str = "Emu-Concurrent-Ingress";

/// What one ingress call answered, reduced to the three things the rule fixes: the HTTP
/// status, the wire error `<Code>` and the `<Message>`. The envelope's request id is
/// per-response and deliberately left out.
type Answer = (u16, String, String);

/// One ingress call, as one of several in flight together.
async fn ingress(
    router: Arc<MultiServiceRouter>,
    action: &'static str,
    members: &'static [(&'static str, &'static str)],
) -> Answer {
    let mut params: Vec<(&str, &str)> = vec![("DBSecurityGroupName", GROUP)];
    params.extend(members.iter().copied());
    let (status, body) = call(&router, action, &params).await;
    (status, text(&body, "Code"), text(&body, "Message"))
}

/// The answer the rule owes every one of these callers.
fn expected_answer() -> Answer {
    (
        400,
        "InvalidParameterValue".to_string(),
        VPC_DB_SECURITY_GROUP_NOT_MODIFIABLE.to_string(),
    )
}

/// `CreateDBSecurityGroup` for [`GROUP`].
async fn create_group(router: &MultiServiceRouter) {
    expect_ok(
        "create the group under test",
        call(
            router,
            "CreateDBSecurityGroup",
            &[
                ("DBSecurityGroupName", GROUP),
                ("DBSecurityGroupDescription", "a concurrency probe group"),
            ],
        )
        .await,
    );
}

/// Read the group back and assert it holds no ingress rule of either form, and that there
/// is exactly one of it.
async fn assert_group_holds_no_rule(router: &MultiServiceRouter, case: &str) {
    let listed = expect_ok(
        case,
        call(
            router,
            "DescribeDBSecurityGroups",
            &[("DBSecurityGroupName", GROUP)],
        )
        .await,
    );
    let actual = (
        subtrees(&listed, "DBSecurityGroup").len(),
        subtrees(&listed, "IPRange").len(),
        subtrees(&listed, "EC2SecurityGroup").len(),
    );
    assert_eq!(
        actual,
        (1, 0, 0),
        "case {case:?}: expected the one group holding no ingress rule, got \
         {actual:?} in {listed}"
    );
}

/// Every caller of a batch in flight together reads the identical refusal — whichever of
/// the two operations the batch calls, and whether its callers ask for the same rule or
/// not — and the group is left holding no rule.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_callers_of_one_batch_all_read_the_same_refusal() {
    let router = Arc::new(emulator());
    create_group(&router).await;
    let expected = expected_answer();

    for Row {
        case,
        action,
        members,
    } in ROWS
    {
        let tasks: Vec<_> = (0..WIDTH)
            .map(|_| tokio::spawn(ingress(Arc::clone(&router), action, members)))
            .collect();

        let mut answers = BTreeSet::new();
        for (caller, task) in tasks.into_iter().enumerate() {
            let actual = task.await.expect("the ingress call ran to completion");
            assert_eq!(
                actual, expected,
                "case {case:?}, caller {caller}: expected every concurrent caller to read \
                 the refusal this API version owes a VPC DB security group, got {actual:?}"
            );
            answers.insert(actual);
        }
        assert_eq!(
            answers.len(),
            1,
            "case {case:?}: expected ONE answer across all {WIDTH} concurrent callers, got \
             {answers:?}"
        );
    }

    assert_group_holds_no_rule(&router, "the group after every concurrent batch").await;
}

/// The same rule under the mixed case: every row of the table in flight at once, so the
/// two operations and all the forms interleave. Each still reads the one refusal, and the
/// group still holds no rule afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_form_in_flight_at_once_reads_the_same_refusal() {
    let router = Arc::new(emulator());
    create_group(&router).await;
    let expected = expected_answer();

    let mut tasks = Vec::with_capacity(ROWS.len() * WIDTH);
    for row in ROWS {
        for caller in 0..WIDTH {
            tasks.push((
                row.case,
                caller,
                tokio::spawn(ingress(Arc::clone(&router), row.action, row.members)),
            ));
        }
    }

    for (case, caller, task) in tasks {
        let actual = task.await.expect("the ingress call ran to completion");
        assert_eq!(
            actual, expected,
            "case {case:?} interleaved, caller {caller}: expected the refusal this API \
             version owes a VPC DB security group, got {actual:?}"
        );
    }

    assert_group_holds_no_rule(&router, "the group after the interleaved batch").await;
}
