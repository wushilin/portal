#!/usr/bin/env bash
set -e

# Expiry: 20 years ≈ 7300 days
DAYS=7300

# Subject defaults
CA_SUBJ="/C=US/ST=California/L=San Francisco/O=ExampleOrg/OU=CA/CN=Example-Root-CA"
SERVER_SUBJ="/C=US/ST=California/L=San Francisco/O=ExampleOrg/OU=Server/CN=server"
CLIENT_SUBJ="/C=US/ST=California/L=San Francisco/O=ExampleOrg/OU=Client/CN=client"

regenerate_pair_if_needed() {
  KEY="$1"
  CERT="$2"
  WHAT="$3"

  if [[ -f "$KEY" && -f "$CERT" ]]; then
    echo "=== $WHAT exists (both $KEY and $CERT found). Skipping. ==="
    return 1
  fi

  echo "=== Regenerating $WHAT (missing one or both files) ==="
  return 0
}

#############################################
# Build SAN list (default or from .sni)
#############################################

build_san_entries() {
  TMP_SAN="$1"

  if [[ -f ".sni" ]]; then
    echo "=== Using .sni file for SAN entries ==="
    > "$TMP_SAN"

    DNS_IDX=1
    IP_IDX=1

    while IFS= read -r line; do
      line="${line%%#*}"   # strip comments
      line="$(echo "$line" | xargs)" # trim

      [[ -z "$line" ]] && continue

      # IP address?
      if [[ "$line" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
        echo "IP.$IP_IDX = $line" >> "$TMP_SAN"
        IP_IDX=$((IP_IDX+1))
      elif [[ "$line" =~ : ]]; then
        echo "IP.$IP_IDX = $line" >> "$TMP_SAN"
        IP_IDX=$((IP_IDX+1))
      else
        echo "DNS.$DNS_IDX = $line" >> "$TMP_SAN"
        DNS_IDX=$((DNS_IDX+1))
      fi
    done < .sni

  else
    echo "=== No .sni file found. Using built-in SAN defaults ==="
    cat > "$TMP_SAN" <<EOF
DNS.1 = *.test.com
DNS.2 = *.wushilin.net
DNS.3 = *.abc.com
DNS.4 = localhost
IP.1  = 127.0.0.1
IP.2  = ::1
EOF
  fi
}

#############################################
# CA
#############################################
if regenerate_pair_if_needed "ca.key" "ca.pem" "CA"; then
  rm -f ca.key ca.pem
  openssl genrsa -out ca.key 4096
  openssl req -x509 -new -nodes -key ca.key -sha256 -days $DAYS \
    -subj "$CA_SUBJ" -out ca.pem
fi

#############################################
# Server
#############################################
if regenerate_pair_if_needed "server.key" "server.pem" "Server certificate"; then
  rm -f server.key server.pem server.csr

  TMP_ALT=$(mktemp)
  build_san_entries "$TMP_ALT"

  SERVER_SAN=$(mktemp)
  cat > "$SERVER_SAN" <<EOF
[ req ]
default_bits       = 4096
distinguished_name = req_distinguished_name
req_extensions     = v3_req
prompt             = no

[ req_distinguished_name ]
C  = US
ST = California
L  = San Francisco
O  = ExampleOrg
OU = Server
CN = server

[ v3_req ]
keyUsage = critical, digitalSignature, keyEncipherment
extendedKeyUsage = serverAuth, clientAuth
subjectAltName = @alt_names

[ alt_names ]
EOF

  cat "$TMP_ALT" >> "$SERVER_SAN"

  echo "Generating server key and CSR..."
  openssl genrsa -out server.key 4096
  openssl req -new -key server.key -out server.csr -config "$SERVER_SAN"

  echo "Signing server certificate..."
  openssl x509 -req -in server.csr -CA ca.pem -CAkey ca.key -CAcreateserial \
    -out server.pem -days $DAYS -sha256 -extensions v3_req -extfile "$SERVER_SAN"
fi

#############################################
# Client
#############################################
if regenerate_pair_if_needed "client.key" "client.pem" "Client certificate"; then
  rm -f client.key client.pem client.csr

  CLIENT_EXT=$(mktemp)
  cat > "$CLIENT_EXT" <<EOF
[ v3_req ]
keyUsage = critical, digitalSignature, keyEncipherment
extendedKeyUsage = clientAuth, serverAuth
EOF

  echo "Generating client key and CSR..."
  openssl genrsa -out client.key 4096
  openssl req -new -key client.key -out client.csr -subj "$CLIENT_SUBJ" -sha256

  echo "Signing client certificate..."
  openssl x509 -req -in client.csr -CA ca.pem -CAkey ca.key -CAserial ca.srl \
    -out client.pem -days $DAYS -sha256 -extensions v3_req -extfile "$CLIENT_EXT"
fi

echo "=== Done. Current certificate files ==="
ls -1 ca.* server.* client.*
