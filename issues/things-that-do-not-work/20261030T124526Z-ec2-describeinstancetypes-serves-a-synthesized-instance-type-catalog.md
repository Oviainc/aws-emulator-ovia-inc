Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-catalog-describe-instance-types-m6i-r4
Issue-Step: task#26
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-catalog-describe-instance-types-m6i-r4 ec2-catalog-describe-instance-types-c9gd-m6gd ec2-catalog-describe-instance-types-r9g-z1d ec2-catalog-describe-instance-types-a1-c9g ec2-catalog-describe-instance-types-r5-r8in
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.memoryInfo.sizeInMiB
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.networkInfo.ipv4AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.networkInfo.ipv4AddressesPerSecondaryInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.networkInfo.ipv6AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.networkInfo.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.nitroTpmSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.placementGroupInfo.supportedStrategies
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.supportedBootModes.item
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.instanceStorageInfo
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.instanceStorageSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.networkInfo.encryptionInTransitSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.vCpuInfo.defaultCores
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.vCpuInfo.defaultThreadsPerCore
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.autoRecoverySupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.ebsInfo.attachmentLimitType
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.gpuInfo
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.processorInfo.manufacturer
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.burstablePerformanceSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.ebsInfo.encryptionSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.networkInfo.connectionTrackingConfiguration
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.supportedRootDeviceTypes.item
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.supportedVirtualizationTypes.item
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.2xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.2xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.2xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.2xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.2xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.2xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.4xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.4xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.4xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.4xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.4xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.4xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.12xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.12xlarge"].currentGeneration
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.12xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.12xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.12xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.12xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.12xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.12xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.12xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.12xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.12xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.12xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.12xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.12xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.12xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.12xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.12xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.12xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.12xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.12xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.12xlarge"].vCpuInfo.validCores.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.16xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.16xlarge"].currentGeneration
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.16xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.16xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.16xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.16xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.16xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.16xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.16xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.16xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.16xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.16xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.16xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.16xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.16xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.16xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.16xlarge"].vCpuInfo.validCores.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.24xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.24xlarge"].currentGeneration
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.24xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.24xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.24xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.24xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.24xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.24xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.24xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.24xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.24xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.24xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.24xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.24xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.24xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.24xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.24xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.24xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.24xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.24xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.24xlarge"].vCpuInfo.validCores.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.2xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.2xlarge"].currentGeneration
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.2xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.2xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.2xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.2xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.2xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.2xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.2xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.2xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.2xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.2xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.2xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.2xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.2xlarge"].vCpuInfo.validCores.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.4xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.4xlarge"].currentGeneration
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.4xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.4xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.4xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.4xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.4xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.4xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.4xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.4xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.4xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.4xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.4xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.4xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.4xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.4xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.4xlarge"].vCpuInfo.validCores.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.8xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.8xlarge"].currentGeneration
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.8xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.8xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.8xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.8xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.8xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.8xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.8xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.8xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.8xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.8xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.8xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.8xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.8xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.8xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.8xlarge"].vCpuInfo.validCores.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.large"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.large"].currentGeneration
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.large"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.large"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.large"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.large"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.large"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.large"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.large"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.large"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.large"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.large"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.large"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.large"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.large"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.large"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.large"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.metal"].autoRecoverySupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.metal"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.metal"].currentGeneration
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.metal"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.metal"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.metal"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.metal"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.metal"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.metal"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.metal"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.metal"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.metal"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.metal"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.metal"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.metal"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.metal"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.metal"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.metal"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.metal"].nitroTpmSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.metal"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.metal"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.metal"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.metal"].supportedBootModes.item
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.xlarge"].currentGeneration
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5.xlarge"].vCpuInfo.validCores.item
Issue-Related: 20261029T200817Z-ec2-read-paths-omit-modeled-members-and-serve.md
Issue-Related: 20261030T000411Z-ec2-read-paths-serve-emulator-authored-catalog-and.md
Issue-Related: 20261030T063325Z-ec2-static-catalogs-diverge-from-aws-instance-type.md
Issue-Commit: e72ab6d9ecc28f469f99eceb42a3c20602b3ead9

# ec2 DescribeInstanceTypes serves a synthesized instance-type catalog instead of the real attribute values

> _Note: this issue was identified against the code base at commit `e72ab6d9ecc28f469f99eceb42a3c20602b3ead9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceTypes

## Description
DescribeInstanceTypes answers HTTP 200 with a structurally valid ec2Query body, but the InstanceTypeInfo it serves is computed from a generic size/family formula rather than taken from the real EC2 instance-type catalog: whole modeled members are left out of every item, and most numeric capability members carry formula-derived values that do not match the instance type that was asked for. The replay clusters this by request, so the one defect appears as 170 DescribeInstanceTypes divergence clusters across the five catalog fixtures; the nine causes below are one cluster each, spanning the distinct symptom groups (omitted members, EBS-optimized numbers, memory, network, storage, GPU, processor, vCPU).

Cause 1 of 9 (fixture ec2-catalog-describe-instance-types-m6i-r4, step task#26) - omitted modeled members plus wrong EBS/memory/network numbers.
AWS: Smithy: DescribeInstanceTypes -> InstanceTypeInfo; NetworkInfo models defaultNetworkCardIndex (DefaultNetworkCardIndex), networkCards (NetworkCardInfoList), maximumNetworkCards, enaSrdSupported, flexibleEnaQueuesSupport, ipv4AddressesPerSecondaryInterface, connectionTrackingConfiguration and secondaryNetworkSupported; InstanceTypeInfo additionally carries cpuBurstModel, phcSupport, rebootMigrationSupport and nitroTpmSupport. For each requested instance type AWS emits these members with that type's published catalog values (recorded behavior for the concrete numbers).
Emulator: those members are absent from the item altogether, and the numbers it does emit are unrelated to the type: ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps 575 instead of 10000, baselineIops 3000 instead of 55000, baselineThroughputInMBps 71.875 instead of 1250.0, maximumBandwidthInMbps 4750 instead of 10000, maximumIops 18750 instead of 55000, maximumThroughputInMBps 593.75 instead of 1250.0, ebsInfo.maximumEbsAttachments 28 instead of 10, memoryInfo.sizeInMiB 98304 instead of 262144, networkInfo.ipv4AddressesPerInterface 10 instead of 30, ipv6AddressesPerInterface 10 instead of 30.
Why: a client that sizes workloads or validates capability from this call (an autoscaler picking a type by memory, an ENI/IP-per-interface planner, an EBS throughput budget check) reads values for a machine that does not exist, and code that dereferences networkInfo.networkCards or defaultNetworkCardIndex - members AWS always returns - hits a missing element and fails to parse.
Fix: the DescribeInstanceTypes handler's instance-type catalog (the table the handler reads InstanceTypeInfo from) must hold per-type published attributes instead of a size-derived formula, and the response serializer must emit the modeled optional members it currently drops.

Cause 2 of 9 (fixture ec2-catalog-describe-instance-types-m6i-r4, step task#27) - instance-store attributes dropped for a type that has local storage.
AWS: InstanceTypeInfo.instanceStorageSupported is true and instanceStorageInfo is present (disks item with count/sizeInGB/type, encryptionSupport, nvmeSupport, totalSizeInGB) for an instance type with local NVMe storage; networkInfo.encryptionInTransitSupported is true for a type that encrypts inter-instance traffic (recorded behavior).
Emulator: instanceStorageSupported 'false', instanceStorageInfo absent, networkInfo.encryptionInTransitSupported 'false', plus the same omitted members and wrong EBS-optimized numbers as Cause 1 (baselineBandwidthInMbps 575 instead of 8000, maximumEbsAttachments 28 instead of 31).
Why: callers that filter instance types by local-storage capability get an empty or wrong answer and will reject types that do support instance store.
Fix: same catalog table in the DescribeInstanceTypes handler - instance-store and in-transit-encryption facts must come from the per-type catalog entry.

Cause 3 of 9 (fixture ec2-catalog-describe-instance-types-m6i-r4, step task#28) - same omissions on a multi-type page.
AWS: every item of instanceTypeSet carries its own catalog values for the members listed in Cause 1.
Emulator: cpuBurstModel, defaultNetworkCardIndex, enaSrdSupported, flexibleEnaQueuesSupport, ipv4AddressesPerSecondaryInterface, maximumNetworkCards, networkCards, nitroTpmSupport, phcSupport and rebootMigrationSupport missing, placementGroupInfo.supportedStrategies, processorInfo.sustainedClockSpeedInGhz, supportedBootModes and the EBS-optimized numbers wrong.
Why: the defect is not confined to a single-type lookup - a paged catalog walk returns the same wrong data for every item.
Fix: as Cause 1.

Cause 4 of 9 (fixture ec2-catalog-describe-instance-types-m6i-r4, step task#30) - interface counts and EBS ceilings wrong for a large type.
AWS: networkInfo.maximumNetworkInterfaces and the ebsOptimizedInfo ceilings scale with the published catalog entry of that type.
Emulator: maximumNetworkInterfaces 3 instead of 8, baselineBandwidthInMbps 575 instead of 14000, baselineIops 3000 instead of 80000, baselineThroughputInMBps 71.875 instead of 1750.0, maximumBandwidthInMbps 4750 instead of 14000, maximumIops 18750 instead of 80000, maximumEbsAttachments 28 instead of 16.
Why: ENI-count and EBS-attachment planning built on this call is wrong in both directions (too few interfaces, too many attachments allowed).
Fix: as Cause 1.

Cause 5 of 9 (fixture ec2-catalog-describe-instance-types-m6i-r4, step task#31) - vCPU topology wrong.
AWS: vCpuInfo.defaultCores and defaultThreadsPerCore describe the real topology of the requested type.
Emulator: both diverge from the recorded catalog values, alongside the Cause 1 omissions.
Why: license-/core-based sizing and CpuOptions validation derived from this topology are wrong.
Fix: as Cause 1, vCpuInfo section of the catalog entry.

Cause 6 of 9 (fixture ec2-catalog-describe-instance-types-c9gd-m6gd, step task#16) - accelerator and host attributes invented.
AWS: for an accelerated type, gpuInfo is present (gpus item with count, manufacturer, name, memoryInfo.sizeInMiB, plus totalGpuMemoryInMiB); autoRecoverySupported, dedicatedHostsSupported, ebsInfo.attachmentLimitType and processorInfo.manufacturer come from the catalog entry.
Emulator: gpuInfo and instanceStorageInfo absent, instanceStorageSupported 'false' instead of 'true', autoRecoverySupported 'true' instead of 'false', dedicatedHostsSupported 'true' instead of 'false', ebsInfo.attachmentLimitType 'shared' instead of 'dedicated', processorInfo.manufacturer wrong, and the EBS-optimized numbers wrong (baselineBandwidthInMbps 4600 instead of 8000, baselineIops 18750 instead of 32000).
Why: a client selecting GPU capacity sees an accelerated type with no accelerator at all, and dedicated-host/placement decisions are taken on inverted booleans.
Fix: as Cause 1, including the gpuInfo and dedicated-host facts.

Cause 7 of 9 (fixture ec2-catalog-describe-instance-types-r9g-z1d, step task#3) - capability booleans inverted.
AWS: burstablePerformanceSupported, ebsInfo.encryptionSupport, supportedRootDeviceTypes and supportedVirtualizationTypes are catalog facts of the requested type; memoryInfo.sizeInMiB is its published memory size.
Emulator: burstablePerformanceSupported 'true' instead of 'false', ebsInfo.encryptionSupport 'unsupported' instead of 'supported', ebsInfo.attachmentLimitType 'dedicated' instead of 'shared', maximumEbsAttachments 27 instead of 39, memoryInfo.sizeInMiB 1024 instead of 627, networkInfo.networkCards / networkPerformance / maximumNetworkCards / ipv6AddressesPerInterface / connectionTrackingConfiguration missing, cpuBurstModel / phcSupport / rebootMigrationSupport missing, supportedRootDeviceTypes and supportedVirtualizationTypes wrong.
Why: an emulator that reports a non-burstable type as burstable (and claims EBS encryption is unsupported) makes capability gating in client code take the opposite branch from production.
Fix: as Cause 1.

Cause 8 of 9 (fixture ec2-catalog-describe-instance-types-a1-c9g, step task#3) - previous-generation family page wholly wrong.
AWS: each item of the page carries its own catalog values, including instanceStorageInfo.disks count/sizeInGB and totalSizeInGB, memoryInfo.sizeInMiB, ebsInfo.ebsOptimizedSupport, placementGroupInfo.supportedStrategies, processorInfo.sustainedClockSpeedInGhz and the full vCpuInfo.validCores list.
Emulator: for the items on this page instanceStorageInfo.disks.count 1 instead of 2 and sizeInGB 800 instead of 320 (totalSizeInGB 800 instead of 640), memoryInfo.sizeInMiB 65536 instead of 61440, ebsInfo.ebsOptimizedSupport 'supported' instead of 'unsupported', attachmentLimitType 'dedicated' instead of 'shared', maximumEbsAttachments 27 instead of 39, autoRecoverySupported / dedicatedHostsSupported inverted, supportedStrategies and validCores entries wrong, cpuBurstModel / networkCards / defaultNetworkCardIndex / connectionTrackingConfiguration / phcSupport / rebootMigrationSupport missing.
Why: the formula also mis-reports EBS-optimization support, so a client can be told a type supports EBS optimization that AWS reports as unsupported.
Fix: as Cause 1.

Cause 9 of 9 (fixture ec2-catalog-describe-instance-types-r5-r8in, step task#1) - generation flag and network performance.
AWS: currentGeneration reflects the type's generation status and networkInfo.networkPerformance is always present (a string such as 'Up to 10 Gigabit'), with networkCards describing the card including baselineBandwidthInGbps, peakBandwidthInGbps, defaultEnaQueueCountPerInterface and maximumNetworkInterfaces.
Emulator: currentGeneration 'true' instead of 'false' for the previous-generation items, networkPerformance and networkCards absent, maximumNetworkCards / defaultNetworkCardIndex / enaSrdSupported / flexibleEnaQueuesSupport / cpuBurstModel / phcSupport / rebootMigrationSupport absent, ebsOptimizedInfo baselines and maximumEbsAttachments off by the formula, vCpuInfo.validCores wrong.
Why: 'currentGeneration' is what clients use to steer off retired families, so an all-true answer defeats that check, and a missing networkPerformance string breaks consumers that display or parse it.
Fix: as Cause 1.

Scope note: the replay reports 170 DescribeInstanceTypes clusters with this cause across fixtures ec2-catalog-describe-instance-types-a1-c9g, -c9gd-m6gd, -m6i-r4, -r5-r8in and -r9g-z1d; they are not separate defects but the same catalog table, so the fix must be verified by re-replaying the whole corpus, not only the nine steps named here.

## Evidence
Replay (trace set simparity-wire-aws-v1), all 200 -> 200 with wrong/omitted body members. Examples, verbatim from the divergence details: fixture ec2-catalog-describe-instance-types-m6i-r4 step task#26 - '$...instanceTypeSet.item.cpuBurstModel: missing (expected none, got None)', '$...ebsInfo.ebsOptimizedInfo.baselineIops: value_mismatch (expected 55000, got 3000)', '$...memoryInfo.sizeInMiB: value_mismatch (expected 262144, got 98304)', '$...networkInfo.networkCards: missing', '$...networkInfo.defaultNetworkCardIndex: missing (expected 0, got None)'; step task#27 - '$...instanceStorageSupported: value_mismatch (expected true, got false)', '$...instanceStorageInfo: missing'; step task#30 - '$...networkInfo.maximumNetworkInterfaces: value_mismatch (expected 8, got 3)'; fixture -c9gd-m6gd step task#16 - '$...gpuInfo: missing', '$...dedicatedHostsSupported: value_mismatch (expected false, got true)'; fixture -r9g-z1d step task#3 - '$...burstablePerformanceSupported: value_mismatch (expected false, got true)', '$...ebsInfo.encryptionSupport: value_mismatch (expected supported, got unsupported)'; fixture -a1-c9g step task#3 - '$...ebsInfo.ebsOptimizedSupport: value_mismatch (expected unsupported, got supported)', '$...instanceStorageInfo.disks.item.sizeInGB: value_mismatch (expected 320, got 800)'; fixture -r5-r8in step task#1 - '$...currentGeneration: value_mismatch (expected false, got true)', '$...networkInfo.networkPerformance: missing (expected Up to 10 Gigabit, got None)'. Smithy model (services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json): NetworkInfo models networkPerformance, maximumNetworkCards, defaultNetworkCardIndex, networkCards, enaSrdSupported, flexibleEnaQueuesSupport, connectionTrackingConfiguration, secondaryNetworkSupported, ipv4AddressesPerSecondaryInterface - all of which the emulator omits.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture ec2-catalog-describe-instance-types-m6i-r4 step task#26: DescribeInstanceTypes for a single instance type. Compare the returned InstanceTypeInfo against the recorded one - the EBS-optimized, memory and network numbers differ and cpuBurstModel, phcSupport, rebootMigrationSupport and the NetworkInfo members defaultNetworkCardIndex/maximumNetworkCards/networkCards/enaSrdSupported/flexibleEnaQueuesSupport are absent.
3. Repeat for steps task#27, task#28, task#30, task#31 of the same fixture and for ec2-catalog-describe-instance-types-c9gd-m6gd#task#16, ec2-catalog-describe-instance-types-r9g-z1d#task#3, ec2-catalog-describe-instance-types-a1-c9g#task#3 and ec2-catalog-describe-instance-types-r5-r8in#task#1 for the storage, GPU, capability-boolean, previous-generation and currentGeneration/networkPerformance variants.
4. After the fix, re-replay the full corpus: all 170 DescribeInstanceTypes clusters must pass, not only these nine steps.
