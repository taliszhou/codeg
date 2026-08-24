#!/bin/sh
# mobilega/codeg-server startup (standalone, loopback + python proxy for LAN)
# Must run with a FULL PATH so spawned ACP agents can find their CLIs.
export PATH="/opt/homebrew/bin:/opt/homebrew/sbin:/Users/taliszhou/.local/bin:/Users/taliszhou/.cargo/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export CODEG_HOST=127.0.0.1
export CODEG_PORT=8099
export CODEG_TOKEN="Qwqwqaz@123"
export CODEG_STATIC_DIR=/Users/taliszhou/code/src/github.com/codeg/web/out
export CODEG_MCP_BIN=/Users/taliszhou/code/src/github.com/codeg/src-tauri/target/release/codeg-mcp
cd /Users/taliszhou/code/src/github.com/codeg
nohup ./src-tauri/target/release/codeg-server > /tmp/server-debug.log 2>&1 &
echo "codeg-server pid=$!"
# python LAN reverse-proxy (Apple-signed, allowed by App Firewall)
sleep 2
nohup python3 /tmp/ga_proxy.py > /tmp/ga_proxy.log 2>&1 &
echo "proxy pid=$!"
