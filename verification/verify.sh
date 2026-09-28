#!/usr/bin/env bash
# Verify the Verus model of the pure core.
# Set VERUS to the path of the verus binary. Or, put `verus` on PATH.
set -euo pipefail
cd "$(dirname "$0")"
"${VERUS:-verus}" core.rs
