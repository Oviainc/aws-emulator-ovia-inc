// SPDX-License-Identifier: CC0-1.0

//! AWS CodePipeline's published default-quota catalog.

use super::*;

/// AWS CodePipeline's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `codepipeline` service code (what
/// `aws service-quotas list-aws-default-service-quotas --service-code codepipeline` reports,
/// which is the "Quotas in AWS CodePipeline" page of the AWS CodePipeline User Guide), and
/// the recorded default-quota listing of that code for which `QuotaCode` names which quota,
/// for the `Total …` / `Minimum …` / `Maximum …` wording its rows carry and for each row's
/// unit and adjustability. CodePipeline publishes LOWER bounds beside its upper ones — the
/// fewest stages and actions a pipeline may have — so a row names which bound it is.
pub(in crate::state) fn codepipeline_quotas() -> Vec<QuotaRow> {
    vec![
        quota("Active executions per pipeline", 50.0)
            .code("L-0097A9B4")
            .fixed()
            .describe("The maximum number of executions running at once in one pipeline."),
        quota("Total pipelines", 1000.0)
            .code("L-1402209C")
            .fixed()
            .describe(
                "The maximum number of pipelines in this account in the current Region.",
            ),
        quota("Total V2 type pipelines", 1000.0)
            .code("L-182AE5D8")
            .fixed()
            .describe(
                "The maximum number of V2 type pipelines in this account in the current Region.",
            ),
        quota("Maximum stages per pipeline", 50.0)
            .code("L-1E23CE75")
            .describe("The maximum number of stages in one pipeline."),
        quota("Total parallel actions per stage", 100.0)
            .code("L-2B3011E2")
            .describe("The maximum number of actions that can run in parallel in one stage."),
        quota("Total sequential actions per stage", 100.0)
            .code("L-31A751EC")
            .describe("The maximum number of actions that run in sequence in one stage."),
        quota("Maximum actions per stage", 50.0)
            .code("L-3ECA8567")
            .describe("The maximum number of actions in one stage."),
        quota("Total custom actions", 50.0)
            .code("L-447DD651")
            .describe(
                "The maximum number of custom actions in this account in the current Region.",
            ),
        quota("Total variables per pipeline", 50.0)
            .code("L-519D5A90")
            .fixed()
            .describe("The maximum number of variables declared on one pipeline."),
        quota("Total triggers per pipeline", 50.0)
            .code("L-570F1605")
            .fixed()
            .describe("The maximum number of triggers on one pipeline."),
        quota("Total rules per condition", 5.0)
            .code("L-5AA45A27")
            .fixed()
            .describe("The maximum number of rules in one stage condition."),
        quota("Total conditions per stage", 5.0)
            .code("L-6F95DA01")
            .fixed()
            .describe("The maximum number of conditions on one stage."),
        quota("Maximum artifact size", 5.0)
            .code("L-78D200AD")
            .unit("Gigabytes")
            .fixed()
            .describe("The maximum size, in GB, of one input or output artifact."),
        quota("Maximum revision size", 5.0)
            .code("L-87C878A5")
            .unit("Megabytes")
            .fixed()
            .describe("The maximum size, in MB, of one source revision."),
        quota("Maximum timeout for a manual approval action", 7.0)
            .code("L-8DF1BAAD")
            .unit("Days")
            .fixed()
            .describe("The number of days a manual approval action waits before it times out."),
        quota("Maximum timeout for custom actions", 24.0)
            .code("L-912817AD")
            .unit("Hours")
            .fixed()
            .describe("The number of hours a custom action runs for before it times out."),
        quota("Maximum timeout for Lambda invoke actions", 20.0)
            .code("L-9D72EF0D")
            .unit("Minutes")
            .fixed()
            .describe("The number of minutes a Lambda invoke action runs for before it times out."),
        quota("Maximum timeout for build actions", 8.0)
            .code("L-A0A99E23")
            .unit("Hours")
            .fixed()
            .describe("The number of hours a build action runs for before it times out."),
        quota("Maximum timeout for test actions", 8.0)
            .code("L-A95CB60D")
            .unit("Hours")
            .fixed()
            .describe("The number of hours a test action runs for before it times out."),
        quota("Maximum timeout for deploy actions", 1.0)
            .code("L-ABB870A7")
            .unit("Hours")
            .fixed()
            .describe("The number of hours a deploy action runs for before it times out."),
        quota("Minimum actions", 1.0).code("L-C57FFB43").fixed().describe(
            "The fewest actions one stage of a pipeline can hold.",
        ),
        quota("Maximum pipeline executions in the queue", 50.0)
            .code("L-C5885C1C")
            .fixed()
            .describe("The maximum number of executions queued for one pipeline."),
        quota("Minimum stages per pipeline", 2.0)
            .code("L-C8CB041B")
            .fixed()
            .describe("The fewest stages one pipeline can hold."),
        quota("Total tags per pipeline", 50.0)
            .code("L-CE1F2EE9")
            .fixed()
            .describe("The maximum number of tags on one pipeline."),
        quota("Total action types", 50.0)
            .code("L-DCBA35EE")
            .describe(
                "The maximum number of action types in this account in the current Region.",
            ),
        quota("Maximum timeout for source actions", 1.0)
            .code("L-F2AD11A6")
            .unit("Hours")
            .fixed()
            .describe("The number of hours a source action runs for before it times out."),
        quota("Maximum size of a pipeline structure", 100.0)
            .code("L-F7192911")
            .unit("Kilobytes")
            .fixed()
            .describe("The maximum size, in KB, of one pipeline's structure."),
        quota("Months of pipeline execution history", 12.0)
            .code("L-F8F56C9F")
            .unit("Months")
            .fixed()
            .describe("The number of months one pipeline's execution history is kept for."),
        quota("Total webhooks", 300.0).code("L-FE939BB2").describe(
            "The maximum number of webhooks in this account in the current Region.",
        ),
    ]
}
