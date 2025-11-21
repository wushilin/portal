#!/usr/bin/env bash
set -e

if [[ $# -ne 1 ]]; then
  echo "Usage: $0 <client-common-name>"
  exit 1
fi

CN="$1"

# Required CA files
CA_KEY="ca.key"
CA_CERT="ca.pem"

if [[ ! -f "$CA_KEY" || ! -f "$CA_CERT" ]]; then
  echo "ERROR: Missing CA key or certificate."
  echo "       Expected files: $CA_KEY and $CA_CERT"
  echo "       Run gen_certs.sh first."
  exit 1
fi

KEY_FILE="client-${CN}.key"
CSR_FILE="client-${CN}.csr"
CRT_FILE="client-${CN}.pem"

if [[ -f "$KEY_FILE" || -f "$CSR_FILE" || -f "$CRT_FILE" ]]; then
  echo "ERROR: One or more output files already exist:"
  echo "  $KEY_FILE"
  echo "  $CSR_FILE"
  echo "  $CRT_FILE"
  echo "Refusing to overwrite."
  exit 1
fi

echo "Generating client key..."
openssl genrsa -out "$KEY_FILE" 4096

# temporary OpenSSL config for CSR
CFG=$(mktemp)
cat > "$CFG" <<EOF
[ req ]
prompt = no
distinguished_name = dn
req_extensions = v3_req

[ dn ]
C  = US
ST = California
L  = San Francisco
O  = ExampleOrg
OU = Client
CN = $CN

[ v3_req ]
keyUsage = digitalSignature, keyEncipherment
extendedKeyUsage = clientAuth
EOF

echo "Generating CSR..."
openssl req -new -key "$KEY_FILE" -out "$CSR_FILE" -config "$CFG"

echo "Signing client certificate..."
openssl x509 -req -in "$CSR_FILE" -CA "$CA_CERT" -CAkey "$CA_KEY" \
  -CAserial ca.srl -out "$CRT_FILE" -days 7300 -sha256 \
  -extensions v3_req -extfile "$CFG"

rm -f "$CFG"

echo "Client certificate generated:"
echo "  $KEY_FILE"
echo "  $CSR_FILE"
echo "  $CRT_FILE"
