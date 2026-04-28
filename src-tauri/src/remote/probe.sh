echo "OS=$(uname -s 2>/dev/null || echo unknown)"
echo "ARCH=$(uname -m 2>/dev/null || echo unknown)"
echo "HAS_CURL=$(command -v curl >/dev/null 2>&1 && echo yes || echo no)"
echo "HAS_WGET=$(command -v wget >/dev/null 2>&1 && echo yes || echo no)"
echo "HAS_TAR=$(command -v tar >/dev/null 2>&1 && echo yes || echo no)"
if command -v sha256sum >/dev/null 2>&1; then
  echo "HAS_SHA256=yes"
elif command -v shasum >/dev/null 2>&1; then
  echo "HAS_SHA256=yes"
else
  echo "HAS_SHA256=no"
fi
echo "HOME_DIR=${HOME}"
DAEMON_DIR="${HOME}/.codeg-remote"
echo "DAEMON_DIR=${DAEMON_DIR}"
if [ -d "${DAEMON_DIR}" ]; then
  echo "DAEMON_EXISTS=yes"
  echo "DAEMON_VERSIONS=$(ls -1 "${DAEMON_DIR}" 2>/dev/null | grep -E '^[0-9]' | tr '\n' ',' | sed 's/,$//')"
else
  echo "DAEMON_EXISTS=no"
fi
