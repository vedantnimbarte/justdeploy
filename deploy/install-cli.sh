#!/bin/sh
# Installs jd, the justdeploy CLI, from the latest release:
#   curl -fsSL https://raw.githubusercontent.com/vedantnimbarte/justdeploy/main/deploy/install-cli.sh | sh
# JD_INSTALL_DIR picks where it goes (default /usr/local/bin). On Windows,
# download jd-windows-x86_64.exe from the release page instead.
set -eu

repo=vedantnimbarte/justdeploy
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) file=jd-linux-x86_64 ;;
  Linux-aarch64 | Linux-arm64) file=jd-linux-arm64 ;;
  Darwin-arm64) file=jd-macos-arm64 ;;
  Darwin-x86_64) file=jd-macos-x86_64 ;;
  *)
    echo "There is no jd build for $(uname -s) $(uname -m)." >&2
    echo "Build it with: cargo install --git https://github.com/$repo justdeploy-cli" >&2
    exit 1
    ;;
esac

dir="${JD_INSTALL_DIR:-/usr/local/bin}"
tmp=$(mktemp)
curl -fsSL "https://github.com/$repo/releases/latest/download/$file" -o "$tmp"
chmod +x "$tmp"
if [ -w "$dir" ]; then
  mv "$tmp" "$dir/jd"
else
  sudo mv "$tmp" "$dir/jd"
fi
echo "Installed $dir/jd. Next: jd login https://your-justdeploy-server <api-token>"
