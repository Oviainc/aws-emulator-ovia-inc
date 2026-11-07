// SPDX-License-Identifier: CC0-1.0

//! AWS CodeDeploy's published default-quota catalog.

use super::*;

/// AWS CodeDeploy's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `codedeploy` service code (what `aws
/// service-quotas list-aws-default-service-quotas --service-code codedeploy` reports, which
/// is the "CodeDeploy quotas" page of the AWS CodeDeploy User Guide), and the recorded
/// default-quota listing of that code for which `QuotaCode` names which quota, for the
/// prose wording its rows carry and for each row's unit and adjustability. CodeDeploy counts
/// its planes per ACCOUNT, per APPLICATION, per DEPLOYMENT GROUP and per DEPLOYMENT, and
/// publishes time limits beside those counts, so a row names the plane it is counted on.
pub(in crate::state) fn codedeploy_quotas() -> Vec<QuotaRow> {
    vec![
        quota("Applications in an AWS account", 1000.0)
            .code("L-0CB3C26F")
            .fixed()
            .describe(
                "The maximum number of applications in this account in the current Region.",
            ),
        quota("Deployment groups associated with a single application", 1000.0)
            .code("L-31E782CD")
            .fixed()
            .describe("The maximum number of deployment groups of one application."),
        quota("Concurrent deployments in an AWS account", 1000.0)
            .code("L-3F19B6A5")
            .describe(
                "The maximum number of deployments running at once in this account in the \
                 current Region.",
            ),
        quota("Custom deployment configurations in an AWS account", 50.0)
            .code("L-464411D9")
            .describe(
                "The maximum number of custom deployment configurations in this account in the \
                 current Region.",
            ),
        quota("Instances in a single deployment", 1000.0)
            .code("L-5AD34096")
            .fixed()
            .describe("The maximum number of instances one deployment can deploy to."),
        quota("Instances in all concurrent deployments in an AWS account", 1000.0)
            .code("L-5FB5F09F")
            .fixed()
            .describe(
                "The maximum number of instances across all deployments running at once in \
                 this account in the current Region.",
            ),
        quota("On-premises instances registered in an AWS account", 1000.0)
            .code("L-604B54B8")
            .describe(
                "The maximum number of on-premises instances registered in this account in the \
                 current Region.",
            ),
        quota("Tags in an on-premises instance", 10.0)
            .code("L-69184564")
            .fixed()
            .describe("The maximum number of tags on one on-premises instance."),
        quota("Amazon EC2 tag filters in a deployment group", 10.0)
            .code("L-696A9A65")
            .fixed()
            .describe("The maximum number of Amazon EC2 tag filters in one deployment group."),
        quota("Auto Scaling groups in a deployment group", 10.0)
            .code("L-6BCCFC85")
            .fixed()
            .describe("The maximum number of Auto Scaling groups in one deployment group."),
        quota("Amazon ECS services in a deployment group", 1.0)
            .code("L-6DACB4EE")
            .fixed()
            .describe("The maximum number of Amazon ECS services in one deployment group."),
        quota("Lambda functions in a deployment group", 1.0)
            .code("L-826E27D8")
            .fixed()
            .describe("The maximum number of Lambda functions in one deployment group."),
        quota("Minutes a deployment lifecycle event can run", 60.0)
            .code("L-86A1A2E1")
            .unit("Minutes")
            .fixed()
            .describe("The number of minutes one deployment lifecycle event can run for."),
        quota("Event notification triggers in a deployment group", 10.0)
            .code("L-877B748B")
            .describe("The maximum number of event notification triggers in one deployment group."),
        quota(
            "Minutes until a deployment fails if a lifecycle event doesn't start",
            5.0,
        )
        .code("L-9231961D")
        .unit("Minutes")
        .describe(
            "The number of minutes a deployment waits for a lifecycle event to start before \
             it fails.",
        ),
        quota(
            "Target groups for a traffic route during an Amazon ECS deployment",
            2.0,
        )
        .code("L-9F835576")
        .fixed()
        .describe(
            "The maximum number of target groups a traffic route can use during an Amazon ECS \
             deployment.",
        ),
        quota("Alarms in a deployment group", 10.0)
            .code("L-A58C473C")
            .fixed()
            .describe(
                "The maximum number of CloudWatch alarms in one deployment group's alarm \
                 configuration.",
            ),
        quota("Concurrent deployments per deployment group", 1.0)
            .code("L-A8B8B32B")
            .describe("The maximum number of deployments running at once in one deployment group."),
        quota("Hours a deployment can run", 8.0)
            .code("L-AB125F0B")
            .unit("Hours")
            .fixed()
            .describe("The number of hours one deployment can run for."),
        quota("Deployments in progress to a single instance", 1.0)
            .code("L-B0CB7B38")
            .fixed()
            .describe("The maximum number of deployments in progress to one instance."),
        quota(
            "Hours original instances are kept after a blue/green deployment",
            48.0,
        )
        .code("L-B2AEFF5A")
        .unit("Hours")
        .fixed()
        .describe(
            "The number of hours the original instances of a blue/green deployment are kept \
             for after the traffic has been rerouted.",
        ),
        quota("Lifecycle event hooks in a deployment", 10.0)
            .code("L-B8C6A115")
            .fixed()
            .describe("The maximum number of lifecycle event hooks in one deployment."),
        quota("Deployment groups an on-premises instance can be in", 10.0)
            .code("L-BB837EF6")
            .fixed()
            .describe("The maximum number of deployment groups one on-premises instance can be in."),
        quota(
            "Number of listeners for a traffic route during an Amazon ECS deployment",
            1.0,
        )
        .code("L-C77AFF36")
        .fixed()
        .describe(
            "The maximum number of listeners a traffic route can use during an Amazon ECS \
             deployment.",
        ),
        quota("Tags in a deployment group", 50.0)
            .code("L-D4ED2A8C")
            .fixed()
            .describe("The maximum number of tags on one deployment group."),
        quota("Deployment groups in an AWS account", 1000.0)
            .code("L-D9088B77")
            .describe(
                "The maximum number of deployment groups in this account in the current Region.",
            ),
        quota("GitHub token connections in an AWS account", 25.0)
            .code("L-DC0E5D95")
            .describe(
                "The maximum number of GitHub token connections in this account in the current \
                 Region.",
            ),
        quota("Revisions kept for an application", 1000.0)
            .code("L-F0A94AA0")
            .fixed()
            .describe("The maximum number of revisions kept for one application."),
    ]
}
