# Durable service and hardening

## Tasks delivered

Implement all routes, durable queue/units, restart/cancel/delete, authentication/cache ownership, limits and deployment artifacts.

## Decisions

SQLite single-server lock, atomic checkpoints, PDF page reuse, deterministic reparse for other formats. Process groups enforce native termination; sampled RSS bounds macOS, RLIMIT_AS adds Linux allocation control.

## Exit evidence

Real multipart/job/result/unit/asset/auth/cache/cancel/timeout/restart/delete tests; malformed input and OpenAPI tests.

## Remaining acceptance

Docker daemon unavailable locally. Linux, long-soak, sandbox, backup restoration and multi-host deployments remain unvalidated.

Implementation present; acceptance is limited to the recorded checks. See ../PHASES.md and ../CAPABILITIES.md.
