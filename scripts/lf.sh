#!/bin/bash
# Text files back to LF line endings (an editor or tool on Windows may write CRLF). Binary
# assets are never touched.
#   bash /mnt/d/code/burin/scripts/lf.sh
cd /mnt/d/code/burin
for f in $(git ls-files crates scripts bench docs apps | grep -E '\.(rs|burin|sh|py|mjs|js|ts|md|html|css|toml|json|ll)$'); do
  if grep -q $'\r' "$f"; then
    echo "LF: $f"
    sed -i 's/\r$//' "$f"
  fi
done
