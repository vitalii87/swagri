# Contributing

Swagri is an experimental project. Contributions should make assumptions easier
to test rather than only making the architecture larger.

## Development setup

Install the stable Rust toolchain with `rustfmt` and `clippy`, then run:

```console
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
```

## Pull requests

Keep pull requests focused and explain:

- the behavior or hypothesis being changed;
- why the change is needed;
- security and trust-boundary effects;
- measurements for performance or scheduling claims;
- tests used to validate the change.

Do not add remote shell execution, implicit trust of discovered peers, or new
network exposure without an explicit threat-model update.

## Research changes

For scheduler or performance work, record:

- baseline policy;
- workload and input size;
- node hardware and relevant dynamic conditions;
- network latency and bandwidth where applicable;
- wall-clock time, CPU time, transferred bytes, and failures;
- whether the result supports or rejects the hypothesis.

Negative results are useful and should be preserved.

## Commit style

Use short imperative subjects, for example:

```text
Add typed task rejection
Measure local and remote hashing
Document trust boundaries
```

