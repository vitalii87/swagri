# Vision

## Swarm Grid

Swagri is short for **Swarm Grid**.

The grid supplies heterogeneous resources. The swarm decides how to combine
them for a particular task. The result is not intended to be one permanent
global cluster. It is a fabric that forms temporary execution areas and changes
their shape as conditions change.

## The beam in fog

A useful metaphor is a beam of light moving through fog:

1. A task establishes a direction.
2. The originating node and the closest suitable peers form the initial beam.
3. The beam widens only when more capacity is useful.
4. Nodes outside the useful area remain idle.
5. When the task ends, the temporary structure dissolves.

"Closest" is not only geographic or network distance. It can mean that a node
already holds the required data or model, has a suitable accelerator, is
trusted by the task owner, has spare thermal capacity, or has the lowest total
execution cost.

## The organism

The second metaphor is an organism. Different components contribute according
to their current condition rather than a fixed nominal capacity. If one node
overheats while another is idle, new work should shift toward the idle node. If
a laptop switches to battery, its contribution may shrink. If a GPU already has
a model loaded, inference may move toward the data and model instead of moving
both across the network.

This requires a continuous feedback loop:

```text
observe -> estimate -> allocate -> execute -> measure -> rebalance
```

## Core hypothesis

Swagri's central hypothesis is not merely that devices can communicate. It is:

> A decentralized system can make useful, explainable decisions about when
> distributed execution is better than local execution, and can revise those
> decisions as resource conditions change.

A scheduling decision should consider expected utility rather than raw speed:

```text
utility = compute gain
        - network cost
        - data movement cost
        - thermal cost
        - energy cost
        - coordination cost
        - reliability risk
        - trust risk
```

The exact model is a research question and must be refined through measurement.

## Swarm AI

Adaptive swarm AI is a major prospective use case, but not the only purpose of
the project. Realistic early AI scenarios include:

- routing inference to a node that already hosts the required model;
- coordinating independent agents that solve different subtasks;
- composing speech, language, vision, verification, and aggregation stages;
- comparing redundant model outputs on trusted nodes;
- keeping sensitive stages on a user's own devices.

Splitting one large neural network across arbitrary internet devices is a much
later research direction because latency and data movement can dominate any
compute gain.

## Non-goals

Swagri is not currently intended to be:

- a blockchain or token network;
- a replacement for every data center or cluster scheduler;
- a system where every peer participates in every task;
- a promise that distributed computing is always faster;
- an excuse to execute untrusted native code without isolation;
- dependent on one AI framework or one network transport forever.

## Engineering principle

The project must be willing to reject its own assumptions. Each complex feature
should eventually be supported by code, tests, benchmarks, and measurements.
If an approach does not improve useful execution, it should be changed or
removed.

