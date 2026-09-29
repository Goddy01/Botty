#!/usr/bin/env bash
# Rebuild, restart tmux session `copybot`, wait for Telegram menu.
# On the VPS:  ./redeploy.sh
# From the PC: ./redeploy.sh push   (copies sources, then runs the VPS path)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
SESSION="${TMUX_SESSION:-copybot}"
OK_RE="${CONFIRM_RE:-telegram poll started}"
HOST="${COPYBOT_HOST:-root@187.127.92.76}"
REMOTE="${COPYBOT_REMOTE:-/root/copybot}"
KEY="${COPYBOT_KEY:-}"

if [ -z "$KEY" ]; then
  if [ -f "$ROOT/../captg" ]; then
    KEY="$ROOT/../captg"
  elif [ -f "$ROOT/captg" ]; then
    KEY="$ROOT/captg"
  fi
fi

ssh_cmd() {
  if [ -n "$KEY" ] && [ -f "$KEY" ]; then
    ssh -i "$KEY" -o IdentitiesOnly=yes "$@"
  else
    ssh "$@"
  fi
}

scp_cmd() {
  if [ -n "$KEY" ] && [ -f "$KEY" ]; then
    scp -i "$KEY" -o IdentitiesOnly=yes "$@"
  else
    scp "$@"
  fi
}

uname_s="$(uname -s 2>/dev/null || echo unknown)"
push=0
case "${1:-}" in
  push) push=1 ;;
  local|here) push=0 ;;
  "")
    case "$uname_s" in
      MINGW*|MSYS*|CYGWIN*|Darwin) push=1 ;;
      *)
        if [ ! -d /root/copybot ]; then
          push=1
        fi
        ;;
    esac
    ;;
  *)
    echo "usage: $0 [push|local]" >&2
    exit 2
    ;;
esac

if [ "$push" -eq 1 ]; then
  echo "==> sync $HOST:$REMOTE"
  scp_cmd \
    "$ROOT/Cargo.toml" \
    "$ROOT/config.toml" \
    "$ROOT/run.sh" \
    "$ROOT/redeploy.sh" \
    "$HOST:$REMOTE/"
  scp_cmd -r "$ROOT/src" "$HOST:$REMOTE/"
  ssh_cmd "$HOST" "bash -lc 'chmod +x $REMOTE/redeploy.sh $REMOTE/run.sh && $REMOTE/redeploy.sh local'"
  exit $?
fi

cd "$ROOT"

if [ -n "${TMUX:-}" ]; then
  current="$(tmux display-message -p '#S' 2>/dev/null || true)"
  if [ "$current" = "$SESSION" ]; then
    echo "you are inside tmux session $SESSION — detach (Ctrl+B D) and run ./redeploy.sh from a normal shell" >&2
    exit 1
  fi
fi

# Non-interactive SSH has no rustup PATH. Login shells usually do.
if [ -f "$HOME/.cargo/env" ]; then
  # shellcheck disable=SC1091
  . "$HOME/.cargo/env"
fi
export PATH="$HOME/.cargo/bin:/usr/local/cargo/bin:$PATH"
if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found (PATH=$PATH). On the VPS: source \$HOME/.cargo/env" >&2
  exit 1
fi

echo "==> cargo build --release ($(command -v cargo))"
cargo build --release

echo "==> stop $SESSION"
tmux kill-session -t "$SESSION" 2>/dev/null || true
pkill -f 'target/release/copybot' 2>/dev/null || true
sleep 1

echo "==> start"
chmod +x "$ROOT/run.sh"
tmux new-session -d -s "$SESSION" -n watch
tmux send-keys -t "$SESSION" "cd '$ROOT' && ./run.sh" C-m

echo "==> wait for: $OK_RE"
deadline=$((SECONDS + 45))
while [ "$SECONDS" -lt "$deadline" ]; do
  pane="$(tmux capture-pane -t "$SESSION" -p 2>/dev/null || true)"
  if printf '%s\n' "$pane" | grep -q "$OK_RE"; then
    echo "==> ok"
    printf '%s\n' "$pane" | tail -n 24
    exit 0
  fi
  if printf '%s\n' "$pane" | grep -qiE '^Error:'; then
    echo "==> failed"
    printf '%s\n' "$pane"
    exit 1
  fi
  sleep 1
done

echo "==> timeout — last pane:" >&2
tmux capture-pane -t "$SESSION" -p || true
exit 1
