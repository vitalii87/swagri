//! Protocol-neutral task and result types used by Swagri nodes.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Wire protocol negotiated by the MVP request/response behaviour.
pub const TASK_PROTOCOL_V1: &str = "/swagri/task/1";

/// Signed, chunked agent-update protocol.
pub const UPDATE_PROTOCOL_V1: &str = "/swagri/update/1";

/// Maximum executable size accepted through the update protocol.
pub const MAX_UPDATE_BYTES: u64 = 128 * 1024 * 1024;

/// Maximum payload returned by one update chunk request.
pub const UPDATE_CHUNK_BYTES: u32 = 256 * 1024;

/// Maximum text payload accepted by the built-in prototype tasks.
pub const MAX_TEXT_BYTES: usize = 1024 * 1024;

/// Maximum number of values accepted by the sum task.
pub const MAX_SUM_VALUES: usize = 100_000;

/// Maximum iterations accepted by the synthetic CPU benchmark.
pub const MAX_BENCHMARK_ITERATIONS: u64 = 50_000_000;

/// A request identifier is unique from the perspective of the originating node.
pub type TaskId = String;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResourceSnapshot {
    pub observed_at_unix_ms: u64,
    pub os: String,
    pub arch: String,
    pub cpu_brand: String,
    pub physical_cores: u16,
    pub logical_cores: u16,
    pub total_memory_bytes: u64,
    pub available_memory_bytes: u64,
    pub host_cpu_percent: f32,
    pub agent_cpu_percent: f32,
    pub agent_memory_bytes: u64,
    pub active_tasks: u32,
    pub cpu_limit_percent: f32,
    pub memory_limit_percent: f32,
    pub allocatable_memory_bytes: u64,
    pub calibrated_cpu_score: f64,
    pub effective_cpu_score: f64,
}

pub fn effective_cpu_score(
    calibrated_score: f64,
    host_cpu_percent: f32,
    agent_cpu_percent: f32,
    cpu_limit_percent: f32,
) -> f64 {
    let host_free = (100.0 - host_cpu_percent.clamp(0.0, 100.0)).max(0.0);
    let policy_free =
        (cpu_limit_percent.clamp(0.0, 100.0) - agent_cpu_percent.clamp(0.0, 100.0)).max(0.0);
    calibrated_score.max(0.0) * f64::from(host_free.min(policy_free)) / 100.0
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UpdateManifest {
    pub version: String,
    pub target_os: String,
    pub target_arch: String,
    pub size: u64,
    pub sha256_hex: String,
}

impl UpdateManifest {
    /// Stable bytes signed by the peer identity key.
    pub fn signing_payload(&self) -> Vec<u8> {
        format!(
            "swagri-update-v1\n{}\n{}\n{}\n{}\n{}\n",
            self.version, self.target_os, self.target_arch, self.size, self.sha256_hex
        )
        .into_bytes()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SignedUpdateManifest {
    pub manifest: UpdateManifest,
    #[serde(with = "serde_bytes")]
    pub signer_public_key: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub signature: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UpdateRequest {
    Manifest,
    Chunk {
        version: String,
        offset: u64,
        length: u32,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UpdateResponse {
    Manifest {
        signed: SignedUpdateManifest,
    },
    Chunk {
        offset: u64,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
    Error {
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskRequest {
    pub id: TaskId,
    pub task: Task,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Task {
    NodeInfo,
    Echo { message: String },
    Sum { values: Vec<f64> },
    Sha256 { text: String },
    CpuBenchmark { iterations: u64 },
}

impl Task {
    pub fn kind(&self) -> TaskKind {
        match self {
            Self::NodeInfo => TaskKind::NodeInfo,
            Self::Echo { .. } => TaskKind::Echo,
            Self::Sum { .. } => TaskKind::Sum,
            Self::Sha256 { .. } => TaskKind::Sha256,
            Self::CpuBenchmark { .. } => TaskKind::CpuBenchmark,
        }
    }

    pub fn validate(&self) -> Result<(), TaskValidationError> {
        match self {
            Self::Echo { message } if message.len() > MAX_TEXT_BYTES => {
                Err(TaskValidationError::TextTooLarge)
            }
            Self::Sha256 { text } if text.len() > MAX_TEXT_BYTES => {
                Err(TaskValidationError::TextTooLarge)
            }
            Self::Sum { values } if values.len() > MAX_SUM_VALUES => {
                Err(TaskValidationError::TooManyValues)
            }
            Self::Sum { values } if values.iter().any(|value| !value.is_finite()) => {
                Err(TaskValidationError::NonFiniteNumber)
            }
            Self::CpuBenchmark { iterations }
                if *iterations == 0 || *iterations > MAX_BENCHMARK_ITERATIONS =>
            {
                Err(TaskValidationError::InvalidBenchmarkIterations)
            }
            _ => Ok(()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    NodeInfo,
    Echo,
    Sum,
    Sha256,
    CpuBenchmark,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NodeCapabilities {
    pub protocol_version: u16,
    pub task_kinds: Vec<TaskKind>,
    pub max_text_bytes: usize,
}

impl Default for NodeCapabilities {
    fn default() -> Self {
        Self {
            protocol_version: 2,
            task_kinds: vec![
                TaskKind::NodeInfo,
                TaskKind::Echo,
                TaskKind::Sum,
                TaskKind::Sha256,
                TaskKind::CpuBenchmark,
            ],
            max_text_bytes: MAX_TEXT_BYTES,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskResponse {
    pub id: TaskId,
    pub duration_ms: u64,
    pub outcome: TaskOutcome,
}

impl TaskResponse {
    pub fn success(id: TaskId, duration_ms: u64, result: TaskResult) -> Self {
        Self {
            id,
            duration_ms,
            outcome: TaskOutcome::Success { result },
        }
    }

    pub fn failure(
        id: TaskId,
        duration_ms: u64,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            id,
            duration_ms,
            outcome: TaskOutcome::Failure {
                code: code.into(),
                message: message.into(),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TaskOutcome {
    Success { result: TaskResult },
    Failure { code: String, message: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TaskResult {
    NodeInfo {
        agent_version: String,
        protocol_version: u16,
        #[serde(default)]
        resources: Option<ResourceSnapshot>,
    },
    Echo {
        message: String,
    },
    Sum {
        value: f64,
    },
    Sha256 {
        digest_hex: String,
    },
    CpuBenchmark {
        checksum: u64,
        iterations: u64,
    },
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum TaskValidationError {
    #[error("text payload exceeds the configured limit")]
    TextTooLarge,
    #[error("sum contains too many values")]
    TooManyValues,
    #[error("sum values must be finite")]
    NonFiniteNumber,
    #[error("benchmark iterations must be between 1 and the configured limit")]
    InvalidBenchmarkIterations,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_finite_sum_values() {
        let task = Task::Sum {
            values: vec![1.0, f64::NAN],
        };

        assert_eq!(task.validate(), Err(TaskValidationError::NonFiniteNumber));
    }

    #[test]
    fn rejects_zero_benchmark_iterations() {
        let task = Task::CpuBenchmark { iterations: 0 };

        assert_eq!(
            task.validate(),
            Err(TaskValidationError::InvalidBenchmarkIterations)
        );
    }

    #[test]
    fn update_signing_payload_is_stable() {
        let manifest = UpdateManifest {
            version: "1.2.3".into(),
            target_os: "windows".into(),
            target_arch: "x86_64".into(),
            size: 42,
            sha256_hex: "abc".into(),
        };

        assert_eq!(
            manifest.signing_payload(),
            b"swagri-update-v1\n1.2.3\nwindows\nx86_64\n42\nabc\n"
        );
    }

    #[test]
    fn effective_score_combines_machine_power_and_current_headroom() {
        assert_eq!(effective_cpu_score(400.0, 72.0, 0.0, 100.0), 112.0);
        assert_eq!(effective_cpu_score(100.0, 12.0, 0.0, 100.0), 88.0);
        assert_eq!(effective_cpu_score(400.0, 10.0, 20.0, 25.0), 20.0);
    }
}
