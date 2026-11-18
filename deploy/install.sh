#!/bin/sh
# Installs justdeploy on a Linux server that already runs Docker.
#
#   curl -fsSL https://raw.githubusercontent.com/vedantnimbarte/justdeploy/main/deploy/install.sh | sh
#
# Optional, set before running:
#   JD_BASE_DOMAIN       domain whose wildcard (*.domain) points here; default <ip>.sslip.io
#   JD_ACME_EMAIL        turns on HTTPS with Let's Encrypt
#   JD_DASHBOARD_DOMAIN  serve the dashboard at this hostname over HTTPS (needs JD_ACME_EMAIL)
#   JD_DIR               install directory; default /opt/justdeploy
set -eu

DIR="${JD_DIR:-/opt/justdeploy}"
RAW="https://raw.githubusercontent.com/vedantnimbarte/justdeploy/main"

if ! command -v docker >/dev/null 2>&1; then
  echo "Docker is required: https://docs.docker.com/engine/install/" >&2
  exit 1
fi
if ! docker compose version >/dev/null 2>&1; then
  echo "Docker Compose v2 is required (the 'docker compose' plugin)." >&2
  exit 1
fi

mkdir -p "$DIR"
cd "$DIR"
curl -fsSL "$RAW/docker-compose.yml" -o docker-compose.yml
curl -fsSL "$RAW/docker-compose.tls.yml" -o docker-compose.tls.yml

ip=$(hostname -I 2>/dev/null | awk '{print $1}')
ip="${ip:-127.0.0.1}"

secret() {
  head -c "$1" /dev/urandom | base64 | tr -d '\n'
}

if [ ! -f .env ]; then
  domain="${JD_BASE_DOMAIN:-$ip.sslip.io}"
  if [ -n "${JD_DASHBOARD_DOMAIN:-}" ]; then
    public_url="https://$JD_DASHBOARD_DOMAIN"
  else
    public_url="http://$ip:8080"
  fi
  umask 077
  cat >.env <<EOF
# Keep this file safe: JD_MASTER_KEY decrypts every stored secret.
JD_MASTER_KEY=$(secret 32)
JD_DB_PASSWORD=$(secret 24 | tr -dc 'A-Za-z0-9')
JD_BASE_DOMAIN=$domain
JD_PUBLIC_URL=$public_url
JD_ACME_EMAIL=${JD_ACME_EMAIL:-}
JD_DASHBOARD_DOMAIN=${JD_DASHBOARD_DOMAIN:-}
EOF
  echo "Wrote $DIR/.env"
else
  echo "Keeping the existing $DIR/.env"
fi

# Every install is a Docker Swarm, so more servers can join later. On a server
# with several addresses, other servers reach this one on $ip.
if [ "$(docker info --format '{{.Swarm.LocalNodeState}}')" != "active" ]; then
  docker swarm init --advertise-addr "$ip" >/dev/null
  echo "Started a Docker Swarm on this server ($ip)"
fi

files="-f docker-compose.yml"
if grep -q '^JD_ACME_EMAIL=.' .env; then
  files="$files -f docker-compose.tls.yml"
fi

# shellcheck disable=SC2086
docker compose $files pull
# shellcheck disable=SC2086
docker compose $files up -d

url=$(grep '^JD_PUBLIC_URL=' .env | cut -d= -f2-)
echo
echo "justdeploy is running. Open $url to create the owner account."
echo "Upgrade later with: cd $DIR && docker compose $files pull && docker compose $files up -d"
