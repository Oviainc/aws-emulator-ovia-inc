// SPDX-License-Identifier: CC0-1.0

//! The MONITORING option of a Managed Instances provider's instance launch template.
//!
//! The rule under test: `InstanceLaunchTemplate$monitoring` is part of the configuration
//! `CreateCapacityProvider` stores, so whichever of the modelled options a create names
//! (`smithy_query` query="shape" shape="ManagedInstancesMonitoringOptions" ->
//! `["BASIC", "DETAILED"]`), that option is what the stored provider reports back —
//! through the create's own answer and through `DescribeCapacityProviders` — and a create
//! that names none reports none, since the member has no value Amazon ECS substitutes
//! into the stored template. One row per modelled option, plus the create that names
//! none; which SPELLINGS of an option reach the stored template, and what a value outside
//! the modelled set is refused with, is
//! [`crate::managed_instances_monitoring_spelling`]'s subject.
//!
//! Every case goes through the shared wire harness in [`crate::ecs_wire`], the same front
//! door a client uses, so nothing reaches into the state struct.

use crate::capacity_providers::managed_instances_monitored;
use crate::ecs_wire::{create_cluster, list, ok, service, ACCOUNT};

use serde_json::json;

#[tokio::test]
async fn the_monitoring_option_a_create_names_is_what_the_provider_reports() {
    let router = service();
    create_cluster(&router, "monitored", "setup").await;

    let rows: [(&str, &str, Option<&str>); 3] = [
        ("basic monitoring", "basicmon", Some("BASIC")),
        ("detailed monitoring", "detailedmon", Some("DETAILED")),
        ("no monitoring option at all", "nomon", None),
    ];

    for (case, provider_name, requested) in rows {
        let created = ok(
            &router,
            "CreateCapacityProvider",
            json!({
                "name": provider_name,
                "cluster": "monitored",
                "managedInstancesProvider": managed_instances_monitored(requested),
            }),
            case,
        )
        .await;
        let read = ok(
            &router,
            "DescribeCapacityProviders",
            json!({ "capacityProviders": [provider_name] }),
            case,
        )
        .await;
        let read = list(&read, "capacityProviders", case);
        assert_eq!(
            read.len(),
            1,
            "case {case:?}: expected the created provider back, got {read:?}"
        );

        for reported in [&created["capacityProvider"], &read[0]] {
            let template =
                &reported["managedInstancesProvider"]["instanceLaunchTemplate"];
            assert_eq!(
                template["monitoring"].as_str(),
                requested,
                "case {case:?}: unexpected monitoring in {reported}"
            );
            // The rest of the template is stored whole beside it, so the monitoring
            // option is not reported in place of the configuration it belongs to.
            assert_eq!(
                template["ec2InstanceProfileArn"].as_str(),
                Some(
                    format!("arn:aws:iam::{ACCOUNT}:instance-profile/ecsInstanceRole")
                        .as_str()
                ),
                "case {case:?}: unexpected instance profile in {reported}"
            );
        }
    }
}
