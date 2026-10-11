# Canonical contract and CLI

## Tasks delivered

Create workspace, structured schema, format adapter, isolated CLI and deterministic fixtures.

## Decisions

Document/unit/block/table/asset/diagram provenance uses JSON as truth. CLI shares worker supervision with HTTP. Word boundaries stay logical.

## Exit evidence

Schema fields and OpenAPI references resolve; malformed XML/archives and resource limits tested.

## Remaining acceptance

Fixture corpus is synthetic; no universal schema-to-every-format fidelity claim.

Implementation present; acceptance is limited to the recorded checks. See ../PHASES.md and ../CAPABILITIES.md.
