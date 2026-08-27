#!/usr/bin/env bash
# Alfred-style script filter: receives the query, prints JSON items.
#
# `uid` keys the row across keystrokes, which is what lets the launcher's
# frecency learn it even though `arg` changes with every query.
# `autocomplete` is what Tab replaces the query with.
q="${1:-}"
upper="$(printf '%s' "$q" | tr '[:lower:]' '[:upper:]')"
printf '{"items":[
  {"uid":"echo","title":"Echo: %s","subtitle":"from the demo plugin","arg":"%s","autocomplete":"demo %s!"},
  {"uid":"upper","title":"Uppercase","subtitle":"%s","arg":"%s"}
]}\n' "$q" "$q" "$q" "$upper" "$q"
