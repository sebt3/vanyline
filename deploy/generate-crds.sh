#!/usr/bin/env bash
# Régénère deploy/crds.yaml depuis les types Rust (controller/src/crds.rs).
# À relancer à chaque changement de schéma des CRDs Owner/Project/Sandbox.
#
#   deploy/generate-crds.sh
#
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
cargo run --release -p vanyline-controller -- --crds > deploy/crds.yaml
echo "deploy/crds.yaml régénéré ($(wc -l < deploy/crds.yaml) lignes)"
