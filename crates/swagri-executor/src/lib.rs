//! Execution of the small, compiled-in task set used by the MVP.

use std::{hint::black_box, time::Instant};

use sha2::{Digest, Sha256};
use swagri_core::{NODE_PROTOCOL_VERSION, Task, TaskRequest, TaskResponse, TaskResult};

pub fn execute(request: TaskRequest) -> TaskResponse {
    let started = Instant::now();

    if let Err(error) = request.task.validate() {
        return TaskResponse::failure(
            request.id,
            elapsed_millis(started),
            "invalid_task",
            error.to_string(),
        );
    }

    let result = match request.task {
        Task::NodeInfo => TaskResult::NodeInfo {
            agent_version: env!("CARGO_PKG_VERSION").into(),
            protocol_version: NODE_PROTOCOL_VERSION,
            resources: None,
        },
        Task::Echo { message } => TaskResult::Echo { message },
        Task::Sum { values } => TaskResult::Sum {
            value: values.into_iter().sum(),
        },
        Task::Sha256 { text } => {
            let digest = Sha256::digest(text.as_bytes());
            TaskResult::Sha256 {
                digest_hex: hex::encode(digest),
            }
        }
        Task::CpuBenchmark { iterations } => TaskResult::CpuBenchmark {
            checksum: cpu_benchmark(iterations),
            iterations,
        },
        Task::MatrixMultiply { size } => TaskResult::MatrixMultiply {
            checksum: matrix_multiply(size),
            size,
        },
    };

    TaskResponse::success(request.id, elapsed_millis(started), result)
}

fn elapsed_millis(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

fn cpu_benchmark(iterations: u64) -> u64 {
    let mut value = 0x9e37_79b9_7f4a_7c15_u64;

    for index in 0..iterations {
        value = value
            .wrapping_add(index ^ 0xa076_1d64_78bd_642f)
            .rotate_left(17)
            .wrapping_mul(0xe703_7ed1_a0b4_28db);
    }

    black_box(value)
}

fn matrix_multiply(size: u16) -> u64 {
    let side = usize::from(size);
    let cells = side * side;
    let mut left = vec![0_u64; cells];
    let mut right = vec![0_u64; cells];
    let mut output = vec![0_u64; cells];

    for row in 0..side {
        for column in 0..side {
            let index = row * side + column;
            left[index] = ((row * 31 + column * 17 + 1) % 251 + 1) as u64;
            right[index] = ((row * 13 + column * 29 + 7) % 241 + 1) as u64;
        }
    }

    for row in 0..side {
        for inner in 0..side {
            let left_value = left[row * side + inner];
            for column in 0..side {
                let index = row * side + column;
                output[index] = output[index]
                    .wrapping_add(left_value.wrapping_mul(right[inner * side + column]));
            }
        }
    }

    black_box(
        output
            .into_iter()
            .enumerate()
            .fold(0x6a09_e667_f3bc_c909_u64, |checksum, (index, value)| {
                checksum.wrapping_add(value ^ index as u64).rotate_left(11)
            }),
    )
}

#[cfg(test)]
mod tests {
    use swagri_core::{Task, TaskOutcome, TaskRequest, TaskResult};

    use super::*;

    #[test]
    fn executes_sha256() {
        let response = execute(TaskRequest {
            id: "hash-1".into(),
            task: Task::Sha256 {
                text: "Swagri".into(),
            },
        });

        assert_eq!(
            response.outcome,
            TaskOutcome::Success {
                result: TaskResult::Sha256 {
                    digest_hex: "5f43e58f1e999f085b217d8ae34875cdde017a748d5d112b4cda1074880f93fd"
                        .into(),
                },
            }
        );
    }

    #[test]
    fn rejects_invalid_work_before_execution() {
        let response = execute(TaskRequest {
            id: "benchmark-0".into(),
            task: Task::CpuBenchmark { iterations: 0 },
        });

        assert!(matches!(
            response.outcome,
            TaskOutcome::Failure { ref code, .. } if code == "invalid_task"
        ));
    }

    #[test]
    fn reports_agent_version() {
        let response = execute(TaskRequest {
            id: "info-1".into(),
            task: Task::NodeInfo,
        });

        assert!(matches!(
            response.outcome,
            TaskOutcome::Success {
                result: TaskResult::NodeInfo {
                    ref agent_version,
                    protocol_version: NODE_PROTOCOL_VERSION,
                    resources: None,
                }
            } if agent_version == env!("CARGO_PKG_VERSION")
        ));
    }

    #[test]
    fn executes_bounded_matrix_workload() {
        let response = execute(TaskRequest {
            id: "matrix-1".into(),
            task: Task::MatrixMultiply { size: 16 },
        });

        assert!(matches!(
            response.outcome,
            TaskOutcome::Success {
                result: TaskResult::MatrixMultiply { size: 16, checksum }
            } if checksum != 0
        ));
    }
}
