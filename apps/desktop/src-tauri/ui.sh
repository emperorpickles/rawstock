#!/bin/sh
# Start or build the shared UI. Resolves the script directory so it works
# whichever directory Tauri uses as the current one.
set -eu
cd "$(CDPATH= cd -- "$(dirname "$0")" && pwd)/../../ui"
exec npm run "$1"
