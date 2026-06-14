#[allow(unused_imports)]
use progenitor_client::{encode_path, ClientHooks, OperationInfo, RequestBuilderExt};
#[allow(unused_imports)]
pub use progenitor_client::{ByteStream, ClientInfo, Error, ResponseValue};
/// Types used as operation parameters and responses.
#[allow(clippy::all)]
pub mod types {
    /// Error types.
    pub mod error {
        /// Error from a `TryFrom` or `FromStr` implementation.
        pub struct ConversionError(::std::borrow::Cow<'static, str>);
        impl ::std::error::Error for ConversionError {}
        impl ::std::fmt::Display for ConversionError {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> Result<(), ::std::fmt::Error> {
                ::std::fmt::Display::fmt(&self.0, f)
            }
        }

        impl ::std::fmt::Debug for ConversionError {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> Result<(), ::std::fmt::Error> {
                ::std::fmt::Debug::fmt(&self.0, f)
            }
        }

        impl From<&'static str> for ConversionError {
            fn from(value: &'static str) -> Self {
                Self(value.into())
            }
        }

        impl From<String> for ConversionError {
            fn from(value: String) -> Self {
                Self(value.into())
            }
        }
    }

    ///`AutoDeploy`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "default": "yes",
    ///  "type": "string"
    ///}
    /// ```
    /// </details>
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    #[serde(transparent)]
    pub struct AutoDeploy(pub ::std::string::String);
    impl ::std::ops::Deref for AutoDeploy {
        type Target = ::std::string::String;
        fn deref(&self) -> &::std::string::String {
            &self.0
        }
    }

    impl ::std::convert::From<AutoDeploy> for ::std::string::String {
        fn from(value: AutoDeploy) -> Self {
            value.0
        }
    }

    impl ::std::convert::From<::std::string::String> for AutoDeploy {
        fn from(value: ::std::string::String) -> Self {
            Self(value)
        }
    }

    impl ::std::str::FromStr for AutoDeploy {
        type Err = ::std::convert::Infallible;
        fn from_str(value: &str) -> ::std::result::Result<Self, Self::Err> {
            Ok(Self(value.to_string()))
        }
    }

    impl ::std::fmt::Display for AutoDeploy {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            self.0.fmt(f)
        }
    }

    ///`BackgroundWorkerDetails`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "autoscaling": {
    ///      "type": "object",
    ///      "properties": {
    ///        "criteria": {
    ///          "type": "object",
    ///          "properties": {
    ///            "cpu": {
    ///              "type": "object",
    ///              "properties": {
    ///                "enabled": {
    ///                  "default": false,
    ///                  "type": "boolean"
    ///                },
    ///                "percentage": {
    ///                  "description": "Determines when your service will be
    /// scaled. If the average resource utilization is significantly above/below
    /// the target, we will increase/decrease the number of instances.\n",
    ///                  "type": "integer"
    ///                }
    ///              }
    ///            },
    ///            "memory": {
    ///              "type": "object",
    ///              "properties": {
    ///                "enabled": {
    ///                  "default": false,
    ///                  "type": "boolean"
    ///                },
    ///                "percentage": {
    ///                  "description": "Determines when your service will be
    /// scaled. If the average resource utilization is significantly above/below
    /// the target, we will increase/decrease the number of instances.\n",
    ///                  "type": "integer"
    ///                }
    ///              }
    ///            }
    ///          }
    ///        },
    ///        "enabled": {
    ///          "default": false,
    ///          "type": "boolean"
    ///        },
    ///        "max": {
    ///          "description": "The maximum number of instances for the
    /// service",
    ///          "type": "integer"
    ///        },
    ///        "min": {
    ///          "description": "The minimum number of instances for the
    /// service",
    ///          "type": "integer"
    ///        }
    ///      }
    ///    },
    ///    "buildPlan": {
    ///      "$ref": "#/components/schemas/buildPlan"
    ///    },
    ///    "disk": {
    ///      "type": "object",
    ///      "properties": {
    ///        "id": {
    ///          "examples": [
    ///            "dsk-cph1rs3idesc73a2b2mg"
    ///          ],
    ///          "type": "string",
    ///          "pattern": "^dsk-[0-9a-z]{20}$"
    ///        },
    ///        "mountPath": {
    ///          "type": "string"
    ///        },
    ///        "name": {
    ///          "type": "string"
    ///        },
    ///        "sizeGB": {
    ///          "type": "integer"
    ///        }
    ///      }
    ///    },
    ///    "env": {
    ///      "$ref": "#/components/schemas/serviceEnv"
    ///    },
    ///    "envSpecificDetails": {
    ///      "$ref": "#/components/schemas/envSpecificDetails"
    ///    },
    ///    "maxShutdownDelaySeconds": {
    ///      "$ref": "#/components/schemas/maxShutdownDelaySeconds"
    ///    },
    ///    "numInstances": {
    ///      "description": "For a *manually* scaled service, this is the number
    /// of instances the service is scaled to. DOES NOT indicate the number of
    /// running instances for an *autoscaled* service.",
    ///      "type": "integer"
    ///    },
    ///    "parentServer": {
    ///      "$ref": "#/components/schemas/resource"
    ///    },
    ///    "plan": {
    ///      "$ref": "#/components/schemas/plan"
    ///    },
    ///    "previews": {
    ///      "$ref": "#/components/schemas/previews"
    ///    },
    ///    "pullRequestPreviewsEnabled": {
    ///      "$ref": "#/components/schemas/pullRequestPreviewsEnabled"
    ///    },
    ///    "region": {
    ///      "$ref": "#/components/schemas/region"
    ///    },
    ///    "runtime": {
    ///      "$ref": "#/components/schemas/serviceRuntime"
    ///    },
    ///    "sshAddress": {
    ///      "$ref": "#/components/schemas/sshAddress"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct BackgroundWorkerDetails {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub autoscaling: ::std::option::Option<BackgroundWorkerDetailsAutoscaling>,
        #[serde(
            rename = "buildPlan",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub build_plan: ::std::option::Option<BuildPlan>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub disk: ::std::option::Option<BackgroundWorkerDetailsDisk>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub env: ::std::option::Option<ServiceEnv>,
        #[serde(
            rename = "envSpecificDetails",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub env_specific_details: ::std::option::Option<EnvSpecificDetails>,
        #[serde(
            rename = "maxShutdownDelaySeconds",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub max_shutdown_delay_seconds: ::std::option::Option<MaxShutdownDelaySeconds>,
        ///For a *manually* scaled service, this is the number of instances the
        /// service is scaled to. DOES NOT indicate the number of running
        /// instances for an *autoscaled* service.
        #[serde(
            rename = "numInstances",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub num_instances: ::std::option::Option<i64>,
        #[serde(
            rename = "parentServer",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub parent_server: ::std::option::Option<Resource>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub plan: ::std::option::Option<Plan>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub previews: ::std::option::Option<Previews>,
        #[serde(
            rename = "pullRequestPreviewsEnabled",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub pull_request_previews_enabled: ::std::option::Option<PullRequestPreviewsEnabled>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub region: ::std::option::Option<Region>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub runtime: ::std::option::Option<ServiceRuntime>,
        #[serde(
            rename = "sshAddress",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub ssh_address: ::std::option::Option<SshAddress>,
    }

    impl ::std::default::Default for BackgroundWorkerDetails {
        fn default() -> Self {
            Self {
                autoscaling: Default::default(),
                build_plan: Default::default(),
                disk: Default::default(),
                env: Default::default(),
                env_specific_details: Default::default(),
                max_shutdown_delay_seconds: Default::default(),
                num_instances: Default::default(),
                parent_server: Default::default(),
                plan: Default::default(),
                previews: Default::default(),
                pull_request_previews_enabled: Default::default(),
                region: Default::default(),
                runtime: Default::default(),
                ssh_address: Default::default(),
            }
        }
    }

    ///`BackgroundWorkerDetailsAutoscaling`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "criteria": {
    ///      "type": "object",
    ///      "properties": {
    ///        "cpu": {
    ///          "type": "object",
    ///          "properties": {
    ///            "enabled": {
    ///              "default": false,
    ///              "type": "boolean"
    ///            },
    ///            "percentage": {
    ///              "description": "Determines when your service will be
    /// scaled. If the average resource utilization is significantly above/below
    /// the target, we will increase/decrease the number of instances.\n",
    ///              "type": "integer"
    ///            }
    ///          }
    ///        },
    ///        "memory": {
    ///          "type": "object",
    ///          "properties": {
    ///            "enabled": {
    ///              "default": false,
    ///              "type": "boolean"
    ///            },
    ///            "percentage": {
    ///              "description": "Determines when your service will be
    /// scaled. If the average resource utilization is significantly above/below
    /// the target, we will increase/decrease the number of instances.\n",
    ///              "type": "integer"
    ///            }
    ///          }
    ///        }
    ///      }
    ///    },
    ///    "enabled": {
    ///      "default": false,
    ///      "type": "boolean"
    ///    },
    ///    "max": {
    ///      "description": "The maximum number of instances for the service",
    ///      "type": "integer"
    ///    },
    ///    "min": {
    ///      "description": "The minimum number of instances for the service",
    ///      "type": "integer"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct BackgroundWorkerDetailsAutoscaling {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub criteria: ::std::option::Option<BackgroundWorkerDetailsAutoscalingCriteria>,
        #[serde(default)]
        pub enabled: bool,
        ///The maximum number of instances for the service
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub max: ::std::option::Option<i64>,
        ///The minimum number of instances for the service
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub min: ::std::option::Option<i64>,
    }

    impl ::std::default::Default for BackgroundWorkerDetailsAutoscaling {
        fn default() -> Self {
            Self {
                criteria: Default::default(),
                enabled: Default::default(),
                max: Default::default(),
                min: Default::default(),
            }
        }
    }

    ///`BackgroundWorkerDetailsAutoscalingCriteria`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "cpu": {
    ///      "type": "object",
    ///      "properties": {
    ///        "enabled": {
    ///          "default": false,
    ///          "type": "boolean"
    ///        },
    ///        "percentage": {
    ///          "description": "Determines when your service will be scaled. If
    /// the average resource utilization is significantly above/below the
    /// target, we will increase/decrease the number of instances.\n",
    ///          "type": "integer"
    ///        }
    ///      }
    ///    },
    ///    "memory": {
    ///      "type": "object",
    ///      "properties": {
    ///        "enabled": {
    ///          "default": false,
    ///          "type": "boolean"
    ///        },
    ///        "percentage": {
    ///          "description": "Determines when your service will be scaled. If
    /// the average resource utilization is significantly above/below the
    /// target, we will increase/decrease the number of instances.\n",
    ///          "type": "integer"
    ///        }
    ///      }
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct BackgroundWorkerDetailsAutoscalingCriteria {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub cpu: ::std::option::Option<BackgroundWorkerDetailsAutoscalingCriteriaCpu>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub memory: ::std::option::Option<BackgroundWorkerDetailsAutoscalingCriteriaMemory>,
    }

    impl ::std::default::Default for BackgroundWorkerDetailsAutoscalingCriteria {
        fn default() -> Self {
            Self {
                cpu: Default::default(),
                memory: Default::default(),
            }
        }
    }

    ///`BackgroundWorkerDetailsAutoscalingCriteriaCpu`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "enabled": {
    ///      "default": false,
    ///      "type": "boolean"
    ///    },
    ///    "percentage": {
    ///      "description": "Determines when your service will be scaled. If the
    /// average resource utilization is significantly above/below the target, we
    /// will increase/decrease the number of instances.\n",
    ///      "type": "integer"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct BackgroundWorkerDetailsAutoscalingCriteriaCpu {
        #[serde(default)]
        pub enabled: bool,
        ///Determines when your service will be scaled. If the average resource
        /// utilization is significantly above/below the target, we will
        /// increase/decrease the number of instances.
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub percentage: ::std::option::Option<i64>,
    }

    impl ::std::default::Default for BackgroundWorkerDetailsAutoscalingCriteriaCpu {
        fn default() -> Self {
            Self {
                enabled: Default::default(),
                percentage: Default::default(),
            }
        }
    }

    ///`BackgroundWorkerDetailsAutoscalingCriteriaMemory`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "enabled": {
    ///      "default": false,
    ///      "type": "boolean"
    ///    },
    ///    "percentage": {
    ///      "description": "Determines when your service will be scaled. If the
    /// average resource utilization is significantly above/below the target, we
    /// will increase/decrease the number of instances.\n",
    ///      "type": "integer"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct BackgroundWorkerDetailsAutoscalingCriteriaMemory {
        #[serde(default)]
        pub enabled: bool,
        ///Determines when your service will be scaled. If the average resource
        /// utilization is significantly above/below the target, we will
        /// increase/decrease the number of instances.
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub percentage: ::std::option::Option<i64>,
    }

    impl ::std::default::Default for BackgroundWorkerDetailsAutoscalingCriteriaMemory {
        fn default() -> Self {
            Self {
                enabled: Default::default(),
                percentage: Default::default(),
            }
        }
    }

    ///`BackgroundWorkerDetailsDisk`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "id": {
    ///      "examples": [
    ///        "dsk-cph1rs3idesc73a2b2mg"
    ///      ],
    ///      "type": "string",
    ///      "pattern": "^dsk-[0-9a-z]{20}$"
    ///    },
    ///    "mountPath": {
    ///      "type": "string"
    ///    },
    ///    "name": {
    ///      "type": "string"
    ///    },
    ///    "sizeGB": {
    ///      "type": "integer"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct BackgroundWorkerDetailsDisk {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub id: ::std::option::Option<BackgroundWorkerDetailsDiskId>,
        #[serde(
            rename = "mountPath",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub mount_path: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub name: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "sizeGB",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub size_gb: ::std::option::Option<i64>,
    }

    impl ::std::default::Default for BackgroundWorkerDetailsDisk {
        fn default() -> Self {
            Self {
                id: Default::default(),
                mount_path: Default::default(),
                name: Default::default(),
                size_gb: Default::default(),
            }
        }
    }

    ///`BackgroundWorkerDetailsDiskId`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "examples": [
    ///    "dsk-cph1rs3idesc73a2b2mg"
    ///  ],
    ///  "type": "string",
    ///  "pattern": "^dsk-[0-9a-z]{20}$"
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
    #[serde(transparent)]
    pub struct BackgroundWorkerDetailsDiskId(::std::string::String);
    impl ::std::ops::Deref for BackgroundWorkerDetailsDiskId {
        type Target = ::std::string::String;
        fn deref(&self) -> &::std::string::String {
            &self.0
        }
    }

    impl ::std::convert::From<BackgroundWorkerDetailsDiskId> for ::std::string::String {
        fn from(value: BackgroundWorkerDetailsDiskId) -> Self {
            value.0
        }
    }

    impl ::std::str::FromStr for BackgroundWorkerDetailsDiskId {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
                ::std::sync::LazyLock::new(|| ::regress::Regex::new("^dsk-[0-9a-z]{20}$").unwrap());
            if PATTERN.find(value).is_none() {
                return Err("doesn't match pattern \"^dsk-[0-9a-z]{20}$\"".into());
            }
            Ok(Self(value.to_string()))
        }
    }

    impl ::std::convert::TryFrom<&str> for BackgroundWorkerDetailsDiskId {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }

    impl ::std::convert::TryFrom<&::std::string::String> for BackgroundWorkerDetailsDiskId {
        type Error = self::error::ConversionError;
        fn try_from(
            value: &::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }

    impl ::std::convert::TryFrom<::std::string::String> for BackgroundWorkerDetailsDiskId {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }

    impl<'de> ::serde::Deserialize<'de> for BackgroundWorkerDetailsDiskId {
        fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
        where
            D: ::serde::Deserializer<'de>,
        {
            ::std::string::String::deserialize(deserializer)?
                .parse()
                .map_err(|e: self::error::ConversionError| {
                    <D::Error as ::serde::de::Error>::custom(e.to_string())
                })
        }
    }

    ///`BackgroundWorkerDetailsPost`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "autoscaling": {
    ///      "type": "object",
    ///      "properties": {
    ///        "criteria": {
    ///          "type": "object",
    ///          "properties": {
    ///            "cpu": {
    ///              "type": "object",
    ///              "properties": {
    ///                "enabled": {
    ///                  "default": false,
    ///                  "type": "boolean"
    ///                },
    ///                "percentage": {
    ///                  "description": "Determines when your service will be
    /// scaled. If the average resource utilization is significantly above/below
    /// the target, we will increase/decrease the number of instances.\n",
    ///                  "type": "integer"
    ///                }
    ///              }
    ///            },
    ///            "memory": {
    ///              "type": "object",
    ///              "properties": {
    ///                "enabled": {
    ///                  "default": false,
    ///                  "type": "boolean"
    ///                },
    ///                "percentage": {
    ///                  "description": "Determines when your service will be
    /// scaled. If the average resource utilization is significantly above/below
    /// the target, we will increase/decrease the number of instances.\n",
    ///                  "type": "integer"
    ///                }
    ///              }
    ///            }
    ///          }
    ///        },
    ///        "enabled": {
    ///          "default": false,
    ///          "type": "boolean"
    ///        },
    ///        "max": {
    ///          "description": "The maximum number of instances for the
    /// service",
    ///          "type": "integer"
    ///        },
    ///        "min": {
    ///          "description": "The minimum number of instances for the
    /// service",
    ///          "type": "integer"
    ///        }
    ///      }
    ///    },
    ///    "disk": {
    ///      "$ref": "#/components/schemas/serviceDisk"
    ///    },
    ///    "env": {
    ///      "$ref": "#/components/schemas/serviceEnv"
    ///    },
    ///    "envSpecificDetails": {
    ///      "$ref": "#/components/schemas/envSpecificDetailsPOST"
    ///    },
    ///    "maxShutdownDelaySeconds": {
    ///      "$ref": "#/components/schemas/maxShutdownDelaySeconds"
    ///    },
    ///    "numInstances": {
    ///      "description": "Defaults to 1",
    ///      "default": 1,
    ///      "type": "integer",
    ///      "minimum": 1.0
    ///    },
    ///    "plan": {
    ///      "$ref": "#/components/schemas/paidPlan"
    ///    },
    ///    "preDeployCommand": {
    ///      "type": "string"
    ///    },
    ///    "previews": {
    ///      "$ref": "#/components/schemas/previews"
    ///    },
    ///    "pullRequestPreviewsEnabled": {
    ///      "$ref": "#/components/schemas/pullRequestPreviewsEnabled"
    ///    },
    ///    "region": {
    ///      "$ref": "#/components/schemas/region"
    ///    },
    ///    "runtime": {
    ///      "$ref": "#/components/schemas/serviceRuntime"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct BackgroundWorkerDetailsPost {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub autoscaling: ::std::option::Option<BackgroundWorkerDetailsPostAutoscaling>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub disk: ::std::option::Option<ServiceDisk>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub env: ::std::option::Option<ServiceEnv>,
        #[serde(
            rename = "envSpecificDetails",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub env_specific_details: ::std::option::Option<EnvSpecificDetailsPost>,
        #[serde(
            rename = "maxShutdownDelaySeconds",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub max_shutdown_delay_seconds: ::std::option::Option<MaxShutdownDelaySeconds>,
        ///Defaults to 1
        #[serde(
            rename = "numInstances",
            default = "defaults::default_nzu64::<::std::num::NonZeroU64, 1>"
        )]
        pub num_instances: ::std::num::NonZeroU64,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub plan: ::std::option::Option<PaidPlan>,
        #[serde(
            rename = "preDeployCommand",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub pre_deploy_command: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub previews: ::std::option::Option<Previews>,
        #[serde(
            rename = "pullRequestPreviewsEnabled",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub pull_request_previews_enabled: ::std::option::Option<PullRequestPreviewsEnabled>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub region: ::std::option::Option<Region>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub runtime: ::std::option::Option<ServiceRuntime>,
    }

    impl ::std::default::Default for BackgroundWorkerDetailsPost {
        fn default() -> Self {
            Self {
                autoscaling: Default::default(),
                disk: Default::default(),
                env: Default::default(),
                env_specific_details: Default::default(),
                max_shutdown_delay_seconds: Default::default(),
                num_instances: defaults::default_nzu64::<::std::num::NonZeroU64, 1>(),
                plan: Default::default(),
                pre_deploy_command: Default::default(),
                previews: Default::default(),
                pull_request_previews_enabled: Default::default(),
                region: Default::default(),
                runtime: Default::default(),
            }
        }
    }

    ///`BackgroundWorkerDetailsPostAutoscaling`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "criteria": {
    ///      "type": "object",
    ///      "properties": {
    ///        "cpu": {
    ///          "type": "object",
    ///          "properties": {
    ///            "enabled": {
    ///              "default": false,
    ///              "type": "boolean"
    ///            },
    ///            "percentage": {
    ///              "description": "Determines when your service will be
    /// scaled. If the average resource utilization is significantly above/below
    /// the target, we will increase/decrease the number of instances.\n",
    ///              "type": "integer"
    ///            }
    ///          }
    ///        },
    ///        "memory": {
    ///          "type": "object",
    ///          "properties": {
    ///            "enabled": {
    ///              "default": false,
    ///              "type": "boolean"
    ///            },
    ///            "percentage": {
    ///              "description": "Determines when your service will be
    /// scaled. If the average resource utilization is significantly above/below
    /// the target, we will increase/decrease the number of instances.\n",
    ///              "type": "integer"
    ///            }
    ///          }
    ///        }
    ///      }
    ///    },
    ///    "enabled": {
    ///      "default": false,
    ///      "type": "boolean"
    ///    },
    ///    "max": {
    ///      "description": "The maximum number of instances for the service",
    ///      "type": "integer"
    ///    },
    ///    "min": {
    ///      "description": "The minimum number of instances for the service",
    ///      "type": "integer"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct BackgroundWorkerDetailsPostAutoscaling {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub criteria: ::std::option::Option<BackgroundWorkerDetailsPostAutoscalingCriteria>,
        #[serde(default)]
        pub enabled: bool,
        ///The maximum number of instances for the service
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub max: ::std::option::Option<i64>,
        ///The minimum number of instances for the service
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub min: ::std::option::Option<i64>,
    }

    impl ::std::default::Default for BackgroundWorkerDetailsPostAutoscaling {
        fn default() -> Self {
            Self {
                criteria: Default::default(),
                enabled: Default::default(),
                max: Default::default(),
                min: Default::default(),
            }
        }
    }

    ///`BackgroundWorkerDetailsPostAutoscalingCriteria`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "cpu": {
    ///      "type": "object",
    ///      "properties": {
    ///        "enabled": {
    ///          "default": false,
    ///          "type": "boolean"
    ///        },
    ///        "percentage": {
    ///          "description": "Determines when your service will be scaled. If
    /// the average resource utilization is significantly above/below the
    /// target, we will increase/decrease the number of instances.\n",
    ///          "type": "integer"
    ///        }
    ///      }
    ///    },
    ///    "memory": {
    ///      "type": "object",
    ///      "properties": {
    ///        "enabled": {
    ///          "default": false,
    ///          "type": "boolean"
    ///        },
    ///        "percentage": {
    ///          "description": "Determines when your service will be scaled. If
    /// the average resource utilization is significantly above/below the
    /// target, we will increase/decrease the number of instances.\n",
    ///          "type": "integer"
    ///        }
    ///      }
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct BackgroundWorkerDetailsPostAutoscalingCriteria {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub cpu: ::std::option::Option<BackgroundWorkerDetailsPostAutoscalingCriteriaCpu>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub memory: ::std::option::Option<BackgroundWorkerDetailsPostAutoscalingCriteriaMemory>,
    }

    impl ::std::default::Default for BackgroundWorkerDetailsPostAutoscalingCriteria {
        fn default() -> Self {
            Self {
                cpu: Default::default(),
                memory: Default::default(),
            }
        }
    }

    ///`BackgroundWorkerDetailsPostAutoscalingCriteriaCpu`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "enabled": {
    ///      "default": false,
    ///      "type": "boolean"
    ///    },
    ///    "percentage": {
    ///      "description": "Determines when your service will be scaled. If the
    /// average resource utilization is significantly above/below the target, we
    /// will increase/decrease the number of instances.\n",
    ///      "type": "integer"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct BackgroundWorkerDetailsPostAutoscalingCriteriaCpu {
        #[serde(default)]
        pub enabled: bool,
        ///Determines when your service will be scaled. If the average resource
        /// utilization is significantly above/below the target, we will
        /// increase/decrease the number of instances.
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub percentage: ::std::option::Option<i64>,
    }

    impl ::std::default::Default for BackgroundWorkerDetailsPostAutoscalingCriteriaCpu {
        fn default() -> Self {
            Self {
                enabled: Default::default(),
                percentage: Default::default(),
            }
        }
    }

    ///`BackgroundWorkerDetailsPostAutoscalingCriteriaMemory`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "enabled": {
    ///      "default": false,
    ///      "type": "boolean"
    ///    },
    ///    "percentage": {
    ///      "description": "Determines when your service will be scaled. If the
    /// average resource utilization is significantly above/below the target, we
    /// will increase/decrease the number of instances.\n",
    ///      "type": "integer"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct BackgroundWorkerDetailsPostAutoscalingCriteriaMemory {
        #[serde(default)]
        pub enabled: bool,
        ///Determines when your service will be scaled. If the average resource
        /// utilization is significantly above/below the target, we will
        /// increase/decrease the number of instances.
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub percentage: ::std::option::Option<i64>,
    }

    impl ::std::default::Default for BackgroundWorkerDetailsPostAutoscalingCriteriaMemory {
        fn default() -> Self {
            Self {
                enabled: Default::default(),
                percentage: Default::default(),
            }
        }
    }

    ///`BuildFilter`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "ignoredPaths": {
    ///      "type": "array",
    ///      "items": {
    ///        "type": "string"
    ///      }
    ///    },
    ///    "paths": {
    ///      "type": "array",
    ///      "items": {
    ///        "type": "string"
    ///      }
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct BuildFilter {
        #[serde(
            rename = "ignoredPaths",
            default,
            skip_serializing_if = "::std::vec::Vec::is_empty"
        )]
        pub ignored_paths: ::std::vec::Vec<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
        pub paths: ::std::vec::Vec<::std::string::String>,
    }

    impl ::std::default::Default for BuildFilter {
        fn default() -> Self {
            Self {
                ignored_paths: Default::default(),
                paths: Default::default(),
            }
        }
    }

    ///`BuildPlan`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "default": "starter",
    ///  "type": "string"
    ///}
    /// ```
    /// </details>
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    #[serde(transparent)]
    pub struct BuildPlan(pub ::std::string::String);
    impl ::std::ops::Deref for BuildPlan {
        type Target = ::std::string::String;
        fn deref(&self) -> &::std::string::String {
            &self.0
        }
    }

    impl ::std::convert::From<BuildPlan> for ::std::string::String {
        fn from(value: BuildPlan) -> Self {
            value.0
        }
    }

    impl ::std::convert::From<::std::string::String> for BuildPlan {
        fn from(value: ::std::string::String) -> Self {
            Self(value)
        }
    }

    impl ::std::str::FromStr for BuildPlan {
        type Err = ::std::convert::Infallible;
        fn from_str(value: &str) -> ::std::result::Result<Self, Self::Err> {
            Ok(Self(value.to_string()))
        }
    }

    impl ::std::fmt::Display for BuildPlan {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            self.0.fmt(f)
        }
    }

    ///`Cache`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "profile": {
    ///      "default": "no-cache",
    ///      "type": "string"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct Cache {
        #[serde(default = "defaults::cache_profile")]
        pub profile: ::std::string::String,
    }

    impl ::std::default::Default for Cache {
        fn default() -> Self {
            Self {
                profile: defaults::cache_profile(),
            }
        }
    }

    ///`CidrBlockAndDescription`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "cidrBlock": {
    ///      "type": "string"
    ///    },
    ///    "description": {
    ///      "description": "User-provided description of the CIDR block",
    ///      "type": "string"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct CidrBlockAndDescription {
        #[serde(
            rename = "cidrBlock",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub cidr_block: ::std::option::Option<::std::string::String>,
        ///User-provided description of the CIDR block
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub description: ::std::option::Option<::std::string::String>,
    }

    impl ::std::default::Default for CidrBlockAndDescription {
        fn default() -> Self {
            Self {
                cidr_block: Default::default(),
                description: Default::default(),
            }
        }
    }

    ///`CreateDeployBody`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "clearCache": {
    ///      "description": "If `clear`, Render clears the service's build cache
    /// before deploying. This can be useful if you're experiencing issues with
    /// your build.",
    ///      "default": "do_not_clear",
    ///      "type": "string"
    ///    },
    ///    "commitId": {
    ///      "description": "The SHA of a specific Git commit to deploy for a service. Defaults to the latest commit on the service's connected branch.\n\nNote that deploying a specific commit with this endpoint does not disable autodeploys for the service.\n\nYou can toggle autodeploys for your service with the [Update service](https://api-docs.render.com/reference/update-service) endpoint or in the Render Dashboard.\n\nNot supported for cron jobs.\n",
    ///      "type": "string"
    ///    },
    ///    "deployMode": {
    ///      "$ref": "#/components/schemas/DeployMode"
    ///    },
    ///    "imageUrl": {
    ///      "description": "The URL of the image to deploy for an image-backed
    /// service.\n\nThe host, repository, and image name all must match the
    /// currently configured image for the service.\n",
    ///      "type": "string"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct CreateDeployBody {
        ///If `clear`, Render clears the service's build cache before
        /// deploying. This can be useful if you're experiencing issues with
        /// your build.
        #[serde(
            rename = "clearCache",
            default = "defaults::create_deploy_body_clear_cache"
        )]
        pub clear_cache: ::std::string::String,
        ///The SHA of a specific Git commit to deploy for a service. Defaults
        /// to the latest commit on the service's connected branch.
        ///
        ///Note that deploying a specific commit with this endpoint does not
        /// disable autodeploys for the service.
        ///
        ///You can toggle autodeploys for your service with the [Update service](https://api-docs.render.com/reference/update-service) endpoint or in the Render Dashboard.
        ///
        ///Not supported for cron jobs.
        #[serde(
            rename = "commitId",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub commit_id: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "deployMode",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub deploy_mode: ::std::option::Option<DeployMode>,
        ///The URL of the image to deploy for an image-backed service.
        ///
        ///The host, repository, and image name all must match the currently
        /// configured image for the service.
        #[serde(
            rename = "imageUrl",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub image_url: ::std::option::Option<::std::string::String>,
    }

    impl ::std::default::Default for CreateDeployBody {
        fn default() -> Self {
            Self {
                clear_cache: defaults::create_deploy_body_clear_cache(),
                commit_id: Default::default(),
                deploy_mode: Default::default(),
                image_url: Default::default(),
            }
        }
    }

    ///`CronJobDetails`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "buildPlan": {
    ///      "$ref": "#/components/schemas/buildPlan"
    ///    },
    ///    "env": {
    ///      "$ref": "#/components/schemas/serviceEnv"
    ///    },
    ///    "envSpecificDetails": {
    ///      "$ref": "#/components/schemas/envSpecificDetails"
    ///    },
    ///    "lastSuccessfulRunAt": {
    ///      "type": "string",
    ///      "format": "date-time"
    ///    },
    ///    "plan": {
    ///      "$ref": "#/components/schemas/plan"
    ///    },
    ///    "region": {
    ///      "$ref": "#/components/schemas/region"
    ///    },
    ///    "runtime": {
    ///      "$ref": "#/components/schemas/serviceRuntime"
    ///    },
    ///    "schedule": {
    ///      "type": "string"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct CronJobDetails {
        #[serde(
            rename = "buildPlan",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub build_plan: ::std::option::Option<BuildPlan>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub env: ::std::option::Option<ServiceEnv>,
        #[serde(
            rename = "envSpecificDetails",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub env_specific_details: ::std::option::Option<EnvSpecificDetails>,
        #[serde(
            rename = "lastSuccessfulRunAt",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub last_successful_run_at:
            ::std::option::Option<::chrono::DateTime<::chrono::offset::Utc>>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub plan: ::std::option::Option<Plan>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub region: ::std::option::Option<Region>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub runtime: ::std::option::Option<ServiceRuntime>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub schedule: ::std::option::Option<::std::string::String>,
    }

    impl ::std::default::Default for CronJobDetails {
        fn default() -> Self {
            Self {
                build_plan: Default::default(),
                env: Default::default(),
                env_specific_details: Default::default(),
                last_successful_run_at: Default::default(),
                plan: Default::default(),
                region: Default::default(),
                runtime: Default::default(),
                schedule: Default::default(),
            }
        }
    }

    ///`CronJobDetailsPost`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "env": {
    ///      "$ref": "#/components/schemas/serviceEnv"
    ///    },
    ///    "envSpecificDetails": {
    ///      "$ref": "#/components/schemas/envSpecificDetails"
    ///    },
    ///    "plan": {
    ///      "$ref": "#/components/schemas/paidPlan"
    ///    },
    ///    "region": {
    ///      "$ref": "#/components/schemas/region"
    ///    },
    ///    "runtime": {
    ///      "$ref": "#/components/schemas/serviceRuntime"
    ///    },
    ///    "schedule": {
    ///      "type": "string"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct CronJobDetailsPost {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub env: ::std::option::Option<ServiceEnv>,
        #[serde(
            rename = "envSpecificDetails",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub env_specific_details: ::std::option::Option<EnvSpecificDetails>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub plan: ::std::option::Option<PaidPlan>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub region: ::std::option::Option<Region>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub runtime: ::std::option::Option<ServiceRuntime>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub schedule: ::std::option::Option<::std::string::String>,
    }

    impl ::std::default::Default for CronJobDetailsPost {
        fn default() -> Self {
            Self {
                env: Default::default(),
                env_specific_details: Default::default(),
                plan: Default::default(),
                region: Default::default(),
                runtime: Default::default(),
                schedule: Default::default(),
            }
        }
    }

    ///`Cursor`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "string"
    ///}
    /// ```
    /// </details>
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    #[serde(transparent)]
    pub struct Cursor(pub ::std::string::String);
    impl ::std::ops::Deref for Cursor {
        type Target = ::std::string::String;
        fn deref(&self) -> &::std::string::String {
            &self.0
        }
    }

    impl ::std::convert::From<Cursor> for ::std::string::String {
        fn from(value: Cursor) -> Self {
            value.0
        }
    }

    impl ::std::convert::From<::std::string::String> for Cursor {
        fn from(value: ::std::string::String) -> Self {
            Self(value)
        }
    }

    impl ::std::str::FromStr for Cursor {
        type Err = ::std::convert::Infallible;
        fn from_str(value: &str) -> ::std::result::Result<Self, Self::Err> {
            Ok(Self(value.to_string()))
        }
    }

    impl ::std::fmt::Display for Cursor {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            self.0.fmt(f)
        }
    }

    ///`DatabaseRole`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "string"
    ///}
    /// ```
    /// </details>
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    #[serde(transparent)]
    pub struct DatabaseRole(pub ::std::string::String);
    impl ::std::ops::Deref for DatabaseRole {
        type Target = ::std::string::String;
        fn deref(&self) -> &::std::string::String {
            &self.0
        }
    }

    impl ::std::convert::From<DatabaseRole> for ::std::string::String {
        fn from(value: DatabaseRole) -> Self {
            value.0
        }
    }

    impl ::std::convert::From<::std::string::String> for DatabaseRole {
        fn from(value: ::std::string::String) -> Self {
            Self(value)
        }
    }

    impl ::std::str::FromStr for DatabaseRole {
        type Err = ::std::convert::Infallible;
        fn from_str(value: &str) -> ::std::result::Result<Self, Self::Err> {
            Ok(Self(value.to_string()))
        }
    }

    impl ::std::fmt::Display for DatabaseRole {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            self.0.fmt(f)
        }
    }

    ///`DatabaseStatus`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "string"
    ///}
    /// ```
    /// </details>
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    #[serde(transparent)]
    pub struct DatabaseStatus(pub ::std::string::String);
    impl ::std::ops::Deref for DatabaseStatus {
        type Target = ::std::string::String;
        fn deref(&self) -> &::std::string::String {
            &self.0
        }
    }

    impl ::std::convert::From<DatabaseStatus> for ::std::string::String {
        fn from(value: DatabaseStatus) -> Self {
            value.0
        }
    }

    impl ::std::convert::From<::std::string::String> for DatabaseStatus {
        fn from(value: ::std::string::String) -> Self {
            Self(value)
        }
    }

    impl ::std::str::FromStr for DatabaseStatus {
        type Err = ::std::convert::Infallible;
        fn from_str(value: &str) -> ::std::result::Result<Self, Self::Err> {
            Ok(Self(value.to_string()))
        }
    }

    impl ::std::fmt::Display for DatabaseStatus {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            self.0.fmt(f)
        }
    }

    ///`Deploy`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "commit": {
    ///      "type": "object",
    ///      "properties": {
    ///        "createdAt": {
    ///          "type": "string",
    ///          "format": "date-time"
    ///        },
    ///        "id": {
    ///          "type": "string"
    ///        },
    ///        "message": {
    ///          "type": "string"
    ///        }
    ///      }
    ///    },
    ///    "createdAt": {
    ///      "type": "string",
    ///      "format": "date-time"
    ///    },
    ///    "finishedAt": {
    ///      "type": "string",
    ///      "format": "date-time"
    ///    },
    ///    "id": {
    ///      "type": "string"
    ///    },
    ///    "image": {
    ///      "description": "Image information used when creating the deploy.
    /// Not present for Git-backed deploys",
    ///      "type": "object",
    ///      "properties": {
    ///        "ref": {
    ///          "description": "Image reference used when creating the deploy",
    ///          "type": "string"
    ///        },
    ///        "registryCredential": {
    ///          "description": "Name of credential used to pull the image, if
    /// provided",
    ///          "type": "string"
    ///        },
    ///        "sha": {
    ///          "description": "SHA that the image reference was resolved to
    /// when creating the deploy",
    ///          "type": "string"
    ///        }
    ///      }
    ///    },
    ///    "startedAt": {
    ///      "type": "string",
    ///      "format": "date-time"
    ///    },
    ///    "status": {
    ///      "$ref": "#/components/schemas/deployStatus"
    ///    },
    ///    "trigger": {
    ///      "type": "string"
    ///    },
    ///    "updatedAt": {
    ///      "type": "string",
    ///      "format": "date-time"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct Deploy {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub commit: ::std::option::Option<DeployCommit>,
        #[serde(
            rename = "createdAt",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub created_at: ::std::option::Option<::chrono::DateTime<::chrono::offset::Utc>>,
        #[serde(
            rename = "finishedAt",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub finished_at: ::std::option::Option<::chrono::DateTime<::chrono::offset::Utc>>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub id: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub image: ::std::option::Option<DeployImage>,
        #[serde(
            rename = "startedAt",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub started_at: ::std::option::Option<::chrono::DateTime<::chrono::offset::Utc>>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub status: ::std::option::Option<DeployStatus>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub trigger: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "updatedAt",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub updated_at: ::std::option::Option<::chrono::DateTime<::chrono::offset::Utc>>,
    }

    impl ::std::default::Default for Deploy {
        fn default() -> Self {
            Self {
                commit: Default::default(),
                created_at: Default::default(),
                finished_at: Default::default(),
                id: Default::default(),
                image: Default::default(),
                started_at: Default::default(),
                status: Default::default(),
                trigger: Default::default(),
                updated_at: Default::default(),
            }
        }
    }

    ///`DeployCommit`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "createdAt": {
    ///      "type": "string",
    ///      "format": "date-time"
    ///    },
    ///    "id": {
    ///      "type": "string"
    ///    },
    ///    "message": {
    ///      "type": "string"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct DeployCommit {
        #[serde(
            rename = "createdAt",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub created_at: ::std::option::Option<::chrono::DateTime<::chrono::offset::Utc>>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub id: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub message: ::std::option::Option<::std::string::String>,
    }

    impl ::std::default::Default for DeployCommit {
        fn default() -> Self {
            Self {
                created_at: Default::default(),
                id: Default::default(),
                message: Default::default(),
            }
        }
    }

    ///Image information used when creating the deploy. Not present for
    /// Git-backed deploys
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "description": "Image information used when creating the deploy. Not
    /// present for Git-backed deploys",
    ///  "type": "object",
    ///  "properties": {
    ///    "ref": {
    ///      "description": "Image reference used when creating the deploy",
    ///      "type": "string"
    ///    },
    ///    "registryCredential": {
    ///      "description": "Name of credential used to pull the image, if
    /// provided",
    ///      "type": "string"
    ///    },
    ///    "sha": {
    ///      "description": "SHA that the image reference was resolved to when
    /// creating the deploy",
    ///      "type": "string"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct DeployImage {
        ///Image reference used when creating the deploy
        #[serde(
            rename = "ref",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub ref_: ::std::option::Option<::std::string::String>,
        ///Name of credential used to pull the image, if provided
        #[serde(
            rename = "registryCredential",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub registry_credential: ::std::option::Option<::std::string::String>,
        ///SHA that the image reference was resolved to when creating the
        /// deploy
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub sha: ::std::option::Option<::std::string::String>,
    }

    impl ::std::default::Default for DeployImage {
        fn default() -> Self {
            Self {
                ref_: Default::default(),
                registry_credential: Default::default(),
                sha: Default::default(),
            }
        }
    }

    ///`DeployList`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "array",
    ///  "items": {
    ///    "$ref": "#/components/schemas/deployWithCursor"
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(transparent)]
    pub struct DeployList(pub ::std::vec::Vec<DeployWithCursor>);
    impl ::std::ops::Deref for DeployList {
        type Target = ::std::vec::Vec<DeployWithCursor>;
        fn deref(&self) -> &::std::vec::Vec<DeployWithCursor> {
            &self.0
        }
    }

    impl ::std::convert::From<DeployList> for ::std::vec::Vec<DeployWithCursor> {
        fn from(value: DeployList) -> Self {
            value.0
        }
    }

    impl ::std::convert::From<::std::vec::Vec<DeployWithCursor>> for DeployList {
        fn from(value: ::std::vec::Vec<DeployWithCursor>) -> Self {
            Self(value)
        }
    }

    ///Controls deployment behavior when triggering a deploy.
    ///
    /// - `deploy_only`: Deploy the last successful build without rebuilding
    ///   (minimizes downtime)
    /// - `build_and_deploy`: Build new code and deploy it (default behavior
    ///   when not specified)
    ///
    ///**Note:** `deploy_only` cannot be combined with `commitId`, `imageUrl`
    /// or `clearCache` parameters, as those are build related fields.
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "description": "Controls deployment behavior when triggering a
    /// deploy.\n\n- `deploy_only`: Deploy the last successful build without
    /// rebuilding (minimizes downtime)\n- `build_and_deploy`: Build new code
    /// and deploy it (default behavior when not specified)\n\n**Note:**
    /// `deploy_only` cannot be combined with `commitId`, `imageUrl` or
    /// `clearCache` parameters,\nas those are build related fields.\n",
    ///  "default": "build_and_deploy",
    ///  "type": "string"
    ///}
    /// ```
    /// </details>
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    #[serde(transparent)]
    pub struct DeployMode(pub ::std::string::String);
    impl ::std::ops::Deref for DeployMode {
        type Target = ::std::string::String;
        fn deref(&self) -> &::std::string::String {
            &self.0
        }
    }

    impl ::std::convert::From<DeployMode> for ::std::string::String {
        fn from(value: DeployMode) -> Self {
            value.0
        }
    }

    impl ::std::convert::From<::std::string::String> for DeployMode {
        fn from(value: ::std::string::String) -> Self {
            Self(value)
        }
    }

    impl ::std::str::FromStr for DeployMode {
        type Err = ::std::convert::Infallible;
        fn from_str(value: &str) -> ::std::result::Result<Self, Self::Err> {
            Ok(Self(value.to_string()))
        }
    }

    impl ::std::fmt::Display for DeployMode {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            self.0.fmt(f)
        }
    }

    ///`DeployStatus`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "string"
    ///}
    /// ```
    /// </details>
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    #[serde(transparent)]
    pub struct DeployStatus(pub ::std::string::String);
    impl ::std::ops::Deref for DeployStatus {
        type Target = ::std::string::String;
        fn deref(&self) -> &::std::string::String {
            &self.0
        }
    }

    impl ::std::convert::From<DeployStatus> for ::std::string::String {
        fn from(value: DeployStatus) -> Self {
            value.0
        }
    }

    impl ::std::convert::From<::std::string::String> for DeployStatus {
        fn from(value: ::std::string::String) -> Self {
            Self(value)
        }
    }

    impl ::std::str::FromStr for DeployStatus {
        type Err = ::std::convert::Infallible;
        fn from_str(value: &str) -> ::std::result::Result<Self, Self::Err> {
            Ok(Self(value.to_string()))
        }
    }

    impl ::std::fmt::Display for DeployStatus {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            self.0.fmt(f)
        }
    }

    ///`DeployWithCursor`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "cursor": {
    ///      "$ref": "#/components/schemas/cursor"
    ///    },
    ///    "deploy": {
    ///      "$ref": "#/components/schemas/deploy"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct DeployWithCursor {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub cursor: ::std::option::Option<Cursor>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub deploy: ::std::option::Option<Deploy>,
    }

    impl ::std::default::Default for DeployWithCursor {
        fn default() -> Self {
            Self {
                cursor: Default::default(),
                deploy: Default::default(),
            }
        }
    }

    ///`DockerDetails`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "dockerCommand": {
    ///      "type": "string"
    ///    },
    ///    "dockerContext": {
    ///      "type": "string"
    ///    },
    ///    "dockerfilePath": {
    ///      "type": "string"
    ///    },
    ///    "preDeployCommand": {
    ///      "type": "string"
    ///    },
    ///    "registryCredential": {
    ///      "$ref": "#/components/schemas/registryCredential"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct DockerDetails {
        #[serde(
            rename = "dockerCommand",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub docker_command: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "dockerContext",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub docker_context: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "dockerfilePath",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub dockerfile_path: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "preDeployCommand",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub pre_deploy_command: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "registryCredential",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub registry_credential: ::std::option::Option<RegistryCredential>,
    }

    impl ::std::default::Default for DockerDetails {
        fn default() -> Self {
            Self {
                docker_command: Default::default(),
                docker_context: Default::default(),
                dockerfile_path: Default::default(),
                pre_deploy_command: Default::default(),
                registry_credential: Default::default(),
            }
        }
    }

    ///`DockerDetailsPost`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "dockerCommand": {
    ///      "type": "string"
    ///    },
    ///    "dockerContext": {
    ///      "type": "string"
    ///    },
    ///    "dockerfilePath": {
    ///      "description": "Defaults to \"./Dockerfile\"",
    ///      "type": "string"
    ///    },
    ///    "registryCredentialId": {
    ///      "type": "string"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct DockerDetailsPost {
        #[serde(
            rename = "dockerCommand",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub docker_command: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "dockerContext",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub docker_context: ::std::option::Option<::std::string::String>,
        ///Defaults to "./Dockerfile"
        #[serde(
            rename = "dockerfilePath",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub dockerfile_path: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "registryCredentialId",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub registry_credential_id: ::std::option::Option<::std::string::String>,
    }

    impl ::std::default::Default for DockerDetailsPost {
        fn default() -> Self {
            Self {
                docker_command: Default::default(),
                docker_context: Default::default(),
                dockerfile_path: Default::default(),
                registry_credential_id: Default::default(),
            }
        }
    }

    ///`EnvSpecificDetails`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "oneOf": [
    ///    {
    ///      "$ref": "#/components/schemas/dockerDetails"
    ///    },
    ///    {
    ///      "$ref": "#/components/schemas/nativeEnvironmentDetails"
    ///    }
    ///  ]
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum EnvSpecificDetails {
        DockerDetails(DockerDetails),
        NativeEnvironmentDetails(NativeEnvironmentDetails),
    }

    impl ::std::convert::From<DockerDetails> for EnvSpecificDetails {
        fn from(value: DockerDetails) -> Self {
            Self::DockerDetails(value)
        }
    }

    impl ::std::convert::From<NativeEnvironmentDetails> for EnvSpecificDetails {
        fn from(value: NativeEnvironmentDetails) -> Self {
            Self::NativeEnvironmentDetails(value)
        }
    }

    ///`EnvSpecificDetailsPost`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "oneOf": [
    ///    {
    ///      "$ref": "#/components/schemas/dockerDetailsPOST"
    ///    },
    ///    {
    ///      "$ref": "#/components/schemas/nativeEnvironmentDetailsPOST"
    ///    }
    ///  ]
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum EnvSpecificDetailsPost {
        DockerDetailsPost(DockerDetailsPost),
        NativeEnvironmentDetailsPost(NativeEnvironmentDetailsPost),
    }

    impl ::std::convert::From<DockerDetailsPost> for EnvSpecificDetailsPost {
        fn from(value: DockerDetailsPost) -> Self {
            Self::DockerDetailsPost(value)
        }
    }

    impl ::std::convert::From<NativeEnvironmentDetailsPost> for EnvSpecificDetailsPost {
        fn from(value: NativeEnvironmentDetailsPost) -> Self {
            Self::NativeEnvironmentDetailsPost(value)
        }
    }

    ///`EnvVar`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "key": {
    ///      "type": "string"
    ///    },
    ///    "value": {
    ///      "type": "string"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct EnvVar {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub key: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub value: ::std::option::Option<::std::string::String>,
    }

    impl ::std::default::Default for EnvVar {
        fn default() -> Self {
            Self {
                key: Default::default(),
                value: Default::default(),
            }
        }
    }

    ///`EnvVarWithCursor`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "cursor": {
    ///      "$ref": "#/components/schemas/cursor"
    ///    },
    ///    "envVar": {
    ///      "$ref": "#/components/schemas/envVar"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct EnvVarWithCursor {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub cursor: ::std::option::Option<Cursor>,
        #[serde(
            rename = "envVar",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub env_var: ::std::option::Option<EnvVar>,
    }

    impl ::std::default::Default for EnvVarWithCursor {
        fn default() -> Self {
            Self {
                cursor: Default::default(),
                env_var: Default::default(),
            }
        }
    }

    ///`Error`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "id": {
    ///      "type": "string"
    ///    },
    ///    "message": {
    ///      "type": "string"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct Error {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub id: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub message: ::std::option::Option<::std::string::String>,
    }

    impl ::std::default::Default for Error {
        fn default() -> Self {
            Self {
                id: Default::default(),
                message: Default::default(),
            }
        }
    }

    ///`HeaderInput`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "name": {
    ///      "description": "Header name",
    ///      "examples": [
    ///        "Cache-Control"
    ///      ],
    ///      "type": "string"
    ///    },
    ///    "path": {
    ///      "description": "The request path to add the header to. Wildcards
    /// will cause headers to be applied to all matching paths.",
    ///      "examples": [
    ///        "/static/*"
    ///      ],
    ///      "type": "string"
    ///    },
    ///    "value": {
    ///      "description": "Header value",
    ///      "examples": [
    ///        "public, max-age=604800"
    ///      ],
    ///      "type": "string"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct HeaderInput {
        ///Header name
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub name: ::std::option::Option<::std::string::String>,
        ///The request path to add the header to. Wildcards will cause headers
        /// to be applied to all matching paths.
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub path: ::std::option::Option<::std::string::String>,
        ///Header value
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub value: ::std::option::Option<::std::string::String>,
    }

    impl ::std::default::Default for HeaderInput {
        fn default() -> Self {
            Self {
                name: Default::default(),
                path: Default::default(),
                value: Default::default(),
            }
        }
    }

    ///`Image`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "imagePath": {
    ///      "description": "Path to the image used for this server (e.g
    /// docker.io/library/nginx:latest).",
    ///      "type": "string"
    ///    },
    ///    "ownerId": {
    ///      "description": "The ID of the owner for this image. This should
    /// match the owner of the service as well as the owner of any specified
    /// registry credential.",
    ///      "type": "string"
    ///    },
    ///    "registryCredentialId": {
    ///      "description": "Optional reference to the registry credential
    /// passed to the image repository to retrieve this image.",
    ///      "type": "string"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct Image {
        ///Path to the image used for this server (e.g
        /// docker.io/library/nginx:latest).
        #[serde(
            rename = "imagePath",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub image_path: ::std::option::Option<::std::string::String>,
        ///The ID of the owner for this image. This should match the owner of
        /// the service as well as the owner of any specified registry
        /// credential.
        #[serde(
            rename = "ownerId",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub owner_id: ::std::option::Option<::std::string::String>,
        ///Optional reference to the registry credential passed to the image
        /// repository to retrieve this image.
        #[serde(
            rename = "registryCredentialId",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub registry_credential_id: ::std::option::Option<::std::string::String>,
    }

    impl ::std::default::Default for Image {
        fn default() -> Self {
            Self {
                image_path: Default::default(),
                owner_id: Default::default(),
                registry_credential_id: Default::default(),
            }
        }
    }

    ///A run of a cron job
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "description": "A run of a cron job",
    ///  "type": "object",
    ///  "properties": {
    ///    "hasMore": {
    ///      "description": "True if there are more logs to fetch",
    ///      "type": "boolean"
    ///    },
    ///    "logs": {
    ///      "type": "array",
    ///      "items": {
    ///        "description": "A log entry with metadata",
    ///        "type": "object",
    ///        "properties": {
    ///          "id": {
    ///            "description": "A unique ID of the log entry",
    ///            "type": "string"
    ///          },
    ///          "labels": {
    ///            "type": "array",
    ///            "items": {
    ///              "description": "A log label",
    ///              "type": "object",
    ///              "properties": {
    ///                "name": {
    ///                  "description": "The name of the log label",
    ///                  "type": "string"
    ///                },
    ///                "value": {
    ///                  "description": "The value of the log label",
    ///                  "type": "string"
    ///                }
    ///              }
    ///            }
    ///          },
    ///          "message": {
    ///            "description": "The message of the log entry",
    ///            "type": "string"
    ///          },
    ///          "timestamp": {
    ///            "description": "The timestamp of the log entry",
    ///            "type": "string",
    ///            "format": "date-time"
    ///          }
    ///        }
    ///      }
    ///    },
    ///    "nextEndTime": {
    ///      "description": "The end time to use in the next query to fetch the
    /// next set of logs",
    ///      "examples": [
    ///        "2021-07-15T07:20:05.777035-07:00"
    ///      ],
    ///      "type": "string",
    ///      "format": "date-time"
    ///    },
    ///    "nextStartTime": {
    ///      "description": "The start time to use in the next query to fetch
    /// the next set of logs",
    ///      "examples": [
    ///        "2021-07-15T07:20:05.777035-07:00"
    ///      ],
    ///      "type": "string",
    ///      "format": "date-time"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct ListLogsResponse {
        ///True if there are more logs to fetch
        #[serde(
            rename = "hasMore",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub has_more: ::std::option::Option<bool>,
        #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
        pub logs: ::std::vec::Vec<ListLogsResponseLogsItem>,
        ///The end time to use in the next query to fetch the next set of logs
        #[serde(
            rename = "nextEndTime",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub next_end_time: ::std::option::Option<::chrono::DateTime<::chrono::offset::Utc>>,
        ///The start time to use in the next query to fetch the next set of
        /// logs
        #[serde(
            rename = "nextStartTime",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub next_start_time: ::std::option::Option<::chrono::DateTime<::chrono::offset::Utc>>,
    }

    impl ::std::default::Default for ListLogsResponse {
        fn default() -> Self {
            Self {
                has_more: Default::default(),
                logs: Default::default(),
                next_end_time: Default::default(),
                next_start_time: Default::default(),
            }
        }
    }

    ///A log entry with metadata
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "description": "A log entry with metadata",
    ///  "type": "object",
    ///  "properties": {
    ///    "id": {
    ///      "description": "A unique ID of the log entry",
    ///      "type": "string"
    ///    },
    ///    "labels": {
    ///      "type": "array",
    ///      "items": {
    ///        "description": "A log label",
    ///        "type": "object",
    ///        "properties": {
    ///          "name": {
    ///            "description": "The name of the log label",
    ///            "type": "string"
    ///          },
    ///          "value": {
    ///            "description": "The value of the log label",
    ///            "type": "string"
    ///          }
    ///        }
    ///      }
    ///    },
    ///    "message": {
    ///      "description": "The message of the log entry",
    ///      "type": "string"
    ///    },
    ///    "timestamp": {
    ///      "description": "The timestamp of the log entry",
    ///      "type": "string",
    ///      "format": "date-time"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct ListLogsResponseLogsItem {
        ///A unique ID of the log entry
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub id: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
        pub labels: ::std::vec::Vec<ListLogsResponseLogsItemLabelsItem>,
        ///The message of the log entry
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub message: ::std::option::Option<::std::string::String>,
        ///The timestamp of the log entry
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub timestamp: ::std::option::Option<::chrono::DateTime<::chrono::offset::Utc>>,
    }

    impl ::std::default::Default for ListLogsResponseLogsItem {
        fn default() -> Self {
            Self {
                id: Default::default(),
                labels: Default::default(),
                message: Default::default(),
                timestamp: Default::default(),
            }
        }
    }

    ///A log label
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "description": "A log label",
    ///  "type": "object",
    ///  "properties": {
    ///    "name": {
    ///      "description": "The name of the log label",
    ///      "type": "string"
    ///    },
    ///    "value": {
    ///      "description": "The value of the log label",
    ///      "type": "string"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct ListLogsResponseLogsItemLabelsItem {
        ///The name of the log label
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub name: ::std::option::Option<::std::string::String>,
        ///The value of the log label
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub value: ::std::option::Option<::std::string::String>,
    }

    impl ::std::default::Default for ListLogsResponseLogsItemLabelsItem {
        fn default() -> Self {
            Self {
                name: Default::default(),
                value: Default::default(),
            }
        }
    }

    ///`MaintenanceMode`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "enabled": {
    ///      "type": "boolean"
    ///    },
    ///    "uri": {
    ///      "description": "The page to be served when [maintenance mode](https://render.com/docs/maintenance-mode) is enabled. When empty, the default maintenance mode page is served.",
    ///      "type": "string"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct MaintenanceMode {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub enabled: ::std::option::Option<bool>,
        ///The page to be served when [maintenance mode](https://render.com/docs/maintenance-mode) is enabled. When empty, the default maintenance mode page is served.
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub uri: ::std::option::Option<::std::string::String>,
    }

    impl ::std::default::Default for MaintenanceMode {
        fn default() -> Self {
            Self {
                enabled: Default::default(),
                uri: Default::default(),
            }
        }
    }

    ///The maximum amount of time (in seconds) that Render waits for your
    /// application process to exit gracefully after sending it a SIGTERM
    /// signal.
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "description": "The maximum amount of time (in seconds) that Render
    /// waits for your application process to exit gracefully after sending it a
    /// SIGTERM signal.",
    ///  "default": 30,
    ///  "type": "integer",
    ///  "maximum": 300.0,
    ///  "minimum": 1.0
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(transparent)]
    pub struct MaxShutdownDelaySeconds(pub ::std::num::NonZeroU64);
    impl ::std::ops::Deref for MaxShutdownDelaySeconds {
        type Target = ::std::num::NonZeroU64;
        fn deref(&self) -> &::std::num::NonZeroU64 {
            &self.0
        }
    }

    impl ::std::convert::From<MaxShutdownDelaySeconds> for ::std::num::NonZeroU64 {
        fn from(value: MaxShutdownDelaySeconds) -> Self {
            value.0
        }
    }

    impl ::std::convert::From<::std::num::NonZeroU64> for MaxShutdownDelaySeconds {
        fn from(value: ::std::num::NonZeroU64) -> Self {
            Self(value)
        }
    }

    impl ::std::str::FromStr for MaxShutdownDelaySeconds {
        type Err = <::std::num::NonZeroU64 as ::std::str::FromStr>::Err;
        fn from_str(value: &str) -> ::std::result::Result<Self, Self::Err> {
            Ok(Self(value.parse()?))
        }
    }

    impl ::std::convert::TryFrom<&str> for MaxShutdownDelaySeconds {
        type Error = <::std::num::NonZeroU64 as ::std::str::FromStr>::Err;
        fn try_from(value: &str) -> ::std::result::Result<Self, Self::Error> {
            value.parse()
        }
    }

    impl ::std::convert::TryFrom<String> for MaxShutdownDelaySeconds {
        type Error = <::std::num::NonZeroU64 as ::std::str::FromStr>::Err;
        fn try_from(value: String) -> ::std::result::Result<Self, Self::Error> {
            value.parse()
        }
    }

    impl ::std::fmt::Display for MaxShutdownDelaySeconds {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            self.0.fmt(f)
        }
    }

    ///`NativeEnvironmentDetails`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "buildCommand": {
    ///      "type": "string"
    ///    },
    ///    "preDeployCommand": {
    ///      "type": "string"
    ///    },
    ///    "startCommand": {
    ///      "type": "string"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct NativeEnvironmentDetails {
        #[serde(
            rename = "buildCommand",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub build_command: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "preDeployCommand",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub pre_deploy_command: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "startCommand",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub start_command: ::std::option::Option<::std::string::String>,
    }

    impl ::std::default::Default for NativeEnvironmentDetails {
        fn default() -> Self {
            Self {
                build_command: Default::default(),
                pre_deploy_command: Default::default(),
                start_command: Default::default(),
            }
        }
    }

    ///Fields for native environment (runtime) services
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "description": "Fields for native environment (runtime) services",
    ///  "type": "object",
    ///  "properties": {
    ///    "buildCommand": {
    ///      "type": "string"
    ///    },
    ///    "startCommand": {
    ///      "type": "string"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct NativeEnvironmentDetailsPost {
        #[serde(
            rename = "buildCommand",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub build_command: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "startCommand",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub start_command: ::std::option::Option<::std::string::String>,
    }

    impl ::std::default::Default for NativeEnvironmentDetailsPost {
        fn default() -> Self {
            Self {
                build_command: Default::default(),
                start_command: Default::default(),
            }
        }
    }

    ///`NotifySetting`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "string"
    ///}
    /// ```
    /// </details>
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    #[serde(transparent)]
    pub struct NotifySetting(pub ::std::string::String);
    impl ::std::ops::Deref for NotifySetting {
        type Target = ::std::string::String;
        fn deref(&self) -> &::std::string::String {
            &self.0
        }
    }

    impl ::std::convert::From<NotifySetting> for ::std::string::String {
        fn from(value: NotifySetting) -> Self {
            value.0
        }
    }

    impl ::std::convert::From<::std::string::String> for NotifySetting {
        fn from(value: ::std::string::String) -> Self {
            Self(value)
        }
    }

    impl ::std::str::FromStr for NotifySetting {
        type Err = ::std::convert::Infallible;
        fn from_str(value: &str) -> ::std::result::Result<Self, Self::Err> {
            Ok(Self(value.to_string()))
        }
    }

    impl ::std::fmt::Display for NotifySetting {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            self.0.fmt(f)
        }
    }

    ///`Owner`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "email": {
    ///      "type": "string"
    ///    },
    ///    "id": {
    ///      "type": "string"
    ///    },
    ///    "ipAllowList": {
    ///      "type": "array",
    ///      "items": {
    ///        "$ref": "#/components/schemas/cidrBlockAndDescription"
    ///      }
    ///    },
    ///    "name": {
    ///      "type": "string"
    ///    },
    ///    "twoFactorAuthEnabled": {
    ///      "description": "Whether two-factor authentication is enabled for
    /// the owner. Only present if `type` is `user`.",
    ///      "type": "boolean"
    ///    },
    ///    "type": {
    ///      "type": "string"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct Owner {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub email: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub id: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "ipAllowList",
            default,
            skip_serializing_if = "::std::vec::Vec::is_empty"
        )]
        pub ip_allow_list: ::std::vec::Vec<CidrBlockAndDescription>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub name: ::std::option::Option<::std::string::String>,
        ///Whether two-factor authentication is enabled for the owner. Only
        /// present if `type` is `user`.
        #[serde(
            rename = "twoFactorAuthEnabled",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub two_factor_auth_enabled: ::std::option::Option<bool>,
        #[serde(
            rename = "type",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub type_: ::std::option::Option<::std::string::String>,
    }

    impl ::std::default::Default for Owner {
        fn default() -> Self {
            Self {
                email: Default::default(),
                id: Default::default(),
                ip_allow_list: Default::default(),
                name: Default::default(),
                two_factor_auth_enabled: Default::default(),
                type_: Default::default(),
            }
        }
    }

    ///Defaults to `starter` when creating a new database.
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "description": "Defaults to `starter` when creating a new database.",
    ///  "default": "starter",
    ///  "type": "string"
    ///}
    /// ```
    /// </details>
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    #[serde(transparent)]
    pub struct PaidPlan(pub ::std::string::String);
    impl ::std::ops::Deref for PaidPlan {
        type Target = ::std::string::String;
        fn deref(&self) -> &::std::string::String {
            &self.0
        }
    }

    impl ::std::convert::From<PaidPlan> for ::std::string::String {
        fn from(value: PaidPlan) -> Self {
            value.0
        }
    }

    impl ::std::convert::From<::std::string::String> for PaidPlan {
        fn from(value: ::std::string::String) -> Self {
            Self(value)
        }
    }

    impl ::std::str::FromStr for PaidPlan {
        type Err = ::std::convert::Infallible;
        fn from_str(value: &str) -> ::std::result::Result<Self, Self::Err> {
            Ok(Self(value.to_string()))
        }
    }

    impl ::std::fmt::Display for PaidPlan {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            self.0.fmt(f)
        }
    }

    ///`PatchRouteResponse`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "headers": {
    ///      "$ref": "#/components/schemas/route"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PatchRouteResponse {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub headers: ::std::option::Option<Route>,
    }

    impl ::std::default::Default for PatchRouteResponse {
        fn default() -> Self {
            Self {
                headers: Default::default(),
            }
        }
    }

    ///The instance type to use. Legacy variants (`*_legacy`) identify
    /// grandfathered plans no longer offered for new services. Note that base
    /// services on any paid instance type can't create preview instances with
    /// the `free` instance type.
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "description": "The instance type to use. Legacy variants (`*_legacy`)
    /// identify grandfathered plans no longer offered for new services. Note
    /// that base services on any paid instance type can't create preview
    /// instances with the `free` instance type.",
    ///  "examples": [
    ///    "starter"
    ///  ],
    ///  "type": "string"
    ///}
    /// ```
    /// </details>
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    #[serde(transparent)]
    pub struct Plan(pub ::std::string::String);
    impl ::std::ops::Deref for Plan {
        type Target = ::std::string::String;
        fn deref(&self) -> &::std::string::String {
            &self.0
        }
    }

    impl ::std::convert::From<Plan> for ::std::string::String {
        fn from(value: Plan) -> Self {
            value.0
        }
    }

    impl ::std::convert::From<::std::string::String> for Plan {
        fn from(value: ::std::string::String) -> Self {
            Self(value)
        }
    }

    impl ::std::str::FromStr for Plan {
        type Err = ::std::convert::Infallible;
        fn from_str(value: &str) -> ::std::result::Result<Self, Self::Err> {
            Ok(Self(value.to_string()))
        }
    }

    impl ::std::fmt::Display for Plan {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            self.0.fmt(f)
        }
    }

    ///`Postgres`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "createdAt": {
    ///      "type": "string",
    ///      "format": "date-time"
    ///    },
    ///    "dashboardUrl": {
    ///      "description": "The URL to view the Postgres instance in the Render
    /// Dashboard",
    ///      "type": "string"
    ///    },
    ///    "databaseName": {
    ///      "type": "string"
    ///    },
    ///    "databaseUser": {
    ///      "type": "string"
    ///    },
    ///    "diskAutoscalingEnabled": {
    ///      "type": "boolean"
    ///    },
    ///    "diskSizeGB": {
    ///      "type": "integer"
    ///    },
    ///    "environmentId": {
    ///      "type": "string"
    ///    },
    ///    "expiresAt": {
    ///      "description": "The time at which the database will be expire.
    /// Applies to free tier databases only.",
    ///      "type": "string",
    ///      "format": "date-time"
    ///    },
    ///    "highAvailabilityEnabled": {
    ///      "type": "boolean"
    ///    },
    ///    "id": {
    ///      "type": "string"
    ///    },
    ///    "ipAllowList": {
    ///      "type": "array",
    ///      "items": {
    ///        "$ref": "#/components/schemas/cidrBlockAndDescription"
    ///      }
    ///    },
    ///    "name": {
    ///      "type": "string"
    ///    },
    ///    "owner": {
    ///      "$ref": "#/components/schemas/owner"
    ///    },
    ///    "plan": {
    ///      "type": "string"
    ///    },
    ///    "primaryPostgresID": {
    ///      "type": "string"
    ///    },
    ///    "readReplicas": {
    ///      "$ref": "#/components/schemas/readReplicas"
    ///    },
    ///    "region": {
    ///      "$ref": "#/components/schemas/region"
    ///    },
    ///    "role": {
    ///      "$ref": "#/components/schemas/databaseRole"
    ///    },
    ///    "status": {
    ///      "$ref": "#/components/schemas/databaseStatus"
    ///    },
    ///    "suspended": {
    ///      "type": "string"
    ///    },
    ///    "suspenders": {
    ///      "type": "array",
    ///      "items": {
    ///        "$ref": "#/components/schemas/suspenderType"
    ///      }
    ///    },
    ///    "updatedAt": {
    ///      "type": "string",
    ///      "format": "date-time"
    ///    },
    ///    "version": {
    ///      "$ref": "#/components/schemas/postgresVersion"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct Postgres {
        #[serde(
            rename = "createdAt",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub created_at: ::std::option::Option<::chrono::DateTime<::chrono::offset::Utc>>,
        ///The URL to view the Postgres instance in the Render Dashboard
        #[serde(
            rename = "dashboardUrl",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub dashboard_url: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "databaseName",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub database_name: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "databaseUser",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub database_user: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "diskAutoscalingEnabled",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub disk_autoscaling_enabled: ::std::option::Option<bool>,
        #[serde(
            rename = "diskSizeGB",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub disk_size_gb: ::std::option::Option<i64>,
        #[serde(
            rename = "environmentId",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub environment_id: ::std::option::Option<::std::string::String>,
        ///The time at which the database will be expire. Applies to free tier
        /// databases only.
        #[serde(
            rename = "expiresAt",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub expires_at: ::std::option::Option<::chrono::DateTime<::chrono::offset::Utc>>,
        #[serde(
            rename = "highAvailabilityEnabled",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub high_availability_enabled: ::std::option::Option<bool>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub id: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "ipAllowList",
            default,
            skip_serializing_if = "::std::vec::Vec::is_empty"
        )]
        pub ip_allow_list: ::std::vec::Vec<CidrBlockAndDescription>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub name: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub owner: ::std::option::Option<Owner>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub plan: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "primaryPostgresID",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub primary_postgres_id: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "readReplicas",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub read_replicas: ::std::option::Option<ReadReplicas>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub region: ::std::option::Option<Region>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub role: ::std::option::Option<DatabaseRole>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub status: ::std::option::Option<DatabaseStatus>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub suspended: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
        pub suspenders: ::std::vec::Vec<SuspenderType>,
        #[serde(
            rename = "updatedAt",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub updated_at: ::std::option::Option<::chrono::DateTime<::chrono::offset::Utc>>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub version: ::std::option::Option<PostgresVersion>,
    }

    impl ::std::default::Default for Postgres {
        fn default() -> Self {
            Self {
                created_at: Default::default(),
                dashboard_url: Default::default(),
                database_name: Default::default(),
                database_user: Default::default(),
                disk_autoscaling_enabled: Default::default(),
                disk_size_gb: Default::default(),
                environment_id: Default::default(),
                expires_at: Default::default(),
                high_availability_enabled: Default::default(),
                id: Default::default(),
                ip_allow_list: Default::default(),
                name: Default::default(),
                owner: Default::default(),
                plan: Default::default(),
                primary_postgres_id: Default::default(),
                read_replicas: Default::default(),
                region: Default::default(),
                role: Default::default(),
                status: Default::default(),
                suspended: Default::default(),
                suspenders: Default::default(),
                updated_at: Default::default(),
                version: Default::default(),
            }
        }
    }

    ///`PostgresConnectionInfo`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "externalConnectionString": {
    ///      "type": "string",
    ///      "format": "password"
    ///    },
    ///    "internalConnectionString": {
    ///      "type": "string",
    ///      "format": "password"
    ///    },
    ///    "password": {
    ///      "type": "string",
    ///      "format": "password"
    ///    },
    ///    "psqlCommand": {
    ///      "type": "string",
    ///      "format": "password"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PostgresConnectionInfo {
        #[serde(
            rename = "externalConnectionString",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub external_connection_string: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "internalConnectionString",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub internal_connection_string: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub password: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "psqlCommand",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub psql_command: ::std::option::Option<::std::string::String>,
    }

    impl ::std::default::Default for PostgresConnectionInfo {
        fn default() -> Self {
            Self {
                external_connection_string: Default::default(),
                internal_connection_string: Default::default(),
                password: Default::default(),
                psql_command: Default::default(),
            }
        }
    }

    ///`PostgresDetail`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "createdAt": {
    ///      "type": "string",
    ///      "format": "date-time"
    ///    },
    ///    "dashboardUrl": {
    ///      "description": "The URL to view the Postgres instance in the Render
    /// Dashboard",
    ///      "type": "string"
    ///    },
    ///    "databaseName": {
    ///      "type": "string"
    ///    },
    ///    "databaseUser": {
    ///      "type": "string"
    ///    },
    ///    "diskAutoscalingEnabled": {
    ///      "type": "boolean"
    ///    },
    ///    "diskSizeGB": {
    ///      "type": "integer"
    ///    },
    ///    "environmentId": {
    ///      "type": "string"
    ///    },
    ///    "expiresAt": {
    ///      "description": "The time at which the database will be expire.
    /// Applies to free tier databases only.",
    ///      "type": "string",
    ///      "format": "date-time"
    ///    },
    ///    "highAvailabilityEnabled": {
    ///      "type": "boolean"
    ///    },
    ///    "id": {
    ///      "type": "string"
    ///    },
    ///    "ipAllowList": {
    ///      "type": "array",
    ///      "items": {
    ///        "$ref": "#/components/schemas/cidrBlockAndDescription"
    ///      }
    ///    },
    ///    "maintenance": {
    ///      "type": "object",
    ///      "properties": {
    ///        "id": {
    ///          "examples": [
    ///            "mrn-cph1rs3idesc73a2b2mg"
    ///          ],
    ///          "type": "string",
    ///          "pattern": "^mrn-[0-9a-z]{20}$"
    ///        },
    ///        "pendingMaintenanceBy": {
    ///          "description": "If present, the maintenance run cannot be
    /// scheduled for later than this date-time.",
    ///          "type": "string",
    ///          "format": "date-time"
    ///        },
    ///        "scheduledAt": {
    ///          "type": "string",
    ///          "format": "date-time"
    ///        },
    ///        "state": {
    ///          "type": "string"
    ///        },
    ///        "type": {
    ///          "type": "string"
    ///        }
    ///      }
    ///    },
    ///    "name": {
    ///      "type": "string"
    ///    },
    ///    "owner": {
    ///      "$ref": "#/components/schemas/owner"
    ///    },
    ///    "parameterOverrides": {
    ///      "$ref": "#/components/schemas/postgresParameterOverrides"
    ///    },
    ///    "plan": {
    ///      "type": "string"
    ///    },
    ///    "primaryPostgresID": {
    ///      "type": "string"
    ///    },
    ///    "readReplicas": {
    ///      "$ref": "#/components/schemas/readReplicas"
    ///    },
    ///    "region": {
    ///      "$ref": "#/components/schemas/region"
    ///    },
    ///    "role": {
    ///      "$ref": "#/components/schemas/databaseRole"
    ///    },
    ///    "status": {
    ///      "$ref": "#/components/schemas/databaseStatus"
    ///    },
    ///    "suspended": {
    ///      "type": "string"
    ///    },
    ///    "suspenders": {
    ///      "type": "array",
    ///      "items": {
    ///        "$ref": "#/components/schemas/suspenderType"
    ///      }
    ///    },
    ///    "updatedAt": {
    ///      "type": "string",
    ///      "format": "date-time"
    ///    },
    ///    "version": {
    ///      "$ref": "#/components/schemas/postgresVersion"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PostgresDetail {
        #[serde(
            rename = "createdAt",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub created_at: ::std::option::Option<::chrono::DateTime<::chrono::offset::Utc>>,
        ///The URL to view the Postgres instance in the Render Dashboard
        #[serde(
            rename = "dashboardUrl",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub dashboard_url: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "databaseName",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub database_name: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "databaseUser",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub database_user: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "diskAutoscalingEnabled",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub disk_autoscaling_enabled: ::std::option::Option<bool>,
        #[serde(
            rename = "diskSizeGB",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub disk_size_gb: ::std::option::Option<i64>,
        #[serde(
            rename = "environmentId",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub environment_id: ::std::option::Option<::std::string::String>,
        ///The time at which the database will be expire. Applies to free tier
        /// databases only.
        #[serde(
            rename = "expiresAt",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub expires_at: ::std::option::Option<::chrono::DateTime<::chrono::offset::Utc>>,
        #[serde(
            rename = "highAvailabilityEnabled",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub high_availability_enabled: ::std::option::Option<bool>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub id: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "ipAllowList",
            default,
            skip_serializing_if = "::std::vec::Vec::is_empty"
        )]
        pub ip_allow_list: ::std::vec::Vec<CidrBlockAndDescription>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub maintenance: ::std::option::Option<PostgresDetailMaintenance>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub name: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub owner: ::std::option::Option<Owner>,
        #[serde(
            rename = "parameterOverrides",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub parameter_overrides: ::std::option::Option<PostgresParameterOverrides>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub plan: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "primaryPostgresID",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub primary_postgres_id: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "readReplicas",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub read_replicas: ::std::option::Option<ReadReplicas>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub region: ::std::option::Option<Region>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub role: ::std::option::Option<DatabaseRole>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub status: ::std::option::Option<DatabaseStatus>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub suspended: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
        pub suspenders: ::std::vec::Vec<SuspenderType>,
        #[serde(
            rename = "updatedAt",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub updated_at: ::std::option::Option<::chrono::DateTime<::chrono::offset::Utc>>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub version: ::std::option::Option<PostgresVersion>,
    }

    impl ::std::default::Default for PostgresDetail {
        fn default() -> Self {
            Self {
                created_at: Default::default(),
                dashboard_url: Default::default(),
                database_name: Default::default(),
                database_user: Default::default(),
                disk_autoscaling_enabled: Default::default(),
                disk_size_gb: Default::default(),
                environment_id: Default::default(),
                expires_at: Default::default(),
                high_availability_enabled: Default::default(),
                id: Default::default(),
                ip_allow_list: Default::default(),
                maintenance: Default::default(),
                name: Default::default(),
                owner: Default::default(),
                parameter_overrides: Default::default(),
                plan: Default::default(),
                primary_postgres_id: Default::default(),
                read_replicas: Default::default(),
                region: Default::default(),
                role: Default::default(),
                status: Default::default(),
                suspended: Default::default(),
                suspenders: Default::default(),
                updated_at: Default::default(),
                version: Default::default(),
            }
        }
    }

    ///`PostgresDetailMaintenance`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "id": {
    ///      "examples": [
    ///        "mrn-cph1rs3idesc73a2b2mg"
    ///      ],
    ///      "type": "string",
    ///      "pattern": "^mrn-[0-9a-z]{20}$"
    ///    },
    ///    "pendingMaintenanceBy": {
    ///      "description": "If present, the maintenance run cannot be scheduled
    /// for later than this date-time.",
    ///      "type": "string",
    ///      "format": "date-time"
    ///    },
    ///    "scheduledAt": {
    ///      "type": "string",
    ///      "format": "date-time"
    ///    },
    ///    "state": {
    ///      "type": "string"
    ///    },
    ///    "type": {
    ///      "type": "string"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PostgresDetailMaintenance {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub id: ::std::option::Option<PostgresDetailMaintenanceId>,
        ///If present, the maintenance run cannot be scheduled for later than
        /// this date-time.
        #[serde(
            rename = "pendingMaintenanceBy",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub pending_maintenance_by:
            ::std::option::Option<::chrono::DateTime<::chrono::offset::Utc>>,
        #[serde(
            rename = "scheduledAt",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub scheduled_at: ::std::option::Option<::chrono::DateTime<::chrono::offset::Utc>>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub state: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "type",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub type_: ::std::option::Option<::std::string::String>,
    }

    impl ::std::default::Default for PostgresDetailMaintenance {
        fn default() -> Self {
            Self {
                id: Default::default(),
                pending_maintenance_by: Default::default(),
                scheduled_at: Default::default(),
                state: Default::default(),
                type_: Default::default(),
            }
        }
    }

    ///`PostgresDetailMaintenanceId`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "examples": [
    ///    "mrn-cph1rs3idesc73a2b2mg"
    ///  ],
    ///  "type": "string",
    ///  "pattern": "^mrn-[0-9a-z]{20}$"
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
    #[serde(transparent)]
    pub struct PostgresDetailMaintenanceId(::std::string::String);
    impl ::std::ops::Deref for PostgresDetailMaintenanceId {
        type Target = ::std::string::String;
        fn deref(&self) -> &::std::string::String {
            &self.0
        }
    }

    impl ::std::convert::From<PostgresDetailMaintenanceId> for ::std::string::String {
        fn from(value: PostgresDetailMaintenanceId) -> Self {
            value.0
        }
    }

    impl ::std::str::FromStr for PostgresDetailMaintenanceId {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
                ::std::sync::LazyLock::new(|| ::regress::Regex::new("^mrn-[0-9a-z]{20}$").unwrap());
            if PATTERN.find(value).is_none() {
                return Err("doesn't match pattern \"^mrn-[0-9a-z]{20}$\"".into());
            }
            Ok(Self(value.to_string()))
        }
    }

    impl ::std::convert::TryFrom<&str> for PostgresDetailMaintenanceId {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }

    impl ::std::convert::TryFrom<&::std::string::String> for PostgresDetailMaintenanceId {
        type Error = self::error::ConversionError;
        fn try_from(
            value: &::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }

    impl ::std::convert::TryFrom<::std::string::String> for PostgresDetailMaintenanceId {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }

    impl<'de> ::serde::Deserialize<'de> for PostgresDetailMaintenanceId {
        fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
        where
            D: ::serde::Deserializer<'de>,
        {
            ::std::string::String::deserialize(deserializer)?
                .parse()
                .map_err(|e: self::error::ConversionError| {
                    <D::Error as ::serde::de::Error>::custom(e.to_string())
                })
        }
    }

    ///`PostgresParameterOverrides`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "additionalProperties": {
    ///    "type": "string"
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(transparent)]
    pub struct PostgresParameterOverrides(
        pub ::std::collections::HashMap<::std::string::String, ::std::string::String>,
    );
    impl ::std::ops::Deref for PostgresParameterOverrides {
        type Target = ::std::collections::HashMap<::std::string::String, ::std::string::String>;
        fn deref(
            &self,
        ) -> &::std::collections::HashMap<::std::string::String, ::std::string::String> {
            &self.0
        }
    }

    impl ::std::convert::From<PostgresParameterOverrides>
        for ::std::collections::HashMap<::std::string::String, ::std::string::String>
    {
        fn from(value: PostgresParameterOverrides) -> Self {
            value.0
        }
    }

    impl
        ::std::convert::From<
            ::std::collections::HashMap<::std::string::String, ::std::string::String>,
        > for PostgresParameterOverrides
    {
        fn from(
            value: ::std::collections::HashMap<::std::string::String, ::std::string::String>,
        ) -> Self {
            Self(value)
        }
    }

    ///Input for creating a database
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "description": "Input for creating a database",
    ///  "type": "object",
    ///  "properties": {
    ///    "databaseName": {
    ///      "default": "randomly generated",
    ///      "type": "string"
    ///    },
    ///    "databaseUser": {
    ///      "default": "randomly generated",
    ///      "type": "string"
    ///    },
    ///    "datadogAPIKey": {
    ///      "description": "The Datadog API key for the Datadog agent to
    /// monitor the new database.",
    ///      "type": "string"
    ///    },
    ///    "datadogSite": {
    ///      "description": "Datadog region to use for monitoring the new
    /// database. Defaults to 'US1'.",
    ///      "examples": [
    ///        "US1"
    ///      ],
    ///      "type": "string"
    ///    },
    ///    "diskSizeGB": {
    ///      "description": "The number of gigabytes of disk space to allocate
    /// for the database",
    ///      "type": "integer"
    ///    },
    ///    "enableDiskAutoscaling": {
    ///      "default": false,
    ///      "type": "boolean"
    ///    },
    ///    "enableHighAvailability": {
    ///      "default": false,
    ///      "type": "boolean"
    ///    },
    ///    "environmentId": {
    ///      "type": "string"
    ///    },
    ///    "ipAllowList": {
    ///      "type": "array",
    ///      "items": {
    ///        "$ref": "#/components/schemas/cidrBlockAndDescription"
    ///      }
    ///    },
    ///    "name": {
    ///      "description": "The name of the database as it will appear in the
    /// Render Dashboard",
    ///      "type": "string"
    ///    },
    ///    "ownerId": {
    ///      "description": "The ID of the workspace to create the database
    /// for",
    ///      "type": "string"
    ///    },
    ///    "parameterOverrides": {
    ///      "$ref": "#/components/schemas/postgresParameterOverrides"
    ///    },
    ///    "plan": {
    ///      "type": "string"
    ///    },
    ///    "readReplicas": {
    ///      "$ref": "#/components/schemas/readReplicasInput"
    ///    },
    ///    "region": {
    ///      "$ref": "#/components/schemas/region"
    ///    },
    ///    "version": {
    ///      "$ref": "#/components/schemas/postgresVersion"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PostgresPostInput {
        #[serde(
            rename = "databaseName",
            default = "defaults::postgres_post_input_database_name"
        )]
        pub database_name: ::std::string::String,
        #[serde(
            rename = "databaseUser",
            default = "defaults::postgres_post_input_database_user"
        )]
        pub database_user: ::std::string::String,
        ///The Datadog API key for the Datadog agent to monitor the new
        /// database.
        #[serde(
            rename = "datadogAPIKey",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub datadog_api_key: ::std::option::Option<::std::string::String>,
        ///Datadog region to use for monitoring the new database. Defaults to
        /// 'US1'.
        #[serde(
            rename = "datadogSite",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub datadog_site: ::std::option::Option<::std::string::String>,
        ///The number of gigabytes of disk space to allocate for the database
        #[serde(
            rename = "diskSizeGB",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub disk_size_gb: ::std::option::Option<i64>,
        #[serde(rename = "enableDiskAutoscaling", default)]
        pub enable_disk_autoscaling: bool,
        #[serde(rename = "enableHighAvailability", default)]
        pub enable_high_availability: bool,
        #[serde(
            rename = "environmentId",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub environment_id: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "ipAllowList",
            default,
            skip_serializing_if = "::std::vec::Vec::is_empty"
        )]
        pub ip_allow_list: ::std::vec::Vec<CidrBlockAndDescription>,
        ///The name of the database as it will appear in the Render Dashboard
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub name: ::std::option::Option<::std::string::String>,
        ///The ID of the workspace to create the database for
        #[serde(
            rename = "ownerId",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub owner_id: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "parameterOverrides",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub parameter_overrides: ::std::option::Option<PostgresParameterOverrides>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub plan: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "readReplicas",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub read_replicas: ::std::option::Option<ReadReplicasInput>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub region: ::std::option::Option<Region>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub version: ::std::option::Option<PostgresVersion>,
    }

    impl ::std::default::Default for PostgresPostInput {
        fn default() -> Self {
            Self {
                database_name: defaults::postgres_post_input_database_name(),
                database_user: defaults::postgres_post_input_database_user(),
                datadog_api_key: Default::default(),
                datadog_site: Default::default(),
                disk_size_gb: Default::default(),
                enable_disk_autoscaling: Default::default(),
                enable_high_availability: Default::default(),
                environment_id: Default::default(),
                ip_allow_list: Default::default(),
                name: Default::default(),
                owner_id: Default::default(),
                parameter_overrides: Default::default(),
                plan: Default::default(),
                read_replicas: Default::default(),
                region: Default::default(),
                version: Default::default(),
            }
        }
    }

    ///The PostgreSQL version
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "description": "The PostgreSQL version",
    ///  "type": "string"
    ///}
    /// ```
    /// </details>
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    #[serde(transparent)]
    pub struct PostgresVersion(pub ::std::string::String);
    impl ::std::ops::Deref for PostgresVersion {
        type Target = ::std::string::String;
        fn deref(&self) -> &::std::string::String {
            &self.0
        }
    }

    impl ::std::convert::From<PostgresVersion> for ::std::string::String {
        fn from(value: PostgresVersion) -> Self {
            value.0
        }
    }

    impl ::std::convert::From<::std::string::String> for PostgresVersion {
        fn from(value: ::std::string::String) -> Self {
            Self(value)
        }
    }

    impl ::std::str::FromStr for PostgresVersion {
        type Err = ::std::convert::Infallible;
        fn from_str(value: &str) -> ::std::result::Result<Self, Self::Err> {
            Ok(Self(value.to_string()))
        }
    }

    impl ::std::fmt::Display for PostgresVersion {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            self.0.fmt(f)
        }
    }

    ///`PostgresWithCursor`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "cursor": {
    ///      "$ref": "#/components/schemas/cursor"
    ///    },
    ///    "postgres": {
    ///      "$ref": "#/components/schemas/postgres"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PostgresWithCursor {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub cursor: ::std::option::Option<Cursor>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub postgres: ::std::option::Option<Postgres>,
    }

    impl ::std::default::Default for PostgresWithCursor {
        fn default() -> Self {
            Self {
                cursor: Default::default(),
                postgres: Default::default(),
            }
        }
    }

    ///`Previews`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "generation": {
    ///      "description": "Defaults to \"off\"",
    ///      "default": "off",
    ///      "type": "string"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct Previews {
        ///Defaults to "off"
        #[serde(default = "defaults::previews_generation")]
        pub generation: ::std::string::String,
    }

    impl ::std::default::Default for Previews {
        fn default() -> Self {
            Self {
                generation: defaults::previews_generation(),
            }
        }
    }

    ///`PrivateServiceDetails`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "autoscaling": {
    ///      "type": "object",
    ///      "properties": {
    ///        "criteria": {
    ///          "type": "object",
    ///          "properties": {
    ///            "cpu": {
    ///              "type": "object",
    ///              "properties": {
    ///                "enabled": {
    ///                  "default": false,
    ///                  "type": "boolean"
    ///                },
    ///                "percentage": {
    ///                  "description": "Determines when your service will be
    /// scaled. If the average resource utilization is significantly above/below
    /// the target, we will increase/decrease the number of instances.\n",
    ///                  "type": "integer"
    ///                }
    ///              }
    ///            },
    ///            "memory": {
    ///              "type": "object",
    ///              "properties": {
    ///                "enabled": {
    ///                  "default": false,
    ///                  "type": "boolean"
    ///                },
    ///                "percentage": {
    ///                  "description": "Determines when your service will be
    /// scaled. If the average resource utilization is significantly above/below
    /// the target, we will increase/decrease the number of instances.\n",
    ///                  "type": "integer"
    ///                }
    ///              }
    ///            }
    ///          }
    ///        },
    ///        "enabled": {
    ///          "default": false,
    ///          "type": "boolean"
    ///        },
    ///        "max": {
    ///          "description": "The maximum number of instances for the
    /// service",
    ///          "type": "integer"
    ///        },
    ///        "min": {
    ///          "description": "The minimum number of instances for the
    /// service",
    ///          "type": "integer"
    ///        }
    ///      }
    ///    },
    ///    "buildPlan": {
    ///      "$ref": "#/components/schemas/buildPlan"
    ///    },
    ///    "disk": {
    ///      "type": "object",
    ///      "properties": {
    ///        "id": {
    ///          "examples": [
    ///            "dsk-cph1rs3idesc73a2b2mg"
    ///          ],
    ///          "type": "string",
    ///          "pattern": "^dsk-[0-9a-z]{20}$"
    ///        },
    ///        "mountPath": {
    ///          "type": "string"
    ///        },
    ///        "name": {
    ///          "type": "string"
    ///        },
    ///        "sizeGB": {
    ///          "type": "integer"
    ///        }
    ///      }
    ///    },
    ///    "env": {
    ///      "$ref": "#/components/schemas/serviceEnv"
    ///    },
    ///    "envSpecificDetails": {
    ///      "$ref": "#/components/schemas/envSpecificDetails"
    ///    },
    ///    "maxShutdownDelaySeconds": {
    ///      "$ref": "#/components/schemas/maxShutdownDelaySeconds"
    ///    },
    ///    "numInstances": {
    ///      "description": "For a *manually* scaled service, this is the number
    /// of instances the service is scaled to. DOES NOT indicate the number of
    /// running instances for an *autoscaled* service.",
    ///      "type": "integer"
    ///    },
    ///    "openPorts": {
    ///      "type": "array",
    ///      "items": {
    ///        "$ref": "#/components/schemas/serverPort"
    ///      }
    ///    },
    ///    "parentServer": {
    ///      "$ref": "#/components/schemas/resource"
    ///    },
    ///    "plan": {
    ///      "$ref": "#/components/schemas/plan"
    ///    },
    ///    "previews": {
    ///      "$ref": "#/components/schemas/previews"
    ///    },
    ///    "pullRequestPreviewsEnabled": {
    ///      "$ref": "#/components/schemas/pullRequestPreviewsEnabled"
    ///    },
    ///    "region": {
    ///      "$ref": "#/components/schemas/region"
    ///    },
    ///    "runtime": {
    ///      "$ref": "#/components/schemas/serviceRuntime"
    ///    },
    ///    "sshAddress": {
    ///      "$ref": "#/components/schemas/sshAddress"
    ///    },
    ///    "url": {
    ///      "type": "string"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PrivateServiceDetails {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub autoscaling: ::std::option::Option<PrivateServiceDetailsAutoscaling>,
        #[serde(
            rename = "buildPlan",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub build_plan: ::std::option::Option<BuildPlan>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub disk: ::std::option::Option<PrivateServiceDetailsDisk>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub env: ::std::option::Option<ServiceEnv>,
        #[serde(
            rename = "envSpecificDetails",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub env_specific_details: ::std::option::Option<EnvSpecificDetails>,
        #[serde(
            rename = "maxShutdownDelaySeconds",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub max_shutdown_delay_seconds: ::std::option::Option<MaxShutdownDelaySeconds>,
        ///For a *manually* scaled service, this is the number of instances the
        /// service is scaled to. DOES NOT indicate the number of running
        /// instances for an *autoscaled* service.
        #[serde(
            rename = "numInstances",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub num_instances: ::std::option::Option<i64>,
        #[serde(
            rename = "openPorts",
            default,
            skip_serializing_if = "::std::vec::Vec::is_empty"
        )]
        pub open_ports: ::std::vec::Vec<ServerPort>,
        #[serde(
            rename = "parentServer",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub parent_server: ::std::option::Option<Resource>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub plan: ::std::option::Option<Plan>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub previews: ::std::option::Option<Previews>,
        #[serde(
            rename = "pullRequestPreviewsEnabled",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub pull_request_previews_enabled: ::std::option::Option<PullRequestPreviewsEnabled>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub region: ::std::option::Option<Region>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub runtime: ::std::option::Option<ServiceRuntime>,
        #[serde(
            rename = "sshAddress",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub ssh_address: ::std::option::Option<SshAddress>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub url: ::std::option::Option<::std::string::String>,
    }

    impl ::std::default::Default for PrivateServiceDetails {
        fn default() -> Self {
            Self {
                autoscaling: Default::default(),
                build_plan: Default::default(),
                disk: Default::default(),
                env: Default::default(),
                env_specific_details: Default::default(),
                max_shutdown_delay_seconds: Default::default(),
                num_instances: Default::default(),
                open_ports: Default::default(),
                parent_server: Default::default(),
                plan: Default::default(),
                previews: Default::default(),
                pull_request_previews_enabled: Default::default(),
                region: Default::default(),
                runtime: Default::default(),
                ssh_address: Default::default(),
                url: Default::default(),
            }
        }
    }

    ///`PrivateServiceDetailsAutoscaling`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "criteria": {
    ///      "type": "object",
    ///      "properties": {
    ///        "cpu": {
    ///          "type": "object",
    ///          "properties": {
    ///            "enabled": {
    ///              "default": false,
    ///              "type": "boolean"
    ///            },
    ///            "percentage": {
    ///              "description": "Determines when your service will be
    /// scaled. If the average resource utilization is significantly above/below
    /// the target, we will increase/decrease the number of instances.\n",
    ///              "type": "integer"
    ///            }
    ///          }
    ///        },
    ///        "memory": {
    ///          "type": "object",
    ///          "properties": {
    ///            "enabled": {
    ///              "default": false,
    ///              "type": "boolean"
    ///            },
    ///            "percentage": {
    ///              "description": "Determines when your service will be
    /// scaled. If the average resource utilization is significantly above/below
    /// the target, we will increase/decrease the number of instances.\n",
    ///              "type": "integer"
    ///            }
    ///          }
    ///        }
    ///      }
    ///    },
    ///    "enabled": {
    ///      "default": false,
    ///      "type": "boolean"
    ///    },
    ///    "max": {
    ///      "description": "The maximum number of instances for the service",
    ///      "type": "integer"
    ///    },
    ///    "min": {
    ///      "description": "The minimum number of instances for the service",
    ///      "type": "integer"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PrivateServiceDetailsAutoscaling {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub criteria: ::std::option::Option<PrivateServiceDetailsAutoscalingCriteria>,
        #[serde(default)]
        pub enabled: bool,
        ///The maximum number of instances for the service
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub max: ::std::option::Option<i64>,
        ///The minimum number of instances for the service
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub min: ::std::option::Option<i64>,
    }

    impl ::std::default::Default for PrivateServiceDetailsAutoscaling {
        fn default() -> Self {
            Self {
                criteria: Default::default(),
                enabled: Default::default(),
                max: Default::default(),
                min: Default::default(),
            }
        }
    }

    ///`PrivateServiceDetailsAutoscalingCriteria`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "cpu": {
    ///      "type": "object",
    ///      "properties": {
    ///        "enabled": {
    ///          "default": false,
    ///          "type": "boolean"
    ///        },
    ///        "percentage": {
    ///          "description": "Determines when your service will be scaled. If
    /// the average resource utilization is significantly above/below the
    /// target, we will increase/decrease the number of instances.\n",
    ///          "type": "integer"
    ///        }
    ///      }
    ///    },
    ///    "memory": {
    ///      "type": "object",
    ///      "properties": {
    ///        "enabled": {
    ///          "default": false,
    ///          "type": "boolean"
    ///        },
    ///        "percentage": {
    ///          "description": "Determines when your service will be scaled. If
    /// the average resource utilization is significantly above/below the
    /// target, we will increase/decrease the number of instances.\n",
    ///          "type": "integer"
    ///        }
    ///      }
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PrivateServiceDetailsAutoscalingCriteria {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub cpu: ::std::option::Option<PrivateServiceDetailsAutoscalingCriteriaCpu>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub memory: ::std::option::Option<PrivateServiceDetailsAutoscalingCriteriaMemory>,
    }

    impl ::std::default::Default for PrivateServiceDetailsAutoscalingCriteria {
        fn default() -> Self {
            Self {
                cpu: Default::default(),
                memory: Default::default(),
            }
        }
    }

    ///`PrivateServiceDetailsAutoscalingCriteriaCpu`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "enabled": {
    ///      "default": false,
    ///      "type": "boolean"
    ///    },
    ///    "percentage": {
    ///      "description": "Determines when your service will be scaled. If the
    /// average resource utilization is significantly above/below the target, we
    /// will increase/decrease the number of instances.\n",
    ///      "type": "integer"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PrivateServiceDetailsAutoscalingCriteriaCpu {
        #[serde(default)]
        pub enabled: bool,
        ///Determines when your service will be scaled. If the average resource
        /// utilization is significantly above/below the target, we will
        /// increase/decrease the number of instances.
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub percentage: ::std::option::Option<i64>,
    }

    impl ::std::default::Default for PrivateServiceDetailsAutoscalingCriteriaCpu {
        fn default() -> Self {
            Self {
                enabled: Default::default(),
                percentage: Default::default(),
            }
        }
    }

    ///`PrivateServiceDetailsAutoscalingCriteriaMemory`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "enabled": {
    ///      "default": false,
    ///      "type": "boolean"
    ///    },
    ///    "percentage": {
    ///      "description": "Determines when your service will be scaled. If the
    /// average resource utilization is significantly above/below the target, we
    /// will increase/decrease the number of instances.\n",
    ///      "type": "integer"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PrivateServiceDetailsAutoscalingCriteriaMemory {
        #[serde(default)]
        pub enabled: bool,
        ///Determines when your service will be scaled. If the average resource
        /// utilization is significantly above/below the target, we will
        /// increase/decrease the number of instances.
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub percentage: ::std::option::Option<i64>,
    }

    impl ::std::default::Default for PrivateServiceDetailsAutoscalingCriteriaMemory {
        fn default() -> Self {
            Self {
                enabled: Default::default(),
                percentage: Default::default(),
            }
        }
    }

    ///`PrivateServiceDetailsDisk`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "id": {
    ///      "examples": [
    ///        "dsk-cph1rs3idesc73a2b2mg"
    ///      ],
    ///      "type": "string",
    ///      "pattern": "^dsk-[0-9a-z]{20}$"
    ///    },
    ///    "mountPath": {
    ///      "type": "string"
    ///    },
    ///    "name": {
    ///      "type": "string"
    ///    },
    ///    "sizeGB": {
    ///      "type": "integer"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PrivateServiceDetailsDisk {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub id: ::std::option::Option<PrivateServiceDetailsDiskId>,
        #[serde(
            rename = "mountPath",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub mount_path: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub name: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "sizeGB",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub size_gb: ::std::option::Option<i64>,
    }

    impl ::std::default::Default for PrivateServiceDetailsDisk {
        fn default() -> Self {
            Self {
                id: Default::default(),
                mount_path: Default::default(),
                name: Default::default(),
                size_gb: Default::default(),
            }
        }
    }

    ///`PrivateServiceDetailsDiskId`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "examples": [
    ///    "dsk-cph1rs3idesc73a2b2mg"
    ///  ],
    ///  "type": "string",
    ///  "pattern": "^dsk-[0-9a-z]{20}$"
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
    #[serde(transparent)]
    pub struct PrivateServiceDetailsDiskId(::std::string::String);
    impl ::std::ops::Deref for PrivateServiceDetailsDiskId {
        type Target = ::std::string::String;
        fn deref(&self) -> &::std::string::String {
            &self.0
        }
    }

    impl ::std::convert::From<PrivateServiceDetailsDiskId> for ::std::string::String {
        fn from(value: PrivateServiceDetailsDiskId) -> Self {
            value.0
        }
    }

    impl ::std::str::FromStr for PrivateServiceDetailsDiskId {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
                ::std::sync::LazyLock::new(|| ::regress::Regex::new("^dsk-[0-9a-z]{20}$").unwrap());
            if PATTERN.find(value).is_none() {
                return Err("doesn't match pattern \"^dsk-[0-9a-z]{20}$\"".into());
            }
            Ok(Self(value.to_string()))
        }
    }

    impl ::std::convert::TryFrom<&str> for PrivateServiceDetailsDiskId {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }

    impl ::std::convert::TryFrom<&::std::string::String> for PrivateServiceDetailsDiskId {
        type Error = self::error::ConversionError;
        fn try_from(
            value: &::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }

    impl ::std::convert::TryFrom<::std::string::String> for PrivateServiceDetailsDiskId {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }

    impl<'de> ::serde::Deserialize<'de> for PrivateServiceDetailsDiskId {
        fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
        where
            D: ::serde::Deserializer<'de>,
        {
            ::std::string::String::deserialize(deserializer)?
                .parse()
                .map_err(|e: self::error::ConversionError| {
                    <D::Error as ::serde::de::Error>::custom(e.to_string())
                })
        }
    }

    ///`PrivateServiceDetailsPost`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "autoscaling": {
    ///      "type": "object",
    ///      "properties": {
    ///        "criteria": {
    ///          "type": "object",
    ///          "properties": {
    ///            "cpu": {
    ///              "type": "object",
    ///              "properties": {
    ///                "enabled": {
    ///                  "default": false,
    ///                  "type": "boolean"
    ///                },
    ///                "percentage": {
    ///                  "description": "Determines when your service will be
    /// scaled. If the average resource utilization is significantly above/below
    /// the target, we will increase/decrease the number of instances.\n",
    ///                  "type": "integer"
    ///                }
    ///              }
    ///            },
    ///            "memory": {
    ///              "type": "object",
    ///              "properties": {
    ///                "enabled": {
    ///                  "default": false,
    ///                  "type": "boolean"
    ///                },
    ///                "percentage": {
    ///                  "description": "Determines when your service will be
    /// scaled. If the average resource utilization is significantly above/below
    /// the target, we will increase/decrease the number of instances.\n",
    ///                  "type": "integer"
    ///                }
    ///              }
    ///            }
    ///          }
    ///        },
    ///        "enabled": {
    ///          "default": false,
    ///          "type": "boolean"
    ///        },
    ///        "max": {
    ///          "description": "The maximum number of instances for the
    /// service",
    ///          "type": "integer"
    ///        },
    ///        "min": {
    ///          "description": "The minimum number of instances for the
    /// service",
    ///          "type": "integer"
    ///        }
    ///      }
    ///    },
    ///    "disk": {
    ///      "$ref": "#/components/schemas/serviceDisk"
    ///    },
    ///    "env": {
    ///      "$ref": "#/components/schemas/serviceEnv"
    ///    },
    ///    "envSpecificDetails": {
    ///      "$ref": "#/components/schemas/envSpecificDetailsPOST"
    ///    },
    ///    "maxShutdownDelaySeconds": {
    ///      "$ref": "#/components/schemas/maxShutdownDelaySeconds"
    ///    },
    ///    "numInstances": {
    ///      "description": "Defaults to 1",
    ///      "default": 1,
    ///      "type": "integer",
    ///      "minimum": 1.0
    ///    },
    ///    "plan": {
    ///      "$ref": "#/components/schemas/paidPlan"
    ///    },
    ///    "preDeployCommand": {
    ///      "type": "string"
    ///    },
    ///    "previews": {
    ///      "$ref": "#/components/schemas/previews"
    ///    },
    ///    "pullRequestPreviewsEnabled": {
    ///      "$ref": "#/components/schemas/pullRequestPreviewsEnabled"
    ///    },
    ///    "region": {
    ///      "$ref": "#/components/schemas/region"
    ///    },
    ///    "runtime": {
    ///      "$ref": "#/components/schemas/serviceRuntime"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PrivateServiceDetailsPost {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub autoscaling: ::std::option::Option<PrivateServiceDetailsPostAutoscaling>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub disk: ::std::option::Option<ServiceDisk>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub env: ::std::option::Option<ServiceEnv>,
        #[serde(
            rename = "envSpecificDetails",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub env_specific_details: ::std::option::Option<EnvSpecificDetailsPost>,
        #[serde(
            rename = "maxShutdownDelaySeconds",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub max_shutdown_delay_seconds: ::std::option::Option<MaxShutdownDelaySeconds>,
        ///Defaults to 1
        #[serde(
            rename = "numInstances",
            default = "defaults::default_nzu64::<::std::num::NonZeroU64, 1>"
        )]
        pub num_instances: ::std::num::NonZeroU64,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub plan: ::std::option::Option<PaidPlan>,
        #[serde(
            rename = "preDeployCommand",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub pre_deploy_command: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub previews: ::std::option::Option<Previews>,
        #[serde(
            rename = "pullRequestPreviewsEnabled",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub pull_request_previews_enabled: ::std::option::Option<PullRequestPreviewsEnabled>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub region: ::std::option::Option<Region>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub runtime: ::std::option::Option<ServiceRuntime>,
    }

    impl ::std::default::Default for PrivateServiceDetailsPost {
        fn default() -> Self {
            Self {
                autoscaling: Default::default(),
                disk: Default::default(),
                env: Default::default(),
                env_specific_details: Default::default(),
                max_shutdown_delay_seconds: Default::default(),
                num_instances: defaults::default_nzu64::<::std::num::NonZeroU64, 1>(),
                plan: Default::default(),
                pre_deploy_command: Default::default(),
                previews: Default::default(),
                pull_request_previews_enabled: Default::default(),
                region: Default::default(),
                runtime: Default::default(),
            }
        }
    }

    ///`PrivateServiceDetailsPostAutoscaling`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "criteria": {
    ///      "type": "object",
    ///      "properties": {
    ///        "cpu": {
    ///          "type": "object",
    ///          "properties": {
    ///            "enabled": {
    ///              "default": false,
    ///              "type": "boolean"
    ///            },
    ///            "percentage": {
    ///              "description": "Determines when your service will be
    /// scaled. If the average resource utilization is significantly above/below
    /// the target, we will increase/decrease the number of instances.\n",
    ///              "type": "integer"
    ///            }
    ///          }
    ///        },
    ///        "memory": {
    ///          "type": "object",
    ///          "properties": {
    ///            "enabled": {
    ///              "default": false,
    ///              "type": "boolean"
    ///            },
    ///            "percentage": {
    ///              "description": "Determines when your service will be
    /// scaled. If the average resource utilization is significantly above/below
    /// the target, we will increase/decrease the number of instances.\n",
    ///              "type": "integer"
    ///            }
    ///          }
    ///        }
    ///      }
    ///    },
    ///    "enabled": {
    ///      "default": false,
    ///      "type": "boolean"
    ///    },
    ///    "max": {
    ///      "description": "The maximum number of instances for the service",
    ///      "type": "integer"
    ///    },
    ///    "min": {
    ///      "description": "The minimum number of instances for the service",
    ///      "type": "integer"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PrivateServiceDetailsPostAutoscaling {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub criteria: ::std::option::Option<PrivateServiceDetailsPostAutoscalingCriteria>,
        #[serde(default)]
        pub enabled: bool,
        ///The maximum number of instances for the service
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub max: ::std::option::Option<i64>,
        ///The minimum number of instances for the service
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub min: ::std::option::Option<i64>,
    }

    impl ::std::default::Default for PrivateServiceDetailsPostAutoscaling {
        fn default() -> Self {
            Self {
                criteria: Default::default(),
                enabled: Default::default(),
                max: Default::default(),
                min: Default::default(),
            }
        }
    }

    ///`PrivateServiceDetailsPostAutoscalingCriteria`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "cpu": {
    ///      "type": "object",
    ///      "properties": {
    ///        "enabled": {
    ///          "default": false,
    ///          "type": "boolean"
    ///        },
    ///        "percentage": {
    ///          "description": "Determines when your service will be scaled. If
    /// the average resource utilization is significantly above/below the
    /// target, we will increase/decrease the number of instances.\n",
    ///          "type": "integer"
    ///        }
    ///      }
    ///    },
    ///    "memory": {
    ///      "type": "object",
    ///      "properties": {
    ///        "enabled": {
    ///          "default": false,
    ///          "type": "boolean"
    ///        },
    ///        "percentage": {
    ///          "description": "Determines when your service will be scaled. If
    /// the average resource utilization is significantly above/below the
    /// target, we will increase/decrease the number of instances.\n",
    ///          "type": "integer"
    ///        }
    ///      }
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PrivateServiceDetailsPostAutoscalingCriteria {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub cpu: ::std::option::Option<PrivateServiceDetailsPostAutoscalingCriteriaCpu>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub memory: ::std::option::Option<PrivateServiceDetailsPostAutoscalingCriteriaMemory>,
    }

    impl ::std::default::Default for PrivateServiceDetailsPostAutoscalingCriteria {
        fn default() -> Self {
            Self {
                cpu: Default::default(),
                memory: Default::default(),
            }
        }
    }

    ///`PrivateServiceDetailsPostAutoscalingCriteriaCpu`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "enabled": {
    ///      "default": false,
    ///      "type": "boolean"
    ///    },
    ///    "percentage": {
    ///      "description": "Determines when your service will be scaled. If the
    /// average resource utilization is significantly above/below the target, we
    /// will increase/decrease the number of instances.\n",
    ///      "type": "integer"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PrivateServiceDetailsPostAutoscalingCriteriaCpu {
        #[serde(default)]
        pub enabled: bool,
        ///Determines when your service will be scaled. If the average resource
        /// utilization is significantly above/below the target, we will
        /// increase/decrease the number of instances.
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub percentage: ::std::option::Option<i64>,
    }

    impl ::std::default::Default for PrivateServiceDetailsPostAutoscalingCriteriaCpu {
        fn default() -> Self {
            Self {
                enabled: Default::default(),
                percentage: Default::default(),
            }
        }
    }

    ///`PrivateServiceDetailsPostAutoscalingCriteriaMemory`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "enabled": {
    ///      "default": false,
    ///      "type": "boolean"
    ///    },
    ///    "percentage": {
    ///      "description": "Determines when your service will be scaled. If the
    /// average resource utilization is significantly above/below the target, we
    /// will increase/decrease the number of instances.\n",
    ///      "type": "integer"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct PrivateServiceDetailsPostAutoscalingCriteriaMemory {
        #[serde(default)]
        pub enabled: bool,
        ///Determines when your service will be scaled. If the average resource
        /// utilization is significantly above/below the target, we will
        /// increase/decrease the number of instances.
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub percentage: ::std::option::Option<i64>,
    }

    impl ::std::default::Default for PrivateServiceDetailsPostAutoscalingCriteriaMemory {
        fn default() -> Self {
            Self {
                enabled: Default::default(),
                percentage: Default::default(),
            }
        }
    }

    ///This field has been deprecated. previews.generation should be used in
    /// its place.
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "description": "This field has been deprecated. previews.generation
    /// should be used in its place.",
    ///  "default": "no",
    ///  "deprecated": true,
    ///  "type": "string"
    ///}
    /// ```
    /// </details>
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    #[serde(transparent)]
    pub struct PullRequestPreviewsEnabled(pub ::std::string::String);
    impl ::std::ops::Deref for PullRequestPreviewsEnabled {
        type Target = ::std::string::String;
        fn deref(&self) -> &::std::string::String {
            &self.0
        }
    }

    impl ::std::convert::From<PullRequestPreviewsEnabled> for ::std::string::String {
        fn from(value: PullRequestPreviewsEnabled) -> Self {
            value.0
        }
    }

    impl ::std::convert::From<::std::string::String> for PullRequestPreviewsEnabled {
        fn from(value: ::std::string::String) -> Self {
            Self(value)
        }
    }

    impl ::std::str::FromStr for PullRequestPreviewsEnabled {
        type Err = ::std::convert::Infallible;
        fn from_str(value: &str) -> ::std::result::Result<Self, Self::Err> {
            Ok(Self(value.to_string()))
        }
    }

    impl ::std::fmt::Display for PullRequestPreviewsEnabled {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            self.0.fmt(f)
        }
    }

    ///`ReadReplica`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "id": {
    ///      "description": "The replica instance identifier.",
    ///      "type": "string"
    ///    },
    ///    "name": {
    ///      "description": "The display name of the replica instance.",
    ///      "type": "string"
    ///    },
    ///    "parameterOverrides": {
    ///      "$ref": "#/components/schemas/postgresParameterOverrides"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct ReadReplica {
        ///The replica instance identifier.
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub id: ::std::option::Option<::std::string::String>,
        ///The display name of the replica instance.
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub name: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "parameterOverrides",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub parameter_overrides: ::std::option::Option<PostgresParameterOverrides>,
    }

    impl ::std::default::Default for ReadReplica {
        fn default() -> Self {
            Self {
                id: Default::default(),
                name: Default::default(),
                parameter_overrides: Default::default(),
            }
        }
    }

    ///`ReadReplicaInput`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "name": {
    ///      "description": "The display name of the replica instance.",
    ///      "type": "string"
    ///    },
    ///    "parameterOverrides": {
    ///      "$ref": "#/components/schemas/postgresParameterOverrides"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct ReadReplicaInput {
        ///The display name of the replica instance.
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub name: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "parameterOverrides",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub parameter_overrides: ::std::option::Option<PostgresParameterOverrides>,
    }

    impl ::std::default::Default for ReadReplicaInput {
        fn default() -> Self {
            Self {
                name: Default::default(),
                parameter_overrides: Default::default(),
            }
        }
    }

    ///`ReadReplicas`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "array",
    ///  "items": {
    ///    "$ref": "#/components/schemas/readReplica"
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(transparent)]
    pub struct ReadReplicas(pub ::std::vec::Vec<ReadReplica>);
    impl ::std::ops::Deref for ReadReplicas {
        type Target = ::std::vec::Vec<ReadReplica>;
        fn deref(&self) -> &::std::vec::Vec<ReadReplica> {
            &self.0
        }
    }

    impl ::std::convert::From<ReadReplicas> for ::std::vec::Vec<ReadReplica> {
        fn from(value: ReadReplicas) -> Self {
            value.0
        }
    }

    impl ::std::convert::From<::std::vec::Vec<ReadReplica>> for ReadReplicas {
        fn from(value: ::std::vec::Vec<ReadReplica>) -> Self {
            Self(value)
        }
    }

    ///`ReadReplicasInput`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "array",
    ///  "items": {
    ///    "$ref": "#/components/schemas/readReplicaInput"
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(transparent)]
    pub struct ReadReplicasInput(pub ::std::vec::Vec<ReadReplicaInput>);
    impl ::std::ops::Deref for ReadReplicasInput {
        type Target = ::std::vec::Vec<ReadReplicaInput>;
        fn deref(&self) -> &::std::vec::Vec<ReadReplicaInput> {
            &self.0
        }
    }

    impl ::std::convert::From<ReadReplicasInput> for ::std::vec::Vec<ReadReplicaInput> {
        fn from(value: ReadReplicasInput) -> Self {
            value.0
        }
    }

    impl ::std::convert::From<::std::vec::Vec<ReadReplicaInput>> for ReadReplicasInput {
        fn from(value: ::std::vec::Vec<ReadReplicaInput>) -> Self {
            Self(value)
        }
    }

    ///Defaults to "oregon"
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "description": "Defaults to \"oregon\"",
    ///  "default": "oregon",
    ///  "type": "string"
    ///}
    /// ```
    /// </details>
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    #[serde(transparent)]
    pub struct Region(pub ::std::string::String);
    impl ::std::ops::Deref for Region {
        type Target = ::std::string::String;
        fn deref(&self) -> &::std::string::String {
            &self.0
        }
    }

    impl ::std::convert::From<Region> for ::std::string::String {
        fn from(value: Region) -> Self {
            value.0
        }
    }

    impl ::std::convert::From<::std::string::String> for Region {
        fn from(value: ::std::string::String) -> Self {
            Self(value)
        }
    }

    impl ::std::str::FromStr for Region {
        type Err = ::std::convert::Infallible;
        fn from_str(value: &str) -> ::std::result::Result<Self, Self::Err> {
            Ok(Self(value.to_string()))
        }
    }

    impl ::std::fmt::Display for Region {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            self.0.fmt(f)
        }
    }

    ///`RegistryCredential`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "id": {
    ///      "description": "Unique identifier for this credential",
    ///      "type": "string"
    ///    },
    ///    "name": {
    ///      "description": "Descriptive name for this credential",
    ///      "type": "string"
    ///    },
    ///    "registry": {
    ///      "$ref": "#/components/schemas/registryCredentialRegistry"
    ///    },
    ///    "updatedAt": {
    ///      "description": "Last updated time for the credential",
    ///      "type": "string",
    ///      "format": "date-time"
    ///    },
    ///    "username": {
    ///      "description": "The username associated with the credential",
    ///      "type": "string"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct RegistryCredential {
        ///Unique identifier for this credential
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub id: ::std::option::Option<::std::string::String>,
        ///Descriptive name for this credential
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub name: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub registry: ::std::option::Option<RegistryCredentialRegistry>,
        ///Last updated time for the credential
        #[serde(
            rename = "updatedAt",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub updated_at: ::std::option::Option<::chrono::DateTime<::chrono::offset::Utc>>,
        ///The username associated with the credential
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub username: ::std::option::Option<::std::string::String>,
    }

    impl ::std::default::Default for RegistryCredential {
        fn default() -> Self {
            Self {
                id: Default::default(),
                name: Default::default(),
                registry: Default::default(),
                updated_at: Default::default(),
                username: Default::default(),
            }
        }
    }

    ///The registry to use this credential with
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "description": "The registry to use this credential with",
    ///  "type": "string"
    ///}
    /// ```
    /// </details>
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    #[serde(transparent)]
    pub struct RegistryCredentialRegistry(pub ::std::string::String);
    impl ::std::ops::Deref for RegistryCredentialRegistry {
        type Target = ::std::string::String;
        fn deref(&self) -> &::std::string::String {
            &self.0
        }
    }

    impl ::std::convert::From<RegistryCredentialRegistry> for ::std::string::String {
        fn from(value: RegistryCredentialRegistry) -> Self {
            value.0
        }
    }

    impl ::std::convert::From<::std::string::String> for RegistryCredentialRegistry {
        fn from(value: ::std::string::String) -> Self {
            Self(value)
        }
    }

    impl ::std::str::FromStr for RegistryCredentialRegistry {
        type Err = ::std::convert::Infallible;
        fn from_str(value: &str) -> ::std::result::Result<Self, Self::Err> {
            Ok(Self(value.to_string()))
        }
    }

    impl ::std::fmt::Display for RegistryCredentialRegistry {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            self.0.fmt(f)
        }
    }

    ///`RegistryCredentialSummary`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "id": {
    ///      "type": "string"
    ///    },
    ///    "name": {
    ///      "type": "string"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct RegistryCredentialSummary {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub id: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub name: ::std::option::Option<::std::string::String>,
    }

    impl ::std::default::Default for RegistryCredentialSummary {
        fn default() -> Self {
            Self {
                id: Default::default(),
                name: Default::default(),
            }
        }
    }

    ///Controls whether render.com subdomains are available for the service
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "description": "Controls whether render.com subdomains are available
    /// for the service",
    ///  "type": "string"
    ///}
    /// ```
    /// </details>
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    #[serde(transparent)]
    pub struct RenderSubdomainPolicy(pub ::std::string::String);
    impl ::std::ops::Deref for RenderSubdomainPolicy {
        type Target = ::std::string::String;
        fn deref(&self) -> &::std::string::String {
            &self.0
        }
    }

    impl ::std::convert::From<RenderSubdomainPolicy> for ::std::string::String {
        fn from(value: RenderSubdomainPolicy) -> Self {
            value.0
        }
    }

    impl ::std::convert::From<::std::string::String> for RenderSubdomainPolicy {
        fn from(value: ::std::string::String) -> Self {
            Self(value)
        }
    }

    impl ::std::str::FromStr for RenderSubdomainPolicy {
        type Err = ::std::convert::Infallible;
        fn from_str(value: &str) -> ::std::result::Result<Self, Self::Err> {
            Ok(Self(value.to_string()))
        }
    }

    impl ::std::fmt::Display for RenderSubdomainPolicy {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            self.0.fmt(f)
        }
    }

    ///`Resource`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "id": {
    ///      "type": "string"
    ///    },
    ///    "name": {
    ///      "type": "string"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct Resource {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub id: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub name: ::std::option::Option<::std::string::String>,
    }

    impl ::std::default::Default for Resource {
        fn default() -> Self {
            Self {
                id: Default::default(),
                name: Default::default(),
            }
        }
    }

    ///`Route`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "destination": {
    ///      "type": "string"
    ///    },
    ///    "id": {
    ///      "type": "string"
    ///    },
    ///    "priority": {
    ///      "description": "Redirect and Rewrite Rules are applied in priority
    /// order starting at 0",
    ///      "type": "integer"
    ///    },
    ///    "source": {
    ///      "type": "string"
    ///    },
    ///    "type": {
    ///      "$ref": "#/components/schemas/routeType"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct Route {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub destination: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub id: ::std::option::Option<::std::string::String>,
        ///Redirect and Rewrite Rules are applied in priority order starting at
        /// 0
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub priority: ::std::option::Option<i64>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub source: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "type",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub type_: ::std::option::Option<RouteType>,
    }

    impl ::std::default::Default for Route {
        fn default() -> Self {
            Self {
                destination: Default::default(),
                id: Default::default(),
                priority: Default::default(),
                source: Default::default(),
                type_: Default::default(),
            }
        }
    }

    ///`RoutePatch`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "priority": {
    ///      "description": "Redirect and Rewrite Rules are applied in priority
    /// order starting at 0. Moves this route to the specified priority and
    /// adjusts other route priorities accordingly.",
    ///      "type": "integer",
    ///      "x-go-type": "*int"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct RoutePatch {
        ///Redirect and Rewrite Rules are applied in priority order starting at
        /// 0. Moves this route to the specified priority and adjusts other
        /// route priorities accordingly.
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub priority: ::std::option::Option<i64>,
    }

    impl ::std::default::Default for RoutePatch {
        fn default() -> Self {
            Self {
                priority: Default::default(),
            }
        }
    }

    ///`RoutePost`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "destination": {
    ///      "examples": [
    ///        "/foo/:bar"
    ///      ],
    ///      "type": "string"
    ///    },
    ///    "priority": {
    ///      "description": "Redirect and Rewrite Rules are applied in priority
    /// order starting at 0. Defaults to last in the priority list.",
    ///      "type": "integer"
    ///    },
    ///    "source": {
    ///      "examples": [
    ///        "/:bar/foo"
    ///      ],
    ///      "type": "string"
    ///    },
    ///    "type": {
    ///      "$ref": "#/components/schemas/routeType"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct RoutePost {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub destination: ::std::option::Option<::std::string::String>,
        ///Redirect and Rewrite Rules are applied in priority order starting at
        /// 0. Defaults to last in the priority list.
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub priority: ::std::option::Option<i64>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub source: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "type",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub type_: ::std::option::Option<RouteType>,
    }

    impl ::std::default::Default for RoutePost {
        fn default() -> Self {
            Self {
                destination: Default::default(),
                priority: Default::default(),
                source: Default::default(),
                type_: Default::default(),
            }
        }
    }

    ///`RoutePut`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "destination": {
    ///      "examples": [
    ///        "/foo/:bar"
    ///      ],
    ///      "type": "string"
    ///    },
    ///    "source": {
    ///      "examples": [
    ///        "/:bar/foo"
    ///      ],
    ///      "type": "string"
    ///    },
    ///    "type": {
    ///      "$ref": "#/components/schemas/routeType"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct RoutePut {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub destination: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub source: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "type",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub type_: ::std::option::Option<RouteType>,
    }

    impl ::std::default::Default for RoutePut {
        fn default() -> Self {
            Self {
                destination: Default::default(),
                source: Default::default(),
                type_: Default::default(),
            }
        }
    }

    ///`RouteType`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "string"
    ///}
    /// ```
    /// </details>
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    #[serde(transparent)]
    pub struct RouteType(pub ::std::string::String);
    impl ::std::ops::Deref for RouteType {
        type Target = ::std::string::String;
        fn deref(&self) -> &::std::string::String {
            &self.0
        }
    }

    impl ::std::convert::From<RouteType> for ::std::string::String {
        fn from(value: RouteType) -> Self {
            value.0
        }
    }

    impl ::std::convert::From<::std::string::String> for RouteType {
        fn from(value: ::std::string::String) -> Self {
            Self(value)
        }
    }

    impl ::std::str::FromStr for RouteType {
        type Err = ::std::convert::Infallible;
        fn from_str(value: &str) -> ::std::result::Result<Self, Self::Err> {
            Ok(Self(value.to_string()))
        }
    }

    impl ::std::fmt::Display for RouteType {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            self.0.fmt(f)
        }
    }

    ///`RouteWithCursor`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "cursor": {
    ///      "type": "string"
    ///    },
    ///    "route": {
    ///      "$ref": "#/components/schemas/route"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct RouteWithCursor {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub cursor: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub route: ::std::option::Option<Route>,
    }

    impl ::std::default::Default for RouteWithCursor {
        fn default() -> Self {
            Self {
                cursor: Default::default(),
                route: Default::default(),
            }
        }
    }

    ///`SecretFileInput`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "content": {
    ///      "type": "string"
    ///    },
    ///    "name": {
    ///      "type": "string"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct SecretFileInput {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub content: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub name: ::std::option::Option<::std::string::String>,
    }

    impl ::std::default::Default for SecretFileInput {
        fn default() -> Self {
            Self {
                content: Default::default(),
                name: Default::default(),
            }
        }
    }

    ///`ServerPort`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "port": {
    ///      "examples": [
    ///        10000
    ///      ],
    ///      "type": "integer"
    ///    },
    ///    "protocol": {
    ///      "type": "string"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct ServerPort {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub port: ::std::option::Option<i64>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub protocol: ::std::option::Option<::std::string::String>,
    }

    impl ::std::default::Default for ServerPort {
        fn default() -> Self {
            Self {
                port: Default::default(),
                protocol: Default::default(),
            }
        }
    }

    ///`Service`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "autoDeploy": {
    ///      "$ref": "#/components/schemas/autoDeploy"
    ///    },
    ///    "branch": {
    ///      "type": "string"
    ///    },
    ///    "buildFilter": {
    ///      "$ref": "#/components/schemas/buildFilter"
    ///    },
    ///    "createdAt": {
    ///      "type": "string",
    ///      "format": "date-time"
    ///    },
    ///    "dashboardUrl": {
    ///      "description": "The URL to view the service in the Render
    /// Dashboard",
    ///      "type": "string"
    ///    },
    ///    "environmentId": {
    ///      "type": "string"
    ///    },
    ///    "id": {
    ///      "type": "string"
    ///    },
    ///    "imagePath": {
    ///      "type": "string"
    ///    },
    ///    "name": {
    ///      "type": "string"
    ///    },
    ///    "notifyOnFail": {
    ///      "$ref": "#/components/schemas/notifySetting"
    ///    },
    ///    "ownerId": {
    ///      "type": "string"
    ///    },
    ///    "registryCredential": {
    ///      "$ref": "#/components/schemas/registryCredentialSummary"
    ///    },
    ///    "repo": {
    ///      "examples": [
    ///        "https://github.com/render-examples/flask-hello-world"
    ///      ],
    ///      "type": "string"
    ///    },
    ///    "rootDir": {
    ///      "type": "string"
    ///    },
    ///    "serviceDetails": {
    ///      "oneOf": [
    ///        {
    ///          "$ref": "#/components/schemas/staticSiteDetails"
    ///        },
    ///        {
    ///          "$ref": "#/components/schemas/webServiceDetails"
    ///        },
    ///        {
    ///          "$ref": "#/components/schemas/privateServiceDetails"
    ///        },
    ///        {
    ///          "$ref": "#/components/schemas/backgroundWorkerDetails"
    ///        },
    ///        {
    ///          "$ref": "#/components/schemas/cronJobDetails"
    ///        }
    ///      ]
    ///    },
    ///    "slug": {
    ///      "type": "string"
    ///    },
    ///    "suspended": {
    ///      "type": "string"
    ///    },
    ///    "suspenders": {
    ///      "type": "array",
    ///      "items": {
    ///        "$ref": "#/components/schemas/suspenderType"
    ///      }
    ///    },
    ///    "type": {
    ///      "$ref": "#/components/schemas/serviceType"
    ///    },
    ///    "updatedAt": {
    ///      "type": "string",
    ///      "format": "date-time"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct Service {
        #[serde(
            rename = "autoDeploy",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub auto_deploy: ::std::option::Option<AutoDeploy>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub branch: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "buildFilter",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub build_filter: ::std::option::Option<BuildFilter>,
        #[serde(
            rename = "createdAt",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub created_at: ::std::option::Option<::chrono::DateTime<::chrono::offset::Utc>>,
        ///The URL to view the service in the Render Dashboard
        #[serde(
            rename = "dashboardUrl",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub dashboard_url: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "environmentId",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub environment_id: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub id: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "imagePath",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub image_path: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub name: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "notifyOnFail",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub notify_on_fail: ::std::option::Option<NotifySetting>,
        #[serde(
            rename = "ownerId",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub owner_id: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "registryCredential",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub registry_credential: ::std::option::Option<RegistryCredentialSummary>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub repo: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "rootDir",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub root_dir: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "serviceDetails",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub service_details: ::std::option::Option<ServiceServiceDetails>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub slug: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub suspended: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
        pub suspenders: ::std::vec::Vec<SuspenderType>,
        #[serde(
            rename = "type",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub type_: ::std::option::Option<ServiceType>,
        #[serde(
            rename = "updatedAt",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub updated_at: ::std::option::Option<::chrono::DateTime<::chrono::offset::Utc>>,
    }

    impl ::std::default::Default for Service {
        fn default() -> Self {
            Self {
                auto_deploy: Default::default(),
                branch: Default::default(),
                build_filter: Default::default(),
                created_at: Default::default(),
                dashboard_url: Default::default(),
                environment_id: Default::default(),
                id: Default::default(),
                image_path: Default::default(),
                name: Default::default(),
                notify_on_fail: Default::default(),
                owner_id: Default::default(),
                registry_credential: Default::default(),
                repo: Default::default(),
                root_dir: Default::default(),
                service_details: Default::default(),
                slug: Default::default(),
                suspended: Default::default(),
                suspenders: Default::default(),
                type_: Default::default(),
                updated_at: Default::default(),
            }
        }
    }

    ///`ServiceAndDeploy`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "deployId": {
    ///      "type": "string"
    ///    },
    ///    "service": {
    ///      "$ref": "#/components/schemas/service"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct ServiceAndDeploy {
        #[serde(
            rename = "deployId",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub deploy_id: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub service: ::std::option::Option<Service>,
    }

    impl ::std::default::Default for ServiceAndDeploy {
        fn default() -> Self {
            Self {
                deploy_id: Default::default(),
                service: Default::default(),
            }
        }
    }

    ///`ServiceDisk`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "mountPath": {
    ///      "type": "string"
    ///    },
    ///    "name": {
    ///      "type": "string"
    ///    },
    ///    "sizeGB": {
    ///      "description": "Defaults to 1",
    ///      "type": "integer",
    ///      "minimum": 1.0
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct ServiceDisk {
        #[serde(
            rename = "mountPath",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub mount_path: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub name: ::std::option::Option<::std::string::String>,
        ///Defaults to 1
        #[serde(
            rename = "sizeGB",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub size_gb: ::std::option::Option<::std::num::NonZeroU64>,
    }

    impl ::std::default::Default for ServiceDisk {
        fn default() -> Self {
            Self {
                mount_path: Default::default(),
                name: Default::default(),
                size_gb: Default::default(),
            }
        }
    }

    ///This field has been deprecated, runtime should be used in its place.
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "description": "This field has been deprecated, runtime should be used
    /// in its place.",
    ///  "deprecated": true,
    ///  "type": "string"
    ///}
    /// ```
    /// </details>
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    #[serde(transparent)]
    pub struct ServiceEnv(pub ::std::string::String);
    impl ::std::ops::Deref for ServiceEnv {
        type Target = ::std::string::String;
        fn deref(&self) -> &::std::string::String {
            &self.0
        }
    }

    impl ::std::convert::From<ServiceEnv> for ::std::string::String {
        fn from(value: ServiceEnv) -> Self {
            value.0
        }
    }

    impl ::std::convert::From<::std::string::String> for ServiceEnv {
        fn from(value: ::std::string::String) -> Self {
            Self(value)
        }
    }

    impl ::std::str::FromStr for ServiceEnv {
        type Err = ::std::convert::Infallible;
        fn from_str(value: &str) -> ::std::result::Result<Self, Self::Err> {
            Ok(Self(value.to_string()))
        }
    }

    impl ::std::fmt::Display for ServiceEnv {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            self.0.fmt(f)
        }
    }

    ///`ServiceList`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "array",
    ///  "items": {
    ///    "$ref": "#/components/schemas/serviceWithCursor"
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(transparent)]
    pub struct ServiceList(pub ::std::vec::Vec<ServiceWithCursor>);
    impl ::std::ops::Deref for ServiceList {
        type Target = ::std::vec::Vec<ServiceWithCursor>;
        fn deref(&self) -> &::std::vec::Vec<ServiceWithCursor> {
            &self.0
        }
    }

    impl ::std::convert::From<ServiceList> for ::std::vec::Vec<ServiceWithCursor> {
        fn from(value: ServiceList) -> Self {
            value.0
        }
    }

    impl ::std::convert::From<::std::vec::Vec<ServiceWithCursor>> for ServiceList {
        fn from(value: ::std::vec::Vec<ServiceWithCursor>) -> Self {
            Self(value)
        }
    }

    ///`ServicePost`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "autoDeploy": {
    ///      "$ref": "#/components/schemas/autoDeploy"
    ///    },
    ///    "branch": {
    ///      "description": "The repo branch to pull, build, and deploy. If
    /// omitted, uses the repository's default branch.",
    ///      "type": "string"
    ///    },
    ///    "buildFilter": {
    ///      "$ref": "#/components/schemas/buildFilter"
    ///    },
    ///    "envVars": {
    ///      "type": "array",
    ///      "items": {
    ///        "type": "object",
    ///        "oneOf": [
    ///          {
    ///            "type": "object",
    ///            "properties": {
    ///              "key": {
    ///                "type": "string"
    ///              },
    ///              "value": {
    ///                "type": "string"
    ///              }
    ///            }
    ///          },
    ///          {
    ///            "type": "object",
    ///            "properties": {
    ///              "generateValue": {
    ///                "description": "If true, Render generates a strong random
    /// value for this environment variable on creation. Cannot be combined with
    /// `value`.",
    ///                "type": "boolean"
    ///              },
    ///              "key": {
    ///                "type": "string"
    ///              }
    ///            }
    ///          }
    ///        ]
    ///      }
    ///    },
    ///    "environmentId": {
    ///      "description": "The ID of the environment the service belongs to,
    /// if any. Obtain an environment's ID from its Settings page in the Render
    /// Dashboard.",
    ///      "type": "string"
    ///    },
    ///    "image": {
    ///      "$ref": "#/components/schemas/image"
    ///    },
    ///    "name": {
    ///      "description": "The service's name. Must be unique within the
    /// workspace.",
    ///      "type": "string"
    ///    },
    ///    "ownerId": {
    ///      "description": "The ID of the workspace the service belongs to.
    /// Obtain your workspace's ID from its Settings page in the Render
    /// Dashboard.",
    ///      "type": "string"
    ///    },
    ///    "repo": {
    ///      "description": "The service's repository URL. Do not specify a
    /// branch in this string (use the `branch` parameter instead).",
    ///      "examples": [
    ///        "https://github.com/render-examples/flask-hello-world"
    ///      ],
    ///      "type": "string"
    ///    },
    ///    "rootDir": {
    ///      "type": "string"
    ///    },
    ///    "secretFiles": {
    ///      "type": "array",
    ///      "items": {
    ///        "$ref": "#/components/schemas/secretFileInput"
    ///      }
    ///    },
    ///    "serviceDetails": {
    ///      "oneOf": [
    ///        {
    ///          "$ref": "#/components/schemas/staticSiteDetailsPOST"
    ///        },
    ///        {
    ///          "$ref": "#/components/schemas/webServiceDetailsPOST"
    ///        },
    ///        {
    ///          "$ref": "#/components/schemas/privateServiceDetailsPOST"
    ///        },
    ///        {
    ///          "$ref": "#/components/schemas/backgroundWorkerDetailsPOST"
    ///        },
    ///        {
    ///          "$ref": "#/components/schemas/cronJobDetailsPOST"
    ///        }
    ///      ]
    ///    },
    ///    "type": {
    ///      "$ref": "#/components/schemas/serviceType"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct ServicePost {
        #[serde(
            rename = "autoDeploy",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub auto_deploy: ::std::option::Option<AutoDeploy>,
        ///The repo branch to pull, build, and deploy. If omitted, uses the
        /// repository's default branch.
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub branch: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "buildFilter",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub build_filter: ::std::option::Option<BuildFilter>,
        #[serde(
            rename = "envVars",
            default,
            skip_serializing_if = "::std::vec::Vec::is_empty"
        )]
        pub env_vars: ::std::vec::Vec<ServicePostEnvVarsItem>,
        ///The ID of the environment the service belongs to, if any. Obtain an
        /// environment's ID from its Settings page in the Render Dashboard.
        #[serde(
            rename = "environmentId",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub environment_id: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub image: ::std::option::Option<Image>,
        ///The service's name. Must be unique within the workspace.
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub name: ::std::option::Option<::std::string::String>,
        ///The ID of the workspace the service belongs to. Obtain your
        /// workspace's ID from its Settings page in the Render Dashboard.
        #[serde(
            rename = "ownerId",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub owner_id: ::std::option::Option<::std::string::String>,
        ///The service's repository URL. Do not specify a branch in this string
        /// (use the `branch` parameter instead).
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub repo: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "rootDir",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub root_dir: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "secretFiles",
            default,
            skip_serializing_if = "::std::vec::Vec::is_empty"
        )]
        pub secret_files: ::std::vec::Vec<SecretFileInput>,
        #[serde(
            rename = "serviceDetails",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub service_details: ::std::option::Option<ServicePostServiceDetails>,
        #[serde(
            rename = "type",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub type_: ::std::option::Option<ServiceType>,
    }

    impl ::std::default::Default for ServicePost {
        fn default() -> Self {
            Self {
                auto_deploy: Default::default(),
                branch: Default::default(),
                build_filter: Default::default(),
                env_vars: Default::default(),
                environment_id: Default::default(),
                image: Default::default(),
                name: Default::default(),
                owner_id: Default::default(),
                repo: Default::default(),
                root_dir: Default::default(),
                secret_files: Default::default(),
                service_details: Default::default(),
                type_: Default::default(),
            }
        }
    }

    ///`ServicePostEnvVarsItem`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "oneOf": [
    ///    {
    ///      "type": "object",
    ///      "properties": {
    ///        "key": {
    ///          "type": "string"
    ///        },
    ///        "value": {
    ///          "type": "string"
    ///        }
    ///      }
    ///    },
    ///    {
    ///      "type": "object",
    ///      "properties": {
    ///        "generateValue": {
    ///          "description": "If true, Render generates a strong random value
    /// for this environment variable on creation. Cannot be combined with
    /// `value`.",
    ///          "type": "boolean"
    ///        },
    ///        "key": {
    ///          "type": "string"
    ///        }
    ///      }
    ///    }
    ///  ]
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum ServicePostEnvVarsItem {
        Variant0 {
            #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
            key: ::std::option::Option<::std::string::String>,
            #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
            value: ::std::option::Option<::std::string::String>,
        },
        Variant1 {
            ///If true, Render generates a strong random value for this
            /// environment variable on creation. Cannot be combined with
            /// `value`.
            #[serde(
                rename = "generateValue",
                default,
                skip_serializing_if = "::std::option::Option::is_none"
            )]
            generate_value: ::std::option::Option<bool>,
            #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
            key: ::std::option::Option<::std::string::String>,
        },
    }

    ///`ServicePostServiceDetails`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "oneOf": [
    ///    {
    ///      "$ref": "#/components/schemas/staticSiteDetailsPOST"
    ///    },
    ///    {
    ///      "$ref": "#/components/schemas/webServiceDetailsPOST"
    ///    },
    ///    {
    ///      "$ref": "#/components/schemas/privateServiceDetailsPOST"
    ///    },
    ///    {
    ///      "$ref": "#/components/schemas/backgroundWorkerDetailsPOST"
    ///    },
    ///    {
    ///      "$ref": "#/components/schemas/cronJobDetailsPOST"
    ///    }
    ///  ]
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum ServicePostServiceDetails {
        StaticSiteDetailsPost(StaticSiteDetailsPost),
        WebServiceDetailsPost(WebServiceDetailsPost),
        PrivateServiceDetailsPost(PrivateServiceDetailsPost),
        BackgroundWorkerDetailsPost(BackgroundWorkerDetailsPost),
        CronJobDetailsPost(CronJobDetailsPost),
    }

    impl ::std::convert::From<StaticSiteDetailsPost> for ServicePostServiceDetails {
        fn from(value: StaticSiteDetailsPost) -> Self {
            Self::StaticSiteDetailsPost(value)
        }
    }

    impl ::std::convert::From<WebServiceDetailsPost> for ServicePostServiceDetails {
        fn from(value: WebServiceDetailsPost) -> Self {
            Self::WebServiceDetailsPost(value)
        }
    }

    impl ::std::convert::From<PrivateServiceDetailsPost> for ServicePostServiceDetails {
        fn from(value: PrivateServiceDetailsPost) -> Self {
            Self::PrivateServiceDetailsPost(value)
        }
    }

    impl ::std::convert::From<BackgroundWorkerDetailsPost> for ServicePostServiceDetails {
        fn from(value: BackgroundWorkerDetailsPost) -> Self {
            Self::BackgroundWorkerDetailsPost(value)
        }
    }

    impl ::std::convert::From<CronJobDetailsPost> for ServicePostServiceDetails {
        fn from(value: CronJobDetailsPost) -> Self {
            Self::CronJobDetailsPost(value)
        }
    }

    ///Runtime
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "description": "Runtime",
    ///  "type": "string"
    ///}
    /// ```
    /// </details>
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    #[serde(transparent)]
    pub struct ServiceRuntime(pub ::std::string::String);
    impl ::std::ops::Deref for ServiceRuntime {
        type Target = ::std::string::String;
        fn deref(&self) -> &::std::string::String {
            &self.0
        }
    }

    impl ::std::convert::From<ServiceRuntime> for ::std::string::String {
        fn from(value: ServiceRuntime) -> Self {
            value.0
        }
    }

    impl ::std::convert::From<::std::string::String> for ServiceRuntime {
        fn from(value: ::std::string::String) -> Self {
            Self(value)
        }
    }

    impl ::std::str::FromStr for ServiceRuntime {
        type Err = ::std::convert::Infallible;
        fn from_str(value: &str) -> ::std::result::Result<Self, Self::Err> {
            Ok(Self(value.to_string()))
        }
    }

    impl ::std::fmt::Display for ServiceRuntime {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            self.0.fmt(f)
        }
    }

    ///`ServiceServiceDetails`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "oneOf": [
    ///    {
    ///      "$ref": "#/components/schemas/staticSiteDetails"
    ///    },
    ///    {
    ///      "$ref": "#/components/schemas/webServiceDetails"
    ///    },
    ///    {
    ///      "$ref": "#/components/schemas/privateServiceDetails"
    ///    },
    ///    {
    ///      "$ref": "#/components/schemas/backgroundWorkerDetails"
    ///    },
    ///    {
    ///      "$ref": "#/components/schemas/cronJobDetails"
    ///    }
    ///  ]
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum ServiceServiceDetails {
        StaticSiteDetails(StaticSiteDetails),
        WebServiceDetails(WebServiceDetails),
        PrivateServiceDetails(PrivateServiceDetails),
        BackgroundWorkerDetails(BackgroundWorkerDetails),
        CronJobDetails(CronJobDetails),
    }

    impl ::std::convert::From<StaticSiteDetails> for ServiceServiceDetails {
        fn from(value: StaticSiteDetails) -> Self {
            Self::StaticSiteDetails(value)
        }
    }

    impl ::std::convert::From<WebServiceDetails> for ServiceServiceDetails {
        fn from(value: WebServiceDetails) -> Self {
            Self::WebServiceDetails(value)
        }
    }

    impl ::std::convert::From<PrivateServiceDetails> for ServiceServiceDetails {
        fn from(value: PrivateServiceDetails) -> Self {
            Self::PrivateServiceDetails(value)
        }
    }

    impl ::std::convert::From<BackgroundWorkerDetails> for ServiceServiceDetails {
        fn from(value: BackgroundWorkerDetails) -> Self {
            Self::BackgroundWorkerDetails(value)
        }
    }

    impl ::std::convert::From<CronJobDetails> for ServiceServiceDetails {
        fn from(value: CronJobDetails) -> Self {
            Self::CronJobDetails(value)
        }
    }

    ///`ServiceType`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "string"
    ///}
    /// ```
    /// </details>
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    #[serde(transparent)]
    pub struct ServiceType(pub ::std::string::String);
    impl ::std::ops::Deref for ServiceType {
        type Target = ::std::string::String;
        fn deref(&self) -> &::std::string::String {
            &self.0
        }
    }

    impl ::std::convert::From<ServiceType> for ::std::string::String {
        fn from(value: ServiceType) -> Self {
            value.0
        }
    }

    impl ::std::convert::From<::std::string::String> for ServiceType {
        fn from(value: ::std::string::String) -> Self {
            Self(value)
        }
    }

    impl ::std::str::FromStr for ServiceType {
        type Err = ::std::convert::Infallible;
        fn from_str(value: &str) -> ::std::result::Result<Self, Self::Err> {
            Ok(Self(value.to_string()))
        }
    }

    impl ::std::fmt::Display for ServiceType {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            self.0.fmt(f)
        }
    }

    ///`ServiceWithCursor`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "cursor": {
    ///      "$ref": "#/components/schemas/cursor"
    ///    },
    ///    "service": {
    ///      "$ref": "#/components/schemas/service"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct ServiceWithCursor {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub cursor: ::std::option::Option<Cursor>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub service: ::std::option::Option<Service>,
    }

    impl ::std::default::Default for ServiceWithCursor {
        fn default() -> Self {
            Self {
                cursor: Default::default(),
                service: Default::default(),
            }
        }
    }

    ///The SSH address for the service. Only present for services that have SSH
    /// enabled.
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "description": "The SSH address for the service. Only present for
    /// services that have SSH enabled.",
    ///  "type": "string"
    ///}
    /// ```
    /// </details>
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    #[serde(transparent)]
    pub struct SshAddress(pub ::std::string::String);
    impl ::std::ops::Deref for SshAddress {
        type Target = ::std::string::String;
        fn deref(&self) -> &::std::string::String {
            &self.0
        }
    }

    impl ::std::convert::From<SshAddress> for ::std::string::String {
        fn from(value: SshAddress) -> Self {
            value.0
        }
    }

    impl ::std::convert::From<::std::string::String> for SshAddress {
        fn from(value: ::std::string::String) -> Self {
            Self(value)
        }
    }

    impl ::std::str::FromStr for SshAddress {
        type Err = ::std::convert::Infallible;
        fn from_str(value: &str) -> ::std::result::Result<Self, Self::Err> {
            Ok(Self(value.to_string()))
        }
    }

    impl ::std::fmt::Display for SshAddress {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            self.0.fmt(f)
        }
    }

    ///`StaticSiteDetails`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "buildCommand": {
    ///      "type": "string"
    ///    },
    ///    "buildPlan": {
    ///      "$ref": "#/components/schemas/buildPlan"
    ///    },
    ///    "ipAllowList": {
    ///      "type": "array",
    ///      "items": {
    ///        "$ref": "#/components/schemas/cidrBlockAndDescription"
    ///      }
    ///    },
    ///    "parentServer": {
    ///      "$ref": "#/components/schemas/resource"
    ///    },
    ///    "previews": {
    ///      "$ref": "#/components/schemas/previews"
    ///    },
    ///    "publishPath": {
    ///      "type": "string"
    ///    },
    ///    "pullRequestPreviewsEnabled": {
    ///      "$ref": "#/components/schemas/pullRequestPreviewsEnabled"
    ///    },
    ///    "renderSubdomainPolicy": {
    ///      "$ref": "#/components/schemas/renderSubdomainPolicy"
    ///    },
    ///    "url": {
    ///      "type": "string"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct StaticSiteDetails {
        #[serde(
            rename = "buildCommand",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub build_command: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "buildPlan",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub build_plan: ::std::option::Option<BuildPlan>,
        #[serde(
            rename = "ipAllowList",
            default,
            skip_serializing_if = "::std::vec::Vec::is_empty"
        )]
        pub ip_allow_list: ::std::vec::Vec<CidrBlockAndDescription>,
        #[serde(
            rename = "parentServer",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub parent_server: ::std::option::Option<Resource>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub previews: ::std::option::Option<Previews>,
        #[serde(
            rename = "publishPath",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub publish_path: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "pullRequestPreviewsEnabled",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub pull_request_previews_enabled: ::std::option::Option<PullRequestPreviewsEnabled>,
        #[serde(
            rename = "renderSubdomainPolicy",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub render_subdomain_policy: ::std::option::Option<RenderSubdomainPolicy>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub url: ::std::option::Option<::std::string::String>,
    }

    impl ::std::default::Default for StaticSiteDetails {
        fn default() -> Self {
            Self {
                build_command: Default::default(),
                build_plan: Default::default(),
                ip_allow_list: Default::default(),
                parent_server: Default::default(),
                previews: Default::default(),
                publish_path: Default::default(),
                pull_request_previews_enabled: Default::default(),
                render_subdomain_policy: Default::default(),
                url: Default::default(),
            }
        }
    }

    ///`StaticSiteDetailsPost`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "buildCommand": {
    ///      "type": "string"
    ///    },
    ///    "headers": {
    ///      "type": "array",
    ///      "items": {
    ///        "$ref": "#/components/schemas/headerInput"
    ///      }
    ///    },
    ///    "ipAllowList": {
    ///      "type": "array",
    ///      "items": {
    ///        "$ref": "#/components/schemas/cidrBlockAndDescription"
    ///      }
    ///    },
    ///    "previews": {
    ///      "$ref": "#/components/schemas/previews"
    ///    },
    ///    "publishPath": {
    ///      "description": "Defaults to \"public\"",
    ///      "type": "string"
    ///    },
    ///    "pullRequestPreviewsEnabled": {
    ///      "$ref": "#/components/schemas/pullRequestPreviewsEnabled"
    ///    },
    ///    "renderSubdomainPolicy": {
    ///      "$ref": "#/components/schemas/renderSubdomainPolicy"
    ///    },
    ///    "routes": {
    ///      "type": "array",
    ///      "items": {
    ///        "$ref": "#/components/schemas/routePost"
    ///      }
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct StaticSiteDetailsPost {
        #[serde(
            rename = "buildCommand",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub build_command: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
        pub headers: ::std::vec::Vec<HeaderInput>,
        #[serde(
            rename = "ipAllowList",
            default,
            skip_serializing_if = "::std::vec::Vec::is_empty"
        )]
        pub ip_allow_list: ::std::vec::Vec<CidrBlockAndDescription>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub previews: ::std::option::Option<Previews>,
        ///Defaults to "public"
        #[serde(
            rename = "publishPath",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub publish_path: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "pullRequestPreviewsEnabled",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub pull_request_previews_enabled: ::std::option::Option<PullRequestPreviewsEnabled>,
        #[serde(
            rename = "renderSubdomainPolicy",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub render_subdomain_policy: ::std::option::Option<RenderSubdomainPolicy>,
        #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
        pub routes: ::std::vec::Vec<RoutePost>,
    }

    impl ::std::default::Default for StaticSiteDetailsPost {
        fn default() -> Self {
            Self {
                build_command: Default::default(),
                headers: Default::default(),
                ip_allow_list: Default::default(),
                previews: Default::default(),
                publish_path: Default::default(),
                pull_request_previews_enabled: Default::default(),
                render_subdomain_policy: Default::default(),
                routes: Default::default(),
            }
        }
    }

    ///`SuspenderType`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "string"
    ///}
    /// ```
    /// </details>
    #[derive(
        :: serde :: Deserialize,
        :: serde :: Serialize,
        Clone,
        Debug,
        Eq,
        Hash,
        Ord,
        PartialEq,
        PartialOrd,
    )]
    #[serde(transparent)]
    pub struct SuspenderType(pub ::std::string::String);
    impl ::std::ops::Deref for SuspenderType {
        type Target = ::std::string::String;
        fn deref(&self) -> &::std::string::String {
            &self.0
        }
    }

    impl ::std::convert::From<SuspenderType> for ::std::string::String {
        fn from(value: SuspenderType) -> Self {
            value.0
        }
    }

    impl ::std::convert::From<::std::string::String> for SuspenderType {
        fn from(value: ::std::string::String) -> Self {
            Self(value)
        }
    }

    impl ::std::str::FromStr for SuspenderType {
        type Err = ::std::convert::Infallible;
        fn from_str(value: &str) -> ::std::result::Result<Self, Self::Err> {
            Ok(Self(value.to_string()))
        }
    }

    impl ::std::fmt::Display for SuspenderType {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
            self.0.fmt(f)
        }
    }

    ///`UpdateEnvVarsForServiceBodyItem`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "oneOf": [
    ///    {
    ///      "type": "object",
    ///      "properties": {
    ///        "key": {
    ///          "type": "string"
    ///        },
    ///        "value": {
    ///          "type": "string"
    ///        }
    ///      }
    ///    },
    ///    {
    ///      "type": "object",
    ///      "properties": {
    ///        "generateValue": {
    ///          "description": "If true, Render generates a strong random value
    /// for this environment variable on creation. Cannot be combined with
    /// `value`.",
    ///          "type": "boolean"
    ///        },
    ///        "key": {
    ///          "type": "string"
    ///        }
    ///      }
    ///    }
    ///  ]
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    #[serde(untagged)]
    pub enum UpdateEnvVarsForServiceBodyItem {
        Variant0 {
            #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
            key: ::std::option::Option<::std::string::String>,
            #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
            value: ::std::option::Option<::std::string::String>,
        },
        Variant1 {
            ///If true, Render generates a strong random value for this
            /// environment variable on creation. Cannot be combined with
            /// `value`.
            #[serde(
                rename = "generateValue",
                default,
                skip_serializing_if = "::std::option::Option::is_none"
            )]
            generate_value: ::std::option::Option<bool>,
            #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
            key: ::std::option::Option<::std::string::String>,
        },
    }

    ///`WebServiceDetails`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "autoscaling": {
    ///      "type": "object",
    ///      "properties": {
    ///        "criteria": {
    ///          "type": "object",
    ///          "properties": {
    ///            "cpu": {
    ///              "type": "object",
    ///              "properties": {
    ///                "enabled": {
    ///                  "default": false,
    ///                  "type": "boolean"
    ///                },
    ///                "percentage": {
    ///                  "description": "Determines when your service will be
    /// scaled. If the average resource utilization is significantly above/below
    /// the target, we will increase/decrease the number of instances.\n",
    ///                  "type": "integer"
    ///                }
    ///              }
    ///            },
    ///            "memory": {
    ///              "type": "object",
    ///              "properties": {
    ///                "enabled": {
    ///                  "default": false,
    ///                  "type": "boolean"
    ///                },
    ///                "percentage": {
    ///                  "description": "Determines when your service will be
    /// scaled. If the average resource utilization is significantly above/below
    /// the target, we will increase/decrease the number of instances.\n",
    ///                  "type": "integer"
    ///                }
    ///              }
    ///            }
    ///          }
    ///        },
    ///        "enabled": {
    ///          "default": false,
    ///          "type": "boolean"
    ///        },
    ///        "max": {
    ///          "description": "The maximum number of instances for the
    /// service",
    ///          "type": "integer"
    ///        },
    ///        "min": {
    ///          "description": "The minimum number of instances for the
    /// service",
    ///          "type": "integer"
    ///        }
    ///      }
    ///    },
    ///    "buildPlan": {
    ///      "$ref": "#/components/schemas/buildPlan"
    ///    },
    ///    "cache": {
    ///      "$ref": "#/components/schemas/cache"
    ///    },
    ///    "disk": {
    ///      "type": "object",
    ///      "properties": {
    ///        "id": {
    ///          "examples": [
    ///            "dsk-cph1rs3idesc73a2b2mg"
    ///          ],
    ///          "type": "string",
    ///          "pattern": "^dsk-[0-9a-z]{20}$"
    ///        },
    ///        "mountPath": {
    ///          "type": "string"
    ///        },
    ///        "name": {
    ///          "type": "string"
    ///        },
    ///        "sizeGB": {
    ///          "type": "integer"
    ///        }
    ///      }
    ///    },
    ///    "env": {
    ///      "$ref": "#/components/schemas/serviceEnv"
    ///    },
    ///    "envSpecificDetails": {
    ///      "$ref": "#/components/schemas/envSpecificDetails"
    ///    },
    ///    "healthCheckPath": {
    ///      "type": "string"
    ///    },
    ///    "ipAllowList": {
    ///      "type": "array",
    ///      "items": {
    ///        "$ref": "#/components/schemas/cidrBlockAndDescription"
    ///      }
    ///    },
    ///    "maintenanceMode": {
    ///      "$ref": "#/components/schemas/maintenanceMode"
    ///    },
    ///    "maxShutdownDelaySeconds": {
    ///      "$ref": "#/components/schemas/maxShutdownDelaySeconds"
    ///    },
    ///    "numInstances": {
    ///      "description": "For a *manually* scaled service, this is the number
    /// of instances the service is scaled to. DOES NOT indicate the number of
    /// running instances for an *autoscaled* service.",
    ///      "type": "integer"
    ///    },
    ///    "openPorts": {
    ///      "type": "array",
    ///      "items": {
    ///        "$ref": "#/components/schemas/serverPort"
    ///      }
    ///    },
    ///    "parentServer": {
    ///      "$ref": "#/components/schemas/resource"
    ///    },
    ///    "plan": {
    ///      "$ref": "#/components/schemas/plan"
    ///    },
    ///    "previews": {
    ///      "$ref": "#/components/schemas/previews"
    ///    },
    ///    "pullRequestPreviewsEnabled": {
    ///      "$ref": "#/components/schemas/pullRequestPreviewsEnabled"
    ///    },
    ///    "region": {
    ///      "$ref": "#/components/schemas/region"
    ///    },
    ///    "renderSubdomainPolicy": {
    ///      "$ref": "#/components/schemas/renderSubdomainPolicy"
    ///    },
    ///    "runtime": {
    ///      "$ref": "#/components/schemas/serviceRuntime"
    ///    },
    ///    "sshAddress": {
    ///      "$ref": "#/components/schemas/sshAddress"
    ///    },
    ///    "url": {
    ///      "type": "string"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct WebServiceDetails {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub autoscaling: ::std::option::Option<WebServiceDetailsAutoscaling>,
        #[serde(
            rename = "buildPlan",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub build_plan: ::std::option::Option<BuildPlan>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub cache: ::std::option::Option<Cache>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub disk: ::std::option::Option<WebServiceDetailsDisk>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub env: ::std::option::Option<ServiceEnv>,
        #[serde(
            rename = "envSpecificDetails",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub env_specific_details: ::std::option::Option<EnvSpecificDetails>,
        #[serde(
            rename = "healthCheckPath",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub health_check_path: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "ipAllowList",
            default,
            skip_serializing_if = "::std::vec::Vec::is_empty"
        )]
        pub ip_allow_list: ::std::vec::Vec<CidrBlockAndDescription>,
        #[serde(
            rename = "maintenanceMode",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub maintenance_mode: ::std::option::Option<MaintenanceMode>,
        #[serde(
            rename = "maxShutdownDelaySeconds",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub max_shutdown_delay_seconds: ::std::option::Option<MaxShutdownDelaySeconds>,
        ///For a *manually* scaled service, this is the number of instances the
        /// service is scaled to. DOES NOT indicate the number of running
        /// instances for an *autoscaled* service.
        #[serde(
            rename = "numInstances",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub num_instances: ::std::option::Option<i64>,
        #[serde(
            rename = "openPorts",
            default,
            skip_serializing_if = "::std::vec::Vec::is_empty"
        )]
        pub open_ports: ::std::vec::Vec<ServerPort>,
        #[serde(
            rename = "parentServer",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub parent_server: ::std::option::Option<Resource>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub plan: ::std::option::Option<Plan>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub previews: ::std::option::Option<Previews>,
        #[serde(
            rename = "pullRequestPreviewsEnabled",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub pull_request_previews_enabled: ::std::option::Option<PullRequestPreviewsEnabled>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub region: ::std::option::Option<Region>,
        #[serde(
            rename = "renderSubdomainPolicy",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub render_subdomain_policy: ::std::option::Option<RenderSubdomainPolicy>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub runtime: ::std::option::Option<ServiceRuntime>,
        #[serde(
            rename = "sshAddress",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub ssh_address: ::std::option::Option<SshAddress>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub url: ::std::option::Option<::std::string::String>,
    }

    impl ::std::default::Default for WebServiceDetails {
        fn default() -> Self {
            Self {
                autoscaling: Default::default(),
                build_plan: Default::default(),
                cache: Default::default(),
                disk: Default::default(),
                env: Default::default(),
                env_specific_details: Default::default(),
                health_check_path: Default::default(),
                ip_allow_list: Default::default(),
                maintenance_mode: Default::default(),
                max_shutdown_delay_seconds: Default::default(),
                num_instances: Default::default(),
                open_ports: Default::default(),
                parent_server: Default::default(),
                plan: Default::default(),
                previews: Default::default(),
                pull_request_previews_enabled: Default::default(),
                region: Default::default(),
                render_subdomain_policy: Default::default(),
                runtime: Default::default(),
                ssh_address: Default::default(),
                url: Default::default(),
            }
        }
    }

    ///`WebServiceDetailsAutoscaling`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "criteria": {
    ///      "type": "object",
    ///      "properties": {
    ///        "cpu": {
    ///          "type": "object",
    ///          "properties": {
    ///            "enabled": {
    ///              "default": false,
    ///              "type": "boolean"
    ///            },
    ///            "percentage": {
    ///              "description": "Determines when your service will be
    /// scaled. If the average resource utilization is significantly above/below
    /// the target, we will increase/decrease the number of instances.\n",
    ///              "type": "integer"
    ///            }
    ///          }
    ///        },
    ///        "memory": {
    ///          "type": "object",
    ///          "properties": {
    ///            "enabled": {
    ///              "default": false,
    ///              "type": "boolean"
    ///            },
    ///            "percentage": {
    ///              "description": "Determines when your service will be
    /// scaled. If the average resource utilization is significantly above/below
    /// the target, we will increase/decrease the number of instances.\n",
    ///              "type": "integer"
    ///            }
    ///          }
    ///        }
    ///      }
    ///    },
    ///    "enabled": {
    ///      "default": false,
    ///      "type": "boolean"
    ///    },
    ///    "max": {
    ///      "description": "The maximum number of instances for the service",
    ///      "type": "integer"
    ///    },
    ///    "min": {
    ///      "description": "The minimum number of instances for the service",
    ///      "type": "integer"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct WebServiceDetailsAutoscaling {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub criteria: ::std::option::Option<WebServiceDetailsAutoscalingCriteria>,
        #[serde(default)]
        pub enabled: bool,
        ///The maximum number of instances for the service
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub max: ::std::option::Option<i64>,
        ///The minimum number of instances for the service
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub min: ::std::option::Option<i64>,
    }

    impl ::std::default::Default for WebServiceDetailsAutoscaling {
        fn default() -> Self {
            Self {
                criteria: Default::default(),
                enabled: Default::default(),
                max: Default::default(),
                min: Default::default(),
            }
        }
    }

    ///`WebServiceDetailsAutoscalingCriteria`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "cpu": {
    ///      "type": "object",
    ///      "properties": {
    ///        "enabled": {
    ///          "default": false,
    ///          "type": "boolean"
    ///        },
    ///        "percentage": {
    ///          "description": "Determines when your service will be scaled. If
    /// the average resource utilization is significantly above/below the
    /// target, we will increase/decrease the number of instances.\n",
    ///          "type": "integer"
    ///        }
    ///      }
    ///    },
    ///    "memory": {
    ///      "type": "object",
    ///      "properties": {
    ///        "enabled": {
    ///          "default": false,
    ///          "type": "boolean"
    ///        },
    ///        "percentage": {
    ///          "description": "Determines when your service will be scaled. If
    /// the average resource utilization is significantly above/below the
    /// target, we will increase/decrease the number of instances.\n",
    ///          "type": "integer"
    ///        }
    ///      }
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct WebServiceDetailsAutoscalingCriteria {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub cpu: ::std::option::Option<WebServiceDetailsAutoscalingCriteriaCpu>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub memory: ::std::option::Option<WebServiceDetailsAutoscalingCriteriaMemory>,
    }

    impl ::std::default::Default for WebServiceDetailsAutoscalingCriteria {
        fn default() -> Self {
            Self {
                cpu: Default::default(),
                memory: Default::default(),
            }
        }
    }

    ///`WebServiceDetailsAutoscalingCriteriaCpu`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "enabled": {
    ///      "default": false,
    ///      "type": "boolean"
    ///    },
    ///    "percentage": {
    ///      "description": "Determines when your service will be scaled. If the
    /// average resource utilization is significantly above/below the target, we
    /// will increase/decrease the number of instances.\n",
    ///      "type": "integer"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct WebServiceDetailsAutoscalingCriteriaCpu {
        #[serde(default)]
        pub enabled: bool,
        ///Determines when your service will be scaled. If the average resource
        /// utilization is significantly above/below the target, we will
        /// increase/decrease the number of instances.
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub percentage: ::std::option::Option<i64>,
    }

    impl ::std::default::Default for WebServiceDetailsAutoscalingCriteriaCpu {
        fn default() -> Self {
            Self {
                enabled: Default::default(),
                percentage: Default::default(),
            }
        }
    }

    ///`WebServiceDetailsAutoscalingCriteriaMemory`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "enabled": {
    ///      "default": false,
    ///      "type": "boolean"
    ///    },
    ///    "percentage": {
    ///      "description": "Determines when your service will be scaled. If the
    /// average resource utilization is significantly above/below the target, we
    /// will increase/decrease the number of instances.\n",
    ///      "type": "integer"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct WebServiceDetailsAutoscalingCriteriaMemory {
        #[serde(default)]
        pub enabled: bool,
        ///Determines when your service will be scaled. If the average resource
        /// utilization is significantly above/below the target, we will
        /// increase/decrease the number of instances.
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub percentage: ::std::option::Option<i64>,
    }

    impl ::std::default::Default for WebServiceDetailsAutoscalingCriteriaMemory {
        fn default() -> Self {
            Self {
                enabled: Default::default(),
                percentage: Default::default(),
            }
        }
    }

    ///`WebServiceDetailsDisk`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "id": {
    ///      "examples": [
    ///        "dsk-cph1rs3idesc73a2b2mg"
    ///      ],
    ///      "type": "string",
    ///      "pattern": "^dsk-[0-9a-z]{20}$"
    ///    },
    ///    "mountPath": {
    ///      "type": "string"
    ///    },
    ///    "name": {
    ///      "type": "string"
    ///    },
    ///    "sizeGB": {
    ///      "type": "integer"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct WebServiceDetailsDisk {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub id: ::std::option::Option<WebServiceDetailsDiskId>,
        #[serde(
            rename = "mountPath",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub mount_path: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub name: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "sizeGB",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub size_gb: ::std::option::Option<i64>,
    }

    impl ::std::default::Default for WebServiceDetailsDisk {
        fn default() -> Self {
            Self {
                id: Default::default(),
                mount_path: Default::default(),
                name: Default::default(),
                size_gb: Default::default(),
            }
        }
    }

    ///`WebServiceDetailsDiskId`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "examples": [
    ///    "dsk-cph1rs3idesc73a2b2mg"
    ///  ],
    ///  "type": "string",
    ///  "pattern": "^dsk-[0-9a-z]{20}$"
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
    #[serde(transparent)]
    pub struct WebServiceDetailsDiskId(::std::string::String);
    impl ::std::ops::Deref for WebServiceDetailsDiskId {
        type Target = ::std::string::String;
        fn deref(&self) -> &::std::string::String {
            &self.0
        }
    }

    impl ::std::convert::From<WebServiceDetailsDiskId> for ::std::string::String {
        fn from(value: WebServiceDetailsDiskId) -> Self {
            value.0
        }
    }

    impl ::std::str::FromStr for WebServiceDetailsDiskId {
        type Err = self::error::ConversionError;
        fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
                ::std::sync::LazyLock::new(|| ::regress::Regex::new("^dsk-[0-9a-z]{20}$").unwrap());
            if PATTERN.find(value).is_none() {
                return Err("doesn't match pattern \"^dsk-[0-9a-z]{20}$\"".into());
            }
            Ok(Self(value.to_string()))
        }
    }

    impl ::std::convert::TryFrom<&str> for WebServiceDetailsDiskId {
        type Error = self::error::ConversionError;
        fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }

    impl ::std::convert::TryFrom<&::std::string::String> for WebServiceDetailsDiskId {
        type Error = self::error::ConversionError;
        fn try_from(
            value: &::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }

    impl ::std::convert::TryFrom<::std::string::String> for WebServiceDetailsDiskId {
        type Error = self::error::ConversionError;
        fn try_from(
            value: ::std::string::String,
        ) -> ::std::result::Result<Self, self::error::ConversionError> {
            value.parse()
        }
    }

    impl<'de> ::serde::Deserialize<'de> for WebServiceDetailsDiskId {
        fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
        where
            D: ::serde::Deserializer<'de>,
        {
            ::std::string::String::deserialize(deserializer)?
                .parse()
                .map_err(|e: self::error::ConversionError| {
                    <D::Error as ::serde::de::Error>::custom(e.to_string())
                })
        }
    }

    ///`WebServiceDetailsPost`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "autoscaling": {
    ///      "type": "object",
    ///      "properties": {
    ///        "criteria": {
    ///          "type": "object",
    ///          "properties": {
    ///            "cpu": {
    ///              "type": "object",
    ///              "properties": {
    ///                "enabled": {
    ///                  "default": false,
    ///                  "type": "boolean"
    ///                },
    ///                "percentage": {
    ///                  "description": "Determines when your service will be
    /// scaled. If the average resource utilization is significantly above/below
    /// the target, we will increase/decrease the number of instances.\n",
    ///                  "type": "integer"
    ///                }
    ///              }
    ///            },
    ///            "memory": {
    ///              "type": "object",
    ///              "properties": {
    ///                "enabled": {
    ///                  "default": false,
    ///                  "type": "boolean"
    ///                },
    ///                "percentage": {
    ///                  "description": "Determines when your service will be
    /// scaled. If the average resource utilization is significantly above/below
    /// the target, we will increase/decrease the number of instances.\n",
    ///                  "type": "integer"
    ///                }
    ///              }
    ///            }
    ///          }
    ///        },
    ///        "enabled": {
    ///          "default": false,
    ///          "type": "boolean"
    ///        },
    ///        "max": {
    ///          "description": "The maximum number of instances for the
    /// service",
    ///          "type": "integer"
    ///        },
    ///        "min": {
    ///          "description": "The minimum number of instances for the
    /// service",
    ///          "type": "integer"
    ///        }
    ///      }
    ///    },
    ///    "disk": {
    ///      "$ref": "#/components/schemas/serviceDisk"
    ///    },
    ///    "env": {
    ///      "$ref": "#/components/schemas/serviceEnv"
    ///    },
    ///    "envSpecificDetails": {
    ///      "$ref": "#/components/schemas/envSpecificDetailsPOST"
    ///    },
    ///    "healthCheckPath": {
    ///      "type": "string"
    ///    },
    ///    "ipAllowList": {
    ///      "type": "array",
    ///      "items": {
    ///        "$ref": "#/components/schemas/cidrBlockAndDescription"
    ///      }
    ///    },
    ///    "maintenanceMode": {
    ///      "$ref": "#/components/schemas/maintenanceMode"
    ///    },
    ///    "maxShutdownDelaySeconds": {
    ///      "$ref": "#/components/schemas/maxShutdownDelaySeconds"
    ///    },
    ///    "numInstances": {
    ///      "description": "Defaults to 1",
    ///      "type": "integer",
    ///      "minimum": 1.0
    ///    },
    ///    "plan": {
    ///      "$ref": "#/components/schemas/plan"
    ///    },
    ///    "preDeployCommand": {
    ///      "type": "string"
    ///    },
    ///    "previews": {
    ///      "$ref": "#/components/schemas/previews"
    ///    },
    ///    "pullRequestPreviewsEnabled": {
    ///      "$ref": "#/components/schemas/pullRequestPreviewsEnabled"
    ///    },
    ///    "region": {
    ///      "$ref": "#/components/schemas/region"
    ///    },
    ///    "renderSubdomainPolicy": {
    ///      "$ref": "#/components/schemas/renderSubdomainPolicy"
    ///    },
    ///    "runtime": {
    ///      "$ref": "#/components/schemas/serviceRuntime"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct WebServiceDetailsPost {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub autoscaling: ::std::option::Option<WebServiceDetailsPostAutoscaling>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub disk: ::std::option::Option<ServiceDisk>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub env: ::std::option::Option<ServiceEnv>,
        #[serde(
            rename = "envSpecificDetails",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub env_specific_details: ::std::option::Option<EnvSpecificDetailsPost>,
        #[serde(
            rename = "healthCheckPath",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub health_check_path: ::std::option::Option<::std::string::String>,
        #[serde(
            rename = "ipAllowList",
            default,
            skip_serializing_if = "::std::vec::Vec::is_empty"
        )]
        pub ip_allow_list: ::std::vec::Vec<CidrBlockAndDescription>,
        #[serde(
            rename = "maintenanceMode",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub maintenance_mode: ::std::option::Option<MaintenanceMode>,
        #[serde(
            rename = "maxShutdownDelaySeconds",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub max_shutdown_delay_seconds: ::std::option::Option<MaxShutdownDelaySeconds>,
        ///Defaults to 1
        #[serde(
            rename = "numInstances",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub num_instances: ::std::option::Option<::std::num::NonZeroU64>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub plan: ::std::option::Option<Plan>,
        #[serde(
            rename = "preDeployCommand",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub pre_deploy_command: ::std::option::Option<::std::string::String>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub previews: ::std::option::Option<Previews>,
        #[serde(
            rename = "pullRequestPreviewsEnabled",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub pull_request_previews_enabled: ::std::option::Option<PullRequestPreviewsEnabled>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub region: ::std::option::Option<Region>,
        #[serde(
            rename = "renderSubdomainPolicy",
            default,
            skip_serializing_if = "::std::option::Option::is_none"
        )]
        pub render_subdomain_policy: ::std::option::Option<RenderSubdomainPolicy>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub runtime: ::std::option::Option<ServiceRuntime>,
    }

    impl ::std::default::Default for WebServiceDetailsPost {
        fn default() -> Self {
            Self {
                autoscaling: Default::default(),
                disk: Default::default(),
                env: Default::default(),
                env_specific_details: Default::default(),
                health_check_path: Default::default(),
                ip_allow_list: Default::default(),
                maintenance_mode: Default::default(),
                max_shutdown_delay_seconds: Default::default(),
                num_instances: Default::default(),
                plan: Default::default(),
                pre_deploy_command: Default::default(),
                previews: Default::default(),
                pull_request_previews_enabled: Default::default(),
                region: Default::default(),
                render_subdomain_policy: Default::default(),
                runtime: Default::default(),
            }
        }
    }

    ///`WebServiceDetailsPostAutoscaling`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "criteria": {
    ///      "type": "object",
    ///      "properties": {
    ///        "cpu": {
    ///          "type": "object",
    ///          "properties": {
    ///            "enabled": {
    ///              "default": false,
    ///              "type": "boolean"
    ///            },
    ///            "percentage": {
    ///              "description": "Determines when your service will be
    /// scaled. If the average resource utilization is significantly above/below
    /// the target, we will increase/decrease the number of instances.\n",
    ///              "type": "integer"
    ///            }
    ///          }
    ///        },
    ///        "memory": {
    ///          "type": "object",
    ///          "properties": {
    ///            "enabled": {
    ///              "default": false,
    ///              "type": "boolean"
    ///            },
    ///            "percentage": {
    ///              "description": "Determines when your service will be
    /// scaled. If the average resource utilization is significantly above/below
    /// the target, we will increase/decrease the number of instances.\n",
    ///              "type": "integer"
    ///            }
    ///          }
    ///        }
    ///      }
    ///    },
    ///    "enabled": {
    ///      "default": false,
    ///      "type": "boolean"
    ///    },
    ///    "max": {
    ///      "description": "The maximum number of instances for the service",
    ///      "type": "integer"
    ///    },
    ///    "min": {
    ///      "description": "The minimum number of instances for the service",
    ///      "type": "integer"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct WebServiceDetailsPostAutoscaling {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub criteria: ::std::option::Option<WebServiceDetailsPostAutoscalingCriteria>,
        #[serde(default)]
        pub enabled: bool,
        ///The maximum number of instances for the service
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub max: ::std::option::Option<i64>,
        ///The minimum number of instances for the service
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub min: ::std::option::Option<i64>,
    }

    impl ::std::default::Default for WebServiceDetailsPostAutoscaling {
        fn default() -> Self {
            Self {
                criteria: Default::default(),
                enabled: Default::default(),
                max: Default::default(),
                min: Default::default(),
            }
        }
    }

    ///`WebServiceDetailsPostAutoscalingCriteria`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "cpu": {
    ///      "type": "object",
    ///      "properties": {
    ///        "enabled": {
    ///          "default": false,
    ///          "type": "boolean"
    ///        },
    ///        "percentage": {
    ///          "description": "Determines when your service will be scaled. If
    /// the average resource utilization is significantly above/below the
    /// target, we will increase/decrease the number of instances.\n",
    ///          "type": "integer"
    ///        }
    ///      }
    ///    },
    ///    "memory": {
    ///      "type": "object",
    ///      "properties": {
    ///        "enabled": {
    ///          "default": false,
    ///          "type": "boolean"
    ///        },
    ///        "percentage": {
    ///          "description": "Determines when your service will be scaled. If
    /// the average resource utilization is significantly above/below the
    /// target, we will increase/decrease the number of instances.\n",
    ///          "type": "integer"
    ///        }
    ///      }
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct WebServiceDetailsPostAutoscalingCriteria {
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub cpu: ::std::option::Option<WebServiceDetailsPostAutoscalingCriteriaCpu>,
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub memory: ::std::option::Option<WebServiceDetailsPostAutoscalingCriteriaMemory>,
    }

    impl ::std::default::Default for WebServiceDetailsPostAutoscalingCriteria {
        fn default() -> Self {
            Self {
                cpu: Default::default(),
                memory: Default::default(),
            }
        }
    }

    ///`WebServiceDetailsPostAutoscalingCriteriaCpu`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "enabled": {
    ///      "default": false,
    ///      "type": "boolean"
    ///    },
    ///    "percentage": {
    ///      "description": "Determines when your service will be scaled. If the
    /// average resource utilization is significantly above/below the target, we
    /// will increase/decrease the number of instances.\n",
    ///      "type": "integer"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct WebServiceDetailsPostAutoscalingCriteriaCpu {
        #[serde(default)]
        pub enabled: bool,
        ///Determines when your service will be scaled. If the average resource
        /// utilization is significantly above/below the target, we will
        /// increase/decrease the number of instances.
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub percentage: ::std::option::Option<i64>,
    }

    impl ::std::default::Default for WebServiceDetailsPostAutoscalingCriteriaCpu {
        fn default() -> Self {
            Self {
                enabled: Default::default(),
                percentage: Default::default(),
            }
        }
    }

    ///`WebServiceDetailsPostAutoscalingCriteriaMemory`
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "type": "object",
    ///  "properties": {
    ///    "enabled": {
    ///      "default": false,
    ///      "type": "boolean"
    ///    },
    ///    "percentage": {
    ///      "description": "Determines when your service will be scaled. If the
    /// average resource utilization is significantly above/below the target, we
    /// will increase/decrease the number of instances.\n",
    ///      "type": "integer"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
    pub struct WebServiceDetailsPostAutoscalingCriteriaMemory {
        #[serde(default)]
        pub enabled: bool,
        ///Determines when your service will be scaled. If the average resource
        /// utilization is significantly above/below the target, we will
        /// increase/decrease the number of instances.
        #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
        pub percentage: ::std::option::Option<i64>,
    }

    impl ::std::default::Default for WebServiceDetailsPostAutoscalingCriteriaMemory {
        fn default() -> Self {
            Self {
                enabled: Default::default(),
                percentage: Default::default(),
            }
        }
    }

    /// Generation of default values for serde.
    pub mod defaults {
        pub(super) fn default_nzu64<T, const V: u64>() -> T
        where
            T: ::std::convert::TryFrom<::std::num::NonZeroU64>,
            <T as ::std::convert::TryFrom<::std::num::NonZeroU64>>::Error: ::std::fmt::Debug,
        {
            T::try_from(::std::num::NonZeroU64::try_from(V).unwrap()).unwrap()
        }

        pub(super) fn cache_profile() -> ::std::string::String {
            "no-cache".to_string()
        }

        pub(super) fn create_deploy_body_clear_cache() -> ::std::string::String {
            "do_not_clear".to_string()
        }

        pub(super) fn postgres_post_input_database_name() -> ::std::string::String {
            "randomly generated".to_string()
        }

        pub(super) fn postgres_post_input_database_user() -> ::std::string::String {
            "randomly generated".to_string()
        }

        pub(super) fn previews_generation() -> ::std::string::String {
            "off".to_string()
        }
    }
}

#[derive(Clone, Debug)]
///Client for Render Public API
///
///Manage everything about your Render services
///
///Version: 1.0.0
pub struct Client {
    pub(crate) baseurl: String,
    pub(crate) client: reqwest::Client,
}

impl Client {
    /// Create a new client.
    ///
    /// `baseurl` is the base URL provided to the internal
    /// `reqwest::Client`, and should include a scheme and hostname,
    /// as well as port and a path stem if applicable.
    pub fn new(baseurl: &str) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let client = {
            let dur = ::std::time::Duration::from_secs(15u64);
            reqwest::ClientBuilder::new()
                .connect_timeout(dur)
                .timeout(dur)
        };
        #[cfg(target_arch = "wasm32")]
        let client = reqwest::ClientBuilder::new();
        Self::new_with_client(baseurl, client.build().unwrap())
    }

    /// Construct a new client with an existing `reqwest::Client`,
    /// allowing more control over its configuration.
    ///
    /// `baseurl` is the base URL provided to the internal
    /// `reqwest::Client`, and should include a scheme and hostname,
    /// as well as port and a path stem if applicable.
    pub fn new_with_client(baseurl: &str, client: reqwest::Client) -> Self {
        Self {
            baseurl: baseurl.to_string(),
            client,
        }
    }
}

impl ClientInfo<()> for Client {
    fn api_version() -> &'static str {
        "1.0.0"
    }

    fn baseurl(&self) -> &str {
        self.baseurl.as_str()
    }

    fn client(&self) -> &reqwest::Client {
        &self.client
    }

    fn inner(&self) -> &() {
        &()
    }
}

impl ClientHooks<()> for &Client {}
#[allow(clippy::all)]
impl Client {
    ///List services
    ///
    ///List services matching the provided filters. If no filters are provided,
    /// returns all services you have permissions to view.
    ///
    ///
    ///Sends a `GET` request to `/services`
    ///
    ///Arguments:
    /// - `created_after`: Filter for resources created after a certain time
    ///   (specified as an ISO 8601 timestamp)
    /// - `created_before`: Filter for resources created before a certain time
    ///   (specified as an ISO 8601 timestamp)
    /// - `cursor`: The position in the result list to start from when fetching paginated results. For details, see [Pagination](https://api-docs.render.com/reference/pagination).
    /// - `env`: Filter for environments (runtimes) of services (deprecated; use
    ///   `runtime` instead)
    /// - `environment_id`: Filter for resources that belong to an environment
    /// - `include_previews`: Include previews in the response
    /// - `limit`: The maximum number of items to return. For details, see [Pagination](https://api-docs.render.com/reference/pagination).
    /// - `name`: Filter by name
    /// - `owner_id`: The ID of the workspaces to return resources for
    /// - `region`: Filter by resource region
    /// - `suspended`: Filter resources based on whether they're suspended or
    ///   not suspended
    /// - `type_`: Filter for types of services
    /// - `updated_after`: Filter for resources updated after a certain time
    ///   (specified as an ISO 8601 timestamp)
    /// - `updated_before`: Filter for resources updated before a certain time
    ///   (specified as an ISO 8601 timestamp)
    pub async fn list_services<'a>(
        &'a self,
        created_after: Option<&'a ::chrono::DateTime<::chrono::offset::Utc>>,
        created_before: Option<&'a ::chrono::DateTime<::chrono::offset::Utc>>,
        cursor: Option<&'a str>,
        env: Option<&'a ::std::vec::Vec<types::ServiceRuntime>>,
        environment_id: Option<&'a ::std::vec::Vec<::std::string::String>>,
        include_previews: Option<bool>,
        limit: Option<::std::num::NonZeroU64>,
        name: Option<&'a ::std::vec::Vec<::std::string::String>>,
        owner_id: Option<&'a ::std::vec::Vec<::std::string::String>>,
        region: Option<&'a ::std::vec::Vec<types::Region>>,
        suspended: Option<&'a ::std::vec::Vec<::std::string::String>>,
        type_: Option<&'a ::std::vec::Vec<types::ServiceType>>,
        updated_after: Option<&'a ::chrono::DateTime<::chrono::offset::Utc>>,
        updated_before: Option<&'a ::chrono::DateTime<::chrono::offset::Utc>>,
    ) -> Result<ResponseValue<types::ServiceList>, Error<types::Error>> {
        let url = format!("{}/services", self.baseurl,);
        let mut header_map = ::reqwest::header::HeaderMap::with_capacity(1usize);
        header_map.append(
            ::reqwest::header::HeaderName::from_static("api-version"),
            ::reqwest::header::HeaderValue::from_static(Self::api_version()),
        );
        #[allow(unused_mut)]
        let mut request = self
            .client
            .get(url)
            .header(
                ::reqwest::header::ACCEPT,
                ::reqwest::header::HeaderValue::from_static("application/json"),
            )
            .query(&progenitor_client::QueryParam::new(
                "createdAfter",
                &created_after,
            ))
            .query(&progenitor_client::QueryParam::new(
                "createdBefore",
                &created_before,
            ))
            .query(&progenitor_client::QueryParam::new("cursor", &cursor))
            .query(&progenitor_client::QueryParam::new("env", &env))
            .query(&progenitor_client::QueryParam::new(
                "environmentId",
                &environment_id,
            ))
            .query(&progenitor_client::QueryParam::new(
                "includePreviews",
                &include_previews,
            ))
            .query(&progenitor_client::QueryParam::new("limit", &limit))
            .query(&progenitor_client::QueryParam::new("name", &name))
            .query(&progenitor_client::QueryParam::new("ownerId", &owner_id))
            .query(&progenitor_client::QueryParam::new("region", &region))
            .query(&progenitor_client::QueryParam::new("suspended", &suspended))
            .query(&progenitor_client::QueryParam::new("type", &type_))
            .query(&progenitor_client::QueryParam::new(
                "updatedAfter",
                &updated_after,
            ))
            .query(&progenitor_client::QueryParam::new(
                "updatedBefore",
                &updated_before,
            ))
            .headers(header_map)
            .build()?;
        let info = OperationInfo {
            operation_id: "list_services",
        };
        self.pre(&mut request, &info).await?;
        let result = self.exec(request, &info).await;
        self.post(&result, &info).await?;
        let response = result?;
        match response.status().as_u16() {
            200u16 => ResponseValue::from_response(response).await,
            401u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            406u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            429u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            500u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            503u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            _ => Err(Error::UnexpectedResponse(response)),
        }
    }

    ///Create service
    ///
    ///Creates a new Render service in the specified workspace with the
    /// specified configuration.
    ///
    ///
    ///Sends a `POST` request to `/services`
    pub async fn create_service<'a>(
        &'a self,
        body: &'a types::ServicePost,
    ) -> Result<ResponseValue<types::ServiceAndDeploy>, Error<types::Error>> {
        let url = format!("{}/services", self.baseurl,);
        let mut header_map = ::reqwest::header::HeaderMap::with_capacity(1usize);
        header_map.append(
            ::reqwest::header::HeaderName::from_static("api-version"),
            ::reqwest::header::HeaderValue::from_static(Self::api_version()),
        );
        #[allow(unused_mut)]
        let mut request = self
            .client
            .post(url)
            .header(
                ::reqwest::header::ACCEPT,
                ::reqwest::header::HeaderValue::from_static("application/json"),
            )
            .json(&body)
            .headers(header_map)
            .build()?;
        let info = OperationInfo {
            operation_id: "create_service",
        };
        self.pre(&mut request, &info).await?;
        let result = self.exec(request, &info).await;
        self.post(&result, &info).await?;
        let response = result?;
        match response.status().as_u16() {
            201u16 => ResponseValue::from_response(response).await,
            400u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            401u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            402u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            404u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            406u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            409u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            429u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            500u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            503u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            _ => Err(Error::UnexpectedResponse(response)),
        }
    }

    ///List Postgres instances
    ///
    ///List Postgres instances matching the provided filters. If no filters are
    /// provided, all Postgres instances are returned.
    ///
    ///
    ///Sends a `GET` request to `/postgres`
    ///
    ///Arguments:
    /// - `created_after`: Filter for resources created after a certain time
    ///   (specified as an ISO 8601 timestamp)
    /// - `created_before`: Filter for resources created before a certain time
    ///   (specified as an ISO 8601 timestamp)
    /// - `cursor`: The position in the result list to start from when fetching paginated results. For details, see [Pagination](https://api-docs.render.com/reference/pagination).
    /// - `environment_id`: Filter for resources that belong to an environment
    /// - `include_replicas`: Include replicas in the response
    /// - `limit`: The maximum number of items to return. For details, see [Pagination](https://api-docs.render.com/reference/pagination).
    /// - `name`: Filter by name
    /// - `owner_id`: The ID of the workspaces to return resources for
    /// - `region`: Filter by resource region
    /// - `suspended`: Filter resources based on whether they're suspended or
    ///   not suspended
    /// - `updated_after`: Filter for resources updated after a certain time
    ///   (specified as an ISO 8601 timestamp)
    /// - `updated_before`: Filter for resources updated before a certain time
    ///   (specified as an ISO 8601 timestamp)
    pub async fn list_postgres<'a>(
        &'a self,
        created_after: Option<&'a ::chrono::DateTime<::chrono::offset::Utc>>,
        created_before: Option<&'a ::chrono::DateTime<::chrono::offset::Utc>>,
        cursor: Option<&'a str>,
        environment_id: Option<&'a ::std::vec::Vec<::std::string::String>>,
        include_replicas: Option<bool>,
        limit: Option<::std::num::NonZeroU64>,
        name: Option<&'a ::std::vec::Vec<::std::string::String>>,
        owner_id: Option<&'a ::std::vec::Vec<::std::string::String>>,
        region: Option<&'a ::std::vec::Vec<types::Region>>,
        suspended: Option<&'a ::std::vec::Vec<::std::string::String>>,
        updated_after: Option<&'a ::chrono::DateTime<::chrono::offset::Utc>>,
        updated_before: Option<&'a ::chrono::DateTime<::chrono::offset::Utc>>,
    ) -> Result<ResponseValue<::std::vec::Vec<types::PostgresWithCursor>>, Error<types::Error>>
    {
        let url = format!("{}/postgres", self.baseurl,);
        let mut header_map = ::reqwest::header::HeaderMap::with_capacity(1usize);
        header_map.append(
            ::reqwest::header::HeaderName::from_static("api-version"),
            ::reqwest::header::HeaderValue::from_static(Self::api_version()),
        );
        #[allow(unused_mut)]
        let mut request = self
            .client
            .get(url)
            .header(
                ::reqwest::header::ACCEPT,
                ::reqwest::header::HeaderValue::from_static("application/json"),
            )
            .query(&progenitor_client::QueryParam::new(
                "createdAfter",
                &created_after,
            ))
            .query(&progenitor_client::QueryParam::new(
                "createdBefore",
                &created_before,
            ))
            .query(&progenitor_client::QueryParam::new("cursor", &cursor))
            .query(&progenitor_client::QueryParam::new(
                "environmentId",
                &environment_id,
            ))
            .query(&progenitor_client::QueryParam::new(
                "includeReplicas",
                &include_replicas,
            ))
            .query(&progenitor_client::QueryParam::new("limit", &limit))
            .query(&progenitor_client::QueryParam::new("name", &name))
            .query(&progenitor_client::QueryParam::new("ownerId", &owner_id))
            .query(&progenitor_client::QueryParam::new("region", &region))
            .query(&progenitor_client::QueryParam::new("suspended", &suspended))
            .query(&progenitor_client::QueryParam::new(
                "updatedAfter",
                &updated_after,
            ))
            .query(&progenitor_client::QueryParam::new(
                "updatedBefore",
                &updated_before,
            ))
            .headers(header_map)
            .build()?;
        let info = OperationInfo {
            operation_id: "list_postgres",
        };
        self.pre(&mut request, &info).await?;
        let result = self.exec(request, &info).await;
        self.post(&result, &info).await?;
        let response = result?;
        match response.status().as_u16() {
            200u16 => ResponseValue::from_response(response).await,
            400u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            401u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            404u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            409u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            429u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            500u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            503u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            _ => Err(Error::UnexpectedResponse(response)),
        }
    }

    ///Create Postgres instance
    ///
    ///Create a new Postgres instance.
    ///
    ///
    ///Sends a `POST` request to `/postgres`
    pub async fn create_postgres<'a>(
        &'a self,
        body: &'a types::PostgresPostInput,
    ) -> Result<ResponseValue<types::PostgresDetail>, Error<types::Error>> {
        let url = format!("{}/postgres", self.baseurl,);
        let mut header_map = ::reqwest::header::HeaderMap::with_capacity(1usize);
        header_map.append(
            ::reqwest::header::HeaderName::from_static("api-version"),
            ::reqwest::header::HeaderValue::from_static(Self::api_version()),
        );
        #[allow(unused_mut)]
        let mut request = self
            .client
            .post(url)
            .header(
                ::reqwest::header::ACCEPT,
                ::reqwest::header::HeaderValue::from_static("application/json"),
            )
            .json(&body)
            .headers(header_map)
            .build()?;
        let info = OperationInfo {
            operation_id: "create_postgres",
        };
        self.pre(&mut request, &info).await?;
        let result = self.exec(request, &info).await;
        self.post(&result, &info).await?;
        let response = result?;
        match response.status().as_u16() {
            201u16 => ResponseValue::from_response(response).await,
            400u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            401u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            404u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            429u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            500u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            503u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            _ => Err(Error::UnexpectedResponse(response)),
        }
    }

    ///Retrieve Postgres connection info
    ///
    ///Retrieve connection info for a Postgres instance by ID. Connection info
    /// includes sensitive information.
    ///
    ///
    ///Sends a `GET` request to `/postgres/{postgresId}/connection-info`
    pub async fn retrieve_postgres_connection_info<'a>(
        &'a self,
        postgres_id: &'a str,
    ) -> Result<ResponseValue<types::PostgresConnectionInfo>, Error<types::Error>> {
        let url = format!(
            "{}/postgres/{}/connection-info",
            self.baseurl,
            encode_path(&postgres_id.to_string()),
        );
        let mut header_map = ::reqwest::header::HeaderMap::with_capacity(1usize);
        header_map.append(
            ::reqwest::header::HeaderName::from_static("api-version"),
            ::reqwest::header::HeaderValue::from_static(Self::api_version()),
        );
        #[allow(unused_mut)]
        let mut request = self
            .client
            .get(url)
            .header(
                ::reqwest::header::ACCEPT,
                ::reqwest::header::HeaderValue::from_static("application/json"),
            )
            .headers(header_map)
            .build()?;
        let info = OperationInfo {
            operation_id: "retrieve_postgres_connection_info",
        };
        self.pre(&mut request, &info).await?;
        let result = self.exec(request, &info).await;
        self.post(&result, &info).await?;
        let response = result?;
        match response.status().as_u16() {
            200u16 => ResponseValue::from_response(response).await,
            400u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            401u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            404u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            429u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            500u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            503u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            _ => Err(Error::UnexpectedResponse(response)),
        }
    }

    ///List environment variables
    ///
    ///List all environment variables for the service with the provided ID.
    ///
    ///
    ///Sends a `GET` request to `/services/{serviceId}/env-vars`
    ///
    ///Arguments:
    /// - `service_id`: The ID of the service
    /// - `cursor`: The position in the result list to start from when fetching paginated results. For details, see [Pagination](https://api-docs.render.com/reference/pagination).
    /// - `limit`: The maximum number of items to return. For details, see [Pagination](https://api-docs.render.com/reference/pagination).
    pub async fn get_env_vars_for_service<'a>(
        &'a self,
        service_id: &'a str,
        cursor: Option<&'a str>,
        limit: Option<::std::num::NonZeroU64>,
    ) -> Result<ResponseValue<::std::vec::Vec<types::EnvVarWithCursor>>, Error<types::Error>> {
        let url = format!(
            "{}/services/{}/env-vars",
            self.baseurl,
            encode_path(&service_id.to_string()),
        );
        let mut header_map = ::reqwest::header::HeaderMap::with_capacity(1usize);
        header_map.append(
            ::reqwest::header::HeaderName::from_static("api-version"),
            ::reqwest::header::HeaderValue::from_static(Self::api_version()),
        );
        #[allow(unused_mut)]
        let mut request = self
            .client
            .get(url)
            .header(
                ::reqwest::header::ACCEPT,
                ::reqwest::header::HeaderValue::from_static("application/json"),
            )
            .query(&progenitor_client::QueryParam::new("cursor", &cursor))
            .query(&progenitor_client::QueryParam::new("limit", &limit))
            .headers(header_map)
            .build()?;
        let info = OperationInfo {
            operation_id: "get_env_vars_for_service",
        };
        self.pre(&mut request, &info).await?;
        let result = self.exec(request, &info).await;
        self.post(&result, &info).await?;
        let response = result?;
        match response.status().as_u16() {
            200u16 => ResponseValue::from_response(response).await,
            401u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            403u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            404u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            406u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            410u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            429u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            500u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            503u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            _ => Err(Error::UnexpectedResponse(response)),
        }
    }

    ///Update environment variables
    ///
    ///Replace all environment variables for a service with the provided list
    /// of environment variables.
    ///
    ///Sends a `PUT` request to `/services/{serviceId}/env-vars`
    ///
    ///Arguments:
    /// - `service_id`: The ID of the service
    /// - `body`
    pub async fn update_env_vars_for_service<'a>(
        &'a self,
        service_id: &'a str,
        body: &'a ::std::vec::Vec<types::UpdateEnvVarsForServiceBodyItem>,
    ) -> Result<ResponseValue<::std::vec::Vec<types::EnvVarWithCursor>>, Error<types::Error>> {
        let url = format!(
            "{}/services/{}/env-vars",
            self.baseurl,
            encode_path(&service_id.to_string()),
        );
        let mut header_map = ::reqwest::header::HeaderMap::with_capacity(1usize);
        header_map.append(
            ::reqwest::header::HeaderName::from_static("api-version"),
            ::reqwest::header::HeaderValue::from_static(Self::api_version()),
        );
        #[allow(unused_mut)]
        let mut request = self
            .client
            .put(url)
            .header(
                ::reqwest::header::ACCEPT,
                ::reqwest::header::HeaderValue::from_static("application/json"),
            )
            .json(&body)
            .headers(header_map)
            .build()?;
        let info = OperationInfo {
            operation_id: "update_env_vars_for_service",
        };
        self.pre(&mut request, &info).await?;
        let result = self.exec(request, &info).await;
        self.post(&result, &info).await?;
        let response = result?;
        match response.status().as_u16() {
            200u16 => ResponseValue::from_response(response).await,
            400u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            401u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            403u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            404u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            406u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            410u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            429u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            500u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            503u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            _ => Err(Error::UnexpectedResponse(response)),
        }
    }

    ///List redirect/rewrite rules
    ///
    ///List a particular service's redirect/rewrite rules that match the
    /// provided filters. If no filters are provided, all rules for the service
    /// are returned.
    ///
    ///
    ///Sends a `GET` request to `/services/{serviceId}/routes`
    ///
    ///Arguments:
    /// - `service_id`: The ID of the service
    /// - `cursor`: The position in the result list to start from when fetching paginated results. For details, see [Pagination](https://api-docs.render.com/reference/pagination).
    /// - `destination`: Filter for the destination path of the route
    /// - `limit`: The maximum number of items to return. For details, see [Pagination](https://api-docs.render.com/reference/pagination).
    /// - `source`: Filter for the source path of the route
    /// - `type_`: Filter for the type of route rule
    pub async fn list_routes<'a>(
        &'a self,
        service_id: &'a str,
        cursor: Option<&'a str>,
        destination: Option<&'a ::std::vec::Vec<::std::string::String>>,
        limit: Option<::std::num::NonZeroU64>,
        source: Option<&'a ::std::vec::Vec<::std::string::String>>,
        type_: Option<&'a ::std::vec::Vec<::std::string::String>>,
    ) -> Result<ResponseValue<::std::vec::Vec<types::RouteWithCursor>>, Error<types::Error>> {
        let url = format!(
            "{}/services/{}/routes",
            self.baseurl,
            encode_path(&service_id.to_string()),
        );
        let mut header_map = ::reqwest::header::HeaderMap::with_capacity(1usize);
        header_map.append(
            ::reqwest::header::HeaderName::from_static("api-version"),
            ::reqwest::header::HeaderValue::from_static(Self::api_version()),
        );
        #[allow(unused_mut)]
        let mut request = self
            .client
            .get(url)
            .header(
                ::reqwest::header::ACCEPT,
                ::reqwest::header::HeaderValue::from_static("application/json"),
            )
            .query(&progenitor_client::QueryParam::new("cursor", &cursor))
            .query(&progenitor_client::QueryParam::new(
                "destination",
                &destination,
            ))
            .query(&progenitor_client::QueryParam::new("limit", &limit))
            .query(&progenitor_client::QueryParam::new("source", &source))
            .query(&progenitor_client::QueryParam::new("type", &type_))
            .headers(header_map)
            .build()?;
        let info = OperationInfo {
            operation_id: "list_routes",
        };
        self.pre(&mut request, &info).await?;
        let result = self.exec(request, &info).await;
        self.post(&result, &info).await?;
        let response = result?;
        match response.status().as_u16() {
            200u16 => ResponseValue::from_response(response).await,
            401u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            403u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            404u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            406u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            410u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            429u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            500u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            503u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            _ => Err(Error::UnexpectedResponse(response)),
        }
    }

    ///Update redirect/rewrite rules
    ///
    ///Replace all redirect/rewrite rules for a particular service with the
    /// provided list.
    ///
    ///**This deletes all existing redirect/rewrite rules for the service that
    /// aren't included in the request.**
    ///
    ///Rule priority is assigned according to list order (the first rule in the
    /// list has the highest priority).
    ///
    ///
    ///Sends a `PUT` request to `/services/{serviceId}/routes`
    ///
    ///Arguments:
    /// - `service_id`: The ID of the service
    /// - `body`
    pub async fn put_routes<'a>(
        &'a self,
        service_id: &'a str,
        body: &'a ::std::vec::Vec<types::RoutePut>,
    ) -> Result<ResponseValue<::std::vec::Vec<types::Route>>, Error<types::Error>> {
        let url = format!(
            "{}/services/{}/routes",
            self.baseurl,
            encode_path(&service_id.to_string()),
        );
        let mut header_map = ::reqwest::header::HeaderMap::with_capacity(1usize);
        header_map.append(
            ::reqwest::header::HeaderName::from_static("api-version"),
            ::reqwest::header::HeaderValue::from_static(Self::api_version()),
        );
        #[allow(unused_mut)]
        let mut request = self
            .client
            .put(url)
            .header(
                ::reqwest::header::ACCEPT,
                ::reqwest::header::HeaderValue::from_static("application/json"),
            )
            .json(&body)
            .headers(header_map)
            .build()?;
        let info = OperationInfo {
            operation_id: "put_routes",
        };
        self.pre(&mut request, &info).await?;
        let result = self.exec(request, &info).await;
        self.post(&result, &info).await?;
        let response = result?;
        match response.status().as_u16() {
            200u16 => ResponseValue::from_response(response).await,
            400u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            401u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            403u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            404u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            406u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            410u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            429u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            500u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            503u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            _ => Err(Error::UnexpectedResponse(response)),
        }
    }

    ///Add redirect/rewrite rules
    ///
    ///Add redirect/rewrite rules to the service with the provided ID.
    ///
    ///
    ///Sends a `POST` request to `/services/{serviceId}/routes`
    ///
    ///Arguments:
    /// - `service_id`: The ID of the service
    /// - `body`
    pub async fn add_route<'a>(
        &'a self,
        service_id: &'a str,
        body: &'a types::RoutePost,
    ) -> Result<ResponseValue<types::Route>, Error<types::Error>> {
        let url = format!(
            "{}/services/{}/routes",
            self.baseurl,
            encode_path(&service_id.to_string()),
        );
        let mut header_map = ::reqwest::header::HeaderMap::with_capacity(1usize);
        header_map.append(
            ::reqwest::header::HeaderName::from_static("api-version"),
            ::reqwest::header::HeaderValue::from_static(Self::api_version()),
        );
        #[allow(unused_mut)]
        let mut request = self
            .client
            .post(url)
            .header(
                ::reqwest::header::ACCEPT,
                ::reqwest::header::HeaderValue::from_static("application/json"),
            )
            .json(&body)
            .headers(header_map)
            .build()?;
        let info = OperationInfo {
            operation_id: "add_route",
        };
        self.pre(&mut request, &info).await?;
        let result = self.exec(request, &info).await;
        self.post(&result, &info).await?;
        let response = result?;
        match response.status().as_u16() {
            201u16 => ResponseValue::from_response(response).await,
            400u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            401u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            403u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            404u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            406u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            410u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            429u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            500u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            503u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            _ => Err(Error::UnexpectedResponse(response)),
        }
    }

    ///Update redirect/rewrite rule priority
    ///
    ///Update the priority for a particular redirect/rewrite rule.
    ///
    ///To apply redirect/rewrite rules to an incoming request, Render starts
    /// from the rule with priority `0` and applies the first encountered rule
    /// that matches the request's path (if any).
    ///
    ///Render increments the priority of other rules by `1` as necessary to
    /// make space for the updated rule.
    ///
    ///
    ///Sends a `PATCH` request to `/services/{serviceId}/routes`
    ///
    ///Arguments:
    /// - `service_id`: The ID of the service
    /// - `body`
    pub async fn patch_route<'a>(
        &'a self,
        service_id: &'a str,
        body: &'a types::RoutePatch,
    ) -> Result<ResponseValue<types::PatchRouteResponse>, Error<types::Error>> {
        let url = format!(
            "{}/services/{}/routes",
            self.baseurl,
            encode_path(&service_id.to_string()),
        );
        let mut header_map = ::reqwest::header::HeaderMap::with_capacity(1usize);
        header_map.append(
            ::reqwest::header::HeaderName::from_static("api-version"),
            ::reqwest::header::HeaderValue::from_static(Self::api_version()),
        );
        #[allow(unused_mut)]
        let mut request = self
            .client
            .patch(url)
            .header(
                ::reqwest::header::ACCEPT,
                ::reqwest::header::HeaderValue::from_static("application/json"),
            )
            .json(&body)
            .headers(header_map)
            .build()?;
        let info = OperationInfo {
            operation_id: "patch_route",
        };
        self.pre(&mut request, &info).await?;
        let result = self.exec(request, &info).await;
        self.post(&result, &info).await?;
        let response = result?;
        match response.status().as_u16() {
            200u16 => ResponseValue::from_response(response).await,
            401u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            403u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            404u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            406u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            410u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            429u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            500u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            503u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            _ => Err(Error::UnexpectedResponse(response)),
        }
    }

    ///List deploys
    ///
    ///List deploys matching the provided filters. If no filters are provided,
    /// all deploys for the service are returned.
    ///
    ///
    ///Sends a `GET` request to `/services/{serviceId}/deploys`
    ///
    ///Arguments:
    /// - `service_id`: The ID of the service
    /// - `created_after`: Filter for deploys created after a certain time
    ///   (specified as an ISO 8601 timestamp)
    /// - `created_before`: Filter for deploys created before a certain time
    ///   (specified as an ISO 8601 timestamp)
    /// - `cursor`: The position in the result list to start from when fetching paginated results. For details, see [Pagination](https://api-docs.render.com/reference/pagination).
    /// - `finished_after`: Filter for deploys finished after a certain time
    ///   (specified as an ISO 8601 timestamp)
    /// - `finished_before`: Filter for deploys finished before a certain time
    ///   (specified as an ISO 8601 timestamp)
    /// - `limit`: The maximum number of items to return. For details, see [Pagination](https://api-docs.render.com/reference/pagination).
    /// - `status`: Filter for deploys with the specified statuses
    /// - `updated_after`: Filter for deploys updated after a certain time
    ///   (specified as an ISO 8601 timestamp)
    /// - `updated_before`: Filter for deploys updated before a certain time
    ///   (specified as an ISO 8601 timestamp)
    pub async fn list_deploys<'a>(
        &'a self,
        service_id: &'a str,
        created_after: Option<&'a ::chrono::DateTime<::chrono::offset::Utc>>,
        created_before: Option<&'a ::chrono::DateTime<::chrono::offset::Utc>>,
        cursor: Option<&'a str>,
        finished_after: Option<&'a ::chrono::DateTime<::chrono::offset::Utc>>,
        finished_before: Option<&'a ::chrono::DateTime<::chrono::offset::Utc>>,
        limit: Option<::std::num::NonZeroU64>,
        status: Option<&'a ::std::vec::Vec<types::DeployStatus>>,
        updated_after: Option<&'a ::chrono::DateTime<::chrono::offset::Utc>>,
        updated_before: Option<&'a ::chrono::DateTime<::chrono::offset::Utc>>,
    ) -> Result<ResponseValue<types::DeployList>, Error<types::Error>> {
        let url = format!(
            "{}/services/{}/deploys",
            self.baseurl,
            encode_path(&service_id.to_string()),
        );
        let mut header_map = ::reqwest::header::HeaderMap::with_capacity(1usize);
        header_map.append(
            ::reqwest::header::HeaderName::from_static("api-version"),
            ::reqwest::header::HeaderValue::from_static(Self::api_version()),
        );
        #[allow(unused_mut)]
        let mut request = self
            .client
            .get(url)
            .header(
                ::reqwest::header::ACCEPT,
                ::reqwest::header::HeaderValue::from_static("application/json"),
            )
            .query(&progenitor_client::QueryParam::new(
                "createdAfter",
                &created_after,
            ))
            .query(&progenitor_client::QueryParam::new(
                "createdBefore",
                &created_before,
            ))
            .query(&progenitor_client::QueryParam::new("cursor", &cursor))
            .query(&progenitor_client::QueryParam::new(
                "finishedAfter",
                &finished_after,
            ))
            .query(&progenitor_client::QueryParam::new(
                "finishedBefore",
                &finished_before,
            ))
            .query(&progenitor_client::QueryParam::new("limit", &limit))
            .query(&progenitor_client::QueryParam::new("status", &status))
            .query(&progenitor_client::QueryParam::new(
                "updatedAfter",
                &updated_after,
            ))
            .query(&progenitor_client::QueryParam::new(
                "updatedBefore",
                &updated_before,
            ))
            .headers(header_map)
            .build()?;
        let info = OperationInfo {
            operation_id: "list_deploys",
        };
        self.pre(&mut request, &info).await?;
        let result = self.exec(request, &info).await;
        self.post(&result, &info).await?;
        let response = result?;
        match response.status().as_u16() {
            200u16 => ResponseValue::from_response(response).await,
            401u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            403u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            404u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            406u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            410u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            429u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            500u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            503u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            _ => Err(Error::UnexpectedResponse(response)),
        }
    }

    ///Trigger deploy
    ///
    ///Trigger a deploy for the service with the provided ID.
    ///
    ///
    ///Sends a `POST` request to `/services/{serviceId}/deploys`
    ///
    ///Arguments:
    /// - `service_id`: The ID of the service
    /// - `body`
    pub async fn create_deploy<'a>(
        &'a self,
        service_id: &'a str,
        body: &'a types::CreateDeployBody,
    ) -> Result<ResponseValue<()>, Error<types::Error>> {
        let url = format!(
            "{}/services/{}/deploys",
            self.baseurl,
            encode_path(&service_id.to_string()),
        );
        let mut header_map = ::reqwest::header::HeaderMap::with_capacity(1usize);
        header_map.append(
            ::reqwest::header::HeaderName::from_static("api-version"),
            ::reqwest::header::HeaderValue::from_static(Self::api_version()),
        );
        #[allow(unused_mut)]
        let mut request = self
            .client
            .post(url)
            .header(
                ::reqwest::header::ACCEPT,
                ::reqwest::header::HeaderValue::from_static("application/json"),
            )
            .json(&body)
            .headers(header_map)
            .build()?;
        let info = OperationInfo {
            operation_id: "create_deploy",
        };
        self.pre(&mut request, &info).await?;
        let result = self.exec(request, &info).await;
        self.post(&result, &info).await?;
        let response = result?;
        match response.status().as_u16() {
            202u16 => Ok(ResponseValue::empty(response)),
            400u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            401u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            404u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            406u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            409u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            410u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            429u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            500u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            503u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            _ => Err(Error::UnexpectedResponse(response)),
        }
    }

    ///Retrieve deploy
    ///
    ///Retrieve the details of a particular deploy for a particular service.
    ///
    ///
    ///Sends a `GET` request to `/services/{serviceId}/deploys/{deployId}`
    ///
    ///Arguments:
    /// - `service_id`: The ID of the service
    /// - `deploy_id`: The ID of the deploy
    pub async fn retrieve_deploy<'a>(
        &'a self,
        service_id: &'a str,
        deploy_id: &'a str,
    ) -> Result<ResponseValue<types::Deploy>, Error<types::Error>> {
        let url = format!(
            "{}/services/{}/deploys/{}",
            self.baseurl,
            encode_path(&service_id.to_string()),
            encode_path(&deploy_id.to_string()),
        );
        let mut header_map = ::reqwest::header::HeaderMap::with_capacity(1usize);
        header_map.append(
            ::reqwest::header::HeaderName::from_static("api-version"),
            ::reqwest::header::HeaderValue::from_static(Self::api_version()),
        );
        #[allow(unused_mut)]
        let mut request = self
            .client
            .get(url)
            .header(
                ::reqwest::header::ACCEPT,
                ::reqwest::header::HeaderValue::from_static("application/json"),
            )
            .headers(header_map)
            .build()?;
        let info = OperationInfo {
            operation_id: "retrieve_deploy",
        };
        self.pre(&mut request, &info).await?;
        let result = self.exec(request, &info).await;
        self.post(&result, &info).await?;
        let response = result?;
        match response.status().as_u16() {
            200u16 => ResponseValue::from_response(response).await,
            401u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            403u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            404u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            406u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            410u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            429u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            500u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            503u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            _ => Err(Error::UnexpectedResponse(response)),
        }
    }

    ///List logs
    ///
    ///List logs matching the provided filters. Logs are paginated by start and
    /// end timestamps. There are more logs to fetch if `hasMore` is true in
    /// the response. Provide the `nextStartTime` and `nextEndTime`
    /// timestamps as the `startTime` and `endTime` query parameters to fetch
    /// the next page of logs.
    ///
    ///You can query for logs across multiple resources, but all resources must
    /// be in the same region and belong to the same owner.
    ///
    ///
    ///Sends a `GET` request to `/logs`
    ///
    ///Arguments:
    /// - `direction`: The direction to query logs for. Backward will return
    ///   most recent logs first.
    ///Forward will start with the oldest logs in the time range.
    ///
    /// - `end_time`: Epoch/Unix timestamp of end of time range to return.
    ///   Defaults to `now()`.
    /// - `host`: Filter request logs by their host. [Wildcards and regex](https://render.com/docs/logging#wildcards-and-regular-expressions)
    ///   are supported.
    /// - `instance`: Filter logs by the instance they were emitted from. An
    ///   instance is the id of a specific running server.
    /// - `level`: Filter logs by their severity level. [Wildcards and regex](https://render.com/docs/logging#wildcards-and-regular-expressions)
    ///   are supported.
    /// - `limit`: The maximum number of items to return. For details, see [Pagination](https://api-docs.render.com/reference/pagination).
    /// - `method`: Filter request logs by their requests method. [Wildcards and
    ///   regex](https://render.com/docs/logging#wildcards-and-regular-expressions)
    ///   are supported.
    /// - `owner_id`: The ID of the workspace to return logs for
    /// - `path`: Filter request logs by their path. [Wildcards and regex](https://render.com/docs/logging#wildcards-and-regular-expressions)
    ///   are supported.
    /// - `resource`: Filter logs by their resource. A resource is the id of a
    ///   server, cronjob, job, postgres, redis, or workflow.
    /// - `start_time`: Epoch/Unix timestamp of start of time range to return.
    ///   Defaults to `now() - 1 hour`.
    /// - `status_code`: Filter request logs by their status code. [Wildcards and regex](https://render.com/docs/logging#wildcards-and-regular-expressions) are supported.
    /// - `task`: Filter logs by their task(s)
    /// - `task_run`: Filter logs by their task run id(s)
    /// - `text`: Filter by the text of the logs. [Wildcards and regex](https://render.com/docs/logging#wildcards-and-regular-expressions)
    ///   are supported.
    /// - `type_`: Filter logs by their type. Types include `app` for
    ///   application logs, `request` for request logs, and `build` for build
    ///   logs. You can find the full set of types available for a query by
    ///   using the `GET /logs/values` endpoint.
    pub async fn list_logs<'a>(
        &'a self,
        direction: Option<&'a str>,
        end_time: Option<&'a ::chrono::DateTime<::chrono::offset::Utc>>,
        host: Option<&'a ::std::vec::Vec<::std::string::String>>,
        instance: Option<&'a ::std::vec::Vec<::std::string::String>>,
        level: Option<&'a ::std::vec::Vec<::std::string::String>>,
        limit: Option<::std::num::NonZeroU64>,
        method: Option<&'a ::std::vec::Vec<::std::string::String>>,
        owner_id: &'a str,
        path: Option<&'a ::std::vec::Vec<::std::string::String>>,
        resource: &'a ::std::vec::Vec<::std::string::String>,
        start_time: Option<&'a ::chrono::DateTime<::chrono::offset::Utc>>,
        status_code: Option<&'a ::std::vec::Vec<::std::string::String>>,
        task: Option<&'a ::std::vec::Vec<::std::string::String>>,
        task_run: Option<&'a ::std::vec::Vec<::std::string::String>>,
        text: Option<&'a ::std::vec::Vec<::std::string::String>>,
        type_: Option<&'a ::std::vec::Vec<::std::string::String>>,
    ) -> Result<ResponseValue<types::ListLogsResponse>, Error<types::Error>> {
        let url = format!("{}/logs", self.baseurl,);
        let mut header_map = ::reqwest::header::HeaderMap::with_capacity(1usize);
        header_map.append(
            ::reqwest::header::HeaderName::from_static("api-version"),
            ::reqwest::header::HeaderValue::from_static(Self::api_version()),
        );
        #[allow(unused_mut)]
        let mut request = self
            .client
            .get(url)
            .header(
                ::reqwest::header::ACCEPT,
                ::reqwest::header::HeaderValue::from_static("application/json"),
            )
            .query(&progenitor_client::QueryParam::new("direction", &direction))
            .query(&progenitor_client::QueryParam::new("endTime", &end_time))
            .query(&progenitor_client::QueryParam::new("host", &host))
            .query(&progenitor_client::QueryParam::new("instance", &instance))
            .query(&progenitor_client::QueryParam::new("level", &level))
            .query(&progenitor_client::QueryParam::new("limit", &limit))
            .query(&progenitor_client::QueryParam::new("method", &method))
            .query(&progenitor_client::QueryParam::new("ownerId", &owner_id))
            .query(&progenitor_client::QueryParam::new("path", &path))
            .query(&progenitor_client::QueryParam::new("resource", &resource))
            .query(&progenitor_client::QueryParam::new(
                "startTime",
                &start_time,
            ))
            .query(&progenitor_client::QueryParam::new(
                "statusCode",
                &status_code,
            ))
            .query(&progenitor_client::QueryParam::new("task", &task))
            .query(&progenitor_client::QueryParam::new("taskRun", &task_run))
            .query(&progenitor_client::QueryParam::new("text", &text))
            .query(&progenitor_client::QueryParam::new("type", &type_))
            .headers(header_map)
            .build()?;
        let info = OperationInfo {
            operation_id: "list_logs",
        };
        self.pre(&mut request, &info).await?;
        let result = self.exec(request, &info).await;
        self.post(&result, &info).await?;
        let response = result?;
        match response.status().as_u16() {
            200u16 => ResponseValue::from_response(response).await,
            400u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            401u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            403u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            404u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            406u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            410u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            429u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            500u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            503u16 => Err(Error::ErrorResponse(
                ResponseValue::from_response(response).await?,
            )),
            _ => Err(Error::UnexpectedResponse(response)),
        }
    }
}

/// Items consumers will typically use such as the Client.
pub mod prelude {
    #[allow(unused_imports)]
    pub use super::Client;
}
