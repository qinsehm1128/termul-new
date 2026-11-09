// Se managed agent hooks v1
import { spawn } from "node:child_process";
export default function (pi: any) {
  const events = ["session_start", "agent_start", "agent_end", "tool_execution_start", "tool_execution_end", "tool_approval_requested", "tool_approval_resolved", "ui_prompt_start", "ui_prompt_end", "session_shutdown"];
  for (const name of events) pi.on(name, (event: any, ctx: any) => {
    if (!process.env.SE_TERMINAL_ID) return;
    try {
      const payload = JSON.stringify({ event: name, session_id: ctx?.sessionManager?.getSessionId?.(), session_file: ctx?.sessionManager?.getSessionFile?.(), tool_name: event.toolName, ok: !event.isError });
      if (Buffer.byteLength(payload) > 1048576) return;
      const child = spawn("/se-hooks-legacy-fixture/home 'with spaces/.se-manager/agent-hooks/pi-hook.sh", [], { stdio: ["pipe", "ignore", "ignore"] });
      child.on("error", () => {});
      child.stdin.on("error", () => {});
      child.stdin.end(payload);
      child.unref();
    } catch { /* Observers must never alter tool decisions or interrupt pi. */ }
  });
}
