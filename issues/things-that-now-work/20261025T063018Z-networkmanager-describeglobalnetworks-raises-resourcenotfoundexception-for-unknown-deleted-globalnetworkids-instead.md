Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: networkmanager.describe-global-networks-filters-by-id networkmanager.global-network-lifecycle-delete-transition
Issue-Shape-Id: com.amazonaws.networkmanager#NetworkManager
Issue-Key: networkmanager|DescribeGlobalNetworks|ResourceNotFoundException
Issue-Commit: 99dcc2e48d0d0b5cf2de71600dd2f7a8b27364a7

# DescribeGlobalNetworks raises ResourceNotFoundException for unknown/deleted GlobalNetworkIds instead of filtering them out

> _Note: this issue was identified against the code base at commit `99dcc2e48d0d0b5cf2de71600dd2f7a8b27364a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeGlobalNetworks

## Description
This issue carries ONE net-new root cause; it must be closed before submitting.

Cause 1 of 1: DescribeGlobalNetworks refuses the whole request with ResourceNotFoundException (404) when the GlobalNetworkIds filter names an id that does not exist (never created, or already deleted), instead of returning the matching subset.

AWS: Smithy: DescribeGlobalNetworks -> GET /global-networks, http code 200, input member GlobalNetworkIds targets GlobalNetworkIdList and is bound as httpQuery "globalNetworkIds"; output DescribeGlobalNetworksResponse{GlobalNetworks: GlobalNetworkList, NextToken}; the operation is @paginated (inputToken NextToken, outputToken NextToken, items GlobalNetworks, pageSize MaxResults). ResourceNotFoundException (@httpError 404) is modeled on the operation, but recorded behavior shows it is NOT used for an unmatched filter value: GlobalNetworkIds is a selector over the caller's global networks, so AWS answers 200 with only the ids that resolve. A request mixing one real and one unknown id returns 200 with exactly the real network; a request naming only an unknown id returns 200 with GlobalNetworks = [] (recorded behavior only for the empty-list case — the model states only the 200/GlobalNetworks shape). This is what makes DescribeGlobalNetworks usable as a delete-completion probe: after DeleteGlobalNetwork the id stops being listed and the call keeps answering 200 with an empty list.

Emulator: both shapes of the request are refused. botocore raises ResourceNotFoundException (HTTP 404) from DescribeGlobalNetworks when the GlobalNetworkIds list contains an id with no stored record — both when the list also contains a valid id (check networkmanager.describe-global-networks-filters-by-id, first call: GlobalNetworkIds=[<real gn id>, "global-network-ffffffffffffffff"]) and when the id was valid but has since been deleted (check networkmanager.global-network-lifecycle-delete-transition, the post-DeleteGlobalNetwork poll on the same id). Recorded error example: code ResourceNotFoundException, message "Resource not found.". Describing only existing ids works, so the defect is specifically the not-found branch of the filter, not the operation as a whole. Hypothesis: the handler resolves each requested id through the same single-resource lookup that Get*/Update* use and propagates its 404 instead of collecting hits and skipping misses.

Why: a client cannot use the documented filter at all unless it already knows every id is live, which defeats the purpose of the filter. Two concrete breakages: (a) a caller that passes a batch of ids it holds (e.g. from its own inventory, where one has been deleted out from under it) loses the networks that DO exist, because the whole call is refused rather than returning the survivors; (b) every delete-confirmation loop that polls DescribeGlobalNetworks until the id disappears — the pattern AWS's own empty-list behavior supports — crashes with an unhandled ResourceNotFoundException instead of observing an empty list, so teardown and any wait-for-deleted logic aborts. Clients also cannot distinguish "this global network is gone" from "this request was rejected".

Fix: the DescribeGlobalNetworks handler and its id-filter path in the networkmanager crate (handlers under services/service-com-amazonaws-networkmanager-networkmanager/src/ops/, store in src/state.rs per the crate layout cited by open issue 20261023T132108Z-networkmanager-resource-state-machine-mutations-and-deletes-not.md). The filter must be evaluated as a set intersection against the stored global networks — ids with no record are dropped, the response is always 200 with GlobalNetworks containing the resolved subset (possibly empty) — rather than per-id resolution that surfaces a single-resource 404. Note the open issue above is about Update*/Delete* persistence and state transitions and about GetCoreNetwork still answering 200 after delete; it does not name DescribeGlobalNetworks and does not cover this 404-on-filter behavior.

Traces: networkmanager.describe-global-networks-filters-by-id, networkmanager.global-network-lifecycle-delete-transition.

## Evidence
Behavioral replay against the emulator built from mainline (networkmanager 4/6 checks passed, bootstrap_signal=false, so the service is serving and CreateGlobalNetwork/DescribeGlobalNetworks-on-a-live-id both succeed).

1) networkmanager.describe-global-networks-filters-by-id [error]
   Setup succeeded: CreateGlobalNetwork returned an id and DescribeGlobalNetworks(GlobalNetworkIds=[gn_id]) polled to State == 'AVAILABLE'.
   Expected: describe_global_networks(GlobalNetworkIds=[gn_id, 'global-network-ffffffffffffffff']) -> HTTP 200, GlobalNetworks == [the one real network]; then describe_global_networks(GlobalNetworkIds=['global-network-ffffffffffffffff']) -> HTTP 200, GlobalNetworks == [].
   Actual: botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeGlobalNetworks operation: Resource not found. (HTTP 404) — raised on the mixed real+unknown call, so the unknown-only call was never reached.

2) networkmanager.global-network-lifecycle-delete-transition [error]
   Create -> poll to AVAILABLE -> DeleteGlobalNetwork all completed; the failure is on the post-delete poll.
   Expected: describe_global_networks(GlobalNetworkIds=[gn_id]) keeps answering HTTP 200 with GlobalNetworks == [] once the delete settles (the check then asserts UpdateGlobalNetwork on the deleted id is refused with ResourceNotFoundException — that assertion was never reached).
   Actual: botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeGlobalNetworks operation: Resource not found. (HTTP 404).

Same exception type, same operation, same message, in both the never-existed and the deleted case — one filter-resolution cause.

Model basis: services/service-com-amazonaws-networkmanager-networkmanager/smithy-model/com-amazonaws-networkmanager-networkmanager.json — DescribeGlobalNetworks: GET /global-networks, code 200, GlobalNetworkIds @httpQuery('globalNetworkIds') -> GlobalNetworkIdList, output {GlobalNetworks, NextToken}, @paginated(items: GlobalNetworks), ResourceNotFoundException @httpError 404 modeled but not triggered by an unmatched filter value.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from mainline.
2. Replay the behavioral suite (simparity-behavioral-ga-prod-v1), service networkmanager.
3. Unknown-id-in-filter reproduction (check networkmanager.describe-global-networks-filters-by-id):
   a. nm.create_global_network(Description=<unique>) -> note GlobalNetworkId.
   b. Poll nm.describe_global_networks(GlobalNetworkIds=[gn_id]) until GlobalNetworks[0].State == 'AVAILABLE' (this succeeds).
   c. Call nm.describe_global_networks(GlobalNetworkIds=[gn_id, 'global-network-ffffffffffffffff'])
      -> observe HTTP 404 ResourceNotFoundException; expected HTTP 200 with GlobalNetworks == [gn_id only].
   d. Call nm.describe_global_networks(GlobalNetworkIds=['global-network-ffffffffffffffff'])
      -> observe HTTP 404 ResourceNotFoundException; expected HTTP 200 with GlobalNetworks == [].
4. Deleted-id reproduction (check networkmanager.global-network-lifecycle-delete-transition):
   a. Create a global network and poll it to AVAILABLE.
   b. nm.delete_global_network(GlobalNetworkId=gn_id).
   c. Poll nm.describe_global_networks(GlobalNetworkIds=[gn_id]) expecting GlobalNetworks == []
      -> observe HTTP 404 ResourceNotFoundException raised out of the poll instead of an empty list.
5. Both calls must answer HTTP 200 with the resolved subset of GlobalNetworks (empty when nothing resolves) for the checks to pass.
