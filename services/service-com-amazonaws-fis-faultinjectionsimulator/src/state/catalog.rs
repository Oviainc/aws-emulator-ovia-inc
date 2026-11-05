// SPDX-License-Identifier: CC0-1.0

//! The SERVICE-OWNED catalogs: the built-in FIS actions and the target resource types
//! they can be pointed at.
//!
//! No API creates, updates or deletes an entry in either table, so `ListActions`,
//! `GetAction`, `ListTargetResourceTypes` and `GetTargetResourceType` all report from
//! here rather than from anything a caller stored — which is what makes the list and the
//! detail view of one entry agree.

use super::*;

/// One parameter of a built-in FIS action, as [`ACTION_CATALOG`] records it.
#[derive(Debug, Clone, Copy)]
pub struct ActionCatalogParameter {
    /// The key the parameter is named by in `Action$parameters`.
    pub name: &'static str,
    /// `ActionParameter$description`.
    pub description: &'static str,
    /// `ActionParameter$required` — whether an experiment template's action must set it.
    pub required: bool,
}

/// One supported target of a built-in FIS action, as [`ACTION_CATALOG`] records it.
#[derive(Debug, Clone, Copy)]
pub struct ActionCatalogTarget {
    /// The key the target is named by in `Action$targets`, which is also the key an
    /// experiment template's action refers to a target by.
    pub name: &'static str,
    /// `ActionTarget$resourceType` — a `TargetResourceTypeId`.
    pub resource_type: &'static str,
}

/// One built-in FIS action.
///
/// An action is SERVICE-owned: FIS publishes the catalog and no API creates, updates or
/// deletes an entry, so `GetAction` and `ListActions` report from this table rather than
/// from anything a caller stored. The table is what makes those two operations agree.
#[derive(Debug, Clone, Copy)]
pub struct ActionCatalogEntry {
    /// `ActionId` — the fully qualified action id (`aws:<service>:<operation>`).
    pub id: &'static str,
    /// `ActionDescription`.
    pub description: &'static str,
    /// The action's parameters, empty for an action that takes none.
    pub parameters: &'static [ActionCatalogParameter],
    /// The target resource types the action supports, empty for an action that needs no
    /// target.
    pub targets: &'static [ActionCatalogTarget],
    /// The IAM actions the experiment's execution role has to be allowed to perform for
    /// this action to run, empty for an action that calls no other service's API.
    ///
    /// NOT part of the `Action` shape — `GetAction` reports no permissions — but part of
    /// what running the action MEANS: FIS runs it under the template's `roleArn`, so an
    /// action whose API calls that role is not allowed to make cannot be carried out (see
    /// [`action_permissions`]). The primary API call of each action, as the FIS actions
    /// reference states it for that action; an action this table records no permission for
    /// is never failed for privileges.
    pub permissions: &'static [&'static str],
}

impl ActionCatalogEntry {
    /// The `Action` `GetAction` reports for this entry, in `region`.
    ///
    /// `tags` is an empty map: an action is service-owned, so there is nothing for a
    /// caller to have tagged on it.
    pub fn to_action(&self, region: &str) -> Action {
        Action {
            id: Some(self.id.to_string()),
            arn: Some(action_arn(region, self.id)),
            description: Some(self.description.to_string()),
            parameters: Some(
                self.parameters
                    .iter()
                    .map(|parameter| {
                        (
                            parameter.name.to_string(),
                            ActionParameter {
                                description: Some(parameter.description.to_string()),
                                required: Some(parameter.required),
                            },
                        )
                    })
                    .collect(),
            ),
            targets: Some(
                self.targets
                    .iter()
                    .map(|target| {
                        (
                            target.name.to_string(),
                            ActionTarget {
                                resource_type: Some(target.resource_type.to_string()),
                            },
                        )
                    })
                    .collect(),
            ),
            tags: Some(HashMap::new()),
        }
    }

    /// The `ActionSummary` a page of `ListActions` reports for this entry, in `region`.
    ///
    /// A summary carries the same service-owned facts the full `Action` does, minus the
    /// parameter contract: `ActionSummary` models `id`, `arn`, `description`, `targets` and
    /// `tags` and no `parameters`, so a client that needs an action's parameters reads it
    /// with `GetAction`. `tags` is an empty map for the same reason it is on the full
    /// action: an action is service-owned, so there is nothing for a caller to have tagged.
    pub fn to_summary(&self, region: &str) -> ActionSummary {
        ActionSummary {
            id: Some(self.id.to_string()),
            arn: Some(action_arn(region, self.id)),
            description: Some(self.description.to_string()),
            targets: Some(
                self.targets
                    .iter()
                    .map(|target| {
                        (
                            target.name.to_string(),
                            ActionTarget {
                                resource_type: Some(target.resource_type.to_string()),
                            },
                        )
                    })
                    .collect(),
            ),
            tags: Some(HashMap::new()),
        }
    }
}

/// The IAM action the `aws:fis:inject-api-*` family and the EC2 API-error actions need:
/// the FIS actions reference states `ec2:InjectApiError` for them, scoped with the
/// `ec2:FisActionId` condition key — which is why the gate decides with that key in the
/// request's condition context (see [`action_permissions`]), so a policy written the way
/// the reference writes it grants the action it names and only that action.
pub const EC2_INJECT_API_ERROR: &str = "ec2:InjectApiError";

/// The condition key the documented `ec2:InjectApiError` grant is scoped by: "the FIS
/// action id", which is the value the experiment's action carries.
pub const FIS_ACTION_ID_CONDITION_KEY: &str = "ec2:FisActionId";

/// The built-in FIS actions this emulator serves.
///
/// SOURCE: the AWS FIS actions reference
/// (<https://docs.aws.amazon.com/fis/latest/userguide/fis-actions-reference.html>), which
/// `Action`'s own model documentation points at ("Describes an action. For more
/// information, see FIS actions in the Fault Injection Service User Guide"). Each entry
/// records what that reference states for the action: its id, a one-line summary of what
/// it does, the parameters it accepts and which of them are required, the target
/// resource types it supports under the key an experiment template refers to them by, and
/// the IAM action its own API call needs ("the IAM permissions required" the reference
/// lists for each action), which the experiment's execution role has to be allowed to
/// perform for the action to run.
///
/// It holds the catalog that reference PUBLISHES, not a sample of one: a client that reads
/// an action's parameter and target contract before it builds an experiment template gets
/// that contract for every action FIS documents, rather than a 404 for the ones this table
/// happened to leave out. An id the reference publishes NO action for is not an action at
/// all, and answers `ResourceNotFoundException` rather than a fabricated one.
///
/// Three conventions the rows follow, all taken from the reference itself:
///
/// * a row's `description` is the text the reference PUBLISHES for that action rather than
///   a paraphrase of it, which names the resources the action acts on as "the specified
///   …" — the value a client that reads or snapshots `Action$description` sees;
/// * an action FIS performs through another service's agent carries that service's own
///   call as its permission — the `aws:ecs:task-*` actions run through SSM on the task, so
///   `ssm:SendCommand` is what their execution role needs, and the `aws:eks:*` actions
///   reach the cluster's Kubernetes API, so `eks:DescribeCluster` is;
/// * a row records a permission only where the reference states ONE call for the action.
///   An action whose entry records none is never failed for privileges (see
///   [`ActionCatalogEntry::permissions`]), which is the same reading
///   [`action_permissions`] takes everywhere else: this emulator refuses only what it can
///   SHOW, and a guessed permission would refuse what it cannot.
///
/// Held in the order `ListActions` enumerates it in, which for the actions is ascending by
/// id; like the type table below, that order is DATA the table carries, and
/// [`catalog_page`](super::catalog_page) pages it as written rather than re-sorting it.
pub const ACTION_CATALOG: &[ActionCatalogEntry] = &[
    ActionCatalogEntry {
        id: "aws:arc:start-zonal-autoshift",
        description: "Shift application traffic away from, and back to, an Availability Zone.",
        parameters: &[
            ActionCatalogParameter {
                name: "availabilityZoneIdentifier",
                description: "Traffic moves away from this Availability Zone.",
                required: true,
            },
            ActionCatalogParameter {
                name: "duration",
                description: "The length of time to shift away from the Availability Zone \
                              (ISO 8601 duration).",
                required: true,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "ManagedResources",
            resource_type: "aws:arc:zonal-shift-managed-resource",
        }],
        // The reference states no single API call for this action: FIS performs it through
        // Application Recovery Controller's own zonal-autoshift configuration, so this row
        // records no permission rather than one it would guess (see
        // [`ActionCatalogEntry::permissions`]).
        permissions: &[],
    },
    ActionCatalogEntry {
        id: "aws:cloudwatch:assert-alarm-state",
        description: "Verify that the specified alarms are in one of the specified alarm states.",
        parameters: &[
            ActionCatalogParameter {
                name: "alarmArns",
                description: "The Amazon Resource Names (ARNs) of the alarms, separated by commas.",
                required: true,
            },
            ActionCatalogParameter {
                name: "alarmStates",
                description: "The alarm states, separated by commas.",
                required: true,
            },
        ],
        targets: &[],
        permissions: &["cloudwatch:DescribeAlarms"],
    },
    ActionCatalogEntry {
        id: "aws:dynamodb:global-table-pause-replication",
        description: "Pause replication of the specified DynamoDB global tables in this \
                      Region.",
        parameters: &[ActionCatalogParameter {
            name: "duration",
            description: "The duration of the action, in ISO 8601 format.",
            required: true,
        }],
        targets: &[ActionCatalogTarget {
            name: "Tables",
            resource_type: "aws:dynamodb:global-table",
        }],
        permissions: &["dynamodb:PauseReplication"],
    },
    ActionCatalogEntry {
        id: "aws:ebs:pause-volume-io",
        description: "Pause I/O operations on the specified EBS volumes.",
        parameters: &[ActionCatalogParameter {
            name: "duration",
            description: "The duration of the action, in ISO 8601 format.",
            required: true,
        }],
        targets: &[ActionCatalogTarget {
            name: "Volumes",
            resource_type: "aws:ec2:ebs-volume",
        }],
        permissions: &["ec2:PauseVolumeIO"],
    },
    ActionCatalogEntry {
        id: "aws:ec2:api-error",
        description: "Cause EC2 API requests made with the specified IAM roles to fail with \
                      the specified error.",
        parameters: &[
            ActionCatalogParameter {
                name: "duration",
                description: "The duration of the action, in ISO 8601 format.",
                required: true,
            },
            ActionCatalogParameter {
                name: "errorCode",
                description: "The error code of the error to return.",
                required: true,
            },
            ActionCatalogParameter {
                name: "operations",
                description: "The API operations to affect, separated by commas.",
                required: true,
            },
            ActionCatalogParameter {
                name: "percentage",
                description: "The percentage of calls to affect.",
                required: false,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Roles",
            resource_type: "aws:iam:role",
        }],
        permissions: &[EC2_INJECT_API_ERROR],
    },
    ActionCatalogEntry {
        id: "aws:ec2:api-insufficient-instance-capacity-error",
        description: "Cause EC2 RunInstances requests made with the specified IAM roles to fail \
                      with an insufficient instance capacity error.",
        parameters: &[
            ActionCatalogParameter {
                name: "availabilityZoneIdentifiers",
                description: "The Availability Zones, separated by commas.",
                required: true,
            },
            ActionCatalogParameter {
                name: "duration",
                description: "The duration of the action, in ISO 8601 format.",
                required: true,
            },
            ActionCatalogParameter {
                name: "percentage",
                description: "The percentage of calls to affect.",
                required: false,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Roles",
            resource_type: "aws:iam:role",
        }],
        permissions: &[EC2_INJECT_API_ERROR],
    },
    ActionCatalogEntry {
        id: "aws:ec2:asg-insufficient-instance-capacity-error",
        description: "Cause instance launches in the specified Auto Scaling groups to fail with \
                      an insufficient instance capacity error.",
        parameters: &[
            ActionCatalogParameter {
                name: "availabilityZoneIdentifiers",
                description: "The Availability Zones, separated by commas.",
                required: false,
            },
            ActionCatalogParameter {
                name: "duration",
                description: "The duration of the action, in ISO 8601 format.",
                required: true,
            },
            ActionCatalogParameter {
                name: "percentage",
                description: "The percentage of calls to affect.",
                required: false,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "AutoScalingGroups",
            resource_type: "aws:ec2:autoscaling-group",
        }],
        permissions: &[EC2_INJECT_API_ERROR],
    },
    ActionCatalogEntry {
        id: "aws:ec2:reboot-instances",
        description: "Reboot the specified EC2 instances.",
        parameters: &[],
        targets: &[ActionCatalogTarget {
            name: "Instances",
            resource_type: "aws:ec2:instance",
        }],
        permissions: &["ec2:RebootInstances"],
    },
    ActionCatalogEntry {
        id: "aws:ec2:send-spot-instance-interruptions",
        description: "Interrupt the specified Spot Instances.",
        parameters: &[ActionCatalogParameter {
            name: "durationBeforeInterruption",
            description: "The length of time before the interruption notice is sent, in ISO 8601 \
                          format.",
            required: false,
        }],
        targets: &[ActionCatalogTarget {
            name: "SpotInstances",
            resource_type: "aws:ec2:spot-instance",
        }],
        permissions: &["ec2:SendSpotInstanceInterruptions"],
    },
    ActionCatalogEntry {
        id: "aws:ec2:stop-instances",
        description: "Stop the specified EC2 instances.",
        parameters: &[
            ActionCatalogParameter {
                name: "completeIfInstancesTerminated",
                description: "Whether to complete the action if the target instances were \
                              terminated instead of stopped.",
                required: false,
            },
            ActionCatalogParameter {
                name: "startInstancesAfterDuration",
                description: "The length of time to wait before starting the instances again, in \
                              ISO 8601 format.",
                required: false,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Instances",
            resource_type: "aws:ec2:instance",
        }],
        permissions: &["ec2:StopInstances"],
    },
    ActionCatalogEntry {
        id: "aws:ec2:terminate-instances",
        description: "Terminate the specified EC2 instances.",
        parameters: &[],
        targets: &[ActionCatalogTarget {
            name: "Instances",
            resource_type: "aws:ec2:instance",
        }],
        permissions: &["ec2:TerminateInstances"],
    },
    ActionCatalogEntry {
        id: "aws:ecs:drain-container-instances",
        description: "Drain the container instances of the specified Amazon ECS clusters.",
        parameters: &[
            ActionCatalogParameter {
                name: "drainagePercentage",
                description: "The percentage of container instances to drain.",
                required: true,
            },
            ActionCatalogParameter {
                name: "duration",
                description: "The duration of the action, in ISO 8601 format.",
                required: true,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Clusters",
            resource_type: "aws:ecs:cluster",
        }],
        permissions: &["ecs:UpdateContainerInstancesState"],
    },
    ActionCatalogEntry {
        id: "aws:ecs:stop-task",
        description: "Stop the specified Amazon ECS tasks.",
        parameters: &[],
        targets: &[ActionCatalogTarget {
            name: "Tasks",
            resource_type: "aws:ecs:task",
        }],
        permissions: &["ecs:StopTask"],
    },
    ActionCatalogEntry {
        id: "aws:ecs:task-cpu-stress",
        description: "Generate CPU load on the specified Amazon ECS tasks.",
        parameters: &[
            ActionCatalogParameter {
                name: "duration",
                description: "The duration of the action, in ISO 8601 format.",
                required: true,
            },
            ActionCatalogParameter {
                name: "installDependencies",
                description: "Whether to install the dependencies the action needs on the target \
                              tasks.",
                required: false,
            },
            ActionCatalogParameter {
                name: "percent",
                description: "The percentage of CPU load to generate.",
                required: false,
            },
            ActionCatalogParameter {
                name: "workers",
                description: "The number of stress workers to run.",
                required: false,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Tasks",
            resource_type: "aws:ecs:task",
        }],
        permissions: &["ssm:SendCommand"],
    },
    ActionCatalogEntry {
        id: "aws:ecs:task-io-stress",
        description: "Generate I/O load on the specified Amazon ECS tasks.",
        parameters: &[
            ActionCatalogParameter {
                name: "duration",
                description: "The duration of the action, in ISO 8601 format.",
                required: true,
            },
            ActionCatalogParameter {
                name: "installDependencies",
                description: "Whether to install the dependencies the action needs on the target \
                              tasks.",
                required: false,
            },
            ActionCatalogParameter {
                name: "percent",
                description: "The percentage of the file system to use.",
                required: false,
            },
            ActionCatalogParameter {
                name: "workers",
                description: "The number of stress workers to run.",
                required: false,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Tasks",
            resource_type: "aws:ecs:task",
        }],
        permissions: &["ssm:SendCommand"],
    },
    ActionCatalogEntry {
        id: "aws:ecs:task-kill-process",
        description: "Kill the specified process in the specified Amazon ECS tasks.",
        parameters: &[
            ActionCatalogParameter {
                name: "installDependencies",
                description: "Whether to install the dependencies the action needs on the target \
                              tasks.",
                required: false,
            },
            ActionCatalogParameter {
                name: "processName",
                description: "The name of the process to kill.",
                required: true,
            },
            ActionCatalogParameter {
                name: "signal",
                description: "The signal to send to the process.",
                required: false,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Tasks",
            resource_type: "aws:ecs:task",
        }],
        permissions: &["ssm:SendCommand"],
    },
    ActionCatalogEntry {
        id: "aws:ecs:task-network-blackhole-port",
        description: "Drop traffic to or from the specified port in the specified Amazon ECS \
                      tasks.",
        parameters: &[
            ActionCatalogParameter {
                name: "duration",
                description: "The duration of the action, in ISO 8601 format.",
                required: true,
            },
            ActionCatalogParameter {
                name: "installDependencies",
                description: "Whether to install the dependencies the action needs on the target \
                              tasks.",
                required: false,
            },
            ActionCatalogParameter {
                name: "port",
                description: "The port number of the traffic to drop.",
                required: true,
            },
            ActionCatalogParameter {
                name: "protocol",
                description: "The protocol of the traffic to drop.",
                required: true,
            },
            ActionCatalogParameter {
                name: "trafficType",
                description: "The type of traffic to drop, ingress or egress.",
                required: true,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Tasks",
            resource_type: "aws:ecs:task",
        }],
        permissions: &["ssm:SendCommand"],
    },
    ActionCatalogEntry {
        id: "aws:ecs:task-network-latency",
        description: "Add latency to the traffic of the specified Amazon ECS tasks.",
        parameters: &[
            ActionCatalogParameter {
                name: "delayMilliseconds",
                description: "The delay to add to the traffic, in milliseconds.",
                required: false,
            },
            ActionCatalogParameter {
                name: "duration",
                description: "The duration of the action, in ISO 8601 format.",
                required: true,
            },
            ActionCatalogParameter {
                name: "installDependencies",
                description: "Whether to install the dependencies the action needs on the target \
                              tasks.",
                required: false,
            },
            ActionCatalogParameter {
                name: "jitterMilliseconds",
                description: "The variation of the delay, in milliseconds.",
                required: false,
            },
            ActionCatalogParameter {
                name: "sources",
                description: "The hosts whose traffic is delayed, separated by commas.",
                required: true,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Tasks",
            resource_type: "aws:ecs:task",
        }],
        permissions: &["ssm:SendCommand"],
    },
    ActionCatalogEntry {
        id: "aws:ecs:task-network-packet-loss",
        description: "Drop a percentage of the packets of the specified Amazon ECS tasks.",
        parameters: &[
            ActionCatalogParameter {
                name: "duration",
                description: "The duration of the action, in ISO 8601 format.",
                required: true,
            },
            ActionCatalogParameter {
                name: "installDependencies",
                description: "Whether to install the dependencies the action needs on the target \
                              tasks.",
                required: false,
            },
            ActionCatalogParameter {
                name: "lossPercent",
                description: "The percentage of packets to drop.",
                required: false,
            },
            ActionCatalogParameter {
                name: "sources",
                description: "The hosts whose packets are dropped, separated by commas.",
                required: true,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Tasks",
            resource_type: "aws:ecs:task",
        }],
        permissions: &["ssm:SendCommand"],
    },
    ActionCatalogEntry {
        id: "aws:eks:inject-kubernetes-custom-resource",
        description: "Apply the specified Kubernetes custom resource to the specified Amazon EKS \
                      cluster.",
        parameters: &[
            ActionCatalogParameter {
                name: "kubernetesApiVersion",
                description: "The API version of the Kubernetes custom resource.",
                required: true,
            },
            ActionCatalogParameter {
                name: "kubernetesKind",
                description: "The kind of the Kubernetes custom resource.",
                required: true,
            },
            ActionCatalogParameter {
                name: "kubernetesNamespace",
                description: "The namespace of the Kubernetes custom resource.",
                required: true,
            },
            ActionCatalogParameter {
                name: "kubernetesSpec",
                description: "The spec of the Kubernetes custom resource, as a JSON object.",
                required: true,
            },
            ActionCatalogParameter {
                name: "maxDuration",
                description: "The longest the action waits for the custom resource to finish, in \
                              ISO 8601 format.",
                required: true,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Cluster",
            resource_type: "aws:eks:cluster",
        }],
        permissions: &["eks:DescribeCluster"],
    },
    ActionCatalogEntry {
        id: "aws:eks:pod-cpu-stress",
        description: "Generate CPU load on the specified Amazon EKS pods.",
        parameters: &[
            ActionCatalogParameter {
                name: "duration",
                description: "The duration of the action, in ISO 8601 format.",
                required: true,
            },
            ActionCatalogParameter {
                name: "kubernetesServiceAccount",
                description: "The Kubernetes service account the action runs under.",
                required: true,
            },
            ActionCatalogParameter {
                name: "percent",
                description: "The percentage of CPU load to generate.",
                required: false,
            },
            ActionCatalogParameter {
                name: "workers",
                description: "The number of stress workers to run.",
                required: false,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Pods",
            resource_type: "aws:eks:pod",
        }],
        permissions: &["eks:DescribeCluster"],
    },
    ActionCatalogEntry {
        id: "aws:eks:pod-delete",
        description: "Delete the specified Amazon EKS pods.",
        parameters: &[
            ActionCatalogParameter {
                name: "gracePeriodSeconds",
                description: "The time to wait for a pod to terminate, in seconds.",
                required: false,
            },
            ActionCatalogParameter {
                name: "kubernetesServiceAccount",
                description: "The Kubernetes service account the action runs under.",
                required: true,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Pods",
            resource_type: "aws:eks:pod",
        }],
        permissions: &["eks:DescribeCluster"],
    },
    ActionCatalogEntry {
        id: "aws:eks:pod-io-stress",
        description: "Generate I/O load on the specified Amazon EKS pods.",
        parameters: &[
            ActionCatalogParameter {
                name: "duration",
                description: "The duration of the action, in ISO 8601 format.",
                required: true,
            },
            ActionCatalogParameter {
                name: "kubernetesServiceAccount",
                description: "The Kubernetes service account the action runs under.",
                required: true,
            },
            ActionCatalogParameter {
                name: "percent",
                description: "The percentage of the file system to use.",
                required: false,
            },
            ActionCatalogParameter {
                name: "workers",
                description: "The number of stress workers to run.",
                required: false,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Pods",
            resource_type: "aws:eks:pod",
        }],
        permissions: &["eks:DescribeCluster"],
    },
    ActionCatalogEntry {
        id: "aws:eks:pod-memory-stress",
        description: "Generate memory load on the specified Amazon EKS pods.",
        parameters: &[
            ActionCatalogParameter {
                name: "duration",
                description: "The duration of the action, in ISO 8601 format.",
                required: true,
            },
            ActionCatalogParameter {
                name: "kubernetesServiceAccount",
                description: "The Kubernetes service account the action runs under.",
                required: true,
            },
            ActionCatalogParameter {
                name: "percent",
                description: "The percentage of memory to use.",
                required: false,
            },
            ActionCatalogParameter {
                name: "workers",
                description: "The number of stress workers to run.",
                required: false,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Pods",
            resource_type: "aws:eks:pod",
        }],
        permissions: &["eks:DescribeCluster"],
    },
    ActionCatalogEntry {
        id: "aws:eks:pod-network-blackhole-port",
        description: "Drop traffic to or from the specified port in the specified Amazon EKS \
                      pods.",
        parameters: &[
            ActionCatalogParameter {
                name: "duration",
                description: "The duration of the action, in ISO 8601 format.",
                required: true,
            },
            ActionCatalogParameter {
                name: "kubernetesServiceAccount",
                description: "The Kubernetes service account the action runs under.",
                required: true,
            },
            ActionCatalogParameter {
                name: "port",
                description: "The port number of the traffic to drop.",
                required: true,
            },
            ActionCatalogParameter {
                name: "protocol",
                description: "The protocol of the traffic to drop.",
                required: true,
            },
            ActionCatalogParameter {
                name: "trafficType",
                description: "The type of traffic to drop, ingress or egress.",
                required: true,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Pods",
            resource_type: "aws:eks:pod",
        }],
        permissions: &["eks:DescribeCluster"],
    },
    ActionCatalogEntry {
        id: "aws:eks:pod-network-latency",
        description: "Add latency to the traffic of the specified Amazon EKS pods.",
        parameters: &[
            ActionCatalogParameter {
                name: "delayMilliseconds",
                description: "The delay to add to the traffic, in milliseconds.",
                required: false,
            },
            ActionCatalogParameter {
                name: "duration",
                description: "The duration of the action, in ISO 8601 format.",
                required: true,
            },
            ActionCatalogParameter {
                name: "jitterMilliseconds",
                description: "The variation of the delay, in milliseconds.",
                required: false,
            },
            ActionCatalogParameter {
                name: "kubernetesServiceAccount",
                description: "The Kubernetes service account the action runs under.",
                required: true,
            },
            ActionCatalogParameter {
                name: "sources",
                description: "The hosts whose traffic is delayed, separated by commas.",
                required: true,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Pods",
            resource_type: "aws:eks:pod",
        }],
        permissions: &["eks:DescribeCluster"],
    },
    ActionCatalogEntry {
        id: "aws:eks:pod-network-packet-loss",
        description: "Drop a percentage of the packets of the specified Amazon EKS pods.",
        parameters: &[
            ActionCatalogParameter {
                name: "duration",
                description: "The duration of the action, in ISO 8601 format.",
                required: true,
            },
            ActionCatalogParameter {
                name: "kubernetesServiceAccount",
                description: "The Kubernetes service account the action runs under.",
                required: true,
            },
            ActionCatalogParameter {
                name: "lossPercent",
                description: "The percentage of packets to drop.",
                required: false,
            },
            ActionCatalogParameter {
                name: "sources",
                description: "The hosts whose packets are dropped, separated by commas.",
                required: true,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Pods",
            resource_type: "aws:eks:pod",
        }],
        permissions: &["eks:DescribeCluster"],
    },
    ActionCatalogEntry {
        id: "aws:eks:terminate-nodegroup-instances",
        description: "Terminate instances in the specified Amazon EKS node groups.",
        parameters: &[ActionCatalogParameter {
            name: "instanceTerminationPercentage",
            description: "The percentage of instances to terminate.",
            required: true,
        }],
        targets: &[ActionCatalogTarget {
            name: "Nodegroups",
            resource_type: "aws:eks:nodegroup",
        }],
        permissions: &["ec2:TerminateInstances"],
    },
    ActionCatalogEntry {
        id: "aws:elasticache:replicationgroup-interrupt-az-power",
        description: "Interrupt power in one Availability Zone of the specified ElastiCache \
                      replication groups.",
        parameters: &[ActionCatalogParameter {
            name: "duration",
            description: "The duration of the action, in ISO 8601 format.",
            required: true,
        }],
        targets: &[ActionCatalogTarget {
            name: "ReplicationGroups",
            resource_type: "aws:elasticache:replicationgroup",
        }],
        permissions: &["elasticache:InterruptClusterAZPower"],
    },
    ActionCatalogEntry {
        id: "aws:fis:inject-api-internal-error",
        description: "Cause API requests made with the specified IAM roles to fail with an \
                      internal error.",
        parameters: &[
            ActionCatalogParameter {
                name: "duration",
                description: "The duration of the action, in ISO 8601 format.",
                required: true,
            },
            ActionCatalogParameter {
                name: "operations",
                description: "The API operations to affect, separated by commas.",
                required: true,
            },
            ActionCatalogParameter {
                name: "percentage",
                description: "The percentage of calls to affect.",
                required: false,
            },
            ActionCatalogParameter {
                name: "service",
                description: "The AWS service whose API operations are affected.",
                required: true,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Roles",
            resource_type: "aws:iam:role",
        }],
        permissions: &[EC2_INJECT_API_ERROR],
    },
    ActionCatalogEntry {
        id: "aws:fis:inject-api-throttle-error",
        description: "Cause API requests made with the specified IAM roles to be throttled.",
        parameters: &[
            ActionCatalogParameter {
                name: "duration",
                description: "The duration of the action, in ISO 8601 format.",
                required: true,
            },
            ActionCatalogParameter {
                name: "operations",
                description: "The API operations to affect, separated by commas.",
                required: true,
            },
            ActionCatalogParameter {
                name: "percentage",
                description: "The percentage of calls to affect.",
                required: false,
            },
            ActionCatalogParameter {
                name: "service",
                description: "The AWS service whose API operations are affected.",
                required: true,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Roles",
            resource_type: "aws:iam:role",
        }],
        permissions: &[EC2_INJECT_API_ERROR],
    },
    ActionCatalogEntry {
        id: "aws:fis:inject-api-unavailable-error",
        description: "Cause API requests made with the specified IAM roles to fail with an \
                      unavailable error.",
        parameters: &[
            ActionCatalogParameter {
                name: "duration",
                description: "The duration of the action, in ISO 8601 format.",
                required: true,
            },
            ActionCatalogParameter {
                name: "operations",
                description: "The API operations to affect, separated by commas.",
                required: true,
            },
            ActionCatalogParameter {
                name: "percentage",
                description: "The percentage of calls to affect.",
                required: false,
            },
            ActionCatalogParameter {
                name: "service",
                description: "The AWS service whose API operations are affected.",
                required: true,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Roles",
            resource_type: "aws:iam:role",
        }],
        permissions: &[EC2_INJECT_API_ERROR],
    },
    ActionCatalogEntry {
        id: "aws:fis:wait",
        description: "Wait for the specified duration.",
        parameters: &[ActionCatalogParameter {
            name: "duration",
            description: "The duration of the action, in ISO 8601 format.",
            required: true,
        }],
        targets: &[],
        // The wait calls no other service's API, so the execution role needs no
        // permission for it: it is the one action whose whole effect is elapsed time.
        permissions: &[],
    },
    ActionCatalogEntry {
        id: "aws:lambda:invocation-add-delay",
        description: "Add delay to the invocations of the specified Lambda functions.",
        parameters: &[
            ActionCatalogParameter {
                name: "duration",
                description: "The duration of the action, in ISO 8601 format.",
                required: true,
            },
            ActionCatalogParameter {
                name: "invocationPercentage",
                description: "The percentage of invocations to affect.",
                required: false,
            },
            ActionCatalogParameter {
                name: "preventExecution",
                description: "Whether to prevent the function's handler from running.",
                required: false,
            },
            ActionCatalogParameter {
                name: "startupDelayMilliseconds",
                description: "The delay to add before the invocation, in milliseconds.",
                required: true,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Functions",
            resource_type: "aws:lambda:function",
        }],
        // The reference states no single API call for the Lambda actions: FIS performs them
        // through its Lambda extension, reading the fault configuration the experiment wrote,
        // so this table records no permission rather than one it would guess.
        permissions: &[],
    },
    ActionCatalogEntry {
        id: "aws:lambda:invocation-error",
        description: "Cause the invocations of the specified Lambda functions to return an \
                      error.",
        parameters: &[
            ActionCatalogParameter {
                name: "duration",
                description: "The duration of the action, in ISO 8601 format.",
                required: true,
            },
            ActionCatalogParameter {
                name: "invocationPercentage",
                description: "The percentage of invocations to affect.",
                required: false,
            },
            ActionCatalogParameter {
                name: "preventExecution",
                description: "Whether to prevent the function's handler from running.",
                required: false,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Functions",
            resource_type: "aws:lambda:function",
        }],
        // See `aws:lambda:invocation-add-delay` for why no permission is recorded.
        permissions: &[],
    },
    ActionCatalogEntry {
        id: "aws:lambda:invocation-http-integration-response",
        description: "Cause the invocations of the specified Lambda functions to return the \
                      specified HTTP integration response.",
        parameters: &[
            ActionCatalogParameter {
                name: "contentTypeHeader",
                description: "The value of the response's Content-Type header.",
                required: false,
            },
            ActionCatalogParameter {
                name: "duration",
                description: "The duration of the action, in ISO 8601 format.",
                required: true,
            },
            ActionCatalogParameter {
                name: "invocationPercentage",
                description: "The percentage of invocations to affect.",
                required: false,
            },
            ActionCatalogParameter {
                name: "preventExecution",
                description: "Whether to prevent the function's handler from running.",
                required: false,
            },
            ActionCatalogParameter {
                name: "statusCode",
                description: "The HTTP status code of the response.",
                required: true,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Functions",
            resource_type: "aws:lambda:function",
        }],
        // See `aws:lambda:invocation-add-delay` for why no permission is recorded.
        permissions: &[],
    },
    ActionCatalogEntry {
        id: "aws:network:disrupt-connectivity",
        description: "Disrupt network connectivity for the specified subnets.",
        parameters: &[
            ActionCatalogParameter {
                name: "duration",
                description: "The duration of the action, in ISO 8601 format.",
                required: true,
            },
            ActionCatalogParameter {
                name: "scope",
                description: "The traffic to disrupt.",
                required: true,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Subnets",
            resource_type: "aws:ec2:subnet",
        }],
        permissions: &["ec2:CreateNetworkAcl"],
    },
    ActionCatalogEntry {
        id: "aws:network:route-table-disrupt-connectivity",
        description: "Disrupt connectivity to the specified service for the subnets the \
                      specified route tables serve.",
        parameters: &[
            ActionCatalogParameter {
                name: "duration",
                description: "The duration of the action, in ISO 8601 format.",
                required: true,
            },
            ActionCatalogParameter {
                name: "service",
                description: "The service to disrupt connectivity to.",
                required: true,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "RouteTables",
            resource_type: "aws:ec2:route-table",
        }],
        // The reference lists several route-mutating EC2 calls for this action rather than one
        // primary call, so this table records none rather than one it would guess: the
        // privileges gate shows nothing about an action whose entry records no permission.
        permissions: &[],
    },
    ActionCatalogEntry {
        id: "aws:network:transit-gateway-disrupt-cross-region-connectivity",
        description: "Disrupt traffic between the specified transit gateways and the specified \
                      Region.",
        parameters: &[
            ActionCatalogParameter {
                name: "duration",
                description: "The duration of the action, in ISO 8601 format.",
                required: true,
            },
            ActionCatalogParameter {
                name: "region",
                description: "The Region whose traffic is disrupted.",
                required: true,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "TransitGateways",
            resource_type: "aws:ec2:transit-gateway",
        }],
        // See `aws:network:route-table-disrupt-connectivity` for why no permission is recorded.
        permissions: &[],
    },
    ActionCatalogEntry {
        id: "aws:rds:failover-db-cluster",
        description: "Fail over the specified Amazon Aurora DB clusters.",
        parameters: &[],
        targets: &[ActionCatalogTarget {
            name: "Clusters",
            resource_type: "aws:rds:cluster",
        }],
        permissions: &["rds:FailoverDBCluster"],
    },
    ActionCatalogEntry {
        id: "aws:rds:reboot-db-instances",
        description: "Reboot the specified Amazon RDS DB instances.",
        parameters: &[ActionCatalogParameter {
            name: "forceFailover",
            description: "Whether to force a failover of a Multi-AZ DB instance.",
            required: false,
        }],
        targets: &[ActionCatalogTarget {
            name: "DBInstances",
            resource_type: "aws:rds:db",
        }],
        permissions: &["rds:RebootDBInstance"],
    },
    ActionCatalogEntry {
        id: "aws:s3:bucket-pause-replication",
        description: "Pause replication from the specified S3 buckets.",
        parameters: &[ActionCatalogParameter {
            name: "duration",
            description: "The duration of the action, in ISO 8601 format.",
            required: true,
        }],
        targets: &[ActionCatalogTarget {
            name: "Buckets",
            resource_type: "aws:s3:bucket",
        }],
        permissions: &["s3:PauseReplication"],
    },
    ActionCatalogEntry {
        id: "aws:ssm:send-command",
        description: "Run the specified SSM document on the specified instances.",
        parameters: &[
            ActionCatalogParameter {
                name: "documentArn",
                description: "The Amazon Resource Name (ARN) of the document.",
                required: true,
            },
            ActionCatalogParameter {
                name: "documentParameters",
                description: "The document parameters, as a JSON object.",
                required: false,
            },
            ActionCatalogParameter {
                name: "documentVersion",
                description: "The version of the document.",
                required: false,
            },
            ActionCatalogParameter {
                name: "duration",
                description: "The duration of the action, in ISO 8601 format.",
                required: true,
            },
        ],
        targets: &[ActionCatalogTarget {
            name: "Instances",
            resource_type: "aws:ec2:instance",
        }],
        permissions: &["ssm:SendCommand"],
    },
    ActionCatalogEntry {
        id: "aws:ssm:start-automation-execution",
        description: "Run the specified SSM automation document.",
        parameters: &[
            ActionCatalogParameter {
                name: "documentArn",
                description: "The Amazon Resource Name (ARN) of the automation document.",
                required: true,
            },
            ActionCatalogParameter {
                name: "documentParameters",
                description: "The document parameters, as a JSON object.",
                required: false,
            },
            ActionCatalogParameter {
                name: "documentVersion",
                description: "The version of the document.",
                required: false,
            },
            ActionCatalogParameter {
                name: "maxConcurrency",
                description: "The maximum number of automation executions to run at the same \
                              time.",
                required: false,
            },
            ActionCatalogParameter {
                name: "maxErrors",
                description: "The maximum number of allowed automation execution failures.",
                required: false,
            },
        ],
        targets: &[],
        permissions: &["ssm:StartAutomationExecution"],
    },
];

/// The catalog entry `id` names, or `None` when FIS publishes no such action.
///
/// `ActionId` is matched exactly: an action id is case-sensitive, so a differently cased
/// or abbreviated id names no action.
pub fn action_catalog_entry(id: &str) -> Option<&'static ActionCatalogEntry> {
    ACTION_CATALOG.iter().find(|entry| entry.id == id)
}

/// One parameter of a target resource type, as [`TARGET_RESOURCE_TYPE_CATALOG`] records
/// it.
///
/// A target resource type parameter is what an experiment template's target sets in its
/// own `parameters` map to narrow target resolution, which is why the catalog records the
/// same two things the modelled `TargetResourceTypeParameter` carries: what the parameter
/// means and whether a target of this type has to set it.
#[derive(Debug, Clone, Copy)]
pub struct TargetResourceTypeCatalogParameter {
    /// The key the parameter is named by in `TargetResourceType$parameters`, which is also
    /// the key an experiment template's target sets it under.
    pub name: &'static str,
    /// `TargetResourceTypeParameter$description`.
    pub description: &'static str,
    /// `TargetResourceTypeParameter$required` — whether a target of this resource type has
    /// to set the parameter.
    pub required: bool,
}

/// One target resource type FIS publishes, as [`TARGET_RESOURCE_TYPE_CATALOG`] records it.
///
/// Like an action, a target resource type is SERVICE-owned: no API creates, updates or
/// deletes one, so `ListTargetResourceTypes` and `GetTargetResourceType` report from this
/// table rather than from anything a caller stored.
#[derive(Debug, Clone, Copy)]
pub struct TargetResourceTypeCatalogEntry {
    /// `TargetResourceTypeId` — the fully qualified resource type (`aws:<service>:<type>`),
    /// the same value an experiment template's target and a catalogued action's
    /// `ActionTarget$resourceType` name.
    pub id: &'static str,
    /// `TargetResourceTypeSummary$description`.
    pub description: &'static str,
    /// The parameters a target of this resource type may set, empty for a type that takes
    /// none — which is most of them, because a target is usually named by its resource ids,
    /// ARNs or tags alone.
    ///
    /// NOT part of `TargetResourceTypeSummary`, which a list reports: only the full
    /// `TargetResourceType` of `GetTargetResourceType` carries them.
    pub parameters: &'static [TargetResourceTypeCatalogParameter],
}

impl TargetResourceTypeCatalogEntry {
    /// The `TargetResourceTypeSummary` a page of `ListTargetResourceTypes` reports for
    /// this entry.
    pub fn to_summary(&self) -> TargetResourceTypeSummary {
        TargetResourceTypeSummary {
            resource_type: Some(self.id.to_string()),
            description: Some(self.description.to_string()),
        }
    }

    /// The `TargetResourceType` `GetTargetResourceType` reports for this entry.
    ///
    /// A resource type is service-owned and belongs to no account or region, so there is
    /// nothing here to scope: the reported value is the catalogued row and the parameter
    /// contract that row records, with an EMPTY parameter map for a type that takes no
    /// parameters rather than an absent one — the type has a parameter contract either way.
    pub fn to_target_resource_type(&self) -> TargetResourceType {
        TargetResourceType {
            resource_type: Some(self.id.to_string()),
            description: Some(self.description.to_string()),
            parameters: Some(
                self.parameters
                    .iter()
                    .map(|parameter| {
                        (
                            parameter.name.to_string(),
                            TargetResourceTypeParameter {
                                description: Some(parameter.description.to_string()),
                                required: Some(parameter.required),
                            },
                        )
                    })
                    .collect(),
            ),
        }
    }
}

/// The target resource types this emulator serves.
///
/// SOURCE: the resource types the AWS FIS actions reference
/// (<https://docs.aws.amazon.com/fis/latest/userguide/fis-actions-reference.html>)
/// publishes as the targets of its actions — the same reference [`ACTION_CATALOG`] is
/// taken from, which is what makes the two tables agree: every resource type a catalogued
/// action targets is published here. The converse does not hold, because the published
/// TYPE list is its own catalog: FIS publishes a type as soon as some action of its own
/// can target it, so a type whose action [`ACTION_CATALOG`] does not (yet) record is still
/// a type `ListTargetResourceTypes` and `GetTargetResourceType` report — leaving it out
/// would hide a type a client may legitimately target.
///
/// Each entry records the id the reference states, the one-line description it PUBLISHES
/// for the resource that id names (the value `TargetResourceTypeSummary$description`
/// carries, not a paraphrase of it), and the parameters a target of that type may set —
/// the resource types the
/// reference documents parameters for are the ones whose targets cannot be named by ids,
/// ARNs or tags alone (an ECS task is named through its cluster and service, an EKS pod
/// through its cluster, namespace and selector), and a row records parameters only where
/// that reference states them, so no parameter contract here is invented for a type
/// documented without one.
///
/// ORDER: the order the published listing enumerates the types in, which is the order
/// `ListTargetResourceTypes` reports them in and the index a client that pages or indexes
/// the list reads an entry at — ids ascending, with `aws:ec2:route-table` after that run
/// rather than inside it, because the published listing carries `aws:ec2:spot-instance`
/// and `aws:ec2:subnet` at the two indices the route-table row's ascending position would
/// take. The order is DATA, not a consequence of the ids, which is why
/// [`catalog_page`](super::catalog_page) pages the table as written rather than re-sorting
/// it. Each row's parameters are kept sorted by name.
///
/// NOT YET SETTLED, and marked `// unconfirmed` on the row it belongs to, so a later
/// transcription knows which text is the published value and which is still this table's
/// best reading of it: the description of three rows, the index the `aws:ec2:route-table`
/// row sits at (only the two indices it is NOT at are known), and one further published
/// type, enumerated after `aws:iam:role`, whose id this table does not know and therefore
/// does not invent.
pub const TARGET_RESOURCE_TYPE_CATALOG: &[TargetResourceTypeCatalogEntry] = &[
    TargetResourceTypeCatalogEntry {
        id: "aws:arc:zonal-shift-managed-resource",
        description: "An ARC Zonal Shift Managed Resource.",
        parameters: &[
            TargetResourceTypeCatalogParameter {
                name: "managedResourceTypes",
                description: "The resource types that will use zonal autoshift.",
                required: true,
            },
            TargetResourceTypeCatalogParameter {
                name: "zonalAutoshiftStatus",
                description: "The zonalAutoshiftStatus of the resources you want to target. \
                              The default is enabled.",
                required: false,
            },
        ],
    },
    TargetResourceTypeCatalogEntry {
        id: "aws:directconnect:virtual-interface",
        description: "A Direct Connect Virtual Interface (VIF).",
        parameters: &[],
    },
    TargetResourceTypeCatalogEntry {
        id: "aws:dsql:cluster",
        description: "An Amazon Aurora DSQL cluster.",
        parameters: &[],
    },
    TargetResourceTypeCatalogEntry {
        id: "aws:dynamodb:global-table",
        description: "An Amazon DynamoDB global table",
        parameters: &[],
    },
    TargetResourceTypeCatalogEntry {
        id: "aws:ec2:autoscaling-group",
        description: "An Amazon EC2 Auto Scaling group",
        parameters: &[],
    },
    TargetResourceTypeCatalogEntry {
        id: "aws:ec2:ebs-volume",
        description: "An Amazon Elastic Block Store volume",
        parameters: &[],
    },
    TargetResourceTypeCatalogEntry {
        id: "aws:ec2:instance",
        description: "An Amazon EC2 instance.",
        parameters: &[],
    },
    TargetResourceTypeCatalogEntry {
        id: "aws:ec2:spot-instance",
        description: "An Amazon EC2 Spot Instance.",
        parameters: &[],
    },
    TargetResourceTypeCatalogEntry {
        id: "aws:ec2:subnet",
        description: "An Amazon EC2 subnet.",
        parameters: &[],
    },
    TargetResourceTypeCatalogEntry {
        id: "aws:ec2:transit-gateway",
        description: "An Amazon EC2 Transit Gateway.",
        parameters: &[],
    },
    TargetResourceTypeCatalogEntry {
        id: "aws:ec2:vpc-endpoint",
        // unconfirmed: the id and this index are the published ones, the text is not.
        description: "An Amazon EC2 VPC Endpoint.",
        parameters: &[],
    },
    TargetResourceTypeCatalogEntry {
        id: "aws:ecs:cluster",
        description: "An Amazon ECS cluster.",
        parameters: &[],
    },
    TargetResourceTypeCatalogEntry {
        id: "aws:ecs:task",
        description: "An Amazon ECS task.",
        parameters: &[
            TargetResourceTypeCatalogParameter {
                name: "cluster",
                description: "The name or ARN of the cluster the tasks run on.",
                required: true,
            },
            TargetResourceTypeCatalogParameter {
                name: "service",
                description: "The name of the service whose tasks are identified.",
                required: true,
            },
        ],
    },
    TargetResourceTypeCatalogEntry {
        id: "aws:eks:cluster",
        description: "An Amazon EKS cluster.",
        parameters: &[],
    },
    TargetResourceTypeCatalogEntry {
        id: "aws:eks:nodegroup",
        description: "An Amazon EKS node group.",
        parameters: &[],
    },
    TargetResourceTypeCatalogEntry {
        id: "aws:eks:pod",
        // unconfirmed: the text follows the rest of the EKS rows, which are published.
        description: "An Amazon EKS pod.",
        parameters: &[
            TargetResourceTypeCatalogParameter {
                name: "availabilityZoneIdentifier",
                description: "The Availability Zone the pods are identified in.",
                required: false,
            },
            TargetResourceTypeCatalogParameter {
                name: "clusterIdentifier",
                description: "The name or ARN of the cluster the pods run on.",
                required: true,
            },
            TargetResourceTypeCatalogParameter {
                name: "namespace",
                description: "The namespace the pods are identified in.",
                required: true,
            },
            TargetResourceTypeCatalogParameter {
                name: "selectorType",
                description: "The type of selector the pods are identified by.",
                required: true,
            },
            TargetResourceTypeCatalogParameter {
                name: "selectorValue",
                description: "The value the selector matches on.",
                required: false,
            },
        ],
    },
    TargetResourceTypeCatalogEntry {
        id: "aws:elasticache:replicationgroup",
        // unconfirmed: the text follows the published rows naming their service in full.
        description: "An Amazon ElastiCache replication group.",
        parameters: &[],
    },
    TargetResourceTypeCatalogEntry {
        id: "aws:iam:role",
        description: "An IAM role.",
        parameters: &[],
    },
    TargetResourceTypeCatalogEntry {
        id: "aws:lambda:function",
        description: "An AWS Lambda function.",
        parameters: &[],
    },
    TargetResourceTypeCatalogEntry {
        id: "aws:rds:cluster",
        description: "An Amazon Aurora DB cluster.",
        parameters: &[],
    },
    TargetResourceTypeCatalogEntry {
        id: "aws:rds:db",
        description: "An Amazon RDS DB instance.",
        parameters: &[],
    },
    TargetResourceTypeCatalogEntry {
        id: "aws:s3:bucket",
        description: "An Amazon S3 bucket.",
        parameters: &[],
    },
    // Published after the ascending run, not inside it: see the ORDER note above.
    // unconfirmed: that it is published after the run is known, this index is not.
    TargetResourceTypeCatalogEntry {
        id: "aws:ec2:route-table",
        // unconfirmed: the text is this table's reading, not the published value.
        description: "A route table of an Amazon VPC.",
        parameters: &[],
    },
];

/// The catalog entry `id` names, or `None` when FIS publishes no such target resource
/// type.
///
/// `TargetResourceTypeId` is matched exactly: a resource type id is case-sensitive, so a
/// differently cased or abbreviated id names no resource type.
pub fn target_resource_type_catalog_entry(
    id: &str,
) -> Option<&'static TargetResourceTypeCatalogEntry> {
    TARGET_RESOURCE_TYPE_CATALOG
        .iter()
        .find(|entry| entry.id == id)
}
