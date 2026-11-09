#!/bin/sh
# Se managed agent hooks v1
endpoint='/se-hooks-legacy-fixture/home '\''with spaces/.se-manager/agent-hooks/endpoint.env'
[ -r "$endpoint" ] && . "$endpoint"
[ -n "${SE_AGENT_HOOK_PORT:-}" ] && [ -n "${SE_AGENT_HOOK_TOKEN:-}" ] && [ -n "${SE_TERMINAL_ID:-}" ] || exit 0
case "$SE_AGENT_HOOK_PORT" in *[!0-9]*|'') exit 0;; esac
[ "$SE_AGENT_HOOK_PORT" -ge 1 ] && [ "$SE_AGENT_HOOK_PORT" -le 65535 ] || exit 0
printf '%s' "${QIN_CODE_HOOK_PAYLOAD:-}" | curl --silent --show-error --connect-timeout 1 --max-time 2 -o /dev/null \
  -H "Authorization: Bearer $SE_AGENT_HOOK_TOKEN" -H 'Content-Type: application/json' \
  -H "x-se-terminal-id: $SE_TERMINAL_ID" -H "x-se-project-id: ${SE_PROJECT_ID:-}" \
  -H "x-se-hook-event: ${QIN_CODE_HOOK_EVENT:-}" -H "x-se-session-id: ${QIN_CODE_HOOK_SESSION_ID:-}" \
  --data-binary @- "http://127.0.0.1:$SE_AGENT_HOOK_PORT/agent-hooks/qin-code" 2>/dev/null || :
exit 0
