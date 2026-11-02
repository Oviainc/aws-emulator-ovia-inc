Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__kinesis__kinesisanalyticsv2.stop-and-force-stop-of-a-never-started-studio-notebook-rejected
Issue-Key: glue__kinesis__kinesisanalyticsv2.stop-and-force-stop-of-a-never-started-studio-notebook-rejected|run#9|AssertionError|assert got == want, f"StopApplication(Force={force}) on READY gave {got!r}, expected {want!r}"
Issue-Commit: 9b689e7a3706c4d3e9e7bd76513ebd3e34560844

# glue__kinesis__kinesisanalyticsv2: StopApplication's ResourceInUseException message for a READY Studio notebook is emulator-authored instead of AWS's fixed text (1 cause)

> _Note: this issue was identified against the code base at commit `9b689e7a3706c4d3e9e7bd76513ebd3e34560844`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopApplication

## Description
ONE cause. The divergence is in kinesisanalyticsv2, a bootstrapped service that answers; the Glue database in the check is only the Studio notebook's catalog configuration and is not involved in the refusal.

Cause 1 of 1: StopApplication (plain or with Force) on an application in READY state is refused with the wrong message text.
AWS: Smithy: StopApplication takes ApplicationName (required) and Force, and declares ResourceInUseException (errorKind client, httpError 400). An INTERACTIVE (Zeppelin Studio) application that was never started is in READY, which is not a stoppable state, so both the plain and the Force=true call are refused. The message is AWS's fixed sentence naming the state in single quotes: "Application cannot be Stopped in '<ApplicationStatus>' state" — fixed text 'Application cannot be Stopped in ' / ' state', with the application's current status (here READY) as the only variable part; the application NAME does not appear. Reference: API_StopApplication.
Emulator: 400 ResourceInUseException (code correct) with the emulator-authored, request-derived sentence "Application <ApplicationName> cannot be stopped while it is in the <status> state" (recorded: 'Application sim2real-flk-stop-<id> cannot be stopped while it is in the READY state') — different wording, lowercase 'stopped', the state unquoted, and the application name interpolated from the request.
Why: the refusal text is part of the contract clients assert on and operators read; the emulator's sentence matches none of AWS's wording, so message-keyed handling of an un-stoppable application fails even though the error code is right.
Fix: the StopApplication handler's state-transition refusal in the kinesisanalyticsv2 service crate — compose AWS's fixed sentence from the application's current status and do not interpolate the application name.
Traces: glue__kinesis__kinesisanalyticsv2.stop-and-force-stop-of-a-never-started-studio-notebook-rejected

## Evidence
AssertionError: StopApplication(Force=False) on READY gave ('ResourceInUseException', 'Application sim2real-flk-stop-5f6bef02675d cannot be stopped while it is in the READY state'), expected ('ResourceInUseException', "Application cannot be Stopped in 'READY' state"). The same divergence applies to the Force=true arm; status and version are otherwise left untouched as required.

## Steps to Reproduce
1. glue:CreateDatabase and create an IAM role for the Studio notebook.
2. kinesisanalyticsv2:CreateApplication with ApplicationMode='INTERACTIVE', a Zeppelin runtime, and ApplicationConfiguration.ZeppelinApplicationConfiguration.CatalogConfiguration.GlueDataCatalogConfiguration.DatabaseARN pointing at that database; DescribeApplication shows ApplicationStatus 'READY'.
3. kinesisanalyticsv2:StopApplication(ApplicationName=<app>) and then StopApplication(ApplicationName=<app>, Force=true).
4. Read Error.Code and Error.Message from both refusals.
