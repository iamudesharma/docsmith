# Source graphs and figure preservation

## Tasks delivered

Decode draw.io pages and retained PNG/SVG/PDF source; implement explicit Mermaid/DOT/PlantUML/DBML subsets; preserve SVG geometry.

## Decisions

Explicit source edges/keys/cardinalities stay native. Flattened PDF labels are candidates, and vector connector topology is unknown. No source FK is invented.

## Exit evidence

Two-node/one-edge source and compressed equivalence; source metadata extraction; ER attribute/cardinality and unsupported-construct checks.

## Remaining acceptance

Visio/SmartArt graph interpretation and arbitrary source syntax remain separate gates. Raster semantics require an optional uncertain model.

Implementation present; acceptance is limited to the recorded checks. See ../PHASES.md and ../CAPABILITIES.md.
