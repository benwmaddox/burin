#!/bin/bash
# The sidebar loader's SQL in the last build (one statement per line).
awk '/define i1 @burin_load_sidebar_panel/,/^}/' ~/burin-gen-campfire/app.ll | grep '^  ; SELECT' | sed 's/^  ; //' | cut -c1-${1:-400}
