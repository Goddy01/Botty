#!/bin/bash
set -euo pipefail
KEY="/c/Users/USER/Desktop/Projects/CaptGBot/captg"
ssh -i "$KEY" -o IdentitiesOnly=yes root@187.127.92.76 bash -s <<'EOF'
tmux has-session -t copybot
tmux pipe-pane -t copybot 2>/dev/null || true
: > /tmp/copybot.follow
tmux capture-pane -t copybot -p -S -150 >> /tmp/copybot.follow
tmux pipe-pane -t copybot -o 'cat >> /tmp/copybot.follow'
echo '--- live copybot logs ---'
tail -F /tmp/copybot.follow
EOF
