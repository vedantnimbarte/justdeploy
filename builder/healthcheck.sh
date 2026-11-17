#!/bin/bash
# Waits until JD_HOST:JD_PORT accepts TCP connections, or until JD_PATH answers
# with a 2xx/3xx when set. Runs on the service's project network.
set -u
timeout_s="${JD_TIMEOUT:-120}"
deadline=$(($(date +%s) + timeout_s))

while [ "$(date +%s)" -lt "$deadline" ]; do
  if [ -n "${JD_PATH:-}" ]; then
    if curl -fsS -o /dev/null --max-time 3 "http://$JD_HOST:$JD_PORT$JD_PATH" 2>/dev/null; then
      echo "Health check passed: GET $JD_PATH"
      exit 0
    fi
  elif timeout 2 bash -c "</dev/tcp/$JD_HOST/$JD_PORT" 2>/dev/null; then
    echo "Port $JD_PORT is accepting connections"
    exit 0
  fi
  sleep 2
done

echo "No answer on port $JD_PORT${JD_PATH:+ at $JD_PATH} after ${timeout_s}s"
exit 1
