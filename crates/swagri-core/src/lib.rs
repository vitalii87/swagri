//! Protocol-neutral task and result types used by Swagri nodes.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Wire protocol negotiated by the MVP request/response behaviour.
pub const TASK_PROTOCOL_V1: &str = "/swagri/task/1";

/// Signed, chunked agent-update protocol.
pub const UPDATE_PROTOCOL_V1: &str = "/swagri/update/1";

/// Trusted manifest inventory and verified content-addressed block exchange.
pub const ARTIFACT_PROTOCOL_V1: &str = "/swagri/artifact/1";

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

/// Smallest accepted side length for the deterministic matrix workload.
pub const MIN_MATRIX_SIZE: u16 = 16;

/// Largest accepted side length for the deterministic matrix workload.
pub const MAX_MATRIX_SIZE: u16 = 384;

/// Largest accepted side length for a row-chunked distributed matrix workload.
pub const MAX_DISTRIBUTED_MATRIX_SIZE: u16 = 768;

/// Maximum number of output rows computed by one distributed matrix request.
pub const MAX_MATRIX_CHUNK_ROWS: u16 = 128;

/// Capability version advertised by nodes that support bounded matrix chunks
/// and runtime contribution pause/resume.
pub const NODE_PROTOCOL_VERSION: u16 = 4;

/// Minimum effective-CPU advantage required before local-first placement sends
/// a CPU-only task over the network.
pub const REMOTE_CPU_MINIMUM_GAIN: f64 = 1.20;

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
    #[serde(default)]
    pub contribution_paused: bool,
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

/// Result of comparing the local CPU headroom with observed remote agents.
/// The index refers to the caller-provided `remote_scores` slice.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CpuPlacementDecision {
    pub remote_candidate_index: Option<usize>,
    pub local_score: f64,
    pub selected_score: f64,
    pub minimum_remote_score: f64,
}

/// Selects a remote CPU only when its current effective score clears the
/// configured network-overhead margin. Invalid or unavailable scores never win.
pub fn choose_cpu_placement(
    local_score: f64,
    remote_scores: &[f64],
    minimum_gain: f64,
) -> CpuPlacementDecision {
    let local_score = finite_non_negative(local_score);
    let minimum_gain = if minimum_gain.is_finite() {
        minimum_gain.max(1.0)
    } else {
        REMOTE_CPU_MINIMUM_GAIN
    };
    let minimum_remote_score = local_score * minimum_gain;
    let best_remote = remote_scores
        .iter()
        .enumerate()
        .filter_map(|(index, score)| {
            score
                .is_finite()
                .then_some((index, score.max(0.0)))
                .filter(|(_, score)| *score > 0.0)
        })
        .max_by(|left, right| left.1.total_cmp(&right.1));

    if let Some((index, score)) = best_remote
        && score >= minimum_remote_score
        && score > local_score
    {
        return CpuPlacementDecision {
            remote_candidate_index: Some(index),
            local_score,
            selected_score: score,
            minimum_remote_score,
        };
    }

    CpuPlacementDecision {
        remote_candidate_index: None,
        local_score,
        selected_score: local_score,
        minimum_remote_score,
    }
}

fn finite_non_negative(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
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

    /// Stable bytes used for a Debugger binary. A distinct domain prevents a
    /// signed Agent manifest from being replayed as a Debugger update.
    pub fn debugger_signing_payload(&self) -> Vec<u8> {
        format!(
            "swagri-debugger-update-v1\n{}\n{}\n{}\n{}\n{}\n",
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
    DebuggerManifest,
    Chunk {
        version: String,
        offset: u64,
        length: u32,
    },
    DebuggerChunk {
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
    Echo {
        message: String,
    },
    Sum {
        values: Vec<f64>,
    },
    Sha256 {
        text: String,
    },
    CpuBenchmark {
        iterations: u64,
    },
    MatrixMultiply {
        size: u16,
    },
    MatrixChunk {
        size: u16,
        row_start: u16,
        row_end: u16,
    },
}

impl Task {
    pub fn kind(&self) -> TaskKind {
        match self {
            Self::NodeInfo => TaskKind::NodeInfo,
            Self::Echo { .. } => TaskKind::Echo,
            Self::Sum { .. } => TaskKind::Sum,
            Self::Sha256 { .. } => TaskKind::Sha256,
            Self::CpuBenchmark { .. } => TaskKind::CpuBenchmark,
            Self::MatrixMultiply { .. } => TaskKind::MatrixMultiply,
            Self::MatrixChunk { .. } => TaskKind::MatrixChunk,
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
            Self::MatrixMultiply { size }
                if !(MIN_MATRIX_SIZE..=MAX_MATRIX_SIZE).contains(size) =>
            {
                Err(TaskValidationError::InvalidMatrixSize)
            }
            Self::MatrixChunk {
                size,
                row_start,
                row_end,
            } if !(MIN_MATRIX_SIZE..=MAX_DISTRIBUTED_MATRIX_SIZE).contains(size)
                || row_start >= row_end
                || row_end > size
                || row_end - row_start > MAX_MATRIX_CHUNK_ROWS =>
            {
                Err(TaskValidationError::InvalidMatrixChunk)
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
    MatrixMultiply,
    MatrixChunk,
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
            protocol_version: NODE_PROTOCOL_VERSION,
            task_kinds: vec![
                TaskKind::NodeInfo,
                TaskKind::Echo,
                TaskKind::Sum,
                TaskKind::Sha256,
                TaskKind::CpuBenchmark,
                TaskKind::MatrixMultiply,
                TaskKind::MatrixChunk,
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
        node_name: String,
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
    MatrixMultiply {
        checksum: u64,
        size: u16,
    },
    MatrixChunk {
        checksum: u64,
        size: u16,
        row_start: u16,
        row_end: u16,
    },
    DistributedMatrix {
        checksum: u64,
        size: u16,
        chunks: u16,
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
    #[error("matrix size must be between 16 and 384")]
    InvalidMatrixSize,
    #[error("matrix chunk must contain 1 to 128 rows within a matrix of size 16 to 768")]
    InvalidMatrixChunk,
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
    fn rejects_matrix_outside_bounded_size() {
        let task = Task::MatrixMultiply {
            size: MIN_MATRIX_SIZE - 1,
        };

        assert_eq!(task.validate(), Err(TaskValidationError::InvalidMatrixSize));
    }

    #[test]
    fn validates_bounded_distributed_matrix_chunk() {
        assert!(
            Task::MatrixChunk {
                size: 768,
                row_start: 640,
                row_end: 768,
            }
            .validate()
            .is_ok()
        );
        assert_eq!(
            Task::MatrixChunk {
                size: 768,
                row_start: 0,
                row_end: 129,
            }
            .validate(),
            Err(TaskValidationError::InvalidMatrixChunk)
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
        assert_eq!(
            manifest.debugger_signing_payload(),
            b"swagri-debugger-update-v1\n1.2.3\nwindows\nx86_64\n42\nabc\n"
        );
    }

    #[test]
    fn effective_score_combines_machine_power_and_current_headroom() {
        assert_eq!(effective_cpu_score(400.0, 72.0, 0.0, 100.0), 112.0);
        assert_eq!(effective_cpu_score(100.0, 12.0, 0.0, 100.0), 88.0);
        assert_eq!(effective_cpu_score(400.0, 10.0, 20.0, 25.0), 20.0);
    }

    #[test]
    fn cpu_placement_stays_local_when_remote_gain_is_too_small() {
        let decision = choose_cpu_placement(100.0, &[119.9, 80.0], REMOTE_CPU_MINIMUM_GAIN);

        assert_eq!(decision.remote_candidate_index, None);
        assert_eq!(decision.selected_score, 100.0);
        assert_eq!(decision.minimum_remote_score, 120.0);
    }

    #[test]
    fn cpu_placement_selects_the_strongest_remote_above_margin() {
        let decision = choose_cpu_placement(100.0, &[125.0, 150.0, 130.0], REMOTE_CPU_MINIMUM_GAIN);

        assert_eq!(decision.remote_candidate_index, Some(1));
        assert_eq!(decision.selected_score, 150.0);
    }

    #[test]
    fn cpu_placement_routes_remote_when_local_contribution_is_unavailable() {
        let decision = choose_cpu_placement(0.0, &[50.0], REMOTE_CPU_MINIMUM_GAIN);

        assert_eq!(decision.remote_candidate_index, Some(0));
        assert_eq!(decision.selected_score, 50.0);
        assert_eq!(decision.minimum_remote_score, 0.0);
    }

    #[test]
    fn cpu_placement_ignores_invalid_remote_scores() {
        let decision = choose_cpu_placement(100.0, &[f64::NAN, -1.0, 0.0], f64::NAN);

        assert_eq!(decision.remote_candidate_index, None);
        assert_eq!(decision.minimum_remote_score, 120.0);
    }
}
