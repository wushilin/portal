#!/usr/bin/env bash
set -e

# Fixed certificate files
FILES=(
  ca.key
  ca.pem
  ca.srl
  server.key
  server.pem
  server.csr
  client.key
  client.pem
  client.csr
)

# Add all client-<CN> files dynamically
FILES+=(client-*.key client-*.csr client-*.pem)

echo "Cleaning up certificate files..."

for f in "${FILES[@]}"; do
    # Use glob expansion and check if file exists
    for file in $f; do
        if [[ -f "$file" ]]; then
            rm "$file" && echo "Removed $file successfully" || echo "Failed to remove $file"
        else
            echo "File $file does not exist"
        fi
    done
done

echo "Cleanup complete."
