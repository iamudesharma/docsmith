# Optional local vision

## Tasks delivered

Provision pinned small-model weights/projector; implement loopback-only OpenAI-compatible requests, serialized inference and bounded outputs.

## Decisions

Baseline never calls models. Raw descriptions/graphs stay inferred; invalid graphs retain the raw description and warnings. No cloud or runtime downloads.

## Exit evidence

Actual SmolVLM2 requests and measured RSS/swap/latency. The initial ER experiment failed JSON adherence; final constrained result is recorded.

## Remaining acceptance

Dense diagram correctness is not guaranteed. Stronger model and M1 Pro 16 GB target tests remain unrun.

Implementation present; acceptance is limited to the recorded checks. See ../PHASES.md and ../CAPABILITIES.md.
