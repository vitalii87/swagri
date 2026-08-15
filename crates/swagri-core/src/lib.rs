//! Protocol-neutral task and result types used by Swagri nodes.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Wire protocol negotiated by the MVP request/response behaviour.
pub const TASK_PROTOCOL_V1: &str = "/swagri/task/1";

/// Maximum text payload accepted by the built-in prototype tasks.
pub const MAX_TEXT_BYTES: usize = 1024 * 1024;

/// Maximum number of values accepted by the sum task.
pub const MAX_SUM_VALUES: usize = 100_000;

/// Maximum iterations accepted by the synthetic CPU benchmark.
pub const MAX_BENCHMARK_ITERATIONS: u64 = 50_000_000;

/// A request identifier is unique from the perspective of the originating node.
pub type TaskId = String;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskRequest {
    pub id: TaskId,
    pub task: Task,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Task {
    Echo { message: String },
    Sum { values: Vec<f64> },
    Sha256 { text: String },
    CpuBenchmark { iterations: u64 },
}

impl Task {
    pub fn kind(&self) -> TaskKind {
        match self {
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
            protocol_version: 1,
            task_kinds: vec![
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
    Echo { message: String },
    Sum { value: f64 },
    Sha256 { digest_hex: String },
    CpuBenchmark { checksum: u64, iterations: u64 },
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
}
